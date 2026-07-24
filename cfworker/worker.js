// Freebuff2API — Cloudflare Worker SaaS
// Single-file: auth, credentials, API keys, OpenAI/Claude proxy, dashboard

// ─── Constants ───────────────────────────────────────────────────────────────
const VERSION = '1.0.0';
const UPSTREAM_BASE = 'https://www.codebuff.com';
const UA = 'ai-sdk/openai-compatible/1.0.25/codebuff';
const MODEL_REFRESH_HOURS = 6;
const SESSION_TTL_HOURS = 24;
const CREDENTIAL_COOLDOWN_MIN = 30;
const DEFAULT_RATE_LIMIT = 60; // requests per minute
const TOKEN_COUNT_MODE = 'estimate'; // 'estimate' until js-tiktoken bundled

const HARDCODED_MODELS = {
  'base2-free': ['minimax/minimax-m2.7', 'z-ai/glm-5.1'],
  'file-picker': ['google/gemini-2.5-flash-lite'],
  'file-picker-max': ['google/gemini-3.1-flash-lite-preview'],
  'file-lister': ['google/gemini-3.1-flash-lite-preview'],
  'researcher-web': ['google/gemini-3.1-flash-lite-preview'],
  'researcher-docs': ['google/gemini-3.1-flash-lite-preview'],
  'basher': ['google/gemini-3.1-flash-lite-preview'],
  'editor-lite': ['minimax/minimax-m2.7', 'z-ai/glm-5.1'],
  'code-reviewer-lite': ['minimax/minimax-m2.7', 'z-ai/glm-5.1'],
};

const MODEL_URL = 'https://raw.githubusercontent.com/CodebuffAI/codebuff/main/common/src/constants/free-agents.ts';

// ─── Crypto Helpers ──────────────────────────────────────────────────────────
async function hashPassword(password, salt) {
  const enc = new TextEncoder();
  const keyMaterial = await crypto.subtle.importKey(
    'raw', enc.encode(password), 'PBKDF2', false, ['deriveBits']
  );
  const bits = await crypto.subtle.deriveBits(
    { name: 'PBKDF2', salt: enc.encode(salt), iterations: 100000, hash: 'SHA-256' },
    keyMaterial, 256
  );
  return bufToHex(new Uint8Array(bits));
}

async function encryptToken(plaintext, key) {
  const enc = new TextEncoder();
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const keyBytes = hexToBuf(key);
  const cryptoKey = await crypto.subtle.importKey(
    'raw', keyBytes, 'AES-GCM', false, ['encrypt']
  );
  const ciphertext = await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv }, cryptoKey, enc.encode(plaintext)
  );
  return { ciphertext: bufToHex(new Uint8Array(ciphertext)), iv: bufToHex(iv) };
}

async function decryptToken(ciphertextHex, ivHex, key) {
  const keyBytes = hexToBuf(key);
  const cryptoKey = await crypto.subtle.importKey(
    'raw', keyBytes, 'AES-GCM', false, ['decrypt']
  );
  const ct = hexToBuf(ciphertextHex);
  const iv = hexToBuf(ivHex);
  const pt = await crypto.subtle.decrypt({ name: 'AES-GCM', iv }, cryptoKey, ct);
  return new TextDecoder().decode(pt);
}

function bufToHex(buf) {
  return [...buf].map(b => b.toString(16).padStart(2, '0')).join('');
}

function hexToBuf(hex) {
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < hex.length; i += 2) bytes[i / 2] = parseInt(hex.substr(i, 2), 16);
  return bytes;
}

function randomId(len = 32) {
  return bufToHex(crypto.getRandomValues(new Uint8Array(len / 2)));
}

function sha256hex(str) {
  return crypto.subtle.digest('SHA-256', new TextEncoder().encode(str))
    .then(buf => bufToHex(new Uint8Array(buf)));
}

// ─── Auth Middleware ──────────────────────────────────────────────────────────
function parseCookies(req) {
  const h = req.headers.get('cookie') || '';
  const c = {};
  h.split(';').forEach(p => {
    const [k, v] = p.trim().split('=');
    if (k) c[k] = decodeURIComponent(v || '');
  });
  return c;
}

async function getUserFromSession(req, env) {
  const cookies = parseCookies(req);
  const sid = cookies.f2db_sid;
  if (!sid) return null;
  const hash = await sha256hex(sid);
  const row = await env.DB.prepare(
    `SELECT s.*, u.id as uid, u.email FROM sessions s JOIN users u ON s.user_id = u.id
     WHERE s.id = ? AND s.expires_at > ?`
  ).bind(hash, Date.now()).first();
  return row ? { id: row.uid, email: row.email } : null;
}

async function getUserFromApiKey(req, env) {
  const auth = req.headers.get('authorization') || '';
  const xkey = req.headers.get('x-api-key') || '';
  const rawKey = auth.startsWith('Bearer ') ? auth.slice(7) : (auth || xkey);
  if (!rawKey || !rawKey.startsWith('f2db-')) return null;
  const hash = await sha256hex(rawKey);
  const row = await env.DB.prepare(
    `SELECT ak.*, u.id as uid, u.email FROM api_keys ak JOIN users u ON ak.user_id = u.id
     WHERE ak.key_hash = ? AND ak.is_active = 1`
  ).bind(hash).first();
  return row ? { id: row.uid, email: row.email, apiKeyId: row.id, rateLimit: row.rate_limit } : null;
}

async function audit(env, userId, action, ip, details = '') {
  try {
    await env.DB.prepare(
      'INSERT INTO audit_events (user_id, action, ip, details, created_at) VALUES (?,?,?,?,?)'
    ).bind(userId || '', action, ip || '', details, Date.now()).run();
  } catch (e) { /* audit failure is non-fatal */ }
}

// ─── Rate Limiting (in-memory per isolate, good enough for CF Workers) ──────
const rateLimitMap = new Map();

function checkRateLimit(key, limit, windowMs = 60000) {
  const now = Date.now();
  let entry = rateLimitMap.get(key);
  if (!entry || entry.windowStart + windowMs < now) {
    entry = { windowStart: now, count: 0 };
    rateLimitMap.set(key, entry);
  }
  entry.count++;
  return entry.count <= limit;
}

// Periodically clean up rate limit map to prevent memory leaks
function cleanupRateLimitMap() {
  const now = Date.now();
  for (const [key, entry] of rateLimitMap) {
    if (entry.windowStart + 120000 < now) rateLimitMap.delete(key);
  }
}

// ─── Model Registry ──────────────────────────────────────────────────────────
async function getModels(env) {
  const cached = await env.KV.get('models', 'json');
  if (cached && cached.timestamp && (Date.now() - cached.timestamp < MODEL_REFRESH_HOURS * 3600000)) {
    return cached.data;
  }
  try {
    const resp = await fetch(MODEL_URL, { headers: { 'User-Agent': UA } });
    if (resp.ok) {
      const text = await resp.text();
      const models = parseFreeAgentsTs(text);
      if (models && Object.keys(models).length > 0) {
        await env.KV.put('models', JSON.stringify({ data: models, timestamp: Date.now() }));
        return models;
      }
    }
  } catch (e) { /* fall through */ }
  return HARDCODED_MODELS;
}

function parseFreeAgentsTs(text) {
  const models = {};
  // Match patterns like: agentId: ['model1', 'model2']
  const lines = text.split('\n');
  for (const line of lines) {
    const m = line.match(/['"]?([a-z][\w-]+)['"]?\s*:\s*\[\s*(['"][^'"]+['"](?:\s*,\s*['"][^'"]+['"])*)\s*\]/);
    if (m) {
      const agentId = m[1];
      const modelList = m[2].match(/['"]([^'"]+)['"]/g).map(s => s.replace(/['"]/g, ''));
      if (modelList.length > 0) models[agentId] = modelList;
    }
  }
  return Object.keys(models).length > 0 ? models : null;
}

function agentForModel(models, requestedModel) {
  // Direct match: "google/gemini-2.5-flash-lite"
  for (const [agent, ms] of Object.entries(models)) {
    if (ms.includes(requestedModel)) return agent;
  }
  // Partial match: "gemini-2.5-flash-lite" or "gemini"
  const lower = requestedModel.toLowerCase();
  for (const [agent, ms] of Object.entries(models)) {
    for (const m of ms) {
      if (m.toLowerCase().includes(lower) || lower.includes(m.toLowerCase().split('/').pop())) return agent;
    }
  }
  return null;
}

// ─── Token Counting (estimate) ───────────────────────────────────────────────
function estimateTokens(text) {
  if (!text) return 0;
  return Math.ceil(text.length / 4);
}

function countPayloadTokens(payload) {
  let total = 0;
  if (payload.messages) {
    for (const msg of payload.messages) {
      const content = msg.content;
      if (typeof content === 'string') total += estimateTokens(content);
      else if (Array.isArray(content)) {
        for (const part of content) {
          if (typeof part === 'string') total += estimateTokens(part);
          else if (part.text) total += estimateTokens(part.text);
        }
      }
    }
  }
  if (payload.system) total += estimateTokens(typeof payload.system === 'string' ? payload.system : JSON.stringify(payload.system));
  if (payload.tools) total += estimateTokens(JSON.stringify(payload.tools));
  return total;
}

// ─── Upstream Proxy ──────────────────────────────────────────────────────────
async function upstreamRequest(token, path, body, signal) {
  const url = `${UPSTREAM_BASE}${path}`;
  const opts = {
    method: body ? 'POST' : 'GET',
    signal,
    headers: {
      'Authorization': `Bearer ${token}`,
      'Content-Type': 'application/json',
      'Accept': 'application/json, text/event-stream',
      'User-Agent': UA,
      'Origin': UPSTREAM_BASE,
      'Referer': `${UPSTREAM_BASE}/`,
    },
  };
  if (body) opts.body = JSON.stringify(body);
  return fetch(url, opts);
}

async function startRun(token, agentId, signal) {
  const resp = await upstreamRequest(token, '/api/v1/agent-runs', {
    action: 'START', agentId
  }, signal);
  if (!resp.ok) return { error: `start run ${resp.status}` };
  const data = await resp.json();
  return { runId: data.runId || data.id };
}

async function finishRun(token, runId, signal) {
  try {
    await upstreamRequest(token, '/api/v1/agent-runs', {
      action: 'FINISH', runId, status: 'completed', totalSteps: 0, directCredits: 0, totalCredits: 0
    }, signal);
  } catch (e) { /* best effort */ }
}

async function ensureSession(token, agentId, signal) {
  const resp = await upstreamRequest(token, '/api/v1/freebuff/session', {
    action: 'CREATE', agentId
  }, signal);
  if (!resp.ok) {
    if (resp.status === 429 || resp.status === 503) {
      return { queued: true, retryAfter: parseInt(resp.headers.get('retry-after') || '5') };
    }
    return { error: `session ${resp.status}` };
  }
  const data = await resp.json();
  if (data.status === 'queued') return { queued: true, retryAfter: data.retryAfter || 5 };
  return { instanceId: data.instanceId || data.id };
}

// ─── Durable Object: UserStateDO ─────────────────────────────────────────────
// Manages per-user run lifecycle, session state, and credential cooldown
export class UserStateDO {
  constructor(state, env) {
    this.state = state;
    this.env = env;
    this.runs = {};       // agentId -> { runId, token, startedAt }
    this.sessions = {};   // agentId -> { instanceId, expiresAt }
    this.cooldowns = {};  // credentialId -> cooldownUntil
  }

  async fetch(request) {
    const url = new URL(request.url);
    const action = url.pathname.slice(1);

    if (action === 'acquire') {
      return this.handleAcquire(request);
    } else if (action === 'release') {
      return this.handleRelease(request);
    } else if (action === 'status') {
      return Response.json({ runs: Object.keys(this.runs), sessions: Object.keys(this.sessions) });
    } else if (action === 'cooldown') {
      return this.handleCooldown(request);
    }
    return Response.json({ error: 'unknown action' }, { status: 400 });
  }

  async handleAcquire(request) {
    const { agentId, credentialId, token } = await request.json();
    const now = Date.now();

    // Check credential cooldown
    const cd = this.cooldowns[credentialId];
    if (cd && cd > now) {
      return Response.json({ error: 'credential_cooled', retryAfter: Math.ceil((cd - now) / 1000) }, { status: 429 });
    }

    // Check if we have an active run for this agent
    let run = this.runs[agentId];
    const ROTATION_MS = 6 * 3600 * 1000; // 6 hours

    if (!run || (now - run.startedAt > ROTATION_MS)) {
      // Need to start a new run
      if (run) await finishRun(token, run.runId);
      const result = await startRun(token, agentId);
      if (result.error) return Response.json({ error: result.error }, { status: 502 });
      run = { runId: result.runId, token, startedAt: now, credentialId };
      this.runs[agentId] = run;
    }

    // Ensure session
    let session = this.sessions[agentId];
    if (!session || (session.expiresAt && session.expiresAt < now)) {
      const sessResult = await ensureSession(token, agentId);
      if (sessResult.queued) {
        return Response.json({ error: 'waiting_room', retryAfter: sessResult.retryAfter }, { status: 503 });
      }
      if (sessResult.error) return Response.json({ error: sessResult.error }, { status: 502 });
      session = { instanceId: sessResult.instanceId, expiresAt: now + 3600000 };
      this.sessions[agentId] = session;
    }

    return Response.json({
      runId: run.runId,
      instanceId: session.instanceId,
      credentialId: run.credentialId || credentialId,
    });
  }

  async handleRelease(request) {
    const { agentId } = await request.json();
    const run = this.runs[agentId];
    if (run) {
      await finishRun(run.token, run.runId);
      delete this.runs[agentId];
      delete this.sessions[agentId];
    }
    return Response.json({ ok: true });
  }

  async handleCooldown(request) {
    const { credentialId, minutes } = await request.json();
    this.cooldowns[credentialId] = Date.now() + minutes * 60000;
    return Response.json({ ok: true });
  }
}

// ─── Route Handlers ──────────────────────────────────────────────────────────

// GET /healthz
function healthzHandler() {
  return Response.json({ status: 'ok', version: VERSION, timestamp: Date.now() });
}

// GET /v1/models (API key or session auth)
async function modelsHandler(req, env, user) {
  const models = await getModels(env);
  const modelList = [];
  for (const [agent, ms] of Object.entries(models)) {
    for (const m of ms) {
      modelList.push({ id: m, object: 'model', created: 1700000000, owned_by: 'freebuff', agent_id: agent });
    }
  }
  return Response.json({ object: 'list', data: modelList });
}

// POST /v1/chat/completions
async function chatCompletionsHandler(req, env, user) {
  const body = await req.json();
  const requestedModel = body.model;
  if (!requestedModel) return apiError('model is required', 400);

  const models = await getModels(env);
  const agentId = agentForModel(models, requestedModel);
  if (!agentId) return apiError(`model '${requestedModel}' not available`, 404);

  // Get active credentials for user
  const creds = await env.DB.prepare(
    'SELECT * FROM credentials WHERE user_id = ? AND is_active = 1 ORDER BY last_used_at ASC'
  ).bind(user.id).all();

  if (!creds.results || creds.results.length === 0) {
    return apiError('No active Freebuff credentials. Add credentials in Dashboard.', 401);
  }

  // Rate limit check
  const rateKey = `api:${user.id}`;
  const limit = user.rateLimit || DEFAULT_RATE_LIMIT;
  if (!checkRateLimit(rateKey, limit)) {
    return apiError('rate limit exceeded', 429);
  }

  // Try each credential
  let lastError = null;
  // Deep-clone body to avoid mutation across retries
  const originalBody = JSON.parse(JSON.stringify(body));
  for (const cred of creds.results) {
    const encryptionKey = env.APP_ENCRYPTION_KEY;
    if (!encryptionKey) return apiError('server misconfigured: encryption key missing', 500);

    const token = await decryptToken(cred.token_encrypted, cred.token_iv, encryptionKey);

    // Use Durable Object for run/session management
    const doId = env.USER_STATE.idFromName(user.id);
    const stub = env.USER_STATE.get(doId);

    const acquireResp = await stub.fetch(new Request('http://do/acquire', {
      method: 'POST',
      body: JSON.stringify({ agentId, credentialId: cred.id, token }),
    }));

    if (acquireResp.status === 429) {
      lastError = 'credential cooled down';
      continue;
    }
    if (acquireResp.status === 503) {
      const data = await acquireResp.json();
      return Response.json(
        { error: { message: 'waiting room active', type: 'waiting_room' } },
        { status: 503, headers: { 'Retry-After': String(data.retryAfter || 5) } }
      );
    }
    if (!acquireResp.ok) {
      lastError = 'failed to acquire run';
      continue;
    }

    const acquireData = await acquireResp.json();
    const runId = acquireData.runId;
    const instanceId = acquireData.instanceId;

    // Build per-attempt payload from clean copy
    const attemptBody = JSON.parse(JSON.stringify(originalBody));
    attemptBody.codebuff_metadata = attemptBody.codebuff_metadata || {};
    attemptBody.codebuff_metadata.run_id = runId;
    attemptBody.codebuff_metadata.cost_mode = 'free';
    attemptBody.codebuff_metadata.client_id = randomId(13);
    if (instanceId) attemptBody.codebuff_metadata.freebuff_instance_id = instanceId;

    // Clean tools schema
    if (attemptBody.tools) attemptBody.tools = cleanToolsSchema(attemptBody.tools);

    // Forward to upstream
    const upstreamResp = await upstreamRequest(token, '/api/v1/chat/completions', attemptBody, req.signal);

    if (upstreamResp.status === 401) {
      // Cooldown this credential
      await stub.fetch(new Request('http://do/cooldown', {
        method: 'POST',
        body: JSON.stringify({ credentialId: cred.id, minutes: CREDENTIAL_COOLDOWN_MIN }),
      }));
      await env.DB.prepare('UPDATE credentials SET cooldown_until = ? WHERE id = ?')
        .bind(Date.now() + CREDENTIAL_COOLDOWN_MIN * 60000, cred.id).run();
      lastError = 'credential expired (401)';
      continue;
    }

    if (!upstreamResp.ok) {
      const errText = await upstreamResp.text().catch(() => '');
      lastError = `upstream ${upstreamResp.status}: ${errText.slice(0, 200)}`;
      continue;
    }

    // Update last_used_at
    await env.DB.prepare('UPDATE credentials SET last_used_at = ? WHERE id = ?')
      .bind(Date.now(), cred.id).run();

    // Record usage
    const tokensIn = countPayloadTokens(attemptBody);
    await env.DB.prepare(
      'INSERT INTO usage_events (user_id, api_key_id, model, tokens_in, tokens_out, status, created_at) VALUES (?,?,?,?,?,?,?)'
    ).bind(user.id, user.apiKeyId || null, requestedModel, tokensIn, 0, 'ok', Date.now()).run();

    // Handle streaming
    const isStream = attemptBody.stream === true;
    if (isStream) {
      return new Response(upstreamResp.body, {
        headers: {
          'Content-Type': 'text/event-stream',
          'Cache-Control': 'no-cache',
          'Connection': 'keep-alive',
        },
      });
    }

    // Non-streaming: return as-is
    const respData = await upstreamResp.json();
    return Response.json(respData);
  }

  return apiError(lastError || 'all credentials failed', 502);
}

// POST /v1/messages (Anthropic Claude compatibility)
async function messagesHandler(req, env, user) {
  const body = await req.json();
  const openaiReq = convertClaudeToOpenAI(body);
  if (!openaiReq) {
    return claudeApiError('failed to convert Claude request to OpenAI format', 400);
  }

  // Create a new request with the converted body
  const newReq = new Request(req.url, {
    method: 'POST',
    headers: req.headers,
    body: JSON.stringify(openaiReq.body),
  });

  const resp = await chatCompletionsHandler(newReq, env, user);

  const isStream = body.stream === true;

  // Handle error responses — convert to Claude error format
  if (!resp.ok) {
    let errBody;
    try {
      errBody = await resp.clone().json();
    } catch (e) {
      errBody = null;
    }
    const msg = errBody?.error?.message || errBody?.error || 'upstream error';
    return claudeApiError(msg, resp.status);
  }

  if (isStream) {
    return convertOpenAIStreamToClaude(resp, body.model || openaiReq.body.model);
  }

  const data = await resp.json();
  return Response.json(convertOpenAIToClaudeResponse(data, body.model || openaiReq.body.model));
}

function claudeApiError(message, status) {
  return Response.json(
    {
      type: 'error',
      error: {
        type: status === 400 ? 'invalid_request_error' : (status === 401 ? 'authentication_error' : 'api_error'),
        message,
      },
    },
    { status }
  );
}

// POST /v1/messages/count_tokens
async function countTokensHandler(req, env, user) {
  const body = await req.json();
  const openaiReq = convertClaudeToOpenAI(body);
  if (!openaiReq) return apiError('failed to convert request', 400);
  const tokens = countPayloadTokens(openaiReq.body);
  return Response.json({ input_tokens: tokens });
}

// ─── Claude ↔ OpenAI Conversion ──────────────────────────────────────────────
function convertClaudeToOpenAI(claudeReq) {
  const body = {
    model: claudeReq.model,
    messages: [],
    stream: claudeReq.stream || false,
  };

  // System message
  if (claudeReq.system) {
    const sysContent = typeof claudeReq.system === 'string'
      ? claudeReq.system
      : (claudeReq.system.text || JSON.stringify(claudeReq.system));
    body.messages.push({ role: 'system', content: sysContent });
  }

  // Convert messages
  for (const msg of (claudeReq.messages || [])) {
    if (msg.role === 'user') {
      body.messages.push({ role: 'user', content: convertClaudeContent(msg.content) });
    } else if (msg.role === 'assistant') {
      const parts = [];
      if (typeof msg.content === 'string') {
        parts.push({ role: 'assistant', content: msg.content });
      } else if (Array.isArray(msg.content)) {
        for (const block of msg.content) {
          if (block.type === 'text') parts.push({ role: 'assistant', content: block.text });
          else if (block.type === 'tool_use') {
            parts.push({
              role: 'assistant',
              content: null,
              tool_calls: [{
                id: block.id,
                type: 'function',
                function: { name: block.name, arguments: JSON.stringify(block.input) }
              }]
            });
          } else if (block.type === 'thinking') {
            // Skip thinking blocks in conversion
          }
        }
      }
      body.messages.push(...parts);
    } else if (msg.role === 'tool') {
      // tool_result
      if (Array.isArray(msg.content)) {
        for (const block of msg.content) {
          if (block.type === 'tool_result') {
            body.messages.push({
              role: 'tool',
              tool_call_id: block.tool_use_id,
              content: typeof block.content === 'string' ? block.content : JSON.stringify(block.content),
            });
          }
        }
      }
    }
  }

  // Parameters
  if (claudeReq.max_tokens) body.max_tokens = claudeReq.max_tokens;
  if (claudeReq.temperature !== undefined) body.temperature = claudeReq.temperature;
  if (claudeReq.top_p !== undefined) body.top_p = claudeReq.top_p;
  if (claudeReq.stop_sequences) body.stop = claudeReq.stop_sequences;

  // Thinking → reasoning_effort
  if (claudeReq.thinking) {
    const budgetTokens = claudeReq.thinking.budget_tokens || 10000;
    body.reasoning_effort = budgetTokens > 5000 ? 'high' : (budgetTokens > 2000 ? 'medium' : 'low');
  }

  // Tools
  if (claudeReq.tools) {
    body.tools = claudeReq.tools.filter(t => t.type !== 'web_search').map(t => ({
      type: 'function',
      function: {
        name: t.name,
        description: t.description,
        parameters: t.input_schema || {},
      },
    }));
  }

  return { body };
}

function convertClaudeContent(content) {
  if (typeof content === 'string') return content;
  if (!Array.isArray(content)) return '';
  const parts = [];
  for (const block of content) {
    if (block.type === 'text') parts.push({ type: 'text', text: block.text });
    else if (block.type === 'image' && block.source) {
      parts.push({
        type: 'image_url',
        image_url: { url: `data:${block.source.media_type};base64,${block.source.data}` }
      });
    }
  }
  return parts.length === 1 && parts[0].type === 'text' ? parts[0].text : parts;
}

function convertOpenAIToClaudeResponse(openaiResp, model, stream) {
  const choice = openaiResp.choices?.[0];
  if (!choice) return { type: 'error', error: { message: 'no response from upstream' } };

  const content = [];
  if (choice.message?.content) {
    content.push({ type: 'text', text: choice.message.content });
  }
  if (choice.message?.tool_calls) {
    for (const tc of choice.message.tool_calls) {
      content.push({
        type: 'tool_use',
        id: tc.id,
        name: tc.function?.name,
        input: JSON.parse(tc.function?.arguments || '{}'),
      });
    }
  }

  return {
    id: `msg_${randomId(24)}`,
    type: 'message',
    role: 'assistant',
    content,
    model: model,
    stop_reason: choice.finish_reason === 'tool_calls' ? 'tool_use' : (choice.finish_reason || 'end_turn'),
    stop_sequence: null,
    usage: {
      input_tokens: openaiResp.usage?.prompt_tokens || 0,
      output_tokens: openaiResp.usage?.completion_tokens || 0,
    },
  };
}

async function convertOpenAIStreamToClaude(openaiResp, model) {
  const reader = openaiResp.body.getReader();
  const decoder = new TextDecoder();
  const encoder = new TextEncoder();
  const msgId = `msg_${randomId(24)}`;

  // Track all content blocks
  const blocks = []; // { type: 'text' | 'tool_use', index: number, id?: string }
  let textBlockStarted = false;
  let inputTokens = 0, outputTokens = 0;

  const stream = new ReadableStream({
    async start(controller) {
      const send = (event, data) => {
        controller.enqueue(encoder.encode(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`));
      };

      // message_start
      send('message_start', {
        type: 'message_start',
        message: {
          id: msgId, type: 'message', role: 'assistant', content: [],
          model, stop_reason: null, stop_sequence: null,
          usage: { input_tokens: 0, output_tokens: 0 },
        },
      });

      let buffer = '';
      let toolCallAccumulators = new Map(); // index -> { id, name, args }

      try {
        while (true) {
          const { done, value } = await reader.read();
          if (done) break;
          buffer += decoder.decode(value, { stream: true });
          const lines = buffer.split('\n');
          buffer = lines.pop() || '';

          for (const line of lines) {
            if (!line.startsWith('data: ')) continue;
            const data = line.slice(6).trim();
            if (data === '[DONE]') continue;
            try {
              const parsed = JSON.parse(data);

              // Handle upstream errors
              if (parsed.error) {
                send('error', {
                  type: 'error',
                  error: {
                    type: 'api_error',
                    message: parsed.error.message || 'upstream error'
                  }
                });
                controller.close();
                return;
              }

              const choice = parsed.choices?.[0];
              if (!choice) continue;

              const delta = choice.delta;
              const finishReason = choice.finish_reason;

              // Text content
              if (delta?.content) {
                if (!textBlockStarted) {
                  send('content_block_start', {
                    type: 'content_block_start', index: 0,
                    content_block: { type: 'text', text: '' },
                  });
                  blocks.push({ type: 'text', index: 0 });
                  textBlockStarted = true;
                }
                send('content_block_delta', {
                  type: 'content_block_delta', index: 0,
                  delta: { type: 'text_delta', text: delta.content },
                });
                outputTokens += estimateTokens(delta.content);
              }

              // Tool calls
              if (delta?.tool_calls) {
                for (const tc of delta.tool_calls) {
                  const tcIndex = tc.index || 0;

                  // New tool call - send content_block_start
                  if (tc.id) {
                    const blockIdx = textBlockStarted ? tcIndex + 1 : tcIndex;
                    send('content_block_start', {
                      type: 'content_block_start',
                      index: blockIdx,
                      content_block: {
                        type: 'tool_use',
                        id: tc.id,
                        name: tc.function?.name || '',
                        input: {}
                      },
                    });
                    blocks.push({ type: 'tool_use', index: blockIdx, id: tc.id });
                    toolCallAccumulators.set(tcIndex, { id: tc.id, args: '' });
                  }

                  // Accumulate arguments
                  if (tc.function?.arguments) {
                    const acc = toolCallAccumulators.get(tcIndex);
                    if (acc) {
                      acc.args += tc.function.arguments;
                      const blockIdx = textBlockStarted ? tcIndex + 1 : tcIndex;
                      send('content_block_delta', {
                        type: 'content_block_delta',
                        index: blockIdx,
                        delta: {
                          type: 'input_json_delta',
                          partial_json: tc.function.arguments
                        },
                      });
                    }
                  }
                }
              }

              // Usage from final chunk
              if (parsed.usage) {
                inputTokens = parsed.usage.prompt_tokens || 0;
                outputTokens = parsed.usage.completion_tokens || outputTokens;
              }
            } catch (e) { /* skip parse errors */ }
          }
        }
      } catch (err) {
        // Stream reading error
        send('error', {
          type: 'error',
          error: {
            type: 'api_error',
            message: err.message || 'stream error'
          }
        });
        controller.close();
        return;
      }

      // Send content_block_stop for all blocks
      for (const block of blocks) {
        send('content_block_stop', { type: 'content_block_stop', index: block.index });
      }

      // Determine stop_reason
      const stopReason = blocks.some(b => b.type === 'tool_use') ? 'tool_use' : 'end_turn';

      // message_delta + message_stop
      send('message_delta', {
        type: 'message_delta',
        delta: { stop_reason: stopReason, stop_sequence: null },
        usage: { output_tokens: outputTokens },
      });
      send('message_stop', { type: 'message_stop' });
      controller.close();
    },
  });

  return new Response(stream, {
    headers: {
      'Content-Type': 'text/event-stream',
      'Cache-Control': 'no-cache',
      'Connection': 'keep-alive',
    },
  });
}

// ─── Tools Schema Cleaner ────────────────────────────────────────────────────
function cleanToolsSchema(tools) {
  if (!Array.isArray(tools)) return tools;
  return tools.map(t => {
    const cleaned = JSON.parse(JSON.stringify(t));
    if (cleaned.function?.parameters) {
      cleanSchema(cleaned.function.parameters);
    }
    return cleaned;
  });
}

function cleanSchema(schema) {
  if (!schema || typeof schema !== 'object') return;
  delete schema.$ref;
  delete schema.$defs;
  delete schema.definitions;
  if (schema.properties) {
    for (const [key, val] of Object.entries(schema.properties)) {
      if (val && typeof val === 'object') {
        // Handle nullable
        if (Array.isArray(val.type) && val.type.includes('null')) {
          val.type = val.type.filter(t => t !== 'null');
          if (val.type.length === 1) val.type = val.type[0];
          else delete val.type;
        }
        // Handle anyOf/oneOf → simplify
        if (val.anyOf && !val.type) {
          const nonNull = val.anyOf.filter(v => v.type !== 'null');
          if (nonNull.length === 1) Object.assign(val, nonNull[0]);
          delete val.anyOf;
        }
        if (val.oneOf && !val.type) {
          const nonNull = val.oneOf.filter(v => v.type !== 'null');
          if (nonNull.length === 1) Object.assign(val, nonNull[0]);
          delete val.oneOf;
        }
        cleanSchema(val);
      }
    }
  }
  if (schema.items) cleanSchema(schema.items);
}

// ─── Dashboard Handlers ──────────────────────────────────────────────────────

async function dashboardHandler(req, env) {
  const user = await getUserFromSession(req, env);
  if (!user) return dashboardLogin(req, env);
  return dashboardMain(req, env, user);
}

async function dashboardLogin(req, env) {
  const url = new URL(req.url);
  const mode = url.searchParams.get('mode') === 'register' ? 'register' : 'login';

  if (req.method === 'POST') {
    const form = await req.formData();
    const email = (form.get('email') || '').toString().trim().toLowerCase();
    const password = (form.get('password') || '').toString();
    const action = form.get('action') || 'login';

    if (action === 'register') {
      // Registration
      if (!email || !password || password.length < 8) {
        return dashboardPage('register', '密码至少8位', email);
      }
      const existing = await env.DB.prepare('SELECT id FROM users WHERE email = ?').bind(email).first();
      if (existing) return dashboardPage('register', '邮箱已注册', email);

      const id = randomId();
      const salt = randomId(16);
      const hash = await hashPassword(password, salt);
      await env.DB.prepare(
        'INSERT INTO users (id, email, password_hash, salt, created_at, updated_at) VALUES (?,?,?,?,?,?)'
      ).bind(id, email, hash, salt, Date.now(), Date.now()).run();
      await audit(env, id, 'register', req.headers.get('cf-connecting-ip'));

      // Auto-login
      return createSessionAndRedirect(env, id, email, req);
    } else {
      // Login
      if (!email || !password) return dashboardPage('login', '请输入邮箱和密码', email);
      const row = await env.DB.prepare('SELECT * FROM users WHERE email = ?').bind(email).first();
      if (!row) return dashboardPage('login', '邮箱或密码错误', email);
      const hash = await hashPassword(password, row.salt);
      if (hash !== row.password_hash) {
        await audit(env, row.id, 'login_failed', req.headers.get('cf-connecting-ip'));
        return dashboardPage('login', '邮箱或密码错误', email);
      }
      await audit(env, row.id, 'login', req.headers.get('cf-connecting-ip'));
      return createSessionAndRedirect(env, row.id, row.email, req);
    }
  }
  return dashboardPage(mode, '', '');
}

async function createSessionAndRedirect(env, userId, email, req) {
  const sid = randomId(48);
  const sidHash = await sha256hex(sid);
  const expires = Date.now() + SESSION_TTL_HOURS * 3600000;
  await env.DB.prepare(
    'INSERT INTO sessions (id, user_id, created_at, expires_at) VALUES (?,?,?,?)'
  ).bind(sidHash, userId, Date.now(), expires).run();

  const cookie = `f2db_sid=${sid}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=${SESSION_TTL_HOURS * 3600}`;
  return new Response(null, {
    status: 302,
    headers: { 'Location': '/', 'Set-Cookie': cookie },
  });
}

async function dashboardMain(req, env, user) {
  const url = new URL(req.url);
  const path = url.pathname;

  // API endpoints for dashboard
  if (path === '/api/credentials' && req.method === 'GET') {
    const creds = await env.DB.prepare(
      'SELECT id, label, is_active, cooldown_until, last_used_at, created_at FROM credentials WHERE user_id = ?'
    ).bind(user.id).all();
    return Response.json(creds.results || []);
  }

  if (path === '/api/credentials' && req.method === 'POST') {
    const body = await req.json();
    const token = (body.token || '').trim();
    const label = (body.label || 'default').trim();
    if (!token) return apiError('token is required', 400);
    if (token.length < 10) return apiError('token too short', 400);

    const encryptionKey = env.APP_ENCRYPTION_KEY;
    if (!encryptionKey) return apiError('server misconfigured: encryption key missing', 500);

    let encrypted;
    try {
      encrypted = await encryptToken(token, encryptionKey);
    } catch (e) {
      return apiError('encryption failed: ' + e.message, 500);
    }

    const { ciphertext, iv } = encrypted;
    const id = randomId();
    try {
      await env.DB.prepare(
        'INSERT INTO credentials (id, user_id, token_encrypted, token_iv, label, is_active, created_at) VALUES (?,?,?,?,?,?,?)'
      ).bind(id, user.id, ciphertext, iv, label, 1, Date.now()).run();
    } catch (e) {
      return apiError('database error: ' + e.message, 500);
    }
    await audit(env, user.id, 'credential_add', req.headers.get('cf-connecting-ip'));
    return Response.json({ id, label, message: 'credential added' });
  }

  if (path.startsWith('/api/credentials/') && req.method === 'DELETE') {
    const credId = path.split('/').pop();
    await env.DB.prepare('DELETE FROM credentials WHERE id = ? AND user_id = ?').bind(credId, user.id).run();
    await audit(env, user.id, 'credential_delete', req.headers.get('cf-connecting-ip'), credId);
    return Response.json({ ok: true });
  }

  if (path === '/api/apikeys' && req.method === 'GET') {
    const keys = await env.DB.prepare(
      'SELECT id, key_prefix, label, rate_limit, created_at, is_active FROM api_keys WHERE user_id = ?'
    ).bind(user.id).all();
    return Response.json(keys.results || []);
  }

  if (path === '/api/apikeys' && req.method === 'POST') {
    const body = await req.json();
    const label = (body.label || 'default').trim();
    const rawKey = `f2db-${randomId(32)}`;
    const keyHash = await sha256hex(rawKey);
    const keyPrefix = rawKey.slice(0, 10) + '...';
    const id = randomId();
    await env.DB.prepare(
      'INSERT INTO api_keys (id, user_id, key_hash, key_prefix, label, rate_limit, created_at, is_active) VALUES (?,?,?,?,?,?,?,?)'
    ).bind(id, user.id, keyHash, keyPrefix, label, DEFAULT_RATE_LIMIT, Date.now(), 1).run();
    await audit(env, user.id, 'apikey_create', req.headers.get('cf-connecting-ip'));
    return Response.json({ id, key: rawKey, label, message: 'save this key now — it will not be shown again' });
  }

  if (path.startsWith('/api/apikeys/') && req.method === 'DELETE') {
    const keyId = path.split('/').pop();
    await env.DB.prepare('DELETE FROM api_keys WHERE id = ? AND user_id = ?').bind(keyId, user.id).run();
    return Response.json({ ok: true });
  }

  if (path === '/api/usage' && req.method === 'GET') {
    const limit = parseInt(url.searchParams.get('limit') || '50');
    const usage = await env.DB.prepare(
      'SELECT * FROM usage_events WHERE user_id = ? ORDER BY created_at DESC LIMIT ?'
    ).bind(user.id, Math.min(limit, 200)).all();
    return Response.json(usage.results || []);
  }

  if (path === '/api/stats' && req.method === 'GET') {
    const stats = await env.DB.prepare(`
      SELECT
        COUNT(*) as total_requests,
        SUM(tokens_in) as total_tokens_in,
        SUM(CASE WHEN status = 'ok' THEN 1 ELSE 0 END) as success_count,
        SUM(CASE WHEN status != 'ok' THEN 1 ELSE 0 END) as error_count
      FROM usage_events WHERE user_id = ?
    `).bind(user.id).first();
    const credCount = await env.DB.prepare('SELECT COUNT(*) as c FROM credentials WHERE user_id = ?').bind(user.id).first();
    const keyCount = await env.DB.prepare('SELECT COUNT(*) as c FROM api_keys WHERE user_id = ? AND is_active = 1').bind(user.id).first();
    return Response.json({
      totalRequests: stats?.total_requests || 0,
      totalTokensIn: stats?.total_tokens_in || 0,
      successCount: stats?.success_count || 0,
      errorCount: stats?.error_count || 0,
      credentials: credCount?.c || 0,
      apiKeys: keyCount?.c || 0,
    });
  }

  if (path === '/api/logout' && req.method === 'POST') {
    const cookies = parseCookies(req);
    const sid = cookies.f2db_sid;
    if (sid) {
      const hash = await sha256hex(sid);
      await env.DB.prepare('DELETE FROM sessions WHERE id = ?').bind(hash).run();
    }
    return new Response(JSON.stringify({ ok: true }), {
      headers: {
        'Content-Type': 'application/json',
        'Set-Cookie': 'f2db_sid=; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=0',
      },
    });
  }

  // Main dashboard HTML
  return new Response(renderDashboardHTML(user), {
    headers: { 'Content-Type': 'text/html; charset=utf-8' },
  });
}

// ─── HTML Rendering ──────────────────────────────────────────────────────────

function dashboardPage(mode, error, email) {
  const isRegister = mode === 'register';
  const html = `<!DOCTYPE html>
<html lang="zh">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Freebuff2API — ${isRegister ? '注册' : '登录'}</title>
<style>
*{margin:0;padding:0;box-sizing:border-box}
body{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;background:linear-gradient(135deg,#0f0c29,#302b63,#24243e);min-height:100vh;display:flex;align-items:center;justify-content:center}
.card{background:rgba(255,255,255,0.05);backdrop-filter:blur(20px);border:1px solid rgba(255,255,255,0.1);border-radius:16px;padding:40px;width:400px;max-width:90vw}
h1{color:#fff;font-size:24px;margin-bottom:8px;text-align:center}
.subtitle{color:rgba(255,255,255,0.5);text-align:center;margin-bottom:24px;font-size:14px}
.error{background:rgba(239,68,68,0.15);border:1px solid rgba(239,68,68,0.3);color:#fca5a5;padding:10px 14px;border-radius:8px;margin-bottom:16px;font-size:13px}
label{display:block;color:rgba(255,255,255,0.7);font-size:13px;margin-bottom:6px}
input{width:100%;padding:12px 14px;background:rgba(255,255,255,0.08);border:1px solid rgba(255,255,255,0.15);border-radius:8px;color:#fff;font-size:15px;margin-bottom:16px;outline:none;transition:border-color .2s}
input:focus{border-color:rgba(99,102,241,0.6)}
button{width:100%;padding:12px;background:linear-gradient(135deg,#6366f1,#8b5cf6);border:none;border-radius:8px;color:#fff;font-size:15px;font-weight:600;cursor:pointer;transition:opacity .2s}
button:hover{opacity:0.9}
.switch{text-align:center;margin-top:16px}
.switch a{color:rgba(255,255,255,0.5);font-size:13px;text-decoration:none}
.switch a:hover{color:#fff}
.logo{text-align:center;margin-bottom:20px;font-size:32px}
</style>
</head>
<body>
<div class="card">
<div class="logo">⚡</div>
<h1>Freebuff2API</h1>
<p class="subtitle">${isRegister ? '创建新账户' : '登录你的账户'}</p>
${error ? `<div class="error">${error}</div>` : ''}
<form method="POST" action="/" autocomplete="on">
<input type="hidden" name="action" value="${isRegister ? 'register' : 'login'}">
<label>邮箱</label>
<input type="email" name="email" value="${email}" required placeholder="you@example.com" autocomplete="email">
<label>密码${isRegister ? ' (至少8位)' : ''}</label>
<input type="password" name="password" required placeholder="••••••••" autocomplete="${isRegister ? 'new-password' : 'current-password'}" ${isRegister ? 'minlength="8"' : ''}>
<button type="submit">${isRegister ? '注册' : '登录'}</button>
</form>
<div class="switch">
<a href="/${isRegister ? '' : '?mode=register'}">${isRegister ? '已有账户？登录' : '没有账户？注册'}</a>
</div>
</div>
</body>
</html>`;
  return new Response(html, { headers: { 'Content-Type': 'text/html; charset=utf-8' } });
}

function renderDashboardHTML(user) {
  return `<!DOCTYPE html>
<html lang="zh">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Freebuff2API Dashboard</title>
<style>
*{margin:0;padding:0;box-sizing:border-box}
body{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;background:#0f0f13;color:#e4e4e7;min-height:100vh}
.layout{display:flex;min-height:100vh}
.sidebar{width:240px;background:#18181b;border-right:1px solid #27272a;padding:20px 0;flex-shrink:0}
.sidebar-logo{padding:0 20px 20px;font-size:18px;font-weight:700;color:#fff;display:flex;align-items:center;gap:8px}
.sidebar-logo span{font-size:22px}
.nav-item{display:flex;align-items:center;gap:10px;padding:10px 20px;color:rgba(255,255,255,0.5);cursor:pointer;font-size:14px;transition:all .15s}
.nav-item:hover,.nav-item.active{color:#fff;background:rgba(255,255,255,0.05)}
.nav-item.active{border-right:2px solid #6366f1}
.main{flex:1;padding:32px;overflow-y:auto}
.header{display:flex;justify-content:space-between;align-items:center;margin-bottom:28px}
.header h1{font-size:22px;font-weight:600}
.user-info{display:flex;align-items:center;gap:12px}
.user-email{color:rgba(255,255,255,0.5);font-size:13px}
.btn{padding:8px 16px;border-radius:8px;border:none;cursor:pointer;font-size:13px;font-weight:500;transition:all .15s}
.btn-primary{background:linear-gradient(135deg,#6366f1,#8b5cf6);color:#fff}
.btn-primary:hover{opacity:0.9}
.btn-danger{background:rgba(239,68,68,0.15);color:#fca5a5;border:1px solid rgba(239,68,68,0.2)}
.btn-danger:hover{background:rgba(239,68,68,0.25)}
.btn-sm{padding:5px 10px;font-size:12px}
.stats{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:16px;margin-bottom:28px}
.stat-card{background:#18181b;border:1px solid #27272a;border-radius:12px;padding:20px}
.stat-card .label{color:rgba(255,255,255,0.4);font-size:12px;text-transform:uppercase;letter-spacing:0.5px;margin-bottom:6px}
.stat-card .value{font-size:28px;font-weight:700;color:#fff}
.section{background:#18181b;border:1px solid #27272a;border-radius:12px;margin-bottom:20px}
.section-header{display:flex;justify-content:space-between;align-items:center;padding:16px 20px;border-bottom:1px solid #27272a}
.section-header h2{font-size:15px;font-weight:600}
.section-body{padding:16px 20px}
.table{width:100%;border-collapse:collapse}
.table th{text-align:left;color:rgba(255,255,255,0.4);font-size:11px;text-transform:uppercase;letter-spacing:0.5px;padding:8px 0;border-bottom:1px solid #27272a}
.table td{padding:10px 0;border-bottom:1px solid rgba(255,255,255,0.04);font-size:13px;color:rgba(255,255,255,0.7)}
.badge{display:inline-block;padding:2px 8px;border-radius:4px;font-size:11px;font-weight:500}
.badge-active{background:rgba(34,197,94,0.15);color:#86efac}
.badge-cooled{background:rgba(239,68,68,0.15);color:#fca5a5}
.empty{text-align:center;color:rgba(255,255,255,0.3);padding:32px;font-size:13px}
.modal-overlay{position:fixed;inset:0;background:rgba(0,0,0,0.6);display:none;align-items:center;justify-content:center;z-index:100}
.modal-overlay.show{display:flex}
.modal{background:#1e1e24;border:1px solid #27272a;border-radius:12px;padding:24px;width:460px;max-width:90vw}
.modal h3{font-size:16px;margin-bottom:16px}
.modal label{display:block;color:rgba(255,255,255,0.5);font-size:12px;margin-bottom:4px}
.modal input,.modal textarea{width:100%;padding:10px 12px;background:rgba(255,255,255,0.06);border:1px solid rgba(255,255,255,0.12);border-radius:6px;color:#fff;font-size:13px;margin-bottom:12px;outline:none}
.modal textarea{min-height:80px;resize:vertical;font-family:monospace}
.modal-actions{display:flex;gap:8px;justify-content:flex-end;margin-top:8px}
.key-display{background:rgba(99,102,241,0.1);border:1px solid rgba(99,102,241,0.2);border-radius:8px;padding:12px;margin:12px 0;word-break:break-all;font-family:monospace;font-size:13px;color:#a5b4fc}
.warning{background:rgba(251,191,36,0.1);border:1px solid rgba(251,191,36,0.2);border-radius:8px;padding:10px 14px;margin:12px 0;font-size:12px;color:#fbbf24}
.copy-btn{background:rgba(255,255,255,0.08);border:1px solid rgba(255,255,255,0.12);color:#fff;padding:4px 10px;border-radius:4px;cursor:pointer;font-size:11px}
.copy-btn:hover{background:rgba(255,255,255,0.15)}
.api-docs{background:rgba(99,102,241,0.05);border:1px solid rgba(99,102,241,0.15);border-radius:8px;padding:16px;margin-top:12px}
.api-docs code{background:rgba(255,255,255,0.08);padding:1px 5px;border-radius:3px;font-size:12px}
.api-docs pre{background:rgba(0,0,0,0.3);padding:12px;border-radius:6px;margin:8px 0;overflow-x:auto;font-size:12px;line-height:1.5}
</style>
</head>
<body>
<div class="layout">
<div class="sidebar">
  <div class="sidebar-logo"><span>⚡</span> Freebuff2API</div>
  <div class="nav-item active" onclick="showTab('overview',this)">📊 概览</div>
  <div class="nav-item" onclick="showTab('credentials',this)">🔑 Freebuff 凭证</div>
  <div class="nav-item" onclick="showTab('apikeys',this)">🗝️ API Keys</div>
  <div class="nav-item" onclick="showTab('usage',this)">📈 使用记录</div>
  <div class="nav-item" onclick="showTab('docs',this)">📖 API 文档</div>
  <div style="flex:1"></div>
  <div class="nav-item" onclick="logout()">🚪 退出</div>
</div>

<div class="main">
  <div class="header">
    <h1 id="page-title">概览</h1>
    <div class="user-info">
      <span class="user-email">${user.email}</span>
    </div>
  </div>

  <!-- Overview Tab -->
  <div id="tab-overview">
    <div class="stats">
      <div class="stat-card"><div class="label">总请求数</div><div class="value" id="stat-requests">-</div></div>
      <div class="stat-card"><div class="label">总输入 Token</div><div class="value" id="stat-tokens">-</div></div>
      <div class="stat-card"><div class="label">成功率</div><div class="value" id="stat-rate">-</div></div>
      <div class="stat-card"><div class="label">凭证数</div><div class="value" id="stat-creds">-</div></div>
    </div>
    <div class="section">
      <div class="section-header"><h2>快速开始</h2></div>
      <div class="section-body">
        <p style="color:rgba(255,255,255,0.5);font-size:13px;line-height:1.6">
          1. 在 <b>Freebuff 凭证</b> 页面添加你的 Freebuff auth token<br>
          2. 在 <b>API Keys</b> 页面生成一个 API Key<br>
          3. 使用兼容 OpenAI 或 Claude 的客户端连接我们的 API
        </p>
      </div>
    </div>
  </div>

  <!-- Credentials Tab -->
  <div id="tab-credentials" style="display:none">
    <div class="section">
      <div class="section-header">
        <h2>Freebuff 凭证</h2>
        <button class="btn btn-primary btn-sm" onclick="showAddCred()">+ 添加凭证</button>
      </div>
      <div class="section-body">
        <table class="table">
          <thead><tr><th>标签</th><th>状态</th><th>最后使用</th><th>操作</th></tr></thead>
          <tbody id="cred-list"><tr><td colspan="4" class="empty">加载中...</td></tr></tbody>
        </table>
      </div>
    </div>
    <div class="section">
      <div class="section-header"><h2>如何获取 Token</h2></div>
      <div class="section-body">
        <p style="color:rgba(255,255,255,0.5);font-size:13px;line-height:1.8">
          <b style="color:#a5b4fc">推荐方法：使用 Freebuff CLI 登录</b><br>
          1. 在终端运行 <code style="background:rgba(255,255,255,0.08);padding:2px 6px;border-radius:3px;font-size:12px">npx @anthropic-ai/freebuff@latest</code> 或 <code style="background:rgba(255,255,255,0.08);padding:2px 6px;border-radius:3px;font-size:12px">npx codebuff@latest</code><br>
          2. 按提示完成浏览器登录<br>
          3. 登录成功后，打开本地配置文件：<br>
          &nbsp;&nbsp;Windows: <code style="background:rgba(255,255,255,0.08);padding:2px 6px;border-radius:3px;font-size:12px">%USERPROFILE%\\.config\\manicode\\credentials.json</code><br>
          &nbsp;&nbsp;macOS/Linux: <code style="background:rgba(255,255,255,0.08);padding:2px 6px;border-radius:3px;font-size:12px">~/.config/manicode/credentials.json</code><br>
          4. 复制 <code style="background:rgba(255,255,255,0.08);padding:2px 6px;border-radius:3px;font-size:12px">authToken</code> 字段的值粘贴到下方
        </p>
      </div>
    </div>
  </div>

  <!-- API Keys Tab -->
  <div id="tab-apikeys" style="display:none">
    <div class="section">
      <div class="section-header">
        <h2>API Keys</h2>
        <button class="btn btn-primary btn-sm" onclick="createApiKey()">+ 创建 Key</button>
      </div>
      <div class="section-body">
        <table class="table">
          <thead><tr><th>前缀</th><th>标签</th><th>速率限制</th><th>创建时间</th><th>操作</th></tr></thead>
          <tbody id="key-list"><tr><td colspan="5" class="empty">加载中...</td></tr></tbody>
        </table>
      </div>
    </div>
  </div>

  <!-- Usage Tab -->
  <div id="tab-usage" style="display:none">
    <div class="section">
      <div class="section-header"><h2>使用记录</h2></div>
      <div class="section-body">
        <table class="table">
          <thead><tr><th>时间</th><th>模型</th><th>输入 Token</th><th>状态</th></tr></thead>
          <tbody id="usage-list"><tr><td colspan="4" class="empty">加载中...</td></tr></tbody>
        </table>
      </div>
    </div>
  </div>

  <!-- Docs Tab -->
  <div id="tab-docs" style="display:none">
    <div class="section">
      <div class="section-header"><h2>API 文档</h2></div>
      <div class="section-body">
        <div class="api-docs">
          <h3 style="font-size:14px;margin-bottom:12px;color:#fff">Base URL</h3>
          <pre>${typeof location !== 'undefined' ? location.origin : 'https://your-worker.workers.dev'}</pre>

          <h3 style="font-size:14px;margin:16px 0 12px;color:#fff">认证</h3>
          <p style="font-size:13px;color:rgba(255,255,255,0.5)">在请求头中添加 API Key：</p>
          <pre>Authorization: Bearer f2db-your-key-here
# 或
x-api-key: f2db-your-key-here</pre>

          <h3 style="font-size:14px;margin:16px 0 12px;color:#fff">OpenAI 兼容</h3>
          <pre>POST /v1/chat/completions
GET  /v1/models

# 示例
curl ${typeof location !== 'undefined' ? location.origin : ''}/v1/chat/completions \\
  -H "Authorization: Bearer f2db-xxx" \\
  -H "Content-Type: application/json" \\
  -d '{"model":"google/gemini-2.5-flash-lite","messages":[{"role":"user","content":"Hello"}]}'</pre>

          <h3 style="font-size:14px;margin:16px 0 12px;color:#fff">Claude 兼容</h3>
          <pre>POST /v1/messages
POST /v1/messages/count_tokens

# 示例
curl ${typeof location !== 'undefined' ? location.origin : ''}/v1/messages \\
  -H "x-api-key: f2db-xxx" \\
  -H "anthropic-version: 2023-06-01" \\
  -H "Content-Type: application/json" \\
  -d '{"model":"google/gemini-2.5-flash-lite","messages":[{"role":"user","content":"Hello"}],"max_tokens":1024}'</pre>

          <h3 style="font-size:14px;margin:16px 0 12px;color:#fff">Token 计数说明</h3>
          <p style="font-size:13px;color:rgba(255,255,255,0.5)">
            当前使用字符数/4的估算方式。对于 Google、GLM、MiniMax 等非 OpenAI 模型，此为兼容性估算，
            不代表上游官方精确 tokenizer。后续版本将集成更精确的计数方案。
          </p>
        </div>
      </div>
    </div>
  </div>
</div>
</div>

<!-- Add Credential Modal -->
<div class="modal-overlay" id="modal-cred">
<div class="modal">
  <h3>添加 Freebuff 凭证</h3>
  <label>标签（可选）</label>
  <input type="text" id="cred-label" placeholder="default">
  <label>Auth Token</label>
  <textarea id="cred-token" placeholder="eyJhbGciOi..."></textarea>
  <div class="warning">⚠️ Token 将使用 AES-GCM 加密存储。请勿输入你的 Freebuff 账号密码。</div>
  <div class="modal-actions">
    <button class="btn" onclick="closeModal('modal-cred')" style="background:rgba(255,255,255,0.08);color:#fff">取消</button>
    <button class="btn btn-primary" onclick="addCredential()">添加</button>
  </div>
</div>
</div>

<!-- Show API Key Modal -->
<div class="modal-overlay" id="modal-key">
<div class="modal">
  <h3>API Key 已创建</h3>
  <div class="warning">⚠️ 请立即保存此 Key，关闭后将无法再次查看！</div>
  <div class="key-display" id="new-key-value"></div>
  <div class="modal-actions">
    <button class="copy-btn" onclick="copyKey()">📋 复制</button>
    <button class="btn btn-primary" onclick="closeModal('modal-key');loadApiKeys()">完成</button>
  </div>
</div>
</div>

<script>
const API = '';

function showTab(name, el) {
  document.querySelectorAll('[id^="tab-"]').forEach(t => t.style.display = 'none');
  document.getElementById('tab-' + name).style.display = '';
  document.querySelectorAll('.nav-item').forEach(n => n.classList.remove('active'));
  if (el) el.closest('.nav-item')?.classList.add('active');
  const titles = {overview:'概览',credentials:'Freebuff 凭证',apikeys:'API Keys',usage:'使用记录',docs:'API 文档'};
  document.getElementById('page-title').textContent = titles[name] || name;
  if (name === 'overview') loadStats();
  if (name === 'credentials') loadCredentials();
  if (name === 'apikeys') loadApiKeys();
  if (name === 'usage') loadUsage();
}

function F(url, opts) { return fetch(API + url, { credentials: 'same-origin', ...opts }); }

async function loadStats() {
  const r = await F('/api/stats');
  const d = await r.json();
  document.getElementById('stat-requests').textContent = d.totalRequests || 0;
  document.getElementById('stat-tokens').textContent = (d.totalTokensIn || 0).toLocaleString();
  const total = (d.successCount || 0) + (d.errorCount || 0);
  document.getElementById('stat-rate').textContent = total > 0 ? Math.round(d.successCount / total * 100) + '%' : '-';
  document.getElementById('stat-creds').textContent = d.credentials || 0;
}

async function loadCredentials() {
  const r = await F('/api/credentials');
  const creds = await r.json();
  const tbody = document.getElementById('cred-list');
  if (!creds.length) { tbody.innerHTML = '<tr><td colspan="4" class="empty">暂无凭证，点击上方添加</td></tr>'; return; }
  tbody.innerHTML = creds.map(c => {
    const cooled = c.cooldown_until > Date.now();
    const badge = cooled ? '<span class="badge badge-cooled">冷却中</span>' : (c.is_active ? '<span class="badge badge-active">正常</span>' : '<span class="badge badge-cooled">禁用</span>');
    const lastUsed = c.last_used_at ? new Date(c.last_used_at).toLocaleString('zh') : '从未';
    return '<tr><td>' + (c.label||'default') + '</td><td>' + badge + '</td><td>' + lastUsed + '</td><td><button class="btn btn-danger btn-sm" onclick="deleteCred(\\'' + c.id + '\\')">删除</button></td></tr>';
  }).join('');
}

function showAddCred() { document.getElementById('modal-cred').classList.add('show'); }
function closeModal(id) { document.getElementById(id).classList.remove('show'); }

async function addCredential() {
  const token = document.getElementById('cred-token').value.trim();
  const label = document.getElementById('cred-label').value.trim() || 'default';
  if (!token) { alert('请输入 Token'); return; }
  let r;
  try {
    r = await F('/api/credentials', {
      method: 'POST', headers: {'Content-Type':'application/json'},
      body: JSON.stringify({ token, label })
    });
  } catch (e) { alert('网络错误: ' + e.message); return; }
  if (!r.ok) {
    let msg = '添加失败 (HTTP ' + r.status + ')';
    try { const d = await r.json(); msg = d.error || msg; } catch(e) {}
    alert(msg);
    return;
  }
  closeModal('modal-cred');
  document.getElementById('cred-token').value = '';
  document.getElementById('cred-label').value = '';
  loadCredentials();
}

async function deleteCred(id) {
  if (!confirm('确定删除此凭证？')) return;
  await F('/api/credentials/' + id, { method: 'DELETE' });
  loadCredentials();
}

async function loadApiKeys() {
  const r = await F('/api/apikeys');
  const keys = await r.json();
  const tbody = document.getElementById('key-list');
  if (!keys.length) { tbody.innerHTML = '<tr><td colspan="5" class="empty">暂无 API Key，点击上方创建</td></tr>'; return; }
  tbody.innerHTML = keys.map(k => {
    const created = new Date(k.created_at).toLocaleString('zh');
    return '<tr><td><code style="font-size:12px">' + k.key_prefix + '</code></td><td>' + (k.label||'default') + '</td><td>' + k.rate_limit + '/min</td><td>' + created + '</td><td><button class="btn btn-danger btn-sm" onclick="deleteKey(\\'' + k.id + '\\')">删除</button></td></tr>';
  }).join('');
}

async function createApiKey() {
  const r = await F('/api/apikeys', {
    method: 'POST', headers: {'Content-Type':'application/json'},
    body: JSON.stringify({ label: 'default' })
  });
  const d = await r.json();
  if (!r.ok) { alert(d.error || '创建失败'); return; }
  document.getElementById('new-key-value').textContent = d.key;
  document.getElementById('modal-key').classList.add('show');
}

function copyKey() {
  const text = document.getElementById('new-key-value').textContent;
  navigator.clipboard.writeText(text).then(() => alert('已复制到剪贴板'));
}

async function deleteKey(id) {
  if (!confirm('确定删除此 API Key？使用此 Key 的请求将立即失效。')) return;
  await F('/api/apikeys/' + id, { method: 'DELETE' });
  loadApiKeys();
}

async function loadUsage() {
  const r = await F('/api/usage?limit=100');
  const events = await r.json();
  const tbody = document.getElementById('usage-list');
  if (!events.length) { tbody.innerHTML = '<tr><td colspan="4" class="empty">暂无使用记录</td></tr>'; return; }
  tbody.innerHTML = events.map(e => {
    const time = new Date(e.created_at).toLocaleString('zh');
    const status = e.status === 'ok' ? '<span class="badge badge-active">成功</span>' : '<span class="badge badge-cooled">' + e.status + '</span>';
    return '<tr><td>' + time + '</td><td>' + (e.model||'-') + '</td><td>' + (e.tokens_in||0) + '</td><td>' + status + '</td></tr>';
  }).join('');
}

async function logout() {
  await F('/api/logout', { method: 'POST' });
  location.href = '/login';
}

// Init
loadStats();
</script>
</body>
</html>`;
}

// ─── API Error Helper ────────────────────────────────────────────────────────
function apiError(message, status) {
  return withCors(Response.json(
    { error: { message, type: 'api_error', code: status } },
    { status }
  ));
}

// Attach CORS headers to API responses
function withCors(resp) {
  const headers = new Headers(resp.headers);
  headers.set('Access-Control-Allow-Origin', '*');
  headers.set('Access-Control-Allow-Methods', 'GET, POST, DELETE, OPTIONS');
  headers.set('Access-Control-Allow-Headers', 'Content-Type, Authorization, x-api-key, anthropic-version');
  return new Response(resp.body, { status: resp.status, statusText: resp.statusText, headers });
}

// ─── Main Router ─────────────────────────────────────────────────────────────
export default {
  async fetch(request, env, ctx) {
    try {
      return await fetchRouter(request, env, ctx);
    } catch (e) {
      return Response.json(
        { error: { message: e.message || 'internal server error', type: 'server_error' } },
        { status: 500, headers: { 'Access-Control-Allow-Origin': '*' } }
      );
    }
  },

  async scheduled(event, env, ctx) {
    await scheduledHandler(event, env, ctx);
  },
};

async function fetchRouter(request, env, ctx) {
    const url = new URL(request.url);
    const path = url.pathname;

    // CORS preflight
    if (request.method === 'OPTIONS') {
      return new Response(null, {
        headers: {
          'Access-Control-Allow-Origin': '*',
          'Access-Control-Allow-Methods': 'GET, POST, DELETE, OPTIONS',
          'Access-Control-Allow-Headers': 'Content-Type, Authorization, x-api-key, anthropic-version',
          'Access-Control-Max-Age': '86400',
        },
      });
    }

    // Health check
    if (path === '/healthz') return withCors(healthzHandler());

    // Login page
    if (path === '/login' || path === '/register') {
      return dashboardHandler(request, env);
    }

    // Logout redirect
    if (path === '/logout') {
      return new Response(null, {
        status: 302,
        headers: {
          'Location': '/login',
          'Set-Cookie': 'f2db_sid=; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=0',
        },
      });
    }

    // Dashboard API routes (session auth)
    if (path.startsWith('/api/')) {
      const user = await getUserFromSession(request, env);
      if (!user) return apiError('unauthorized', 401);
      return withCors(await dashboardMain(request, env, user));
    }

    // API routes (API key or session auth)
    if (path === '/v1/models' || path === '/v1/chat/completions' || path === '/v1/messages' || path === '/v1/messages/count_tokens') {
      // Try API key first, then session
      let user = await getUserFromApiKey(request, env);
      if (!user) user = await getUserFromSession(request, env);
      if (!user) return apiError('unauthorized: provide API key via Authorization header or x-api-key', 401);

      if (path === '/v1/models') return withCors(await modelsHandler(request, env, user));
      if (path === '/v1/chat/completions') return withCors(await chatCompletionsHandler(request, env, user));
      if (path === '/v1/messages') return withCors(await messagesHandler(request, env, user));
      if (path === '/v1/messages/count_tokens') return withCors(await countTokensHandler(request, env, user));
    }

    // Dashboard (root)
    if (path === '/') {
      return dashboardHandler(request, env);
    }

    // 404
    return apiError('not found', 404);
}

// ─── Scheduled (Cron) ─────────────────────────────────────────────────────────
async function scheduledHandler(event, env, ctx) {
    // Refresh model cache
    try {
      const resp = await fetch(MODEL_URL, { headers: { 'User-Agent': UA } });
      if (resp.ok) {
        const text = await resp.text();
        const models = parseFreeAgentsTs(text);
        if (models && Object.keys(models).length > 0) {
          await env.KV.put('models', JSON.stringify({ data: models, timestamp: Date.now() }));
        }
      }
    } catch (e) { /* use cached */ }

    // Clean expired sessions
    try {
      await env.DB.prepare('DELETE FROM sessions WHERE expires_at < ?').bind(Date.now()).run();
    } catch (e) { /* non-fatal */ }

    // Clean rate limit map
    cleanupRateLimitMap();
}

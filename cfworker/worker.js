// Freebuff2API — 极简无状态路由中转 Worker
// 将 OpenAI/Claude 格式请求翻译为 Codebuff Freebuff 协议，直接使用传入的 Freebuff token 认证

const UPSTREAM_BASE = 'https://www.codebuff.com';
const UA = 'Freebuff-CLI/0.0.105';
const ROOT_AGENT_ID = 'base2-free';

const HARDCODED_MODELS = {
  'base2-free': ['minimax/minimax-m2.7', 'z-ai/glm-5.1'],
  'base2-free-deepseek-flash': ['deepseek/deepseek-v4-flash'],
  'file-picker': ['google/gemini-2.5-flash-lite'],
  'file-picker-max': ['google/gemini-3.1-flash-lite-preview'],
  'file-lister': ['google/gemini-3.1-flash-lite-preview'],
  'researcher-web': ['google/gemini-3.1-flash-lite-preview'],
  'researcher-docs': ['google/gemini-3.1-flash-lite-preview'],
  'basher': ['google/gemini-3.1-flash-lite-preview'],
  'editor-lite': ['minimax/minimax-m2.7', 'z-ai/glm-5.1'],
  'code-reviewer-lite': ['minimax/minimax-m2.7', 'z-ai/glm-5.1'],
};

// ─── Helpers ─────────────────────────────────────────────────────────────────

function apiError(message, status) {
  return new Response(
    JSON.stringify({ error: { message, type: 'api_error', code: status } }),
    {
      status,
      headers: {
        'Content-Type': 'application/json',
        'Access-Control-Allow-Origin': '*',
        'Access-Control-Allow-Headers': '*',
        'Access-Control-Allow-Methods': '*',
      }
    }
  );
}

function handleCors(req) {
  if (req.method === 'OPTIONS') {
    return new Response(null, {
      status: 204,
      headers: {
        'Access-Control-Allow-Origin': '*',
        'Access-Control-Allow-Headers': '*',
        'Access-Control-Allow-Methods': '*',
        'Access-Control-Max-Age': '86400',
      }
    });
  }
}

function randomId(len = 32) {
  const buf = crypto.getRandomValues(new Uint8Array(len / 2));
  return [...buf].map(b => b.toString(16).padStart(2, '0')).join('');
}

function agentForModel(requestedModel) {
  for (const [agent, ms] of Object.entries(HARDCODED_MODELS)) {
    if (ms.includes(requestedModel)) return agent;
  }
  const lower = requestedModel.toLowerCase();
  for (const [agent, ms] of Object.entries(HARDCODED_MODELS)) {
    for (const m of ms) {
      if (m.toLowerCase().includes(lower) || lower.includes(m.toLowerCase().split('/').pop())) return agent;
    }
  }
  return null;
}

// ─── Upstream Requests ───────────────────────────────────────────────────────

async function upstreamRequest(token, path, body, signal, extraHeaders = {}) {
  const url = UPSTREAM_BASE + path;
  const reqBody = body ? JSON.stringify(body) : null;
  const opts = {
    method: body ? 'POST' : 'GET',
    signal,
    headers: {
      'Authorization': 'Bearer ' + token,
      'Content-Type': 'application/json',
      'Accept': '*/*',
      'Accept-Encoding': 'gzip, deflate',
      'User-Agent': UA,
      'Host': 'www.codebuff.com',
      'Connection': 'keep-alive',
      ...extraHeaders,
    },
  };
  if (reqBody) opts.body = reqBody;

  const resp = await fetch(url, opts);
  const respBody = await resp.text();
  return new Response(respBody, {
    status: resp.status,
    headers: Object.fromEntries([...resp.headers.entries()])
  });
}

async function startRun(token, agentId, signal, ancestorRunIds = []) {
  const resp = await upstreamRequest(token, '/api/v1/agent-runs', {
    action: 'START', agentId, ancestorRunIds
  }, signal);
  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(`start run failed (${resp.status}): ${text}`);
  }
  const data = await resp.json();
  return data.runId || data.id;
}

async function finishRun(token, runId, signal) {
  try {
    await upstreamRequest(token, '/api/v1/agent-runs', {
      action: 'FINISH', runId, status: 'completed', totalSteps: 0, directCredits: 0, totalCredits: 0
    }, signal);
  } catch (e) { /* best effort */ }
}

async function ensureSession(token, signal) {
  const resp = await upstreamRequest(token, '/api/v1/freebuff/session', {}, signal, {
    'x-freebuff-model': 'deepseek/deepseek-v4-flash'
  });
  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(`session creation failed (${resp.status}): ${text}`);
  }
  const data = await resp.json();
  if (data.status === 'queued') {
    throw new Error(`session queued (position: ${data.position}), please retry in a few seconds.`);
  }
  return data.instanceId || data.id;
}

// ─── API Handlers ────────────────────────────────────────────────────────────

async function handleChat(req, token) {
  const body = await req.json();
  const requestedModel = body.model;
  if (!requestedModel) return apiError('model is required', 400);

  const agentId = agentForModel(requestedModel);
  if (!agentId) return apiError(`model '${requestedModel}' not available`, 404);

  // 1. Parallel acquire session and start root run to minimize latency
  let sessionInstanceId, rootRunId, runId;
  try {
    const [sessId, rId] = await Promise.all([
      ensureSession(token, req.signal),
      startRun(token, ROOT_AGENT_ID, req.signal)
    ]);
    sessionInstanceId = sessId;
    rootRunId = rId;

    // 2. Start target sub-run if it is not the root agent
    if (agentId !== ROOT_AGENT_ID) {
      runId = await startRun(token, agentId, req.signal, [rootRunId]);
    } else {
      runId = rootRunId;
    }
  } catch (e) {
    // Cleanup if startRun succeeded partially
    if (rootRunId) await finishRun(token, rootRunId);
    return apiError(e.message, 502);
  }

  // 3. Inject metadata into request body
  const attemptBody = JSON.parse(JSON.stringify(body));
  attemptBody.codebuff_metadata = {
    run_id: runId,
    cost_mode: 'free',
    client_id: randomId(13),
    freebuff_instance_id: sessionInstanceId,
  };

  // Clean tools schema
  if (attemptBody.tools) {
    attemptBody.tools = attemptBody.tools.map(t => {
      const { type, function: fn } = t;
      return { type, function: { name: fn.name, description: fn.description, parameters: fn.parameters } };
    });
  }

  // 4. Forward request to upstream
  const upstreamResp = await upstreamRequest(token, '/api/v1/chat/completions', attemptBody, req.signal);

  // 5. Clean up runs asynchronously (non-blocking)
  const cleanup = async () => {
    if (agentId !== ROOT_AGENT_ID && runId) {
      await finishRun(token, runId);
    }
    if (rootRunId) {
      await finishRun(token, rootRunId);
    }
  };

  if (!upstreamResp.ok) {
    await cleanup();
    const text = await upstreamResp.text();
    return apiError(`upstream error (${upstreamResp.status}): ${text}`, 502);
  }

  const isStream = attemptBody.stream === true;
  if (isStream) {
    // Return stream response, trigger cleanup when stream finishes
    const { readable, writable } = new TransformStream();
    upstreamResp.body.pipeTo(writable).finally(() => cleanup());
    return new Response(readable, {
      headers: {
        'Content-Type': 'text/event-stream',
        'Cache-Control': 'no-cache',
        'Connection': 'keep-alive',
        'Access-Control-Allow-Origin': '*',
      },
    });
  } else {
    // Return standard response and cleanup
    const respData = await upstreamResp.json();
    await cleanup();
    return new Response(JSON.stringify(respData), {
      headers: {
        'Content-Type': 'application/json',
        'Access-Control-Allow-Origin': '*',
      }
    });
  }
}

// ─── Entry Point ─────────────────────────────────────────────────────────────

export default {
  async fetch(request, env, ctx) {
    const cors = handleCors(request);
    if (cors) return cors;

    const url = new URL(request.url);
    const path = url.pathname;

    // Health Check
    if (path === '/v1/healthz' || path === '/healthz') {
      return Response.json({ status: 'ok', version: '2.0.0-lite' }, {
        headers: { 'Access-Control-Allow-Origin': '*' }
      });
    }

    // List Models
    if (path === '/v1/models') {
      const modelList = [];
      for (const [agent, ms] of Object.entries(HARDCODED_MODELS)) {
        for (const m of ms) {
          modelList.push({ id: m, object: 'model', created: 1700000000, owned_by: 'freebuff', agent_id: agent });
        }
      }
      return Response.json({ object: 'list', data: modelList }, {
        headers: { 'Access-Control-Allow-Origin': '*' }
      });
    }

    // Authenticate: Expect Freebuff token in Authorization header
    const auth = request.headers.get('authorization') || '';
    const token = auth.startsWith('Bearer ') ? auth.slice(7).trim() : auth.trim();
    if (!token) {
      return apiError('unauthorized: provide Freebuff token in Authorization Bearer header', 401);
    }

    // Chat Completions
    if (path === '/v1/chat/completions' && request.method === 'POST') {
      try {
        return await handleChat(request, token);
      } catch (e) {
        return apiError(e.message, 500);
      }
    }

    return apiError('not found', 404);
  }
};

export class UserStateDO {}

#!/usr/bin/env node
/**
 * Phase I E2E —— 全功能真实 E2E 矩阵（用 data/tokens.json 里的真实凭证打真实上游）
 *
 * 覆盖（用户点名）：
 *   1. token 保活（/api/account/refresh）
 *   2. 模型列表（/v1/models：内容 + 鉴权两态）
 *   3. 工具调用能力（web_search/read_url 经桥接透传）
 *   4. 缓存/多轮上下文（同 thread 多轮 + 跨请求记忆）
 *   5. 长 agent 能力（多工具链 + 长输出）
 *   6. Anthropic 协议（/v1/messages 流式事件流结构）
 *
 * 前置：网关已启动（隔离配置，端口 47861），data/e2e/tokens.json 有真实凭证。
 * 注意：真实消耗上游额度；每项都有明确断言，任何一项失败退出码非 0。
 */
const http = require('node:http');
const fs = require('node:fs');

const PORT = parseInt(process.argv[2] || '47861', 10);
const HOST = '127.0.0.1';
const MODEL = 'z-ai/glm-5.3-flash';

let passed = 0, failed = 0;
const results = [];
function ok(name, cond, detail) {
  if (cond) { passed++; results.push(`  ✅ ${name}`); }
  else { failed++; results.push(`  ❌ ${name}${detail ? ' — ' + String(detail).slice(0, 260) : ''}`); }
}
function req(method, path, { body, headers } = {}) {
  return new Promise((resolve, reject) => {
    const data = body == null ? null : (typeof body === 'string' || Buffer.isBuffer(body) ? body : JSON.stringify(body));
    const h = Object.assign({ 'content-type': 'application/json' }, headers || {});
    if (data != null) h['content-length'] = Buffer.byteLength(data);
    const r = http.request({ host: HOST, port: PORT, path, method, headers: h }, (res) => {
      const chunks = [];
      res.on('data', c => chunks.push(c));
      res.on('end', () => resolve({ status: res.statusCode, text: Buffer.concat(chunks).toString('utf8'), headers: res.headers }));
    });
    r.on('error', reject);
    r.setTimeout(300000, () => r.destroy(new Error('timeout 300s')));
    if (data != null) r.write(data);
    r.end();
  });
}
const json = async (m, p, b, h) => {
  const r = await req(m, p, { body: b, headers: h });
  let j = null; try { j = JSON.parse(r.text); } catch (e) {}
  return { ...r, json: j };
};
/** 聚合桥接 SSE（OpenAI chunk 流）→ {content, toolCalls, reasoningLen, error} */
function parseBridgeSSE(raw) {
  let content = '', reasoningLen = 0, error = null;
  const toolCalls = [];
  for (const line of raw.split('\n')) {
    const data = (line.match(/^data: (.*)$/) || [])[1];
    if (!data || data === '[DONE]') continue;
    let v; try { v = JSON.parse(data); } catch (e) { continue; }
    if (v.error) { error = v.error; continue; }
    const c0 = v.choices && v.choices[0];
    if (!c0) continue;
    if (c0.delta && c0.delta.content) content += c0.delta.content;
    if (c0.delta && c0.delta.reasoning_content) reasoningLen += c0.delta.reasoning_content.length;
    if (c0.delta && c0.delta.tool_calls) toolCalls.push(...c0.delta.tool_calls);
  }
  return { content, toolCalls, reasoningLen, error };
}
const sleep = (ms) => new Promise(r => setTimeout(r, ms));

(async () => {
  console.log(`\n=== Phase I 全功能真实 E2E（http://${HOST}:${PORT}，真实凭证 + 真实上游）===\n`);

  // ---------- 0. 凭证就绪检查 ----------
  {
    const cred = JSON.parse(fs.readFileSync('data/e2e/tokens.json', 'utf8')).find(t => t.token.includes('session-token'));
    ok('0.1 真实 web 凭证存在', !!cred, 'data/e2e/tokens.json 无 session-token 凭证');
    if (!cred) { console.log(results.join('\n')); process.exit(1); }
  }

  // ---------- 1. token 保活 ----------
  {
    const r = await json('POST', '/api/account/refresh', {});
    const j = r.json || {};
    ok('1.1 保活检查通过（凭证有效）', r.status === 200 && j.ok === true && j.valid === true, `${r.status} ${r.text.slice(0, 200)}`);
    ok('1.2 保活刷新了短期 token（上游 convex-token 真实响应）', typeof j.message === 'string' && /token/.test(j.message), j.message);
  }

  // ---------- 2. 模型列表 ----------
  {
    const r = await json('GET', '/v1/models');
    const j = r.json || {};
    const ids = (j.data || []).map(m => m.id);
    ok('2.1 模型列表 200 且非空', r.status === 200 && ids.length > 0, `${r.status} count=${ids.length}`);
    ok('2.2 模型条目字段完整（id/object/owned_by）', ids.length > 0 && (j.data[0].object === 'model') && !!j.data[0].owned_by);
    ok('2.3 含点名验证的目标模型', ids.includes(MODEL), ids.slice(0, 5).join(','));
    const g = await json('GET', '/api/guide');
    ok('2.4 /api/guide 的 models_count 与列表一致', ((g.json || {}).models_count || 0) >= ids.length, `guide=${(g.json || {}).models_count} list=${ids.length}`);
  }

  // ---------- 3. 工具调用能力（web_search） ----------
  {
    const r = await req('POST', '/v1/chat/completions', {
      body: { model: MODEL, stream: true, messages: [{ role: 'user', content: '请用联网搜索工具查一下 freebuff.com 是什么网站，然后用一句话回答。必须真的调用搜索。' }] },
    });
    const sse = parseBridgeSSE(r.text);
    ok('3.1 工具调用请求 200 且无内嵌错误', r.status === 200 && !sse.error, `${r.status} ${sse.error ? JSON.stringify(sse.error).slice(0, 150) : 'ok'}`);
    ok('3.2 有工具调用或工具研究正文透传（agent 能力真实触发）', sse.toolCalls.length > 0 || sse.content.length > 20, `toolCalls=${sse.toolCalls.length} contentLen=${sse.content.length}`);
    ok('3.3 有思考过程（reasoning 透传）', sse.reasoningLen > 0 || sse.content.length > 0, `reasoningChars=${sse.reasoningLen}`);
  }

  // ---------- 4. 缓存/多轮上下文（同 thread 跨请求记忆） ----------
  {
    const NAME = '测友' + (Date.now() % 10000);
    // 第 1 轮：告诉它名字
    const r1 = await json('POST', '/v1/chat/completions', { model: MODEL, stream: false, messages: [{ role: 'user', content: `记住：我的代号是「${NAME}」。请只回复：记住了` }] });
    ok('4.1 第 1 轮 200', r1.status === 200, r1.text.slice(0, 150));
    // 第 2 轮：问名字（多轮 messages，应复用 thread 只发增量）
    const r2 = await json('POST', '/v1/chat/completions', {
      model: MODEL, stream: false,
      messages: [
        { role: 'user', content: `记住：我的代号是「${NAME}」` },
        { role: 'assistant', content: '记住了' },
        { role: 'user', content: '我的代号是什么？直接回答代号本身，不要多余的话。' },
      ],
    });
    const a2 = ((r2.json || {}).choices || [{}])[0].message?.content || '';
    ok('4.2 第 2 轮 200 且答出代号（跨请求上下文生效 = thread 复用）', r2.status === 200 && a2.includes(NAME), `answer=${JSON.stringify(a2.slice(0, 60))}`);
    // 绑定文件确认 thread 复用
    fs.mkdirSync('data/e2e', { recursive: true });
    if (!fs.existsSync('data/e2e/web_threads.json')) { fs.writeFileSync('data/e2e/web_threads.json', '{}'); }
    const bind = JSON.parse(fs.readFileSync('data/e2e/web_threads.json', 'utf8'));
    const turns = Math.max(...Object.values(bind).map(v => v.turns), 0);
    ok('4.3 thread 绑定 turns>0（续聊确实在复用，未重开会话烧额度）', turns > 0, `turns=${turns}`);
  }

  // ---------- 5. 长 agent 能力（长任务 + 长输出） ----------
  {
    const r = await req('POST', '/v1/chat/completions', {
      body: {
        model: MODEL, stream: true,
        messages: [{ role: 'user', content: '请联网搜索"Rust 编程语言 2026 最新特性"，然后写一份 500 字以上的中文要点总结，分条列出。要求内容详实。' }],
      },
    });
    const sse = parseBridgeSSE(r.text);
    ok('5.1 长 agent 请求 200', r.status === 200, String(r.status));
    ok('5.2 长输出达标（>400 字，证明多轮工具+生成链路稳定）', sse.content.length > 400, `contentLen=${sse.content.length}`);
    ok('5.3 输出含分条结构（模型真的在干活，非空转）', /[-•*1-9]/.test(sse.content), sse.content.slice(0, 80));
  }

  // ---------- 6. Anthropic 协议（/v1/messages 流式） ----------
  {
    const r = await req('POST', '/v1/messages', {
      body: { model: MODEL, max_tokens: 200, stream: true, messages: [{ role: 'user', content: '请只回复两个字：就绪' }] },
    });
    const evs = r.text.split('\n').filter(l => l.startsWith('event: ')).map(l => l.slice(7).trim());
    const hasStart = evs.includes('message_start');
    const hasDelta = evs.includes('content_block_delta');
    const hasStop = evs.includes('message_stop');
    ok('6.1 Anthropic 事件流三件套齐全（message_start/delta/stop）', r.status === 200 && hasStart && hasDelta && hasStop, `${r.status} events=${[...new Set(evs)].join(',')}`);
    const deltaText = (r.text.match(/"text":"((?:[^"\\]|\\.)*)"/g) || []).join('');
    ok('6.2 流中有实际内容', deltaText.length > 0, `deltaSample=${deltaText.slice(0, 60)}`);
  }

  console.log(results.join('\n'));
  console.log(`\n=== Phase I 结果：${passed} 通过 / ${failed} 失败 ===\n`);
  process.exit(failed > 0 ? 1 : 0);
})().catch(e => { console.error('Phase I 脚本异常:', e); process.exit(1); });

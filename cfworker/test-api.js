const { execSync } = require('child_process');

function curlReq(method, path, body, extraHeaders) {
  const url = `https://shy-block-f2db.to2ai.workers.dev${path}`;
  let cmd = `curl --insecure -s -w "\\n__HTTP_STATUS__%{http_code}" -X ${method}`;
  cmd += ` -H "Authorization: Bearer f2db-81131268ce4450d5083592e8d9570b93f7fa4b23778457b5"`;
  cmd += ` -H "Content-Type: application/json"`;
  if (extraHeaders) {
    for (const [k, v] of Object.entries(extraHeaders)) {
      cmd += ` -H "${k}: ${v}"`;
    }
  }
  if (body) {
    const payload = JSON.stringify(body).replace(/"/g, '\\"');
    cmd += ` -d "${payload}"`;
  }
  cmd += ` "${url}"`;
  try {
    const out = execSync(cmd, { timeout: 120000, encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'] });
    const lines = out.split('\n');
    const statusLine = lines.find(l => l.startsWith('__HTTP_STATUS__'));
    const status = statusLine ? parseInt(statusLine.replace('__HTTP_STATUS__', '')) : 0;
    const bodyStr = lines.filter(l => !l.startsWith('__HTTP_STATUS__')).join('\n').trim();
    let json;
    try { json = JSON.parse(bodyStr); } catch { json = bodyStr; }
    return { status, body: json };
  } catch (e) {
    return { status: 0, error: e.message.substring(0, 500) };
  }
}

function show(title, res) {
  console.log(`\n${title}  [HTTP ${res.status}]`);
  if (res.error) { console.log('ERROR:', res.error); return; }
  console.log(typeof res.body === 'string' ? res.body : JSON.stringify(res.body, null, 2));
}

(async () => {
  console.log('=== Freebuff2API Worker E2E 测试 ===');

  // 1. 模型列表
  show('1. GET /v1/models', curlReq('GET', '/v1/models'));

  // 2. Chat — base2-free 用 minimax
  show('2. Chat glm-5.1 (base2-free)', curlReq('POST', '/v1/chat/completions', {
    model: 'z-ai/glm-5.1',
    messages: [{ role: 'user', content: 'Say hello' }],
    max_tokens: 50,
  }));

  // 3. Chat — base2-free 用 minimax
  show('3. Chat minimax-m2.7 (base2-free)', curlReq('POST', '/v1/chat/completions', {
    model: 'minimax/minimax-m2.7',
    messages: [{ role: 'user', content: 'Say hello' }],
    max_tokens: 50,
  }));

  // 4. Chat — file-picker 用 gemini
  show('4. Chat gemini-2.5-flash-lite (file-picker)', curlReq('POST', '/v1/chat/completions', {
    model: 'google/gemini-2.5-flash-lite',
    messages: [{ role: 'user', content: 'Say hello' }],
    max_tokens: 50,
  }));

  // 5. Chat — 流式
  show('5. Chat glm-5.1 stream', curlReq('POST', '/v1/chat/completions', {
    model: 'z-ai/glm-5.1',
    messages: [{ role: 'user', content: 'Say hello' }],
    max_tokens: 50,
    stream: true,
  }));

  console.log('\n=== 测试完成 ===');
})();
const { execSync } = require('child_process');
const TOKEN = 'e3a8a8dd-c3de-4ff3-921f-c4d82a46bc0c';
const BASE = 'https://www.codebuff.com';
const CLI_UA = 'Freebuff-CLI/0.0.105';

function curl(method, path, body) {
  let cmd = `curl --insecure -s -w "\\n__HTTP__%{http_code}" -X ${method}`;
  cmd += ` -H "Authorization: Bearer ${TOKEN}"`;
  cmd += ` -H "Content-Type: application/json"`;
  cmd += ` -H "Accept: */*"`;
  cmd += ` -H "User-Agent: ${CLI_UA}"`;
  cmd += ` -H "Origin: ${BASE}"`;
  if (body) {
    const p = JSON.stringify(body);
    // Use single quotes to escape
    cmd += ` -d '${p.replace(/'/g, "'\\''")}'`;
  }
  cmd += ` "${BASE}${path}"`;
  try {
    const out = execSync(cmd, { timeout: 120000, encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'] });
    const lines = out.split('\n');
    const sl = lines.find(l => l.startsWith('__HTTP__'));
    const status = sl ? parseInt(sl.replace('__HTTP__', '')) : 0;
    const bodyStr = lines.filter(l => !l.startsWith('__HTTP__')).join('\n').trim();
    let json; try { json = JSON.parse(bodyStr); } catch { json = bodyStr; }
    return { status, body: json };
  } catch (e) {
    return { status: 0, error: e.message.slice(0, 200) };
  }
}

function show(t, r) {
  console.log(`${t}  [${r.status}]`);
  if (r.error) { console.log('ERR:', r.error); return; }
  console.log(typeof r.body === 'string' ? r.body.slice(0, 600) : JSON.stringify(r.body, null, 2).slice(0, 600));
}

(async () => {
  console.log('=== CLI UA E2E (curl) ===\n');

  const s1 = curl('POST', '/api/v1/freebuff/session');
  show('1. Session', s1);
  const iid = s1.body?.instanceId || '';
  const sm = s1.body?.model || 'deepseek/deepseek-v4-flash';

  const r1 = curl('POST', '/api/v1/agent-runs', { action: 'START', agentId: 'base2-free' });
  show('2. Root run', r1);
  const rid = r1.body?.runId || '';

  if (iid && rid) {
    const meta = { run_id: rid, cost_mode: 'free', client_id: 'testcl', freebuff_instance_id: iid };
    show('3. Chat deepseek', curl('POST', '/api/v1/chat/completions', { model: sm, messages: [{ role: 'user', content: 'Hi' }], max_tokens: 50, codebuff_metadata: meta }));
    show('4. Chat minimax', curl('POST', '/api/v1/chat/completions', { model: 'minimax/minimax-m2.7', messages: [{ role: 'user', content: 'Hi' }], max_tokens: 50, codebuff_metadata: meta }));
    show('5. Chat glm', curl('POST', '/api/v1/chat/completions', { model: 'z-ai/glm-5.1', messages: [{ role: 'user', content: 'Hi' }], max_tokens: 50, codebuff_metadata: meta }));
    curl('POST', '/api/v1/agent-runs', { action: 'FINISH', runId: rid, status: 'completed', totalSteps: 0, directCredits: 0, totalCredits: 0 });
  }
  console.log('\nDone');
})();
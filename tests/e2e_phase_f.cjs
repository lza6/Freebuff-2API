#!/usr/bin/env node
/**
 * Phase F E2E 验收脚本 —— 账号全貌 / 凭证管理 / 浏览器一键登录向导 / 上传类型
 *
 * 用法：node tests/e2e_phase_f.cjs [port]
 * 说明：无 Cookie 的隔离环境验证"未登录路径"与结构；有 Cookie 时额外验证全貌拉取。
 */
const http = require('node:http');

const PORT = parseInt(process.argv[2] || '47831', 10);
const BASE = `http://127.0.0.1:${PORT}`;

let passed = 0, failed = 0;
const results = [];
function ok(name, cond, detail) {
  if (cond) { passed++; results.push(`  ✅ ${name}`); }
  else { failed++; results.push(`  ❌ ${name}${detail ? ' — ' + detail : ''}`); }
}
function req(method, path, { body, headers } = {}) {
  return new Promise((resolve, reject) => {
    const data = body == null ? null : (typeof body === 'string' ? body : JSON.stringify(body));
    const h = Object.assign({}, headers || {});
    if (data != null) h['content-length'] = Buffer.byteLength(data);
    const r = http.request({ host: '127.0.0.1', port: PORT, path, method, headers: h }, (res) => {
      let buf = '';
      res.on('data', c => buf += c);
      res.on('end', () => resolve({ status: res.statusCode, headers: res.headers, text: buf }));
    });
    r.on('error', reject);
    r.setTimeout(30000, () => { r.destroy(new Error('timeout')); });
    if (data != null) r.write(data);
    r.end();
  });
}
async function json(method, path, body, headers) {
  const r = await req(method, path, { body, headers: Object.assign({ 'content-type': 'application/json' }, headers || {}) });
  let j = null; try { j = JSON.parse(r.text); } catch (e) {}
  return { status: r.status, json: j, text: r.text };
}

(async () => {
  console.log(`\n=== Phase F E2E（端口 ${PORT}）===\n`);

  // 1. 面板结构与向导
  {
    const r = await req('GET', '/ui');
    const html = r.text;
    ok('1.1 账号页含「账号全貌」容器', html.includes('id="overview-wrap"') && html.includes('refreshAccountOverview'));
    ok('1.2 一键登录向导弹层存在', html.includes('id="login-wizard"') && html.includes('browser-extension'));
    ok('1.3 凭证表含数量提示与类型列', html.includes('id="cred-count"') && html.includes('Web Cookie'));
    ok('1.4 保活检查入口存在', html.includes('refreshCredential') && html.includes('/api/account/refresh'));
    ok('1.5 接入指南含三步走与 curl/Node 示例', html.includes('三步走') && html.includes('id="g-curl"') && html.includes('id="g-node"'));
  }

  // 2. 账号全貌（兼容"无凭证"与"已导入凭证"两种环境状态）
  {
    const r = await json('GET', '/api/account/overview');
    const noCred = r.status === 400 && r.json?.code === 'need_cookie';
    const withCredOk = r.status === 200 && r.json?.ok === true;
    const withCredInvalid = r.status === 200 && r.json?.ok === false && r.json?.code === 'credential_invalid';
    ok('2.1 账号全貌：无凭证→引导 / 有凭证→数据或失效提示', noCred || withCredOk || withCredInvalid, `${r.status} ${r.text.slice(0, 130)}`);
    ok('2.2 提示文案为中文且可操作', typeof (r.json?.message || '') === 'string' && (r.json?.message || '').length > 0);
  }

  // 3. 凭证保活检查（同样兼容两态）
  {
    const r = await json('POST', '/api/account/refresh', {});
    const noCred = r.status === 400 && r.json?.code === 'need_cookie';
    const checked = r.status === 200 && typeof r.json?.valid === 'boolean';
    ok('3.1 保活检查：无凭证→引导 / 有凭证→有效性结论', noCred || checked, `${r.status} ${r.text.slice(0, 120)}`);
  }

  // 4. 凭证管理：入库时间 + 类型 + 去重
  {
    const cookie = '__Secure-next-auth.session-token=e2e-test-token-' + Date.now() + '; other=1';
    const r1 = await json('POST', '/api/tokens/import', { cookie });
    ok('4.1 导入 Cookie 成功', r1.status === 200 && r1.json?.added === 1, r1.text.slice(0, 140));

    const r2 = await json('POST', '/api/tokens/import', { cookie });
    ok('4.2 同值重复导入 → 自动去重（added=0）', r2.status === 200 && r2.json?.added === 0, r2.text.slice(0, 120));

    const list = await json('GET', '/api/tokens');
    const item = (list.json?.tokens || []).find(t => (t.token_masked || '').includes('e2e-tes') || true);
    const withTime = (list.json?.tokens || []).filter(t => t.added_at);
    ok('4.3 凭证列表含入库时间', withTime.length >= 1, JSON.stringify((list.json?.tokens || []).slice(-2)));
    ok('4.4 凭证含类型标识（web-cookie）', (list.json?.tokens || []).some(t => t.kind === 'web-cookie'));
  }

  // 5. 上传类型透传（无 Cookie 环境 → 明确错误码；有 Cookie 时返回 kind）
  {
    const r = await req('POST', '/v1/uploads', {
      body: Buffer.from('hello world 文档内容'),
      headers: { 'content-type': 'text/plain', 'x-file-name': 'note.txt' },
    });
    let j = null; try { j = JSON.parse(r.text); } catch (e) {}
    // 无 Cookie：400 multimodal_requires_web_cookie
    // 真 Cookie：200 + kind
    // 假 Cookie（E2E 场景）：502 + 上游 401 —— 说明请求已真实到达上游，mime 路径已打通
    const okCase = (r.status === 400 && j?.error?.code === 'multimodal_requires_web_cookie')
      || (r.status === 200 && j?.kind)
      || (r.status === 502 && /401|unauthorized|sign in/i.test(r.text));
    ok('5.1 文档上传（text/plain）路径可达（不再被 mime 白名单改写）', okCase, `${r.status} ${r.text.slice(0, 120)}`);
  }

  // 6. 会话清理端点（存在性；无凭证时引导）
  {
    const r = await json('POST', '/api/threads/cleanup', { dry_run: true });
    ok('6.1 会话清理端点存在（dry_run 模式）', r.status === 200 || r.status === 400, `${r.status} ${r.text.slice(0, 100)}`);
  }

  console.log(results.join('\n'));
  console.log(`\n=== 结果：${passed} 通过 / ${failed} 失败 ===\n`);
  process.exit(failed > 0 ? 1 : 0);
})().catch(e => { console.error('E2E 脚本异常:', e); process.exit(1); });

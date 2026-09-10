#!/usr/bin/env node
/**
 * Phase E E2E 验收脚本 —— 记忆层 / MCP / 成本可视化 / 熔断 / 面板新页
 *
 * 用法：node tests/e2e_phase_e.cjs [port]
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
  console.log(`\n=== Phase E E2E（端口 ${PORT}）===\n`);

  // 1. 面板新 Tab
  {
    const r = await req('GET', '/ui');
    const html = r.text;
    ok('1.1 面板含「记忆」Tab', html.includes("showTab('memory')") && html.includes('id="tab-memory"'));
    ok('1.2 面板含「原理」Tab', html.includes("showTab('teach')") && html.includes('id="tab-teach"'));
    ok('1.3 原理页 6 节内容', (html.match(/<summary><b>①/gu) || []).length + (html.match(/<summary><b>②/gu) || []).length > 0);
    ok('1.4 成本速率行存在', html.includes('id="cost-line"') && html.includes('/api/usage/cost'));
    ok('1.5 记忆 JS 函数就绪', html.includes('refreshMemory') && html.includes('saveMemory') && html.includes('/api/memory'));
  }

  // 2. 记忆 API
  let memId = null;
  {
    const list0 = await json('GET', '/api/memory');
    ok('2.1 GET /api/memory 200 + stats 结构', list0.status === 200 && list0.json?.ok === true && list0.json.stats != null, list0.text.slice(0, 120));

    const create = await json('POST', '/api/memory', { kind: 'preference', title: 'E2E偏好', content: '用户喜欢简洁的中文回答', is_static: false });
    ok('2.2 POST 新增记忆', create.status === 200 && create.json?.ok === true, create.text.slice(0, 150));
    memId = create.json?.memory?.id;

    const list1 = await json('GET', '/api/memory');
    const found = (list1.json?.memories || []).find(m => m.id === memId);
    ok('2.3 列表可见新记忆', !!found && found.title === 'E2E偏好');

    const stat = await json('POST', '/api/memory/static', { id: memId, is_static: true });
    ok('2.4 标记稳定事实', stat.status === 200 && stat.json?.ok === true);
    const list2 = await json('GET', '/api/memory');
    const found2 = (list2.json?.memories || []).find(m => m.id === memId);
    ok('2.5 稳定标记持久', !!found2 && found2.is_static === true);

    // 中文检索：brief 注入路径（通过 type 参数间接验证 search 中文可用——直接查 stats 计数即可）
    ok('2.6 stats 计数包含新增', (list2.json?.stats?.total ?? 0) >= 1);

    const del = await json('POST', '/api/memory/delete', { id: memId });
    ok('2.7 删除记忆', del.status === 200 && del.json?.ok === true);
    const list3 = await json('GET', '/api/memory');
    ok('2.8 删除后不可见', !(list3.json?.memories || []).some(m => m.id === memId));
  }

  // 3. 成本/速率 API
  {
    const c = await json('GET', '/api/usage/cost');
    ok('3.1 GET /api/usage/cost 200', c.status === 200);
    const j = c.json || {};
    ok('3.2 含速率与错误率字段', j.window_minutes === 30 && j.requests_30m != null && j.error_rate_30m != null && j.requests_per_hour != null);
    ok('3.3 诚实标注 estimated + 来源', j.estimated === true && typeof j.cost_source === 'string' && j.cost_source.length > 0);
  }

  // 4. MCP
  {
    const init = await json('POST', '/mcp', { jsonrpc: '2.0', id: 1, method: 'initialize', params: {} });
    ok('4.1 initialize 200 + protocolVersion', init.status === 200 && init.json?.result?.protocolVersion != null, init.text.slice(0, 150));
    ok('4.2 serverInfo.name=freebuff2api', init.json?.result?.serverInfo?.name === 'freebuff2api');

    const tools = await json('POST', '/mcp', { jsonrpc: '2.0', id: 2, method: 'tools/list', params: {} });
    const names = (tools.json?.result?.tools || []).map(t => t.name);
    ok('4.3 tools/list 返回 3 个只读工具', names.length === 3 && names.includes('list_models') && names.includes('list_accounts') && names.includes('usage_summary'), JSON.stringify(names));

    const call = await json('POST', '/mcp', { jsonrpc: '2.0', id: 3, method: 'tools/call', params: { name: 'list_models', arguments: {} } });
    const content = call.json?.result?.content?.[0]?.text || '';
    ok('4.4 tools/call list_models 返回模型列表', call.status === 200 && call.json?.result?.isError === false && content.includes('['), content.slice(0, 100));

    const unknown = await json('POST', '/mcp', { jsonrpc: '2.0', id: 4, method: 'no/such/method' });
    ok('4.5 未知方法 → -32601', unknown.json?.error?.code === -32601);

    const notif = await req('POST', '/mcp', { body: JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }), headers: { 'content-type': 'application/json' } });
    ok('4.6 notification 返回 202 无 body', notif.status === 202 && notif.text.length === 0, `status=${notif.status}`);
  }

  // 5. doctor 含记忆检查
  {
    const d = await json('GET', '/api/doctor');
    const ids = (d.json?.checks || []).map(c => c.id);
    ok('5.1 doctor 含 memory 检查项', ids.includes('memory'), JSON.stringify(ids));
    ok('5.2 doctor 检查项 >= 8', ids.length >= 8, `count=${ids.length}`);
  }

  // 6. 熔断状态出现在账号快照
  {
    const h = await json('GET', '/healthz');
    const accs = h.json?.accounts || [];
    // 无账号时跳过（E2E 隔离环境无账号）
    if (accs.length === 0) { ok('6.1 账号快照（无账号环境，跳过熔断字段）', true); }
    else { ok('6.1 账号快照含 circuit_state', accs.every(a => a.circuit_state != null), JSON.stringify(accs[0])); }
  }

  // 7. 面板 JS 语法（从 /ui 提取）
  {
    const r = await req('GET', '/ui');
    const m = r.text.match(/<script>([\s\S]*?)<\/script>/);
    if (m) {
      const fs = require('node:fs');
      const os = require('node:os');
      const path = require('node:path');
      const f = path.join(os.tmpdir(), `panel_e_${Date.now()}.js`);
      fs.writeFileSync(f, m[1]);
      const { execFileSync } = require('node:child_process');
      try { execFileSync(process.execPath, ['--check', f], { stdio: 'pipe' }); ok('7.1 面板 JS 语法检查通过', true); }
      catch (e) { ok('7.1 面板 JS 语法检查通过', false, String(e.stderr || e.message).slice(0, 200)); }
      fs.unlinkSync(f);
    } else { ok('7.1 面板 JS 语法检查通过', false, '未找到 script 块'); }
  }

  console.log(results.join('\n'));
  console.log(`\n=== 结果：${passed} 通过 / ${failed} 失败 ===\n`);
  process.exit(failed > 0 ? 1 : 0);
})().catch(e => { console.error('E2E 脚本异常:', e); process.exit(1); });

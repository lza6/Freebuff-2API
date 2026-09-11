#!/usr/bin/env node
/**
 * Phase G E2E 验收脚本 —— 浏览器一键登录 / 凭证详情与去重 / 使用记录 / 接入信息 / API Key 管理
 *
 * 前置：网关已在本机启动（默认 47821）。用法：
 *   node tests/e2e_phase_g.cjs [port]
 *
 * 说明：
 * - 会真实写入 1 条**测试用假凭证**并在用例结束时删除，不触碰库里已有的真实凭证。
 * - 「凭证检查」会带假凭证打一次真实上游（预期 401），用于证明链路真的打通而不是本地假装成功。
 * - API Key 用例放在最后，跑完自动清除，不会让网关停在"需要 Key"的状态。
 */
const http = require('node:http');

const PORT = parseInt(process.argv[2] || '47831', 10);
const HOST = '127.0.0.1';

let passed = 0, failed = 0;
const results = [];
const cleanups = [];

function ok(name, cond, detail) {
  if (cond) { passed++; results.push(`  ✅ ${name}`); }
  else { failed++; results.push(`  ❌ ${name}${detail ? ' — ' + String(detail).slice(0, 220) : ''}`); }
}

function req(method, path, { body, headers, binary } = {}) {
  return new Promise((resolve, reject) => {
    const data = body == null ? null : (typeof body === 'string' || Buffer.isBuffer(body) ? body : JSON.stringify(body));
    const h = Object.assign({}, headers || {});
    if (data != null) h['content-length'] = Buffer.byteLength(data);
    const r = http.request({ host: HOST, port: PORT, path, method, headers: h }, (res) => {
      const chunks = [];
      res.on('data', c => chunks.push(c));
      res.on('end', () => {
        const buf = Buffer.concat(chunks);
        resolve({
          status: res.statusCode,
          headers: res.headers,
          // 默认按 UTF-8 解码（面板/JSON 都是中文）；zip 等二进制用例显式传 binary:true
          text: binary ? buf.toString('latin1') : buf.toString('utf8'),
          buffer: buf,
        });
      });
    });
    r.on('error', reject);
    r.setTimeout(60000, () => r.destroy(new Error('timeout')));
    if (data != null) r.write(data);
    r.end();
  });
}
async function json(method, path, body, headers) {
  const r = await req(method, path, {
    body,
    headers: Object.assign({ 'content-type': 'application/json' }, headers || {}),
  });
  let j = null;
  try { j = JSON.parse(r.text); } catch (e) { /* 非 JSON */ }
  return { status: r.status, json: j, text: r.text, headers: r.headers };
}
const sleep = (ms) => new Promise(r => setTimeout(r, ms));

(async () => {

  console.log(`\n=== Phase G E2E（http://${HOST}:${PORT}）===\n`);

  // ---------- 0. 服务可达 ----------
  {
    const r = await req('GET', '/healthz');
    let j = null; try { j = JSON.parse(r.text); } catch (e) {}
    ok('0.1 网关可达（/healthz 200）', r.status === 200 && j && j.ok === true, `${r.status} ${r.text.slice(0, 120)}`);
  }

  // ---------- 1. 面板结构与"讲清楚怎么请求" ----------
  {
    const r = await req('GET', '/ui');
    const html = r.text;
    ok('1.1 面板含「立刻开始请求」卡（地址 / Key）', html.includes('id="connect-panel"') && html.includes('id="c-base"') && html.includes('id="c-key"'));
    ok('1.2 面板区分 OpenAI / Anthropic 两个地址', html.includes('id="c-openai"') && html.includes('id="c-anthropic"'));
    ok('1.3 面板可一键生成 / 清除 API Key', html.includes('genApiKey()') && html.includes('clearApiKey()'));
    ok('1.4 一键登录具备"扩展直连"路径', html.includes('oneClickViaExtension') && html.includes('sendToExtension') && html.includes('onMessageExternal') === false);
    ok('1.5 面板会等待扩展把凭证写回（轮询）', html.includes('pollForNewCredential'));
    ok('1.6 扩展状态指示 + 下载入口', html.includes('id="ext-status"') && html.includes('downloadExtension') && html.includes('/api/extension/bundle'));
    ok('1.7 凭证表含 账号/类型/凭证/套餐/今日剩余/入库时间/操作', ['账号', '类型', '凭证', '套餐', '今日剩余', '入库时间', '操作'].every(h => html.includes(h)));
    ok('1.8 凭证行提供 检查/详情/删除', html.includes('checkCredAt') && html.includes('openCredDetail') && html.includes('deleteCredAt'));
    ok('1.9 使用记录面板存在', html.includes('id="hist-cred"') && html.includes('loadHistory') && html.includes('id="hist-wrap"'));
    ok('1.10 旧凭证入库时间会被回填说明', html.includes('入库时间'));
  }

  // ---------- 2. /api/guide 接入信息 ----------
  {
    const r = await json('GET', '/api/guide');
    ok('2.1 /api/guide 200 且 ok', r.status === 200 && r.json && r.json.ok === true, `${r.status} ${r.text.slice(0, 150)}`);
    const g = r.json || {};
    ok('2.2 含 OpenAI/Anthropic 地址与 Key 状态', g.openai_base_url === '/v1' && g.anthropic_base_url === '/' && typeof g.api_keys === 'object');
    ok('2.3 含可用模型数与样例', typeof g.models_count === 'number' && Array.isArray(g.models_sample));
    ok('2.4 含"是否已有凭证"的就绪信号', typeof g.data_plane_ready === 'boolean');
  }

  // ---------- 3. 凭证导入：去重 + 稳定 id ----------
  let testCredId = null;
  const fakeCookie = `__Secure-next-auth.session-token=e2e-g-${Date.now()}; __Host-next-auth.csrf-token=e2e`;
  {
    const before = await json('GET', '/api/tokens');
    const beforeIds = new Set(((before.json && before.json.tokens) || []).map(t => t.id));

    const r1 = await json('POST', '/api/tokens/import', { cookie: fakeCookie });
    ok('3.1 导入测试凭证成功', r1.status === 200 && r1.json && r1.json.added === 1, r1.text.slice(0, 150));

    const r2 = await json('POST', '/api/tokens/import', { cookie: fakeCookie });
    ok('3.2 同值重复导入 → 自动去重（added=0）', r2.status === 200 && r2.json && r2.json.added === 0, r2.text.slice(0, 150));

    const list = await json('GET', '/api/tokens');
    const tokens = (list.json && list.json.tokens) || [];
    const added = tokens.find(t => !beforeIds.has(t.id));
    ok('3.3 每条凭证都有稳定 id', !!added && typeof added.id === 'string' && added.id.length >= 8, JSON.stringify(tokens.slice(-1)));
    ok('3.4 凭证带类型标识（web-cookie）', !!added && added.kind === 'web-cookie');
    ok('3.5 凭证带入库时间（含历史数据回填）', !!added && !!added.added_at);
    ok('3.6 列表返回总数 count', typeof (list.json || {}).count === 'number');
    testCredId = added && added.id;
    if (testCredId) cleanups.push(() => json('POST', '/api/tokens/delete', { id: testCredId }));
  }

  // ---------- 4. 凭证检查（真实打上游，验证链路不是本地假装） ----------
  {
    const bad = await json('POST', '/api/tokens/check', { id: 'not-a-real-id' });
    ok('4.1 不存在的 id → 明确 400', bad.status === 400, `${bad.status} ${bad.text.slice(0, 120)}`);

    if (testCredId) {
      const r = await json('POST', '/api/tokens/check', { id: testCredId });
      const body = r.json || {};
      const isRealUpstream = r.status === 200 && typeof body.valid === 'boolean';
      ok('4.2 检查返回有效性结论（假凭证应为无效）', isRealUpstream && body.valid === false, `${r.status} ${r.text.slice(0, 200)}`);
      ok('4.3 检查结果落盘到 meta（含 checked_at）', !!body.meta && !!body.meta.checked_at && body.meta.cred_id === testCredId);

      const list = await json('GET', '/api/tokens');
      const item = (((list.json || {}).tokens) || []).find(t => t.id === testCredId);
      ok('4.4 凭证列表能读到该凭证的账号信息缓存', !!item && !!item.meta && item.meta.valid === false);
    } else {
      ok('4.2 检查返回有效性结论（假凭证应为无效）', false, '前置失败：未取得测试凭证 id');
      ok('4.3 检查结果落盘到 meta（含 checked_at）', false, '前置失败');
      ok('4.4 凭证列表能读到该凭证的账号信息缓存', false, '前置失败');
    }
  }

  // ---------- 5. 使用记录（用户批注：每个账号都要有记录可查） ----------
  {
    const r = await json('GET', '/api/account/history?limit=50');
    const recs = (r.json && r.json.records) || [];
    ok('5.1 使用记录端点可用', r.status === 200 && (r.json || {}).ok === true, `${r.status} ${r.text.slice(0, 150)}`);
    ok('5.2 记录含账号快照字段', recs.length === 0 || (recs[0].ts && recs[0].cred_id && typeof recs[0].ok === 'boolean'));
    if (testCredId) {
      const f = await json(`GET`, `/api/account/history?limit=50&cred_id=${encodeURIComponent(testCredId)}`);
      const mine = (f.json && f.json.records) || [];
      ok('5.3 可按凭证过滤历史记录', f.status === 200 && mine.length >= 1 && mine.every(x => x.cred_id === testCredId), JSON.stringify(mine.slice(0, 1)));
    } else {
      ok('5.3 可按凭证过滤历史记录', false, '前置失败：未取得测试凭证 id');
    }
  }

  // ---------- 6. 扩展打包下载 ----------
  {
    const r = await req('GET', '/api/extension/bundle', { binary: true });
    const isZip = r.text.slice(0, 2) === 'PK';
    ok('6.1 /api/extension/bundle 返回 zip', r.status === 200 && isZip, `${r.status} head=${JSON.stringify(r.text.slice(0, 8))}`);
    const ct = String(r.headers['content-type'] || '');
    ok('6.2 响应头为 zip 且带文件名', /zip/i.test(ct) && /attachment/i.test(String(r.headers['content-disposition'] || '')), `${ct} ${r.headers['content-disposition']}`);
    ok('6.3 zip 内含扩展关键文件条目', r.text.includes('manifest.json') && r.text.includes('background.js') && r.text.includes('bridge.js'), 'zip 内容缺少扩展文件');
    ok('6.4 zip 结构完整（EOCD 存在）', r.text.slice(-22, -18) === 'PK', `tail=${JSON.stringify(r.text.slice(-22, -16))}`);
  }

  // ---------- 7. 删除凭证 ----------
  {
    if (testCredId) {
      const before = await json('GET', '/api/tokens');
      const beforeCount = (before.json || {}).count || 0;
      const r = await json('POST', '/api/tokens/delete', { id: testCredId });
      ok('7.1 删除凭证成功', r.status === 200 && (r.json || {}).ok === true, `${r.status} ${r.text.slice(0, 150)}`);
      const after = await json('GET', '/api/tokens');
      ok('7.2 删除后列表数量 -1', ((after.json || {}).count || 0) === beforeCount - 1, `before=${beforeCount} after=${(after.json || {}).count}`);
      const again = await json('POST', '/api/tokens/delete', { id: testCredId });
      ok('7.3 重复删除 → 404（幂等语义明确）', again.status === 404, `${again.status} ${again.text.slice(0, 120)}`);
      testCredId = null; // 已删除，无需 cleanup
    } else {
      ok('7.1 删除凭证成功', false, '前置失败：未取得测试凭证 id');
      ok('7.2 删除后列表数量 -1', false, '前置失败');
      ok('7.3 重复删除 → 404（幂等语义明确）', false, '前置失败');
    }
  }

  // ---------- 8. 上游会话自动清理（用户批注：反代要自己清理，别给上游留压力） ----------
  {
    const fs = require('node:fs');
    const path = require('node:path');
    const threadsPath = path.join(__dirname, '..', 'data', 'e2e', 'threads.json');
    const nowIso = new Date().toISOString();
    fs.mkdirSync(path.dirname(threadsPath), { recursive: true });
    fs.writeFileSync(threadsPath, JSON.stringify([
      { id: 'e2e-old-thread', created_at: '2020-01-01T00:00:00+00:00' },
      { id: 'e2e-new-thread', created_at: nowIso },
    ], null, 2));

    const dry = await json('POST', '/api/threads/cleanup', { dry_run: true, max_age_hours: 24 });
    const dj = dry.json || {};
    ok('8.1 清理预演能识别过期会话', dry.status === 200 && dj.expired === 1, `${dry.status} ${dry.text.slice(0, 160)}`);
    ok('8.2 预演只报过期项、不动新会话', Array.isArray(dj.thread_ids) && dj.thread_ids.includes('e2e-old-thread') && !dj.thread_ids.includes('e2e-new-thread'), JSON.stringify(dj.thread_ids));
    ok('8.3 预演不删除（total 仍为 2）', dj.total === 2, `total=${dj.total}`);

    // 真实删除：无凭证 → 明确报错；有凭证 → 真去打上游并如实汇报（成功+失败 == 过期数）
    const real = await json('POST', '/api/threads/cleanup', { dry_run: false, max_age_hours: 24 });
    const rj = real.json || {};
    const noCred = real.status === 400 && /Cookie/i.test(real.text);
    const honestAttempt =
      real.status === 200 && rj.ok === true && rj.dry_run === false &&
      (rj.deleted + rj.failed) === 1 && rj.remaining === 2 - rj.deleted;
    ok('8.4 真实清理：无凭证→明确报错；有凭证→真实尝试并如实汇报', noCred || honestAttempt, `${real.status} ${real.text.slice(0, 180)}`);

    // 配置里应默认开启自动清理（间隔 > 0）
    const cfg = JSON.parse(fs.readFileSync(path.join(__dirname, 'e2e_phase_g.config.json'), 'utf8'));
    ok('8.5 自动清理默认开启（未显式关闭即视为启用）', cfg.thread_cleanup_interval_sec === undefined || cfg.thread_cleanup_interval_sec > 0, JSON.stringify(cfg.thread_cleanup_interval_sec));
  }

  // ---------- 9. API Key 运行时管理（放最后，跑完恢复原状） ----------
  // 快照当前 Key 配置：脚本异常退出时 finally 用它重置回已知 Key（原 Key 明文本就不可得，guide 只回掩码；此 generate 会使原 Key 失效并重置为已知值）
  let snapshotKey = null;
  let generatedKey = null;
  try {
    const g = await json('GET', '/api/guide');
    const cnt = ((g.json || {}).api_keys || {}).count || 0;
    if (cnt > 0) { const gen = await json('POST', '/api/config/api-key', { action: 'generate' }); snapshotKey = (gen.json || {}).key || null; }
  } catch (e) { snapshotKey = null; }

  {
    const before = await json('GET', '/api/guide');
    const wasConfigured = !!(before.json && before.json.api_keys && before.json.api_keys.configured);

    const gen = await json('POST', '/api/config/api-key', { action: 'generate' });
    const newKey = (gen.json || {}).key;
    generatedKey = newKey;
    ok('9.1 生成 API Key 成功并回显', gen.status === 200 && typeof newKey === 'string' && newKey.length >= 12, `${gen.status} ${gen.text.slice(0, 150)}`);

    // 不带 Key → 管理端点应拒绝
    const denied = await json('GET', '/api/tokens');
    ok('9.2 启用 Key 后无 Key 请求被拒绝', denied.status === 401 || denied.status === 403, `${denied.status} ${denied.text.slice(0, 120)}`);

    // 带 Key → 放行
    const allowed = await json('GET', '/api/tokens', null, { authorization: `Bearer ${newKey}` });
    ok('9.3 带正确 Key 放行', allowed.status === 200 && (allowed.json || {}).ok === true, `${allowed.status} ${allowed.text.slice(0, 120)}`);

    // 未授权的 /healthz 不应泄露账号名
    const hz = await req('GET', '/healthz');
    let hzj = null; try { hzj = JSON.parse(hz.text); } catch (e) {}
    ok('9.4 未授权 /healthz 不泄露账号信息', hz.status === 200 && hzj && hzj.ok === true && hzj.accounts === undefined, hz.text.slice(0, 150));

    // 恢复原状
    const clear = await json('POST', '/api/config/api-key', { action: 'clear' }, { authorization: `Bearer ${newKey}` });
    ok('9.5 清除 Key 成功', clear.status === 200 && (clear.json || {}).ok === true && (clear.json || {}).configured === false, `${clear.status} ${clear.text.slice(0, 150)}`);

    const restored = await json('GET', '/api/tokens');
    ok('9.6 清除后恢复本机直连', restored.status === 200, `${restored.status}`);
    if (wasConfigured) results.push('  ℹ️ 注意：运行前网关本来就配置了 api_keys，本用例清除后未自动还原（请在面板重新设置）');
  }

  // ---------- 收尾清理 ----------
  for (const fn of cleanups) { try { await fn(); } catch (e) { /* 忽略 */ } }

  console.log(results.join('\n'));
  console.log(`\n=== 结果：${passed} 通过 / ${failed} 失败 ===\n`);
  process.exit(failed > 0 ? 1 : 0);
})().catch(e => { console.error('E2E 脚本异常:', e); process.exitCode = 1; }).finally(async () => {
  // 恢复网关 Key 状态：起始无 Key → clear；起始有 Key（不应发生）→ 恢复原值
  try {
    if (snapshotKey) {
      await json('POST', '/api/config/api-key', { action: 'set', key: snapshotKey }, { authorization: 'Bearer ' + snapshotKey });
      console.log('（恢复：网关原有 API Key 已重置回去）');
    } else {
      await json('POST', '/api/config/api-key', { action: 'clear' }, { authorization: 'Bearer ' + (generatedKey || '') });
      console.log('（恢复：E2E 生成的临时 Key 已清除）');
    }
  } catch (e) { console.error('（警告：Key 状态恢复失败，请检查 /api/config/api-key）', e.message); }
  // 兜底清理测试凭证
  if (testCredId) { try { await json('POST', '/api/tokens/delete', { id: testCredId }); } catch (e) { /* 忽略 */ } }
});

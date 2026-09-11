// Freebuff2API 一键登录扩展（MV3）
//
// 四条能力：
//   1) 面板 → 扩展直连：/ui 面板通过 chrome.runtime.sendMessage(扩展ID, {type:'freebuff2api.import'})
//      触发导入；扩展 ID 由 bridge.js（content script）postMessage 告知面板。
//   2) 自动导航 + 等待登录：未登录时自动打开 freebuff.com，每 2 秒轮询 session-token（最长 180 秒），
//      登录成功立即自动导入。
//   3) 点击扩展图标 = 同一条流程（读得到就直接导入，读不到就走自动导航等待）。
//   4) 多端口探测：面板端口 > 选项页端口 > 默认端口列表；只有连接层失败才换端口，
//      HTTP 有响应（含 4xx/5xx）就停在该端口并报告错误。
//   5) 网关 API Key：面板传入 > 选项页保存；有 Key 时带 authorization: Bearer 头，收到 401 明确提示。
//
// 安全边界：只读取 freebuff.com 域下的 Cookie（HttpOnly 的 session-token 只有扩展读得到），
// 只发送到本机 127.0.0.1，不接触账号密码。

const DEFAULT_PORTS = [47821, 47822, 8787];
const SESSION_COOKIE = '__Secure-next-auth.session-token';
const FREEBUFF_URL = 'https://freebuff.com/';
const COOKIE_URL = 'https://freebuff.com';
const FREEBUFF_TAB_MATCH = 'https://freebuff.com/*';
const POLL_INTERVAL_MS = 2000;
const LOGIN_TIMEOUT_MS = 180000;
const FETCH_TIMEOUT_MS = 8000;
const RESPONSE_GUARD_MS = 1500;

// ---------- 基础工具 ----------

/** 系统通知；任何失败（权限被关/图标缺失）都静默，不影响导入主流程 */
function notify(title, message) {
  try {
    chrome.notifications.create(
      {
        type: 'basic',
        iconUrl: chrome.runtime.getURL('icons/icon128.png'),
        title: String(title || 'Freebuff2API'),
        message: String(message || ''),
      },
      function () { void chrome.runtime.lastError; } // 必须读取，否则控制台报未检查错误
    );
  } catch (e) { /* 通知不可用时静默 */ }
}

/** 回调式 chrome API → Promise：强制消费 lastError，失败/异常一律返回 null，绝不抛未捕获异常 */
function chromeCall(invoke) {
  return new Promise(function (resolve) {
    try {
      invoke(function (result) {
        const err = chrome.runtime.lastError;
        resolve(err ? null : result);
      });
    } catch (e) {
      resolve(null);
    }
  });
}

function sleep(ms) {
  return new Promise(function (r) { setTimeout(r, ms); });
}

function normalizePort(v) {
  const n = Number(v);
  return Number.isInteger(n) && n > 0 && n <= 65535 ? n : 0;
}

/** API Key 规整：字符串、去换行（防头注入）、限长；无效/空返回 ''（表示不发送 Authorization 头） */
function normalizeApiKey(v) {
  if (typeof v !== 'string') return '';
  const s = v.replace(/[\r\n]/g, '').trim();
  return s.length > 0 && s.length <= 512 ? s : '';
}

/** 回退来源：选项页保存的 API Key */
async function storedApiKey() {
  const r = await chromeCall(function (cb) {
    chrome.storage.local.get('apiKey', cb);
  });
  return normalizeApiKey(r && r.apiKey);
}

// ---------- Cookie ----------

/** 读取 freebuff.com 全部 Cookie 并拼成 Cookie 头（含 HttpOnly） */
async function readFreebuffCookie() {
  const cookies = (await chromeCall(function (cb) {
    chrome.cookies.getAll({ url: COOKIE_URL }, cb);
  })) || [];
  if (cookies.length === 0) return { ok: false, reason: 'no_cookie' };
  const str = cookies
    .filter(function (c) { return c && c.value; })
    .map(function (c) { return c.name + '=' + c.value; })
    .join('; ');
  if (str.indexOf(SESSION_COOKIE) === -1) return { ok: false, reason: 'not_logged_in' };
  return { ok: true, cookie: str, count: cookies.length };
}

/** 是否已有登录会话（毫秒级探测，用于 onMessageExternal 的 needLogin 字段） */
async function hasSessionCookie() {
  const c = await chromeCall(function (cb) {
    chrome.cookies.get({ url: COOKIE_URL, name: SESSION_COOKIE }, cb);
  });
  return !!(c && c.value);
}

// ---------- 端口探测 ----------

/** 候选端口：面板传入 > 选项页设置 > 默认列表，去重 */
async function candidatePorts(panelPort) {
  const list = [];
  const push = function (p) {
    const n = normalizePort(p);
    if (n && list.indexOf(n) === -1) list.push(n);
  };
  push(panelPort);
  const stored = await chromeCall(function (cb) {
    chrome.storage.local.get('gatewayPort', cb);
  });
  push(stored && stored.gatewayPort);
  DEFAULT_PORTS.forEach(push);
  return list;
}

// ---------- 导入 ----------

/**
 * POST 到指定端口。
 * reached=false 仅代表连接层失败（拒绝连接/超时/中断），此时才应该换下一个端口；
 * 一旦收到 HTTP 响应（无论状态码），都视为「到达网关」。
 */
async function postToGateway(port, cookie, apiKey) {
  const ctrl = new AbortController();
  const timer = setTimeout(function () { ctrl.abort(); }, FETCH_TIMEOUT_MS);
  try {
    const headers = { 'content-type': 'application/json' };
    if (apiKey) headers.authorization = 'Bearer ' + apiKey; // 网关未配置 api_keys 时不加此头
    const resp = await fetch('http://127.0.0.1:' + port + '/api/tokens/import', {
      method: 'POST',
      headers: headers,
      body: JSON.stringify({ cookie: cookie }),
      signal: ctrl.signal,
    });
    const text = await resp.text();
    let body = null;
    try { body = JSON.parse(text); } catch (e) { /* 非 JSON 响应 */ }
    return { reached: true, httpOk: resp.ok, status: resp.status, body: body };
  } catch (e) {
    return { reached: false, error: String((e && e.message) || e) };
  } finally {
    clearTimeout(timer);
  }
}

async function importCookieToGateway(cookie, panelPort, panelApiKey) {
  const apiKey = normalizeApiKey(panelApiKey) || (await storedApiKey()); // 面板优先，回退选项页
  const ports = await candidatePorts(panelPort);
  let lastErr = '';
  for (const port of ports) {
    const r = await postToGateway(port, cookie, apiKey);
    if (!r.reached) { lastErr = r.error || '连接失败'; continue; } // 仅连接失败才换端口
    if (r.httpOk && r.body && r.body.ok) {
      return { ok: true, port: port, added: Number(r.body.added) || 0, message: r.body.message || '' };
    }
    const msg = (r.body && (r.body.message || r.body.error)) || ('HTTP ' + r.status);
    return { ok: false, port: port, error: String(msg), unauthorized: r.status === 401 }; // HTTP 有响应：停在此端口报告
  }
  return { ok: false, port: 0, error: lastErr || '连接本地网关失败（连接被拒绝）' };
}

function notifyImportResult(res) {
  if (res.ok) {
    if (res.added > 0) {
      notify(
        '✅ 已自动导入 ' + res.added + ' 个凭证',
        '来源：freebuff.com 登录 Cookie（网关端口 ' + res.port + '）。回到面板点「刷新」即可看到账号全貌。'
      );
    } else {
      notify('✅ 凭证已存在，无需重复导入', '同值自动去重（网关端口 ' + res.port + '）。');
    }
    return;
  }
  if (res.unauthorized) {
    notify(
      '🔑 网关已启用 API Key 校验',
      '请在扩展选项页填入面板显示的 Key（右键扩展图标 →「选项」→ 网关 API Key），然后重试。网关端口 ' + res.port + '。'
    );
    return;
  }
  if (res.port) {
    notify('导入失败（端口 ' + res.port + '）', String(res.error).slice(0, 180));
  } else {
    notify(
      '连接本地网关失败',
      '请确认 Freebuff2API 已启动（默认端口 47821）。若改过端口：右键扩展图标 →「选项」填写，' +
        '或直接在网关面板点「一键登录」（面板会自动带上自己的端口）。' +
        (res.error ? ' 最后错误：' + String(res.error).slice(0, 120) : '')
    );
  }
}

// ---------- 自动导航 + 等待登录 ----------

/** 已有 freebuff.com 标签页则聚焦复用，否则新开登录页 */
async function openOrFocusFreebuff() {
  const tabs = await chromeCall(function (cb) {
    chrome.tabs.query({ url: FREEBUFF_TAB_MATCH }, cb);
  });
  if (tabs && tabs.length > 0) {
    const tab = tabs[0];
    const updated = await chromeCall(function (cb) {
      chrome.tabs.update(tab.id, { active: true }, cb);
    });
    if (updated && typeof tab.windowId === 'number' && tab.windowId >= 0) {
      await chromeCall(function (cb) {
        chrome.windows.update(tab.windowId, { focused: true }, cb);
      });
    }
    if (updated) return updated;
  }
  const created = await chromeCall(function (cb) {
    chrome.tabs.create({ url: FREEBUFF_URL }, cb);
  });
  if (!created) {
    notify('无法打开 freebuff.com', '请手动打开 https://freebuff.com/ 完成登录，登录后点扩展图标或面板「一键登录」即可导入。');
  }
  return created;
}

/** 每 2 秒轮询 session-token，最长 timeoutMs；检测到即返回 true */
async function waitForSessionCookie(timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const c = await chromeCall(function (cb) {
      chrome.cookies.get({ url: COOKIE_URL, name: SESSION_COOKIE }, cb);
    });
    if (c && c.value) return true;
    await sleep(POLL_INTERVAL_MS);
  }
  return false;
}

// ---------- 主流程（同一时间只跑一条，重复触发复用进行中的流程） ----------

let runningFlow = null;
let flowPanelPort = 0; // 进行中的流程读取最新面板端口：等待登录期间面板再次触发也能用上
let flowApiKey = '';   // 同上：面板传入的 API Key 优先，流程中途加入也能生效

function startFlow(panelPort, panelApiKey) {
  const p = normalizePort(panelPort);
  if (p) flowPanelPort = p;
  const k = normalizeApiKey(panelApiKey);
  if (k) flowApiKey = k;
  if (runningFlow) return runningFlow;
  runningFlow = (async function () {
    try {
      const r = await readFreebuffCookie();
      if (r.ok) {
        const res = await importCookieToGateway(r.cookie, flowPanelPort, flowApiKey);
        notifyImportResult(res);
        return res;
      }
      // 未登录：自动打开 freebuff.com 并等待登录
      notify(
        '已打开 freebuff.com，请完成 GitHub 登录',
        '登录成功后会自动把凭证导入本地网关，无需手动复制。'
      );
      await openOrFocusFreebuff();
      const got = await waitForSessionCookie(LOGIN_TIMEOUT_MS);
      if (!got) {
        notify('⏱ 等待登录超时', '已等待 3 分钟仍未检测到登录凭证。完成登录后，再点一次扩展图标或面板「一键登录」即可立即导入。');
        return { ok: false, port: 0, error: 'login_timeout' };
      }
      const after = await readFreebuffCookie();
      if (!after.ok) return { ok: false, port: 0, error: after.reason };
      const res = await importCookieToGateway(after.cookie, flowPanelPort, flowApiKey);
      notifyImportResult(res);
      return res;
    } catch (e) {
      notify('导入流程异常', String((e && e.message) || e).slice(0, 180));
      return { ok: false, port: 0, error: 'unexpected_error' };
    } finally {
      // 流程结束（含异常），允许下一次触发启动新流程；本身不会 reject
      runningFlow = null;
      flowPanelPort = 0;
      flowApiKey = '';
    }
  })();
  return runningFlow;
}

// ---------- 面板 → 扩展直连 ----------

function senderOrigin(sender) {
  try {
    if (sender && sender.origin) return String(sender.origin);
    if (sender && sender.url) return new URL(sender.url).origin;
  } catch (e) { /* 忽略 */ }
  return '';
}

/** 只放行本机网关面板（http://127.0.0.1[:端口] 或 http://localhost[:端口]） */
function isLocalPanelOrigin(origin) {
  return origin === 'http://127.0.0.1' || origin.indexOf('http://127.0.0.1:') === 0 ||
         origin === 'http://localhost' || origin.indexOf('http://localhost:') === 0;
}

chrome.runtime.onMessageExternal.addListener(function (msg, sender, sendResponse) {
  const origin = senderOrigin(sender);
  if (!isLocalPanelOrigin(origin)) {
    try { sendResponse({ ok: false, error: 'forbidden_origin', message: '来源不在允许范围（仅本机网关面板可用）' }); } catch (e) { /* 通道已关 */ }
    return false;
  }
  if (!msg || msg.type !== 'freebuff2api.import') {
    try { sendResponse({ ok: false, error: 'unknown_type', message: '未知的消息类型' }); } catch (e) { /* 通道已关 */ }
    return false;
  }
  const panelPort = normalizePort(msg.gatewayPort);
  const panelApiKey = normalizeApiKey(msg.apiKey);
  (async function () {
    // 只等一次毫秒级登录态探测（带 1.5s 保险），绝不等长达 3 分钟的登录/导入流程
    const loggedIn = (await Promise.race([
      hasSessionCookie(),
      sleep(RESPONSE_GUARD_MS).then(function () { return null; }),
    ])) === true;
    // 同步完成路径：已登录时直接把导入跑完，第一个响应就带 done/added，面板无需再轮询。
    // guard = 1.5s 登录探测 + 3s 流程兜底：超时说明网关极慢/多端口探测，回退 started 由面板轮询收敛。
    if (loggedIn) {
      let doneRes = null;
      try {
        doneRes = await Promise.race([
          startFlow(panelPort, panelApiKey),
          sleep(RESPONSE_GUARD_MS * 2).then(function () { return null; }),
        ]);
      } catch (e) { /* 极端情况：回退 started */ }
      if (doneRes && doneRes.ok) {
        try {
          sendResponse({
            ok: true,
            phase: 'done',
            done: true,
            added: doneRes.added || 0,
            needLogin: false,
          });
        } catch (e) { /* 面板已离开 */ }
        return;
      }
      // doneRes 为 null（超时）或失败：失败结果的通知已由流程发出，这里回退 started。
      // 流程已在同步路径启动过（跑完或失败），不再重复 startFlow（否则端口序列会跑两遍），
      // 面板按 started 轮询只会看到凭证数量不变，自然收敛。
      try {
        sendResponse({ ok: true, phase: 'started', needLogin: false });
      } catch (e) { /* 面板已离开 */ }
      return;
    }
    // 未登录：立即响应并启动完整流程（自动导航 → 等待登录 → 自动导入）
    try {
      sendResponse({ ok: true, phase: 'started', needLogin: true });
    } catch (e) { /* 面板已离开，流程照跑 */ }
    startFlow(panelPort, panelApiKey);
  })();
  return true; // 通道保持到上面 sendResponse 被调用（毫秒级）
});

// ---------- 点击扩展图标：同一条流程（API Key 走选项页回退） ----------

chrome.action.onClicked.addListener(function () {
  startFlow(0, '');
});

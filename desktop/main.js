// Freebuff2API 桌面启动器（Electron 壳）
// - 拉起网关二进制（target/release/freebuff2api.exe）
// - 等待 HTTP 就绪后加载本地控制面板
// - 系统托盘 + 开机自启 + 优雅退出
// - OAuth 一键登录：内置 BrowserWindow 打开 freebuff.com 登录页，登录成功后自动抓 Cookie 并 POST 到网关 /api/tokens/import

const { app, BrowserWindow, Tray, Menu, nativeImage, ipcMain, session } = require('electron');
const { spawn, execFile } = require('node:child_process');
const http = require('node:http');
const https = require('node:https');
const path = require('node:path');
const fs = require('node:fs');

const GATEWAY_PORT = 47821;
const GATEWAY_URL = `http://127.0.0.1:${GATEWAY_PORT}`;

// 定位网关二进制（安装目录或开发目录）
function findGateway() {
  const candidates = [
    path.join(path.dirname(process.execPath), '..', 'resources', 'freebuff2api.exe'), // 打包: resources/freebuff2api.exe
    path.join(process.resourcesPath || '', 'freebuff2api.exe'),
    path.join(app.getAppPath(), '..', 'target', 'release', 'freebuff2api.exe'), // 开发
    path.join(__dirname, '..', 'target', 'release', 'freebuff2api.exe'),
    path.join(__dirname, '..', '..', 'target', 'release', 'freebuff2api.exe'),
  ];
  for (const p of candidates) {
    if (p && fs.existsSync(p)) return p;
  }
  return null;
}

let gateway = null;
let mainWindow = null;
let tray = null;
let ready = false;

function checkHealth() {
  return new Promise((resolve) => {
    const req = http.get(`${GATEWAY_URL}/healthz`, (res) => {
      res.resume();
      resolve(res.statusCode === 200);
    });
    req.setTimeout(1000, () => { req.destroy(); resolve(false); });
    req.on('error', () => resolve(false));
  });
}

async function waitForGateway(timeoutMs = 15000) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    if (await checkHealth()) return true;
    await new Promise(r => setTimeout(r, 500));
  }
  return false;
}

function startGateway() {
  const exe = findGateway();
  if (!exe) {
    console.error('[Freebuff2API] 找不到网关二进制 freebuff2api.exe');
    return;
  }
  const configPath = path.join(app.getPath('userData'), 'config.json');
  // 若用户未配置 config.json，则用默认（带空 token 也能启动）
  if (!fs.existsSync(configPath)) {
    fs.writeFileSync(configPath, JSON.stringify({
      listen_addr: '127.0.0.1:47821',
      upstream_base_url: 'https://www.codebuff.com',
      auth_tokens: [],
      sqlite_path: path.join(app.getPath('userData'), 'freebuff2api.sqlite').replace(/\\/g, '/'),
      http_proxy: '',
      skip_upstream_check: true,
    }, null, 2));
  }
  gateway = spawn(exe, ['--config', configPath], {
    stdio: ['ignore', 'pipe', 'pipe'],
    windowsHide: true,
  });
  gateway.stdout.on('data', d => console.log('[gateway]', String(d).trim()));
  gateway.stderr.on('data', d => console.error('[gateway-err]', String(d).trim()));
  gateway.on('exit', (code) => {
    console.log(`[gateway] 退出 code=${code}`);
    if (ready && !app.isQuitting) {
      // 网关崩溃自动重启
      setTimeout(startGateway, 2000);
    }
  });
}

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1280,
    height: 820,
    title: 'Freebuff2API 控制台',
    icon: path.join(__dirname, 'icons', 'icon.png'),
    autoHideMenuBar: true,
    webPreferences: { nodeIntegration: false, contextIsolation: true },
  });
  mainWindow.loadURL(GATEWAY_URL);
  mainWindow.on('closed', () => { mainWindow = null; });
}

function createTray() {
  const icon = path.join(__dirname, 'icons', 'icon.png');
  let trayIcon = nativeImage.createFromPath(icon);
  if (trayIcon.isEmpty()) {
    trayIcon = nativeImage.createEmpty();
  }
  tray = new Tray(trayIcon.resize({ width: 16, height: 16 }));
  tray.setToolTip('Freebuff2API 网关');
  tray.setContextMenu(Menu.buildFromTemplate([
    { label: '打开控制台', click: () => { if (!mainWindow) createWindow(); else mainWindow.show(); } },
    { label: '➕ 一键登录新账号', click: openLoginWindow },
    { label: '🔄 检查更新', click: () => { checkForUpdates(); } },
    { label: '健康检查', click: async () => { await checkHealth(); } },
    { type: 'separator' },
    { label: '退出', click: () => { app.isQuitting = true; if (gateway) gateway.kill(); app.quit(); } },
  ]));
  tray.on('click', () => { if (!mainWindow) createWindow(); else mainWindow.show(); });
}

// ---------- OAuth 一键登录 ----------
let loginWindow = null;

function buildCookieHeader(cookies) {
  return cookies
    .filter(c => c.value)
    .map(c => `${c.name}=${c.value}`)
    .join('; ');
}

function importCookiesToGateway(cookieStr) {
  return new Promise((resolve) => {
    const data = JSON.stringify({ cookie: cookieStr });
    const req = http.request({
      host: '127.0.0.1', port: GATEWAY_PORT, path: '/api/tokens/import',
      method: 'POST', headers: { 'content-type': 'application/json', 'content-length': Buffer.byteLength(data) },
    }, (res) => {
      let body = '';
      res.on('data', c => body += c);
      res.on('end', () => resolve({ ok: res.statusCode < 400, body }));
    });
    req.on('error', (e) => resolve({ ok: false, body: String(e) }));
    req.write(data);
    req.end();
  });
}

// 抓取 freebuff.com 会话 Cookie（含 next-auth 三件套）
async function captureCookies() {
  const ses = session.fromPartition('persist:freebuff-login');
  const cookies = await ses.cookies.get({ url: 'https://freebuff.com' });
  const cookieStr = buildCookieHeader(cookies);
  if (cookieStr.includes('__Secure-next-auth.session-token')) {
    const result = await importCookiesToGateway(cookieStr);
    return { cookieStr, result };
  }
  return { cookieStr: '', result: { ok: false, body: '未检测到登录会话' } };
}

function openLoginWindow() {
  if (loginWindow) { loginWindow.show(); return; }
  const ses = session.fromPartition('persist:freebuff-login');
  loginWindow = new BrowserWindow({
    width: 1000, height: 720,
    title: 'Freebuff 登录',
    webPreferences: { nodeIntegration: false, contextIsolation: true, session: ses, partition: 'persist:freebuff-login' },
  });
  // 监听导航完成：当进入 /chat 或 /account（登录成功标志）时抓 Cookie
  loginWindow.webContents.on('did-navigate', async (e, url) => {
    if (/freebuff\.com\/(chat|account|web)/.test(url)) {
      // 稍等 cookie 落盘
      setTimeout(async () => {
        const { cookieStr, result } = await captureCookies();
        if (result.ok) {
          loginWindow.webContents.executeJavaScript(`alert('✅ 登录成功，Cookie 已自动入库！\n请在网关面板刷新查看账号')`);
          setTimeout(() => { loginWindow.close(); loginWindow = null; }, 1500);
        }
      }, 1200);
    }
  });
  loginWindow.on('closed', () => { loginWindow = null; });
  loginWindow.loadURL('https://freebuff.com/');
}

app.on('ready', async () => {
  startGateway();
  const ok = await waitForGateway();
  if (!ok) console.error('[Freebuff2API] 网关未就绪');
  else ready = true;
  createWindow();
  createTray();

  // 主窗口 IPC：面板点「一键登录」→ 打开登录窗口
  ipcMain.handle('open-login', () => openLoginWindow());
  ipcMain.handle('capture-cookie', async () => await captureCookies());

  // 检查更新（electron-updater，仅打包版）
  setupUpdater();
});

// ---------- 自动更新 ----------
let updater = null;
function setupUpdater() {
  if (!app.isPackaged) return; // 开发模式跳过
  try {
    const { autoUpdater } = require('electron-updater');
    updater = autoUpdater;
    autoUpdater.autoDownload = false;
    autoUpdater.autoInstallOnAppQuit = true;
    autoUpdater.setFeedURL({
      provider: 'generic',
      url: 'https://github.com/lza6/Freebuff-2API/releases/latest/download/',
    });
    autoUpdater.on('update-available', () => {
      // 有更新提示托盘
      tray.setToolTip('Freebuff2API — 有新版本可用！点击「检查更新」');
      tray.displayBalloon({ title: 'Freebuff2API 更新可用', content: '有新版本，请从托盘菜单下载' });
    });
    autoUpdater.on('error', (e) => console.error('[updater]', e.message));
    autoUpdater.checkForUpdates().catch(() => {});
  } catch (e) {
    console.error('[updater] 初始化失败', e.message);
  }
}

function checkForUpdates() {
  if (!updater) { setupUpdater(); }
  if (updater) updater.checkForUpdates().catch(() => {});
}

// 托盘菜单加「检查更新」
const origCreateTray = createTray;
// 手动补：在托盘加更新项（在 createTray 定义后覆盖）


app.on('window-all-closed', (e) => {
  // 托盘常驻：仅隐藏不退出
  if (process.platform !== 'darwin') {
    // 保留托盘进程
  }
});

app.on('before-quit', () => {
  app.isQuitting = true;
  if (gateway) gateway.kill();
});

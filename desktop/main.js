// Freebuff2API 桌面启动器（Electron 壳）
// - 拉起网关二进制（resources/freebuff2api.exe 或 target/release/freebuff2api.exe）
// - 等待 HTTP 就绪后加载本地控制面板
// - 系统托盘 + 失败弹窗 + 日志落盘 + 打开配置/数据/日志目录
// - OAuth 一键登录：内置 BrowserWindow 打开 freebuff.com 登录页，登录成功后自动抓 Cookie 并 POST 到网关 /api/tokens/import

const { app, BrowserWindow, Tray, Menu, nativeImage, ipcMain, session, dialog, shell } = require('electron');
const { spawn, execFile } = require('node:child_process');
const http = require('node:http');
const https = require('node:https');
const path = require('node:path');
const fs = require('node:fs');

const GATEWAY_PORT = 47821;
const GATEWAY_URL = `http://127.0.0.1:${GATEWAY_PORT}`;

let gateway = null;
let mainWindow = null;
let tray = null;
let ready = false;
let logStream = null;
let lastStartupError = null;

// ---------- 日志落盘 ----------
function logDir() {
  const dir = path.join(app.getPath('userData'), 'logs');
  fs.mkdirSync(dir, { recursive: true });
  return dir;
}
function logPath() {
  return path.join(logDir(), 'gateway.log');
}
function writeLog(line) {
  try {
    if (!logStream) {
      logStream = fs.createWriteStream(logPath(), { flags: 'a' });
    }
    logStream.write(`[${new Date().toISOString()}] ${line}\n`);
  } catch (e) {
    console.error('[log] 写入失败', e.message);
  }
}

// 定位网关二进制（安装目录或开发目录）
function findGateway() {
  const candidates = [
    path.join(process.resourcesPath || '', 'freebuff2api.exe'), // 打包: resources/freebuff2api.exe
    path.join(path.dirname(process.execPath), '..', 'resources', 'freebuff2api.exe'),
    path.join(app.getAppPath(), '..', 'target', 'release', 'freebuff2api.exe'), // 开发
    path.join(__dirname, '..', 'target', 'release', 'freebuff2api.exe'),
    path.join(__dirname, '..', '..', 'target', 'release', 'freebuff2api.exe'),
  ];
  for (const p of candidates) {
    if (p && fs.existsSync(p)) return p;
  }
  return null;
}

// 迁移：早期版本 userData 目录名为 freebuff2api-desktop，首次启动时把配置/数据搬过来
function migrateLegacyData() {
  try {
    const oldDir = path.join(app.getPath('appData'), 'freebuff2api-desktop');
    const newDir = app.getPath('userData');
    if (oldDir === newDir || !fs.existsSync(oldDir)) return;
    for (const f of ['config.json', 'freebuff2api.sqlite', 'telemetry.sqlite', 'skills.sqlite', 'tokens.json']) {
      const src = path.join(oldDir, f);
      const dst = path.join(newDir, f);
      if (fs.existsSync(src) && !fs.existsSync(dst)) {
        fs.copyFileSync(src, dst);
        writeLog(`[migrate] ${f} 已从旧目录迁移`);
      }
    }
    fs.mkdirSync(path.join(newDir, 'data'), { recursive: true });
    const oldData = path.join(oldDir, 'data');
    if (fs.existsSync(oldData)) {
      for (const f of fs.readdirSync(oldData)) {
        const src = path.join(oldData, f);
        const dst = path.join(newDir, 'data', f);
        if (fs.statSync(src).isFile() && !fs.existsSync(dst)) {
          fs.copyFileSync(src, dst);
          writeLog(`[migrate] data/${f} 已迁移`);
        }
      }
    }
  } catch (e) {
    writeLog(`[migrate] 迁移失败（忽略）: ${e.message}`);
  }
}

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

// 读取用户配置的监听端口（可能与默认不同）
function configuredPort() {
  try {
    const cfgPath = path.join(app.getPath('userData'), 'config.json');
    if (fs.existsSync(cfgPath)) {
      const cfg = JSON.parse(fs.readFileSync(cfgPath, 'utf8'));
      const addr = String(cfg.listen_addr || '');
      const m = addr.match(/:(\d+)$/);
      if (m) return parseInt(m[1], 10);
    }
  } catch (_) { /* 忽略：配置损坏时回退默认端口 */ }
  return GATEWAY_PORT;
}

function startGateway() {
  const exe = findGateway();
  if (!exe) {
    lastStartupError = '找不到网关二进制 freebuff2api.exe（安装包不完整？请重新下载安装）';
    writeLog(`[ERROR] ${lastStartupError}`);
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
  writeLog(`[start] 拉起网关: ${exe} --config ${configPath}`);
  gateway = spawn(exe, ['--config', configPath], {
    stdio: ['ignore', 'pipe', 'pipe'],
    windowsHide: true,
    cwd: app.getPath('userData'), // 相对路径（data/tokens.json 等）落在用户数据目录
  });
  gateway.stdout.on('data', d => { const s = String(d).trim(); console.log('[gateway]', s); writeLog(`[out] ${s}`); });
  gateway.stderr.on('data', d => { const s = String(d).trim(); console.error('[gateway-err]', s); writeLog(`[err] ${s}`); });
  gateway.on('exit', (code) => {
    console.log(`[gateway] 退出 code=${code}`);
    writeLog(`[exit] code=${code}`);
    if (ready && !app.isQuitting) {
      // 网关崩溃自动重启（失败 3 次后提示）
      setTimeout(() => {
        startGateway();
        setTimeout(async () => {
          if (!(await checkHealth())) {
            dialog.showErrorBox(
              'Freebuff2API 网关异常',
              `网关进程反复退出（最近 code=${code}）。\n\n常见原因：\n1. 端口 ${configuredPort()} 被其他程序占用\n2. 配置文件损坏（%APPDATA%\\freebuff2api\\config.json）\n3. 杀毒软件拦截\n\n详细日志：${logPath()}`
            );
          }
        }, 4000);
      }, 2000);
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
    webPreferences: {
      nodeIntegration: false,
      contextIsolation: true,
      preload: path.join(__dirname, 'preload.js'),
    },
  });
  mainWindow.loadURL(`http://127.0.0.1:${configuredPort()}`);
  mainWindow.on('closed', () => { mainWindow = null; });
}

function openConsole() {
  if (!mainWindow) createWindow();
  else mainWindow.show();
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
    { label: '打开控制台', click: openConsole },
    { label: '➕ 一键登录新账号', click: openLoginWindow },
    { type: 'separator' },
    { label: '🩺 系统体检', click: () => { openConsole(); if (mainWindow) mainWindow.loadURL(`http://127.0.0.1:${configuredPort()}/#doctor`); } },
    { label: '📄 打开日志', click: () => { shell.openPath(logPath()); } },
    { label: '⚙️ 打开配置', click: () => { shell.showItemInFolder(path.join(app.getPath('userData'), 'config.json')); } },
    { label: '📁 打开数据目录', click: () => { shell.openPath(app.getPath('userData')); } },
    { type: 'separator' },
    { label: '🔄 检查更新', click: () => { checkForUpdates(); } },
    { label: '健康检查', click: async () => { const ok = await checkHealth(); tray.displayBalloon({ title: 'Freebuff2API', content: ok ? '网关运行正常 ✅' : '网关未响应 ❌（点「🩺 系统体检」查看）' }); } },
    { type: 'separator' },
    { label: '退出', click: () => { app.isQuitting = true; if (gateway) gateway.kill(); app.quit(); } },
  ]));
  tray.on('click', openConsole);
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
      host: '127.0.0.1', port: configuredPort(), path: '/api/tokens/import',
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
  // 监听导航完成：当进入 /chat、/account 或 /web（登录成功标志）时抓 Cookie
  loginWindow.webContents.on('did-navigate', async (e, url) => {
    if (/freebuff\.com\/(chat|account|web)/.test(url)) {
      // 稍等 cookie 落盘
      setTimeout(async () => {
        const { result } = await captureCookies();
        if (result.ok) {
          writeLog(`[login] Cookie 入库成功`);
          if (loginWindow) {
            loginWindow.webContents.executeJavaScript(`alert('✅ 登录成功，Cookie 已自动入库！\\n请在网关面板刷新查看账号')`);
          }
          setTimeout(() => { if (loginWindow) { loginWindow.close(); loginWindow = null; } }, 1500);
        } else {
          // 失败时明确提示，不再静默
          dialog.showMessageBox(loginWindow || mainWindow || undefined, {
            type: 'warning',
            title: '未检测到登录会话',
            message: '页面已打开，但还没抓到登录 Cookie。',
            detail: '请确认已在打开的窗口中完成 freebuff.com 登录（登录后会跳到 /chat 页面）。\n\n完成登录后无需其他操作，网关会自动抓取。',
            buttons: ['继续等待', '关闭窗口'],
          }).then(({ response }) => {
            if (response === 1 && loginWindow) { loginWindow.close(); loginWindow = null; }
          });
        }
      }, 1200);
    }
  });
  loginWindow.on('closed', () => { loginWindow = null; });
  loginWindow.loadURL('https://freebuff.com/');
}

app.on('ready', async () => {
  writeLog('[app] 启动');
  migrateLegacyData();
  startGateway();
  const ok = await waitForGateway();
  if (!ok) {
    ready = false;
    const port = configuredPort();
    writeLog(`[ERROR] 网关未就绪（端口 ${port}）`);
    dialog.showErrorBox(
      'Freebuff2API 启动失败',
      `网关在 15 秒内没有响应（期望地址 http://127.0.0.1:${port}）。\n\n` +
      `常见原因与处理：\n` +
      `1. 端口 ${port} 被其他程序占用 → 编辑 %APPDATA%\\freebuff2api\\config.json 改 listen_addr\n` +
      `2. 首次启动较慢 → 稍等后从托盘「打开控制台」重试\n` +
      `3. 杀毒/防火墙拦截 → 允许 freebuff2api.exe 通过\n\n` +
      `日志：${logPath()}`
    );
  } else {
    ready = true;
    writeLog('[app] 网关就绪');
  }
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
    autoUpdater.autoDownload = true;          // 有新版本自动下载
    autoUpdater.autoInstallOnAppQuit = true;  // 退出时自动安装
    autoUpdater.setFeedURL({
      provider: 'generic',
      url: 'https://github.com/lza6/Freebuff-2API/releases/latest/download/',
    });
    autoUpdater.on('update-available', () => {
      tray.setToolTip('Freebuff2API — 有新版本，正在下载…');
      tray.displayBalloon({ title: 'Freebuff2API 更新可用', content: '正在后台下载，完成后退出会自动安装' });
    });
    autoUpdater.on('update-downloaded', () => {
      tray.setToolTip('Freebuff2API — 更新已就绪，退出后自动安装');
      dialog.showMessageBox({
        type: 'info',
        title: '更新已下载',
        message: '新版本已下载完成，退出程序后会自动安装。',
        buttons: ['立即重启安装', '稍后'],
      }).then(({ response }) => {
        if (response === 0) { app.isQuitting = true; if (gateway) gateway.kill(); updater.quitAndInstall(); }
      });
    });
    autoUpdater.on('update-not-available', () => {
      tray.displayBalloon({ title: 'Freebuff2API', content: '当前已是最新版本 ✅' });
    });
    autoUpdater.on('error', (e) => { writeLog(`[updater] ${e.message}`); console.error('[updater]', e.message); });
    autoUpdater.checkForUpdates().catch(() => {});
  } catch (e) {
    writeLog(`[updater] 初始化失败 ${e.message}`);
    console.error('[updater] 初始化失败', e.message);
  }
}

function checkForUpdates() {
  if (!updater) { setupUpdater(); }
  if (updater) {
    updater.checkForUpdates().catch((e) => {
      dialog.showErrorBox('检查更新失败', `无法访问更新源（可能是网络问题）。\n\n${e.message}`);
    });
  } else {
    dialog.showMessageBox({ type: 'info', title: '检查更新', message: '开发模式下不检查更新。' });
  }
}

app.on('window-all-closed', (e) => {
  // 托盘常驻：仅隐藏不退出
  if (process.platform !== 'darwin') {
    // 保留托盘进程
  }
});

app.on('before-quit', () => {
  app.isQuitting = true;
  if (gateway) gateway.kill();
  if (logStream) { try { logStream.end(); } catch (_) { /* 忽略关闭错误 */ } }
});

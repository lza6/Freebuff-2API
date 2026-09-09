// Freebuff2API 桌面启动器（Electron 壳）
// - 拉起网关二进制（target/release/freebuff2api.exe）
// - 等待 HTTP 就绪后加载本地控制面板
// - 系统托盘 + 开机自启 + 优雅退出

const { app, BrowserWindow, Tray, Menu, nativeImage, ipcMain } = require('electron');
const { spawn } = require('node:child_process');
const http = require('node:http');
const path = require('node:path');
const fs = require('node:fs');

const GATEWAY_PORT = 8787;
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
      listen_addr: '127.0.0.1:8787',
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
    // 无图标时用 1x1 透明占位
    trayIcon = nativeImage.createEmpty();
  }
  tray = new Tray(trayIcon.resize({ width: 16, height: 16 }));
  tray.setToolTip('Freebuff2API 网关');
  tray.setContextMenu(Menu.buildFromTemplate([
    { label: '打开控制台', click: () => { if (!mainWindow) createWindow(); else mainWindow.show(); } },
    { label: '健康检查', click: async () => { await checkHealth(); } },
    { type: 'separator' },
    { label: '退出', click: () => { app.isQuitting = true; if (gateway) gateway.kill(); app.quit(); } },
  ]));
  tray.on('click', () => { if (!mainWindow) createWindow(); else mainWindow.show(); });
}

app.on('ready', async () => {
  startGateway();
  const ok = await waitForGateway();
  if (!ok) console.error('[Freebuff2API] 网关未就绪');
  else ready = true;
  createWindow();
  createTray();
});

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

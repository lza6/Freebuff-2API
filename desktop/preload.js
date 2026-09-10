// 桌面端 preload：向渲染进程（控制面板）暴露最小 IPC 能力
// 安全约束：只暴露白名单方法，不暴露 ipcRenderer 本体
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('freebuffDesktop', {
  /** 打开 freebuff.com 登录窗口，登录成功后自动抓 Cookie 入库 */
  openLogin: () => ipcRenderer.invoke('open-login'),
  /** 手动触发一次 Cookie 抓取（已登录过的情况下） */
  captureCookie: () => ipcRenderer.invoke('capture-cookie'),
  isDesktop: true,
});

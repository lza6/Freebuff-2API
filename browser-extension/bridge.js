// Freebuff2API 面板 ↔ 扩展 桥接（content script，仅注入 127.0.0.1 / localhost）
// 作用：把扩展 ID 与版本号通过 window.postMessage 告诉网关控制面板（/ui），
// 面板拿到 ID 后即可用 chrome.runtime.sendMessage(扩展ID, {type:'freebuff2api.import'}) 直连扩展。
// 背景：content script 与页面共享同一个 window，postMessage 是两者唯一可用的通信方式。

(function () {
  'use strict';

  var SOURCE_EXT = 'freebuff2api-extension';
  var SOURCE_PAGE = 'freebuff2api-page';

  function handshake() {
    try {
      // 扩展被重载后旧 content script 的 runtime 会失效，此时 chrome.runtime.id 抛异常——静默即可
      window.postMessage(
        {
          source: SOURCE_EXT,
          version: chrome.runtime.getManifest().version,
          id: chrome.runtime.id,
        },
        location.origin
      );
    } catch (e) { /* 扩展上下文失效，等待页面刷新 */ }
  }

  window.addEventListener('message', function (ev) {
    // 只接受本窗口、本源的页面消息
    if (ev.source !== window) return;
    if (ev.origin !== location.origin) return;
    var d = ev.data;
    if (!d || typeof d !== 'object') return;
    if (d.source !== SOURCE_PAGE) return;
    // 面板可能在 bridge 注入前就已开始监听，因此每次 ping 都重发一次握手
    if (d.type === 'ping') handshake();
  });

  // 尽早广播一次；若页面脚本此时尚未注册监听，会通过上面的 ping 机制补发
  handshake();
})();

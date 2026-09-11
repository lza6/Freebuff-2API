// Freebuff2API 扩展设置页逻辑（单独文件：MV3 扩展页 CSP 禁止内联脚本）
'use strict';

const portEl = document.getElementById('port');
const apiKeyEl = document.getElementById('apiKey');
const msgEl = document.getElementById('msg');
const saveEl = document.getElementById('save');
const resetEl = document.getElementById('reset');

function setMsg(text, ok) {
  msgEl.style.color = ok ? '#3fb950' : '#f85149';
  msgEl.textContent = text;
}

function getStored(cb) {
  chrome.storage.local.get(['gatewayPort', 'apiKey'], function (r) {
    void chrome.runtime.lastError; // 必须读取，避免未检查错误
    cb(r || {});
  });
}

/** 写入/清除配置：value 为 null 表示删除该键 */
function writeConfig(setObj, removeKeys, cb) {
  chrome.storage.local.set(setObj, function () {
    void chrome.runtime.lastError;
    if (!removeKeys.length) { cb(); return; }
    chrome.storage.local.remove(removeKeys, function () {
      void chrome.runtime.lastError;
      cb();
    });
  });
}

getStored(function (r) {
  if (r.gatewayPort) portEl.value = r.gatewayPort;
  if (r.apiKey) apiKeyEl.value = r.apiKey;
});

saveEl.addEventListener('click', function () {
  const rawPort = portEl.value.trim();
  const rawKey = apiKeyEl.value.trim();

  let portValue = null; // null = 留空 = 清除自定义端口
  if (rawPort !== '') {
    const v = parseInt(rawPort, 10);
    if (!v || v < 1 || v > 65535) {
      setMsg('端口不合法：请输入 1–65535 之间的整数', false);
      return;
    }
    portValue = v;
  }
  if (rawKey.length > 512) {
    setMsg('API Key 过长（最多 512 字符）', false);
    return;
  }

  const setObj = {};
  const removeKeys = [];
  if (portValue !== null) setObj.gatewayPort = portValue; else removeKeys.push('gatewayPort');
  if (rawKey) setObj.apiKey = rawKey; else removeKeys.push('apiKey');

  writeConfig(setObj, removeKeys, function () {
    const parts = [];
    parts.push(portValue !== null ? '端口 ' + portValue : '端口改为自动探测（47821 → 47822 → 8787）');
    parts.push(rawKey ? '已保存 API Key' : '未设置 API Key（网关未配置 api_keys 时无需填写）');
    setMsg('已保存：' + parts.join('；'), true);
  });
});

resetEl.addEventListener('click', function () {
  portEl.value = '';
  apiKeyEl.value = '';
  writeConfig({}, ['gatewayPort', 'apiKey'], function () {
    setMsg('已清空：端口恢复自动探测（47821 → 47822 → 8787），不再发送 Authorization 头', true);
  });
});

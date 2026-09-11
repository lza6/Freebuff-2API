# Freebuff2API 一键登录扩展（Chrome / Edge）

浏览器版无法用网页脚本读取 `__Secure-next-auth.session-token`——它是 **HttpOnly** Cookie，浏览器安全策略禁止 `document.cookie` 访问。本扩展通过官方的 `chrome.cookies` API 读取（这是浏览器唯一的合法途径），把登录凭证自动发送到本机网关。

## 安装（30 秒）

1. 确认本目录存在（`Freebuff2API/browser-extension/`）
2. 打开浏览器扩展页：
   - Chrome：地址栏输入 `chrome://extensions`
   - Edge：地址栏输入 `edge://extensions`
3. 打开右上角 **开发者模式**
4. 点 **加载已解压的扩展程序** → 选择本目录（`browser-extension`）
5. 扩展栏出现 Freebuff2API 图标
6. **刷新已打开的网关面板页面**（让扩展的 `bridge.js` 注入并完成握手）

## 使用方式 A：面板「一键登录」（推荐 · 全自动）

1. 打开网关控制面板（如 `http://127.0.0.1:47821/ui`）→「账号」页
2. 点 **🔑 一键登录**
3. 若浏览器里已有 freebuff.com 登录态 → 凭证立即自动入库，面板刷新即可看到账号
4. 若尚未登录 → 扩展自动打开 freebuff.com 登录页并提示「请完成 GitHub 登录」；**登录成功后无需任何操作**，扩展自动检测到会话并导入（最长等待 3 分钟）
5. 通知栏出现「✅ 已自动导入 N 个凭证」

重复导入不会重复入库（同值自动去重）。

## 网关 API Key（可选）

网关配置了 `api_keys` 时，导入接口会校验 `Authorization: Bearer <key>`：

- **从面板点「一键登录」**：你在面板顶部填的 Key 会随请求一起传给扩展，无需在扩展里再填一次。
- **直接点扩展图标**：右键扩展图标 →「选项」→「网关 API Key」填入面板显示的 Key。
- **网关未配置 `api_keys` 时留空即可**：扩展不发送 `Authorization` 头，本机直连照常可用。
- 通知出现「🔑 网关已启用 API Key 校验」即表示 Key 缺失或不正确，按上面两步填写后重试。

## 使用方式 B：点击扩展图标（等效路径）

点扩展图标 = 上面同一条流程：能读到登录态就直接导入；读不到就自动打开 freebuff.com 等待登录，登录后自动导入。适合不想开面板、只想先把凭证存进网关的场景。

## 工作原理（为什么能全自动）

- 扩展的 `bridge.js` 作为 content script 注入本机网关面板页面，通过 `window.postMessage` 把扩展 ID 告知面板；面板用 `chrome.runtime.sendMessage(扩展ID, ...)` 直连扩展（`externally_connectable` 只放行 `127.0.0.1` / `localhost`）。
- 扩展收到请求后：先查 `__Secure-next-auth.session-token`；没有就打开 freebuff.com 并 **每 2 秒轮询一次**，拿到后立即 POST 到网关 `/api/tokens/import`（网关配置了 `api_keys` 时自动带 `Authorization: Bearer <key>`）。
- 网关端口按 `面板传入 > 选项页设置 > 47821 → 47822 → 8787` 探测；**只有连接被拒才换端口**，网关有 HTTP 响应（含报错）就停在该端口并如实提示。

## 常见问题

**面板点「一键登录」没反应 / 提示未检测到扩展**：确认扩展已启用、版本为 1.1.0，然后**刷新面板页面**（`Ctrl+F5`）让 `bridge.js` 重新注入。

**提示「未找到登录凭证」后没有自动打开登录页**：多为浏览器拦截了新标签页，手动打开 [freebuff.com](https://freebuff.com) 登录即可；登录后扩展仍会在轮询中检测到并自动导入（或再点一次图标）。

**提示「连接本地网关失败」**：确认 Freebuff2API 已启动（默认端口 47821）。如果你改过端口：右键扩展图标 →「选项」→ 填入端口；或直接从网关面板点「一键登录」（面板会自动带上自己的端口）。

**提示「网关已启用 API Key 校验」（401）**：网关配置了 `api_keys`。在面板顶部填入 Key，并右键扩展图标 →「选项」→「网关 API Key」填入同一个 Key；从面板点「一键登录」时会自动携带，无需手填。

**提示「等待登录超时」**：3 分钟内没检测到登录。完成登录后再点一次图标或面板按钮即可立即导入。

**它会读取我的密码吗？** 不会。扩展只读取 freebuff.com 域下的 Cookie（其中包含登录会话标识），不接触账号密码，也不会把数据发往除你本机 `127.0.0.1` 之外的任何地方。

## 权限说明

| 权限 | 用途 |
|------|------|
| `cookies` | 读取 freebuff.com 的登录 Cookie（HttpOnly 只能这样读） |
| `host_permissions: freebuff.com` | 限定只能读取该域 |
| `host_permissions: 127.0.0.1/localhost` | 只能发送到你本机的网关 |
| `notifications` | 导入结果/登录提醒通知 |
| `storage` | 记住你设置的网关端口与（可选的）网关 API Key |
| `content_scripts`（bridge.js，仅 127.0.0.1/localhost） | 把扩展 ID 告知本机网关面板，实现面板→扩展直连 |
| `externally_connectable`（仅 127.0.0.1/localhost） | 只允许本机网关面板向扩展发消息 |

## 源码文件（可自行审阅，纯原生 JS 无依赖）

| 文件 | 作用 |
|------|------|
| `background.js` | 主流程：读 Cookie、多端口导入、自动导航、轮询等待登录、面板消息处理 |
| `bridge.js` | content script：面板 ↔ 扩展握手（只注入 127.0.0.1 / localhost） |
| `options.html` / `options.js` | 端口与网关 API Key 设置页 |

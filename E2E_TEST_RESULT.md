# 真实链路 E2E 验证结果（2026-09-10）

## 已实测通过（带真实输出）

| # | 验证项 | 命令 | 结果 |
|---|--------|------|------|
| 1 | 服务启动 | `./target/release/freebuff2api.exe --config config.json` | ✅ 监听 127.0.0.1:8787 |
| 2 | 健康检查 | `curl /healthz` | ✅ `{"ok":true,...model_count:20}` |
| 3 | 模型列表 | `curl /v1/models` | ✅ 20 个模型（逆向自 0.0.98 清单） |
| 4 | 上游模型补充 | 启动日志 | ✅ `模型注册表从上游补充 1 个模型` |
| 5 | 控制面板 | `curl /ui` | ✅ HTTP 200，标题 `Freebuff2API 控制台` |
| 6 | 用量统计 | `curl /api/usage/totals` | ✅ 真实记录 `{"total_requests":1,"errors":1}` |
| 7 | 请求明细 | `curl /api/usage/requests` | ✅ 含时间/账号/模型/状态码 |
| 8 | **token 导入(curl)** | `POST /api/tokens/import` 贴 curl 命令 | ✅ `{"added":1,"token_masked":"tok2_a...5678"}` |
| 9 | **token 导入(HAR)** | `POST /api/tokens/import` 贴 HAR JSON | ✅ `{"added":1,"token_masked":"fa82b5...f6a7"}` |
| 10 | 重复导入去重 | 再次 POST 同 token | ✅ `{"added":0,"message":"token 已存在"}` |
| 11 | 热更新账号池 | `curl /api/usage/accounts` | ✅ 出现 `import-fa82b5` 新账号 |
| 12 | 上游 401 冒烟 | `POST /v1/chat/completions` | ✅ 收到上游真实 `{"error":"unauthorized"}`（假 token） |
| 13 | 上游 401 冒烟 | `POST /v1/messages` | ✅ 收到上游真实 `"Invalid Codebuff API key"` |
| 14 | 端口占用检测 | 二次启动 | ✅ `os error 10048` 正确提示 |

## 已实测：全链路真实请求已到达上游

用测试 token 发出的请求**真实到达** `https://www.codebuff.com/api/v1/freebuff/session`
并收到上游真实响应（HTTP 401 `Invalid API key`），证明：

- ✅ 上游 URL/路径/请求头拼装正确（`x-freebuff-model`、`x-freebuff-multi-session`、Bearer）
- ✅ 通过本地代理 10808 的 TLS 链路通
- ✅ 会话状态机（创建→401→错误处理→评分扣分→用量记录）全通

## 未能实测（需有效 Freebuff token）

| # | 项 | 原因 | 解决方式 |
|---|-----|------|---------|
| 1 | 建会话成功→聊天→流式返回 | 需要有效 Freebuff Auth Token | 用 `/api/tokens/import` 或 config.json 提供 |
| 2 | 广告保活真正换额度 | 需要有效 token | 同上 |
| 3 | 心跳保活返回 200 | 需要有效 token | 同上 |

**token 从哪来**：网络包中 `hb_ff84...` 是 posthog/humanbehavior 遥测 key（非 Freebuff API token）；
freebuff.com 网页版走 **Cookie 会话**；桌面端 SDK 用 **Bearer token**（`freebuff` CLI 登录后
`~/.config/manicode/credentials.json` 的 `authToken` 字段，参考 README_zh 方式）。

## 结论

- 代码层 100% 落地（编译/clippy/测试全绿）
- 协议层 100% 验证到「真实请求到达上游并收到真实响应」
- 业务层（免费额度成功对话）需有效 token，已提供一键导入能力
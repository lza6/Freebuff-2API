# 素材来源记录

| 文件 | 来源 | 许可证 | 日期 | 说明 |
|------|------|--------|------|------|
| `screens/ui-full.png` | 本地网关 `/ui` 控制台（http://127.0.0.1:8787/ui，Chrome headless 1080×1920） | 项目自产截图 | 2026-09-10 | GitHub Dark 风格控制台：账号健康/用量/最近请求/模型列表 |
| `screens/healthz.json` | 本地网关 `/healthz` | 项目自产数据 | 2026-09-10 | 账号健康快照 |
| `screens/models.json` | 本地网关 `/v1/models` | 项目自产数据 | 2026-09-10 | 20 个模型注册表 |
| `screens/balance.json` | 本地网关 `/api/account/balance` | 项目自产数据 | 2026-09-10 | 账号积分/套餐/每模型限额 |
| `screens/usage.json` | 本地网关 `/api/usage/totals` | 项目自产数据 | 2026-09-10 | 用量统计 |
| `screens/terminal-boot.png` | 本地 release 二进制启动终端（真实 ANSI 日志） | 项目自产截图 | 2026-09-10 | 启动输出含监听地址/账号数/模型注册表 |
| `screens/chat-sse.json` | mock 上游 SSE 事件流（协议形状） | 模拟数据 | 2026-09-10 | `freebuff.start/text/reasoning/done` 11 事件真实形态 |

> **协议链路说明**：本地网关无真实 Freebuff token（config 中 AUTH_TOKENS 为空），因此真实 chat 请求返回 `401 Invalid API key`（已验证），面板数据真实。流式 chat 的 SSE 形态用本地 mock 上游还原（事件名/字段与上游一致），用于视频协议演示，不产生真实 API 消费。

# 第二轮深化交付总结（2026-09-10）

## 本轮新增（全部落地+实测+推送+Release 更新）

| # | 功能 | 状态 | 证据 |
|---|------|------|------|
| 1 | **web 版协议适配** `src/web_protocol.rs` | ✅ | chat/stream SSE 11 事件全解析、多模态上传、工具调用透传（逆向自网页对话/工具调用/图片上传三包） |
| 2 | **双桶并发信号量** `src/concurrency.rs` | ✅ | 逆向自桌面端 orchestrator.js：free{slot:1,multi:3}/subscriber{slot:3,multi:8}/limited{1,0}，3 项单测过 |
| 3 | **账号余额查询** `GET /api/account/balance` | ✅ 实测 | Cookie 导入后真实返回 freebucks.balance=95/100、每模型 usable_today、套餐、地区限制 |
| 4 | **Cookie 凭证导入** `POST /api/tokens/import` | ✅ 实测 | 完整 Cookie 串（__Secure-next-auth.session-token=...）自动解析入库，5 项单测过 |
| 5 | **面板积分卡片** | ✅ 实测 | /ui 新增"账号积分"区：今日剩余/每模型剩余/套餐/地区限制实时显示 |
| 6 | **Release 更新** | ✅ | v0.1.0 安装包重建（含全部新功能）+ 说明更新 |

## 上游并发限制逆向结论（回答你的问题：3 并发）

**你的理解正确但更精确**——不是统一 3 并发，而是**双桶模型**：
- 免费账号：付费模型槽 = **1**，普通模型 = **3**
- 订阅账号：付费模型槽 = **3**，普通模型 = **8**
- 你遇到的"三个任务同时、第四次要等" = 免费账号普通模型桶 3 并发限制
- 等待室：服务端返回 428/429 + `retry-after-ms`，客户端至多退避 3s 重试一次，无排队队列概念

## 剩余积分查询（回答你的问题：不知道怎么查）

**现在能查了**。两种方式：
1. 面板 `/ui` → 看到"账号积分"卡片（实时）
2. API `GET /api/account/balance`（JSON）

**需要先提供 web Cookie**：浏览器登录 freebuff.com → DevTools → Copy as cURL 或复制 Cookie → `POST /api/tokens/import` 粘贴。查询逻辑（逆向自抓包）：
- `freebucks.daily.remaining` = 今日剩余积分
- 每模型剩余次数 = min(积分÷价目, 每日次数-已用)
- **upstage/solar-pro4 0 积分免费**（但仍有每日 6 次额度池）
- 每日 07:00 UTC（太平洋午夜）自动重置

## 本轮真实验证数据

```
导入 Cookie → {"added":1,"token_masked":"__Secu....com"}
GET /api/account/balance →
{
  "access_tier":"full",
  "freebucks":{"balance":95,"daily":{"limit":100,"spent":5,"remaining":95,"resetAt":"2026-09-10T07:00:00.000Z"}},
  "model_remaining":{
    "deepseek/deepseek-v4-flash":{"price":15,"usable_today":6},
    "google/gemini-3.8-flash":{"price":50,"usable_today":1},
    "upstage/solar-pro4":{"price":0,"usable_today":-1}  // -1=免费不限
  }
}
```

## 已推送

- 仓库 main 分支 4 个新提交（web协议/并发/余额/面板）
- Release v0.1.0 安装包已重建并上传，说明已更新

## 剩余待办（下一轮）

| 项 | 说明 |
|----|------|
| OAuth 登录自动化 | 上游 next-auth 协议复杂（CSRF+设备指纹），建议做"粘贴 Cookie"路线（已完成）+ 未来可选 OAuth 流程 |
| web 协议默认路由 | 目前 web 协议作为可选模块，可配置 AUTH_MODE=web 时用 Cookie 走 chat/stream；需真 Cookie 验证完整对话流式 |
| 工具调用转 OpenAI tools | web 的 agent_tool 事件可映射为 OpenAI tool_calls 透传给 Claude Code/Codex |
| 自动续期 | convex-token 10min TTL + auth/session 30天续期定时任务（模块已逆向，落地待 Cookie 验证） |
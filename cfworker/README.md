# Freebuff2API — Cloudflare Worker（实验性）

> ⚠️ **当前状态：对线上游不可用，仅供研究参考。**
>
> Codebuff 上游以 `free_mode_cli_required` 拒绝来自 Cloudflare Worker 的请求。这是 **TLS 指纹层（Client Hello / JA3）** 的检测——Worker 的 `fetch` 使用 Cloudflare 自己的 TLS 栈，指纹与官方 CLI（Node.js undici/OpenSSL）不同，**无法通过修改 User-Agent 或任何 HTTP 头绕过**。本地 Go 二进制使用原生 TLS 栈，不在黑名单内，可正常通过。
>
> **请使用仓库根目录的本地 Go 服务端**（见 [../README_zh.md](../README_zh.md)）。本目录代码保留了协议翻译与 run/session 管理的实现，供后续研究。

---

## 本目录内容

- `worker.js` — 无状态路由中转版（已移除 D1/KV/DO 依赖）。接收 OpenAI 格式请求，注入 `codebuff_metadata` 后转发到 Codebuff 上游，凭证由调用方在 `Authorization` 头直接携带。
- `migrations/` — D1 数据库迁移（早期带凭证管理/审计统计的 SaaS 版遗留）。
- `test-api.js`、`test-upstream.js` — E2E 测试脚本。

## 逆向要点（本实现已覆盖）

1. **会话创建**：`POST /api/v1/freebuff/session`，带 `{}` 空体 + `x-freebuff-model` 头，返回 `instanceId`。
2. **Run 层级**：先建根 run（`base2-free`，`ancestorRunIds: []`），子 run 的 `ancestorRunIds` 只能包含根 run id，否则报 `free_mode_invalid_agent_hierarchy`。
3. **metadata 注入**：聊天载荷必须带 `codebuff_metadata`：`run_id`、`cost_mode: "free"`、`client_id`、`freebuff_instance_id`。
4. **模型收紧**：实测仅 `google/gemini-2.5-flash-lite` 与 `google/gemini-3.1-flash-lite-preview` 可用，其余模型返回 `free_mode_invalid_agent_model`。

---

## 许可证

MIT

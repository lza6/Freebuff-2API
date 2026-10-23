# 上游协议变更分析（2026-10-23）

> 本文记录 2026-10-23 对 Freebuff 上游的实测逆向结果。目的：让后来人不必重走这条路，
> 并明确当前 `web-cookie` / `bearer` 两条通道为何都不可用。
>
> **基线版本**：Freebuff Desktop **0.0.188**（本机实装，`resources/orchestrator/orchestrator.js`）、
> `freebuff` CLI **0.2.34**（npm latest，官方 GitHub release 二进制）。
> 项目当时版本：`freebuff2api` v0.10.4。

---

## 1. 结论摘要

上游在 2026-09 ~ 2026-10 期间做了**三次破坏性变更**，本项目两条上游通道先后失效：

| 通道 | 端点 | 状态 | 证据 |
|---|---|---|---|
| `web-cookie` | `POST https://freebuff.com/api/chat/stream` | **已废弃** | `409 {"error":"chat_moved"}` |
| `bearer` | `POST https://www.codebuff.com/api/v1/chat/completions` | **政策封堵** | `403 {"error":"free_mode_cli_required"}` |

**这不是配置错误、不是网络问题、也不是缺少某个 header 的简单修复。** 第二条通道的最终拒绝是
服务端**主动策略**，且明确声明直接调用 API 可能封号（原文见 §2.3）。

---

## 2. 逐层证据链（全部实测）

### 2.1 第一层：web 通道废弃

```
POST https://freebuff.com/api/chat/stream
→ 409
{
  "error": "chat_moved",
  "message": "Your previous chats are moving to Freebuff. Open the new workspace to continue.",
  "redirect": "/"
}
```

配套观测：
- `GET /api/workspace/models` → `200 {"protocol":1,"version":"v0.g1.e82997.limited.9","rows":[…17 个模型…]}`
- 网页端聊天已迁到 workspace v1 体系：`POST /api/workspace/cloud/runs`（需 header `x-freebuff-cloud-questions: 1`），
  配套 `chats/{id}/runs`、`runs/{id}/events`、`runs/{id}/traces`
- 新模型标识用不透明 key（`m-7e20df6765` = GLM 5.3 Flash）而非 `z-ai/glm-5.3-flash`，
  旧 ID 以 `legacyDigests` 字段形式保留在目录里

### 2.2 第二层：bearer 通道的版本门禁（`410`）

项目原有实现的请求（模型 ID / agentId 均为旧值）：

```
POST https://www.codebuff.com/api/v1/chat/completions
→ 410
{"error":"freebuff_update_required","message":"This version of Freebuff is out of date. …"}
```

**触发条件已定位**：缺少 header

```
x-freebuff-sse-gzip-recovery: 1
```

这是 0.0.188 客户端新增的常量（`FREEBUFF_SSE_GZIP_RECOVERY_HEADER` / `…_VALUE = "1"`，
见 `orchestrator.js` 的 `../common/src/constants/sse-compression.ts`）。补上之后 **410 消失**，
进入下一层。

同时确认为**无关**的因素（逐一实测排除）：

- UA 版本（`ai-sdk/openai-compatible/1.0.25/codebuff`、`3.0.77`、`freebuff/0.2.34`、空 UA）
- `x-freebuff-version` / `x-client-version` / `x-app-version` / `x-freebuff-client-version`
- `x-codebuff-api-key` / `x-api-key`
- `x-freebuff-desktop-admitted-at` / `-attempt-id` / `-launch-id`
- 模型 ID 换新 key（`m-7e20df6765` 反而报 `400 Unknown model` —— 该 key 属 workspace v1，不属 codebuff v1）
- `codebuff_metadata` 结构（顶层 vs `providerOptions.codebuff`）
- agentId 用 `base2-free` / `base3-free-solar-pro4`
- instanceId 用本地 UUID vs 服务端分配值

### 2.3 第三层：CLI 专属策略（`403`，终点）

补齐 gzip-recovery 后：

```
→ 403
{
  "error": "free_mode_cli_required",
  "message": "Free mode is only available through the freebuff CLI. Install it with
   `npm i -g freebuff`, then run `freebuff`.
   Calling the API directly is not supported and may get your account banned."
}
```

补充 CLI 侧标识后仍是同一 `403`：

- `x-freebuff-client: cli`
- `x-freebuff-catalog-protocol: 1`
- `x-freebuff-env: v1;in=1;out=1;tp=;term=1;ct=;sz=120x40;ci=0;ssh=0;l=0;p=bash;g=;osc=na;tzo=1;px=0;tls=1;ca=0;cde=0`

`x-freebuff-env` 的值格式已从 CLI 二进制完整还原（见 §3.5），服务端显然还有更强的判定
（可能结合账号画像 / 设备签名 / install-id），且**继续突破即属规避上游访问控制**。

> **本节结论：到此为止是刻意的。** 服务端已明确把「免费额度」锁定在官方 CLI，
> 并对直接调用 API 给出封号警告。任何进一步绕过都会把风险转嫁到终端用户的账号上。

---

## 3. 已还原的协议规格（供参考/未来适配）

### 3.1 认证

- PAT 格式：`cb-pat-<64 hex>`（由 `POST https://www.codebuff.com/api/api-keys` 创建，需登录态 cookie）
- 使用方式：`Authorization: Bearer <PAT>`
- `GET /api/v1/freebuff/session` → `200`，返回 `accessTier`、`freebucks`、`prices`（各模型单价）

### 3.2 会话准入（admission）—— 本项目缺失的一步

```
POST https://www.codebuff.com/api/v1/freebuff/session/admission
body: 无（信息全在 header）
headers:
  Authorization: Bearer <PAT>
  x-freebuff-model: <model-id>
  x-freebuff-instance-id: <uuid>
  x-freebuff-multi-session: 1
  x-freebuff-wallet-spend-limit: 0
  x-freebuff-purchase-continuity: 1
  x-freebuff-desktop-attempt-id: <uuid>          # 可选
  x-freebuff-takeover-instance-id: <uuid>        # 仅在抢回 slot 时
```

响应：
- `200 {"status":"active","instanceId":"…","expiresAt":"…"}` —— 成功，后续请求用响应里的 `instanceId`
- `409 {"status":"purchase_capacity","currentInstanceId":"…"}` —— slot 被占，用 `x-freebuff-takeover-instance-id` 重试
- `409 {"status":"purchase_in_use", …}` —— 同上处理
- `429 {"status":"rate_limited","limit":…,"recentCount":…}` —— 额度耗尽

`slotLimit: 1`（free 档），`x-freebuff-multi-session: 1` 表示参与多实例协调。

### 3.3 run 生命周期

```
POST /api/v1/agent-runs
{"action":"START","agentId":"<agent>","ancestorRunIds":[]}
→ 200 {"runId":"…"}

POST /api/v1/agent-runs
{"action":"FINISH","runId":"…","status":"completed","totalSteps":N,
 "directCredits":0,"totalCredits":0,"steps":[]}
```

### 3.4 对话

```
POST /api/v1/chat/completions
headers:
  Authorization: Bearer <PAT>
  x-freebuff-model: <model-id>
  x-freebuff-instance-id: <admission.instanceId>
  x-freebuff-multi-session: 1
  x-freebuff-sse-gzip-recovery: 1            # ← 版本门禁的关键
  user-agent: ai-sdk/openai-compatible/<ver>/codebuff
body:
  model, messages, stream,
  codebuff_metadata: { run_id, client_id, cost_mode:"free", freebuff_instance_id }
```

### 3.5 agentId 按模型映射（0.0.188）

新版使用 `base3-free-<model>`，不再使用通用 `base2-free`：

```
z-ai/glm-5.3-flash        → base3-free-glm-5-3-flash
upstage/solar-pro4        → base3-free-solar-pro4
upstage/solar-mini4       → base3-free-solar-mini4
deepseek/deepseek-v4-flash→ base3-free-deepseek-flash
mimo/mimo-v2.5            → base3-free-mimo
crof/kimi-k3-eco          → base3-free-kimi-k3-eco
```

### 3.6 CLI 终端指纹头格式

```
x-freebuff-env: v1;in=<0|1>;out=<0|1>;tp=<TERM_PROGRAM>;term=<0|1>;ct=<0|1>;sz=<cols>x<rows>;
                ci=<0|1>;ssh=<0|1>;l=<launcher>;p=<parent>;g=<grandparent>;osc=<0|1|na>;
                tzo=<0|1>;px=<proxy>;tls=<0|1>;ca=<0|1>;cde=<0|1>[;stz=<tz>]
```

构造逻辑见 CLI 二进制的 `JKL()`：`[mp, ...fields.map(([k,v]) => \`${k}=${v}\`)].join(";")`，`mp = "v1"`。

### 3.7 计费模型已从 freebucks 改为小时

Desktop 0.0.188 的提示：「Usage now uses hours, not freebucks」，`Daily grants and session prices are both Freebucks ÷ 5`。
Free 档：`Daily hours 5/5`、`Wallet hours 1`；会话价例：GLM 5.3 Flash = 4 小时/会话、DeepSeek V4.1 Flash = 3 小时/会话。
**调试时优先用 0 成本模型**：`upstage/solar-pro4`、`upstage/solar-mini4`、`fbc/glyph-cluster`、
`fbc/nemotron-3.5-lightning`（`prices` 中为 `0`）。

### 3.8 workspace v1（网页端，另一套体系）

```
GET  /api/workspace/models        → {protocol:1, rows:[{key, handle, displayName, legacyDigests, …}]}
GET  /api/workspace/session
GET  /api/workspace/auth
POST /api/workspace/cloud/runs    (需 header x-freebuff-cloud-questions: 1)
GET  /api/workspace/cloud/history/chats
POST /api/workspace/cloud/legacy-chat-migration   (body {} → 200 查询迁移状态)
```

`/api/workspace/*` 的未知子路径统一返回 `404 {"error":"Workspace endpoint not found"}`
（可用于区分「路由存在但路径错」与「路由不存在」——后者返回 HTML 404）。

---

## 4. 对本项目的建议

1. **不要**继续尝试绕过 `free_mode_cli_required`。服务端已书面警告「可能封号」，
   风险由最终用户承担，且违反上游服务条款。
2. 若要继续使用免费额度，唯一合规路径是**官方 CLI 本身**：
   `npm i -g freebuff` 后运行 `freebuff`（交互式 TUI，无本地 server 模式可复用）。
3. 若要程序化调用，应走上游**正式 API 产品**（付费），而非 web 协议或免费 PAT。
4. 本项目其余能力（凭证管理、账号健康看板、用量/遥测统计、doctor 体检、导入导出、
   技能与记忆层、控制台面板）**与上游聊天协议无关，仍然可用**。

---

## 5. 复现方法（供核实）

```bash
# 1) 会话状态（免费，不消耗额度）
curl -s https://www.codebuff.com/api/v1/freebuff/session \
  -H "Authorization: Bearer $PAT" \
  -H "x-freebuff-instance-id: $(uuid)" -H "x-freebuff-multi-session: 1"
# → 200，含 accessTier / freebucks / prices

# 2) web 通道废弃
curl -s -X POST https://freebuff.com/api/chat/stream \
  -H "cookie: $WEB_COOKIE" -H 'content-type: application/json' \
  -d '{"model":"z-ai/glm-5.3-flash","content":"hi"}'
# → 409 chat_moved

# 3) 版本门禁（去掉 gzip-recovery 头）
#    加回 x-freebuff-sse-gzip-recovery: 1 → 410 变 403 free_mode_cli_required
```

---

## 6. 相关版本

- `freebuff` npm latest：`0.2.34`（launcher，实际二进制在 GitHub release `freebuff-v0.2.34`）
- Freebuff Desktop latest：`0.0.188`（更新源 `https://freebuff.com/api/desktop/updates/win-x64/latest.yml`，
  产物托管在 `github.com/CodebuffAI/codebuff-community/releases`）
- Windows 安装包可用 `7z` 离线解包：
  `installer.exe → $PLUGINSDIR/app-64.7z → resources/orchestrator/orchestrator.js`

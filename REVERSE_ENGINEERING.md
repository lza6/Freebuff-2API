# 逆向 Codebuff 免费层：把 Freebuff 免费模型变成 OpenAI 兼容 API 的全过程

> 一次完整的协议逆向实录：从抓包分析、绕过 CLI 检测，到修复 run 层级与模型收紧，最终落地一个本地可用的 OpenAI / Claude 双协议代理。
>
> 项目：[Freebuff2API](https://github.com/Quorinex/Freebuff2API)（Go 实现，单文件二进制）

---

## 背景

[Codebuff](https://www.codebuff.com) 是一个 AI 编程助手，旗下免费层 **Freebuff** 提供了一批可免费调用的模型（Gemini、DeepSeek、GLM、Kimi 等）。官方只提供 CLI 客户端，没有公开 API。

**目标**：把 Freebuff 的免费模型逆向成标准的 `/v1/chat/completions`（OpenAI 协议）和 `/v1/messages`（Claude 协议），让任意兼容客户端（LobeChat、NextChat、Claude Code、Codex 等）直接可用。

---

## 一、协议全貌：四个关键端点

通过抓包 CLI 的真实流量，梳理出 Freebuff 后端的完整调用链。所有请求都带 `Authorization: Bearer <authToken>`。

| 步骤 | 端点 | 作用 |
|------|------|------|
| 1 | `POST /api/v1/freebuff/session` | 创建/保活免费会话，返回 `instanceId` 和 `expiresAt` |
| 2 | `POST /api/v1/agent-runs` (action=START) | 启动一个 agent run，返回 `runId` |
| 3 | `POST /api/v1/chat/completions` | 真正的聊天请求，载荷里注入 `codebuff_metadata` |
| 4 | `POST /api/v1/agent-runs` (action=FINISH) | 结束 run，上报 steps/credits |

### 1. 会话创建

```http
POST /api/v1/freebuff/session
Authorization: Bearer <token>
x-freebuff-model: deepseek/deepseek-v4-flash
Content-Type: application/json

{}
```

响应：

```json
{
  "status": "active",
  "instanceId": "xxxx",
  "model": "...",
  "expiresAt": "2026-07-27T...",
  "rateLimit": {...}
}
```

注意是 **POST 带 `{}` 空体 + `x-freebuff-model` 头**，不是 GET。会话可能进入排队（`status: "queued"`，带 `position`/`queueDepth`），需要轮询。

### 2. 启动 Run

```http
POST /api/v1/agent-runs
Authorization: Bearer <token>

{
  "action": "START",
  "agentId": "base2-free",
  "ancestorRunIds": []
}
```

响应：

```json
{"runId": "2b56444d-..."}
```

### 3. 聊天请求（核心是 metadata 注入）

聊天载荷基本就是 OpenAI 格式，但必须注入 `codebuff_metadata`，四个字段缺一不可：

```json
{
  "model": "google/gemini-2.5-flash-lite",
  "messages": [...],
  "stream": false,
  "codebuff_metadata": {
    "run_id": "<第2步拿到的runId>",
    "cost_mode": "free",
    "client_id": "<随机13位hex>",
    "freebuff_instance_id": "<第1步的instanceId>"
  }
}
```

---

## 二、踩坑实录：三道真正的坎

### 坎 1：`free_mode_invalid_agent_hierarchy` —— run 必须挂在会话根下

最初直接对每个 agent 都 `START` 一个 run 然后用，结果聊天一律被拒：

```json
{"error":"free_mode_invalid_agent_hierarchy","message":"Free mode subagents must run under an active freebuff session root."}
```

**根因**：Freebuff 强制一棵以会话为根的 run 树——

```
会话 (instanceId)
└── 根 run: base2-free          ← 必须先建，ancestorRunIds: []
    ├── 子 run: file-picker      ← ancestorRunIds: [根runId]
    └── 子 run: code-reviewer-*  ← ancestorRunIds: [根runId]
```

子 run 的 `ancestorRunIds` **只能包含根 run 的 id**。我一开始犯的错误是把所有平级兄弟 run 的 id 都塞进祖先列表，上游立刻识别为非法层级。

**修复**：每个 token 保活**唯一一个根 run**（`base2-free`），子 run 惰性创建、祖先只指根。

### 坎 2：`400 Invalid request body` —— null 和 [] 的区别

根 run 没有祖先，`ancestorRunIds` 该传什么？Go 里 `var ancestors []string` 是 `nil`，`json.Marshal` 出来是 `null`：

```json
{"ancestorRunIds": null}
```

上游直接 400：

```json
{"error":"Invalid request body","details":{"ancestorRunIds":{"_errors":["Invalid input: expected array, received null"]}}}
```

**修复**：初始化成空切片 `ancestors := []string{}`，序列化成 `[]` 而非 `null`。一个典型的强类型语言踩弱类型 schema 校验的坑。

### 坎 3：`free_mode_invalid_agent_model` —— 免费层模型大幅收紧

按照官方开源仓库的 `free-agents.ts`，免费层本应支持一大批模型。我把它们全注册进去，结果**除 Gemini 外全军覆没**：

```json
{"error":"free_mode_invalid_agent_model","message":"Free mode is only available for specific agent and model combinations."}
```

逐个实测后的真实情况（2026-07）：

| 模型 | 结果 |
|------|------|
| `google/gemini-2.5-flash-lite` | ✅ |
| `google/gemini-3.1-flash-lite-preview` | ✅ |
| deepseek-v4-pro/flash、minimax-m3、glm-v5.2、kimi-k2-thinking、mimo-v2.5、hy3、laguna-s-2-1 | ❌ 全部拒绝 |

**教训**：`free-agents.ts` 源码列表 ≠ 后端实际放行的组合。免费层已收紧，**只能用实测收敛**，不能信文档。

---

## 三、CLI 检测：Cloudflare Worker 为什么行不通

最初想把代理部署成 Cloudflare Worker（免运维、全球分发）。结果无论怎么伪装，Worker 发出的请求一律被拒：

```json
{"error":"free_mode_cli_required"}
```

尝试过的绕过手段，全部无效：

- `User-Agent: Freebuff-CLI/0.0.105`（和官方 CLI 完全一致）
- 补 `Origin` / `Referer` / `Host` / `Accept-Encoding` 等全套浏览器/CLI 头
- 去掉所有会暴露 Worker 身份的头
- 处理 gzip 响应体

**结论**：检测不在 HTTP 头层，而在 **TLS 指纹层**（Client Hello / JA3）。Cloudflare Worker 的 `fetch` 用的是 Cloudflare 自己的 TLS 栈，指纹和 Node.js 的 undici/OpenSSL 完全不同，无法通过改头绕过。

**破局点**：本地 Go 二进制用的是 Go 原生 TLS 栈，指纹不在上游黑名单里，直接通过。所以最终形态是**本地代理服务**而非 Serverless。

---

## 四、最终架构

```
┌─────────────┐   OpenAI/Claude 协议   ┌──────────────────┐   Freebuff 私有协议   ┌────────────┐
│  任意客户端  │ ────────────────────▶ │  Freebuff2API     │ ───────────────────▶ │ Codebuff   │
│ (LobeChat等)│ ◀──────────────────── │  (Go, 本地 :8080) │ ◀─────────────────── │  上游后端  │
└─────────────┘                        └──────────────────┘                      └────────────┘
                                              │
                       ┌──────────────────────┼──────────────────────┐
                       │  ModelRegistry       │  RunManager          │  SessionPool
                       │  实测硬编码+远程补充  │  每token保活根run     │  会话保活/排队
                       │  agent→model 映射    │  子run惰性创建        │  过期自动刷新
                       └──────────────────────┴──────────────────────┘
```

### 关键设计

**1. 模型注册表：硬编码为底座，远程拉取为补充**

上游 `free-agents.ts` 重构后改用 `FREEBUFF_*_MODEL_ID` 常量引用，正则再也解析不出字面量。策略改为：硬编码一份**实测可用**的 agent→model 映射作为权威底座，远程解析结果只作增量合并，保证列表只增不减。

**2. Run 生命周期管理**

- 启动只预热 1 个根 run（不预热全部 28 个 agent，避免无意义的 START/FINISH 风暴）。
- 子 run 首次用到才创建，祖先指向当前根。
- 根 run 到期轮换时，子 run 跟着轮换以指向新根。
- 请求结束释放租约，轮换下来的旧 run 排空后 FINISH。

**3. 会话管理**

- 每个 token 缓存一个会话，过期前 5 秒自动刷新。
- 排队态（waiting room）返回 `Retry-After` 给客户端，不傻等。
- 401 时把 token 打入 30 分钟冷却，避免连环失败。

**4. 双协议出口**

- `/v1/chat/completions`：标准 OpenAI，流式/非流式都支持。
- `/v1/messages`：Claude 协议，做了请求/响应双向格式转换。
- `/v1/models`：返回实测可用模型列表。

---

## 五、实测验证

```
GET /v1/models
→ gemini-2.5-flash-lite, gemini-3.1-flash-lite-preview

POST /v1/chat/completions  (gemini-2.5-flash-lite, "Say OK")
→ {"choices":[{"message":{"content":"Alright"}}], "usage":{...}}

POST /v1/chat/completions  (stream: true, "17*23?")
→ data: {...391...}  data: [DONE]   ← 流式正常

POST /v1/messages  (Claude 协议)
→ {"content":[{"text":"All right.","type":"text"}], "role":"assistant", ...}
```

---

## 六、经验总结

1. **抓包 > 文档**。`free-agents.ts` 写了不等于后端放行，一切以实测为准。
2. **私有协议的约束往往在隐藏字段里**。`codebuff_metadata` 的四个字段、`ancestorRunIds` 的层级语义，缺一个就 400。
3. **`null` ≠ `[]`**。Go 的 nil slice 序列化成 null，被上游 schema 校验拒了，改成空切片解决。
4. **TLS 指纹是绕不过的坎**。Serverless（Worker）的 TLS 栈被识别，本地原生 TLS 才能过——这决定了部署形态。
5. **层级资源要建模对**。把"会话→根run→子run"的树形约束在代码里显式建模（根 run 单独字段），而不是平铺在一个 map 里靠约定，能避免一整类 bug。

---

## 项目地址

- **Freebuff2API**：https://github.com/Quorinex/Freebuff2API
- 单文件 Go 二进制，`config.json` 填入 authToken 即可跑
- Token 获取：https://freebuff.llm.pm （登录后直接显示）

> 免责声明：本项目仅供学习交流，与 Codebuff/Freebuff 无官方关联。请遵守上游服务条款，控制调用频率。

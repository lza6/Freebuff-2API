# Freebuff2API

> 中文文档。English version: [README_en.md](README_en.md)

Freebuff2API 是一个本地代理服务器，将 [Codebuff Freebuff](https://www.codebuff.com) 免费层逆向为 **OpenAI 兼容** 与 **Claude 兼容** 的 API 端点。运行一个 Go 二进制，即可在任意 OpenAI/Claude 客户端、SDK 或命令行工具中使用 Freebuff 的免费模型。

> 完整的逆向实录（协议、踩坑、架构）见 **[REVERSE_ENGINEERING.md](REVERSE_ENGINEERING.md)**。

## 核心特性

- **双协议出口** — `POST /v1/chat/completions`（OpenAI，流式/非流式）与 `POST /v1/messages`（Claude），支持 LobeChat、NextChat、Claude Code、Codex、Cursor 及任意 OpenAI SDK。
- **自动 run 层级** — 自动维护上游要求的 会话 → 根 run（`base2-free`）→ 子 agent run 树；子 run 首次使用时惰性创建，且仅以根 run 作为唯一祖先。
- **会话保活** — freebuff 会话过期前自动刷新，排队时返回 `Retry-After`，遇到 401 自动冷却 token。
- **多 Token 轮换** — 支持多个 auth token 周期轮换，按 token 分配并发租约。
- **实测模型注册表** — 内置经端到端实测验证的可用组合硬编码清单，并定期拉取上游 `free-agents.ts` 作为增量补充（只增不减）。
- **HTTP 代理支持** — 可为所有出站请求配置上游 HTTP 代理。

## 获取 Auth Token

Freebuff2API 需要至少一个 Freebuff **auth token**。

### 方式一 — 网页获取（推荐）

访问 **[https://freebuff.llm.pm](https://freebuff.llm.pm)**，使用 Freebuff 账号登录后，页面会直接显示你的 auth token，复制即可。

### 方式二 — Freebuff CLI

```bash
npm i -g freebuff
freebuff   # 首次启动会引导你完成登录
```

登录后，token 会保存到本地凭证文件：

| 系统 | 凭证文件路径 |
|---|---|
| Windows | `C:\Users\<用户名>\.config\manicode\credentials.json` |
| Linux / macOS | `~/.config/manicode/credentials.json` |

从该文件中复制 `authToken` 的值，即为所需的 **AUTH_TOKENS**。

> **提示：** 可配置多个账号的 token 以提升并发吞吐量。

## 配置指南

支持 JSON 文件和环境变量两种配置方式（键名一致）。默认读取当前目录的 `config.json`，可用 `-config` 指定其他路径。参考 `config.example.json`。

```json
{
  "LISTEN_ADDR": ":8080",
  "UPSTREAM_BASE_URL": "https://codebuff.com",
  "AUTH_TOKENS": ["token"],
  "ROTATION_INTERVAL": "6h",
  "REQUEST_TIMEOUT": "15m",
  "API_KEYS": [],
  "HTTP_PROXY": ""
}
```

| 属性 / 环境变量 | 说明 |
|---|---|
| `LISTEN_ADDR` | 代理监听地址（默认 `:8080`） |
| `UPSTREAM_BASE_URL` | Freebuff 后端地址（默认 `https://codebuff.com`） |
| `AUTH_TOKENS` | Freebuff auth token（JSON 数组或逗号分隔的环境变量） |
| `ROTATION_INTERVAL` | Run 自动轮换间隔（默认 `6h`） |
| `REQUEST_TIMEOUT` | 上游请求超时时间（默认 `15m`） |
| `API_KEYS` | 客户端鉴权 API Key（留空则无需鉴权） |
| `HTTP_PROXY` | 上游 HTTP 代理地址 |

同时设置时，环境变量优先于 JSON 配置文件。

## 可用模型与上游限制

可用模型列表**并不等于** Freebuff 官方宣传的全部模型——上游免费层目前在请求时会拒绝绝大多数模型。下表为**实际可用**的组合，已对线上游逐一做过端到端实测（2026-07）。

| 模型 | 状态 |
|---|---|
| `google/gemini-2.5-flash-lite` | ✅ 可用（聊天、流式、Claude 协议均验证通过） |
| `google/gemini-3.1-flash-lite-preview` | ✅ 可用 |
| `deepseek/deepseek-v4-pro`、`deepseek/deepseek-v4-flash` | ❌ 上游拒绝（`free_mode_invalid_agent_model`） |
| `minimax/minimax-m3`、`z-ai/glm-v5.2`、`moonshotai/kimi-k2-thinking` | ❌ 上游拒绝 |
| `xiaomi/mimo-v2.5-flash/pro`、`hy3/hy3*`、`poolside/laguna-s-2-1*` | ❌ 上游拒绝 |

**为什么会有出入：** Codebuff 免费层收紧了校验。他们开源仓库里的 `free-agents.ts` 仍然列着很多模型，但后端现在只认特定的 agent + model 组合，非 Gemini 模型一律返回：

```json
{"error":"free_mode_invalid_agent_model","message":"Free mode is only available for specific agent and model combinations."}
```

因此模型注册表内置了一份经实测筛选的硬编码清单（见 `models.go`）作为权威底座，仍会定期拉取上游 `free-agents.ts` 作为补充——即使上游源码重构（例如改用 `FREEBUFF_*_MODEL_ID` 常量引用，正则无法解析）也不会让可用列表缩水。

### Run 层级（为什么重要）

Freebuff 强制要求以会话为根的 run 树：

1. 必须先建立**会话**（`POST /api/v1/freebuff/session`）。
2. 必须在会话下启动**根 run**（`base2-free`）。
3. 任何**子 agent run**（如 `file-picker`、`code-reviewer-*`）必须在 `ancestorRunIds` 中声明根 run，且只能声明根 run。

违反此规则会返回 `free_mode_invalid_agent_hierarchy`。Freebuff2API 会自动处理：每个 token 保活一个根 run 并按周期轮换；子 agent run 在首次使用时惰性创建，且仅以根 run 作为唯一祖先。

## 使用示例

```bash
# OpenAI 协议
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"google/gemini-2.5-flash-lite","messages":[{"role":"user","content":"你好"}]}'

# Claude 协议
curl http://localhost:8080/v1/messages \
  -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{"model":"google/gemini-2.5-flash-lite","max_tokens":1024,"messages":[{"role":"user","content":"你好"}]}'

# 模型列表
curl http://localhost:8080/v1/models
```

将任意 OpenAI SDK 的 base_url 指向 `http://localhost:8080/v1`，或为 Claude Code 设置 `ANTHROPIC_BASE_URL=http://localhost:8080` 即可。

## 部署运行

### 源码编译

**环境要求：** Go 1.23+

```bash
git clone https://github.com/lza6/Freebuff-2API.git
cd Freebuff-2API
go build -o freebuff2api .
./freebuff2api -config config.json
```

### Docker 部署

```bash
docker build -t freebuff2api .
docker run -d -p 8080:8080 -e AUTH_TOKENS="token1,token2" freebuff2api
```

> 仓库自带 GitHub Actions 工作流（`.github/workflows/docker.yml`），推送时会构建多架构镜像。如需发布到自己的 GHCR，请先修改其中的 `IMAGE_NAME` 环境变量为你的命名空间。

## Cloudflare Worker（实验性，暂不可用）

`cfworker/` 目录为 Cloudflare Worker 移植版，**目前对线上游不可用**：Codebuff 会以 `free_mode_cli_required` 拒绝来自 Worker 的请求，这是 TLS 指纹层（Client Hello / JA3）的检测，无法通过修改 HTTP 头绕过。本地 Go 二进制的原生 TLS 栈可通过。请使用 Go 服务端，`cfworker/` 仅供研究参考。

## 友情链接

- [linux.do](https://linux.do)

## 免责声明

本项目与 OpenAI、Codebuff 或 Freebuff 无任何官方关联，相关商标和版权均归其各自所有者所有。

本仓库的所有内容仅供交流、实验和学习使用，不构成任何生产环境服务或专业建议。本项目按"原样（As-Is）"提供，使用者需自行承担使用风险。作者不对因使用、修改或分发本项目而导致的任何直接或间接损失承担责任，亦不提供任何形式的明示或暗示保证。

## 开源协议

MIT

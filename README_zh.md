# Freebuff2API

> 简体中文。主文档：[README.md](README.md)（中文）｜English: [README_en.md](README_en.md)

Freebuff2API 是 [Freebuff](https://freebuff.com) 的 OpenAI 兼容代理服务器。本项目将标准 OpenAI API 请求转化为 Freebuff 后端格式，让你能在任何 OpenAI 兼容客户端、SDK 或命令行工具中直接使用 Freebuff 的免费模型。

## 核心特性

- **OpenAI 兼容 API** — 标准 OpenAI 端点，开箱即用，支持任意兼容客户端。
- **高隐匿性请求处理** — 动态随机客户端特征标识，模拟官方 Freebuff SDK 行为模式。
- **多 Token 轮换** — 支持多个认证 Token，内置定期自动轮换机制。
- **HTTP 代理支持** — 可为所有外部请求配置上游 HTTP 代理。

## 获取 Auth Token

Freebuff2API 需要至少一个 Freebuff **Auth Token**。目前有以下两种获取方式：

### 方式一 — 网页获取（推荐）

访问 **[https://freebuff.llm.pm](https://freebuff.llm.pm)**，使用你的 Freebuff 账号登录后，页面会直接显示你的 Auth Token。复制该值即可作为 **AUTH_TOKENS** 使用，无需在本地安装任何工具。

### 方式二 — Freebuff CLI

安装 Freebuff CLI 并完成登录：

```bash
npm i -g freebuff
```

安装完成后，在终端执行 `freebuff`，首次启动时会自动引导你完成登录。

登录后，Token 会自动保存到本地凭证文件中：

| 系统 | 凭证文件路径 |
|---|---|
| Windows | `C:\Users\<用户名>\.config\manicode\credentials.json` |
| Linux / macOS | `~/.config/manicode/credentials.json` |

文件结构如下：

```json
{
  "default": {
    "id": "user_10293847",
    "name": "张三",
    "email": "zhangsan@example.com",
    "authToken": "fa82b5c1-e39d-4c7a-961f-d2b3c4e5f6a7",
    ...
  }
}
```

将 `authToken` 的值复制出来，即为所需的 **AUTH_TOKENS**。

> **提示：** 可登录多个账号并配置所有 Token，以提升并发吞吐量。

## 配置指南

支持 JSON 文件和环境变量两种配置方式。JSON 属性名与环境变量名一致。默认在当前目录查找 `config.json`，可通过 `-config` 参数指定其他路径。

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

### 配置参考

| 属性 / 环境变量 | 说明 |
|---|---|
| `LISTEN_ADDR` | 代理监听地址（默认 `:8080`） |
| `UPSTREAM_BASE_URL` | Freebuff 后端地址（默认 `https://codebuff.com`） |
| `AUTH_TOKENS` | Freebuff Auth Token（JSON 数组或逗号分隔的环境变量） |
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

为什么会有出入：Codebuff 免费层收紧了校验。他们开源仓库里的 `free-agents.ts` 仍然列着很多模型，但后端现在只认特定的 agent + model 组合。非 Gemini 模型一律返回：

```json
{"error":"free_mode_invalid_agent_model","message":"Free mode is only available for specific agent and model combinations."}
```

因此本项目的模型注册表内置了一份**经过实测筛选的硬编码清单**（见 `models.go`）。程序仍会定期拉取上游 `free-agents.ts` 作为**增量补充**，但硬编码清单是权威底座——即使上游源码重构（例如改用 `FREEBUFF_*_MODEL_ID` 常量引用，正则无法解析）也不会让可用列表缩水。

### Run 层级（为什么重要）

Freebuff 强制要求以会话为根的 run 层级结构：

1. 必须先建立**会话**（`POST /api/v1/freebuff/session`）。
2. 必须在会话下启动**根 run**（`base2-free`）。
3. 任何**子 agent run**（如 `file-picker`、`code-reviewer-*`）必须在 `ancestorRunIds` 中声明根 run，且只能声明根 run。

违反此规则会返回 `free_mode_invalid_agent_hierarchy`。Freebuff2API 会自动处理：每个 token 保活一个根 run 并按周期轮换；子 agent run 在首次使用时惰性创建，且仅以根 run 作为唯一祖先。

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

本仓库的所有内容仅供交流、实验和学习使用，不构成任何生产环境服务或专业建议。本项目按“原样（As-Is）”提供，使用者需自行承担使用风险。作者不对因使用、修改或分发本项目而导致的任何直接或间接损失承担责任，亦不提供任何形式的明示或暗示保证。

## 开源协议

MIT

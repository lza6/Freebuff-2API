# Freebuff2API

[English](README.md) | [简体中文](README_zh.md)

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

## 部署运行

### Docker 部署

预构建多架构镜像已发布至 GHCR：

```bash
docker run -d --name Freebuff2API \
  -p 8080:8080 \
  -e AUTH_TOKENS="token1,token2" \
  ghcr.io/quorinex/freebuff2api:latest
```

手动构建：

```bash
docker build -t Freebuff2API .
docker run -d -p 8080:8080 -e AUTH_TOKENS="token1,token2" Freebuff2API
```

### 源码编译

**环境要求：** Go 1.23+

```bash
git clone https://github.com/Quorinex/Freebuff2API.git
cd Freebuff2API
go build -o Freebuff2API .
./Freebuff2API -config config.json
```

## 友情链接

- [linux.do](https://linux.do)

## 免责声明

本项目与 OpenAI、Codebuff 或 Freebuff 无任何官方关联，相关商标和版权均归其各自所有者所有。

本仓库的所有内容仅供交流、实验和学习使用，不构成任何生产环境服务或专业建议。本项目按“原样（As-Is）”提供，使用者需自行承担使用风险。作者不对因使用、修改或分发本项目而导致的任何直接或间接损失承担责任，亦不提供任何形式的明示或暗示保证。

## 开源协议

MIT

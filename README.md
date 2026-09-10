# Freebuff2API

> 中文文档（Rust 版）。English version: [README_en.md](README_en.md)

Freebuff2API 将 [Freebuff](https://freebuff.com) 免费层逆向为 **OpenAI 兼容** 与 **Anthropic 兼容** 的本地 API 网关。**Rust(axum) 实现**，单二进制零依赖，可在任意 OpenAI/Claude 客户端（Claude Code、Codex、Cursor、LobeChat 等）中使用 Freebuff 免费模型。

## 核心特性

- **双协议出口** — `POST /v1/chat/completions`（OpenAI，流式/非流式）+ `POST /v1/messages`（Claude），适配任意 OpenAI SDK。
- **多账号智能轮询** — 多 Bearer token / web Cookie，健康评分 + 冷却熔断 + 最优账号选择。
- **双桶并发信号量** — 逆向自桌面端：免费 `{付费槽:1, 普通:3}`、订阅 `{付费槽:3, 普通:8}`。
- **会话保活** — 45s 心跳 + 广告刷新延长额度；排队返回 Retry-After；401 自动冷却。
- **思考程度降级** — 逆向自上游 efforts 字段：glm/deepseek 支持 `low/high/max`，solar/minimax/mimo 不支持自动剥离；Codex 选超范围 effort 自动降级。
- **余额/积分查询** — `GET /api/account/balance`：freebucks 积分、每模型每日剩余、套餐、地区限制。
- **token 一键导入** — 粘贴 curl / HAR / Cookie 串自动解析入库；桌面版托盘「一键登录」内置浏览器自动抓 Cookie。
- **web 版协议适配** — `POST /api/chat/stream`（Cookie 鉴权 SSE 11 事件）、多模态上传、工具调用映射。
- **用量统计** — SQLite 记录请求/token/延迟/错误 + 内置控制面板（`/ui`）。
- **桌面安装包** — Electron 壳自动拉起网关 + 托盘 + OAuth 一键登录 + 检查更新。
- **Docker / CI** — 多阶段镜像 + GitHub Actions 自动构建安装包。

## 快速开始

### 桌面版（推荐）
1. 下载 `Freebuff2API Setup 0.3.0.exe`（Release 页）
2. 安装后双击 → 自动拉起网关 + 打开控制台
3. 托盘「一键登录新账号」→ 浏览器登录 freebuff.com → 自动抓 Cookie 入库

### 源码
```bash
build.bat                      # Windows 编译
./target/release/freebuff2api  # Linux/macOS 编译 cargo build --release
start.bat                      # Windows 启动
```

### Docker
```bash
docker build -t freebuff2api -f docker/Dockerfile .
docker run -d -p 47821:47821 -v /data:/data freebuff2api
```

## 配置（config.json）

```jsonc
{
  "listen_addr": "127.0.0.1:47821",
  "upstream_base_url": "https://www.codebuff.com",
  "auth_tokens": ["bearer-token-1", "bearer-token-2"],
  "api_keys": ["sk-local"],
  "http_proxy": "http://127.0.0.1:10808",
  "ad_providers": ["gravity"],
  "sqlite_path": "data/freebuff2api.sqlite",
  "token_saver": false
}
```

环境变量优先：`AUTH_TOKENS` / `API_KEYS` / `HTTP_PROXY` / `LISTEN_ADDR` / `AD_PROVIDERS` / `SQLITE_PATH`。

## API

| 端点 | 方法 | 说明 |
|------|------|------|
| `/v1/chat/completions` | POST | OpenAI 聊天 |
| `/v1/messages` | POST | Claude 聊天 |
| `/v1/models` | GET | 模型列表 |
| `/api/tokens/import` | POST | 导入 curl/HAR/Cookie |
| `/api/account/balance` | GET | 账号积分/每模型剩余 |
| `/api/account/detail` | POST | 账号详情卡片 |
| `/api/usage/*` | GET | 用量统计 |
| `/ui` | GET | 控制面板 |
| `/healthz` | GET | 健康检查 |

完整教程见 [docs/API_GUIDE.md](docs/API_GUIDE.md)。

## 多账号轮询与并发
- 每请求自动选健康度最高的账号
- 上游双桶并发限制（逆向自桌面端 orchestrator.js）：免费 `{slot:1, multi:3}`、订阅 `{slot:3, multi:8}`
- 等待室：429 + retry-after 自动退避

## 思考程度支持矩阵（逆向自上游）

| 模型 | 支持 efforts |
|------|-------------|
| deepseek/*、z-ai/glm、stealth/ox-alpha | `low, high, max` |
| openai/gpt-5.6*、gemini-3.8、claude-fable-5 | `low, medium, high, xhigh, max` |
| meta/muse-spark* | `minimal, low, medium, high, xhigh` |
| solar-pro4、minimax-m3、mimo-v2.5、kimi-k3 | 不支持（自动剥离） |

## 测试与验证

```bash
cargo test        # 32 项测试全绿（24 单元 + 8 集成）
cargo clippy      # 零警告
```

真实 E2E 已实测：token 导入（curl/HAR/Cookie）✅、余额查询 ✅、账号详情 ✅、面板 ✅、上游冒烟 ✅。

## 目录结构

```
src/                Rust 网关源码
  api.rs            HTTP 路由
  web_protocol.rs   web 版协议（Cookie 鉴权 chat/stream/余额）
  import.rs         token 导入解析
  usage.rs          SQLite 用量统计
desktop/            Electron 桌面壳
legacy-go/          旧 Go 版实现（归档）
reference/          上游逆向源码归档
docs/               API 教程
```

## 免责声明

本项目与 OpenAI、Codebuff、Freebuff 无官方关联。仅供交流、实验与学习使用，按"原样"提供，使用者自行承担风险。

## 开源协议

MIT

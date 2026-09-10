# Freebuff2API — Rust 版

基于逆向上游 Freebuff-0.0.98 桌面端协议的全新实现，提供 **OpenAI 兼容 + Anthropic 兼容 + 多账号智能轮询 + 广告保活 + 用量统计面板** 的本地 API 网关。

## 核心特性

| 特性 | 说明 |
|------|------|
| 🚀 Rust(axum) 高性能网关 | 单二进制、内存友好、~17MB，无运行时依赖 |
| 🔄 多账号智能轮询 | 会话健康评分 + 冷却熔断 + 最优 token 选择 |
| ⏱️ 会话保活 | 每 45s 心跳 + `x-freebuff-heartbeat`，过期前广告刷新延长额度 |
| 📊 用量统计 | SQLite 记录请求/token/延迟/错误，内置控制面板 |
| 🧭 模型路由降级 | 主模型失败自动切换备选免费模型 |
| 🖥️ 桌面安装包 | Electron 壳 + 进程自启动（`desktop/` 目录）|
| 🐳 Docker 镜像 | 多阶段构建，全静态二进制 |

## 快速开始

```bash
# 1. 配置
cp config.example.json config.json
# 填入 AUTH_TOKENS（多个用逗号分隔）

# 2. 运行
cargo run --release -- --config config.json
# 或直接运行预编译二进制
./target/release/freebuff2api.exe --config config.json
```

## 配置项

```jsonc
{
  "listen_addr": "127.0.0.1:47821",     // 监听地址（桌面版固定本机）
  "upstream_base_url": "https://www.codebuff.com",
  "auth_tokens": ["token1","token2"],  // 多账号轮询
  "api_keys": ["sk-local"],            // 网关鉴权（空则不校验）
  "http_proxy": "http://127.0.0.1:10808",  // 支持 http/socks5
  "ad_providers": ["gravity"],         // 广告保活 provider
  "sqlite_path": "data/freebuff2api.sqlite",
  "token_saver": false
}
```

## API

| 端点 | 说明 |
|------|------|
| `GET /healthz` | 健康 + 账号健康度快照 |
| `GET /v1/models` | OpenAI 模型列表 |
| `POST /v1/chat/completions` | OpenAI 兼容聊天 |
| `POST /v1/messages` | Anthropic 兼容聊天 |
| `GET /` `GET /ui` | 内置控制面板 |
| `GET /api/usage/totals` | 用量汇总 |
| `GET /api/usage/daily` | 按日/模型统计 |
| `GET /api/usage/requests` | 最近请求 |
| `GET /api/usage/accounts` | 账号健康 |

## 协议逆向来源

协议细节完全逆向自 `reference/reverse/orchestrator/orchestrator.js`（Freebuff-0.0.98 桌面端打包产物）：

- `POST /api/v1/freebuff/session` 带 `x-freebuff-model` `x-freebuff-instance-id` `x-freebuff-multi-session: 1`
- `GET /api/v1/freebuff/session` 带 `x-freebuff-include-unused-rate-limits` 拉取账号可用 RateLimits
- 心跳 `GET` + `x-freebuff-heartbeat: 1`
- `POST /api/v1/agent-runs`（START/FINISH + `ancestorRunIds`）
- `POST /api/v1/ads`（广告拍卖）→ `/api/v1/ads/impression`（first_party 确认）
- `POST /api/v1/chat/completions`（OpenAI 兼容，注入 `codebuff_metadata`）

## 目录结构

```
src/
├── main.rs      # 启动与依赖装配
├── config.rs    # 配置加载（JSON+环境变量）
├── models.rs    # 模型注册表（上游拉取+硬编码底座）
├── upstream.rs  # 上游 Codebuff HTTP 客户端
├── session.rs   # Freebuff 会话管理（排队/活跃/心跳）
├── pool.rs      # 多账号池（评分/冷却/熔断）
├── ads.rs       # 广告换 token 保活
├── router.rs    # 模型路由降级 + token 节省
├── usage.rs     # SQLite 用量统计
├── web.rs       # 内嵌控制面板
└── api.rs       # HTTP 路由
desktop/       # Electron 桌面壳（打包 NSIS 安装包）
legacy-go/     # 上一版 Go 实现（已归档）
reference/     # 上游逆向源码归档
```

## 测试

```bash
cargo test        # 单元+集成测试
cargo clippy      # 静态检查零警告
```

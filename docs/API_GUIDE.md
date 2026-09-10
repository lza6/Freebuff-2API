# Freebuff2API API 文档与使用教程

Rust 版 OpenAI/Anthropic 兼容网关 + 多账号轮询 + 余额查询 + 桌面安装包。

## 快速开始

### 方式一：桌面安装包（推荐）
1. 下载 `Freebuff2API Setup 0.1.0.exe`（Release 页）
2. 安装后双击桌面快捷方式 → 自动拉起网关 + 打开控制台
3. 托盘「一键登录新账号」→ 浏览器登录 freebuff.com → 自动抓 Cookie 入库

### 方式二：源码
```bash
# 编译
build.bat          # 或 cargo build --release
# 启动
start.bat          # 或 ./target/release/freebuff2api.exe --config config.json
```

### 方式三：Docker
```bash
docker build -t freebuff2api -f docker/Dockerfile .
docker run -d -p 47821:47821 -v /data:/data freebuff2api
```

## 配置（config.json）

```jsonc
{
  "listen_addr": "127.0.0.1:47821",
  "upstream_base_url": "https://www.codebuff.com",
  "auth_tokens": ["token1", "token2"],   // 桌面版 API token（codebuff.com）
  "api_keys": ["sk-local"],               // 本网关鉴权（空则不校验）
  "http_proxy": "http://127.0.0.1:10808", // 支持 http/socks5
  "ad_providers": ["gravity"],            // 广告保活
  "sqlite_path": "data/freebuff2api.sqlite",
  "token_saver": false
}
```

环境变量优先：`AUTH_TOKENS` / `API_KEYS` / `HTTP_PROXY` / `LISTEN_ADDR` / `UPSTREAM_BASE_URL` / `AD_PROVIDERS` / `SQLITE_PATH`。

## API 端点

### OpenAI 兼容
| 端点 | 方法 | 说明 |
|------|------|------|
| `/v1/chat/completions` | POST | 聊天（流式/非流式），自动多账号轮询 |
| `/v1/models` | GET | 可用模型列表 |
| `/v1/messages` | POST | Anthropic 协议聊天 |

### 账号与凭证
| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/tokens/import` | POST | 粘贴 curl/HAR/Cookie 自动提取凭证入库 |
| `/api/tokens` | GET | 列出已导入 token（脱敏） |
| `/api/account/balance` | GET | 账号积分/每模型剩余次数/套餐/地区限制 |
| `/api/account/detail` | POST | 账号详情卡片（余额+用量+用户+套餐） |

### 用量统计
| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/usage/totals` | GET | 总请求/Token/错误 |
| `/api/usage/daily` | GET | 按日/模型统计 |
| `/api/usage/requests` | GET | 最近请求明细 |
| `/api/usage/accounts` | GET | 账号健康度 |

### 面板与健康
| 端点 | 方法 | 说明 |
|------|------|------|
| `/` `/ui` | GET | 内置控制面板 |
| `/healthz` | GET | 健康检查 |

## 多账号轮询策略
- 每请求自动选评分最高的健康账号（冷却中/错误多自动降权）
- 上游并发双桶限制：免费 `{付费槽:1, 普通:3}`、订阅 `{付费槽:3, 普通:8}`（逆向自桌面端）
- 等待室：429 + retry-after，自动退避

## 思考程度（reasoning_effort）支持矩阵（逆向自上游）

| 模型 | 支持 efforts |
|------|-------------|
| deepseek/*、z-ai/glm、stealth/ox-alpha | `low, high, max` |
| openai/gpt-5.6*、google/gemini-3.8、claude-fable-5 | `low, medium, high, xhigh, max` |
| meta/muse-spark* | `minimal, low, medium, high, xhigh` |
| solar-pro4、minimax-m3、mimo-v2.5、kimi-k3 | **不支持**（自动剥离） |

> 在 Codex/Claude Code 选了不支持思考程度的模型 → 网关自动剥离或降级到支持上限。

## 常见问题

### 为什么余额查不到？
余额端点需要 **web 版 Cookie**（`__Secure-next-auth.session-token=...`），不是桌面版 Bearer token。浏览器登录 freebuff.com → DevTools → Copy as cURL → 粘贴到 `/api/tokens/import`。

### 如何多账号？
桌面版：多个 Bearer token 填 `auth_tokens`。web 版：多次「一键登录」或多次导入 Cookie。

### 0 积分模型
`upstage/solar-pro4` 0 积分免费（仍有每日 6 次池限制）；`z-ai/glm-5.3-flash` 走 Reward 奖励池。

## 更新
- 桌面版：托盘检查更新（electron-updater）
- Docker：`docker pull` 最新镜像
- GitHub Actions 自动构建：打 tag `v*` 自动出安装包
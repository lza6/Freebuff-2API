# Freebuff2API API 文档与使用教程

Rust 版 OpenAI/Anthropic 兼容网关 + 多账号轮询 + 余额查询 + 桌面安装包。

## 快速开始

### 方式一：桌面安装包（推荐）
1. 下载 `Freebuff2API Setup 0.3.0.exe`（Release 页）
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
  "api_keys": ["sk-local"],               // 本网关鉴权（空则不校验；配置后面板右上角填 Key）
  "http_proxy": "http://127.0.0.1:10808", // 支持 http/socks5
  "ad_providers": ["gravity"],            // 广告保活
  "sqlite_path": "data/freebuff2api.sqlite",
  "telemetry_path": "data/telemetry.sqlite",  // 请求详情/事件链
  "tokens_path": "data/tokens.json",          // 导入凭证存储位置
  "skills_dir": "data/skills",                // 技能目录（SKILL.md）
  "skills_inject_mode": "roster",             // roster（名称+描述）| full（全量）
  "max_roster_tokens": 2000,                  // roster 注入预算
  "token_saver": false
}
```

环境变量优先：`AUTH_TOKENS` / `API_KEYS` / `HTTP_PROXY` / `LISTEN_ADDR` / `UPSTREAM_BASE_URL` / `AD_PROVIDERS` / `SQLITE_PATH` / `TELEMETRY_PATH` / `TOKENS_PATH` / `SKILLS_DIR` / `SKILLS_INJECT_MODE` / `MAX_ROSTER_TOKENS`。

## API 端点

### OpenAI / Anthropic 兼容
| 端点 | 方法 | 说明 |
|------|------|------|
| `/v1/chat/completions` | POST | 聊天（流式/非流式），自动多账号轮询 |
| `/v1/models` | GET | 可用模型列表 |
| `/v1/messages` | POST | Anthropic 协议聊天（流式为标准 Anthropic 事件流） |
| `/v1/web/chat` | POST | web 协议对话（Cookie 鉴权；`images` 支持多模态） |
| `/v1/uploads` | POST | 上传文件换 storageId（裸 body + `x-file-name` 头，上限 20MB） |

### 账号与凭证
| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/tokens/import` | POST | 粘贴 curl/HAR/Cookie 自动提取凭证入库（也接受 `{"cookie":"..."}` JSON）；同值自动去重，返回 `added` |
| `/api/tokens` | GET | 列出已导入凭证：稳定 `id`、掩码、类型、入库时间、最近一次账号信息缓存 `meta` |
| `/api/tokens/check` | POST | 对指定凭证拉取账号全貌并刷新缓存 `{id}` → `{ok, valid, meta}` |
| `/api/tokens/delete` | POST | 删除凭证 `{id}`（同时移出运行中的账号池） |
| `/api/account/overview` | GET | 账号全貌（身份/用量统计/套餐/额度积分），聚合上游 4 个端点 |
| `/api/account/history` | GET | 账号使用记录（每次检查/刷新一条快照）`?cred_id=&limit=` |
| `/api/account/balance` | GET | 账号积分/每模型剩余次数/套餐/地区限制 |
| `/api/account/detail` | POST | 账号详情卡片（余额+用量+用户+套餐） |
| `/api/account/refresh` | POST | 凭证保活检查（调上游 convex-token 验证 Cookie 是否有效） |
| `/api/guide` | GET | 客户端接入信息：监听地址、OpenAI/Anthropic 地址、Key 状态、模型数与样例 |

### 浏览器扩展
| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/extension/bundle` | GET | 下载一键登录扩展 zip（内容编译期内嵌，单文件分发亦可用；配 api_keys 时可用 `?key=`） |

### 配置
| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/config/api-key` | POST | 运行时管理下游 API Key：`{"action":"generate"\|"set"\|"clear","key"?}`；**立即生效**并写回 `config.json`（非本机监听时禁止清空） |

### 技能（Skills）
| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/skills` | GET | 技能列表 + roster 注入预览（含 token 预算） |
| `/api/skills` | POST | 新建/更新技能 `{id?, name, description, body, force?}` |
| `/api/skills/toggle` | POST | 启用/禁用 `{id, enabled}` |
| `/api/skills/delete` | POST | 删除自定义技能 `{id}`（内置不可删） |
| `/api/skills/gate` | POST | 质量门预检 `{body}` → `{issues: []}` |

### 可观测
| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/logs/stream` | GET | 实时日志 SSE（支持 `Last-Event-ID` 断线补发；配 api_keys 时用 `?key=`） |
| `/api/logs/recent` | GET | 最近日志（`?limit=200`） |
| `/api/usage/requests/{id}` | GET | 单条请求详情（含遥测富字段与事件链） |
| `/api/usage/cost` | GET | 速率与错误率（30 分钟滑窗；诚实标注 estimated） |
| `/api/doctor` | GET | 系统体检（四态：ok/fault/unknown/fact） |

### 记忆（AI 更懂用户）
| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/memory` | GET | 记忆列表 + 统计（总数/稳定事实/纠正数） |
| `/api/memory` | POST | 手动新增 `{kind, title, content, is_static?}` |
| `/api/memory/delete` | POST | 删除 `{id}` |
| `/api/memory/static` | POST | 标记/取消稳定事实 `{id, is_static}` |

> 自动记录（零 LLM）：常用模型偏好、推理档位降级、用户纠正语句（"记住…/别再…/always/never"）。记忆按当前问题检索后注入 system 前缀（512 token 预算、低权威块）。

### MCP（只读工具）
| 端点 | 方法 | 说明 |
|------|------|------|
| `/mcp` | POST | JSON-RPC 2.0（`initialize` / `tools/list` / `tools/call` / `ping`）；工具：`list_models`、`list_accounts`、`usage_summary`；鉴权与 /v1 一致 |

### 用量统计
| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/usage/totals` | GET | 总请求/Token/错误 |
| `/api/usage/daily` | GET | 按日/模型统计 |
| `/api/usage/requests` | GET | 最近请求明细 |
| `/api/usage/models` | GET | 模型列表（代理注册表） |
| `/api/usage/accounts` | GET | 账号健康度 |

### 面板与健康
| 端点 | 方法 | 说明 |
|------|------|------|
| `/` `/ui` | GET | 内置控制面板（总览/账号/技能/日志/体检/接入指南） |
| `/healthz` | GET | 健康检查 |
| `/api/prompts`、`/api/prompts/toggle` | GET/POST | 内置提示词（旧接口，保留兼容） |

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

### 浏览器版怎么一键登录？
上游登录 Cookie 是 **HttpOnly**，网页脚本读不到，所以浏览器版必须借助扩展：

1. 面板「账号」页 → 点「⬇ 下载扩展」得到 zip（或直接用项目里的 `browser-extension/` 目录）→ 解压
2. 浏览器打开 `chrome://extensions`（Edge 为 `edge://extensions`）→ 打开「开发者模式」→「加载已解压的扩展程序」→ 选中解压目录
3. 回到面板 → 点「重新检测」，状态变成 **扩展已就绪** → 点「🔑 一键登录」

之后是**全自动**的：扩展自动打开 freebuff.com → 你完成 GitHub 登录 → 扩展自动把凭证写回网关 → 面板自动刷新出账号全貌。

没装扩展也能用：点「一键登录」会打开 freebuff.com 并给出 3 步复制向导（约 30 秒）。桌面版（Electron）则由主进程直接读取，托盘一键全自动。

### 为什么余额查不到？
余额端点需要 **web 版 Cookie**（`__Secure-next-auth.session-token=...`），不是桌面版 Bearer token。浏览器登录 freebuff.com → DevTools → Copy as cURL → 粘贴到 `/api/tokens/import`。

### 导入凭证后怎么开始请求？
面板「总览」页顶部「🚀 立刻开始请求」卡片直接给出 Base URL 与 API Key（可一键复制）：

- OpenAI 协议：`http://127.0.0.1:47821/v1`（Cursor / LobeChat / SDK）
- Anthropic 协议：`http://127.0.0.1:47821`（Claude Code）
- API Key：未配置 `api_keys` 时任意非空字符串即可（如 `sk-local`）；点「生成并启用 Key」可一键生成并立即生效

「接入指南」页有各客户端的现成配置片段，直接复制即可。

> **只导入 web Cookie（一键登录）也能用 /v1**：账号池没有 Bearer token 时，网关会把 `/v1/chat/completions` 与 `/v1/messages` 自动桥接到上游 web 协议（多轮对话复用同一个上游 thread，避免烧光每日会话额度）。

### 凭证列表里的"今日剩余"是什么意思？
来自上游 `/api/web/freebuff-session` 的 `freebucks.daily` 与逐模型 `rateLimitsByModel`：
- **今日剩余** = 每日积分（freebucks）剩余额度，太平洋时间午夜重置，隔天自动刷新
- 逐模型「今日剩余次数」= 该模型当日可用次数上限 − 已用（点凭证行「详情」查看）
- 每次「刷新账号全貌」或「检查」都会往「使用记录」追加一条快照，可按账号查询历史

### 如何多账号？
桌面版：多个 Bearer token 填 `auth_tokens`。web 版：多次「一键登录」或多次导入 Cookie。凭证按值自动去重；每行可单独「检查 / 详情 / 删除」。

### 0 积分模型
`upstage/solar-pro4` 0 积分免费（仍有每日 6 次池限制）；`z-ai/glm-5.3-flash` 走 Reward 奖励池。

## 更新
- 桌面版：托盘检查更新（electron-updater）
- Docker：`docker pull` 最新镜像
- GitHub Actions 自动构建：打 tag `v*` 自动出安装包
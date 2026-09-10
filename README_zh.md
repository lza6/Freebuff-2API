# Freebuff2API（中文使用手册）

> Rust(axum) 版。主文档：[README.md](README.md)｜English: [README_en.md](README_en.md)

Freebuff2API 把 [Freebuff](https://freebuff.com) 免费层的模型逆向为 **OpenAI 兼容**与 **Anthropic 兼容** 的本地 API 网关。单二进制、零依赖，可在任意 OpenAI/Claude 客户端（Claude Code、Codex、Cursor、LobeChat、NextChat 等）中使用 Freebuff 免费模型。

---

## 一、快速开始（3 步）

### 第 1 步：安装

**桌面版（推荐，小白首选）**

1. 打开 [Releases 页面](https://github.com/lza6/Freebuff-2API/releases) 下载 `Freebuff2API Setup x.y.z.exe`
2. 双击安装（一路「下一步」）
3. 安装完成后启动，程序会自动拉起网关并打开控制台

**源码 / 服务器**

```bash
# Windows 一键编译
build.bat

# 或手动
cargo build --release
./target/release/freebuff2api --config config.json
```

### 第 2 步：添加账号

三种方式任选其一（面板「添加账号」卡片）：

| 方式 | 适合谁 | 怎么做 |
|------|--------|--------|
| 一键登录 | 桌面版用户 | 托盘菜单 →「➕ 一键登录新账号」→ 浏览器里登录 freebuff.com → Cookie 自动入库 |
| 粘贴导入 | 有浏览器的人 | 浏览器 F12 → Network → 复制任意请求的 Cookie → 粘贴到面板导入框 |
| 文件导入 | 开发者 | 把 curl 命令或 HAR 文件内容粘贴到导入框 |

> 多账号可重复添加，网关自动轮询、健康评分、失败冷却。

### 第 3 步：接入你的客户端

网关默认监听 `http://127.0.0.1:47821`。**下面的配置直接抄**：

**Claude Code**（Anthropic 协议）

```bash
# macOS / Linux
export ANTHROPIC_BASE_URL=http://127.0.0.1:47821
export ANTHROPIC_API_KEY=sk-local   # config.json 未配置 api_keys 时可随意填

# Windows PowerShell
$env:ANTHROPIC_BASE_URL="http://127.0.0.1:47821"
$env:ANTHROPIC_API_KEY="sk-local"
```

**Cursor / Continue / 任意 OpenAI 兼容客户端**

```
Base URL: http://127.0.0.1:47821/v1
API Key:  sk-local（未配置 api_keys 时随意填）
模型:      从 http://127.0.0.1:47821/v1/models 的列表里选
```

**OpenAI SDK（Python）**

```python
from openai import OpenAI
client = OpenAI(base_url="http://127.0.0.1:47821/v1", api_key="sk-local")
resp = client.chat.completions.create(
    model="z-ai/glm-5.3-flash",   # 换成 /v1/models 里的模型
    messages=[{"role": "user", "content": "你好"}],
)
print(resp.choices[0].message.content)
```

**LobeChat / NextChat / Cherry Studio**：在设置里选「OpenAI」，接口地址填 `http://127.0.0.1:47821/v1`，密钥随意，模型名手填 `/v1/models` 列表中的值。

---

## 二、配置（config.json）

```jsonc
{
  "listen_addr": "127.0.0.1:47821",       // 监听地址（默认本机；对外提供服务再改 0.0.0.0）
  "upstream_base_url": "https://www.codebuff.com",
  "auth_tokens": ["bearer-token-1"],       // 也可留空，用面板导入
  "api_keys": [],                          // 留空=不校验客户端密钥
  "http_proxy": "",                        // 如 http://127.0.0.1:10808
  "ad_providers": ["gravity"],             // 广告保活 provider
  "sqlite_path": "data/freebuff2api.sqlite",
  "token_saver": false                     // 压缩超长 tool_result 省 token
}
```

环境变量优先：`LISTEN_ADDR` / `AUTH_TOKENS` / `API_KEYS` / `HTTP_PROXY` / `SQLITE_PATH` / `AD_PROVIDERS`。

**端口被占用？** 把 `listen_addr` 改成 `127.0.0.1:47822`（或任意空闲端口）即可；桌面版请在 `%APPDATA%\freebuff2api\config.json` 里改。

---

## 三、API 端点

| 端点 | 方法 | 说明 |
|------|------|------|
| `/v1/chat/completions` | POST | OpenAI 聊天（流式 / 非流式） |
| `/v1/messages` | POST | Claude 聊天（双向转换，流式为 Anthropic 事件流） |
| `/v1/models` | GET | 可用模型列表 |
| `/v1/web/chat` | POST | web 协议对话（Cookie 鉴权，支持多模态 images） |
| `/v1/uploads` | POST | 上传文件换取 storageId（裸 body + `x-file-name` 头） |
| `/api/tokens/import` | POST | 导入 curl / HAR / Cookie（`{"cookie": "..."}` 或纯文本） |
| `/api/tokens` | GET | 已导入凭证（脱敏） |
| `/api/account/balance` | GET | 账号积分 / 每模型今日剩余 |
| `/api/account/detail` | POST | 账号详情卡片 |
| `/api/skills` | GET / POST | 技能列表（含 roster 预览）/ 新建·更新 |
| `/api/skills/toggle` | POST | 启用 / 禁用技能 |
| `/api/skills/delete` | POST | 删除自定义技能 |
| `/api/skills/gate` | POST | 质量门预检（返回问题列表） |
| `/api/logs/stream` | GET | 实时日志（SSE，支持 `Last-Event-ID` 断线补发） |
| `/api/logs/recent` | GET | 最近日志（`?limit=200`） |
| `/api/usage/requests/{id}` | GET | 单条请求详情（含遥测与事件链） |
| `/api/usage/totals`｜`/daily`｜`/requests`｜`/models`｜`/accounts` | GET | 用量与账号统计 |
| `/api/prompts`、`/api/prompts/toggle` | GET / POST | 内置提示词（旧接口，保留兼容） |
| `/api/doctor` | GET | 系统体检 |
| `/ui` | GET | 控制面板 |
| `/healthz` | GET | 健康检查 |

---

## 四、思考程度（reasoning_effort）

上游按模型支持不同的思考深度，网关自动降级 / 剥离不支持的档位：

| 模型 | 支持范围 |
|------|---------|
| `deepseek/*`、`z-ai/glm`、`stealth/ox-alpha` | `low, high, max` |
| `openai/gpt-5.6*`、`gemini-3.8`、`claude-fable-5` | `low, medium, high, xhigh, max` |
| `meta/muse-spark*` | `minimal, low, medium, high, xhigh` |
| `solar-pro4`、`minimax-m3`、`mimo-v2.5`、`kimi-k3` | 不支持（自动剥离） |

---

## 五、常见问题（FAQ）

**Q1：提示「no healthy upstream auth token available」**
没有可用账号。打开面板 →「添加账号」，或按上文第 2 步导入。

**Q2：请求很慢 / 返回 429 / `waiting_room_queued`**
免费层有上游排队。网关会把排队状态翻译进错误信息；稍等重试，或添加更多账号提升并发。

**Q3：账号突然失效（401/403）**
Cookie 过期。网关会自动冷却该账号（10 分钟）并在日志里标注。到面板重新「一键登录」。

**Q4：桌面版启动后窗口空白 / 连不上**
多为端口冲突（47821 被占）或配置文件问题。托盘菜单 →「🩺 系统体检」可看到逐项诊断（配置 / 账号池 / 模型注册表 / 遥测 / 技能库 / 日志 / 版本），每项带修复建议；启动失败时也会弹窗提示原因（含端口冲突与日志路径）。

**Q5：Claude Code 用不了**
确认 `ANTHROPIC_BASE_URL` 没有多余斜杠，且网关在跑（浏览器打开 http://127.0.0.1:47821/healthz 应返回 JSON）。

**Q6：想看网关到底做了什么**
面板「实时日志」页显示每一次请求的路由、账号选择、重试、降级、上游错误；点开单条请求可看到「为什么慢 / 为什么失败」的人话解释。

**Q7：配置了 api_keys 后，浏览器面板打不开数据**
在面板右上角的「API Key」输入框填入你的 key（仅保存在本机浏览器 localStorage），刷新页面即可。

---

## 六、技能（Skills）

面板「技能库」可以：

- 启用 / 禁用技能（启用项的**名称与描述**会注入每次对话的 system 前缀）
- 新建 / 编辑 / 删除自定义技能（Markdown 正文 + 触发描述；保存前有质量门检查，可疑的提示注入短语会被拦截）
- 内置技能只读（不可编辑/删除，仅可启停）

技能以 **roster 模式**注入（只注入名称与描述，预算默认 2000 token，可用 `max_roster_tokens` 调整），避免技能越多、每次请求越贵。技能文件存放在 `skills_dir`（默认 `data/skills/<id>/SKILL.md`），可直接编辑文件后重启同步。

---

## 七、免责声明

本项目与 OpenAI、Codebuff、Freebuff 无官方关联，相关商标版权归各自所有者。

所有内容仅供交流、实验与学习使用，按「原样（As-Is）」提供，不构成生产服务或专业建议，使用者自行承担风险。

## 八、开源协议

MIT

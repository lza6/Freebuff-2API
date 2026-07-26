# Freebuff2API — Cloudflare Worker SaaS

将 [Codebuff](https://codebuff.com) 免费 AI 模型代理为兼容 OpenAI / Claude 格式的 API 服务。

---

## 功能特性

- **OpenAI 兼容** — `POST /v1/chat/completions`、`GET /v1/models`
- **Claude 兼容** — `POST /v1/messages`、`POST /v1/messages/count_tokens`
- **流式支持** — SSE streaming，自动在 OpenAI ↔ Claude 格式间转换
- **工具调用** — 支持 function calling / tools（部分模型）
- **用户认证** — 注册/登录、Session Cookie、API Key 双认证
- **凭证管理** — 多 Freebuff token 轮换、401 自动冷却
- **Dashboard** — 可视化凭证管理、API Key 生成、使用统计
- **Durable Objects** — 管理 run/session 生命周期，避免重复启动
- **Cron 清理** — 自动刷新模型列表、清理过期会话

---

## 部署步骤

### 1. 安装依赖

```bash
cd cfworker
npm install
```

### 2. 配置 Wrangler

`wrangler.toml` 已预配置。Worker 名称、KV/D1 ID 已设置好。

### 3. 运行数据库迁移

```bash
# 本地开发
npm run db:migrate

# 远程生产
npm run db:migrate:remote
```

### 4. 部署

```bash
npm run deploy
```

---

## 可用模型

### 支持的模型列表

| 模型 ID | 说明 | 支持工具调用 |
|---------|------|-------------|
| `minimax/minimax-m2.7` | MiniMax M2.7 | ❌ |
| `z-ai/glm-5.1` | 智谱 GLM-5.1 | ❌ |
| `google/gemini-2.5-flash-lite` | Gemini 2.5 Flash Lite | ✅ |
| `google/gemini-3.1-flash-lite-preview` | Gemini 3.1 Flash Lite (预览) | ✅ |

### 获取模型列表

```bash
curl https://your-worker.workers.dev/v1/models \
  -H "Authorization: Bearer YOUR_API_KEY"
```

---

## 使用

### Dashboard

访问 Worker URL 根路径（如 `https://shy-block-f2db.to2ai.workers.dev/`）：

1. 注册账户并登录
2. 在 **Freebuff 凭证** 页面添加你的 Freebuff auth token
3. 在 **API Keys** 页面生成一个 API Key
4. 使用下方 API 端点调用

### 获取 Freebuff Token

- 方法 1：访问 [freebuff.llm.pm](https://freebuff.llm.pm) 获取 authToken
- 方法 2：安装 Freebuff CLI，从 `~/.config/manicode/credentials.json` 获取 `authToken`

### API 调用

#### OpenAI 兼容格式

```bash
# 基础对话
curl -X POST https://your-worker.workers.dev/v1/chat/completions \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "google/gemini-2.5-flash-lite",
    "messages": [{"role": "user", "content": "你好"}]
  }'

# 流式输出
curl -X POST https://your-worker.workers.dev/v1/chat/completions \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "google/gemini-2.5-flash-lite",
    "messages": [{"role": "user", "content": "你好"}],
    "stream": true
  }'

# 工具调用（Function Calling）
curl -X POST https://your-worker.workers.dev/v1/chat/completions \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "google/gemini-2.5-flash-lite",
    "messages": [{"role": "user", "content": "今天北京天气怎么样？"}],
    "tools": [
      {
        "type": "function",
        "function": {
          "name": "get_weather",
          "description": "获取天气信息",
          "parameters": {
            "type": "object",
            "properties": {
              "city": {"type": "string", "description": "城市名称"}
            },
            "required": ["city"]
          }
        }
      }
    ]
  }'
```

#### Claude 兼容格式

```bash
# 基础对话
curl -X POST https://your-worker.workers.dev/v1/messages \
  -H "x-api-key: YOUR_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "google/gemini-2.5-flash-lite",
    "messages": [{"role": "user", "content": "你好"}],
    "max_tokens": 1024
  }'

# 流式输出
curl -X POST https://your-worker.workers.dev/v1/messages \
  -H "x-api-key: YOUR_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "google/gemini-2.5-flash-lite",
    "messages": [{"role": "user", "content": "你好"}],
    "max_tokens": 1024,
    "stream": true
  }'
```

#### Token 计数

```bash
curl -X POST https://your-worker.workers.dev/v1/messages/count_tokens \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "google/gemini-2.5-flash-lite",
    "messages": [{"role": "user", "content": "你好世界"}]
  }'
```

---

## 在 IDE 中使用

### Claude Code / Claude CLI

由于 Worker 支持 Claude Messages API，可以直接使用：

```bash
# 设置环境变量
export ANTHROPIC_API_KEY="YOUR_F2DB_API_KEY"
export ANTHROPIC_BASE_URL="https://shy-block-f2db.to2ai.workers.dev"

# 使用 Claude Code
claude

# 或使用 API
curl -X POST https://shy-block-f2db.to2api.workers.dev/v1/messages \
  -H "x-api-key: YOUR_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{"model":"google/gemini-2.5-flash-lite","max_tokens":1024,"messages":[{"role":"user","content":"你好"}]}'
```

### OpenAI 兼容 SDK

所有支持 OpenAI API 格式的客户端都可以使用：

```python
# Python (OpenAI SDK)
from openai import OpenAI

client = OpenAI(
    api_key="YOUR_F2DB_API_KEY",
    base_url="https://shy-block-f2db.to2ai.workers.dev/v1"
)

response = client.chat.completions.create(
    model="google/gemini-2.5-flash-lite",
    messages=[{"role": "user", "content": "你好"}]
)
print(response.choices[0].message.content)
```

```javascript
// JavaScript/Node.js
const OpenAI = require('openai');

const client = new OpenAI({
  apiKey: 'YOUR_F2DB_API_KEY',
  baseURL: 'https://shy-block-f2db.to2ai.workers.dev/v1'
});

const response = await client.chat.completions.create({
  model: 'google/gemini-2.5-flash-lite',
  messages: [{role: 'user', content: '你好'}]
});
console.log(response.choices[0].message.content);
```

```typescript
// TypeScript (Vercel AI SDK)
import { generateText } from 'ai';

const { text } = await generateText({
  model: 'custom',
  apiKey: 'YOUR_F2DB_API_KEY',
  baseURL: 'https://shy-block-f2db.to2ai.workers.dev/v1',
  modelName: 'google/gemini-2.5-flash-lite',
  prompt: '你好'
});
```

### Cursor

在 Cursor 设置中配置：

1. 打开 Settings → Models
2. 添加自定义 provider：
   - API URL: `https://shy-block-f2db.to2ai.workers.dev/v1`
   - API Key: `YOUR_F2DB_API_KEY`
3. 选择模型：`google/gemini-2.5-flash-lite`

### VS Code Copilot / 其他 IDE

使用 OpenAI API 格式配置即可。

---

## API 端点总览

| 端点 | 方法 | 说明 | 格式 |
|------|------|------|------|
| `/v1/models` | GET | 获取可用模型列表 | OpenAI |
| `/v1/chat/completions` | POST | 对话补全 | OpenAI |
| `/v1/messages` | POST | Claude 消息 API | Claude |
| `/v1/messages/count_tokens` | POST | 统计 token 数量 | Claude |
| `/v1/healthz` | GET | 健康检查 | - |

---

## 架构

```
Client (OpenAI/Claude SDK / Claude Code / Cursor)
  ↓ API Key 认证
Cloudflare Worker
  ↓ Durable Object (run/session 管理)
  ↓ Credential (凭证轮换/冷却)
Codebuff Upstream API
```

---

## 本地开发

```bash
cd cfworker
npm install
npm run dev
# 访问 http://localhost:8787
```

---

## 环境变量/Secrets

| Secret | 说明 |
|--------|------|
| `APP_ENCRYPTION_KEY` | AES-GCM 加密密钥（32+ 字符 hex，已禁用） |

---

## 许可证

MIT

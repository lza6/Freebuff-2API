# Freebuff2API — Cloudflare Worker SaaS

将 [Codebuff](https://codebuff.com) 免费 AI 模型代理为兼容 OpenAI / Claude 格式的 API 服务。

## 功能

- **OpenAI 兼容** — `POST /v1/chat/completions`、`GET /v1/models`
- **Claude 兼容** — `POST /v1/messages`、`POST /v1/messages/count_tokens`
- **流式支持** — SSE streaming，自动在 OpenAI ↔ Claude 格式间转换
- **用户认证** — 注册/登录、Session Cookie、API Key 双认证
- **凭证管理** — 多 Freebuff token 轮换、401 自动冷却、AES-GCM 加密存储
- **Dashboard** — 可视化凭证管理、API Key 生成、使用统计
- **Durable Objects** — 管理 run/session 生命周期，避免重复启动
- **Cron 清理** — 自动刷新模型列表、清理过期会话

## 部署步骤

### 1. 安装依赖

```bash
cd cfworker
npm install
```

### 2. 配置 Wrangler

`wrangler.toml` 已预配置。如需修改 Worker 名称、KV/D1 ID，编辑该文件。

### 3. 设置加密密钥

用于 AES-GCM 加密存储用户 Freebuff token：

```bash
# 生成随机密钥（至少 32 字符）
node -e "console.log(require('crypto').randomBytes(32).toString('hex'))"

# 设置为 Worker secret
npm run secret:set
# 粘贴上面生成的密钥
```

### 4. 运行数据库迁移

```bash
# 本地开发
npm run db:migrate

# 远程生产
npm run db:migrate:remote
```

### 5. 部署

```bash
npm run deploy
```

### 6. 本地开发

```bash
npm run dev
# 访问 http://localhost:8787
```

## 使用

### Dashboard

访问 Worker URL 根路径，注册账户后：

1. 在 **Freebuff 凭证** 页面添加你的 Freebuff auth token
2. 在 **API Keys** 页面生成一个 API Key
3. 使用下方的 API 端点

### API 调用

**OpenAI 兼容：**

```bash
curl https://your-worker.workers.dev/v1/chat/completions \
  -H "Authorization: Bearer f2db-your-key" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "google/gemini-2.5-flash-lite",
    "messages": [{"role": "user", "content": "Hello"}]
  }'
```

**Claude 兼容：**

```bash
curl https://your-worker.workers.dev/v1/messages \
  -H "x-api-key: f2db-your-key" \
  -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "google/gemini-2.5-flash-lite",
    "messages": [{"role": "user", "content": "Hello"}],
    "max_tokens": 1024
  }'
```

### 获取 Freebuff Token

- 方法 1：访问 [freebuff.llm.pm](https://freebuff.llm.pm) 获取 authToken
- 方法 2：安装 Freebuff CLI，从 `~/.config/manicode/credentials.json` 获取 `authToken`

## 架构

```
Client (OpenAI/Claude SDK)
  ↓ API Key / Session Cookie
Cloudflare Worker
  ↓ Durable Object (run/session 管理)
  ↓ Credential (AES-GCM 解密)
Codebuff Upstream API
```

## 环境变量/Secrets

| Secret | 说明 |
|--------|------|
| `APP_ENCRYPTION_KEY` | AES-GCM 加密密钥（32+ 字符 hex） |

## 许可证

MIT

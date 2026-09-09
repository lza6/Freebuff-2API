# 素材计划 — Freebuff2API 宣传片

## 素材总表

| # | Shot | 素材 | 来源 | 规格 | 说明 | 状态 |
|---|------|------|------|------|------|------|
| 1 | Hook | 终端打字四行 | Pack A（代码实现） | 1080×1920 | 等宽字 + 绿/白/蓝/灰 | ✅ |
| 2 | Problem | 红 ✕ + 痛点文案 | Pack A | 1080×1920 | 纯 CSS/文字 | ✅ |
| 3 | Product | 终端启动卡片 + 12 行日志 | Pack A（对齐真实启动输出） | 1080×1920 | 日志形态取自真实二进制输出 | ✅ |
| 4 | 双协议 | 协议 Tab + 客户端卡 | Pack A | 1080×1920 | LobeChat/Claude Code/Codex/Cursor | ✅ |
| 5 | Run 树 | 树形结构 + chip | Pack A | 1080×1920 | session→root→sub-agent | ✅ |
| 6 | 多 Token | token 卡 + 进度条 | Pack A | 1080×1920 | token-A/B/C | ✅ |
| 7 | Web 控制台 | 浏览器窗口 + 统计卡 | Pack A（视觉 token 取自真实面板） | 1080×1920 | GitHub Dark 主题 | ✅ |
| 8 | CTA | ★ + 标题 + 按钮 | Pack A | 1080×1920 | 仓库地址真实 | ✅ |

## Pack B — 真实素材（本地网关采集）

| 文件 | 说明 | 状态 |
|------|------|------|
| `pack-b/screens/ui-full.png` | 本地 `/ui` 面板真实截图（1080×1920 Chrome headless） | ✅ |
| `pack-b/screens/healthz.json` | `/healthz` 真实响应（账号健康快照） | ✅ |
| `pack-b/screens/v1-models.json` | `/v1/models` 真实响应（20 个模型） | ✅ |
| `pack-b/screens/api-account-balance.json` | `/api/account/balance` 真实响应（freebucks/套餐/地区） | ✅ |
| `pack-b/screens/api-usage-totals.json` | `/api/usage/totals` 真实响应 | ✅ |
| `pack-b/screens/terminal-boot.txt` | 真实二进制启动日志（ANSI） | ✅ |
| `pack-b/screens/chat-sse.txt` | mock 上游还原的 SSE 流事件（freebuff.start/text/reasoning/done） | ✅ |
| `pack-b/pack-b-sources.md` | 来源记录 | ✅ |

## 质量自检

### P0
- [x] 无素材 404（渲染日志 0 缺失）
- [x] 比例统一（全部 1080×1920）
- [x] 无真实敏感数据（终端数值/账号名为演示占位）
- [x] 风格一致（全部 GitHub Dark 视觉 token）

### P1
- [x] 每镜头有微动画（逐行/stagger/弹性）
- [x] 留白充足（Apple 风字重 200-300）
- [x] 素材来源可追溯（pack-b-sources.md）

### 待验证
- [ ] 真实 chat 流式（本地无 token → 401，已验证行为；SSE 形态 mock 还原）
- [ ] BGM（需 Mureka API key 或用户提供）

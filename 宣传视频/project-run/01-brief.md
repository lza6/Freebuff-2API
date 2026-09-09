# 创意简报 — Freebuff2API 宣传视频

> 面向抖音开源项目推广 · 60s · 竖屏 1080×1920 · 12:9 → 适配抖音 9:16
> 目标渠道：抖音（竖屏短视频），兼顾 GitHub 开源项目调性

---

## 产品信息

- **名称**：Freebuff2API
- **一句话**：把 Codebuff Freebuff 免费额度逆向成一个 OpenAI / Claude 兼容 API 网关，跑一个二进制就能在任意 AI 客户端免费调用 Gemini 模型
- **解决的问题**：
  1. Freebuff 官方只提供自家 CLI，无法在 OpenAI/Claude 客户端中使用
  2. 免费层模型选择被上游收窄，普通人不知道怎么组合才能用
  3. 多账号 token 各自为战，无法统一轮换、并发、保活
- **核心卖点**：
  1. **双协议出口** — OpenAI `/v1/chat/completions` + Claude `/v1/messages`，LobeChat / NextChat / Claude Code / Codex / Cursor 全兼容
  2. **零成本调用** — 用 Gemini 免费额度当 API 用，无需付费 key
  3. **自动化 run 层级** — 会话→根 run→子 run 树自动维护，用户零感知
  4. **多 Token 轮换** — 周期轮换 + 并发租约 + 401 自动冷却，吞吐拉满
  5. **Rust 单二进制** — 高性能、无依赖、内置 Web 控制台 + 用量统计
- **目标受众**：AI 玩家 / 开发者 / 开源爱好者 / 学生党（想白嫖免费模型但又想用顺手客户端的群体）
- **差异化**：同类项目多数只能转发单一协议或需要自己维护 run 树；Freebuff2API 把最麻烦的上游协议层完全封装，开箱即用

---

## 视觉方案

- **推荐风格**：**风格 3 赛博科技（CLI/Hacker）为主 × Apple 发布会级留白为辅**
  - 理由：产品本身是终端网关 + 逆向工程，天然适合等宽字体、终端绿、代码流、扫描线
  - 但目标人群在抖音，需要「大字号冲击 + 丝滑运镜」抓 3 秒停留，所以叠加 Apple 风的大留白排版
- **时长**：60s（抖音完播率甜区）
- **画幅**：1080×1920 竖屏（9:16）
- **镜头数**：8 个 Shot
- **主视觉 tokens**（从产品自身提取，不另造皮肤）：
  - 背景：#0d1117（GitHub Dark，取自内置 Web 面板 `--bg`）
  - 卡片：#161b22，边框 #30363d
  - 文字：#e6edf3 / #8b949e
  - 强调：#2f81f7（GitHub 蓝，面板 `--accent`）+ 终端绿 #3fb950（面板 `--ok`）
  - 错误红 #f85149（面板 `--err`）
  - 字体：JetBrains Mono（代码/数字）+ Inter（正文标题），100-400 字重

---

## 叙事结构

| 段落 | 时长 | 目的 | 关键信息 |
|------|------|------|---------|
| Hook | 0-4s | 抓注意力 | 终端打字 + 巨大数字「Free Model API」 |
| Problem | 4-11s | 建立共鸣 | 「AI 客户端要 key？收费？」一行命令戳破 |
| Product | 11-20s | 产品登场 | 一行 `./freebuff2api` 跑起来，双协议亮出 |
| Features | 20-48s | 展示能力 | 4 个功能镜头：双协议 / run 树自动 / 多 token 轮换 / Web 面板 |
| CTA | 48-60s | 行动号召 | GitHub 仓库地址 + Star 引导 |

---

## 素材需求（Pack A + Pack B）

| # | Shot | 素材 | 来源 |
|---|------|------|------|
| 1 | Hook | 终端打字动效 | 代码实现（Pack A） |
| 2 | Problem | 收费墙概念 / 终端快速演示 | 代码实现（Pack A） |
| 3 | Product | **真实终端启动演示** | Pack B：本地跑 release 二进制 + Chrome 捕获 |
| 4 | 双协议 | **真实 `curl` 流式响应** | Pack B：本地起网关 + 真实调用 |
| 5 | run 树 | 概念示意 + 真实日志 | 代码实现（Pack A）+ 真实日志 |
| 6 | 多 token | 概念示意 | 代码实现（Pack A） |
| 7 | Web 面板 | **真实面板截图** | Pack B：Chrome 访问 localhost:8787/ui |
| 8 | CTA | 仓库地址 + Star | 代码实现（Pack A） |

---

## 风险与待确认

- [x] **仓库地址**：`github.com/lza6/Freebuff-2API`（已确认）
- [ ] **config.json 无 token**：本地演示跑 `freebuff2api` 无需 token 也能启动并服务 `/v1/models` / `/ui` / `/healthz`，但真实 `chat` 调用需要 token → 用**模拟 SSE 流**演示协议效果（画面真实、无真实消费），标注待验证
- [ ] **抖音 BGM**：无授权 BGM 源 → 用程序合成的节奏音轨（music-plan 里给生成方案）
- [ ] **画面上出现的 token 等敏感信息**：全部用占位符，绝不真实展示

---

## 定位确认

- [x] 完整成片 MP4（1080×1920 竖屏）
- [x] 仓库地址 github.com/lza6/Freebuff-2API
- [x] 赛博科技 × Apple 留白混合风格
- [ ] 叙事结构是否认可？（Hook→Problem→Product→Features→CTA）

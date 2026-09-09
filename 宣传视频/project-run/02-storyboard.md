# 分镜脚本 — Freebuff2API 宣传片

**规格**: 1080×1920 竖屏 · 30fps · 60s · 风格：赛博科技 × Apple 留白

---

## Shot 1: Hook — 终端打字 (0-4s)

| 维度 | 描述 |
|------|------|
| 类型 | 数据冲击 / 氛围 |
| 时长 | 4 秒 |
| 目标 | 用终端打字感瞬间抓住「这是开源 / Hacker」身份认同 |
| 画面 | 深色渐变底（#0d1117→#010409）。左上起终端绿 `> ./freebuff2api`，随后白字 `Free Model API`、蓝字 `zero-cost gateway`、灰字 `ready.` 依次浮现，字重 200，留白充足 |
| 文字 | `> ./freebuff2api`(绿) / `Free Model API`(白, 200) / `zero-cost gateway`(蓝) / `ready.`(灰) |
| 动效 | 每行 opacity+y16 逐行 stagger .22s，power2.out；3.55s 整镜淡出 |
| 素材 | 代码实现（Pack A） |

---

## Shot 2: Problem — 付费墙 (4-11s)

| 维度 | 描述 |
|------|------|
| 类型 | 痛点展示 |
| 时长 | 7 秒 |
| 目标 | 建立共鸣：现在用 AI 客户端都要付费 key / API 账单贵 |
| 画面 | 纯黑底。红色 ✕（220px, 字重 100）elastic 弹入居中。下方白字「AI 客户端都要付费 key？」，灰字「API 账单越用越贵。」，最后金句 `There's a better way.` 淡入 |
| 文字 | `✕`(红 #f85149) / `AI 客户端都要付费 key？`(白) / `API 账单越用越贵。`(灰) / `There's a better way.`(白, 200) |
| 动效 | ✕ scale(0→1) elastic.out(1,.5) @4.4s；两行痛点 y20→0 依次；金句 fade @9s；10.4s 整镜淡出 |
| 素材 | 代码实现（Pack A） |

---

## Shot 3: Product — 终端启动 (11-20s)

| 维度 | 描述 |
|------|------|
| 类型 | 产品登场 |
| 时长 | 9 秒 |
| 目标 | 产品是什么 + 双协议核心价值 |
| 画面 | 居中一个深色终端卡片（圆角 14px, 边框 #30363d）。12 行启动日志逐行打出：命令 → 版本 → 监听 8787 → 账号数 2 → 20 模型 → HTTP 就绪 → UI 地址 → 双协议路径。下方白字大标题 `One binary. Two protocols.`，灰字 `OpenAI + Claude 兼容` |
| 文字 | `[OK] 监听 127.0.0.1:8787` 等（绿色 [OK]） / `One binary. Two protocols.`(52px, 200) / `OpenAI + Claude 兼容` |
| 动效 | 卡片 opacity+scale .96→1 @11.4s；日志逐行 fade stagger .28s；标题 y20→0 @17s |
| 素材 | 代码实现（Pack A），日志形态对齐真实启动输出 |

---

## Shot 4: Feature 1 — 双协议出口 (20-28s)

| 维度 | 描述 |
|------|------|
| 类型 | 功能演示 |
| 时长 | 8 秒 |
| 目标 | 展示核心卖点：一个网关同时输出 OpenAI + Claude 协议 |
| 画面 | 渐变底。标题 `双协议出口`。两个协议 Tab 卡（/v1/chat/completions 蓝色高亮、/v1/messages 灰）先后浮入。4 个客户端 logo 卡（LobeChat / Claude Code / Codex / Cursor）依次浮入。底部小字「任意 OpenAI / Claude SDK 即插即用」 |
| 文字 | `双协议出口` / `/v1/chat/completions` / `/v1/messages` / 4 客户端名 / 副标 |
| 动效 | 标题 y-20；Tab y24 先后；logo 卡 stagger .15s；副标 fade；27.4s 淡出 |
| 素材 | 代码实现（Pack A） |

---

## Shot 5: Feature 2 — Run 层级自动维护 (28-35s)

| 维度 | 描述 |
|------|------|
| 类型 | 功能演示 / 架构 |
| 时长 | 7 秒 |
| 目标 | 展示「最麻烦的上游 run 树被自动封装」 |
| 画面 | 纯黑底。标题 `Run 层级自动维护`。等宽树形三行逐行展开：`session` → `└── root run base2-free`(绿) → `└── sub-agent run code-reviewer-*`(蓝)。底部绿色 chip `零感知 · 自动保活 · 惰性创建` back.out 弹出 |
| 文字 | 标题 / 树三行 / chip |
| 动效 | 标题 y-20；树行 x-14 stagger .4s；chip scale .9→1 back.out(1.6)；34.4s 淡出 |
| 素材 | 代码实现（Pack A） |

---

## Shot 6: Feature 3 — 多 Token 轮换 (35-43s)

| 维度 | 描述 |
|------|------|
| 类型 | 功能演示 |
| 时长 | 8 秒 |
| 目标 | 展示吞吐能力：多 token 并发轮换 |
| 画面 | 渐变底。标题 `多 Token 轮换`。3 个 token 卡（token-A/B/C）浮入。进度条（16px 高）从 0% 拉到 100%（5s）。底部小字「并发租约 · 401 自动冷却 · 周期轮换」 |
| 文字 | 标题 / token-A/B/C / 副标 |
| 动效 | 卡 stagger .2s；进度条 width 0→100% 5s power1.inOut；副标 fade；42.4s 淡出 |
| 素材 | 代码实现（Pack A） |

---

## Shot 7: Feature 4 — 内置 Web 控制台 (43-52s)

| 维度 | 描述 |
|------|------|
| 类型 | 功能演示 |
| 时长 | 9 秒 |
| 目标 | 展示运维可视化：账号健康 / 用量 / 请求 |
| 画面 | 浏览器窗口卡片（mac 风格红黄绿按钮 + 地址栏 `127.0.0.1:8787/ui`）。内部 GitHub Dark 统计卡片：总请求 2,048、总 Token 1.2M、账号健康度 token-A/B ok / token-C wait、近 24h 用量 +38%（进度条）。全部依次浮入 |
| 文字 | 各卡片标题/数值/账号徽标 |
| 动效 | 标题 y-20；统计卡 stagger .25s；用量条 y16；51.4s 淡出 |
| 素材 | 代码实现（Pack A），视觉 token 取自真实面板（#0d1117/#2f81f7/#3fb950） |

---

## Shot 8: CTA — GitHub 仓库 (52-60s)

| 维度 | 描述 |
|------|------|
| 类型 | 行动号召 |
| 时长 | 8 秒 |
| 目标 | 引导去 GitHub 点星 |
| 画面 | 径向渐变底（#1b2735→#010409）。金色五角星（130px）弹性弹出。`Freebuff2API` 大标题 → 蓝色等宽 `github.com/lza6/Freebuff-2API` → 灰副标 `Rust 单二进制 · 免费模型当 API 用` → 绿色圆角按钮 `去 GitHub 点亮 ⭐`（随后两次呼吸缩放） |
| 文字 | ★(金) / `Freebuff2API` / 仓库地址(蓝) / 副标(灰) / 按钮(绿) |
| 动效 | ★ scale+rotate elastic；标题/仓库/副标/按钮递进；按钮 scale 1→1.06→1 呼吸 |
| 素材 | 代码实现（Pack A） |

---

## 审核结论
- 8 镜头完整覆盖 Hook→Problem→Product→4×Feature→CTA
- 视觉 tokens 全部取自产品真实面板（GitHub Dark）
- 每镜头有动画、留白充足、转场交替
- 无真实敏感数据（终端数值/账号名全为演示占位）

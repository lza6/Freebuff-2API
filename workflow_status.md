# Workflow Status — Freebuff2API

> 单一状态源（长期任务恢复 / 节点协作 / 最终验收）。只记录事实与证据。
> Phase G（v0.5.0）、Phase H+I（v0.6.0）已发布；当前跟踪 **Phase J（v0.7.0，内嵌浏览器一键登录）**。

## Task Contract — Phase J（2026-09-11 启动）

- **用户核心诉求（原文）**：「一键登录只能用扩展这种东西吗？不能直接用内联浏览器去抓取？扩展的话用户不经常用啊」
- **目标**：浏览器版用户不装扩展也能一键登录——网关内置内嵌浏览器窗口（WebView2），登录完成后自动抓取 Cookie（含 HttpOnly）入库。
- **授权**：全部。

## 技术决策（已评估）

| 方案 | 结论 |
|------|------|
| **WebView2 内嵌（选定）** | 本机已装 WebView2 Runtime 151.x（`C:/Program Files (x86)/Microsoft/EdgeWebView/`）；`webview2-com`/`wry` 可用；能拿到 HttpOnly Cookie（ICoreWebView2CookieManager 可读 HttpOnly，与 Electron `session.cookies` 同级能力）；Rust 原生、零 Node 依赖 |
| Chrome/Edge CDP（remote-debugging） | 需用户重启浏览器带 `--remote-debugging-port`，体验差且新版 Chrome 限制 `--user-data-dir` 默认目录，放弃 |
| 扩展（v0.5.0 已交付） | 保留为备选路径（Linux/macOS 用户 WebView2 不可用） |
| Electron 桌面版 | 已有同款实现（desktop/main.js:219-293），行为基准：登录窗口 → did-navigate 检测 /chat|/account|/web → 抓 cookie → POST /api/tokens/import |

## Phase J Task Graph

| ID | 目标 | 交付物 | 状态 |
|----|------|--------|------|
| J1 | WebView2 登录窗口模块（同二进制 `--login-window` 子进程模式，避免阻塞网关 tokio 运行时） | `src/login_window.rs`（216 单测全绿） | ✅ DONE |
| J2 | 登录完成检测（Cookie 含 session-token 即成功，600ms 轮询 + 10 分钟超时） | 同上 | ✅ DONE |
| J3 | 自动入库（POST /api/tokens/import，多端口探测，连接失败换端口/明确拒绝即停） | 同上 | ✅ DONE |
| J4 | 登录路径优先级统一：方案 A=内嵌窗口（openEmbedLogin → /api/login/embed）> 扩展直连 > 方案 B=剪贴板 > 手动向导 | `src/web.rs` + `src/api.rs` | ✅ DONE |
| J5 | 降级链：WebView2 初始化失败 → 明确报错提示装 Runtime 或用扩展/手动向导 | map_err 文案 | ✅ DONE |
| J6 | **真实 E2E（已完成两轮真实窗口验证）**：① 窗口真实弹出 + WebView2 真实渲染 + 用户关窗 → stderr 正确输出"窗口已关闭但未完成登录"（report(1) 真实执行）；② 60s 未登录 → timeout 124（超时语义正确）；③ 全程网关不受影响（healthz 200）、凭证文件未被误改。**完整成功路径（人工登录 GitHub → 自动入库）需用户配合一次** | 证据（本文件） | 部分 DONE |
| J7 | 独立审查（Critic-J）+ 修复 | 报告 | ✅ DONE（CONDITIONAL PASS：1×P1 Linux 构建破坏→平台门控修复；3×P2 防重入/降级可见性/UI 卡顿→全修；P3 注释修正） |
| J8 | CHANGELOG/README + 发布 v0.7.0 | release | ✅ DONE（commit d09397a，Release: github.com/lza6/Freebuff-2API/releases/tag/v0.7.0） |

## J6 真实 E2E 证据（2026-09-11）

| 场景 | 结果 |
|------|------|
| `--login-window` 启动 | 真实窗口弹出，WebView2 Runtime 151.x 真实渲染 freebuff.com |
| 用户关闭窗口（未登录） | stderr 输出"窗口已关闭但未完成登录（可重试，或改用扩展/手动导入）"，report(1) 真实执行 |
| 60s 超时（未登录） | timeout 124，符合超时语义 |
| 网关共存 | 登录窗口全程网关 healthz 200，tokens.json 未被误改 |
| 完整成功路径（登录 GitHub → 自动入库） | **待用户配合一次人工登录**（无浏览器自动化环境；机制与 Electron 桌面版逐行对齐） |

## 明确不做 / Backlog（沿用）

- ~~桥接错误检测点前移~~ → ✅ v0.7.2 已完成（commit bb9f7b4）
- SSE 攒批提交（NewAPI-Gateway 模式）→ **评估后保留 backlog**：改动面 ~200 行触碰核心转发路径，上游 200 内嵌错误在桥接侧已由 v0.7.2 bypass 覆盖、桌面侧已能如实记录（Phase E），收益/风险比不支持本轮实施
- web 版 agent-runs/stream、web 版广告链、视频上传
- 密码学随机 Key、桌面版多账号轮询增强

## Phase H/I 存档（v0.6.0 已发布，commit 1435a8f）

- models 鉴权 / config 原子写 / OsRng Key（192 位熵）/ 桥接去误报 / E2E finally 恢复
- Phase I 全功能真实 E2E 18/18（保活/模型列表/工具调用/多轮记忆/长 agent/Anthropic 流式）
- 验证基线：213 单测全绿 · clippy 0 警告 · phase_g 46/46 + phase_i 18/18

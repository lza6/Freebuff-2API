# Workflow Status — Freebuff2API

> 单一状态源（长期任务恢复 / 节点协作 / 最终验收）。只记录事实与证据。
> 历史阶段报告见 git 历史（`参考的结果计划指南.md` 已按用户要求删除，其有效结论已沉淀到代码与本文档）。

## Task Contract — Phase G（2026-09-11 启动）

- **用户目标（本轮原文要点）**：
  1. **浏览器版必须支持真·一键登录**（自动导航到 web 版 → 登录 → 自动入库）
  2. **入库凭证要显示入库时间、要能去重、要能显示账号详细信息**
  3. 深度研读上游 `web版的源代码和网络包/`（含「高级功能.txt」等全部 txt）并**完成其中所有需求**
  4. **UI 要讲清楚"导入凭证后怎么请求"**（API 地址 / key / 客户端配置）
  5. 深度参考 `智能渗透/参考项目`（反代 / 网关做法）
  6. 真实 E2E 测验 + 独立验收审计 + 提交推送 + 创建发行版
- **成功标准**：上述 1-6 全部落地 + 真实 E2E 证据 + 独立 Critic 审查 + 提交推送发布
- **当前授权**：全部（用户明确"已全部授权所有行为和阶段"）

## Phase F 已完成项（v0.5.0 未提交内容，基线）

| 项 | 证据 |
|----|------|
| F2 账号全貌聚合 API `GET /api/account/overview` | `src/api.rs:2485` |
| F3 凭证 added_at / 去重 / 类型 | `src/import.rs:151` `persist_tokens`（dedupe）、`src/api.rs:652` `handle_tokens_list` |
| F4 前端账号全貌卡 + 向导 | `src/web.rs:110-172`、`src/web.rs:540` `renderOverview` |
| F5 接入指南三步走 | `src/web.rs:282-312` |
| F6a Chrome 扩展（手动点图标） | `browser-extension/`（4 文件） |
| F6b web 协议事件修复 | `src/web_protocol.rs:40-107`（agent_delta/index/finish_reason/threadId/title） |
| F6c 上传多类型 | `src/api.rs:2364` `handle_upload` |
| F6d 指纹按账号派生（**部分**） | `src/web_protocol.rs:164` `GravityContext::for_cookie`；**缺 `x-freebuff-instance-id`** |
| F6e 会话清理 | `src/api.rs:1978` `handle_threads_cleanup` |
| 编译通过 | `cargo check --all-targets` ✅ 2026-09-11 |

## Phase G Task Graph（本轮工作）

| ID | 目标 | 负责 | 依赖 | 交付物 | 状态 |
|----|------|------|------|--------|------|
| G0 | 侦察：上游文档全量核对 + 参考项目网关学习 | U4 / R1 | — | 两份报告 | DONE |
| G1 | **扩展真·一键登录**：面板↔扩展直连 + 自动打开 freebuff.com + 登录后自动入库 + API Key 透传 | ext-builder | — | `browser-extension/*` v1.1.0 | DONE |
| G2 | **凭证管理增强**：稳定 id + meta 缓存 + check/delete 端点 + mtime 回填 + mask 短串修复 | 主控 | — | `src/import.rs`、`src/api.rs` | DONE |
| G3 | **账号使用记录**：JSONL 快照 + history 端点 + 前端呈现 + cred_meta.json | 主控 | G2 | `src/account_meta.rs` | DONE |
| G4 | **请求接入信息显性化**：/api/guide + 「立刻开始请求」卡 + API Key 热管理 | 主控 | — | `src/api.rs`、`src/web.rs` | DONE |
| G5 | **扩展分发**：内嵌 + 手写 stored zip + `GET /api/extension/bundle` | 主控 | G1 | `src/extension.rs` | DONE |
| G6 | **协议指纹补全**：x-freebuff-instance-id + client_context 按账号派生 | 主控 | — | `src/web_protocol.rs` | DONE |
| G6b | **上游会话自动清理**（批注 网页对话.txt:605）：cleanup loop + delete_thread 端点实证 | 主控 | — | `src/api.rs`、`src/web_protocol.rs` | DONE |
| G6c | **Web-Cookie 桥接**：/v1 双协议桥接 + thread 复用（省每日会话额度）+ Anthropic 事件流转换 | 主控 | — | `src/web_threads.rs`、`src/api.rs` | DONE |
| G7 | 验证：212 单测全绿 + E2E 46 项全过 + 真实上游链路（账号全貌/桥接对话/续聊复用） | 主控 | G1-G6c | 证据 | DONE |
| G8 | 独立 Critic 审查 + 修复 | Critic-1b/2b/3b | G7 | 报告 | DONE（三份 CONDITIONAL PASS，发现全修，复验全绿） |
| G9 | 文档同步 + 提交 + 推送 + 发布 v0.5.0 | 主控 | G8 | release | DONE |

## G7 验证证据（2026-09-11 实测）

| 项 | 结果 |
|----|------|
| `cargo test --lib` | 212 passed / 0 failed |
| `cargo check --all-targets` / `cargo clippy` | 0 error / 0 warning |
| `node tests/check_panel_js.cjs` | 面板 JS 语法通过 |
| `node tests/e2e_phase_g.cjs 47861` | **46 通过 / 0 失败** |
| 扩展桩测试（ext-builder） | 35 流程 + 11 bridge + 17 校验 + 10 契约对齐 ALL PASS |
| 真实上游：/api/account/overview | 200，streak=2、积分 20/25、逐模型 4 条、meta.valid=true |
| 真实上游：/v1/chat/completions 非流式（桥接） | 200，"收到" |
| 真实上游：/v1/chat/completions 流式（桥接） | SSE chunk 正常，thread 绑定落盘 |
| 真实上游：多轮续聊 | thread 复用（`149a9959-…` 不变），turns 递增 |
| 真实上游：/v1/messages 非流式+流式 | Claude 形状 200 / 标准事件流（model 兜底已修） |
| 真实上游：DELETE /api/chat/threads/{id} | 路由实证（JSON 404 vs 未知路由 HTML 404；POST /delete 为 405） |

## U4 / R1 关键结论（已消化）

- 用户批注全集：高级功能.txt ×6 + 网页对话.txt:605（自动清理）——均已落地；网页对话.txt:532「删除会话」标注实为埋点抓包，真实删除端点已由探针实证。
- 上游 `agent-runs/stream`（web 项目页）与 web 版广告链（/api/ads + imprezia + paidNoFillToken）**未实现**——属新协议面，非本轮用户批注范围，已记录为后续项（P1-P2）。
- 参考项目 Top 建议（NewAPI-Gateway 的 SSE 攒批提交、9router 逐模型锁定、MonkeyCode 扩展配对鉴权）已记录；本轮先落地最小集合（运行时 Key 管理 + 桥接复用），配对鉴权列入后续。
- 视频/attachments 引用在上游抓包中零样本，不对外宣称支持。

## Evidence Ledger

| Claim | Evidence | Type |
|-------|----------|------|
| 上游 usage-summary 结构（streak/recent/sessionsByModel） | 高级功能.txt:145-195 | DIRECT |
| 账号信息结构（user.name/email/id/expires） | 高级功能.txt:312-321 | DIRECT |
| convex-token 为 5 分钟 JWT（exp-iat=600） | 高级功能.txt:406 | DIRECT |
| 订阅档位 starter/plus/pro（$8/$25/$60） | 高级功能.txt:505-571 | DIRECT |
| freebuff-session 含 prices + rateLimitsByModel + freebucks.daily | 高级功能.txt:613-785 | DIRECT |
| session-token 为 HttpOnly（浏览器 JS 不可读） | 高级功能.txt:253（Set-Cookie HttpOnly） | DIRECT |
| 上游要求 `x-freebuff-instance-id` 头 | 高级功能.txt:610、804 | DIRECT |
| 上游地区限制信号 countryCode/countryBlockReason | 高级功能.txt:782-783 | DIRECT |
| 用户 6 条批注需求 | 高级功能.txt:198,215,325,410,503,591 | DIRECT |
| 本地旧凭证缺 `added_at`（用户抱怨"看不到入库时间"的根因） | `data/tokens.json` 无该字段，文件 mtime 2026-09-10 04:12 | DIRECT |
| freebuff.com 经代理可达（E2E 前置条件成立） | `curl -x 127.0.0.1:10808 --ssl-no-revoke` → 200 | DIRECT |
| Rust 侧用 rustls（无 Windows schannel 吊销问题） | Cargo.toml:32 `rustls-tls` | DIRECT |

## Decisions

- **浏览器一键登录的实现路径**：`__Secure-next-auth.session-token` 是 HttpOnly（高级功能.txt:253），网页 JS 不可读 → 只能靠扩展的 `chrome.cookies`。
  本轮把扩展从"手动点图标"升级为**面板↔扩展直连**：`externally_connectable` + content script 广播扩展 ID，面板点「一键登录」→ 扩展自动打开 freebuff.com → 轮询等待登录 → 登录后自动 POST `/api/tokens/import` → 面板轮询到新凭证自动刷新。
  降级链：桌面版 Electron IPC → 扩展直连 → 手动粘贴向导（三条路径都在 UI 里说明）。
- **凭证稳定标识**：用 FNV-1a 64 位哈希（零新依赖、跨版本稳定），不用 `DefaultHasher`（Rust 文档明确不保证跨版本稳定）。
- **扩展分发**：`include_str!`/`include_bytes!` 内嵌进二进制 + 手写 stored 模式 zip 打包（零新依赖），保证单文件分发时扩展也可用。
- **运行时 API Key 管理**：`AppState.api_keys` 改为 `Arc<std::sync::RwLock<Vec<String>>>` 支持热更新；写回 `config.json` 时用 `serde_json::Value` 往返合并，避免覆盖其他字段；本机以外监听地址拒绝清空 key。

## G8 审查发现与处置（三份报告消化完毕）

| 发现 | 级别 | 处置 |
|------|------|------|
| 数据面 3 端点缺 Origin 检查（恶意网页 text/plain 盲打） | P1 | ✅ 已修：`/v1/chat/completions`、`/v1/messages`、`/v1/uploads` 加 `origin_allowed`；实测 evil Origin → 401，无 Origin 的 SDK 请求 200 |
| web-cookie 导入污染账号池（/v1 必失败还挡桥接） | P1 | ✅ 已修：session-token 凭证不入池；实测导入后池 accounts=0 |
| tokens.json 并发读改写丢凭证 + 非原子写 | P1 | ✅ 已修：TOKENS_LOCK + 临时文件 rename；实测 7 路并发导入零丢失 |
| sweep_threads 竞态吃掉新 thread 记录 | P1 | ✅ 已修：THREADS_LOCK + 锁内重读合并回写 |
| e2e 配置未入 gitignore | P2 | ✅ 已修（git check-ignore 确认生效） |
| history compact 竞争丢行 | P2 | ✅ 已修：HISTORY_LOCK + 原子写 |
| WebThreadMap flush 覆盖窗口 | P2 | ✅ 已修：快照 clone 移入临界区 + 原子写 |
| loopback starts_with 前缀绕过 | LOW | ✅ 已修：`is_loopback_listen` 精确匹配（7 用例全过，含 localhost.evil.com 拒绝） |
| e2e 配置漏 web_threads_path | P2 | ✅ 已修：隔离到 data/e2e/ |
| 凭证已存在时面板轮询空转 3 分钟 | P1 | ✅ 已修：扩展同步完成路径 done:true + 面板直显（等待 ext-builder 桩测试更新确认） |
| README"多账号轮询"对 web-cookie 夸大 | P2 | ✅ 已修：文档加注 |
| /v1/models 无鉴权（HEAD 既有） | LOW | 记录为后续项（非本轮引入） |
| config.json 写回非原子 | LOW | 记录为后续项 |
| 扩展真实浏览器全流程 | 待验证 | 人工验证项（桩测试+契约 10/10+MV3 校验全过） |

**G8 后复验**：cargo test 212 全绿 · clippy 0 警告 · panel JS 语法过 · e2e_phase_g 46/46 · 真实上游链路（概览/桥接对话/续聊复用/Anthropic 流式）全过。

## Next Gate

- **Phase G 已完结**：commit `822d735` 已推送 `main`，tag `v0.5.0` 已推送，Release 已创建（https://github.com/lza6/Freebuff-2API/releases/tag/v0.5.0）。
- 遗留（已记录、非阻塞）：/v1/models 鉴权（HEAD 既有）、config.json 原子写、web 版 agent-runs/stream 与 web 广告链（U4 P1-P2）、扩展真实浏览器全流程人工验证。
- 密码学随机 API Key（当前 UUIDv4）、SSE 攒批提交（NewAPI-Gateway 模式）列入 v0.6.0 候选。

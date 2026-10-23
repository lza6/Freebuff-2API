# Changelog

本项目遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [0.10.5] - 2026-10-23

### 修复

- **内嵌登录窗口（WebView2）不吃系统代理**：`--login-window` 子进程现通过 `GATEWAY_PROXY` 继承网关 `config.http_proxy`，
  并以 `WebViewBuilder::with_additional_browser_args("--proxy-server=…")` 注入。此前在没有全局代理继承的环境下，
  窗口内点 Google 登录会直接 `ERR_CONNECTION_CLOSED`（实测 `accounts.google.com` / `accounts.youtube.com` 直连不可达，
  而 `github.com` 可达），登录向导形同不可用。`spawn_login_window` 一并透传代理，非 Windows stub 签名同步。

### 新增

- **`docs/UPSTREAM_PROTOCOL_2026-10.md`**：上游协议变更完整逆向记录（基线 Desktop 0.0.188 / CLI 0.2.34）。包含：
  - 两条通道失效的逐层证据（`409 chat_moved` → `410 freebuff_update_required` → `403 free_mode_cli_required`）
  - `session/admission` 完整契约、`agent-runs` 生命周期、`chat/completions` 请求规格
  - 版本门禁关键头 `x-freebuff-sse-gzip-recovery: 1`
  - agentId 按模型映射（`base3-free-<model>`）、CLI 终端指纹头 `x-freebuff-env` 格式
  - 计费模型从 freebucks 改为小时；0 成本模型清单（调试用）
  - workspace v1 端点群与 `404 Workspace endpoint not found` 的区分用法
  - 复现命令与离线解包方法（`installer.exe → app-64.7z → orchestrator.js`）

### 变更

- **README 增加状态说明**：明确对话转发当前不可用（上游政策封堵 + 明确封号警告），
  并列出仍然可用的能力（面板/凭证/健康看板/用量/doctor/导入导出/技能/记忆）。

## [0.10.4] - 2026-10-23

### 修复

- **测试时间炸弹**：`telemetry::tests::insights_worst_hours_ranking` 硬编码 `2026-09-19` 时间戳，而 `insights()` 仅统计最近 24h（`window_hours=24`）→ 窗口滑过后用例必然失败（`worst_hours` 空数组，断言 `left: 0, right: 2`），自 2026-09-20 起 `cargo test`/CI 必红。改为相对时间（`Utc::now() - 1h` 与 `Utc::now()`）并由时间戳截取 `hour_utc` 期望值，行为语义不变

## [0.10.3] - 2026-09-19

### 测试/质量

- **覆盖率提升闭环**：新增 22 个 Router 级集成用例（config 读写白名单/非法值、skills list+gate 注入检测、memory CRUD+toggle、tokens import+list、accounts health、usage totals/daily/models/cost/insights、logs recent、doctor、export schema、import 坏 schema、prompts list+toggle、threads cleanup、guide、models data+meta、web-chat 无凭证降级、upload 无凭证 400、usage accounts、account history、跨站写拒绝）
  - router_test 11 -> **33 用例**；api.rs 行覆盖 30.6% -> **45.8%**；TOTAL 行覆盖 66.8% -> **72.25%**
- **CI 覆盖率门禁收紧 65% -> 70%**（`--fail-under-lines 70` + `continue-on-error: false`；本机实测 72.25% 留 2.25pt 缓冲）
- docs/TESTING.md 基线更新；提升路径（api.rs -> 60 再总体 75/80）为 backlog

## [0.10.2] - 2026-09-19

### 新增

- Dockerfile 容器健康检查：运行阶段安装 `curl` + `HEALTHCHECK`（`/healthz`，interval 30s / start_period 10s）
- CI 覆盖率**真门禁**：`--fail-under-lines 65` + `continue-on-error: false`（替代原 always-true；本机基线 66.8%，关键模块 ≥80%）
- `docs/DOCKER.md` 容器实跑指南（本机无 Docker 时由 CI docker.yml 多架构构建 → ghcr.io/lza6/freebuff2api）

### 工程

- 覆盖率基线记录于 `docs/TESTING.md`（TOTAL 66.8%；提升路径 70→75→80 为 backlog）

## [0.10.1] - 2026-09-19

### 修复（代码审计闭环：CRITICAL 0 / HIGH 1 / MEDIUM 2 / LOW 6 / NIT 5）

- **[HIGH] 生产接线**：`main.rs` 启动路径加载 vendored 上游模型快照 → `refresh_strategy_from_snapshot`（时间窗/efforts/fallback 策略真正生效；失败静默降级 warn，不阻断启动）
- **[MEDIUM] availableAt 归因**：仅"不可用由时间窗导致"时给出恢复时刻；静态暂停模型不编造 availableAt
- **[MEDIUM] 兜底模型校验**：`resolve_at` 兜底 DEFAULT_MODEL 也做可用性校验，全不可用才原样返回默认（让上游给出可读错误）
- **[LOW] SQLite 鲁棒性**：telemetry `open_db` 设 busy_timeout(5s)；`/api/usage/insights` 改 `spawn_blocking` 不占 tokio worker
- **[LOW] 未知策略不过度拒绝**：`availability_now` 对未识别策略按可用处理（避免上游新增策略静默禁用模型），文案与"已暂停"区分
- **[LOW] XSS 面**：推荐表 `price`/`usable_today` 补 `esc()`（上游字段半可信）
- **[LOW] ARIA 完整性**：11 个面板补 `role=tabpanel`+`aria-labelledby`；tabs 实现 roving tabindex（激活项 0 / 其余 -1）
- **[LOW] CI E2E 可靠性**：健康轮询超时显式 `::error::`+exit 1；`trap` 兜底清理网关进程
- **[NIT]** 快照输出确定性排序；空策略覆盖跳过；上传 filename 日志净化（去 CR/LF）；冷却到期文案"已到期"

### 验证

- 275 单测 + core 8 + router 11 + web_pool 5 + model_meta 7 全绿；clippy `-D warnings` 零警告；fmt 通过；check_panel_js 通过
- 真实 E2E（v0_8 26 + v0_9 26）在 CI e2e-win job 全绿；Release v0.10.1 由 CI 自动构建

## [0.10.0] - 2026-09-19

### 新增

- **上游模型策略合同实时化**（`src/models.rs` / `src/router.rs`，对照上游 freebuff-models.ts 快照）：
  - `ModelMeta` 新增 `availability`（always/deployment_hours/off_peak_only）+ `available_at`（off_peak_only 窗口内给出 ISO 恢复时刻，与上游 freebuffModelUnavailableAt 对齐，不编造时间）
  - DeepSeek 高价窗 00:00–10:00 UTC（北京周末豁免）；`refresh_strategy_from_snapshot` 快照同步（幂等、失败降级静态底座）
  - `resolve/resolve_available/unavailable_reason` 时间感知，降级链跳过暂停/高峰模型
  - 目录对齐：收录 `mimo/mimo-v2.5`（上游免费无限、FALLBACK 落点）；按上游快照修正 5 处漂移（deepseek-v4-flash premium、kimi premium、ox-alpha premium/multimodal、fable multimodal、glm-5.2 multimodal）
  - 新增 `tests/fixtures/freebuff-models.snapshot.json`（21 行）+ `scripts/check_model_contract.mjs` + `scripts/extract_upstream_models.mjs` 漂移检测
- **面板可访问性与体验**（`src/web.rs`）：ARIA tabs（role=tablist/aria-selected/键盘 ←/→/Home/End）、日志 aria-live、focus-visible、44px 触控目标；日志页级别筛选按钮组/暂停滚动/导出 JSON/错误计数徽标；推荐卡可用性列（暂停/高峰 + availableAt + 未经策略验证）；凭证冷却警告条（一键去账号页）；上传白名单/20MB/空文件校验 + 发送中/上传中按钮态
- **遥测"三最"聚合**：`src/telemetry.rs::insights()` + `/api/usage/insights`（最慢账号 Top3/最常用模型 Top5/错误率最高时段 Top3）
- **web 凭证池全冷却结构化降级**：`web_pool_exhausted`（503 + code=web_pool_exhausted + 最短恢复秒）；`cooldown_until` 输出 ISO + 新增 `cooldown_seconds`
- **请求热路径 panic 收窄**：api.rs 7 处运行时 unwrap → unwrap_or_default
- **CI/工程**：新增 `e2e-win` job（真实网关 E2E + 面板 JS 检查）、`cargo test --doc` 门禁（rust-docs 组件）、覆盖率纪律注释；`docs/TESTING.md`、`scripts/verify_release.ps1`、`scripts/check_artifacts.ps1`（只报告不删）

### 测试

- 单测 253 → **275**（models 时间窗/同步器/路由时间感知 + telemetry insights + router 3 项时间路由）
- model_meta_test 4 → **7**（mimo 目录/meta 全盖/fixture 零漂移）；router_test 11、web_pool_test 5、core_test 8 保持
- `cargo clippy --all-targets -- -D warnings` 零警告；`cargo fmt --check` 通过

### 文档

- `docs/API_GUIDE.md` 补 /api/usage/insights、/api/accounts/health、/api/export、/api/import、/api/login/embed/result、/v1/models meta
- `README_zh.md` 模型矩阵补 mimo/glm-5.2；v0.10 特性说明；`计划书/0-项目全景与版本路线图.md` 刷新至 v0.10
- `workflow_status.md` 开 Phase M

## [0.9.0] - 2026-09-18

### 新增

- **web Cookie 凭证池化 + 多账号轮询**（`src/web_pool.rs`，修复 README 长期承认的"桥接路径只用第一个有效凭证"）：
  - 复用 Bearer 池熔断语义（Closed/Open/HalfOpen、指数冷却封顶 10 分钟、HalfOpen 探测闸门）
  - 全部 web Cookie 路径（chat/messages 桥接、余额、详情、上传、会话清理）改从池内按健康分/熔断/冷却挑选
  - 401/403 确定性失效立即冷却，网络/5xx 连续失败累计熔断；成功 mark_ok 逐步恢复
  - 导入/删除凭证后热刷新（`reload` 保留既有健康状态）；账号列表与健康看板展示 web 凭证
- **上游模型元数据契约**（`src/models.rs` / `src/router.rs`，对照上游 freebuff-models.ts 当前快照）：
  - 新增 `ModelMeta` 静态权威表：premium / multimodal / available / efforts 阶梯 / fallback
  - 已暂停/下架模型标记不可用并给回落：gemini-3.8-flash、deepseek-v4-pro、minimax-m3、muse-spark-1.3、ox-alpha、glm-5.2
  - GLM 5.3 阶梯对齐当前上游 `['low','high','max']`（max 原样保留）；solar/minimax/kimi/glm-5.2 无阶梯自动剥离
  - `/v1/models` 响应带 `meta`（字段稳定，`data` 保持兼容）；`router::resolve_available/unavailable_reason` 供路由与面板消费
- **面板（v0.9）**：
  - 对话测试台升级：多轮会话上下文、system 提示词、reasoning_effort 下拉（按模型阶梯联动）、图片上传（拖拽/粘贴/选择 → /v1/uploads，失败自动降级 base64）、复制回复 / 导出 Markdown / 新会话
  - 凭证健康看板：Bearer + web Cookie 合并展示（熔断徽章/评分/失败次数/冷却 + 每账号历史时间线）
  - 总览"今日推荐"卡片：按 rateLimitsByModel 剩余排序（已暂停模型自动靠后）
  - 设置页"数据迁移"：一键导出/导入（导入前二次确认 + 自动备份）
  - 请求详情加耗时时间线（首字节 / 总耗时）
- **鉴权纵深**：`inject_peer` 中间件把真实 TCP 对端（ConnectInfo）写入 `x-fb-peer`，`is_loopback_request` 改为"无代理头 && 对端回环"才算本机（127.0.0.1 默认行为不变）；`/api/doctor` 新增 `listen_scope` 检查（监听非回环且未配 api_keys → fault + 修复建议）
- **全配置导出/导入**（`src/export.rs` + `/api/export` + `/api/import`）：schema 版本校验、大小上限 5MB、写前自动备份 `data/backup-<ts>/`、安全最小集（绝不覆盖 api_keys/auth_tokens）

### 测试

- 新增 `src/web_pool.rs` 7 项单测（多号优选/冷却跳过/半开恢复/连续失败熔断/脱敏快照/空池）
- 新增 `src/models.rs` 4 项单测（元数据全量覆盖/单模型查询/暂停清单/meta_snapshot 字段）
- 新增 `tests/model_meta_test.rs` 4 项（阶梯对齐/可用性回落/可读原因/未知模型默认可用）
- 新增 `tests/web_pool_test.rs` 5 项；`tests/router_test.rs` 增至 11 项（含新端点）
- 新增 `tests/e2e_phase_v0_9.cjs` **26 断言真实网关全绿**；`tests/e2e_phase_v0_8.cjs` 26 断言回归全绿
- 真实浏览器（headless Chrome）渲染面板：JS 完整执行（model-count 占位符 → 20、模型 chips 渲染、全部 v0.9 控件在 DOM）
- **规模：253 单测 + 8 core + 4 model_meta + 11 router + 5 web_pool 全绿**；`cargo clippy --all-targets -- -D warnings` 零警告；`cargo fmt --check` 通过

### 修复

- `tests/e2e_phase_v0_9.cjs` 契约对齐：健康端点返回合并 accounts（含 kind + 每条 history 时间线）
- 文档：README 测试计数 236→253；旧 v0.1.0 验收报告移入 `docs/archive/`（顶部标注历史归档）
- CI：`cargo fmt --check` + llvm-cov 覆盖率门禁（首次 continue-on-error 收集基线）

## [0.8.0] - 2026-09-15

### 新增

- **双桶并发信号量落地**（README 宣称的能力补实，`src/semaphore.rs`）：
  - tokio `Semaphore` 零新依赖；免费 `{槽:1, 并发:3}`、订阅 `{槽:3, 并发:8}`（可配：`concurrency_free_slots/free_multi/sub_slots/sub_multi`，环境变量 `CONCURRENCY_*`）
  - 接入 `/v1/chat/completions`、`/v1/messages`、web 桥接三路径，**首字节写出前 acquire**；`TierGuard` RAII 自动归还（流式任务结束才释放）
  - 2s 超时返回 429（`concurrency_busy`），不无限排队；订阅判定保守（账号池任一凭证含套餐特征走订阅桶）
  - 注：每请求同时占"槽"与"并发"各一，实际并发上限 = 槽位容量（免费 1 / 订阅 3），"并发"桶为上游策略预留维度
- **Claude 路径补齐重试 + 记账 + 记忆**（`/v1/messages`，此前三缺）：
  - 请求级重试循环（与 OpenAI 同策略）：失败换号、5xx/限流/网络自动重试、熔断冷却
  - waiting_room 排队返回 503 + `overloaded_error` 可读消息（不再裸 502）
  - 非流式成功补 `usage_db.record_ex` + `telemetry.record`；流式补 usage 落库；成功路径补 `memory.observe`
- **面板现代化**（`src/web.rs`）：
  - 新增 **对话测试台**（调 `/v1/chat/completions` 流式渲染回复）、**设置页**（监听地址/记忆开关/token_saver/脱敏/技能模式/预算/代理/清理间隔/信号量容量 UI 化写回 config.json）、**关于页**（版本/运行时长/上游/免责声明）
  - 新增 `GET /api/config` + `POST /api/config/save`：白名单校验 + 类型/合法值检查 + 原子写回；`memory_enabled` 热生效
  - 视觉品牌化：CSS 分层 token（色板/间距/圆角/阴影/动效）、`prefers-reduced-motion` 尊重、`focus-visible` 焦点环、窄屏导航横向滚动
  - 大日志 **windowed 虚拟滚动**（只渲染可视区 + 缓冲，>1000 条流畅）
- **安全加固**：
  - 全部响应加 `X-Content-Type-Options: nosniff`、`Referrer-Policy: strict-origin-when-cross-origin`；面板页加 CSP `default-src 'self'`
  - **日志/遥测脱敏**（`src/redact.rs`，`redact_logs` 默认开）：Cookie 值 / Bearer / authorization / sk- 长串在写入日志总线与遥测前替换为 `***`
  - 跨站 Origin 拦截改为 **403 Forbidden**（CSRF，语义区别于 401）
- **桌面壳加固**：
  - 多开保护：`app.requestSingleInstanceLock()`，二次启动激活已有窗口，不重复拉起网关
  - 托盘「系统体检」改用 hash 跳转（`location.hash='#doctor'`），不再全页刷新
- **健壮性**：
  - 端口绑定失败给出明确中文错误（含占用进程排查提示 `netstat -ano | findstr :port`）
  - `web_threads` 绑定表加容量上限（2000）与 TTL 清理（24h），防文件/内存膨胀
  - Cookie 判定收窄：`handle_account_balance` 不再用 `%3A` 兜底（防 URL 编码串误判）

### 测试

- 新增 `src/semaphore.rs` 6 项单测（桶独立/超时/RAII 无泄漏/订阅判定）
- 新增 `src/redact.rs` 6 项单测（Cookie/Bearer/sk- 脱敏、普通文本不误伤）
- 新增 `tests/router_test.rs` **10 项 Router 级集成测试**（Mock TCP 上游）：chat 非流式/流式、messages 非流式、排队 503、401、跨站 403、healthz、桥接触发、5xx 重试耗尽、401 凭证失效
- 补充 api 单测（Cookie 判定收窄回归、Claude tool 往返语义）、web_threads 单测（TTL/容量）
- **测试规模：238 单测 + 8 集成 + 10 路由级集成全绿**；`cargo clippy -- -D warnings` 零警告
- 新增 `tests/e2e_phase_v0_8.cjs`（26 断言真实网关冒烟：安全头/CSP/配置读写/新 tab/windowed 渲染/脱敏）+ headless Edge 真实浏览器面板验证

### 修复

- `desktop/main.js` 托盘「系统体检」跳转失效（`loadURL('#doctor')` 不触发 hash 路由 → 改 `executeJavaScript` 设 hash）
- README 版本失真（`Setup 0.3.0` → 当前版本；信号量表述与实现对齐）

## [0.7.3] - 2026-09-11

### 修复

- **web 流不再被 300s 总超时截断**（`ERR_INCOMPLETE_CHUNKED_ENCODING`）：`WebClient` 由 reqwest `.timeout(300s)`（总请求超时，流式增量还在吐也会被整点掐断）改为 `read_timeout(300s)`（单次读块超时，与上游客户端同款）——只要增量还在就一直收，仅完全静默 5 分钟才断。
- **桥接 token 记账不再永远为 0**：上游 web 协议 SSE 不返回 usage（done 事件为空），改为按内容长度估算输入/输出 token（输入=发出内容，输出=转换后 chunk 正文+推理；每 2 字符≈1 token，偏保守不虚报）。面板用量、趋势、请求详情恢复正常展示。

### 变更

- **记忆层独立总开关（默认关闭）**：
  - `config.rs` 默认 `memory_enabled: false`（用户批注：记忆不是每个人都需要的）
  - 新增 `POST /api/memory/toggle`：热切换关/开，关闭后既不自动记录也不注入 system；写回 config.json 立即生效，无需重启
  - `GET /api/memory` 回传 `enabled` 状态；面板「记忆」页顶部新增 switch 开关
  - 新增 `MEMORY_ENABLED` 环境变量覆盖

## [0.7.0] - 2026-09-11

### 新增

- **内嵌浏览器一键登录（方案 A · 零安装零复制）**：浏览器版用户不再必须装扩展——
  - 面板「一键登录」→ `POST /api/login/embed` → 网关派生 `--login-window` 子进程（tao 窗口 + wry WebView2，独立进程不阻塞网关 tokio 运行时）
  - 窗口里正常完成 GitHub 登录 → 子进程通过 WebView2 CookieManager 抓取**全部 Cookie（含 HttpOnly 的 session-token，OS 级组件不受网页 JS 限制）** → 自动 POST `/api/tokens/import` 入库 → 窗口自关
  - 多端口探测（面板端口 > 47821/47822/8787，连接失败换端口、明确拒绝即停）；600ms 轮询登录态 + 10 分钟超时；结果通过进程退出码协议汇报（0 成功 / 1 失败）
  - WebView2 不可用时明确报错并引导降级（扩展 / 剪贴板 / 手动）
- **面板登录向导重组**（四条路径按体验排序）：
  - **方案 A · 内嵌窗口**（最推荐 · 零安装零复制）
  - **方案 B · 剪贴板自动检测**（推荐 · 无需安装 · 约 30 秒）：复制 Cookie 回来点一下按钮，自动填框导入并**即贴即验**（自动检查新凭证有效性、显示账号名/邮箱；空剪贴板/权限拒绝/无 clipboard API 三种降级均引导手动粘贴）
  - **方案 C · Chrome/Edge 扩展**（全自动 · 装一次以后都不用管 · 首次约 2 分钟）
  - 手动粘贴兜底（Cookie 串 / cURL / HAR，约 1 分钟）
  - 向导降级高亮描边动画；HttpOnly 原理说明随方案引用同步更新

### Phase I 存档（随 v0.6.0 已交付）

- 全功能真实 E2E 矩阵 `tests/e2e_phase_i.cjs`（18 断言：保活/模型列表/工具调用/多轮记忆/长 agent/Anthropic 流式）

## [0.6.0] - 2026-09-11

### 收尾（Phase G 审查遗留项全清）

- **`/v1/models` 鉴权补齐**：配置 `api_keys` 时模型清单不再对未授权方公开（未配置时保持本机直连语义）。实测：无 Key 401 / 对 Key 200 / 错 Key 401 / 清除后恢复直连。
- **config.json 原子写**：面板生成/清除 API Key 的写回改为临时文件 + rename，写入中途崩溃不再损坏配置。
- **密码学随机 API Key**：一键生成从 UUIDv4（122 位、格式可识别）升级为 OsRng 全熵随机（`sk-fb-` + 32 字符 base64url 字符集，192 位有效熵）。
- **桥接错误判定去误报**：web 桥接流的成功判定不再因响应正文出现 "Unauthorized" 字样而误标失败（旧逻辑会把模型正文提及该词的成功请求误记 502）；错误分类函数 `detect_bridge_error` 按既有错误规则表实现（当前转换管线中上游 error envelope 不进入判定文本，检测点前移列入后续项）。
- **E2E 基础设施**：`tests/e2e_phase_g.cjs` 增加 finally 强制恢复（脚本崩溃也不残留测试 Key）；新增 `tests/e2e_phase_i.cjs` 全功能真实 E2E 矩阵（token 保活 / 模型列表 / 工具调用 / 多轮上下文记忆 / 长 agent / Anthropic 流式，18 断言）。

### Phase G（v0.5.0）——见 [0.5.0] 段；其全部审查发现（含 4 个 P1、4 个 P2、3 个 LOW）已在此前提交修复并实测。

## [0.5.0] - 2026-09-11

### 新增

- **Web-Cookie 桥接（关键链路补全）**：只导入 web Cookie（一键登录路径）、账号池为空时，`/v1/chat/completions` 与 `/v1/messages` **自动桥接到上游 web 协议**，浏览器用户"照指南填 /v1"即可直接对话（此前会报 `no healthy upstream auth token available`）。
  - **上游会话复用**：续聊轮次只发最后一条用户消息并复用同一 thread（`data/web_threads.json` 绑定），避免每请求新开 thread 烧光每日会话准入（免费 6 次/天）——这是"用一会儿就 429"的直接原因
  - Anthropic 流式实时转换为标准事件流（message_start → content_block_delta → message_stop）；非流式做完整消息转换
  - 绑定的 thread 被上游清理时自动重置，客户端重试即恢复
  - 全链路遥测（路由原因标注 continue/new thread、usage、threadId 进自动清理清单）

- **浏览器一键登录扩展**（`browser-extension/`，Chrome/Edge MV3）：读取 freebuff.com 登录凭证（含 **HttpOnly** Cookie，网页 JS 无法读取）并发送到本机网关；含选项页（自定义端口 / 可选 API Key）与安全说明（仅读取 freebuff.com、仅发送本机）。
- **浏览器「真·一键登录」闭环**（面板 ↔ 扩展直连）：
  - 扩展通过 `externally_connectable` + content script 广播自身 id，面板拿到 id 后即可直接指挥扩展
  - 面板点「一键登录」→ 扩展**自动打开 freebuff.com** → 轮询等待登录（最长 3 分钟）→ 登录成功**自动把凭证写回网关** → 面板轮询到新凭证自动刷新账号信息
  - 未安装扩展时自动降级为 3 步手动向导（三条路径都在 UI 中说明）；扩展与面板的请求会带上网关 API Key（面板透传），`Origin: chrome-extension://` 已纳入 CSRF 白名单
- **扩展一键分发**：`GET /api/extension/bundle` 把扩展（编译期内嵌，单文件分发同样可用）打包为 zip 下载，面板「⬇ 下载扩展」直接可用。
- **凭证管理增强**：
  - 每条凭证稳定 `id`（FNV-1a 64，跨版本可重现）
  - 凭证列表直接显示**账号昵称 / 邮箱 / 类型 / 套餐 / 今日剩余积分 / 入库时间**，并可展开详情（逐模型额度、连续天数、近 7 天 token、地区限制、错误原因）
  - **旧数据入库时间回填**：早于该字段引入的凭证用 `tokens.json` 的 mtime 回填并落盘，不再显示"—"
  - `POST /api/tokens/check` 单条凭证检查；`POST /api/tokens/delete` 删除凭证（同时移出运行中的账号池）
  - 同值自动去重（重复导入明确提示）
- **账号使用记录**（用户批注："每个账号当然你也要有记录查询"）：每次检查/刷新写入一条 JSONL 快照，`GET /api/account/history?cred_id=&limit=` 按账号查询，面板「使用记录」页可视化（套餐/剩余/已用/token/连续天数/成败）。
- **「立刻开始请求」接入卡 + 运行时 API Key 管理**：总览页首屏直接给出 Base URL、OpenAI/Anthropic 两个地址与 API Key，全部可一键复制；`GET /api/guide` 提供真实监听地址、Key 状态与模型数；面板可**一键生成/清除 API Key**，**热生效无需重启**（同时写回 `config.json`，非本机监听时禁止清空）。
- **协议指纹补全**：按账号派生 `x-freebuff-instance-id`（上游网页版每个请求都带，此前网关完全不发，是最容易被风控识别的差异之一）；gravity `client_context` 的 screen/viewport/DPR/内存/核数也改为按账号派生，不再所有账号共用同一套环境。
- **上游会话自动清理**（用户批注："我们要做到自动清理，防止反代给上游制造压力"）：每小时自动清理超过 24 小时的旧会话（`thread_cleanup_interval_sec` / `thread_max_age_hours` 可调，间隔 0 关闭）；手动端点 `POST /api/threads/cleanup` 保留预演模式；「原理」页新增说明。
- **账号全貌面板**（「账号」页，全中文呈现）：
  - 身份：昵称 / 邮箱 / 头像 / 用户 ID / 凭证有效期
  - 使用统计：连续使用天数（streak）/ 累计活跃天数 / 近 7 天消息数与 token 消耗（输入/输出/缓存/合计）/ 各模型会话数
  - 今日额度：账号层级 / 订阅套餐 / 积分剩余与上限 / 重置时间（太平洋时间午夜，隔天自动刷新）/ **逐模型今日剩余次数、限额、已用、积分价、下次重置**
- **凭证保活检查**（`POST /api/account/refresh`）：调上游 convex-token 验证 Cookie 是否仍有效，失效时给出重新登录提示。
- **账号全貌 API**（`GET /api/account/overview`）：并发聚合上游 4 个端点（auth/session、usage-summary、subscriptions、freebuff-session），任一失败降级不整体失败。
- **接入指南强化**：三步走总览、API Key 说明（含实际是否启用校验）、Node.js SDK 与 curl 示例，全部用真实地址与真实 Key 填充。

### 修复（浏览器一键登录 / 凭证判定）

- **web-cookie 凭证不再污染账号池**：导入的 session-token 此前会被塞进桌面版 Bearer 账号池，/v1 请求被路由到必然失败的桌面协议（熔断后报"no healthy token"），还会阻止 web 桥接触发——现在 web Cookie 只由桥接路径使用，导入后 /v1 立即可用。
- **数据面 CSRF 防线补齐**：`/v1/chat/completions`、`/v1/messages`、`/v1/uploads` 此前不校验 Origin，恶意网页可用 `text/plain` 简单请求盲打（借用户 Cookie 消耗上游额度/触发风控）——现已与 `handle_web_chat` 一致拦截跨站 Origin（SDK/curl 不带 Origin 不受影响）。
- **tokens.json 并发安全**：导入/删除/回填改为进程内互斥 + 临时文件原子替换——并发导入不再互相覆盖丢凭证（实测 7 路并发零丢失），写入中途崩溃不再损坏全部凭证。
- **threads.json 并发安全**：会话清理的回写改为锁内重读合并——sweep 跨网络删除期间新产生的 thread 记录不再被旧快照覆盖（该会话此前会永不清理）。
- **history/绑定文件原子化**：使用记录压缩与追加串行化；web 会话绑定快照在锁内 clone 后原子写盘，消除旧快照覆盖窗口。
- **loopback 判定精确化**：`localhost.evil.com:47821` 这类前缀伪装地址不再被当作本机（改为 host 精确匹配 + IP 解析判定）。
- **E2E 配置隔离**：`tests/e2e_phase_g.config.json` 加入 .gitignore（测试生成的 Key 永不进 git）。
- **无效凭证被误判为「有效」**：上游 `/api/auth/session` 对未登录/失效凭证返回的是 **HTTP 200 + `{}`**（不是 401），此前只判断"请求是否成功"，导致任何伪造 Cookie 都显示"凭证有效"（实测确认）。改为必须检查响应里真的有 user 主体（id/email/name），额度端点作为辅助信号。
- **`/healthz` 信息泄露**：配置 `api_keys` 后，未授权请求不再返回账号名与模型构成，只回存活时长与版本。
- **扩展请求被 CSRF 防护拒绝**：`Origin: chrome-extension://` 此前不在白名单，扩展导入会被拒——现已放行（配置了 api_keys 时扩展仍需携带 Key）。
- **短凭证脱敏泄漏**：`mask()` 对 ≤8 字符的凭证会把"末尾 4 位"回显出来（如 `sk-local` → 泄漏 5/8 字符）。改为短串只露 2 位、≤4 字符全遮（借鉴参考项目 freellmapi 的 maskKey 修复记录）。
- **删除会话端点实证**：`DELETE /api/chat/threads/{id}` 经真实凭证探针确认存在（不存在 thread 返回 JSON 404，未知路由返回 HTML）；移除无效的 `POST /api/chat/threads/delete` 回退（实测 405）。
- **面板鉴权失败体验**：网关启用 API Key 后首次打开面板不再"红灯 + toast 每 6 秒狂闪"，改为一次性引导横幅并暂停自动刷新，填 Key 后自动恢复；凭证已存在时的一键登录不再空转轮询 3 分钟报"超时"（扩展同步完成路径直接回显结果）。

### 修复（web 协议对齐上游抓包）

- **`agent_delta` 正文不再丢弃**：上游工具（web_search/read_url）产出的研究结果正文在 agent_delta 事件中，此前被静默丢弃导致用户只看到工具调用不见结果。
- **并行工具调用 `index` 递增**：此前恒为 0，并行 5 个工具时客户端互相覆盖。
- **流结束发送 `finish_reason` chunk**（stop / tool_calls），严格客户端不再判为异常结束。
- **threadId 透出**（meta/title 事件）：多轮续聊可用（`WebClient::last_thread_id()`）。
- **title 二次更新采用后到覆盖**（保留模型生成的摘要而非用户原文）。
- **上传支持任意文件类型**：此前 mime 白名单只放行 image/pdf，文档上传被改写为 image/png，上游返回不了 `kind:"document"`——文档链路名存实亡。
- **上传响应透出完整字段**：`kind`（image/document）/ `url`（图片）/ `chars`、`truncated`（文档）/ `descriptionStorageId`，并附用途说明。
- **`attachments` 解析**：`/v1/web/chat` 支持文档附件引用（此前恒为空数组）。

## [0.4.0] - 2026-09-11

### 新增

- **记忆层（AI 更懂用户）**：`data/memory.sqlite` 独立库；**零 LLM 规则 observe**（自动记录常用模型偏好、推理档位降级、用户纠正信号"记住…/别再…/always/never"）；trigram FTS5 中文检索；有界注入（512 token 预算、低权威标记、marker 转义、按 id 排序保字节稳定）；面板「记忆」页可查看/新增/删除/置为稳定事实。
- **熔断三态**：账号池从"裸冷却时间戳"升级为 Closed/Open/HalfOpen 熔断器（连续失败 4 次断开，冷却随次数指数增长封顶 10 分钟，半开探测连续成功 2 次恢复）；`mark_success`/`mark_failure` 全程接线。
- **请求级重试循环**：上游失败自动换号重试（最多 3 次，含 429/5xx/网络错误；401/403 冷却该账号后换号）；严格 committed 边界——只在尚未向客户端写出任何字节前重试。
- **错误规则表**：文本优先 + 状态码兜底的上游错误分类（waiting_room/rate_limit/model_unavailable/auth_expired 等 8 类），带可重试判定与 Retry-After 提示。
- **MCP 最小暴露**：`POST /mcp`（JSON-RPC 2.0，手写零新依赖）提供 3 个只读工具：`list_models` / `list_accounts` / `usage_summary`，供外部 agent（Claude Code/Cursor）直接查询网关状态。
- **成本/速率可视化**：`GET /api/usage/cost`（30 分钟滑窗请求数/错误率/平均延迟/速率）；面板总览页显示速率行（诚实标注"免费层无货币成本"）。
- **教学页（原理速览）**：面板新增「原理」Tab，6 节讲清网关工作原理（请求链路/多账号轮询/注入机制/黑匣子/广告保活/数据位置）。
- **配置正式生效**：此前解析但零消费的 `fallback_models`（降级链）、`token_saver`（tool_result 压缩）已接线；新增 `memory_path` 配置。

### 修复

- **`compress_tool_result` 多字节 panic**：按字符边界切分（中文 tool_result 不再 panic；与 v0.3.0 修复的 tail 截断同类问题）。
- **skills 库路径**：避开 `with_extension` 截断（目录名含 `.` 时路径错误）。
- **`/v1/uploads` 错误体截断**：上游错误消息限 300 字符（防回显账号/内部细节）。

### 工程

- 新增模块：`memory.rs`（记忆层）、`mcp.rs`（MCP 只读服务）、`errors.rs`（错误规则表）、`pool.rs` 熔断器。
- 测试：108 → **130+ 单元测试**（新增熔断器 4 / 记忆 9 / MCP 10 / 错误表 12 / 压缩多字节回归 1），clippy 零警告。

## [0.3.0] - 2026-09-11

### 新增

- **技能系统（持久化）**：面板「技能」页可新建/编辑/启停/删除技能；文件（`data/skills/<id>/SKILL.md`）为真相源 + SQLite 索引；**roster 模式**按需注入（只注入名称与描述，预算 2000 token，可配置）；内置质量门（注入短语、超长、格式检查）；重启持久化。
- **黑匣子日志（可观测）**：
  - `GET /api/logs/stream`（SSE 实时日志）+ `GET /api/logs/recent`（历史回放），面板「实时日志」页；
  - 请求详情抽屉：`GET /api/usage/requests/{id}` 返回路由/账号/延迟/首字节/tokens/错误 + 人话解释（如"上游免费队列排队中，不是网关故障"）；
  - `GET /api/doctor` 系统体检（四态：ok / fault / unknown / fact），面板「系统体检」页。
- **真实用量统计**：流式/非流式 token 真实采集（此前恒为 0）；Claude 路径 `/v1/messages` 用量记录（此前完全缺失）；上游错误分类落库（`error_kind`）。
- **Claude 流式协议转换**：`/v1/messages` 流式请求不再透传 OpenAI SSE，按 canonical event 转换为 Anthropic 事件流（`message_start` / `content_block_start|delta|stop` / `message_delta` / `message_stop`），工具调用块完整支持。
- **多模态上传**：`POST /v1/uploads`（裸 body + `x-file-name` 头换取 storageId）；`/v1/web/chat` 支持 `images` 参数（storageId 数组或对象数组）。
- **面板重写**：Tab 导航（总览 / 账号 / 技能 / 实时日志 / 系统体检 / 接入指南）；账号页支持粘贴 Cookie/cURL/HAR 导入（此前无导入入口）；接入指南内置 Claude Code / Cursor / OpenAI SDK / LobeChat 配置片段一键复制。
- **配置项**：`telemetry_path`、`skills_dir`、`skills_inject_mode`、`max_roster_tokens`（均有默认值，旧配置兼容）。

### 修复

- **README_zh.md** 从 Go 旧版重写为 Rust 版（端口 47821 / `--config` / cargo 命令 / 客户端接入指南 / FAQ）——此前中文用户第一步即被带错。
- **面板 4 个旧 bug**：`{model_count}` 占位符字面量；两个按钮 `location.href` 把用户带离面板进 JSON 裸页；每次刷新 DOM 无限堆积；空态渲染出字符串 "undefined"。
- **桌面端**：启动/登录失败弹窗（此前仅 console.error）；托盘新增「系统体检 / 打开日志 / 打开配置 / 打开数据目录」；网关 stdout/stderr 落盘 `userData/logs/gateway.log`；自动更新闭环（自动下载 + 下载完成提示安装）；非默认端口检测。
- **启动健壮性**：模型注册表网络同步增加超时保护（connect 5s / total 10s）——此前网络异常时阻塞启动 30 秒以上。

### 工程

- 新增模块：`protocol/`（流式转换）、`skills/`（技能持久化）、`retry.rs`（失败分类/退避/committed 语义）、`logbus.rs`（日志广播+环形缓冲）、`telemetry.rs`（独立写线程遥测库）。
- 测试：单元测试 24 → **105**，集成测试 8；`cargo clippy --all-targets -- -D warnings` 零警告。
- 新增 E2E 验收脚本 `tests/e2e_phase_d.cjs`（29 项断言：面板元素 / 技能 CRUD / SSE / 体检 / 鉴权 / 上传错误码 / 模型列表）。
- 桌面壳新增 `preload.js`（contextBridge 白名单 IPC）。

### 已知限制（下一批次）

- 请求级「换号重试」仅落地失败分类 + 冷却接线，完整重试循环待接入。
- 遥测 events 表目前仅在失败路径写入（成功路径事件链为增强项）。
- web 协议（`/v1/web/chat`）上游不返回 usage 字段，token 记为 0（延迟/字节/首字节正常记录）。
- `data/tokens.json` 路径仍为硬编码（其他路径均已可配置）。

## [0.2.0] - 2026-09-10

- CI 修复（Docker 多架构 / GHCR 命名 / rust 1.95 锁定 / .cargo 代理移出 git）。
- 安全与工程质量加固：管理端点鉴权、API key 脱敏、跨域 token 导入拒绝、熔断接线、Claude 非流式协议转换。
- 默认端口 8787 → 47821；面板内置提示词/技能管理；上游错误透传；流式长连接无整体超时。

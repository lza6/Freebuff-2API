# Changelog

本项目遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

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

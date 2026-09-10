# Changelog

本项目遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

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

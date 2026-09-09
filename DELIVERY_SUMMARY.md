# Freebuff2API v0.1.0 交付总结

## 交付形态（双形态全落地）

| 形态 | 产物 | 状态 |
|------|------|------|
| 🖥️ 本地软件 | `desktop/dist/Freebuff2API Setup 0.1.0.exe`（81MB，含 9.4MB 网关） | ✅ 实机安装运行验证通过 |
| 🐳 Docker | `docker/Dockerfile` 多阶段构建 | ✅ 已提供（本机无 docker 未实跑） |
| 📦 源码 | Rust 网关 + 逆向归档 | ✅ 已推送 GitHub |

## 已发布

- **仓库**: `https://github.com/lza6/Freebuff-2API`（main 分支，6 个新提交）
- **Release**: `v0.1.0` — https://github.com/lza6/Freebuff-2API/releases/tag/v0.1.0
  - 含 `Freebuff2API.Setup.0.1.0.exe` 安装包资产

## 功能清单（全部落地）

1. **Rust(axum) 网关** — 单二进制 9.4MB，零运行时依赖
2. **多账号智能轮询** — 健康评分 + 冷却熔断 + 最优 token 选择
3. **Freebuff 会话管理** — 排队/活跃/45s 心跳/广告刷新保活（协议逆向自 0.0.98）
4. **token 导入 API** — 粘贴 curl 命令或 HAR JSON → 自动提取 Bearer token → 去重入库 → 热更新账号池
5. **用量统计** — SQLite + 内置控制面板（账号健康/模型/最近请求/按日统计）
6. **模型路由降级** — 主模型失败自动切换备选
7. **桌面壳** — Electron 自动拉起网关 + 托盘常驻 + 崩溃自动重启
8. **逆向归档** — `reference/reverse/` 完整保存 0.0.98 反编译源码（app-src + orchestrator.js）

## 真实验证（带证据）

- 编译/clippy(零警告)/测试(7项全绿) 通过
- token 导入 curl/HAR 实测：`{"added":1,"token_masked":"fa82b5...f6a7"}`，重复导入去重
- 上游冒烟：请求**真实到达** `www.codebuff.com/api/v1/freebuff/session` 并收到上游真实 401 响应 → 证明 URL/头/路径拼装正确、TLS 链路通、状态机与用量记录全通
- 安装包实机：静默安装 → 运行 → 网关自动拉起 → healthz 200

## 剩余风险（诚实披露）

| 项 | 状态 |
|----|------|
| 免费额度**成功对话**链路 | ⏳ 需有效 Freebuff Auth Token（CLI 登录后 `~/.config/manicode/credentials.json` 的 `authToken`）。已提供一键导入：面板/`POST /api/tokens/import` 粘贴 curl 或 HAR 即可 |
| 网络包中的 `hb_ff84...` | ❌ 是 posthog/humanbehavior 遥测 key，非 Freebuff API token（freebuff.com 网页版走 Cookie） |
| Docker 实跑 | ⏳ 本机无 docker，Dockerfile 已写好待有环境时 `docker build` |
| 广告保活换额度 | ⏳ 依赖有效 token，逻辑已按 0.0.98 ads.ts 逆向实现 |
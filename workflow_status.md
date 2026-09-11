# Workflow Status — Freebuff2API

> 单一状态源（长期任务恢复 / 节点协作 / 最终验收）。只记录事实与证据。
> Phase G（v0.5.0）已完结发布；Phase H（v0.6.0 收尾）实施完毕待审查放行。
> **Phase I（2026-09-11 启动，用户追加）**：全功能真实 E2E 矩阵——token 保活 / 模型列表 / 工具调用 / 缓存（多轮 thread 复用）/ 长 agent 能力。

## Phase I Task Graph（全功能真实 E2E 矩阵）

| ID | 目标 | 验证方式 | 状态 |
|----|------|----------|------|
| I1 | **token 保活**：保活检查端点真实运行 | 真实凭证调 `/api/account/refresh` → 凭证有效 + 短期 token 已刷新 | ✅ DONE |
| I2 | **模型列表**：`/v1/models` 全链路 | 断言模型数 >0、字段完整、含目标模型、与 /api/guide 一致 | ✅ DONE |
| I3 | **工具调用能力**：web_search/read_url 经桥接透传 | 真实对话触发联网搜索 → 200 + 工具/正文/思考全透传 | ✅ DONE |
| I4 | **缓存/多轮上下文**：同 thread 跨请求记忆 + 复用 | 三轮对话答出代号 + thread 绑定 turns>0 | ✅ DONE |
| I5 | **长 agent 能力**：长任务 + 长输出 | 搜索+500 字总结 → 200 + 内容 >400 字 + 分条结构 | ✅ DONE |
| I6 | **Anthropic 协议流式**：/v1/messages 事件流 | message_start/content_block_delta/message_stop 三件套 + 内容非空 | ✅ DONE |
| I7 | E2E 落盘 `tests/e2e_phase_i.cjs`（18 断言，可重复） | 退出码 0 | ✅ DONE |

**Phase I 执行记录（2026-09-11）**：首跑 15/18（第 4 节 3 失败 = **测试脚本自身 bug**——`json()` helper 双重包装 body 导致网关收到 `{body:{...}}`，model 字段为空；非产品缺陷）。修正脚本后重跑 **18/18 全过**，并回归 phase_g 46/46 + 单测 213 全绿。真实消耗上游额度（多轮真实对话 + 2 次联网搜索）。

## 明确不做（记录理由）

- **web 版 agent-runs/stream（项目页构建模式）**：属新协议面（web 项目页），当前产品形态（对话网关）不覆盖，需先做协议逆向验证；记入 backlog。
- **web 版广告链（/api/ads + imprezia + paidNoFillToken）**：桌面版广告保活已工作；web 版广告协议涉及展示确认闭环，风险高于收益，需独立验证周期；记入 backlog。
- **视频上传**：上游抓包零样本，不对外宣称支持。

## Phase G 交付存档（已完成）

- v0.5.0：commit 822d735 + e050bc9 + e3fc84c，tag v0.5.0，Release 已发布。
- 浏览器一键登录闭环 / 凭证详情与使用记录 / 接入信息显性化 / Web-Cookie 桥接 / 三轮独立审查修复全落地。
- 验证基线：cargo test 213 全绿 · E2E 46/46 · 真实上游链路实测过（含真实凭证八项）。

## Phase H 交付存档（实施完毕，待 critic-h 放行后随 v0.6.0 发布）

- `/v1/models` 鉴权（实测 401/200/401/200 四态）
- config.json 原子写（tmp+rename，往返无字段丢失）
- OsRng 密码学随机 API Key（sk-fb- + 38 字符）
- 桥接流内嵌错误识别接入 errors::classify（单测 5 断言）
- E2E 脚本 try/finally Key 恢复（退出后 config 自动清干净，实测）

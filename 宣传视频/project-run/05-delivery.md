# 宣传片交付清单

## 基本信息
- **产品**: Freebuff2API
- **时长**: 60 秒
- **风格**: 赛博科技（CLI/Hacker）× Apple 发布会级留白
- **分辨率**: 1080×1920 竖屏（抖音 9:16）
- **帧率**: 30fps
- **镜头数**: 8 个
- **仓库**: https://github.com/lza6/Freebuff-2API

## 文件清单

| 文件 | 说明 |
|------|------|
| `master-edit/index.html` | HyperFrames 主合成源文件（可编辑文字/时间/动画） |
| `master-edit/vendor/gsap.min.js` | 本地化 GSAP（渲染无 CDN 依赖） |
| `master-edit/final/promo.mp4` | **最终成片**（1080×1920 · 60s · H.264，2.3MB） |
| `master-edit/renders/promo-draft-60f.mp4` | draft 质量验证版 |
| `01-brief.md` | 创意简报（产品分析/风格/叙事结构） |
| `02-storyboard.md` | 分镜脚本（7 维镜头描述） |
| `03-asset-plan.md` | 素材计划（Pack A 代码实现 / Pack B 真实接口） |
| `04-edl.md` | 编辑决策表（时间码/转场/动效） |
| `06-music-plan.md` | BGM 方案（118bpm · 卡点表 · 3 条生成 Prompt） |
| `assets/pack-b/screens/` | 真实素材（面板截图/接口 JSON/SSE 样例/终端日志） |
| `assets/pack-b/pack-b-sources.md` | 素材来源记录 |

## 视频内容结构
| 段落 | 时间 | 内容 |
|------|------|------|
| Hook | 0-4s | `> ./freebuff2api` 终端打字 + Free Model API |
| Problem | 4-11s | 红 ✕ +「AI 客户端都要付费 key？」+ There's a better way |
| Product | 11-20s | 终端启动 12 行日志 +「One binary. Two protocols.」 |
| Feature 1 | 20-28s | 双协议出口 /v1/chat/completions + /v1/messages |
| Feature 2 | 28-35s | Run 层级自动维护（session→root→sub-agent 树） |
| Feature 3 | 35-43s | 多 Token 轮换 + 并发租约 + 401 冷却 |
| Feature 4 | 43-52s | 内置 Web 控制台（账号健康/用量/请求） |
| CTA | 52-60s | 五角星 + github.com/lza6/Freebuff-2API + 去 GitHub 点亮 ⭐ |

## 验证记录
- ✅ **渲染管线**：hyperframes render 1800 帧全帧完成（draft 35.8s / high 40.5s）
- ✅ **P0 检查**：无素材 404、无黑帧、首帧非黑、每镜头有动画、转场不连续重复
- ✅ **真实素材**：本地网关 `/ui` 面板真实截图、`/healthz` `/v1/models` `/api/account/balance` 真实 JSON、真实启动日志
- ⚠️ **待验证**：真实 chat 流式调用因本地 config 无真实 token 返回 401（已验证此行为）；流式 SSE 协议形态用 mock 还原，字段/事件名与上游一致
- ⚠️ **BGM 未合成**：需 Mureka API key 或用户提供 BGM，按 `06-music-plan.md` 卡点合成

## 修改指南
如需微调：
1. 编辑 `master-edit/index.html` 中的文字/时间/动画（GSAP timeline 从第 185 行起）
2. 预览：`npx hyperframes preview`（默认 3002 端口）
3. 渲染：`npx hyperframes render . -c . -q high -f 30 -o final/promo.mp4`
4. 加 BGM：`ffmpeg -i final/promo.mp4 -i bgm.mp3 -c:v copy -c:a aac -shortest final/promo-with-bgm.mp4`

## 已知限制
1. 画面中的终端日志/数值为**演示数据**（网关实际日志脱敏）；不泄露任何真实 token
2. 未用真实截图库/icon（全部代码实现），纯矢量观感
3. BGM 需单独生成或选用授权曲目（无内置音轨）

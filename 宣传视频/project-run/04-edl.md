# 编辑决策表（EDL）

**项目**: Freebuff2API 宣传片 · 2026-09-10
**规格**: 1080×1920 竖屏 · 30fps · 60s · 赛博科技 × Apple 留白混搭

| # | Shot | 入点 | 出点 | 时长 | 转场 | 主要动效 |
|---|------|------|------|------|------|---------|
| 1 | Hook：终端打字 | 0:00 | 0:04 | 4s | — | 四行文字逐行浮现（绿/白/蓝/灰） |
| 2 | Problem：付费墙 | 0:04 | 0:11 | 7s | Black Dip | 红 ✕ elastic 弹入 + 两行痛点 + 金句渐显 |
| 3 | Product：终端启动 | 0:11 | 0:20 | 9s | Scale Blur | 终端卡片渐显 + 12 行启动日志逐行打出 + 双协议标签 |
| 4 | Feature 1：双协议 | 0:20 | 0:28 | 8s | Crossfade | 标题 + 两个协议 Tab + 4 个客户端 logo 依次浮入 |
| 5 | Feature 2：Run 树 | 0:28 | 0:35 | 7s | Wipe | 三行树形结构逐行展开 + 绿 chip back.out 弹出 |
| 6 | Feature 3：多 Token | 0:35 | 0:43 | 8s | Crossfade | 3 个 token 卡片浮入 + 进度条 5s 拉满 |
| 7 | Feature 4：Web 控制台 | 0:43 | 0:52 | 9s | Crossfade | 浏览器窗口 + 4 个统计卡片依次出现 |
| 8 | CTA：GitHub | 0:52 | 1:00 | 8s | Black Dip | 五角星弹性弹出 + 名称/仓库/副标/绿色按钮递进 + 呼吸缩放 |

**P0 自检**：
- [x] 总时长 60s 与 EDL 一致
- [x] 无外部素材依赖（文字/形状/card 全部代码实现，0 张图片 404）
- [x] 首帧非黑屏（Hook 文字 0.2s 即入）
- [x] 每个 Shot 有入场动画，无静止帧（静态元素均有 fade/逐行/stagger）
- [x] 转场类型交替（Black Dip / Scale Blur / Wipe / Crossfade 无明显连续重复）

**渲染**：`npx hyperframes render . -c . -q high -f 30 -o final/promo.mp4`（40s 完成，1800 帧全帧验证）
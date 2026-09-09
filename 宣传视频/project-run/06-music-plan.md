# BGM 方案 — Freebuff2API 宣传片

## 视频摘要
- 产品：Freebuff2API（Codebuff Freebuff 免费层 → OpenAI/Claude 兼容 API 网关）
- 时长：60s
- 风格：赛博科技 × Apple 留白
- 音乐目标：节奏感强的 product launch / UI groove，无口播，纯 BGM 撑节奏

## 音乐方向
- 类型：Commercial Product Launch × Clean UI Groove
- BPM：**118**（服务卡点：镜头切点 4/11/20/28/35/43/52 均靠近 118bpm 小节边界）
- 拍号：4/4
- 调性：D 小调（现代科技感，略暗但有推进）
- 情绪曲线：`branded intro → product reveal lift → workflow groove → confident CTA resolve`
- 主要乐器：tight electronic drums · sidechained synth bass · bright plucked synth hook · warm chord pads
- 声音质感：polished / confident / forward-moving，不 ambient
- 避免：无口播、无俗套企业钢琴、无 trap hats、无 EDM festival drop、无 trailer booms

## 卡点表

| 时间点 | 画面事件 | 音乐动作 | 强度 |
|--------|---------|---------|------|
| 0-4s | Hook 四行打字 | 滤波 intro + 低频脉冲，hook 埋点 | 1 |
| 4s | Problem 红 ✕ | clean glass hit | 2 |
| 11s | 终端启动 | low pulse opens + airy shimmer | 3 |
| 16.5s | 双协议标签 | muted tick sequence | 3 |
| 20s | 双协议标题 | bass 进入，节奏完全立起 | 4 |
| 28s | Run 树 | pulsing synth ostinato | 4 |
| 35s | 多 Token 进度条 | arpeggio notes 推高 | 4 |
| 43s | Web 控制台 | 卡片矩阵 + crisp accents | 4 |
| 52s | CTA | bass resolve + long warm pad tail | 5 |
| 56.2s | 按钮呼吸 | 最后 kick 收束 | 5 |

## 生成 Prompt

### Primary Prompt（Commercial Product Launch）
```
modern commercial product launch music for a 60-second AI software showcase, 118 BPM, 4/4, instrumental only, polished, confident, forward-moving. Tight electronic drums, sidechained synth bass, bright plucked synth hook, warm chord pads, clean UI impact hits. Structure: 0-6s filtered branded intro; 6-15s groove builds under problem section; 15-24s full product-demo beat as terminal boots; 24-34s hook variation and crisp accents as feature cards assemble; 34-44s stronger rhythmic lift for run-tree and token rotation; 44-52s open uplifting section for web console; 52-60s short confident resolve for GitHub CTA. Hit accents at 4, 11, 16.5, 20, 28, 35, 43, 52, 56.2s. No vocals, no lyrics, no sleepy ambient, no generic corporate piano, no trap hats, no EDM festival drop, no trailer booms.
```

### Alternative Prompt A（Clean UI Demo Groove）
```
clean UI demo groove for a software product promo video, 112 BPM, 4/4, instrumental only, premium, precise, modular. Muted kick, crisp snare snaps, syncopated digital percussion, rubbery synth bass, short glass-pluck motif, soft sparkle pads. Structure: 0-6s sparse tick intro; 6-15s pulsing grid rhythm; 15-24s UI clicks and glass hits as code types; 24-34s rhythm lifts with arpeggio runs; 34-44s denser groove with sub-drop; 44-52s open airy break; 52-60s warm pad resolve. Hit accents at 11, 20, 28, 35, 43, 52s. No vocals, no trap hats, no festival drops, no corporate jingle, no sleepy pads.
```

### Alternative Prompt B（Tech-Pop Instrumental）
```
premium tech-pop instrumental for a developer-tool launch film, 124 BPM, 4/4, instrumental only, energetic, bright, optimistic. Punchy kick, driving synth bass, anthemic plucked lead hook, layered digital strings, crisp claps. Structure: 0-6s filtered riser intro; 6-15s groove establishes; 15-24s beat drops full; 24-34s hook variation with risers; 34-44s breakdown with arpeggio and sidechain pump; 44-52s build to climax; 52-60s confident anthem resolve. Hit accents at 4, 11, 20, 28, 35, 43, 52, 56.2s. No vocals, no lofi, no ambient, no EDM festival drop, no trap hats.
```

### Negative Prompt
```
no vocals, no lyrics, no singing, no narration, no sleepy ambient, no generic corporate piano, no trap hats, no EDM festival drop, no trailer booms, no distorted bass, no dissonance, no samples with watermarks, no low-quality compression artifacts
```

## 生成建议
- 推荐生成数量：n=3（Primary + A + B 三选一）
- 推荐输出时长：60-75s（后续按 EDL 精确裁剪到 60s）
- 后期剪辑方式：用 beat 对齐工具（librosa）检测 kick 瞬态，将切点钉在真实 kick 上（误差 ≤3 帧）
- 音量建议：-16 LUFS（宣传片标准），BGM 比字幕/界面音效低 6dB

## 说明
- 本方案仅输出可执行 Prompt；**实际 BGM 生成需调用 Mureka/Skywork Music Maker API（需 `MUREKA_API_KEY`）**，或用抖音素材库/BGM 库选曲后按本卡点表手动对齐
- 用户选择「完整成片」交付，成片已渲染为无 BGM 版；用户提供 BGM 后可 `ffmpeg -i final/promo.mp4 -i bgm.mp3 -c:v copy -c:a aac -shortest final/promo-with-bgm.mp4` 合成

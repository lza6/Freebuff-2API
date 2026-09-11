//! 内置 Web 控制面板（总览 / 账号 / 技能 / 日志 / 体检 / 接入指南）
//!
//! 轻量无构建：单 HTML + 原生 JS + CSS，Rust 直接内嵌字符串。
//! 设计要点：
//! - 固定容器 + innerHTML 重建（避免 DOM 堆积）
//! - Tab 内切换（不再跳转裸 JSON 页）
//! - 空态有引导，失败有提示（不含糊）
//! - SSE 实时日志 + 请求详情抽屉 + 系统体检

pub const INDEX_HTML: &str = r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Freebuff2API 控制台</title>
<style>
:root { --bg:#0d1117; --card:#161b22; --border:#30363d; --text:#e6edf3; --muted:#8b949e; --accent:#2f81f7; --ok:#3fb950; --warn:#d29922; --err:#f85149; }
* { box-sizing:border-box; margin:0; padding:0; }
body { background:var(--bg); color:var(--text); font-family:-apple-system,'Segoe UI',Roboto,'Microsoft YaHei',sans-serif; min-height:100vh; }
header { display:flex; align-items:center; justify-content:space-between; padding:12px 24px; border-bottom:1px solid var(--border); background:var(--card); position:sticky; top:0; z-index:10; }
header h1 { font-size:17px; font-weight:600; }
header .dot { display:inline-block; width:8px; height:8px; border-radius:50%; background:var(--muted); margin-right:8px; vertical-align:middle; }
header .dot.ok { background:var(--ok); } header .dot.err { background:var(--err); }
.hstat { display:flex; gap:16px; font-size:12px; color:var(--muted); }
.hstat b { color:var(--text); }
main { max-width:1240px; margin:0 auto; padding:20px 24px 60px; }
nav { display:flex; gap:4px; margin-bottom:20px; border-bottom:1px solid var(--border); flex-wrap:wrap; }
nav button { background:transparent; border:none; color:var(--muted); padding:10px 16px; cursor:pointer; font-size:14px; border-bottom:2px solid transparent; border-radius:0; }
nav button.active { color:var(--text); border-bottom-color:var(--accent); }
nav button:hover { color:var(--text); }
.cards { display:grid; grid-template-columns:repeat(auto-fit,minmax(170px,1fr)); gap:14px; margin-bottom:20px; }
.card { background:var(--card); border:1px solid var(--border); border-radius:10px; padding:14px 16px; }
.card .num { font-size:26px; font-weight:700; margin-top:4px; }
.card .lbl { color:var(--muted); font-size:13px; }
.grid { display:grid; grid-template-columns:1fr 1fr; gap:20px; }
@media(max-width:920px){ .grid{grid-template-columns:1fr} }
.panel { background:var(--card); border:1px solid var(--border); border-radius:10px; padding:16px; margin-bottom:20px; }
.panel h2 { font-size:15px; margin-bottom:12px; color:var(--muted); font-weight:600; }
.panel h1 { font-size:18px; margin-bottom:12px; }
table { width:100%; border-collapse:collapse; font-size:13px; }
th,td { text-align:left; padding:7px 10px; border-bottom:1px solid var(--border); }
th { color:var(--muted); font-weight:500; }
tr.click { cursor:pointer; } tr.click:hover { background:#1c2129; }
.badge { display:inline-block; padding:2px 8px; border-radius:20px; font-size:12px; }
.badge.ok { background:#1a5e2a; color:var(--ok); }
.badge.warn { background:#5a4a1a; color:var(--warn); }
.badge.err { background:#5e1a1a; color:var(--err); }
.badge.dim { background:#21262d; color:var(--muted); }
.chip { display:inline-block; background:#21262d; border:1px solid var(--border); border-radius:6px; padding:2px 8px; margin:2px; font-size:12px; }
button { background:var(--accent); color:#fff; border:none; border-radius:6px; padding:6px 14px; cursor:pointer; font-size:13px; }
button:hover { filter:brightness(1.12); }
button.ghost { background:transparent; border:1px solid var(--border); color:var(--text); }
button.sm { padding:3px 10px; font-size:12px; }
input,textarea,select { background:#0d1117; border:1px solid var(--border); color:var(--text); border-radius:6px; padding:8px 10px; font-size:13px; width:100%; font-family:inherit; }
textarea { min-height:90px; resize:vertical; }
label { display:block; color:var(--muted); font-size:12px; margin:8px 0 4px; }
.row { display:flex; gap:8px; align-items:center; flex-wrap:wrap; }
.empty { color:var(--muted); font-size:13px; padding:14px 4px; }
.empty b { color:var(--text); }
#toast { position:fixed; bottom:24px; left:50%; transform:translateX(-50%); background:var(--card); border:1px solid var(--accent); padding:10px 20px; border-radius:8px; display:none; z-index:100; font-size:13px; }
.logs { max-height:460px; overflow:auto; font-family:ui-monospace,Consolas,monospace; font-size:12px; background:#0a0d12; border:1px solid var(--border); border-radius:8px; padding:8px; }
.logs div { padding:2px 4px; border-bottom:1px dashed #1c2129; white-space:pre-wrap; word-break:break-all; }
.logs .lv-warn { color:var(--warn); } .logs .lv-error { color:var(--err); } .logs .lv-info { color:var(--muted); }
#drawer { position:fixed; top:0; right:-560px; width:560px; max-width:92vw; height:100vh; background:var(--card); border-left:1px solid var(--border); transition:right .18s ease; overflow:auto; padding:20px; z-index:50; }
#drawer.open { right:0; }
#drawer h3 { margin-bottom:10px; }
.kv { font-size:13px; margin:4px 0; } .kv b { color:var(--muted); font-weight:500; display:inline-block; min-width:110px; }
pre { background:#0a0d12; border:1px solid var(--border); border-radius:8px; padding:10px; overflow:auto; font-size:12px; }
details { margin:6px 0; } summary { cursor:pointer; color:var(--muted); font-size:13px; }
.banner { background:linear-gradient(135deg,#132a4a,#161b22); border:1px solid var(--accent); border-radius:10px; padding:16px; margin-bottom:20px; }
.banner h2 { color:var(--text); margin-bottom:8px; }
.banner ol { margin-left:20px; font-size:13px; color:var(--muted); line-height:2; }
.banner code { background:#0a0d12; padding:2px 6px; border-radius:4px; }
.doctor-item { display:flex; gap:10px; padding:10px 0; border-bottom:1px solid var(--border); font-size:13px; align-items:flex-start; }
.doctor-item .st { min-width:56px; }
.tok { color:var(--ok); } .twarn { color:var(--warn); } .terr { color:var(--err); }
#login-wizard { scroll-margin-top:70px; }
@keyframes wizardFlash { 0%,100% { box-shadow:0 0 0 0 rgba(47,129,247,0); } 50% { box-shadow:0 0 0 4px rgba(47,129,247,.5); } }
.wizard-flash { animation:wizardFlash .8s ease-in-out 2; }
</style>
</head>
<body>
<header>
  <h1><span class="dot" id="dot"></span>Freebuff2API 控制台 <span style="color:var(--muted);font-size:12px" id="ver"></span></h1>
  <div class="hstat" id="hstat"></div>
  <input id="api-key-input" type="password" placeholder="API Key（配了 api_keys 才需要）" title="配置了 api_keys 时，面板请求需带此 Key；仅存本机浏览器" style="width:200px;font-size:12px" onchange="setApiKey(this.value)">
</header>
<main>
  <div id="banner"></div>
  <div class="cards" id="cards"></div>
  <div id="cost-line" style="font-size:13px;color:var(--muted);margin:-8px 0 16px 2px"></div>
  <nav>
    <button data-tab="overview" class="active" onclick="showTab('overview')">总览</button>
    <button data-tab="account" onclick="showTab('account')">账号</button>
    <button data-tab="skills" onclick="showTab('skills')">技能</button>
    <button data-tab="memory" onclick="showTab('memory')">记忆</button>
    <button data-tab="logs" onclick="showTab('logs')">实时日志</button>
    <button data-tab="teach" onclick="showTab('teach')">原理</button>
    <button data-tab="doctor" onclick="showTab('doctor')">系统体检</button>
    <button data-tab="guide" onclick="showTab('guide')">接入指南</button>
  </nav>

  <section id="tab-overview">
    <!-- 立刻开始请求：地址 + Key + 一键复制（回答"导入凭证之后呢？"） -->
    <div class="panel" id="connect-panel">
      <div class="row" style="margin-bottom:10px">
        <h2 style="margin:0">🚀 立刻开始请求</h2>
        <span style="flex:1"></span>
        <span id="connect-ready" style="font-size:12px;color:var(--muted)"></span>
      </div>
      <div class="grid" style="gap:16px">
        <div>
          <label>接口地址（Base URL）</label>
          <div class="row"><input id="c-base" readonly style="flex:1"><button class="ghost sm" onclick="copyText(document.getElementById('c-base').value)">复制</button></div>
          <label style="margin-top:8px">OpenAI 协议地址（Cursor / LobeChat / SDK）</label>
          <div class="row"><input id="c-openai" readonly style="flex:1"><button class="ghost sm" onclick="copyText(document.getElementById('c-openai').value)">复制</button></div>
          <label style="margin-top:8px">Anthropic 协议地址（Claude Code）</label>
          <div class="row"><input id="c-anthropic" readonly style="flex:1"><button class="ghost sm" onclick="copyText(document.getElementById('c-anthropic').value)">复制</button></div>
        </div>
        <div>
          <label>API Key</label>
          <div class="row"><input id="c-key" readonly style="flex:1"><button class="ghost sm" onclick="copyText(document.getElementById('c-key').value)">复制</button></div>
          <div class="row" style="margin-top:8px">
            <button class="sm" onclick="genApiKey()">生成并启用 Key</button>
            <button class="ghost sm" onclick="clearApiKey()">清除 Key</button>
          </div>
          <div id="c-key-hint" style="font-size:12px;color:var(--muted);margin-top:8px"></div>
          <div id="c-model-hint" style="font-size:12px;color:var(--muted);margin-top:6px"></div>
        </div>
      </div>
      <div style="margin-top:12px;font-size:13px;color:var(--muted)">把上面两项填进客户端就能用了 —— 现成配置片段见
        <a href="#" onclick="showTab('guide');return false" style="color:var(--accent)">接入指南</a>。</div>
    </div>

    <div class="grid">
      <div class="panel"><h2>账号健康度</h2><div id="acc-wrap"></div></div>
      <div class="panel"><h2>近 7 天用量</h2><div id="daily-wrap"></div></div>
    </div>
    <div class="panel"><h2>最近请求 <span style="font-weight:400">（点击行查看详情）</span></h2><div id="reqs-wrap"></div></div>
    <div class="panel"><h2>可用模型（<span id="model-count">…</span>）</h2><div id="models-wrap"></div></div>
    <div class="panel" id="balance-panel" style="display:none"><h2>账号积分</h2><div id="balance-wrap"></div></div>
  </section>

  <section id="tab-account" style="display:none">
    <!-- 账号全貌（身份 / 用量 / 套餐 / 积分） -->
    <div class="panel">
      <div class="row" style="margin-bottom:10px">
        <h2 style="margin:0">账号全貌</h2>
        <span style="flex:1"></span>
        <button class="ghost sm" onclick="refreshAccountOverview()">刷新</button>
        <button class="ghost sm" onclick="refreshCredential()">🔄 保活检查</button>
      </div>
      <div id="overview-wrap"><div class="empty">点「刷新」拉取账号信息（身份 / 连续使用天数 / token 消耗 / 套餐 / 今日剩余积分）</div></div>
    </div>

    <!-- 添加账号（一键登录向导 / 粘贴导入） -->
    <div class="panel">
      <h2>添加账号</h2>
      <div class="row" style="margin-bottom:10px">
        <button onclick="oneClickLogin()">🔑 一键登录</button>
        <span id="ext-status" class="badge dim">检测扩展中…</span>
        <button class="ghost sm" onclick="downloadExtension()">⬇ 下载扩展</button>
        <button class="ghost sm" onclick="pingExtension(true)">重新检测</button>
      </div>

      <div id="login-wizard" style="display:none;border:1px solid var(--accent);border-radius:8px;padding:14px;margin-bottom:12px;background:linear-gradient(135deg,#132a4a,#161b22)">
        <div class="row" style="margin-bottom:8px"><b>浏览器版一键登录</b><span style="flex:1"></span><button class="ghost sm" onclick="document.getElementById('login-wizard').style.display='none'">收起</button></div>
        <div id="wizard-embed" style="font-size:13px;margin-bottom:10px;padding:8px;background:#0d1117;border-radius:6px">
          <b>方案 A（最推荐 · 零安装零复制）：内嵌登录窗口</b> <span class="badge ok">登录完即完事</span>
          <div style="margin:6px 0;color:var(--muted);line-height:1.9">点下面的按钮会弹出一个内置浏览器窗口（系统自带的 WebView2 组件）→ 在窗口里正常完成 GitHub 登录 → 网关<b>自动</b>抓取凭证入库并关闭窗口。全程无需装扩展、无需复制任何东西。</div>
          <button onclick="openEmbedLogin()">🪟 弹出内嵌登录窗口</button>
          <span id="embed-status" style="font-size:12px;color:var(--muted);margin-left:8px"></span>
        </div>
        <div id="wizard-clip" style="font-size:13px;margin-bottom:10px;padding:8px;background:#0d1117;border-radius:6px">
          <b>方案 B（推荐 · 无需安装）：剪贴板自动检测</b> <span class="badge ok">约 30 秒</span>
          <div style="margin:6px 0;color:var(--muted);line-height:1.9">去 freebuff.com 登录 → 按 <b>F12</b> → <b>Network</b> → 点任意请求 → 在 <b>Headers</b> 里找到 <code>Cookie:</code> 开头那一行并整行复制（Ctrl+C）→ 回来点下面这个按钮，剩下的自动完成。</div>
          <button onclick="importFromClipboard()">📋 自动检测剪贴板</button>
          <span id="clip-status" style="font-size:12px;color:var(--muted);margin-left:8px"></span>
        </div>
        <div id="wizard-ext" style="font-size:13px;margin-bottom:10px;padding:8px;background:#0d1117;border-radius:6px">
          <b>方案 C（全自动 · 装一次以后都不用管）：Chrome / Edge 扩展</b> <span class="badge dim">首次约 2 分钟</span>
          <ol style="margin:6px 0 0 20px;color:var(--muted);line-height:1.9">
            <li>点上方「⬇ 下载扩展」得到 zip → 解压到任意目录（也可直接用项目里的 <code>browser-extension/</code> 目录）</li>
            <li>打开 <code>chrome://extensions</code>（Edge 为 <code>edge://extensions</code>）→ 打开「开发者模式」→「加载已解压的扩展程序」→ 选中刚解压的目录</li>
            <li>回到本页点「重新检测」→ 状态变成 <span class="badge ok">已就绪</span> 后，再点「一键登录」即可<b>全自动</b>：自动打开 freebuff.com → 你完成 GitHub 登录 → 凭证自动入库（含 HttpOnly Cookie，网页 JS 读不到，只有扩展能读）</li>
          </ol>
        </div>
        <b style="font-size:13px">手动粘贴（兜底）：<span style="color:var(--muted);font-weight:400">支持 Cookie 串 / cURL / HAR</span></b> <span class="badge dim">约 1 分钟</span>
        <ol style="margin:6px 0 10px 20px;font-size:13px;color:var(--muted);line-height:2">
          <li><button class="ghost sm" onclick="window.open('https://freebuff.com/','_blank','noopener')">① 打开 freebuff.com 并登录</button>（GitHub 登录即可）</li>
          <li>按 <b>F12</b> → <b>Network</b> → 刷新 → 点任意请求 → <b>Headers</b> 里找 <code>Cookie:</code> 整行复制（或 Application → Cookies 里复制 <code>__Secure-next-auth.session-token</code> 的值）</li>
          <li>回到本页面粘贴到下方输入框 → 点「导入」（导入后会自动验证凭证是否有效）</li>
        </ol>
        <div style="font-size:12px;color:var(--muted)">💡 为什么网页不能全自动？上游登录 Cookie 标记为 HttpOnly（浏览器禁止网页脚本读取）——方案 B 的"复制"动作由你亲手完成（Ctrl+C 什么都能复制，HttpOnly 也拦不住剪贴板），网页脚本只负责读剪贴板和导入；扩展可以合法直接读 Cookie；桌面版由 Electron 主进程读取，因此桌面版是托盘一键全自动。</div>
      </div>

      <textarea id="import-text" placeholder="粘贴以下任意一种：
1) 浏览器 Cookie 串（含 __Secure-next-auth.session-token=...）
2) 从 DevTools 复制的 cURL (bash) 命令
3) HAR 导出文件的 JSON 内容
提示：也可以在 DevTools 里复制 Cookie 整行后，用向导里的「📋 自动检测剪贴板」一步完成"></textarea>
      <div class="row" style="margin-top:10px">
        <button onclick="doImport()">导入</button>
        <span id="import-result" style="font-size:13px;color:var(--muted)"></span>
      </div>
      <div id="import-verify" style="display:none;margin-top:10px;font-size:13px"></div>
      <details style="margin-top:12px"><summary>怎么获取 Cookie？（点击展开详细图文说明）</summary>
        <ol style="margin:10px 0 0 20px;font-size:13px;color:var(--muted);line-height:1.9">
          <li>浏览器登录 freebuff.com</li>
          <li>按 F12 打开开发者工具 → Network 标签</li>
          <li>刷新页面，点任意请求 → Headers → 找到 <code>Cookie:</code> 开头那一整行</li>
          <li>整行复制，粘贴到上面的框里，点「导入」</li>
          <li>也可以在 Application → Cookies → https://freebuff.com 里逐项复制（需要包含 <code>__Secure-next-auth.session-token</code>）</li>
        </ol>
      </details>
    </div>

    <!-- 凭证列表 -->
    <div class="panel">
      <h2>已入库凭证 <span id="cred-count" style="font-weight:400;color:var(--muted);font-size:12px"></span></h2>
      <div id="tokens-wrap"></div>
    </div>

    <!-- 使用记录（每个账号的额度/消耗快照，可查历史） -->
    <div class="panel">
      <div class="row" style="margin-bottom:10px">
        <h2 style="margin:0">使用记录</h2>
        <span style="flex:1"></span>
        <select id="hist-cred" style="width:auto" onchange="loadHistory()"><option value="">全部账号</option></select>
        <button class="ghost sm" onclick="loadHistory()">刷新</button>
      </div>
      <div id="hist-wrap"><div class="empty">每次「刷新账号全貌」或「检查」都会记录一条 —— 点「刷新」查看</div></div>
    </div>
  </section>

  <section id="tab-skills" style="display:none">
    <div class="panel">
      <h2>技能库 <span style="font-weight:400;color:var(--muted);font-size:12px">（启用后注入对话 system 前缀；roster 模式只注入名称与描述）</span></h2>
      <div class="row" style="margin-bottom:10px">
        <button onclick="newSkill()">＋ 新建技能</button>
        <span id="roster-info" style="font-size:12px;color:var(--muted)"></span>
      </div>
      <div id="skills-wrap"></div>
    </div>
    <div class="panel" id="skill-editor" style="display:none">
      <h2 id="skill-editor-title">编辑技能</h2>
      <label>名称</label><input id="sk-name" placeholder="例如：周报助手">
      <label>描述（写给 AI 的触发说明：什么时候用这个技能）</label><input id="sk-desc" placeholder="Use when the user wants to write a weekly report...">
      <label>指令正文（Markdown）</label><textarea id="sk-body" style="min-height:200px" placeholder="技能的完整指令内容…"></textarea>
      <div class="row" style="margin-top:10px">
        <button onclick="saveSkill()">保存</button>
        <button class="ghost" onclick="document.getElementById('skill-editor').style.display='none'">取消</button>
        <span id="skill-save-result" style="font-size:13px;color:var(--muted)"></span>
      </div>
      <details style="margin-top:10px"><summary>质量门检查（保存前会提示问题）</summary><div id="gate-result" style="font-size:13px;color:var(--muted);margin-top:6px"></div></details>
    </div>
  </section>

  <section id="tab-memory" style="display:none">
    <div class="panel">
      <h2>记忆库 <span style="font-weight:400;color:var(--muted);font-size:12px">（AI 从这里学习你的偏好与纠正；零 LLM 规则记录，纯本地）</span></h2>
      <div id="mem-stats" style="margin-bottom:10px;font-size:13px;color:var(--muted)"></div>
      <div class="row" style="margin-bottom:10px">
        <button onclick="newMemory()">＋ 手动添加</button>
        <span style="font-size:12px;color:var(--muted)">自动记录：常用模型 / 推理档位降级 / 你的纠正（"记住…"、"别再…"、"always/never"）</span>
      </div>
      <div id="mem-editor" style="display:none;border:1px solid var(--border);border-radius:8px;padding:12px;margin-bottom:12px">
        <label>类型</label>
        <select id="mem-kind">
          <option value="preference">偏好</option><option value="correction">纠正</option>
          <option value="habit">习惯</option><option value="project">项目</option><option value="feedback">反馈</option>
        </select>
        <label>标题</label><input id="mem-title" placeholder="例如：偏好中文回答 / 常用模型 glm-5.3-flash">
        <label>内容</label><textarea id="mem-content" style="min-height:80px" placeholder="具体内容（相关对话时会注入 system 前缀，低权威）"></textarea>
        <div class="row" style="margin-top:10px">
          <button onclick="saveMemory()">保存</button>
          <button class="ghost" onclick="document.getElementById('mem-editor').style.display='none'">取消</button>
          <span id="mem-save-result" style="font-size:13px;color:var(--muted)"></span>
        </div>
      </div>
      <div id="mem-wrap"></div>
    </div>
  </section>

  <section id="tab-teach" style="display:none">
    <div class="panel">
      <h2>原理速览 <span style="font-weight:400;color:var(--muted);font-size:12px">（这个网关背后发生了什么）</span></h2>
      <details open><summary><b>① 请求进来之后</b></summary>
        <p style="font-size:13px;color:var(--muted);line-height:1.9;margin-top:6px">
        客户端（Claude Code / Cursor）把 OpenAI 或 Claude 格式的请求发到本地 <code>47821</code> 端口。
        网关先解析模型名 → 从账号池挑健康账号（评分 + 熔断状态）→ 确保该账号在上游有活跃会话
        → 把请求改写为上游格式（注入 run 元数据、按模型校正思考档位、拼上提示词/技能/记忆）
        → 转发上游，再把响应（流式）实时转回客户端格式。</p></details>
      <details><summary><b>② 多账号是怎么"轮询"的</b></summary>
        <p style="font-size:13px;color:var(--muted);line-height:1.9;margin-top:6px">
        每个账号有独立的健康分与熔断器（Closed / Open / HalfOpen 三态）：连续失败 4 次自动断开，
        冷却随断开次数指数增长（封顶 10 分钟）；冷却结束进入半开状态放行探测，连续成功 2 次恢复。
        请求失败会自动换号重试（最多 3 次）——但只在"尚未向客户端写出任何字节"之前重试，
        绝不会把半截响应写给你。</p></details>
      <details><summary><b>③ 提示词 / 技能 / 记忆是怎么注入的</b></summary>
        <p style="font-size:13px;color:var(--muted);line-height:1.9;margin-top:6px">
        注入顺序：基础提示词 → 启用的提示词 → 技能 roster（名称+描述）→ 记忆块（低权威），
        拼成一条 system 消息放最前面。技能与记忆有严格预算（默认 2000 / 512 token），超出整条丢弃，
        避免"越装越贵"。记忆按你当前的问题检索（本地 trigram 全文索引，支持中文）。</p></details>
      <details><summary><b>④ 黑匣子：为什么这次慢 / 为什么失败</b></summary>
        <p style="font-size:13px;color:var(--muted);line-height:1.9;margin-top:6px">
        每次请求都记录：路由决策（请求模型→实际模型→账号）、上游状态码、首字节时间、总耗时、token 用量、
        错误类型与片段。在「总览 → 最近请求」点任意一行看细节与人话解释；「实时日志」页实时推送
        正在发生的事（断线自动补发）。</p></details>
      <details><summary><b>⑤ 免费额度与广告保活</b></summary>
        <p style="font-size:13px;color:var(--muted);line-height:1.9;margin-top:6px">
        上游免费层通过"会话 + 广告刷新"维持额度：网关每 45 秒心跳，会话剩余不足时触发广告刷新延长。
        出现 <code>waiting_room_queued</code> 表示上游在排队——不是网关故障，稍等重试或多加账号提升并发。</p></details>
      <details><summary><b>⑥ 上游会话自动清理</b></summary>
        <p style="font-size:13px;color:var(--muted);line-height:1.9;margin-top:6px">
        每次对话在上游都会生成一个 thread。网关记录自己创建的 thread，并<b>每小时</b>清理超过
        <b>24 小时</b>的旧会话（<code class="ok">thread_cleanup_interval_sec</code> /
        <code class="ok">thread_max_age_hours</code> 可调，间隔设 0 关闭）——避免反代长期堆积给上游制造压力。
        也可在「原理」页对应的 API 手动预演：<code>POST /api/threads/cleanup {"dry_run":true}</code>。</p></details>
      <details><summary><b>⑦ 数据都存在哪</b></summary>
        <p style="font-size:13px;color:var(--muted);line-height:1.9;margin-top:6px">
        全部本地：<code>data/freebuff2api.sqlite</code>（用量）、<code>data/telemetry.sqlite</code>（请求详情）、
        <code>data/memory.sqlite</code>（记忆）、<code>data/skills/</code>（技能 Markdown，真相源）、
        <code>data/tokens.json</code>（凭证）。备份或整体删除即可重置。</p></details>
    </div>
  </section>

  <section id="tab-logs" style="display:none">
    <div class="panel">
      <div class="row" style="margin-bottom:10px">
        <h2 style="margin:0">实时日志</h2>
        <span style="flex:1"></span>
        <select id="log-filter" style="width:auto" onchange="renderLogs()">
          <option value="">全部级别</option><option value="info">info</option><option value="warn">warn</option><option value="error">error</option>
        </select>
        <button class="ghost sm" onclick="clearLogs()">清空显示</button>
      </div>
      <div class="logs" id="logbox"><div class="empty">等待日志…（发起一次对话即可看到请求链路）</div></div>
    </div>
  </section>

  <section id="tab-doctor" style="display:none">
    <div class="panel">
      <h2>系统体检 <span style="font-weight:400;color:var(--muted);font-size:12px">（检查结果只是信号，不是判决；"未检查"就是未检查）</span></h2>
      <button class="ghost sm" onclick="refreshDoctor()">重新检查</button>
      <div id="doctor-wrap" style="margin-top:10px"><div class="empty">点击「重新检查」开始</div></div>
    </div>
  </section>

  <section id="tab-guide" style="display:none">
    <div class="panel">
      <h2>把这个网关接入你的 AI 客户端</h2>
      <div style="background:#0d1117;border:1px solid var(--border);border-radius:8px;padding:12px;margin:10px 0 16px;font-size:13px;line-height:2">
        <b>三步走：</b>
        ① 在「账号」页导入凭证（或一键登录）→
        ② 确认上方状态灯为绿色（网关运行中）→
        ③ 按下面任意一种方式配置你的客户端即可开始对话。
      </div>
      <p style="font-size:13px;color:var(--muted);margin-bottom:10px">网关地址：<code id="guide-base">http://127.0.0.1:47821</code>（OpenAI 协议加 <code>/v1</code> 后缀；Anthropic 协议不加）</p>
      <p style="font-size:13px;color:var(--muted);margin-bottom:10px"><b>API Key 填什么？</b> <span id="guide-key-hint">config.json 未配置 api_keys 时，任意字符串即可（如 <code>sk-local</code>）</span></p>

      <h2 style="margin-top:16px">Claude Code（Anthropic 协议）</h2>
      <pre id="g-claude"></pre><button class="ghost sm" onclick="copyText(document.getElementById('g-claude').textContent)">复制</button>

      <h2 style="margin-top:16px">Cursor / Continue / 通用 OpenAI 客户端</h2>
      <pre id="g-openai"></pre><button class="ghost sm" onclick="copyText(document.getElementById('g-openai').textContent)">复制</button>

      <h2 style="margin-top:16px">OpenAI SDK (Python)</h2>
      <pre id="g-py"></pre><button class="ghost sm" onclick="copyText(document.getElementById('g-py').textContent)">复制</button>

      <h2 style="margin-top:16px">OpenAI SDK (Node.js)</h2>
      <pre id="g-node"></pre><button class="ghost sm" onclick="copyText(document.getElementById('g-node').textContent)">复制</button>

      <h2 style="margin-top:16px">curl 快速验证</h2>
      <pre id="g-curl"></pre><button class="ghost sm" onclick="copyText(document.getElementById('g-curl').textContent)">复制</button>

      <h2 style="margin-top:16px">LobeChat / NextChat / Cherry Studio</h2>
      <pre id="g-lobe"></pre><button class="ghost sm" onclick="copyText(document.getElementById('g-lobe').textContent)">复制</button>
    </div>
  </section>
</main>

<div id="drawer"><div class="row"><h3 id="dr-title">请求详情</h3><span style="flex:1"></span><button class="ghost sm" onclick="closeDrawer()">关闭</button></div><div id="dr-body"></div></div>
<div id="toast"></div>
<script>
// ---------- 基础工具 ----------
const $ = (id) => document.getElementById(id);
// 可选 API Key（配置了 api_keys 时，面板请求需带 Authorization）
function apiKey() { try { return localStorage.getItem('freebuff_api_key') || ''; } catch (e) { return ''; } }
function setApiKey(v) {
  try {
    localStorage.setItem('freebuff_api_key', v.trim());
    toast(v.trim() ? 'API Key 已保存，正在重新加载…' : 'API Key 已清除，正在重新加载…');
    authWarned = false;
    loadGuide();
    refreshOverview();
  } catch (e) {}
}
async function api(url, opt) {
  opt = opt || {};
  opt.headers = Object.assign({}, opt.headers || {});
  const k = apiKey();
  if (k) opt.headers['authorization'] = 'Bearer ' + k;
  const r = await fetch(url, opt);
  const text = await r.text();
  if (!r.ok) { let m = text; try { m = JSON.parse(text).message || JSON.parse(text).error?.message || text; } catch (e) {} throw new Error(m); }
  try { return JSON.parse(text); } catch (e) { return text; }
}
function toast(msg, ms) { const t = $('toast'); t.textContent = msg; t.style.display = 'block'; clearTimeout(t._h); t._h = setTimeout(() => t.style.display = 'none', ms || 2600); }
function esc(s) { return String(s == null ? '' : s).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c])); }
function badge(text, cls) { return `<span class="badge ${cls || 'dim'}">${esc(text)}</span>`; }
function copyText(t) { navigator.clipboard?.writeText(t).then(() => toast('已复制')).catch(() => toast('复制失败，请手动选择')); }
function fmtTime(ts) { try { return new Date(ts).toLocaleTimeString('zh-CN', { hour12: false }); } catch (e) { return ts || '—'; } }

// ---------- Tab ----------
function showTab(name) {
  document.querySelectorAll('nav button').forEach(b => b.classList.toggle('active', b.dataset.tab === name));
  for (const t of ['overview','account','skills','memory','logs','teach','doctor','guide']) {
    const el = $('tab-' + t); if (el) el.style.display = (t === name) ? '' : 'none';
  }
  if (name === 'skills') refreshSkills();
  if (name === 'memory') refreshMemory();
  if (name === 'doctor') refreshDoctor();
  if (name === 'account') { pingExtension(); refreshTokens(); loadHistory(); }
  if (name === 'overview') { loadBalance(); loadGuide(); }
  if (name === 'logs') initLogs();
  if (name === 'guide') loadGuide();
}

// ---------- 接入信息（地址 / Key / 客户端配置） ----------
let guideCache = null;
/**
 * 拉取 /api/guide 并渲染「立刻开始请求」卡 + 接入指南。
 * 地址与 Key 都由服务端给出，避免前端硬编码与实际监听地址不一致。
 */
async function loadGuide() {
  let g = null;
  try { g = await api('/api/guide'); } catch (e) { g = null; }
  // 区分两种"拿不到"：接口明确回了未配置 vs 请求本身失败（后者最常见的原因是已启用 Key 但本地没存）
  const unknown = !g || !g.ok;
  if (unknown) {
    g = { listen_addr: location.host, openai_base_url: '/v1', anthropic_base_url: '/', api_keys: null, api_key_hint: '', models_count: null, models_sample: [], data_plane_ready: null };
  }
  guideCache = g;
  const base = location.origin;
  if ($('c-base')) {
    $('c-base').value = base;
    $('c-openai').value = base + '/v1';
    $('c-anthropic').value = base;
    const configured = !!(g.api_keys && g.api_keys.configured);
    const local = apiKey();
    $('c-key').value = unknown
      ? (local || '（无法读取接入信息 —— 请在页面右上角填入 API Key）')
      : (configured
        ? (local || '（已启用校验 — 点「生成并启用 Key」或把已有 Key 粘到右上角输入框）')
        : 'sk-local');
    $('c-key-hint').innerHTML = unknown
      ? `⚠️ 读取接入信息失败${local ? '（本地已存 Key，若仍失败说明 Key 不正确）' : ''} —— 若你在 config.json 配置了 api_keys，请把它粘到页面右上角的输入框。`
      : (configured
        ? `🔒 已启用 API Key 校验（${g.api_keys.count} 个：${(g.api_keys.masked || []).map(esc).join('、')}）。客户端必须填对 Key。`
        : `🔓 未配置 API Key —— 仅本机可访问，客户端随便填一个非空字符串（如 <code>sk-local</code>）即可。`);
    $('c-model-hint').innerHTML = unknown
      ? '模型列表与可用数量需要鉴权后才能读取。'
      : `可用模型 <b>${g.models_count}</b> 个${(g.models_sample || []).length ? '，例如 ' + g.models_sample.slice(0, 3).map(esc).join('、') + ' …' : ''}（完整列表：<code>${esc(base)}/v1/models</code>）`;
    $('connect-ready').innerHTML = g.data_plane_ready === true
      ? '<span class="tok">✅ 凭证已就绪，可以开始请求</span>'
      : g.data_plane_ready === false
        ? '<span class="twarn">⚠️ 还没有凭证 —— 先去「账号」页一键登录</span>'
        : '<span style="color:var(--muted)">状态未知（需要鉴权）</span>';
  }
  fillGuide(g);
}
async function genApiKey() {
  if (!confirm('生成新的 API Key 并立即生效？\n\n生成后你的客户端需要填这个新 Key（本面板会自动记住）。')) return;
  try {
    const r = await api('/api/config/api-key', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ action: 'generate' }) });
    if (r.key) { try { localStorage.setItem('freebuff_api_key', r.key); } catch (e) {} }
    toast(r.message || '已生成', 6000);
    loadGuide();
  } catch (e) { toast('生成失败：' + e.message, 7000); }
}
async function clearApiKey() {
  if (!confirm('清除 API Key？\n\n清除后网关变回「仅本机可访问、Key 随便填」模式。')) return;
  try {
    const r = await api('/api/config/api-key', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ action: 'clear' }) });
    try { localStorage.removeItem('freebuff_api_key'); } catch (e) {}
    toast(r.message || '已清除', 6000);
    loadGuide();
  } catch (e) { toast('清除失败：' + e.message, 7000); }
}
function fillGuide(g) {
  const base = location.origin;
  const key = (g && g.api_keys && g.api_keys.configured) ? (apiKey() || '<你在面板生成的 Key>') : 'sk-local';
  const model = (g && (g.models_sample || [])[0]) || 'z-ai/glm-5.3-flash';
  const gb = $('guide-base'); if (gb) gb.textContent = base;
  const gk = $('guide-key-hint');
  if (gk) gk.innerHTML = (g && g.api_keys && g.api_keys.configured)
    ? '已在面板生成并启用了 API Key —— 客户端必须填这个 Key（本页面右上角已自动记住）'
    : 'config.json 未配置 api_keys 时，任意字符串即可（如 <code>sk-local</code>）';
  $('g-claude').textContent = `# macOS / Linux\nexport ANTHROPIC_BASE_URL=${base}\nexport ANTHROPIC_API_KEY=${key}\n\n# Windows PowerShell\n$env:ANTHROPIC_BASE_URL="${base}"\n$env:ANTHROPIC_API_KEY="${key}"\n\n# 然后正常启动 Claude Code 即可（claude 命令）`;
  $('g-openai').textContent = `Base URL: ${base}/v1\nAPI Key:  ${key}\n模型:     在 ${base}/v1/models 中选一个（共 ${(g && g.models_count) || '?'} 个）`;
  $('g-py').textContent = `from openai import OpenAI\nclient = OpenAI(base_url="${base}/v1", api_key="${key}")\nresp = client.chat.completions.create(model="${model}", messages=[{"role":"user","content":"你好"}])\nprint(resp.choices[0].message.content)`;
  $('g-node').textContent = `import OpenAI from "openai";\nconst client = new OpenAI({ baseURL: "${base}/v1", apiKey: "${key}" });\nconst r = await client.chat.completions.create({ model: "${model}", messages: [{ role: "user", content: "你好" }] });\nconsole.log(r.choices[0].message.content);`;
  $('g-curl').textContent = `curl ${base}/v1/chat/completions \\\n  -H "content-type: application/json" \\\n  -H "authorization: Bearer ${key}" \\\n  -d "{\\"model\\":\\"${model}\\",\\"messages\\":[{\\"role\\":\\"user\\",\\"content\\":\\"说句话证明通啦\\"}]}"`;
  $('g-lobe').textContent = `接口地址: ${base}/v1\nAPI Key:  ${key}\n模型名:   手动填 /v1/models 列表中的值（如 ${model}）`;
}

// ---------- 总览 ----------
let lastHealth = null;
let authWarned = false;      // 鉴权失败只提示一次，不狂闪
let overviewTimer = null;    // 总览自动刷新句柄（鉴权失败时暂停，填 Key 后恢复）
async function refreshOverview() {
  try {
    const health = await api('/healthz');
    lastHealth = health;
    $('dot').className = 'dot ok';
    $('ver').textContent = 'v' + (health.version || '');
    const accs = health.accounts || [];
    const alive = accs.filter(a => a.healthy).length;
    $('hstat').innerHTML = `账号 <b>${alive}/${accs.length}</b> · 运行 <b>${Math.floor((health.uptime_sec || 0) / 60)}m</b>`;
    const totals = await api('/api/usage/totals');
    const cards = [
      ['总请求', totals.total_requests ?? 0],
      ['总 Token', (totals.total_tokens ?? 0).toLocaleString()],
      ['错误', totals.errors ?? 0],
      ['账号', `${alive}/${accs.length}`],
    ];
    $('cards').innerHTML = cards.map(([l, n]) => `<div class="card"><div class="lbl">${l}</div><div class="num">${n}</div></div>`).join('');
    // 速率与错误率（免费层无货币成本，诚实标注来源）
    try {
      const cost = await api('/api/usage/cost');
      $('cost-line').innerHTML = `近 ${cost.window_minutes} 分钟：<b>${cost.requests_30m}</b> 请求 · 错误率 <b>${((cost.error_rate_30m || 0) * 100).toFixed(0)}%</b> · 平均延迟 <b>${((cost.avg_latency_ms_30m || 0) / 1000).toFixed(1)}s</b> · 约 <b>${cost.requests_per_hour}</b> 请求/小时 <span style="opacity:.7">（${esc(cost.cost_source || '')}）</span>`;
    } catch (e) { $('cost-line').textContent = ''; }
    // 无账号引导
    if (accs.length === 0) {
      $('banner').innerHTML = `<div class="banner"><h2>👋 三步开始使用</h2><ol>
        <li><b>添加账号</b>：切到「账号」页粘贴 Cookie，或桌面版托盘「一键登录」</li>
        <li><b>接入客户端</b>：切到「接入指南」页，复制配置到 Claude Code / Cursor 等</li>
        <li><b>开始对话</b>：回来这里就能看到请求、Token、日志与体检</li></ol></div>`;
    } else { $('banner').innerHTML = ''; }
    // 账号表
    $('acc-wrap').innerHTML = accs.length ? `<table><thead><tr><th>账号</th><th>状态</th><th>评分</th><th>会话</th><th>错误</th></tr></thead><tbody>${
      accs.map(a => {
        const st = a.session?.status || 'unknown';
        const cls = (st === 'active' || a.healthy) ? 'ok' : (st === 'queued' ? 'warn' : 'err');
        return `<tr><td>${esc(a.name)}</td><td>${badge(st, cls)}</td><td>${Math.round(a.score ?? 0)}</td><td>${a.session?.instance_id ? esc(String(a.session.instance_id).slice(0, 8)) + '…' : '—'}</td><td>${esc(a.last_error || a.session?.last_error || '')}</td></tr>`;
      }).join('')}</tbody></table>` : '<div class="empty">还没有账号 — 去「账号」页添加</div>';
    // 用量
    const daily = await api('/api/usage/daily');
    $('daily-wrap').innerHTML = (daily && daily.length) ? `<table><thead><tr><th>日期</th><th>模型</th><th>请求</th><th>输入tok</th><th>输出tok</th><th>错误</th></tr></thead><tbody>${
      daily.slice(0, 30).map(d => `<tr><td>${esc(d.date)}</td><td>${esc(d.model)}</td><td>${d.requests}</td><td>${d.prompt_tokens}</td><td>${d.completion_tokens}</td><td>${d.errors}</td></tr>`).join('')}</tbody></table>` : '<div class="empty">暂无用量 — 发起一次对话后这里会有数据</div>';
    // 请求
    const reqs = await api('/api/usage/requests');
    $('reqs-wrap').innerHTML = (reqs && reqs.length) ? `<table><thead><tr><th>时间</th><th>账号</th><th>模型</th><th>状态</th><th>延迟</th><th>Tokens</th></tr></thead><tbody>${
      reqs.slice(0, 20).map(r => {
        const cls = r.status < 400 ? 'ok' : (r.status < 500 ? 'warn' : 'err');
        const tk = (r.prompt_tokens || 0) + (r.completion_tokens || 0);
        return `<tr class="click" onclick="openDrawer(${r.id})"><td>${fmtTime(r.ts)}</td><td>${esc(r.account)}</td><td>${esc(r.model)}</td><td>${badge(r.status, cls)}</td><td>${(r.latency_ms / 1000).toFixed(2)}s</td><td>${tk || '—'}</td></tr>`;
      }).join('')}</tbody></table>` : '<div class="empty">暂无请求记录</div>';
    // 模型
    const models = await api('/api/usage/models');
    $('model-count').textContent = (models || []).length;
    $('models-wrap').innerHTML = (models || []).map(m => `<span class="chip">${esc(m)}</span>`).join('') || '<div class="empty">模型列表为空（检查上游连通性）</div>';
    if (authWarned) { authWarned = false; $('banner').innerHTML = ''; }
    if (!overviewTimer) startOverviewTimer();
  } catch (e) {
    $('dot').className = 'dot err';
    const m = String(e.message || '');
    // 网关启用了 API Key 而浏览器还没填：给一次性引导，并暂停自动刷新（否则红灯 + toast 每 6 秒狂闪）
    if (/unauthorized|api key|鉴权|认证/i.test(m)) {
      if (!authWarned) {
        authWarned = true;
        toast('该网关已启用 API Key 校验 —— 请在页面右上角输入框填入 Key', 8000);
        $('banner').innerHTML = `<div class="banner"><h2>🔑 需要鉴权</h2>
          <p style="font-size:13px;color:var(--muted);margin-top:6px">网关配置了 <code>api_keys</code>。在<b>页面右上角的输入框</b>填入 Key（只存本机浏览器），填完这里会自动恢复。</p></div>`;
        if (overviewTimer) { clearInterval(overviewTimer); overviewTimer = null; }
      }
    } else {
      toast('加载失败: ' + m);
    }
  }
}
function startOverviewTimer() {
  if (overviewTimer) clearInterval(overviewTimer);
  overviewTimer = setInterval(() => { if ($('tab-overview').style.display !== 'none') refreshOverview(); }, 6000);
}

async function loadBalance() {
  try {
    const r = await fetch('/api/account/balance', { headers: apiKey() ? { authorization: 'Bearer ' + apiKey() } : {} });
    if (!r.ok) {
      $('balance-panel').style.display = '';
      $('balance-wrap').innerHTML = '<div class="empty">需要 web Cookie 凭证才能查询积分（先在「添加账号」导入 Cookie）</div>';
      return;
    }
    const bal = await r.json();
    if (bal && bal.ok !== false) {
      $('balance-panel').style.display = '';
      const d = bal.freebucks?.daily || {};
      let html = `<p style="font-size:13px;color:var(--muted)">套餐 <b>${esc(bal.subscription?.tierId || '免费')}</b> · 层级 ${esc(bal.access_tier || '—')}${bal.country_block_reason ? ' · ⚠️ 地区受限(' + esc(bal.country_block_reason) + ')' : ''}</p>`;
      if (d.limit != null) html += `<p style="font-size:13px;color:var(--muted)">今日积分 <b class="tok">${d.remaining ?? '—'}</b> / ${d.limit ?? '—'}（已用 ${d.spent ?? 0}）</p>`;
      const mr = bal.model_remaining || {};
      const rows = Object.entries(mr).slice(0, 30).map(([m, v]) => `<tr><td>${esc(m)}</td><td>${v.price === 0 ? '<b class="tok">免费</b>' : v.price}</td><td>${v.usable_today === -1 ? '不限' : v.usable_today}</td></tr>`).join('');
      if (rows) html += `<table style="margin-top:8px"><thead><tr><th>模型</th><th>积分价</th><th>今日剩余</th></tr></thead><tbody>${rows}</tbody></table>`;
      $('balance-wrap').innerHTML = html;
    } else {
      $('balance-panel').style.display = '';
      $('balance-wrap').innerHTML = '<div class="empty">需要 web Cookie 凭证才能查询积分（先在「添加账号」导入 Cookie）</div>';
    }
  } catch (e) { /* 无 Cookie 时静默 */ }
}

// ---------- 请求详情 ----------
async function openDrawer(id) {
  $('drawer').classList.add('open');
  $('dr-title').textContent = '请求 #' + id;
  $('dr-body').innerHTML = '<div class="empty">加载中…</div>';
  try {
    const d = await api('/api/usage/requests/' + id);
    const r = d.request || {};
    const events = d.events || [];
    let html = '';
    html += `<div class="kv"><b>时间</b>${esc(r.ts)}</div>`;
    html += `<div class="kv"><b>端点</b>${esc(r.endpoint || '—')}</div>`;
    html += `<div class="kv"><b>账号</b>${esc(r.account)}</div>`;
    html += `<div class="kv"><b>请求模型</b>${esc(r.requested_model || r.model)}</div>`;
    html += `<div class="kv"><b>实际模型</b>${esc(r.resolved_model || r.model)}</div>`;
    html += `<div class="kv"><b>状态</b>${r.status} ${esc(r.error_kind ? '(' + r.error_kind + ')' : '')}</div>`;
    html += `<div class="kv"><b>延迟</b>${(r.latency_ms / 1000).toFixed(2)}s${r.ttft_ms ? '（首字节 ' + r.ttft_ms + 'ms）' : ''}</div>`;
    html += `<div class="kv"><b>Tokens</b>输入 ${r.prompt_tokens || 0} / 输出 ${r.completion_tokens || 0}</div>`;
    if (r.route_reason) html += `<div class="kv"><b>路由原因</b>${esc(r.route_reason)}</div>`;
    if (r.error_excerpt) html += `<details open><summary>错误详情</summary><pre>${esc(r.error_excerpt)}</pre></details>`;
    html += `<div class="kv" style="margin-top:10px"><b>人话解释</b>${esc(explain(r))}</div>`;
    if (events.length) html += `<details open><summary>事件链（${events.length}）</summary>${events.map(e => `<div class="kv" style="font-size:12px"><b>${fmtTime(e.ts)} ${esc(e.kind)}</b>${esc(e.detail)}</div>`).join('')}</details>`;
    $('dr-body').innerHTML = html;
  } catch (e) { $('dr-body').innerHTML = `<div class="empty">加载失败：${esc(e.message)}</div>`; }
}
function closeDrawer() { $('drawer').classList.remove('open'); }
function explain(r) {
  const k = r.error_kind || '';
  if (k === 'waiting_room') return '上游免费队列排队中 —— 这不是网关故障，稍等重试即可。';
  if (k === 'no_account') return '没有可用账号 —— 去「账号」页添加。';
  if (k === 'upstream_4xx') return '上游拒绝了这次请求（多为账号凭证过期或模型不可用）。查看错误详情。';
  if (k === 'upstream_5xx') return '上游服务端错误 —— 通常重试即可，网关会自动切换账号。';
  if (k === 'network' || k === 'timeout') return '网络超时/中断 —— 检查代理设置与上游连通性（体检页可测）。';
  if (r.status >= 200 && r.status < 300) return '请求成功。' + (r.ttft_ms ? `首字节 ${r.ttft_ms}ms，` : '') + `总耗时 ${(r.latency_ms / 1000).toFixed(2)}s。`;
  return '暂无解释数据。';
}

// ---------- 余额卡 ----------
// ---------- 浏览器扩展桥（面板 ↔ 扩展 直连） ----------
// 扩展的 bridge.js content script 会 postMessage 广播自己的 id；
// 拿到 id 后本页就能用 chrome.runtime.sendMessage 直接指挥扩展读 Cookie（真正的一键登录）。
let extId = null, extVersion = '';
function pingExtension(showToast) {
  try { window.postMessage({ source: 'freebuff2api-page', type: 'ping' }, location.origin); } catch (e) {}
  if (showToast) setTimeout(() => {
    toast(extId ? `扩展已就绪（v${extVersion || '?'}）` : '未检测到扩展 —— 可点「⬇ 下载扩展」安装，或用手动向导', 5000);
  }, 600);
}
window.addEventListener('message', (e) => {
  if (e.source !== window) return;
  const d = e.data;
  if (!d || d.source !== 'freebuff2api-extension' || typeof d.id !== 'string') return;
  const isNew = extId !== d.id;
  extId = d.id; extVersion = d.version || '';
  renderExtStatus();
  if (isNew && $('tab-account') && $('tab-account').style.display !== 'none') { /* 首次进入时静默 */ }
});
function renderExtStatus() {
  const el = $('ext-status');
  if (!el) return;
  if (extId) { el.className = 'badge ok'; el.textContent = `扩展已就绪 v${extVersion || '?'}`; }
  else { el.className = 'badge dim'; el.textContent = '未检测到扩展（可手动粘贴导入）'; }
}
function extensionAvailable() {
  return !!extId && typeof chrome !== 'undefined' && chrome.runtime && typeof chrome.runtime.sendMessage === 'function';
}
function sendToExtension(msg) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('扩展未在规定时间内响应（可能被浏览器回收，请刷新页面重试）')), 20000);
    try {
      chrome.runtime.sendMessage(extId, msg, (resp) => {
        clearTimeout(timer);
        if (chrome.runtime.lastError) { reject(new Error(chrome.runtime.lastError.message)); return; }
        resolve(resp);
      });
    } catch (e) { clearTimeout(timer); reject(e); }
  });
}
function downloadExtension() {
  const k = apiKey();
  window.open('/api/extension/bundle' + (k ? '?key=' + encodeURIComponent(k) : ''), '_blank');
}

// ---------- 账号 / 导入 ----------
// ---------- 内嵌 WebView2 登录窗口（网关派生 --login-window 子进程） ----------
/**
 * 弹出内嵌登录窗口：后端 spawn `当前exe --login-window`（独立进程，tao+WebView2 事件循环），
 * 用户在窗口里完成 GitHub 登录后由后端自动抓 Cookie（含 HttpOnly）并入库。
 * 结果通过面板轮询 /api/tokens 感知（凭证数量增加即成功）。
 */
async function openEmbedLogin() {
  const st = $('embed-status');
  if (st) st.textContent = '正在弹出窗口…';
  try {
    const r = await api('/api/login/embed', { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' });
    if (!r.ok) {
      if (st) st.innerHTML = '<span class="terr">' + esc(r.message || '当前环境不支持') + '</span>';
      toast('内嵌窗口不可用：' + (r.message || '请改用方案 B/C'), 8000);
      return;
    }
    toast('内嵌登录窗口已弹出 —— 在窗口里完成 GitHub 登录即可自动入库', 8000);
    if (st) st.innerHTML = '<span class="tok">窗口已弹出，等待登录…</span>';
    const before = await tokenCount();
    // 双通道感知：凭证数量增加（成功）或结果文件报告失败（子进程退出码非 0）
    const deadline = Date.now() + 600000;
    while (Date.now() < deadline) {
      await new Promise(res => setTimeout(res, 3000));
      const now = await tokenCount();
      if (before >= 0 && now > before) {
        toast('✅ 内嵌窗口登录成功，凭证已自动入库', 6000);
        if (st) st.innerHTML = '<span class="tok">✅ 已入库</span>';
        refreshTokens(); refreshAccountOverview(); loadHistory();
        return;
      }
      // 失败结果文件（子进程异常退出时后端落盘）
      try {
        const fr = await api('/api/login/result');
        if (fr && fr.ok === false && fr.message) {
          if (st) st.innerHTML = '<span class="terr">' + esc(fr.message) + '</span>';
          toast('内嵌登录窗口退出：' + fr.message, 9000);
          return;
        }
      } catch (e2) { /* 结果文件接口失败不阻塞主轮询 */ }
    }
    if (st) st.innerHTML = '<span class="twarn">等待超时（10 分钟）</span>';
    toast('等待登录超时 —— 请重试或改用方案 B/C', 8000);
  } catch (e) {
    if (st) st.innerHTML = '<span class="terr">' + esc(e.message) + '</span>';
    toast('弹出失败：' + e.message + ' —— 请改用方案 B/C', 8000);
  }
}

async function oneClickLogin() {
  // [主控接线] 内嵌窗口分派将插入此处（内嵌 WebView2 登录窗口优先，由主控实现）
  // 路径 1：桌面版 Electron（主进程可直接读 HttpOnly Cookie）
  if (window.freebuffDesktop && window.freebuffDesktop.openLogin) {
    window.freebuffDesktop.openLogin();
    toast('已打开登录窗口，登录 freebuff.com 后 Cookie 会自动入库');
    return;
  }
  // 路径 2：浏览器扩展直连 —— 全自动（自动打开 freebuff.com → 等待登录 → 自动入库）
  if (extensionAvailable()) {
    await oneClickViaExtension();
    return;
  }
  // 路径 3：降级为手动向导（高亮"剪贴板自动检测"——步骤最少的手动路径）
  const wiz = $('login-wizard');
  wiz.style.display = '';
  wiz.scrollIntoView({ behavior: 'smooth', block: 'start' });
  const clipCard = $('wizard-clip');
  if (clipCard) {
    clipCard.classList.remove('wizard-flash');
    // 强制 reflow 让动画可以重复触发
    void clipCard.offsetWidth;
    clipCard.classList.add('wizard-flash');
  }
  window.open('https://freebuff.com/', '_blank', 'noopener');
  toast('未检测到浏览器扩展 —— 已打开 freebuff.com，推荐用向导「方案 B：自动检测剪贴板」（复制回来点一下即可）', 9000);
}
/**
 * 剪贴板自动导入：读取剪贴板文本 → 填入输入框 → 自动触发导入。
 * 剪贴板 API 需要安全上下文且部分浏览器要求页面先获得焦点，失败时明确提示改用手动粘贴（降级路径始终存在）。
 */
async function importFromClipboard() {
  const st = $('clip-status');
  if (st) st.textContent = '正在读取剪贴板…（若浏览器弹窗询问权限，请允许）';
  let text = '';
  try {
    if (!navigator.clipboard || typeof navigator.clipboard.readText !== 'function') {
      throw new Error('当前浏览器不支持读取剪贴板（或页面不在 HTTPS/localhost 安全上下文）');
    }
    text = (await navigator.clipboard.readText()).trim();
  } catch (e) {
    if (st) st.innerHTML = `<span class="twarn">读取剪贴板失败：${esc(e.message)}</span> —— 请改用下方输入框手动粘贴（Ctrl+V），效果完全一样`;
    toast('无法读取剪贴板 —— 请手动粘贴到输入框后点「导入」', 7000);
    $('import-text').focus();
    return;
  }
  if (!text) {
    if (st) st.innerHTML = '<span class="twarn">剪贴板是空的 —— 请先去 freebuff.com 的 DevTools 里复制 Cookie 整行（Ctrl+C）</span>';
    return;
  }
  if (st) st.textContent = '已从剪贴板取到内容，开始自动导入…';
  $('import-text').value = text;
  toast('已从剪贴板读取内容，正在自动导入…', 4000);
  await doImport('clipboard');
}
/**
 * 导入成功后的即贴即验：找出最新入库的凭证 → 调上游检查端点 → 把结果直接显示在向导里。
 * id 匹配不到时静默跳过（列表刷新已由 doImport 完成，不阻塞主流程）。
 */
async function verifyNewCredential() {
  const box = $('import-verify');
  if (!box) return;
  box.style.display = '';
  box.innerHTML = '<span style="color:var(--muted)">⏳ 正在向上游验证刚导入的凭证…（需要几秒）</span>';
  try {
    const r = await api('/api/tokens');
    const list = r.tokens || [];
    // 列表接口不保证排序，比较入库时间取最新一条
    const newest = list.slice().sort((a, b) => String(b.added_at || '').localeCompare(String(a.added_at || '')))[0];
    if (!newest || !newest.id) { box.style.display = 'none'; return; }
    const c = await api('/api/tokens/check', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ id: newest.id }) });
    if (c.ok && c.valid) {
      const m = c.meta || {};
      const who = m.email || m.name || '账号信息已更新';
      box.innerHTML = `<span class="tok">✅ 已验证：${esc(who)}</span> <span style="color:var(--muted)">（${esc([m.name, m.email].filter(Boolean).join(' · ') || '凭证有效')}）</span>`;
    } else {
      box.innerHTML = `<span class="twarn">⚠️ Cookie 已录入但上游校验失败（可能未登录或已过期），请重新复制</span>${c.message ? `<div style="font-size:12px;color:var(--muted);margin-top:4px">上游说：${esc(c.message)}</div>` : ''}`;
    }
  } catch (e) {
    box.innerHTML = `<span class="twarn">⚠️ 自动验证失败：${esc(e.message)}</span> <span style="color:var(--muted)">—— 凭证已入库，可稍后在凭证列表点「检查」手动验证</span>`;
  }
}
async function oneClickViaExtension() {
  toast('正在通知扩展…', 3000);
  const before = await tokenCount();
  let resp = null;
  try {
    resp = await sendToExtension({ type: 'freebuff2api.import', gatewayPort: Number(location.port || 47821), apiKey: apiKey() });
  } catch (e) {
    toast('调用扩展失败：' + e.message, 7000);
    return;
  }
  if (resp && resp.ok === false) { toast('扩展返回：' + (resp.message || '导入失败'), 8000); return; }
  if (resp && resp.done) {
    // 扩展已同步完成（已登录且已导入 / 或已存在去重）—— 无需轮询
    if (resp.added > 0) {
      toast(`✅ 扩展已导入 ${resp.added} 个凭证`, 6000);
      refreshTokens(); refreshAccountOverview(); loadHistory();
    } else {
      toast('该账号凭证已在库中（同值自动去重，无需重复导入）', 7000);
      refreshTokens();
    }
    return;
  }
  if (resp && resp.needLogin) {
    toast('已自动打开 freebuff.com —— 完成 GitHub 登录后凭证会自动入库（最多等 3 分钟）', 9000);
  } else {
    toast('扩展已开始导入，正在等待凭证入库…', 6000);
  }
  pollForNewCredential(before, 180000);
}
async function tokenCount() {
  try { const r = await api('/api/tokens'); return (r.tokens || []).length; } catch (e) { return -1; }
}
/** 轮询等待扩展把凭证写进网关（扩展是独立进程，只能靠轮询收敛） */
async function pollForNewCredential(before, timeoutMs) {
  // 基线没取到（before<0）时先补取一次，避免"任何请求成功都误报为入库"
  if (before < 0) before = await tokenCount();
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    await new Promise(r => setTimeout(r, 3000));
    const now = await tokenCount();
    if (before >= 0 && now > before) {
      toast('✅ 凭证已自动入库，正在拉取账号信息…', 6000);
      refreshTokens(); refreshAccountOverview(); loadHistory();
      return;
    }
  }
  toast('等待登录超时 —— 完成登录后点扩展图标，或回到本页点「刷新」', 8000);
}
async function doImport(source) {
  const text = $('import-text').value.trim();
  if (!text) { toast('请先粘贴内容'); return; }
  $('import-result').textContent = '导入中…';
  const verifyBox = $('import-verify');
  if (verifyBox) { verifyBox.style.display = 'none'; verifyBox.innerHTML = ''; }
  try {
    const r = await api('/api/tokens/import', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ cookie: text }) });
    if (r.added > 0) {
      $('import-result').innerHTML = `<span class="tok">✅ 成功导入 ${r.added} 个凭证${source === 'clipboard' ? '（来源：剪贴板）' : ''}</span>`;
      $('import-text').value = '';
      $('login-wizard').style.display = 'none';
      toast(source === 'clipboard' ? '✅ 剪贴板内容导入成功，正在验证凭证…' : '导入成功，正在拉取账号信息…');
      refreshTokens();
      refreshAccountOverview();
      loadHistory();
      // 即贴即验：导入成功后立刻向上游验证新凭证并把结果显示在向导附近
      verifyNewCredential();
    } else {
      $('import-result').innerHTML = `<span class="twarn">该凭证已存在（同值自动去重，未重复入库）</span>`;
      refreshTokens();
    }
  } catch (e) { $('import-result').innerHTML = `<span class="terr">导入失败：${esc(e.message)}</span>`; }
}

// ---------- 凭证列表（账号详细信息 / 入库时间 / 检查 / 删除） ----------
let tokenCache = [];
async function refreshTokens() {
  try {
    const r = await api('/api/tokens');
    const list = r.tokens || [];
    tokenCache = list;
    $('cred-count').textContent = list.length ? `（${list.length} 个 · 同值自动去重）` : '';
    fillHistoryCredOptions(list);
    $('tokens-wrap').innerHTML = list.length ? `<table><thead><tr>
        <th>账号</th><th>类型</th><th>凭证</th><th>套餐</th><th>今日剩余</th><th>入库时间</th><th>操作</th>
      </tr></thead><tbody>${
      list.map((t, i) => {
        const m = t.meta || null;
        const acct = m
          ? `<div style="font-weight:600">${esc(m.name || '—')}</div><div style="font-size:12px;color:var(--muted)">${esc(m.email || '')}</div>`
          : `<span style="color:var(--muted)">未检查</span>`;
        const validBadge = m ? (m.valid ? badge('有效', 'ok') : badge('可能失效', 'err')) : '';
        const tier = m ? (m.tier_id ? badge(m.tier_id, 'ok') : badge('免费', 'dim')) : '—';
        const remain = (m && m.daily_remaining != null) ? `<b>${m.daily_remaining}</b> / ${m.daily_limit ?? '—'}` : '—';
        const added = t.added_at ? new Date(t.added_at).toLocaleString('zh-CN', { hour12: false }) : '—';
        return `<tr>
          <td>${acct} ${validBadge}</td>
          <td>${t.kind === 'web-cookie' ? badge('Web Cookie', 'ok') : badge('Bearer', 'dim')}</td>
          <td><code>${esc(t.token_masked)}</code><div style="font-size:11px;color:var(--muted)">${esc(t.source || '')}${t.host ? ' · ' + esc(t.host) : ''}</div></td>
          <td>${tier}</td>
          <td style="font-size:12px">${remain}</td>
          <td style="font-size:12px">${esc(added)}</td>
          <td class="row">
            <button class="ghost sm" onclick="checkCredAt(${i})">检查</button>
            <button class="ghost sm" onclick="openCredDetail(${i})">详情</button>
            <button class="ghost sm" onclick="deleteCredAt(${i})">删除</button>
          </td></tr>`;
      }).join('')}</tbody></table>` : '<div class="empty">还没有导入凭证 — 用上方「一键登录」或粘贴导入</div>';
  } catch (e) { $('tokens-wrap').innerHTML = `<div class="empty">加载失败：${esc(e.message)}</div>`; }
}
function checkCredAt(i) { const t = tokenCache[i]; if (t) checkCred(t.id); }
function deleteCredAt(i) { const t = tokenCache[i]; if (t) deleteCred(t.id, (t.meta && t.meta.email) || t.token_masked); }
function openCredDetail(i) { const t = tokenCache[i]; if (t) renderCredDetail(t); }

async function checkCred(id) {
  toast('正在检查该凭证…（需要几秒）');
  try {
    const r = await api('/api/tokens/check', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ id }) });
    toast(r.message || (r.ok ? '凭证有效' : '凭证可能已失效'), 7000);
    refreshTokens();
    loadHistory();
  } catch (e) { toast('检查失败：' + e.message, 7000); }
}
async function deleteCred(id, label) {
  if (!confirm(`确定删除凭证 ${label || ''}？\n\n删除后会同时从运行中的账号池移除，需要重新登录导入才能恢复。`)) return;
  try {
    const r = await api('/api/tokens/delete', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ id }) });
    toast(r.message || '已删除', 5000);
    refreshTokens();
    loadHistory();
  } catch (e) { toast('删除失败：' + e.message, 7000); }
}
function renderCredDetail(t) {
  const m = t.meta;
  $('drawer').classList.add('open');
  $('dr-title').textContent = '凭证详情';
  let html = '';
  html += `<div class="kv"><b>凭证</b><code>${esc(t.token_masked)}</code></div>`;
  html += `<div class="kv"><b>类型</b>${t.kind === 'web-cookie' ? 'Web Cookie（网页版登录）' : 'Bearer Token'}</div>`;
  html += `<div class="kv"><b>来源</b>${esc(t.source || '—')}${t.host ? ' · ' + esc(t.host) : ''}</div>`;
  html += `<div class="kv"><b>入库时间</b>${t.added_at ? new Date(t.added_at).toLocaleString('zh-CN', { hour12: false }) : '未知（旧数据）'}</div>`;
  if (!m) {
    html += `<div class="empty" style="margin-top:12px">这条凭证还没检查过 —— 点凭证列表里的「检查」即可拉取账号详细信息。</div>`;
    $('dr-body').innerHTML = html;
    return;
  }
  html += `<div class="kv"><b>检查时间</b>${m.checked_at ? new Date(m.checked_at).toLocaleString('zh-CN', { hour12: false }) : '—'} ${m.valid ? badge('有效', 'ok') : badge('可能失效', 'err')}</div>`;
  html += '<div style="border-top:1px solid var(--border);margin:10px 0;padding-top:8px"><b>账号</b></div>';
  if (m.image) html += `<img src="${esc(m.image)}" style="width:40px;height:40px;border-radius:50%;vertical-align:middle" onerror="this.style.display='none'">`;
  html += `<div class="kv"><b>昵称</b>${esc(m.name || '—')}</div>`;
  html += `<div class="kv"><b>邮箱</b>${esc(m.email || '—')}</div>`;
  if (m.user_id) html += `<div class="kv"><b>用户 ID</b><code>${esc(m.user_id)}</code></div>`;
  if (m.expires) html += `<div class="kv"><b>登录有效期</b>${new Date(m.expires).toLocaleString('zh-CN', { hour12: false })}</div>`;
  html += '<div style="border-top:1px solid var(--border);margin:10px 0;padding-top:8px"><b>额度</b></div>';
  html += `<div class="kv"><b>层级 / 套餐</b>${esc(m.access_tier || '—')} / ${esc(m.tier_id || '免费')}</div>`;
  if (m.daily_limit != null) html += `<div class="kv"><b>今日积分</b>剩余 <b class="tok">${m.daily_remaining ?? '—'}</b> / ${m.daily_limit}（已用 ${m.daily_spent ?? 0}）</div>`;
  if (m.reset_at) html += `<div class="kv"><b>下次重置</b>${new Date(m.reset_at).toLocaleString('zh-CN', { hour12: false })}</div>`;
  if (m.streak_current != null) html += `<div class="kv"><b>连续使用</b>${m.streak_current} 天（累计活跃 ${m.all_time_active_days ?? '—'} 天）</div>`;
  if (m.tokens_7d != null) html += `<div class="kv"><b>近 7 天 token</b>${m.tokens_7d.toLocaleString()}</div>`;
  if (m.country_code) html += `<div class="kv"><b>地区</b>${esc(m.country_code)}${m.country_block_reason ? ' ' + badge(m.country_block_reason, 'err') : ''}</div>`;
  if (m.error) html += `<div class="kv"><b>错误</b><span class="terr">${esc(m.error)}</span></div>`;
  if ((m.models || []).length) {
    html += `<details open style="margin-top:8px"><summary>逐模型今日剩余（${m.models.length}）</summary><table style="margin-top:6px"><thead><tr><th>模型</th><th>剩余</th><th>限额</th><th>已用</th><th>积分价</th></tr></thead><tbody>${
      m.models.map(x => `<tr><td>${esc(x.model)}</td><td><b>${x.remaining ?? '—'}</b></td><td>${x.limit ?? '—'}</td><td>${x.used ?? 0}</td><td>${x.price === 0 ? '<b class="tok">免费</b>' : (x.price ?? '—')}</td></tr>`).join('')}</tbody></table></details>`;
  }
  $('dr-body').innerHTML = html;
}

// ---------- 使用记录 ----------
function fillHistoryCredOptions(list) {
  const sel = $('hist-cred');
  if (!sel) return;
  const cur = sel.value;
  const opts = ['<option value="">全部账号</option>'].concat(
    (list || []).map(t => {
      const m = t.meta || {};
      const label = m.email || m.name || t.token_masked;
      return `<option value="${esc(t.id)}">${esc(label)}</option>`;
    })
  );
  sel.innerHTML = opts.join('');
  sel.value = cur;
}
async function loadHistory() {
  const wrap = $('hist-wrap');
  if (!wrap) return;
  wrap.innerHTML = '<div class="empty">加载中…</div>';
  try {
    if (!tokenCache.length) { try { const r = await api('/api/tokens'); tokenCache = r.tokens || []; fillHistoryCredOptions(tokenCache); } catch (e) {} }
    const cred = $('hist-cred') ? $('hist-cred').value : '';
    const q = '/api/account/history?limit=100' + (cred ? '&cred_id=' + encodeURIComponent(cred) : '');
    const d = await api(q);
    const rows = d.records || [];
    wrap.innerHTML = rows.length ? `<table><thead><tr>
        <th>时间</th><th>账号</th><th>套餐</th><th>今日剩余</th><th>已用</th><th>近 7 天 token</th><th>连续天数</th><th>结果</th>
      </tr></thead><tbody>${
      rows.map(r => `<tr>
        <td style="font-size:12px">${new Date(r.ts).toLocaleString('zh-CN', { hour12: false })}</td>
        <td>${esc(r.email || r.name || r.cred_id.slice(0, 8))}</td>
        <td>${esc(r.tier_id || '免费')}</td>
        <td>${r.daily_remaining ?? '—'} / ${r.daily_limit ?? '—'}</td>
        <td>${r.daily_spent ?? '—'}</td>
        <td>${r.tokens_7d != null ? r.tokens_7d.toLocaleString() : '—'}</td>
        <td>${r.streak_current ?? '—'}</td>
        <td>${r.ok ? badge('成功', 'ok') : badge('失败', 'err')}</td>
      </tr>`).join('')}</tbody></table>` : '<div class="empty">暂无记录 — 点「刷新账号全貌」或凭证行的「检查」即可产生记录</div>';
  } catch (e) { wrap.innerHTML = `<div class="empty">加载失败：${esc(e.message)}</div>`; }
}

// ---------- 账号全貌（中文呈现上游数据） ----------
async function refreshAccountOverview() {
  $('overview-wrap').innerHTML = '<div class="empty">拉取中…（上游可能需要几秒）</div>';
  try {
    const d = await api('/api/account/overview');
    $('overview-wrap').innerHTML = renderOverview(d);
  } catch (e) {
    const m = String(e.message || '');
    $('overview-wrap').innerHTML = `<div class="empty">拉取失败：${esc(m)}${m.includes('Cookie') ? ' — 请先在上方导入凭证' : ''}</div>`;
  }
}
function renderOverview(d) {
  const id = (d.identity && d.identity.user) || {};
  let html = '';
  // 身份
  html += '<div class="row" style="gap:12px;margin-bottom:14px">';
  if (id.image) html += `<img src="${esc(id.image)}" style="width:44px;height:44px;border-radius:50%" onerror="this.style.display='none'">`;
  html += `<div><div style="font-size:16px;font-weight:600">${esc(id.name || '未命名账号')}</div>
    <div style="font-size:12px;color:var(--muted)">${esc(id.email || '')}${id.id ? ' · 用户 ID ' + esc(String(id.id).slice(0, 8)) + '…' : ''}</div></div>`;
  if (d.identity && d.identity.expires) html += `<span style="flex:1"></span><span style="font-size:12px;color:var(--muted)">凭证有效期至 ${new Date(d.identity.expires).toLocaleString('zh-CN', { hour12: false })}</span>`;
  html += '</div>';

  // 使用统计（中文）
  const u = d.usage || {};
  if (u.streak || u.recent) {
    html += '<div style="border-top:1px solid var(--border);padding-top:12px;margin-bottom:4px"><b style="font-size:14px">📊 使用统计</b></div>';
    if (u.streak) html += `<p style="font-size:13px;margin-top:6px">🔥 连续使用 <b class="tok">${u.streak.current ?? 0}</b> 天（最长 ${u.streak.longest ?? 0} 天） · 累计活跃 <b>${u.allTimeActiveDays ?? 0}</b> 天</p>`;
    const r = u.recent || {};
    if (r.totalTokens != null) html += `<p style="font-size:13px;color:var(--muted);margin-top:4px">近 ${r.days ?? 7} 天：<b>${r.messages ?? 0}</b> 条消息 · 输入 <b>${(r.inputTokens || 0).toLocaleString()}</b> · 输出 <b>${(r.outputTokens || 0).toLocaleString()}</b> · 缓存 <b>${(r.cacheReadTokens || 0).toLocaleString()}</b> · 合计 <b>${(r.totalTokens || 0).toLocaleString()}</b> tokens</p>`;
    if ((u.sessionsByModel || []).length) {
      html += `<table style="margin-top:8px"><thead><tr><th>模型</th><th>会话数</th><th>消耗单位</th></tr></thead><tbody>${u.sessionsByModel.map(m => `<tr><td>${esc(m.model)}</td><td>${m.sessions}</td><td>${m.units}</td></tr>`).join('')}</tbody></table>`;
    }
  }

  // 今日额度
  const q = d.quota || {};
  const fb = q.freebucks || {};
  const daily = fb.daily || {};
  const tierId = (d.subscription && d.subscription.subscription && d.subscription.subscription.tierId) || null;
  html += '<div style="border-top:1px solid var(--border);padding-top:12px;margin-top:12px"><b style="font-size:14px">💰 今日额度</b></div>';
  html += `<p style="font-size:13px;margin-top:6px">账号层级 <b>${esc(q.accessTier || '—')}</b> · 订阅套餐 <b>${esc(tierId || '免费')}</b></p>`;
  if (daily.limit != null) {
    const pct = daily.limit > 0 ? Math.round(((daily.remaining || 0) / daily.limit) * 100) : 0;
    const reset = daily.resetAt ? new Date(daily.resetAt).toLocaleString('zh-CN', { hour12: false }) : '—';
    html += `<p style="font-size:13px;margin-top:4px">积分：剩余 <b class="tok">${daily.remaining ?? '—'}</b> / ${daily.limit ?? '—'}（已用 ${daily.spent ?? 0}） · 重置 <b>${reset}</b> <span style="color:var(--muted)">（太平洋时间午夜，隔天自动刷新）</span></p>`;
    html += `<div style="height:8px;background:#21262d;border-radius:4px;overflow:hidden;margin:8px 0 10px"><div style="height:100%;width:${pct}%;background:${pct > 50 ? 'var(--ok)' : pct > 20 ? 'var(--warn)' : 'var(--err)'}"></div></div>`;
  }
  const prices = fb.prices || {};
  const rl = q.rateLimitsByModel || {};
  const models = Object.keys(rl);
  if (models.length) {
    html += `<table><thead><tr><th>模型</th><th>今日剩余次数</th><th>限额</th><th>已用</th><th>积分价</th><th>下次重置</th></tr></thead><tbody>${
      models.map(m => {
        const v = rl[m] || {};
        const remain = v.limit != null ? Math.max(0, (v.limit || 0) - (v.recentCount || 0)) : '—';
        const price = prices[m] != null ? (prices[m] === 0 ? '<b class="tok">免费</b>' : prices[m]) : '—';
        const reset = v.resetAt ? new Date(v.resetAt).toLocaleString('zh-CN', { hour12: false, month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' }) : '—';
        const pool = v.poolLabel ? ` <span style="color:var(--muted);font-size:11px">${esc(v.poolLabel)}</span>` : '';
        return `<tr><td>${esc(m)}${pool}</td><td><b>${remain}</b></td><td>${v.limit ?? '—'}</td><td>${v.recentCount ?? 0}</td><td>${price}</td><td style="font-size:12px">${reset}</td></tr>`;
      }).join('')}</tbody></table>`;
  }
  const extra = Object.entries(prices).filter(([m]) => !rl[m]);
  if (extra.length) {
    html += `<details style="margin-top:8px"><summary>其他模型积分价（${extra.length}）</summary><div style="margin-top:6px">${extra.map(([m, p]) => `<span class="chip">${esc(m)}: ${p === 0 ? '免费' : p}</span>`).join('')}</div></details>`;
  }

  // 凭证与刷新时间
  const c = d.credential || {};
  html += `<div style="font-size:12px;color:var(--muted);margin-top:12px;border-top:1px solid var(--border);padding-top:8px">当前凭证 ${esc(c.token_masked || '')} · 来源 ${esc(c.source || '')}${c.added_at ? ' · 入库 ' + new Date(c.added_at).toLocaleString('zh-CN', { hour12: false }) : ''} · 数据更新于 ${d.fetched_at ? new Date(d.fetched_at).toLocaleTimeString('zh-CN', { hour12: false }) : '—'}</div>`;
  return html;
}
async function refreshCredential() {
  toast('正在做凭证保活检查…');
  try {
    const r = await api('/api/account/refresh', { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' });
    toast(r.message || (r.ok ? '凭证有效' : '凭证可能已失效'), 7000);
    if (r.ok) refreshAccountOverview();
  } catch (e) { toast('检查失败：' + e.message, 6000); }
}

// ---------- 技能 ----------
let editingSkillId = null;
let skillsCache = [];   // 索引化引用，避免把用户数据拼进内联 JS（防 XSS/引号破坏）
async function refreshSkills() {
  try {
    const d = await api('/api/skills');
    const skills = d.skills || [];
    skillsCache = skills;
    $('roster-info').textContent = `roster 预览约 ${d.roster_tokens ?? '—'} tokens（注入预算 ${d.max_roster_tokens ?? 2000}）`;
    $('skills-wrap').innerHTML = skills.length ? `<table><thead><tr><th>名称</th><th>描述</th><th>来源</th><th>状态</th><th>操作</th></tr></thead><tbody>${
      skills.map((s, i) => `<tr>
        <td>${esc(s.name)} ${s.builtin ? badge('内置', 'dim') : ''}</td>
        <td style="max-width:340px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap" title="${esc(s.description)}">${esc(s.description)}</td>
        <td>${esc(s.source || 'local')}</td>
        <td>${s.enabled ? badge('已启用', 'ok') : badge('已禁用', 'dim')}</td>
        <td class="row">
          <button class="ghost sm" onclick="toggleSkillAt(${i})">${s.enabled ? '禁用' : '启用'}</button>
          ${s.builtin ? '<span style="color:var(--muted);font-size:12px">内置项不可编辑</span>' : `<button class="ghost sm" onclick="editSkillAt(${i})">编辑</button><button class="ghost sm" onclick="delSkillAt(${i})">删除</button>`}
        </td></tr>`).join('')}</tbody></table>` : '<div class="empty">技能库为空（内置技能会在首次启动时自动写入）</div>';
  } catch (e) { $('skills-wrap').innerHTML = `<div class="empty">加载失败：${esc(e.message)}</div>`; }
}
function toggleSkillAt(i) { const s = skillsCache[i]; if (s) toggleSkill(s.id, !s.enabled); }
function delSkillAt(i) { const s = skillsCache[i]; if (s) delSkill(s.id); }
function editSkillAt(i) {
  const s = skillsCache[i];
  if (!s) return;
  editingSkillId = s.id;
  $('skill-editor-title').textContent = '编辑技能：' + s.name;
  $('sk-name').value = s.name; $('sk-desc').value = s.description; $('sk-body').value = s.body || '';
  $('skill-editor').style.display = '';
  $('gate-result').textContent = '';
}
function newSkill() {
  editingSkillId = null;
  $('skill-editor-title').textContent = '新建技能';
  $('sk-name').value = ''; $('sk-desc').value = ''; $('sk-body').value = '';
  $('skill-editor').style.display = '';
  $('gate-result').textContent = '';
}
function editSkill(jsonStr) {
  const s = JSON.parse(jsonStr);
  editingSkillId = s.id;
  $('skill-editor-title').textContent = '编辑技能：' + s.name + (s.builtin ? '（内置，仅可改正文）' : '');
  $('sk-name').value = s.name; $('sk-desc').value = s.description; $('sk-body').value = s.body || '';
  $('skill-editor').style.display = '';
}
async function saveSkill() {
  const name = $('sk-name').value.trim(), desc = $('sk-desc').value.trim(), body = $('sk-body').value;
  if (!name || !desc) { toast('名称和描述不能为空'); return; }
  let force = false;
  try {
    // 保存前质量门：有问题先提示，用户确认后 force 保存
    const g = await api('/api/skills/gate', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ body: name + '\n' + desc + '\n' + body }) });
    const issues = g.issues || [];
    if (issues.length) {
      $('gate-result').innerHTML = '<span class="terr">质量门发现问题：</span><br>' + issues.map(x => '• ' + esc(x)).join('<br>');
      if (!confirm('质量门发现 ' + issues.length + ' 个问题：\n\n' + issues.join('\n') + '\n\n仍要强制保存吗？')) return;
      force = true;
    } else {
      $('gate-result').textContent = '质量门通过 ✅';
    }
    await api('/api/skills', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ id: editingSkillId, name, description: desc, body, force }) });
    $('skill-save-result').textContent = '✅ 已保存';
    $('skill-editor').style.display = 'none';
    refreshSkills();
  } catch (e) { $('skill-save-result').innerHTML = `<span class="terr">保存失败：${esc(e.message)}</span>`; }
}
async function toggleSkill(id, enabled) {
  try { await api('/api/skills/toggle', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ id, enabled }) }); refreshSkills(); }
  catch (e) { toast('操作失败: ' + e.message); }
}
async function delSkill(id) {
  if (!confirm('确定删除这个技能？')) return;
  try { await api('/api/skills/delete', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ id }) }); refreshSkills(); }
  catch (e) { toast('删除失败: ' + e.message); }
}

// ---------- 记忆 ----------
let memCache = [];
function kindLabel(k) { return ({ preference: '偏好', correction: '纠正', habit: '习惯', project: '项目', feedback: '反馈' })[k] || k; }
async function refreshMemory() {
  try {
    const d = await api('/api/memory');
    const s = d.stats || {};
    $('mem-stats').innerHTML = `共 <b>${s.total ?? 0}</b> 条 · 稳定事实 ${s.static_count ?? 0} · 纠正 ${s.corrections ?? 0}`;
    const items = d.memories || [];
    memCache = items;
    $('mem-wrap').innerHTML = items.length ? `<table><thead><tr><th>类型</th><th>标题</th><th>内容</th><th>状态</th><th>操作</th></tr></thead><tbody>${
      items.map((m, i) => `<tr>
        <td>${esc(kindLabel(m.kind))}</td>
        <td>${esc(m.title)}</td>
        <td style="max-width:420px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap" title="${esc(m.content)}">${esc(m.content)}</td>
        <td>${m.is_static ? badge('稳定', 'ok') : badge('近期', 'dim')}</td>
        <td class="row">
          <button class="ghost sm" onclick="toggleMemStaticAt(${i})">${m.is_static ? '转近期' : '转稳定'}</button>
          <button class="ghost sm" onclick="delMemAt(${i})">删除</button>
        </td></tr>`).join('')}</tbody></table>` : '<div class="empty">还没有记忆 — 正常使用即可自动积累，或点「手动添加」</div>';
  } catch (e) { $('mem-wrap').innerHTML = `<div class="empty">加载失败：${esc(e.message)}</div>`; }
}
function newMemory() {
  $('mem-editor').style.display = '';
  $('mem-title').value = ''; $('mem-content').value = ''; $('mem-save-result').textContent = '';
}
async function saveMemory() {
  const title = $('mem-title').value.trim(), content = $('mem-content').value.trim(), kind = $('mem-kind').value;
  if (!title || !content) { toast('标题和内容不能为空'); return; }
  try {
    await api('/api/memory', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ kind, title, content, is_static: false }) });
    $('mem-save-result').textContent = '✅ 已保存';
    $('mem-editor').style.display = 'none';
    refreshMemory();
  } catch (e) { $('mem-save-result').innerHTML = `<span class="terr">保存失败：${esc(e.message)}</span>`; }
}
function toggleMemStaticAt(i) { const m = memCache[i]; if (m) setMemStatic(m.id, !m.is_static); }
function delMemAt(i) { const m = memCache[i]; if (m) delMemory(m.id); }
async function setMemStatic(id, v) {
  try { await api('/api/memory/static', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ id, is_static: v }) }); refreshMemory(); }
  catch (e) { toast('操作失败: ' + e.message); }
}
async function delMemory(id) {
  if (!confirm('删除这条记忆？')) return;
  try { await api('/api/memory/delete', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ id }) }); refreshMemory(); }
  catch (e) { toast('删除失败: ' + e.message); }
}

// ---------- 日志 ----------
let logEvents = [];
let logSource = null;
async function initLogs() {
  if (logSource) return;
  try {
    const d = await api('/api/logs/recent?limit=100');
    logEvents = d.events || [];
    renderLogs();
    logSource = new EventSource('/api/logs/stream' + (apiKey() ? '?key=' + encodeURIComponent(apiKey()) : ''));
    logSource.onmessage = (e) => {
      try { const ev = JSON.parse(e.data); logEvents.push(ev); if (logEvents.length > 500) logEvents = logEvents.slice(-500); renderLogs(); } catch (err) {}
    };
    logSource.onerror = () => { /* 自动重连由浏览器处理 */ };
  } catch (e) { $('logbox').innerHTML = `<div class="empty">日志加载失败：${esc(e.message)}</div>`; }
}
function renderLogs() {
  const filter = $('log-filter')?.value || '';
  const list = logEvents.filter(e => !filter || e.level === filter).slice(-300);
  $('logbox').innerHTML = list.length ? list.map(e =>
    `<div class="lv-${esc(e.level)}">[${fmtTime(e.ts)}] ${esc(e.level).toUpperCase()} ${esc(e.kind)}${e.req_id ? ' #' + esc(e.req_id) : ''} — ${esc(e.message)}</div>`
  ).join('') : '<div class="empty">暂无日志</div>';
  const box = $('logbox'); box.scrollTop = box.scrollHeight;
}
function clearLogs() { logEvents = []; renderLogs(); }

// ---------- 体检 ----------
async function refreshDoctor() {
  $('doctor-wrap').innerHTML = '<div class="empty">检查中…</div>';
  try {
    const d = await api('/api/doctor');
    const checks = d.checks || [];
    const icon = (s) => s === 'ok' ? '<span class="tok">✅ ok</span>' : s === 'fault' ? '<span class="terr">❌ fault</span>' : s === 'fact' ? '<span class="twarn">ℹ️ fact</span>' : '<span style="color:var(--muted)">◻ not checked</span>';
    $('doctor-wrap').innerHTML = checks.map(c => `<div class="doctor-item"><div class="st">${icon(c.state)}</div><div style="flex:1"><b>${esc(c.label)}</b><div style="color:var(--muted);margin-top:2px">${esc(c.detail)}</div>${c.fix ? `<div style="color:var(--accent);margin-top:2px">→ ${esc(c.fix)}</div>` : ''}</div></div>`).join('');
  } catch (e) { $('doctor-wrap').innerHTML = `<div class="empty">体检失败：${esc(e.message)}</div>`; }
}

// ---------- 启动 ----------
refreshOverview();
loadGuide();
// 扩展可能在页面加载后才被激活，启动后多探几次（最多 5 次，探测到即停）
renderExtStatus();
(() => {
  let tries = 0;
  const t = setInterval(() => { pingExtension(); if (++tries >= 5 || extId) clearInterval(t); }, 2000);
})();
// hash 路由：托盘「系统体检」→ /#doctor
if (location.hash === '#doctor') showTab('doctor');
startOverviewTimer();
</script>
</body>
</html>"##;

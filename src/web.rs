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
    <div class="grid">
      <div class="panel"><h2>账号健康度</h2><div id="acc-wrap"></div></div>
      <div class="panel"><h2>近 7 天用量</h2><div id="daily-wrap"></div></div>
    </div>
    <div class="panel"><h2>最近请求 <span style="font-weight:400">（点击行查看详情）</span></h2><div id="reqs-wrap"></div></div>
    <div class="panel"><h2>可用模型（<span id="model-count">…</span>）</h2><div id="models-wrap"></div></div>
    <div class="panel" id="balance-panel" style="display:none"><h2>账号积分</h2><div id="balance-wrap"></div></div>
  </section>

  <section id="tab-account" style="display:none">
    <div class="panel">
      <h2>添加账号</h2>
      <div class="row" style="margin-bottom:10px">
        <button onclick="oneClickLogin()">🔑 一键登录（桌面版）</button>
        <span style="color:var(--muted);font-size:12px">或在下方粘贴 Cookie / cURL / HAR 内容</span>
      </div>
      <textarea id="import-text" placeholder="粘贴以下任意一种：
1) 浏览器 Cookie 串（含 __Secure-next-auth.session-token=...）
2) 从 DevTools 复制的 cURL (bash) 命令
3) HAR 导出文件的 JSON 内容"></textarea>
      <div class="row" style="margin-top:10px">
        <button onclick="doImport()">导入</button>
        <span id="import-result" style="font-size:13px;color:var(--muted)"></span>
      </div>
      <details style="margin-top:12px"><summary>怎么获取 Cookie？（点击展开）</summary>
        <ol style="margin:10px 0 0 20px;font-size:13px;color:var(--muted);line-height:1.9">
          <li>浏览器登录 freebuff.com</li>
          <li>按 F12 打开开发者工具 → Network 标签</li>
          <li>刷新页面，点任意请求 → Headers → 找到 <code>Cookie:</code> 开头那一整行</li>
          <li>整行复制，粘贴到上面的框里，点「导入」</li>
        </ol>
      </details>
    </div>
    <div class="panel"><h2>已入库凭证</h2><div id="tokens-wrap"></div></div>
    <div class="panel"><h2>账号详情</h2><button class="ghost sm" onclick="loadAccountDetail()">查询余额与订阅</button><div id="detail-wrap" style="margin-top:10px"></div></div>
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
      <details><summary><b>⑥ 数据都存在哪</b></summary>
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
      <p style="font-size:13px;color:var(--muted);margin-bottom:10px">网关地址：<code id="guide-base">http://127.0.0.1:47821</code>（API 前缀加 <code>/v1</code>）</p>
      <h2 style="margin-top:16px">Claude Code（Anthropic 协议）</h2>
      <pre id="g-claude"></pre><button class="ghost sm" onclick="copyText(document.getElementById('g-claude').textContent)">复制</button>
      <h2 style="margin-top:16px">Cursor / Continue / 通用 OpenAI 客户端</h2>
      <pre id="g-openai"></pre><button class="ghost sm" onclick="copyText(document.getElementById('g-openai').textContent)">复制</button>
      <h2 style="margin-top:16px">OpenAI SDK (Python)</h2>
      <pre id="g-py"></pre><button class="ghost sm" onclick="copyText(document.getElementById('g-py').textContent)">复制</button>
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
function setApiKey(v) { try { localStorage.setItem('freebuff_api_key', v.trim()); toast(v.trim() ? 'API Key 已保存（刷新页面生效）' : 'API Key 已清除'); } catch (e) {} }
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
  if (name === 'account') { refreshTokens(); loadBalance(); }
  if (name === 'logs') initLogs();
  if (name === 'guide') fillGuide();
}
function fillGuide() {
  const base = location.origin;
  $('guide-base').textContent = base;
  $('g-claude').textContent = `# macOS / Linux\nexport ANTHROPIC_BASE_URL=${base}\nexport ANTHROPIC_API_KEY=sk-local\n\n# Windows PowerShell\n$env:ANTHROPIC_BASE_URL="${base}"\n$env:ANTHROPIC_API_KEY="sk-local"`;
  $('g-openai').textContent = `Base URL: ${base}/v1\nAPI Key:  sk-local（本机未配置 api_keys 时随意填）\n模型:     在 ${base}/v1/models 中选一个`;
  $('g-py').textContent = `from openai import OpenAI\nclient = OpenAI(base_url="${base}/v1", api_key="sk-local")\nresp = client.chat.completions.create(model="z-ai/glm-5.3-flash", messages=[{"role":"user","content":"你好"}])\nprint(resp.choices[0].message.content)`;
  $('g-lobe').textContent = `接口地址: ${base}/v1\nAPI Key:  sk-local\n模型名:   手动填 /v1/models 列表中的值（如 z-ai/glm-5.3-flash）`;
}

// ---------- 总览 ----------
let lastHealth = null;
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
  } catch (e) { $('dot').className = 'dot err'; toast('加载失败: ' + e.message); }
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

// ---------- 账号 / 导入 ----------
async function oneClickLogin() {
  if (window.freebuffDesktop && window.freebuffDesktop.openLogin) {
    window.freebuffDesktop.openLogin();
    toast('已打开登录窗口，请在其中登录 freebuff.com');
  } else {
    toast('浏览器访问时请用下方「粘贴导入」；桌面版支持托盘一键登录');
  }
}
async function doImport() {
  const text = $('import-text').value.trim();
  if (!text) { toast('请先粘贴内容'); return; }
  $('import-result').textContent = '导入中…';
  try {
    const r = await api('/api/tokens/import', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ cookie: text }) });
    $('import-result').innerHTML = r.added > 0 ? `<span class="tok">✅ 成功导入 ${r.added} 个账号</span>` : `<span class="twarn">未新增（token 可能已存在）：${esc(r.message || '')}</span>`;
    $('import-text').value = '';
    refreshTokens(); refreshOverview();
  } catch (e) { $('import-result').innerHTML = `<span class="terr">导入失败：${esc(e.message)}</span>`; }
}
async function refreshTokens() {
  try {
    const r = await api('/api/tokens');
    const list = r.tokens || [];
    $('tokens-wrap').innerHTML = list.length ? `<table><thead><tr><th>Token</th><th>来源</th><th>Host</th><th>路径</th></tr></thead><tbody>${
      list.map(t => `<tr><td><code>${esc(t.token_masked)}</code></td><td>${esc(t.source)}</td><td>${esc(t.host)}</td><td>${esc(t.path)}</td></tr>`).join('')}</tbody></table>` : '<div class="empty">还没有导入凭证</div>';
  } catch (e) { $('tokens-wrap').innerHTML = `<div class="empty">加载失败：${esc(e.message)}</div>`; }
}
async function loadAccountDetail() {
  $('detail-wrap').innerHTML = '<div class="empty">查询中…</div>';
  try {
    const d = await api('/api/account/detail', { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' });
    $('detail-wrap').innerHTML = `<pre>${esc(JSON.stringify({ user: d.user, subscriptions: d.subscriptions, balance: d.balance, usage_summary: d.usage_summary }, null, 2))}</pre>`;
  } catch (e) { $('detail-wrap').innerHTML = `<div class="empty">查询失败：${esc(e.message)}（需要先导入 web Cookie）</div>`; }
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
// hash 路由：托盘「系统体检」→ /#doctor
if (location.hash === '#doctor') showTab('doctor');
setInterval(() => { if ($('tab-overview').style.display !== 'none') refreshOverview(); }, 6000);
</script>
</body>
</html>"##;

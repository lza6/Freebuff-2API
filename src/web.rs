//! 内置 Web 控制面板（用量统计 / 账号健康 / 模型列表 / 日志）
//!
//! 轻量无构建：单 HTML + 原生 JS + CSS，Rust 直接内嵌字符串。
//! 既有网关功能 + 面板双端口：网关 8787，面板由同一进程 /ui 路径提供。

pub const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Freebuff2API 控制台</title>
<style>
:root { --bg:#0d1117; --card:#161b22; --border:#30363d; --text:#e6edf3; --muted:#8b949e; --accent:#2f81f7; --ok:#3fb950; --warn:#d29922; --err:#f85149; }
* { box-sizing:border-box; margin:0; padding:0; }
body { background:var(--bg); color:var(--text); font-family:-apple-system,'Segoe UI',Roboto,sans-serif; min-height:100vh; }
header { display:flex; align-items:center; justify-content:space-between; padding:16px 24px; border-bottom:1px solid var(--border); background:var(--card); position:sticky; top:0; z-index:10; }
header h1 { font-size:18px; font-weight:600; }
header .dot { display:inline-block; width:8px; height:8px; border-radius:50%; background:var(--ok); margin-right:8px; vertical-align:middle; }
main { max-width:1200px; margin:0 auto; padding:24px; }
.cards { display:grid; grid-template-columns:repeat(auto-fit,minmax(180px,1fr)); gap:16px; margin-bottom:24px; }
.card { background:var(--card); border:1px solid var(--border); border-radius:10px; padding:16px; }
.card .num { font-size:28px; font-weight:700; margin-top:4px; }
.card .lbl { color:var(--muted); font-size:13px; }
.grid { display:grid; grid-template-columns:1fr 1fr; gap:24px; }
@media(max-width:900px){ .grid{grid-template-columns:1fr} }
.panel { background:var(--card); border:1px solid var(--border); border-radius:10px; padding:16px; }
.panel h2 { font-size:15px; margin-bottom:12px; color:var(--muted); font-weight:600; }
table { width:100%; border-collapse:collapse; font-size:13px; }
th,td { text-align:left; padding:8px 10px; border-bottom:1px solid var(--border); }
th { color:var(--muted); font-weight:500; }
.badge { display:inline-block; padding:2px 8px; border-radius:20px; font-size:12px; }
.badge.ok { background:#1a5e2a; color:var(--ok); }
.badge.warn { background:#5a4a1a; color:var(--warn); }
.badge.err { background:#5e1a1a; color:var(--err); }
.chip { display:inline-block; background:#21262d; border:1px solid var(--border); border-radius:6px; padding:2px 8px; margin:2px; font-size:12px; }
.status-row { display:flex; gap:8px; align-items:center; flex-wrap:wrap; }
.actions { display:flex; gap:8px; }
button { background:var(--accent); color:#fff; border:none; border-radius:6px; padding:6px 14px; cursor:pointer; font-size:13px; }
button:hover { filter:brightness(1.1); }
button.ghost { background:transparent; border:1px solid var(--border); color:var(--text); }
#toast { position:fixed; bottom:24px; left:50%; transform:translateX(-50%); background:var(--card); border:1px solid var(--border); padding:10px 20px; border-radius:8px; display:none; }
.logs { max-height:400px; overflow:auto; font-family:ui-monospace,monospace; font-size:12px; }
.logs div { padding:2px 0; border-bottom:1px dashed var(--border); color:var(--muted); }
</style>
</head>
<body>
<header>
  <h1><span class="dot"></span>Freebuff2API 控制台</h1>
  <div class="actions">
    <button class="ghost" onclick="location.href='/healthz'">健康检查</button>
    <button onclick="location.href='/v1/models'">模型列表</button>
    <button onclick="refresh()">刷新</button>
  </div>
</header>
<main>
  <div class="cards" id="cards"></div>
  <div class="grid">
    <div class="panel">
      <h2>账号健康度</h2>
      <table id="accounts"><thead><tr><th>账号</th><th>状态</th><th>评分</th><th>会话</th><th>错误</th></tr></thead><tbody></tbody></table>
    </div>
    <div class="panel">
      <h2>近 24h 用量</h2>
      <table id="daily"><thead><tr><th>日期</th><th>模型</th><th>请求</th><th>输入tok</th><th>输出tok</th><th>错误</th></tr></thead><tbody></tbody></table>
    </div>
  </div>
  <div class="grid" style="margin-top:24px">
    <div class="panel">
      <h2>最近请求</h2>
      <table id="reqs"><thead><tr><th>时间</th><th>账号</th><th>模型</th><th>状态</th><th>延迟</th></tr></thead><tbody></tbody></table>
    </div>
    <div class="panel">
      <h2>可用模型（{model_count}）</h2>
      <div id="models"></div>
    </div>
  </div>
</main>
<div id="toast"></div>
<script>
async function j(url){ const r=await fetch(url); if(!r.ok) throw new Error(await r.text()); return r.json(); }
function el(tag,cls,text){ const e=document.createElement(tag); if(cls)e.className=cls; if(text)e.textContent=text; return e; }
function badge(text){ const b=el('span','badge'); b.textContent=text; if(text==='ok'||text==='healthy'||text==='active') b.className='badge ok'; else if(text==='queued'||text==='cooldown') b.className='badge warn'; else b.className='badge err'; return b; }
function toast(msg){ const t=document.getElementById('toast'); t.textContent=msg; t.style.display='block'; setTimeout(()=>t.style.display='none',2500); }
async function refresh(){
  try{
    const health=await j('/healthz');
    const totals=await j('/api/usage/totals');
    const daily=await j('/api/usage/daily');
    const reqs=await j('/api/usage/requests');
    const models=await j('/api/usage/models');
    // cards
    const cards=document.getElementById('cards');
    cards.innerHTML='';
    const cardData=[
      ['总请求',totals.total_requests??0],
      ['总 Token',(totals.total_tokens??0).toLocaleString()],
      ['错误',totals.errors??0],
      ['账号数',health.pool?.total??health.accounts?.length??0],
    ];
    cardData.forEach(([lbl,num])=>{ const c=el('div','card'); c.append(el('div','lbl',lbl),el('div','num',String(num))); cards.append(c); });
    // accounts
    const accs=health.accounts||health.pool?.accounts||[];
    const atb=document.querySelector('#accounts tbody'); atb.innerHTML='';
    accs.forEach(a=>{
      const tr=el('tr');
      tr.append(el('td',null,a.name));
      const st=el('td',null); st.append(badge(a.session?.status||'unknown'));
      tr.append(st,el('td',null,String(Math.round(a.score??0))));
      tr.append(el('td',null,a.session?.instance_id?String(a.session.instance_id).slice(0,8)+'…':'—'));
      tr.append(el('td',null,a.last_error||a.session?.last_error||''));
      atb.append(tr);
    });
    // daily
    const dtb=document.querySelector('#daily tbody'); dtb.innerHTML='';
    (daily||[]).forEach(d=>{
      const tr=el('tr');
      tr.append(el('td',null,d.date),el('td',null,d.model),el('td',null,String(d.requests)),el('td',null,String(d.prompt_tokens)),el('td',null,String(d.completion_tokens)),el('td',null,String(d.errors)));
      dtb.append(tr);
    });
    if(!daily||!daily.length){ dtb.append(Object.assign(el('tr'),{}).append(el('td',{colspan:6,style:'color:var(--muted)'}).textContent='暂无数据')); }
    // reqs
    const rtb=document.querySelector('#reqs tbody'); rtb.innerHTML='';
    (reqs||[]).slice(0,20).forEach(r=>{
      const tr=el('tr');
      const st=el('td',null); st.append(badge(r.status<400?'ok':(r.status<500?'warn':'err')));
      tr.append(el('td',null,new Date(r.ts).toLocaleTimeString()),el('td',null,r.account),el('td',null,r.model),st,el('td',null,(r.latency_ms/1000).toFixed(2)+'s'));
      rtb.append(tr);
    });
    // models
    const mb=document.getElementById('models'); mb.innerHTML='';
    (models||[]).forEach(m=>mb.append(Object.assign(el('span','chip'),{textContent:m})));
    // 提示词/技能面板
    fetch('/api/prompts').then(r=>r.json()).then(pd=>{
      if(!pd || !pd.prompts) return;
      const panel=document.createElement('div'); panel.className='panel'; panel.style.marginTop='24px';
      panel.innerHTML='<h2>🧠 内置提示词 & 技能</h2>';
      let html='<table><thead><tr><th>名称</th><th>类型</th><th>状态</th><th>操作</th></tr></thead><tbody>';
      for(const p of pd.prompts){ html+=`<tr><td>${p.name}</td><td>提示词</td><td>${p.enabled?'✅':'⬜'}</td><td><button class="ghost" onclick="togglePrompt('prompt','${p.id}',${!p.enabled})">${p.enabled?'禁用':'启用'}</button></td></tr>`; }
      for(const s of pd.skills){ html+=`<tr><td>${s.name}</td><td>技能</td><td>${s.enabled?'✅':'⬜'}</td><td><button class="ghost" onclick="togglePrompt('skill','${s.id}',${!s.enabled})">${s.enabled?'禁用':'启用'}</button></td></tr>`; }
      html+='</tbody></table>';
      html+=`<p style="color:var(--muted);font-size:12px">system 前缀预览（注入聊天）:<br><code style="white-space:pre-wrap;display:block;background:#21262d;padding:8px;border-radius:6px">${(pd.system_prefix_preview||'').slice(0,300)}</code></p>`;
      panel.innerHTML+=html;
      document.querySelector('.grid').after(panel);
    }).catch(()=>{});
  }catch(e){ toast('加载失败: '+e.message); }
}
async function togglePrompt(type,id,enabled){
  try{
    const r=await fetch('/api/prompts/toggle',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({type,id,enabled})});
    if(!r.ok) throw new Error(await r.text());
    refresh();
  }catch(e){ toast('操作失败: '+e.message); }
}
    // 账号余额
    fetch('/api/account/balance').then(r=>r.json()).then(bal=>{
      if(bal && bal.ok!==false){
        const bc=document.createElement('div'); bc.className='panel'; bc.style.marginTop='24px';
        bc.innerHTML='<h2>📊 账号积分（web Cookie）</h2>';
        let html=`<p style="color:var(--muted);font-size:13px">套餐: <b>${bal.subscription?.tierId||'免费'}</b> · 层级: ${bal.access_tier||'—'} ${bal.country_block_reason?`· ⚠️ 地区受限(${bal.country_block_reason})`:''}</p>`;
        if(bal.freebucks){ const d=bal.freebucks.daily||{}; html+=`<p style="color:var(--muted);font-size:13px">今日积分: <b style="color:var(--ok)">${d.remaining??'—'} / ${d.limit??'—'}</b>（已用 ${d.spent??0}）· 重置 ${new Date(d.resetAt).toLocaleString('zh-CN',{hour12:false})}</p>`; }
        html+='<table><thead><tr><th>模型</th><th>积分价</th><th>今日剩余</th></tr></thead><tbody>';
        if(bal.model_remaining){ for(const [m,v] of Object.entries(bal.model_remaining)){
          const usable = v.usable_today===-1?'不限':v.usable_today;
          const price = v.price===0?'<b style="color:var(--ok)">免费</b>':v.price;
          html+=`<tr><td>${m}</td><td>${price}</td><td>${usable}</td></tr>`;
        }}
        html+='</tbody></table>';
        bc.innerHTML+=html;
        document.querySelector('.grid').after(bc);
      }
    }).catch(()=>{});
  }catch(e){ toast('加载失败: '+e.message); }
}
refresh();
setInterval(refresh,5000);
</script>
</body>
</html>"#;

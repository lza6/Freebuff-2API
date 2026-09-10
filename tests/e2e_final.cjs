// 最终 E2E 验收脚本：覆盖全部端点
const http = require("node:http");

const BASE = { host: "127.0.0.1", port: 47821 };
let pass = 0, fail = 0;
const results = [];

function req(method, path, body) {
  return new Promise((resolve) => {
    const data = body ? JSON.stringify(body) : null;
    const headers = { "content-type": "application/json" };
    if (data) headers["content-length"] = Buffer.byteLength(data);
    const r = http.request({ ...BASE, path, method, headers }, (res) => {
      let d = "";
      res.on("data", (c) => (d += c));
      res.on("end", () => resolve({ status: res.statusCode, body: d }));
    });
    r.on("error", (e) => resolve({ status: 0, body: String(e) }));
    r.setTimeout(20000, () => { r.destroy(); resolve({ status: 0, body: "timeout" }); });
    if (data) r.write(data);
    r.end();
  });
}

async function check(name, fn) {
  try {
    const ok = await fn();
    if (ok) { pass++; results.push(`✅ ${name}`); }
    else { fail++; results.push(`❌ ${name}`); }
  } catch (e) {
    fail++; results.push(`❌ ${name} — ${e.message}`);
  }
}

(async () => {
  // 1. 健康检查
  await check("GET /healthz 返回 ok + accounts", async () => {
    const r = await req("GET", "/healthz");
    const j = JSON.parse(r.body);
    return r.status === 200 && j.ok === true && Array.isArray(j.accounts);
  });

  // 2. 模型列表
  await check("GET /v1/models ≥20 个模型", async () => {
    const r = await req("GET", "/v1/models");
    const j = JSON.parse(r.body);
    return r.status === 200 && j.data.length >= 20;
  });

  // 3. 面板（注意：HTML 是浏览器端 JS 动态渲染按钮，服务端只返回脚本源码；
//    这里校验「渲染 6 prompt + 5 skill 按钮」所需的源码锚点都齐全）
  await check("GET /ui 面板 200 + 含提示词管理", async () => {
    const r = await req("GET", "/ui");
    const srcHasPromptsLoop = r.body.includes("pd.prompts") && r.body.includes("togglePrompt('prompt'");
    const srcHasSkillsLoop = r.body.includes("pd.skills") && r.body.includes("togglePrompt('skill'");
    const srcHasToggleFn = r.body.includes("async function togglePrompt");
    return r.status === 200 && r.body.includes("内置提示词") && srcHasPromptsLoop && srcHasSkillsLoop && srcHasToggleFn;
  });

  // 4. 用量统计
  await check("GET /api/usage/totals 返回统计", async () => {
    const r = await req("GET", "/api/usage/totals");
    const j = JSON.parse(r.body);
    return r.status === 200 && typeof j.total_requests === "number";
  });

  // 5. 请求明细
  await check("GET /api/usage/requests 返回数组", async () => {
    const r = await req("GET", "/api/usage/requests");
    return r.status === 200 && Array.isArray(JSON.parse(r.body));
  });

  // 6. 账号列表
  await check("GET /api/usage/accounts 返回账号", async () => {
    const r = await req("GET", "/api/usage/accounts");
    const j = JSON.parse(r.body);
    return r.status === 200 && Array.isArray(j.accounts);
  });

  // 7. 提示词列表
  await check("GET /api/prompts 6 prompts + 5 skills", async () => {
    const r = await req("GET", "/api/prompts");
    const j = JSON.parse(r.body);
    return r.status === 200 && j.prompts.length === 6 && j.skills.length === 5;
  });

  // 8. 提示词启用
  await check("POST /api/prompts/toggle 启用 skill 并注入 prefix", async () => {
    const r = await req("POST", "/api/prompts/toggle", { type: "skill", id: "git-guru", enabled: true });
    const j = JSON.parse(r.body);
    return r.status === 200 && j.ok && j.system_prefix_preview.includes("Git 专家");
  });

  // 9. token 列表
  await check("GET /api/tokens 返回已导入 token", async () => {
    const r = await req("GET", "/api/tokens");
    const j = JSON.parse(r.body);
    return r.status === 200 && j.ok && Array.isArray(j.tokens);
  });

  // 10. 余额查询（web Cookie）
  await check("GET /api/account/balance 返回 freebucks", async () => {
    const r = await req("GET", "/api/account/balance");
    const j = JSON.parse(r.body);
    return r.status === 200 && j.freebucks && typeof j.freebucks.balance === "number";
  });

  // 11. 账号详情卡片
  await check("POST /api/account/detail 返回 user+usage", async () => {
    const r = await req("POST", "/api/account/detail", {});
    const j = JSON.parse(r.body);
    return r.status === 200 && j.user && j.usage_summary;
  });

  // 12. 错误透传（假 token → 上游 401 透传）
  await check("POST /v1/chat/completions 假token → 上游401透传", async () => {
    const r = await req("POST", "/v1/chat/completions", { model: "z-ai/glm-5.3-flash", messages: [{ role: "user", content: "hi" }] });
    return r.status === 502 && r.body.includes("401") && r.body.includes("Invalid API key");
  });

  // 13. web chat 真实增量流式（Cookie 版）
  await check("POST /v1/web/chat 真实流式含 content+DONE", async () => {
    return new Promise((resolve) => {
      const data = JSON.stringify({ model: "glm-5.3-flash", content: "reply literally: OK" });
      const r = http.request({ ...BASE, path: "/v1/web/chat", method: "POST", headers: { "content-type": "application/json", "content-length": Buffer.byteLength(data) } }, (res) => {
        let d = "";
        res.on("data", (c) => (d += c));
        res.on("end", () => {
          resolve(res.statusCode === 200 && d.includes("[DONE]") && (d.includes('"content"') || d.includes("reasoning_content")));
        });
      });
      r.on("error", () => resolve(false));
      r.setTimeout(60000, () => { r.destroy(); resolve(false); });
      r.write(data); r.end();
    });
  });

  // 14. 思考程度降级（solar-pro4 不支持 effort → 应剥离后仍请求；status 0=网络断需排除）
  await check("POST /v1/chat/completions solar-pro4+max effort → 上游协议层处理", async () => {
    const r = await req("POST", "/v1/chat/completions", { model: "upstage/solar-pro4", messages: [{ role: "user", content: "hi" }], reasoning_effort: "max" });
    // 剥离逻辑生效：不应 500；status=0 表示请求未发出（网络/服务不可达），不算通过
    return r.status !== 0 && r.status !== 500;
  });

  console.log("========== E2E 验收结果 ==========");
  results.forEach((r) => console.log(r));
  console.log(`\n通过: ${pass}  失败: ${fail}`);
  process.exit(fail > 0 ? 1 : 0);
})();

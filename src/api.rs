//! HTTP 路由：OpenAI 兼容 / Anthropic 兼容 / 控制面板 / 用量 API
//!
//! 依赖上游逆向协议：
//! - POST /v1/chat/completions → 选 token→建会话→注入 codebuff_metadata→转发→SSE 流回
//! - POST /v1/messages → Claude 协议转 OpenAI
//! - GET  /v1/models → 注册表
//! - GET  /healthz → 含账号健康快照
//! - /ui 面板 + /api/usage/* 统计

use crate::ads::AdRefresher;
use crate::config::Config;
use crate::models::ModelRegistry;
use crate::pool::Pool;
use crate::router::ModelRouter;

use crate::upstream::UpstreamClient;
use crate::usage::UsageDb;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use std::sync::Arc;

/// 统一 AppState（必须 Send+Sync+Clone 才能被 axum State 提取器使用）
#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub client: Arc<UpstreamClient>,
    pub pool: Arc<Pool>,
    pub registry: Arc<ModelRegistry>,
    pub router: Arc<ModelRouter>,
    pub usage: Arc<UsageDb>,
    pub ads: Arc<AdRefresher>,
    pub started: std::time::Instant,
}

// axum_core 已对 `S: Clone` 提供 blanket impl FromRef<S> for S，此处无需手动实现。
// 保留 FromRef 导入供后续扩展（若需要子状态 FromRef 时使用）。

pub fn build_router(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/usage/totals", get(handle_usage_totals))
        .route("/api/usage/daily", get(handle_usage_daily))
        .route("/api/usage/requests", get(handle_usage_requests))
        .route("/api/usage/models", get(handle_usage_models))
        .route("/api/usage/accounts", get(handle_accounts));

    Router::new()
        .route("/", get(handle_dashboard))
        .route("/ui", get(handle_dashboard))
        .route("/healthz", get(handle_healthz))
        .route("/v1/models", get(handle_v1_models))
        .route("/v1/chat/completions", axum::routing::post(handle_chat_completions))
        .route("/v1/messages", axum::routing::post(handle_claude_messages))
        .route("/api/tokens/import", axum::routing::post(handle_token_import))
        .route("/api/tokens", get(handle_tokens_list))
        .route("/api/account/balance", get(handle_account_balance))
        .route("/api/account/detail", axum::routing::post(handle_account_detail))
        .route("/v1/web/chat", axum::routing::post(handle_web_chat))
        .merge(api)
        .with_state(state)
}

// ---------- 面板 & 健康 ----------

async fn handle_dashboard() -> impl IntoResponse {
    Response::builder()
        .header("content-type", "text/html; charset=utf-8")
        .body(Body::from(crate::web::INDEX_HTML))
        .unwrap()
}

async fn handle_healthz(State(st): State<AppState>) -> Response {
    let dur = st.started.elapsed();
    let json = serde_json::json!({
        "ok": true,
        "uptime_sec": dur.as_secs(),
        "version": env!("CARGO_PKG_VERSION"),
        "accounts": st.pool.snapshot().await.accounts,
        "model_count": st.registry.snapshot().await.model_count,
        "ads": st.ads.snapshot().await,
    });
    Json(json).into_response()
}

async fn handle_v1_models(State(st): State<AppState>) -> impl IntoResponse {
    let models = st.registry.models().await;
    let data: Vec<serde_json::Value> = models
        .iter()
        .map(|m| {
            serde_json::json!({
                "id": m,
                "object": "model",
                "created": st.started.elapsed().as_secs() as i64,
                "owned_by": "Freebuff2API",
                "root": m,
                "permission": [],
            })
        })
        .collect();
    Json(serde_json::json!({ "object": "list", "data": data }))
}

// ---------- 用量 API ----------

async fn handle_usage_totals(State(st): State<AppState>) -> Response {
    match st.usage.totals() {
        Ok(v) => Json(v).into_response(),
        Err(e) => internal_err(&e),
    }
}

async fn handle_usage_daily(State(st): State<AppState>) -> Response {
    match st.usage.daily_usage(7) {
        Ok(v) => Json(serde_json::json!(v)).into_response(),
        Err(e) => internal_err(&e),
    }
}

async fn handle_usage_requests(State(st): State<AppState>) -> Response {
    match st.usage.recent_requests(50) {
        Ok(v) => Json(serde_json::json!(v)).into_response(),
        Err(e) => internal_err(&e),
    }
}

// ---------- 账号余额查询（web 版协议） ----------

/// GET /api/account/balance — 查询账号积分/每模型限额/套餐（用 web Cookie）
async fn handle_account_balance(State(st): State<AppState>) -> Response {
    // 候选：先找看起来像 Cookie 的 token（含 session-token），config 优先，其次导入库
    let looks_like_cookie = |t: &str| t.contains("session-token") || t.contains(".next-auth") || t.contains("callback-url") || t.contains("%3A");
    let cookie_candidate = st
        .cfg
        .auth_tokens
        .iter()
        .find(|t| looks_like_cookie(t))
        .cloned()
        .or_else(|| load_imported_token().filter(|t| looks_like_cookie(t)));
    let cookie = match cookie_candidate {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "ok": false, "message": "未找到 web 版 Cookie 凭证。请在浏览器登录 freebuff.com 后从 DevTools 复制完整 Cookie（含 __Secure-next-auth.session-token=...）并 POST /api/tokens/import 导入" })),
            )
                .into_response();
        }
    };

    let client = match crate::web_protocol::WebClient::new(cookie, "glm-5.3-flash".into()) {
        Ok(c) => c,
        Err(e) => return internal_err(&anyhow::Error::msg(e.to_string())),
    };
    match client.freebuff_session().await {
        Ok(sess) => {
            // 计算每模型剩余可用次数
            let mut model_remaining = serde_json::Map::new();
            if let Some(freebucks) = &sess.freebucks {
                let remaining = freebucks.daily.as_ref().map(|d| d.remaining).unwrap_or(0.0);
                if let Some(prices) = &freebucks.prices {
                    for (model, price) in prices {
                        let by_credit = if *price > 0.0 { (remaining / price).floor() as i64 } else { i64::MAX };
                        let by_limit = if let Some(rl) = sess.rate_limits_by_model.as_ref().and_then(|v| v.get(model)) {
                            let limit = rl.get("limit").and_then(|v| v.as_i64()).unwrap_or(0);
                            let recent = rl.get("recentCount").and_then(|v| v.as_i64()).unwrap_or(0);
                            if limit > 0 { (limit - recent).max(0) } else { i64::MAX }
                        } else {
                            i64::MAX
                        };
                        let usable = by_credit.min(by_limit);
                        let usable = if usable == i64::MAX { -1 } else { usable }; // -1 = 该模型不消费积分也不限次数
                        model_remaining.insert(model.clone(), serde_json::json!({
                            "price": price,
                            "by_credit_remaining": if by_credit == i64::MAX { -1 } else { by_credit },
                            "by_limit_remaining": if by_limit == i64::MAX { -1 } else { by_limit },
                            "usable_today": usable,
                        }));
                    }
                }
            }
            Json(serde_json::json!({
                "ok": true,
                "status": sess.status,
                "access_tier": sess.access_tier,
                "freebucks": sess.freebucks,
                "subscription": sess.subscription,
                "rate_limits_by_model": sess.rate_limits_by_model,
                "referral": sess.referral,
                "country_code": sess.country_code,
                "country_block_reason": sess.country_block_reason,
                "model_remaining": model_remaining,
                "message": sess.message,
            }))
            .into_response()
        }
        Err(e) => internal_err(&e),
    }
}

/// 读取 data/tokens.json 中第一个导入 token
fn load_imported_token() -> Option<String> {
    crate::import::load_tokens("data/tokens.json").ok().and_then(|t| t.into_iter().next()).map(|t| t.token)
}
fn internal_err(e: &anyhow::Error) -> Response {
    tracing::error!("用量统计查询失败: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))).into_response()
}

/// web 版完整对话代理（Cookie 鉴权 chat/stream → 真实增量 SSE 透传 → OpenAI 兼容）
/// POST /v1/web/chat — body: { model, content, thread_id?, images? }
async fn handle_web_chat(State(_st): State<AppState>, body: axum::body::Bytes) -> Response {
    let cookie = load_imported_token().filter(|t| t.contains("session-token"));
    let cookie = match cookie {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": { "message": "未导入 web Cookie 凭证。请 POST /api/tokens/import 粘贴 Cookie", "type": "invalid_request_error" } })),
            )
                .into_response();
        }
    };
    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": { "message": "invalid json", "type": "invalid_request_error" } })),
            )
                .into_response();
        }
    };
    let model = parsed.get("model").and_then(|m| m.as_str()).unwrap_or("glm-5.3-flash").to_string();
    let content = parsed.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string();
    let thread_id = parsed.get("thread_id").and_then(|t| t.as_str()).map(|s| s.to_string());
    let reasoning_effort = parsed.get("reasoning_effort").and_then(|r| r.as_str());
    if content.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": { "message": "content is required", "type": "invalid_request_error" } })),
        )
            .into_response();
    }

    let client = match crate::web_protocol::WebClient::new(cookie, model) {
        Ok(c) => c,
        Err(e) => return internal_err(&anyhow::Error::msg(e.to_string())),
    };
    // 真实增量流：上游 chat/stream SSE 逐事件实时转发为 OpenAI chunk
    match client.chat_stream_raw(thread_id.as_deref(), &content, reasoning_effort, vec![], vec![]).await {
        Ok(body) => {
            Response::builder()
                .header("content-type", "text/event-stream")
                .header("cache-control", "no-cache")
                .header("connection", "keep-alive")
                .header("x-accel-buffering", "no")
                .body(body)
                .unwrap()
        }
        Err(e) => internal_err(&e),
    }
}

async fn handle_usage_models(State(st): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!(st.registry.models().await))
}

async fn handle_accounts(State(st): State<AppState>) -> Response {
    Json(st.pool.snapshot().await).into_response()
}

// ---------- Token 导入（curl / HAR 自动解析入库） ----------

/// POST /api/tokens/import — body 传 curl 命令文本或 HAR JSON，自动提取 Bearer token 入库
async fn handle_token_import(State(st): State<AppState>, body: axum::body::Bytes) -> Response {
    let text = match std::str::from_utf8(&body) {
        Ok(t) => t,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "ok": false, "message": "输入不是合法 UTF-8 文本" })),
            )
                .into_response();
        }
    };
    let tokens = match crate::import::sniff_tokens(text) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "ok": false, "message": e.to_string() })),
            )
                .into_response();
        }
    };
    let tokens_path = "data/tokens.json";
    match crate::import::persist_tokens(tokens_path, &tokens) {
        Ok(added) => {
            if added.is_empty() {
                return Json(serde_json::json!({ "ok": true, "added": 0, "message": "token 已存在，未重复添加" }))
                    .into_response();
            }
            // 热更新到账号池
            let new_accounts: Vec<crate::pool::AccountEntry> = added
                .iter()
                .map(|t| {
                    crate::pool::AccountEntry {
                        name: format!("import-{}", t.token.chars().take(6).collect::<String>()),
                        token: t.token.clone(),
                        session: std::sync::Arc::new(crate::session::SessionManager::new(
                            st.client.clone(),
                            t.token.clone(),
                            (*st.cfg).clone(),
                        )),
                        score: tokio::sync::RwLock::new(0.0),
                        cooldown_until: tokio::sync::RwLock::new(None),
                    }
                })
                .collect();
            let mut _added_count: usize = 0;
            for acc in new_accounts {
                let added_flag = st.pool.add_account(acc).await;
                if added_flag {
                    _added_count += 1;
                }
            }
            Json(serde_json::json!({
                "ok": true,
                "added": added.len(),
                "tokens": added.iter().map(|t| serde_json::json!({
                    "token_masked": mask(&t.token),
                    "host": t.host,
                    "method": t.method,
                    "path": t.path,
                })).collect::<Vec<_>>(),
            }))
            .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "message": e.to_string() })),
        )
            .into_response(),
    }
}

/// GET /api/tokens — 列出已入库 token（脱敏）
async fn handle_tokens_list(State(st): State<AppState>) -> Response {
    let _ = &st;
    match crate::import::load_tokens("data/tokens.json") {
        Ok(tokens) => Json(serde_json::json!({
            "ok": true,
            "tokens": tokens.iter().map(|t| serde_json::json!({
                "token_masked": mask(&t.token),
                "source": t.source,
                "host": t.host,
                "path": t.path,
                "method": t.method,
            })).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "message": e.to_string() })),
        )
            .into_response(),
    }
}

/// 账户详情卡片数据（每个账号的余额/套餐/限额）
/// POST /api/account/detail — body { cookie: "..." } 可选；空则用已导入第一个
async fn handle_account_detail(State(st): State<AppState>, body: axum::body::Bytes) -> Response {
    // 解析 body（可选 cookie）
    let mut cookie: Option<String> = None;
    if !body.is_empty() {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) {
            if let Some(c) = v.get("cookie").and_then(|c| c.as_str()) {
                if c.contains("session-token") {
                    cookie = Some(c.to_string());
                }
            }
        }
    }
    let cookie = cookie.or_else(|| {
        st.cfg.auth_tokens.iter().find(|t| t.contains("session-token")).cloned()
            .or_else(|| load_imported_token().filter(|t| t.contains("session-token")))
    });
    let cookie = match cookie {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "ok": false, "message": "未找到 web Cookie 凭证" })),
            )
                .into_response();
        }
    };
    let client = match crate::web_protocol::WebClient::new(cookie.clone(), "glm-5.3-flash".into()) {
        Ok(c) => c,
        Err(e) => return internal_err(&anyhow::Error::msg(e.to_string())),
    };
    // 并发拉取余额+用量+用户+套餐
    let balance = client.freebuff_session().await;
    let usage = client.usage_summary().await;
    let auth = client.auth_session().await;
    let subs = client.subscriptions().await;
    Json(serde_json::json!({
        "ok": true,
        "cookie_masked": mask(&cookie),
        "balance": balance.ok(),
        "usage_summary": usage.ok(),
        "user": auth.ok(),
        "subscriptions": subs.ok(),
    }))
    .into_response()
}

/// 脱敏：只显示前 6 + 后 4
fn mask(token: &str) -> String {
    if token.len() <= 12 {
        return format!("{}***", &token[..token.len().saturating_sub(3).max(1)]);
    }
    let head = &token[..6];
    let tail = &token[token.len() - 4..];
    format!("{head}...{tail}")
}

// ---------- OpenAI 兼容 ----------

async fn handle_chat_completions(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let start = std::time::Instant::now();
    // 解析请求
    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": { "message": "request body must be valid JSON", "type": "invalid_request_error" } })),
            )
                .into_response();
        }
    };

    let requested = parsed.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
    if requested.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": { "message": "model is required", "type": "invalid_request_error" } })),
        )
            .into_response();
    }

    // API key 校验
    if !st.cfg.api_keys.is_empty() && !authorized(&headers, &st.cfg.api_keys) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": { "message": "invalid proxy api key", "type": "authentication_error" } })),
        )
            .into_response();
    }

    // 模型路由降级
    let model = st.router.resolve(&requested).await;

    // 挑 token
    let account = match st.pool.pick_best().await {
        Some(a) => a,
        None => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": { "message": "no healthy upstream auth token available", "type": "server_error" } })),
            )
                .into_response();
        }
    };
    let account_name = account.name.clone();
    let token = account.token.clone();

    // 确保会话
    let instance_id = match account.session.ensure_session(&model).await {
        Ok(id) => Some(id),
        Err(e) => {
            let msg = e.to_string();
            if msg.starts_with("waiting_room_queued") {
                st.usage.record(&account_name, &model, 0, 0, 0, 429, "", "").ok();
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({ "error": { "message": msg, "type": "server_error", "code": "waiting_room_queued" } })),
                )
                    .into_response();
            }
            st.pool
                .update_score(&account_name, -30.0)
                .await;
            st.usage.record(&account_name, &model, 0, 0, 0, 502, "", "").ok();
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": { "message": format!("failed to acquire free session: {msg}"), "type": "server_error" } })),
            )
                .into_response();
        }
    };

    // run 管理：取根 run（惰性）
    let run_id = match ensure_root_run(&st, &account_name, &token).await {
        Ok(id) => id,
        Err(e) => {
            st.usage.record(&account_name, &model, 0, 0, 0, 502, "", "").ok();
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": { "message": format!("create run failed: {e}"), "type": "server_error" } })),
            )
                .into_response();
        }
    };

    // 配置上行 body：设 model + 思考程度降级/剥离 + codebuff_metadata 注入
    let mut up_body = parsed.clone();
    up_body["model"] = serde_json::json!(model);
    if let Some(effort) = up_body.get("reasoning_effort").and_then(|v| v.as_str()) {
        match st.router.clamp_effort(&model, effort) {
            Some(clamped) => {
                if clamped != effort {
                    tracing::debug!("模型 {model} effort {effort} 降级为 {clamped}");
                    up_body["reasoning_effort"] = serde_json::json!(clamped);
                }
            }
            None => {
                tracing::debug!("模型 {model} 不支持 reasoning_effort，自动剥离");
                if let Some(obj) = up_body.as_object_mut() {
                    obj.remove("reasoning_effort");
                }
            }
        }
    }
    remove_passthrough_fields(&mut up_body);

    // 转发上游
    let upstream_resp = match st
        .client
        .chat_completions(&token, up_body.clone(), &run_id, instance_id.as_deref())
        .await
    {
        Ok(r) => r,
        Err(e) => {
            st.pool.update_score(&account_name, -40.0).await;
            let msg = e.to_string();
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": { "message": msg, "type": "server_error" } })),
            )
                .into_response();
        }
    };

    let status = upstream_resp.status();
    let latency_ms = start.elapsed().as_millis() as i64;

    // 用量记录
    let api_key = headers
        .get("x-api-key")
        .or_else(|| headers.get("authorization"))
        .map(|v| v.to_str().unwrap_or("").to_string())
        .unwrap_or_default();
    let client_ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or("").to_string())
        .unwrap_or_default();

    if status.is_success() {
        st.usage
            .record(&account_name, &model, 0, 0, latency_ms, 200, &api_key, &client_ip)
            .ok();
        st.pool.update_score(&account_name, 10.0).await;
    } else {
        st.usage
            .record(&account_name, &model, 0, 0, latency_ms, status.as_u16() as i64, &api_key, &client_ip)
            .ok();
        st.pool.update_score(&account_name, -20.0).await;
    }

    // 流式转发上游响应（真实增量透传 + 上游错误原样透传）
    let mut builder = Response::builder().status(status);
    for (k, v) in upstream_resp.headers() {
        if k != "content-length" && k != "transfer-encoding" {
            builder = builder.header(k, v);
        }
    }
    // 非 2xx：透传上游真实错误体（含 message/type/code）
    if !status.is_success() {
        let err_body = upstream_resp.bytes().await.unwrap_or_default();
        let err_text = String::from_utf8_lossy(&err_body).to_string();
        tracing::warn!("[上游错误] {model} HTTP {status}: {}", err_text.chars().take(500).collect::<String>());
        return (
            status,
            Json(serde_json::json!({
                "error": {
                    "message": err_text,
                    "type": "upstream_error",
                    "code": status.as_u16(),
                    "model": model,
                    "upstream": st.client.base_url(),
                }
            })),
        )
            .into_response();
    }
    builder
        .body(Body::from_stream(upstream_resp.bytes_stream()))
        .unwrap()
        .into_response()
}

async fn ensure_root_run(st: &AppState, account_name: &str, token: &str) -> anyhow::Result<String> {
    // 简化：每个账号惰性建根 run 并缓存（进程内 Arc<Mutex> 缓存方案由后续打补）
    // 此处直接新建根 run，避免首次建 run 的复杂度
    let run_id = st
        .client
        .start_run(token, crate::models::ROOT_AGENT_ID, &[])
        .await?;
    st.pool.update_score(account_name, 5.0).await;
    Ok(run_id)
}

// ---------- Anthropic 兼容 ----------

async fn handle_claude_messages(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "type": "error", "error": { "type": "invalid_request_error", "message": "invalid json" } })),
            )
                .into_response();
        }
    };

    if !st.cfg.api_keys.is_empty() && !authorized(&headers, &st.cfg.api_keys) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "type": "error", "error": { "type": "authentication_error", "message": "invalid proxy api key" } })),
        )
            .into_response();
    }

    let model = parsed.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
    if model.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "type": "error", "error": { "type": "invalid_request_error", "message": "model is required" } })),
        )
            .into_response();
    }
    let resolved = st.router.resolve(&model).await;

    let account = match st.pool.pick_best().await {
        Some(a) => a,
        None => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "type": "error", "error": { "type": "api_error", "message": "no healthy token" } })),
            )
                .into_response();
        }
    };
    let token = account.token.clone();
    let _ = account.session.ensure_session(&resolved).await;

    // 简化 Claude 协议：转为 OpenAI 请求体转发
    let mut up_body = serde_json::json!({
        "model": resolved,
        "messages": parsed.get("messages").cloned().unwrap_or(serde_json::json!([])),
        "stream": parsed.get("stream").and_then(|v| v.as_bool()).unwrap_or(false),
    });
    if let Some(maxt) = parsed.get("max_tokens").and_then(|v| v.as_u64()) {
        up_body["max_tokens"] = serde_json::json!(maxt);
    }
    // 系统提示 → 首条 user
    if let Some(sys) = parsed.get("system") {
        let mut msgs = up_body["messages"].as_array().cloned().unwrap_or_default();
        msgs.insert(0, serde_json::json!({ "role": "system", "content": sys }));
        up_body["messages"] = serde_json::json!(msgs);
    }

    let run_id = match ensure_root_run(&st, &account.name, &token).await {
        Ok(id) => id,
        Err(_) => "no-run".into(),
    };

    let upstream_resp = match st
        .client
        .chat_completions(&token, up_body, &run_id, None)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "type": "error", "error": { "type": "api_error", "message": e.to_string() } })),
            )
                .into_response();
        }
    };

    let status = upstream_resp.status();
    let mut builder = Response::builder().status(status);
    for (k, v) in upstream_resp.headers() {
        if k != "content-length" && k != "transfer-encoding" {
            builder = builder.header(k, v);
        }
    }
    builder
        .body(Body::from_stream(upstream_resp.bytes_stream()))
        .unwrap()
        .into_response()
}

// ---------- 工具 ----------

fn authorized(headers: &HeaderMap, keys: &[String]) -> bool {
    let extract = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(|s| s.to_string());
    if let Some(k) = extract("x-api-key") {
        if keys.contains(&k) {
            return true;
        }
    }
    if let Some(auth) = extract("authorization") {
        let bearer = auth
            .strip_prefix("Bearer ")
            .or_else(|| auth.strip_prefix("bearer "))
            .map(|s| s.to_string());
        if let Some(k) = bearer {
            return keys.contains(&k);
        }
    }
    false
}

/// 上游不强需的字段预处理（去掉 stream 外的自定义字段避免污染）
fn remove_passthrough_fields(body: &mut serde_json::Value) {
    if let Some(obj) = body.as_object_mut() {
        obj.remove("stream_options");
    }
}
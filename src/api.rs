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
use crate::logbus::LogBus;
use crate::models::ModelRegistry;
use crate::pool::Pool;
use crate::router::ModelRouter;
use crate::skills::SkillsManager;
use crate::telemetry::{TelemetryWriter, TraceRow};

use crate::upstream::UpstreamClient;
use crate::usage::UsageDb;
use axum::body::Body;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
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
    pub telemetry: Arc<TelemetryWriter>,
    pub logs: Arc<LogBus>,
    pub memory: Arc<crate::memory::MemoryStore>,
    pub skills: Arc<SkillsManager>,
    pub ads: Arc<AdRefresher>,
    pub prompts: Arc<crate::prompts::PromptManager>,
    /// 凭证账号信息缓存 + 使用记录
    pub meta: Arc<crate::account_meta::AccountMetaStore>,
    /// 当前生效的下游 API Key（支持运行时热更新，见 `POST /api/config/api-key`）
    pub api_keys: Arc<std::sync::RwLock<Vec<String>>>,
    /// web 协议桥接：客户端会话 → 上游 thread 绑定（复用会话省每日额度）
    pub web_threads: Arc<crate::web_threads::WebThreadMap>,
    pub started: std::time::Instant,
}

/// 读取当前生效的下游 API Key 列表（运行时热更新，不重启即生效）
fn api_keys_of(st: &AppState) -> Vec<String> {
    st.api_keys.read().map(|k| k.clone()).unwrap_or_default()
}

/// 写入运行时 API Key 列表（同时持久化到 config.json 由调用方负责）
fn set_api_keys(st: &AppState, keys: Vec<String>) {
    if let Ok(mut w) = st.api_keys.write() {
        *w = keys;
    }
}

// axum_core 已对 `S: Clone` 提供 blanket impl FromRef<S> for S，此处无需手动实现。
// 保留 FromRef 导入供后续扩展（若需要子状态 FromRef 时使用）。

/// 管理端点鉴权：配置了 api_keys 则要求请求头匹配；未配置则仅放行本机直连（无 X-Forwarded-For/X-Real-IP）。
/// 本地单机软件默认监听 127.0.0.1，行为不变；跨机/代理访问必须显式配置 api_keys。
/// 额外 CSRF 防护：带 Origin 头（浏览器发起）且非同源本机时拒绝。
fn admin_authorized(headers: &HeaderMap, st: &AppState) -> bool {
    if !origin_allowed(headers) {
        return false;
    }
    let keys = api_keys_of(st);
    if keys.is_empty() {
        return is_loopback_request(headers);
    }
    authorized(headers, &keys)
}

/// CSRF 防护：浏览器跨站请求会带 Origin；非同源（非本机面板）一律拒绝。
/// 非浏览器客户端（curl/SDK/桌面 IPC）不带 Origin，不受影响。
///
/// 例外：浏览器扩展（`chrome-extension://` / `moz-extension://`）—— 扩展是"浏览器版一键登录"
/// 的唯一合法路径（HttpOnly Cookie 只能由扩展读取），其 fetch 会带扩展 Origin。
/// 配置了 `api_keys` 时扩展仍需携带 Key（面板会把 Key 透传给扩展），安全边界不变。
fn origin_allowed(headers: &HeaderMap) -> bool {
    match headers.get("origin").and_then(|v| v.to_str().ok()) {
        None => true,
        Some(o) => {
            o.starts_with("http://127.0.0.1:")
                || o.starts_with("http://localhost:")
                || o == "http://127.0.0.1"
                || o == "http://localhost"
                || o.starts_with("file://")
                || o.starts_with("chrome-extension://")
                || o.starts_with("moz-extension://")
        }
    }
}

/// 判断请求是否来自本机：无任何代理头 = 直连本机（环回）；出现 X-Forwarded-For 等视为经代理/跨机
fn is_loopback_request(headers: &HeaderMap) -> bool {
    headers.get("x-forwarded-for").is_none() && headers.get("x-real-ip").is_none()
}

pub fn build_router(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/usage/totals", get(handle_usage_totals))
        .route("/api/usage/daily", get(handle_usage_daily))
        .route("/api/usage/requests", get(handle_usage_requests))
        .route("/api/usage/requests/{id}", get(handle_usage_request_detail))
        .route("/api/usage/models", get(handle_usage_models))
        .route("/api/usage/cost", get(handle_usage_cost))
        .route("/api/usage/accounts", get(handle_accounts))
        .route("/api/skills", get(handle_skills_list).post(handle_skills_upsert))
        .route("/api/skills/toggle", post(handle_skills_toggle))
        .route("/api/skills/delete", post(handle_skills_delete))
        .route("/api/skills/gate", post(handle_skills_gate))
        .route("/api/logs/recent", get(handle_logs_recent))
        .route("/api/logs/stream", get(handle_logs_stream))
        .route("/api/memory", get(handle_memory_list).post(handle_memory_upsert))
        .route("/api/memory/delete", post(handle_memory_delete))
        .route("/api/memory/static", post(handle_memory_static))
        .route("/api/threads/cleanup", post(handle_threads_cleanup))
        .route("/mcp", post(handle_mcp))
        .route("/api/doctor", get(handle_doctor));

    Router::new()
        .route("/", get(handle_dashboard))
        .route("/ui", get(handle_dashboard))
        .route("/healthz", get(handle_healthz))
        .route("/v1/models", get(handle_v1_models))
        .route("/v1/chat/completions", post(handle_chat_completions))
        .route("/v1/messages", post(handle_claude_messages))
        .route("/api/tokens/import", post(handle_token_import))
        .route("/api/tokens", get(handle_tokens_list))
        .route("/api/tokens/check", post(handle_token_check))
        .route("/api/tokens/delete", post(handle_token_delete))
        .route("/api/account/balance", get(handle_account_balance))
        .route("/api/account/detail", post(handle_account_detail))
        .route("/api/account/overview", get(handle_account_overview))
        .route("/api/account/refresh", post(handle_account_refresh))
        .route("/api/account/history", get(handle_account_history))
        .route("/api/guide", get(handle_guide))
        .route("/api/login/embed", post(handle_login_embed))
        .route("/api/login/result", get(handle_login_result))
        .route("/api/extension/bundle", get(handle_extension_bundle))
        .route("/api/config/api-key", post(handle_config_api_key))
        .route("/v1/web/chat", post(handle_web_chat))
        .route(
            "/v1/uploads",
            post(handle_upload).layer(axum::extract::DefaultBodyLimit::max(20 * 1024 * 1024)),
        )
        .route("/api/prompts", get(handle_prompts_list))
        .route("/api/prompts/toggle", post(handle_prompts_toggle))
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

async fn handle_healthz(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let dur = st.started.elapsed();
    // 配置了 api_keys 且未通过校验时，只回存活信息——账号名/模型构成不该对未授权方可见
    let keys = api_keys_of(&st);
    let authorized_ok = if keys.is_empty() { is_loopback_request(&headers) } else { authorized(&headers, &keys) };
    if !authorized_ok {
        return Json(serde_json::json!({
            "ok": true,
            "uptime_sec": dur.as_secs(),
            "version": env!("CARGO_PKG_VERSION"),
        }))
        .into_response();
    }
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

async fn handle_v1_models(State(st): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    // 与数据面同一套防线：配置 api_keys 时必须校验（模型清单不对未授权方公开）；
    // 未配置时仅本机直连。
    let keys = api_keys_of(&st);
    let authorized_ok = if keys.is_empty() {
        is_loopback_request(&headers)
    } else {
        authorized(&headers, &keys)
    };
    if !authorized_ok {
        return admin_denied().into_response();
    }
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
    Json(serde_json::json!({ "object": "list", "data": data })).into_response()
}

// ---------- 用量 API ----------

async fn handle_usage_totals(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !admin_authorized(&headers, &st) {
        return admin_denied();
    }
    match st.usage.totals() {
        Ok(v) => Json(v).into_response(),
        Err(e) => internal_err(&e),
    }
}

async fn handle_usage_daily(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !admin_authorized(&headers, &st) {
        return admin_denied();
    }
    match st.usage.daily_usage(7) {
        Ok(v) => Json(serde_json::json!(v)).into_response(),
        Err(e) => internal_err(&e),
    }
}

async fn handle_usage_requests(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !admin_authorized(&headers, &st) {
        return admin_denied();
    }
    match st.usage.recent_requests(50) {
        Ok(v) => Json(serde_json::json!(v)).into_response(),
        Err(e) => internal_err(&e),
    }
}

// ---------- 账号余额查询（web 版协议） ----------

/// GET /api/account/balance — 查询账号积分/每模型限额/套餐（用 web Cookie）
async fn handle_account_balance(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !admin_authorized(&headers, &st) {
        return admin_denied();
    }
    // 候选：先找看起来像 Cookie 的 token（含 session-token），config 优先，其次导入库
    let looks_like_cookie = |t: &str| t.contains("session-token") || t.contains(".next-auth") || t.contains("callback-url") || t.contains("%3A");
    let cookie_candidate = st
        .cfg
        .auth_tokens
        .iter()
        .find(|t| looks_like_cookie(t))
        .cloned()
        .or_else(|| load_imported_token(&st.cfg.tokens_path).filter(|t| looks_like_cookie(t)));
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

/// 读取导入凭证文件中第一个 token（路径可配置）
fn load_imported_token(path: &str) -> Option<String> {
    crate::import::load_tokens(path).ok().and_then(|t| t.into_iter().next()).map(|t| t.token)
}
/// 统一 500 JSON 错误响应
/// GET /api/prompts — 列出内置提示词与技能（含启用状态）
async fn handle_prompts_list(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !admin_authorized(&headers, &st) {
        return admin_denied();
    }
    Json(serde_json::json!({
        "ok": true,
        "prompts": st.prompts.prompts_snapshot().await,
        "skills": st.prompts.skills_snapshot().await,
        "system_prefix_preview": st.prompts.system_prefix().await,
    }))
    .into_response()
}

/// POST /api/prompts/toggle — body: { type: "prompt"|"skill", id, enabled }
async fn handle_prompts_toggle(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if let Some(resp) = write_guard(&headers, &st) {
        return resp;
    }
    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "ok": false, "message": "invalid json" }))).into_response(),
    };
    let kind = parsed.get("type").and_then(|v| v.as_str()).unwrap_or("prompt");
    let id = parsed.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let enabled = parsed.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
    let ok = if kind == "skill" {
        st.prompts.set_skill_enabled(id, enabled).await
    } else {
        st.prompts.set_prompt_enabled(id, enabled).await
    };
    Json(serde_json::json!({
        "ok": ok,
        "kind": kind,
        "id": id,
        "enabled": enabled,
        "system_prefix_preview": st.prompts.system_prefix().await,
    }))
    .into_response()
}

fn internal_err(e: &anyhow::Error) -> Response {
    tracing::error!("用量统计查询失败: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))).into_response()
}

/// 管理端点鉴权失败统一响应
fn admin_denied() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "ok": false, "message": "unauthorized: 管理端点需要 API key（config.json api_keys），本地未配置时仅本机可访问" })),
    )
        .into_response()
}

/// web 版完整对话代理（Cookie 鉴权 chat/stream → 真实增量 SSE 透传 → OpenAI 兼容）
/// POST /v1/web/chat — body: { model, content, thread_id?, images? }
/// images 支持两种形态：["storageId1", ...] 或 [{ storageId, mediaType?, name? }, ...]
/// 鉴权与 /v1/chat/completions 一致：配置 api_keys 时校验；未配置时仅本机可访问。
async fn handle_web_chat(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if !origin_allowed(&headers) {
        return admin_denied();
    }
    let keys = api_keys_of(&st);
    if !keys.is_empty() && !authorized(&headers, &keys) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": { "message": "invalid proxy api key", "type": "authentication_error" } })),
        )
            .into_response();
    }
    if keys.is_empty() && !is_loopback_request(&headers) {
        return admin_denied();
    }
    let cookie = load_imported_token(&st.cfg.tokens_path).filter(|t| t.contains("session-token"));
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
    // 多模态：解析 images（storageId 字符串数组或对象数组）
    let images: Vec<crate::web_protocol::WebImage> = parsed
        .get("images")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|it| {
                    if let Some(s) = it.as_str() {
                        Some(crate::web_protocol::WebImage {
                            storage_id: s.to_string(),
                            media_type: "image/png".into(),
                            name: "image.png".into(),
                            description_storage_id: None,
                        })
                    } else {
                        let sid = it.get("storageId").and_then(|x| x.as_str())?;
                        Some(crate::web_protocol::WebImage {
                            storage_id: sid.to_string(),
                            media_type: it.get("mediaType").and_then(|x| x.as_str()).unwrap_or("image/png").to_string(),
                            name: it.get("name").and_then(|x| x.as_str()).unwrap_or("image.png").to_string(),
                            description_storage_id: it.get("descriptionStorageId").and_then(|x| x.as_str()).map(String::from),
                        })
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    if content.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": { "message": "content is required", "type": "invalid_request_error" } })),
        )
            .into_response();
    }

    let images_count = images.len();
    // 文档/文件附件（attachments）：storageId 字符串或对象数组
    let attachments: Vec<crate::web_protocol::WebAttachment> = parsed
        .get("attachments")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|it| {
                    if let Some(s) = it.as_str() {
                        Some(crate::web_protocol::WebAttachment {
                            storage_id: s.to_string(),
                            media_type: "application/octet-stream".into(),
                            name: "file".into(),
                            chars: None,
                            truncated: None,
                        })
                    } else {
                        let sid = it.get("storageId").and_then(|x| x.as_str())?;
                        Some(crate::web_protocol::WebAttachment {
                            storage_id: sid.to_string(),
                            media_type: it.get("mediaType").and_then(|x| x.as_str()).unwrap_or("application/octet-stream").to_string(),
                            name: it.get("name").and_then(|x| x.as_str()).unwrap_or("file").to_string(),
                            chars: it.get("chars").and_then(|x| x.as_i64()),
                            truncated: it.get("truncated").and_then(|x| x.as_bool()),
                        })
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let attachments_count = attachments.len();
    let model_name = model.clone();
    let client = match crate::web_protocol::WebClient::new(cookie, model) {
        Ok(c) => c,
        Err(e) => return internal_err(&anyhow::Error::msg(e.to_string())),
    };
    // 真实增量流：上游 chat/stream SSE 逐事件实时转发为 OpenAI chunk（images 透传多模态）
    match client.chat_stream_raw(thread_id.as_deref(), &content, reasoning_effort, images, attachments).await {
        Ok(body) => {
            // 遥测：旁路扫描流（首字节/字节数/usage）+ 完成后上报（web 协议用 Cookie，标记 web-cookie）
            let req_id = uuid::Uuid::new_v4().to_string();
            let (tx, rx) = tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(16);
            let telem = st.telemetry.clone();
            let logs = st.logs.clone();
            let usage_db = st.usage.clone();
            let client_track = client.clone();
            let threads_path = st.cfg.threads_path.clone();
            tokio::spawn(async move {
                let t0 = std::time::Instant::now();
                let mut stream = body.into_data_stream();
                let mut ttft: Option<u64> = None;
                let mut bytes: u64 = 0;
                let mut tail = String::new();
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(b) => {
                            if ttft.is_none() {
                                ttft = Some(t0.elapsed().as_millis() as u64);
                            }
                            bytes += b.len() as u64;
                            tail.push_str(&String::from_utf8_lossy(&b));
                            if tail.len() > 8000 {
                                tail = tail_keep(&tail, 4000);
                            }
                            if tx.send(Ok(b)).await.is_err() {
                                break; // 客户端断开
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Err(std::io::Error::other(e.to_string()))).await;
                            break;
                        }
                    }
                }
                let total_ms = t0.elapsed().as_millis() as u64;
                let (pt, ct) = extract_usage(&tail).unwrap_or((0, 0));
                usage_db
                    .record_ex("web-cookie", &model_name, pt as i64, ct as i64, total_ms as i64, 200, "", "", &req_id)
                    .ok();
                telem.record(TraceRow {
                    req_id: req_id.clone(),
                    endpoint: "/v1/web/chat".into(),
                    requested_model: model_name.clone(),
                    resolved_model: model_name.clone(),
                    account: "web-cookie".into(),
                    status: 200,
                    latency_ms: total_ms,
                    ttft_ms: ttft,
                    prompt_tokens: pt,
                    completion_tokens: ct,
                    stream: true,
                    error_kind: None,
                    error_excerpt: None,
                    route_reason: Some(format!("web protocol, images={images_count}, attachments={attachments_count}")),
                    api_key: None,
                    client_ip: None,
                });
                logs.emit(
                    "info",
                    "request",
                    Some(&req_id),
                    format!(
                        "{model_name} web 流式完成 {bytes}B / {images_count} 图（首字节 {}ms / 总 {:.2}s）",
                        ttft.unwrap_or(0),
                        total_ms as f64 / 1000.0
                    ),
                );
                // 记录上游 threadId（供会话清理，防止长期堆积给上游制造压力/暴露指纹）
                if let Some(tid) = client_track.last_thread_id() {
                    record_thread(&threads_path, &tid);
                }
            });
            let body_stream = futures::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|item| (item, rx))
            });
            Response::builder()
                .header("content-type", "text/event-stream")
                .header("cache-control", "no-cache")
                .header("connection", "keep-alive")
                .header("x-accel-buffering", "no")
                .body(Body::from_stream(body_stream))
                .unwrap()
        }
        Err(e) => internal_err(&e),
    }
}

async fn handle_usage_models(State(st): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if !admin_authorized(&headers, &st) {
        return admin_denied();
    }
    Json(serde_json::json!(st.registry.models().await)).into_response()
}

// ---------- Web 协议桥接（OpenAI/Anthropic 客户端 → freebuff.com /api/chat/stream） ----------

/// 桥接决策：复用既有 thread 还是新开一个
enum BridgeMode {
    /// 续聊：复用绑定的 thread，只发最后一条用户消息
    Continue { thread_id: String },
    /// 新会话：发完整上下文（摊平后的 transcript）
    Fresh { prompt: String },
}

/// 按会话绑定决定本轮怎么发。
/// 启发式：客户端 messages 里出现 assistant 消息 = 多轮对话的后续轮次，且已有绑定 → 复用 thread 只发增量；
/// 否则新开会话发全文。为什么必须复用：上游每日按**会话准入**计数（rateLimitsByModel.limit，免费 6 次/天），
/// 每请求都开新 thread 会迅速烧光额度——这是"照指南填 /v1 却很快 429"的直接原因。
fn decide_bridge_mode(
    map: &crate::web_threads::WebThreadMap,
    cred_id: &str,
    messages: &serde_json::Value,
) -> Option<(BridgeMode, String)> {
    let full = crate::web_threads::flatten_messages(messages, false)?;
    let last_user = crate::web_threads::flatten_messages(messages, true).map(|(_, l)| l)?;
    let has_assistant = messages
        .as_array()
        .map(|a| a.iter().any(|m| m.get("role").and_then(|r| r.as_str()) == Some("assistant")))
        .unwrap_or(false);
    if has_assistant {
        if let Some(b) = map.get(cred_id) {
            if !b.thread_id.is_empty() {
                return Some((BridgeMode::Continue { thread_id: b.thread_id }, last_user));
            }
        }
    }
    Some((BridgeMode::Fresh { prompt: full.0 }, last_user))
}

/// OpenAI `/v1/chat/completions` 桥接到 web 协议（账号池为空、仅有 web Cookie 时启用）。
/// `parsed` 是原始请求体（含 messages/stream），`model` 已过路由解析。
async fn web_bridge_openai(
    st: AppState,
    parsed: serde_json::Value,
    requested: String,
    model: String,
    cookie: String,
    cred: serde_json::Value,
    cid: String,
) -> Response {
    let start = std::time::Instant::now();
    let req_id = uuid::Uuid::new_v4().to_string();
    let stream = parsed.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
    let messages = parsed.get("messages").cloned().unwrap_or_else(|| serde_json::json!([]));

    // 记忆/技能注入（与桌面协议同管线），再摊平为 web 协议 content
    let mut effective = messages.clone();
    let mem_query: String = crate::web_threads::flatten_messages(&messages, true)
        .map(|(_, l)| l.chars().take(200).collect())
        .unwrap_or_default();
    let sys_prefix = build_system_prefix(&st, &mem_query).await;
    if !sys_prefix.trim().is_empty() {
        if let Some(arr) = effective.as_array_mut() {
            arr.insert(0, serde_json::json!({ "role": "system", "content": sys_prefix }));
        }
    }

    let (mode, last_user) = match decide_bridge_mode(&st.web_threads, &cid, &effective) {
        Some(v) => v,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": { "message": "messages 里没有可发送的用户内容", "type": "invalid_request_error" } })),
            )
                .into_response();
        }
    };

    let client = match crate::web_protocol::WebClient::new(cookie, model.clone()) {
        Ok(c) => c,
        Err(e) => return internal_err(&anyhow::Error::msg(e.to_string())),
    };
    let (thread_id, content, reason) = match &mode {
        BridgeMode::Continue { thread_id } => (Some(thread_id.clone()), last_user.clone(), "web bridge, reuse thread".to_string()),
        BridgeMode::Fresh { prompt } => (None, prompt.clone(), "web bridge, new thread".to_string()),
    };
    st.telemetry.event(
        &req_id,
        "route",
        &format!(
            "{requested} -> {model} via web-cookie({}) [{}]",
            cred.get("token_masked").and_then(|v| v.as_str()).unwrap_or("?"),
            match &mode {
                BridgeMode::Continue { thread_id } => format!("continue {thread_id}"),
                BridgeMode::Fresh { .. } => "new thread".into(),
            }
        ),
    );

    let upstream = client
        .chat_stream_raw(thread_id.as_deref(), &content, None, Vec::new(), Vec::new())
        .await;
    let body = match upstream {
        Ok(b) => b,
        Err(e) => {
            // 绑定的 thread 可能已被上游清理 → 清绑定，客户端重试即自动新开会话
            if matches!(mode, BridgeMode::Continue { .. }) {
                let _ = st.web_threads.clear(&cid);
                st.telemetry.event(&req_id, "thread_reset", &e.to_string());
            }
            let ms = start.elapsed().as_millis() as i64;
            st.usage.record_ex("web-cookie", &model, 0, 0, ms, 502, "upstream_5xx", &e.to_string(), &req_id).ok();
            st.telemetry.record(TraceRow {
                req_id: req_id.clone(),
                endpoint: "/v1/chat/completions".into(),
                requested_model: requested.clone(),
                resolved_model: model.clone(),
                account: "web-cookie".into(),
                status: 502,
                latency_ms: ms as u64,
                ttft_ms: None,
                prompt_tokens: 0,
                completion_tokens: 0,
                stream,
                error_kind: Some("upstream_5xx".into()),
                error_excerpt: Some(e.to_string()),
                route_reason: Some(reason),
                api_key: None,
                client_ip: None,
            });
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": { "message": format!("上游 web 协议请求失败：{e}"), "type": "server_error" } })),
            )
                .into_response();
        }
    };

    // 旁路扫描：转发字节 + 绑定 threadId + usage 统计（与非桥接路径同一套遥测）
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(16);
    let telem = st.telemetry.clone();
    let usage_db = st.usage.clone();
    let client_track = client.clone();
    let threads_path = st.cfg.threads_path.clone();
    let web_threads = st.web_threads.clone();
    let cid_track = cid.clone();
    let bound_track = last_user.clone();
    let model_track = model.clone();
    let requested_track = requested.clone();
    let req_id_track = req_id.clone();
    tokio::spawn(async move {
        let mut stream_ = body.into_data_stream();
        let mut ttft: Option<u64> = None;
        let mut bytes: u64 = 0;
        let mut tail = String::new();
        while let Some(chunk) = stream_.next().await {
            match chunk {
                Ok(b) => {
                    if ttft.is_none() {
                        ttft = Some(start.elapsed().as_millis() as u64);
                    }
                    bytes += b.len() as u64;
                    tail.push_str(&String::from_utf8_lossy(&b));
                    if tail.len() > 8000 {
                        tail = tail_keep(&tail, 4000);
                    }
                    if tx.send(Ok(b)).await.is_err() {
                        break; // 客户端断开
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(std::io::Error::other(e.to_string()))).await;
                    break;
                }
            }
        }
        let total_ms = start.elapsed().as_millis() as u64;
        // threadId 绑定：供续聊复用（省每日会话额度）；同时进全局清理清单
        if let Some(tid) = client_track.last_thread_id() {
            let _ = web_threads.bind(&cid_track, &tid, &bound_track);
            record_thread(&threads_path, &tid);
        }
        let (pt, ct) = extract_usage(&tail).unwrap_or((0, 0));
        // 桥接流内嵌错误识别（Critic-J P2-1 闭环）：优先用 StreamEncoder 旁路槽
        // （能看到上游原始 error envelope），tail 扫描 detect_bridge_error 作兜底。
        let bypass_error = client_track.last_upstream_error();
        let bridge_kind = bypass_error
            .as_deref()
            .map(|e| crate::errors::classify(200, e).as_str().to_string())
            .or_else(|| detect_bridge_error(&tail).map(|s| s.to_string()));
        let bridge_kind: Option<&str> = bridge_kind.as_deref();
        let status: i64 = if bridge_kind.is_some() { 502 } else { 200 };
        usage_db
            .record_ex(
                "web-cookie",
                &model_track,
                pt as i64,
                ct as i64,
                total_ms as i64,
                status,
                bridge_kind.unwrap_or(""),
                bridge_kind.map(|k| format!("上游桥接流内嵌错误（{k}）：凭证可能失效或额度耗尽")).as_deref().unwrap_or(""),
                &req_id_track,
            )
            .ok();
        telem.record(TraceRow {
            req_id: req_id_track.clone(),
            endpoint: "/v1/chat/completions".into(),
            requested_model: requested_track,
            resolved_model: model_track.clone(),
            account: "web-cookie".into(),
            status: status as u16,
            latency_ms: total_ms,
            ttft_ms: ttft,
            prompt_tokens: pt,
            completion_tokens: ct,
            stream: true,
            error_kind: bridge_kind.map(|k| k.to_string()),
            error_excerpt: bridge_kind.map(|_| tail_keep(&tail, 300)),
            route_reason: Some(reason),
            api_key: None,
            client_ip: None,
        });
        st_logs_done(&telem, &req_id_track, &model_track, bytes, total_ms as u128);
    });

    let body_stream = futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|item| (item, rx)) });
    if !stream {
        return aggregate_bridge_sse(Box::pin(body_stream), &model, &req_id).await;
    }
    Response::builder()
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .header("connection", "keep-alive")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(body_stream))
        .unwrap()
}

/// 桥接路径完成日志（后台任务收尾时调用，避免闭包捕获整个 AppState）
fn st_logs_done(telem: &TelemetryWriter, req_id: &str, model: &str, bytes: u64, total_ms: u128) {
    telem.event(req_id, "done", &format!("web bridge {model}：{bytes}B / {total_ms}ms"));
}

/// 识别桥接 SSE 流中的上游内嵌错误（200 内嵌 error envelope）。
/// 返回 Some(kind) 表示流里出现了错误（对齐 `errors::classify` 的 kind 命名）。
fn detect_bridge_error(tail: &str) -> Option<&'static str> {
    // 快路径：无引号 error 字样直接放行（避免对每行做 JSON 解析）
    if !tail.contains("\"error\"") {
        return None;
    }
    for line in tail.lines() {
        let Some(data) = line.strip_prefix("data: ") else { continue };
        let data = data.trim();
        if data == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else { continue };
        if let Some(err) = v.get("error") {
            let body_text = err.as_str().map(String::from).unwrap_or_else(|| err.to_string());
            let kind = crate::errors::classify(200, &body_text);
            return Some(kind.as_str());
        }
    }
    None
}

/// 桥接响应的 OpenAI→Claude 形状适配（/v1/messages 桥接路径用）。
/// - 非流式 JSON：直接用现有 `openai_to_claude_response` 转换
/// - SSE 流：实时把 OpenAI chunk 流转成 Anthropic 事件流（message_start → content_block_delta → message_stop）
/// - 错误 JSON：转 Claude error 形状
async fn openai_error_to_claude(resp: Response, model: &str) -> Response {
    let (mut parts, body) = resp.into_parts();
    if !model.is_empty() {
        parts.headers.insert(
            axum::http::HeaderName::from_static("x-bridge-model"),
            axum::http::HeaderValue::from_str(model).unwrap_or(axum::http::HeaderValue::from_static("")),
        );
    }
    let ct = parts
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let status = parts.status;

    // SSE：整条收完再一次性转换（桥接流通常几秒内结束，聚合实现简单且正确；
    // chunk 级实时转换留作后续优化）
    if ct.contains("text/event-stream") {
        let bytes = axum::body::to_bytes(body, 8 * 1024 * 1024).await.unwrap_or_default();
        let raw = String::from_utf8_lossy(&bytes);
        let (text, mut model, finish, usage) = collect_openai_stream(&raw);
        if model.is_empty() {
            model = parts
                .headers
                .get("x-bridge-model")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
        }
        let events = claude_stream_events(&text, &model, finish.as_deref(), usage.as_ref());
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream; charset=utf-8")
            .header("cache-control", "no-cache")
            .body(Body::from(events))
            .unwrap(); // model 兜底已从 x-bridge-model 头读取
    }

    // JSON（成功或错误）
    let bytes = axum::body::to_bytes(body, 8 * 1024 * 1024).await.unwrap_or_default();
    let v: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "type": "error", "error": { "type": "api_error", "message": "桥接响应不是合法 JSON" } })),
            )
                .into_response();
        }
    };
    if let Some(err) = v.get("error") {
        let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("上游错误");
        let code = if status == StatusCode::UNAUTHORIZED { "authentication_error" } else { "api_error" };
        return (
            status,
            Json(serde_json::json!({ "type": "error", "error": { "type": code, "message": msg } })),
        )
            .into_response();
    }
    let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
    Json(openai_to_claude_response(&v, &model)).into_response()
}

/// 从聚合好的 OpenAI SSE 文本抽取（正文, 模型, finish_reason, usage）
fn collect_openai_stream(raw: &str) -> (String, String, Option<String>, Option<serde_json::Value>) {
    let mut text = String::new();
    let mut model = String::new();
    let mut finish = None;
    let mut usage = None;
    for line in raw.lines() {
        let Some(data) = line.strip_prefix("data: ") else { continue };
        let data = data.trim();
        if data == "[DONE]" {
            break;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else { continue };
        if model.is_empty() {
            model = v.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
        }
        if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
            if let Some(c0) = choices.first() {
                if let Some(d) = c0.get("delta").and_then(|d| d.get("content")).and_then(|t| t.as_str()) {
                    text.push_str(d);
                }
                if let Some(fr) = c0.get("finish_reason").and_then(|f| f.as_str()) {
                    finish = Some(fr.to_string());
                }
            }
        }
        if v.get("usage").is_some_and(|u| u.is_object()) {
            usage = v.get("usage").cloned();
        }
    }
    (text, model, finish, usage)
}

/// 生成完整的 Anthropic 事件流文本（message_start → content_block_delta×N → message_stop）
fn claude_stream_events(
    text: &str,
    model: &str,
    finish_reason: Option<&str>,
    usage: Option<&serde_json::Value>,
) -> String {
    let (input_tokens, output_tokens) = usage
        .map(|u| {
            (
                u.get("prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
                u.get("completion_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
            )
        })
        .unwrap_or((0, 0));
    let mut out = String::new();
    let mut send = |event: &str, data: serde_json::Value| {
        out.push_str("event: ");
        out.push_str(event);
        out.push_str("\ndata: ");
        out.push_str(&serde_json::to_string(&data).unwrap_or_else(|_| "{}".into()));
        out.push_str("\n\n");
    };
    send(
        "message_start",
        serde_json::json!({
            "type": "message_start",
            "message": {
                "id": format!("msg_webbridge_{}", uuid::Uuid::new_v4().simple()),
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [],
                "stop_reason": serde_json::Value::Null,
                "usage": { "input_tokens": input_tokens, "output_tokens": 0 },
            },
        }),
    );
    send("content_block_start", serde_json::json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }));
    // 按 512 字节切片推 delta，保持"流式"语义（长回答不至于一次性砸给客户端）
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let mut end = (i + 512).min(bytes.len());
        // 不切断多字节字符
        while end < bytes.len() && (bytes[end] & 0xC0) == 0x80 {
            end += 1;
        }
        let piece = String::from_utf8_lossy(&bytes[i..end]);
        send("content_block_delta", serde_json::json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": piece } }));
        i = end;
    }
    send("content_block_stop", serde_json::json!({ "type": "content_block_stop", "index": 0 }));
    let stop_reason = match finish_reason {
        Some("tool_calls") => "tool_use",
        Some("length") => "max_tokens",
        _ => "end_turn",
    };
    send(
        "message_delta",
        serde_json::json!({
            "type": "message_delta",
            "delta": { "stop_reason": stop_reason, "stop_sequence": serde_json::Value::Null },
            "usage": { "output_tokens": output_tokens },
        }),
    );
    send("message_stop", serde_json::json!({ "type": "message_stop" }));
    out
}

/// 把上游 SSE（OpenAI chunk 文本）聚合为单个非流式 OpenAI 响应。
/// 拼回 `data: {...}` 行里的 delta，攒成完整 content；失败（上游 JSON 错误）转为 502。
async fn aggregate_bridge_sse(
    mut stream: impl futures::Stream<Item = Result<axum::body::Bytes, std::io::Error>> + Unpin,
    model: &str,
    req_id: &str,
) -> Response {
    let mut raw = String::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(b) => raw.push_str(&String::from_utf8_lossy(&b)),
            Err(e) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "error": { "message": format!("上游流中断：{e}"), "type": "server_error" } })),
                )
                    .into_response();
            }
        }
    }
    let mut content = String::new();
    let mut finish_reason: Option<String> = None;
    let mut usage: Option<serde_json::Value> = None;
    for line in raw.lines() {
        let Some(data) = line.strip_prefix("data: ") else { continue };
        let data = data.trim();
        if data == "[DONE]" {
            break;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else { continue };
        if let Some(err) = v.get("error") {
            let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("上游错误");
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": { "message": msg, "type": "server_error" } })),
            )
                .into_response();
        }
        if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
            if let Some(c0) = choices.first() {
                if let Some(d) = c0.get("delta").and_then(|d| d.get("content")).and_then(|t| t.as_str()) {
                    content.push_str(d);
                }
                if let Some(fr) = c0.get("finish_reason").and_then(|f| f.as_str()) {
                    finish_reason = Some(fr.to_string());
                }
            }
        }
        if v.get("usage").is_some_and(|u| u.is_object()) {
            usage = v.get("usage").cloned();
        }
    }
    if content.is_empty() && finish_reason.is_none() {
        return (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": { "message": "上游未返回任何内容（凭证可能已失效，请在面板重新登录）", "type": "server_error" } })),
        )
            .into_response();
    }
    let (pt, ct) = usage
        .as_ref()
        .map(|u| {
            (
                u.get("prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
                u.get("completion_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
            )
        })
        .unwrap_or((0, 0));
    let mut resp = serde_json::json!({
        "id": format!("chatcmpl-webbridge-{}", &req_id[..8.min(req_id.len())]),
        "object": "chat.completion",
        "created": chrono::Utc::now().timestamp(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": content },
            "finish_reason": finish_reason.unwrap_or_else(|| "stop".into()),
        }],
    });
    if let Some(u) = usage {
        resp["usage"] = u;
    } else {
        resp["usage"] = serde_json::json!({ "prompt_tokens": pt, "completion_tokens": ct, "total_tokens": pt + ct });
    }
    Json(resp).into_response()
}

async fn handle_accounts(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !admin_authorized(&headers, &st) {
        return admin_denied();
    }
    Json(st.pool.snapshot().await).into_response()
}

// ---------- Token 导入（curl / HAR 自动解析入库） ----------

/// POST /api/tokens/import — body 传 curl 命令文本或 HAR JSON，自动提取 Bearer token 入库
async fn handle_token_import(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if let Some(resp) = write_guard(&headers, &st) {
        return resp;
    }
    let raw = match std::str::from_utf8(&body) {
        Ok(t) => t,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "ok": false, "message": "输入不是合法 UTF-8 文本" })),
            )
                .into_response();
        }
    };
    // 兼容 JSON 包裹：{"cookie": "..."} / {"text": "..."}（面板两种提交方式都支持）
    let text: std::borrow::Cow<'_, str> = if raw.trim_start().starts_with('{') {
        serde_json::from_str::<serde_json::Value>(raw)
            .ok()
            .and_then(|v| {
                v.get("cookie")
                    .or_else(|| v.get("text"))
                    .and_then(|x| x.as_str())
                    .map(|s| std::borrow::Cow::Owned(s.to_string()))
            })
            .unwrap_or(std::borrow::Cow::Borrowed(raw))
    } else {
        std::borrow::Cow::Borrowed(raw)
    };
    let tokens = match crate::import::sniff_tokens(&text) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "ok": false, "message": e.to_string() })),
            )
                .into_response();
        }
    };
    let tokens_path = st.cfg.tokens_path.clone();
    match crate::import::persist_tokens(&tokens_path, &tokens) {
        Ok(added) => {
            if added.is_empty() {
                return Json(serde_json::json!({ "ok": true, "added": 0, "message": "token 已存在，未重复添加" }))
                    .into_response();
            }
            // 热更新到账号池。
            // 注意：web-cookie（session-token）凭证**不入池**——账号池走的是桌面版 Bearer 协议，
            // 把 Cookie 塞进去会被当成 Bearer 打上游必失败并熔断；web Cookie 由 web 桥接路径使用。
            let new_accounts: Vec<crate::pool::AccountEntry> = added
                .iter()
                .filter(|t| !t.token.contains("session-token"))
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
                        breaker: tokio::sync::RwLock::new(crate::pool::CircuitBreaker::new()),
                    }
                })
                .collect();
            // 非 Bearer 凭证（web-cookie）不入池，但仍计入"导入成功"响应
            for acc in new_accounts {
                st.pool.add_account(acc).await;
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

/// GET /api/tokens — 列出已入库 token（脱敏 + 稳定 id + 账号信息缓存 + 入库时间）
async fn handle_tokens_list(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !admin_authorized(&headers, &st) {
        return admin_denied();
    }
    match crate::import::load_tokens_healed(&st.cfg.tokens_path) {
        Ok(tokens) => {
            let metas = st.meta.all();
            let list: Vec<serde_json::Value> = tokens
                .iter()
                .map(|t| {
                    let id = crate::import::cred_id(&t.token);
                    serde_json::json!({
                        "id": id,
                        "token_masked": mask(&t.token),
                        "source": t.source,
                        "host": t.host,
                        "path": t.path,
                        "method": t.method,
                        "added_at": t.added_at,
                        "kind": crate::import::kind_of(&t.token),
                        // 最近一次拉取到的账号信息（昵称/邮箱/套餐/今日剩余）；从未拉取过则为 null
                        "meta": metas.get(&id),
                    })
                })
                .collect();
            Json(serde_json::json!({ "ok": true, "count": list.len(), "tokens": list })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "message": e.to_string() })),
        )
            .into_response(),
    }
}

/// POST /api/tokens/check — body `{"id":"<凭证id>"}`：对指定凭证拉取账号全貌并刷新缓存
async fn handle_token_check(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if let Some(resp) = write_guard(&headers, &st) {
        return resp;
    }
    let id = match serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("id").and_then(|x| x.as_str()).map(String::from))
    {
        Some(v) if !v.is_empty() => v,
        _ => return bad_req("缺少 id 参数（凭证列表里的 id 字段）"),
    };
    let (cookie, tok) = match cookie_by_id(&st, &id) {
        Some(v) => v,
        None => return bad_req("未找到该凭证（可能已被删除，请刷新列表）"),
    };
    let client = match crate::web_protocol::WebClient::new(cookie, "glm-5.3-flash".into()) {
        Ok(c) => c,
        Err(e) => return internal_err(&anyhow::Error::msg(e.to_string())),
    };
    let (identity, usage, subs, quota) = tokio::join!(
        client.auth_session(),
        client.usage_summary(),
        client.subscriptions(),
        client.freebuff_session(),
    );
    let iv = value_of(&identity);
    let uv = value_of(&usage);
    let sv = value_of(&subs);
    let qv = value_of(&quota);
    let ok = credential_usable(iv.as_ref(), qv.as_ref());
    let mut meta = build_cred_meta(&id, iv.as_ref(), uv.as_ref(), sv.as_ref(), qv.as_ref());
    meta.valid = ok;
    if !ok {
        meta.error = Some(
            identity
                .as_ref()
                .err()
                .map(|e| e.to_string())
                .or_else(|| quota.as_ref().err().map(|e| e.to_string()))
                .unwrap_or_else(|| "上游未返回登录账号（凭证已失效或未登录）".into()),
        );
    }
    if let Err(e) = st.meta.upsert(meta.clone()) {
        tracing::warn!("写入账号信息缓存失败: {e}");
    }
    record_history(&st, &meta, ok);
    st.logs.emit(
        if ok { "info" } else { "warn" },
        "account",
        None,
        format!("凭证检查 {}：{}", mask(&tok.token), if ok { "有效" } else { "可能已失效" }),
    );
    Json(serde_json::json!({
        "ok": ok,
        "valid": ok,
        "id": id,
        "meta": meta,
        "message": if ok { "凭证有效，账号信息已更新".to_string() } else { meta.error.clone().unwrap_or_else(|| "凭证可能已失效".into()) },
    }))
    .into_response()
}

/// POST /api/tokens/delete — body `{"id":"<凭证id>"}`：删除凭证（落盘 + 内存账号池 + 缓存）
async fn handle_token_delete(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if let Some(resp) = write_guard(&headers, &st) {
        return resp;
    }
    let id = match serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("id").and_then(|x| x.as_str()).map(String::from))
    {
        Some(v) if !v.is_empty() => v,
        _ => return bad_req("缺少 id 参数（凭证列表里的 id 字段）"),
    };
    match crate::import::delete_token(&st.cfg.tokens_path, &id) {
        Ok(Some(removed)) => {
            let in_pool = st.pool.remove_account(&removed.token).await;
            let _ = st.meta.remove(&id);
            st.logs.emit("warn", "account", None, format!("已删除凭证 {}", mask(&removed.token)));
            Json(serde_json::json!({
                "ok": true,
                "removed": mask(&removed.token),
                "removed_from_pool": in_pool,
                "message": if in_pool { "凭证已删除（含运行中的账号池）" } else { "凭证已删除" },
            }))
            .into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "ok": false, "message": "未找到该凭证（可能已被删除）" }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "message": format!("删除失败：{e}") })),
        )
            .into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
struct HistoryQuery {
    #[serde(default)]
    cred_id: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

/// GET /api/account/history — 账号使用记录（按凭证可查；最新在前）
async fn handle_account_history(State(st): State<AppState>, headers: HeaderMap, Query(q): Query<HistoryQuery>) -> Response {
    if !admin_authorized(&headers, &st) {
        return admin_denied();
    }
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    match st.meta.history(q.cred_id.as_deref(), limit) {
        Ok(records) => Json(serde_json::json!({
            "ok": true,
            "count": records.len(),
            "records": records,
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "message": e.to_string() })),
        )
            .into_response(),
    }
}

/// GET /api/guide — 客户端接入所需的全部信息（地址 / Key 状态 / 模型数），供面板与用户直接复制
async fn handle_guide(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !admin_authorized(&headers, &st) {
        return admin_denied();
    }
    let keys = api_keys_of(&st);
    let masked: Vec<String> = keys.iter().map(|k| mask(k)).collect();
    let models = st.registry.models().await;
    let web_cookie = pick_web_cookie(&st).map(|(_, cred, _)| cred);
    Json(serde_json::json!({
        "ok": true,
        "listen_addr": st.cfg.listen_addr,
        "openai_base_url": "/v1",
        "anthropic_base_url": "/",
        "api_keys": {
            "configured": !keys.is_empty(),
            "count": keys.len(),
            "masked": masked,
        },
        // 未配置 key 时给客户端填什么（本机直连场景）
        "api_key_hint": if keys.is_empty() { "sk-local（未配置 api_keys，任意非空字符串即可）" } else { "上面显示的 Key（面板请求会自动带上）" },
        "models_count": models.len(),
        "models_sample": models.iter().take(8).cloned().collect::<Vec<_>>(),
        "credential": web_cookie,
        "data_plane_ready": web_cookie.is_some(),
    }))
    .into_response()
}

#[derive(Debug, serde::Deserialize)]
struct KeyQuery {
    #[serde(default)]
    key: Option<String>,
}

/// GET /api/login/result — 读内嵌登录窗口的失败结果文件（面板 openEmbedLogin 轮询用）。
/// 无文件/已消费即返回 ok:false（面板继续等或引导降级）。
async fn handle_login_result(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !admin_authorized(&headers, &st) {
        return admin_denied();
    }
    let path = std::path::Path::new("data/login_window_result.json");
    match std::fs::read_to_string(path) {
        Ok(text) => {
            // 读取即消费（一次性结果）
            let _ = std::fs::remove_file(path);
            let v: serde_json::Value = serde_json::from_str(&text)
                .unwrap_or(serde_json::json!({ "ok": false, "message": "结果文件损坏" }));
            Json(v).into_response()
        }
        Err(_) => Json(serde_json::json!({ "ok": false, "consumed": true })).into_response(),
    }
}

/// POST /api/login/embed — 派生内嵌 WebView2 登录窗口子进程（非阻塞）。
///
/// 浏览器版「一键登录」的首选路径：窗口里完成 GitHub 登录后，
/// 子进程通过 WebView2 CookieManager 抓取含 HttpOnly 的全部 Cookie 并自动 POST
/// `/api/tokens/import` 入库，然后窗口自关。面板轮询 `/api/tokens` 感知凭证增加。
///
/// 平台支持：Windows（WebView2 Runtime）；不支持时返回 ok:false + 原因，前端降级到扩展/剪贴板/手动。
async fn handle_login_embed(State(st): State<AppState>, headers: HeaderMap, _body: axum::body::Bytes) -> Response {
    if let Some(resp) = write_guard(&headers, &st) {
        return resp;
    }
    // 解析监听端口（面板与本机网关同端口）
    let port = st
        .cfg
        .listen_addr
        .rsplit_once(':')
        .and_then(|(_, p)| p.parse::<u16>().ok())
        .unwrap_or(47821);
    let spawned = crate::login_window::spawn_login_window(port);
    if spawned {
        st.logs.emit("info", "login", None, "内嵌 WebView2 登录窗口已弹出（等待用户完成 GitHub 登录）");
        Json(serde_json::json!({
            "ok": true,
            "message": "登录窗口已弹出，完成 GitHub 登录后凭证将自动入库",
        }))
        .into_response()
    } else {
        // 两种失败：① 登录窗口已在运行（防重入拒绝）② 平台不支持 WebView2 / spawn 失败。
        // 文案不预设原因，引导用户看当前状态并给出替代路径。
        (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "ok": false,
                "message": "登录窗口未能启动——可能已有一个登录窗口在运行（请查看任务栏），或当前平台不支持 WebView2。可改用剪贴板/手动导入。",
            })),
        )
            .into_response()
    }
}

/// GET /api/extension/bundle — 下载浏览器一键登录扩展（zip）
///
/// 扩展文件编译期内嵌，单文件分发（没有源码目录）时同样可用。
/// 面板用 `window.open` 下载，浏览器不会带 Authorization 头，故同时接受 `?key=`（与日志 SSE 同一策略）。
async fn handle_extension_bundle(State(st): State<AppState>, headers: HeaderMap, Query(q): Query<KeyQuery>) -> Response {
    let keys = api_keys_of(&st);
    let query_ok = !keys.is_empty()
        && q.key.as_deref().map(|k| keys.iter().any(|x| x == k)).unwrap_or(false);
    if !admin_authorized(&headers, &st) && !query_ok {
        return admin_denied();
    }
    let zip = crate::extension::build_zip();
    let filename = format!("freebuff2api-extension-v{}.zip", crate::extension::version());
    match Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/zip")
        .header("content-disposition", format!("attachment; filename=\"{filename}\""))
        .header("cache-control", "no-store")
        .body(Body::from(zip))
    {
        Ok(r) => r,
        Err(e) => internal_err(&anyhow::Error::msg(format!("构建下载响应失败: {e}"))),
    }
}

/// POST /api/config/api-key — body `{"action":"generate"|"set"|"clear","key":"..."}`
/// 一键生成/设置/清除下游 API Key，写回 config.json **并立即热生效**（无需重启）。
async fn handle_config_api_key(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if let Some(resp) = write_guard(&headers, &st) {
        return resp;
    }
    let v: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return bad_req("body 必须是 JSON"),
    };
    let action = v.get("action").and_then(|x| x.as_str()).unwrap_or("generate");
    let current = api_keys_of(&st);

    let new_keys: Vec<String> = match action {
        "generate" => {
            // 密码学随机（OsRng 32 字节 → 32 字符 base64url 字符集映射，192 位有效熵），
            // 不可预测/不可枚举；UUIDv4 只有 122 位随机且格式可识别
            use rand::RngCore;
            use rand::rngs::OsRng;
            const B64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let mut raw = [0u8; 32];
            OsRng.fill_bytes(&mut raw);
            let token: String = raw.iter().map(|b| B64URL[(b & 63) as usize] as char).collect();
            let k = format!("sk-fb-{token}");
            vec![k]
        }
        "set" => {
            let k = v.get("key").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            if k.len() < 8 {
                return bad_req("Key 太短（至少 8 个字符）");
            }
            vec![k]
        }
        "clear" => {
            // 安全守卫：非本机监听时禁止清空（否则管理端点/凭证对网络裸奔）
            if !crate::config::is_loopback_listen(&st.cfg.listen_addr) {
                return bad_req("当前监听的是非本机地址，禁止清空 API Key（否则面板与凭证会暴露给网络）");
            }
            vec![]
        }
        other => return bad_req(&format!("未知 action: {other}（支持 generate/set/clear）")),
    };

    // 持久化到 config.json（用 Value 往返合并，避免覆盖用户其他配置）
    let path = crate::config::resolve_config_path();
    if let Some(p) = path.as_deref() {
        let mut root: serde_json::Value = std::fs::read_to_string(p)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        if !root.is_object() {
            root = serde_json::json!({});
        }
        root["api_keys"] = serde_json::json!(new_keys);
        let write_res = serde_json::to_string_pretty(&root)
            .map_err(|e| e.to_string())
            .and_then(|s| {
                // 原子写：临时文件 + rename，防写入中途崩溃损坏 config.json
                let tmp = format!("{p}.tmp");
                std::fs::write(&tmp, s.clone()).map_err(|e| e.to_string())?;
                #[cfg(windows)]
                if std::path::Path::new(p).exists() {
                    let _ = std::fs::remove_file(p);
                }
                std::fs::rename(&tmp, p).map_err(|e| e.to_string())
            });
        match write_res {
            Ok(()) => tracing::info!("api_keys 已写回 {p}"),
            Err(e) => tracing::warn!("api_keys 写回 {p} 失败: {e}（仅内存生效）"),
        }
    } else {
        tracing::warn!("未找到 config.json，api_keys 仅在本次运行内生效");
    }

    set_api_keys(&st, new_keys.clone());
    st.logs.emit("warn", "config", None, format!("下游 API Key 已更新（{} 个）", new_keys.len()));
    Json(serde_json::json!({
        "ok": true,
        "action": action,
        "configured": !new_keys.is_empty(),
        "count": new_keys.len(),
        // 生成/设置时回显明文一次，供用户复制到客户端
        "key": new_keys.first().cloned(),
        "previous_count": current.len(),
        "persisted": path.is_some(),
        "message": match action {
            "generate" => "已生成新的 API Key 并立即生效（同时写入 config.json）",
            "set" => "API Key 已更新并立即生效",
            _ => "API Key 已清除（本机直连模式）",
        },
    }))
    .into_response()
}

/// 账户详情卡片数据（每个账号的余额/套餐/限额）
/// POST /api/account/detail — body { cookie: "..." } 可选；空则用已导入第一个
async fn handle_account_detail(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if !admin_authorized(&headers, &st) {
        return admin_denied();
    }
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
            .or_else(|| load_imported_token(&st.cfg.tokens_path).filter(|t| t.contains("session-token")))
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

/// 脱敏：长串露 前6 + 后4；短串**必须露得更少**。
///
/// 教训（参考项目 freellmapi 的 maskKey 修复记录）：对短 key 用"取末尾 4 位"会等于完整回显。
/// 凭证列表是给人看的，不是给人抄的——短凭证一律只露极少量字符。
fn mask(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    let n = chars.len();
    if n <= 4 {
        return "***".into();
    }
    if n <= 8 {
        return format!("{}***", chars[..2].iter().collect::<String>());
    }
    if n <= 12 {
        let head: String = chars[..2].iter().collect();
        let tail: String = chars[n - 2..].iter().collect();
        return format!("{head}***{tail}");
    }
    let head: String = chars[..6].iter().collect();
    let tail: String = chars[n - 4..].iter().collect();
    format!("{head}...{tail}")
}

// ---------- OpenAI 兼容 ----------

async fn handle_chat_completions(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    // 数据面 CSRF 防线：浏览器跨站请求会带 Origin（text/plain 简单请求可绕过预检，
    // handler 不看 content-type 照样解析 body——必须拦 Origin）。
    // SDK/curl 不带 Origin，不受影响。
    if !origin_allowed(&headers) {
        return admin_denied();
    }
    let start = std::time::Instant::now();
    let req_id = uuid::Uuid::new_v4().to_string();
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
    let keys = api_keys_of(&st);
    if !keys.is_empty() && !authorized(&headers, &keys) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": { "message": "invalid proxy api key", "type": "authentication_error" } })),
        )
            .into_response();
    }

    // 模型路由降级
    let model = st.router.resolve(&requested).await;

    // ---------- Web-Cookie 桥接（关键链路补全） ----------
    // 用户只导入了 web Cookie（一键登录路径）而没有 Bearer token 时，账号池是空的，
    // pick_best 必然失败。此时把请求桥接到 web 协议（/api/chat/stream），
    // 让「照着接入指南填 /v1」的浏览器用户真正能对话。
    // 会话复用：续聊轮次只发最后一条用户消息 + 复用同一个上游 thread，
    // 避免每次请求都新开 thread 烧光每日会话准入（rateLimitsByModel.limit）。
    // 池里有账号还不够——占位符（如 __TEST_SKIP__）或已失效的 Bearer 会挡住桥接又必然 401。
    // 池内全是占位符（长度 < 20 或命中已知占位模式）时视为"没有可用账号"，走桥接。
    let pool_tokens: Vec<String> = {
        let accounts = st.pool.accounts.lock().await;
        accounts.iter().map(|a| a.token.clone()).collect()
    };
    let pool_has_usable = pool_tokens
        .iter()
        .any(|tok| tok.len() >= 20 && !tok.starts_with("__TEST_SKIP__"));
    if !pool_has_usable {
        if let Some((cookie, cred, cid)) = pick_web_cookie(&st) {
            if cookie.contains("session-token") {
                return web_bridge_openai(st, parsed, requested, model, cookie, cred, cid).await;
            }
        }
    }

    // 用量记录字段（api_key 脱敏；提前计算供重试路径复用）
    let api_key = headers
        .get("x-api-key")
        .or_else(|| headers.get("authorization"))
        .map(|v| v.to_str().unwrap_or("").to_string())
        .map(|k| if k.len() > 12 { format!("{}***{}", &k[..8], &k[k.len() - 4..]) } else { "***".to_string() })
        .unwrap_or_default();
    let client_ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or("").to_string())
        .unwrap_or_default();

    // 配置上行 body：设 model + 思考程度降级/剥离 + 提示词/技能注入（与账号无关，重试时复用）
    let mut up_body = parsed.clone();
    up_body["model"] = serde_json::json!(model);
    // 记忆检索 query：最后一条 user 消息（截断 200 字符）
    let mem_query: String = up_body
        .get("messages")
        .and_then(|m| m.as_array())
        .and_then(|arr| {
            arr.iter()
                .rev()
                .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
                .and_then(|m| m.get("content"))
                .map(|c| match c {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
        })
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect();
    // 内置提示词 + 技能 roster + 记忆注入（system 消息前插）
    // roster 模式：提示词走 prompts（base+启用项），技能走 skills 模块（只注入名称+描述）
    let sys_prefix = build_system_prefix(&st, &mem_query).await;
    if !sys_prefix.trim().is_empty() {
        if let Some(messages) = up_body.get_mut("messages").and_then(|m| m.as_array_mut()) {
            messages.insert(0, serde_json::json!({ "role": "system", "content": sys_prefix }));
        }
    }
    let mut effort_downgraded: Option<String> = None;
    if let Some(effort) = up_body.get("reasoning_effort").and_then(|v| v.as_str()) {
        match st.router.clamp_effort(&model, effort) {
            Some(clamped) => {
                if clamped != effort {
                    tracing::debug!("模型 {model} effort {effort} 降级为 {clamped}");
                    effort_downgraded = Some(clamped.clone());
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
    // token_saver（可选）：压缩超长 tool 结果（只处理 tool 角色消息，绝不触碰 system 前缀与历史头部）
    if st.cfg.token_saver {
        if let Some(msgs) = up_body.get_mut("messages").and_then(|m| m.as_array_mut()) {
            for m in msgs.iter_mut() {
                if m.get("role").and_then(|r| r.as_str()) != Some("tool") {
                    continue;
                }
                let long: Option<String> = m
                    .get("content")
                    .and_then(|c| c.as_str())
                    .filter(|c| c.len() > 8000)
                    .map(|s| s.to_string());
                if let Some(c) = long {
                    m["content"] = serde_json::json!(crate::router::compress_tool_result(&c, 8000));
                }
            }
        }
    }
    // 流式请求：向上游显式申请 usage 帧（真实 token 统计所需；非流式响应本身含 usage）
    if up_body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false) {
        up_body["stream_options"] = serde_json::json!({ "include_usage": true });
    }

    // 请求级重试循环：失败换号重试。
    // 此时尚未向客户端写出任何字节，天然满足 committed 边界（首字节后绝不重试）。
    let retry_policy = crate::retry::RetryPolicy::default();
    let mut account_name = String::new();
    let mut token = String::new();
    let mut run_id = String::new();
    let mut attempt_count = 0usize;
    let mut last_error = String::new();
    let mut last_status: u16 = 502;
    let mut upstream_resp: Option<reqwest::Response> = None;

    for attempt in 0..retry_policy.max_attempts {
        attempt_count = attempt + 1;
        // 挑 token（每次重试重新选号；熔断 Open 的账号会被跳过）
        let account = match st.pool.pick_best().await {
            Some(a) => a,
            None => {
                if attempt == 0 {
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({ "error": { "message": "no healthy upstream auth token available", "type": "server_error" } })),
                    )
                        .into_response();
                }
                last_error = "no healthy upstream auth token available".into();
                break;
            }
        };
        account_name = account.name.clone();
        token = account.token.clone();
        if attempt == 0 {
            st.telemetry.event(&req_id, "route", &format!("{requested} -> {model} via {account_name}"));
        } else {
            st.telemetry.event(&req_id, "retry", &format!("第 {} 次尝试 via {account_name}", attempt + 1));
            st.logs.emit("warn", "retry", Some(&req_id), format!("换号重试（第 {} 次）→ {account_name}", attempt + 1));
        }

        // 确保会话
        let instance_id = match account.session.ensure_session(&model).await {
            Ok(id) => Some(id),
            Err(e) => {
                let msg = e.to_string();
                if msg.starts_with("waiting_room_queued") {
                    st.usage.record_ex(&account_name, &model, 0, 0, 0, 429, "", "", &req_id).ok();
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(serde_json::json!({ "error": { "message": msg, "type": "server_error", "code": "waiting_room_queued" } })),
                    )
                        .into_response();
                }
                st.pool.mark_failure(&account_name, &format!("session: {msg}")).await;
                st.pool.update_score(&account_name, -30.0).await;
                last_status = 502;
                last_error = format!("failed to acquire free session: {msg}");
                continue;
            }
        };

        // run 管理：取根 run（惰性）
        let rid = match ensure_root_run(&st, &account_name, &token).await {
            Ok(id) => id,
            Err(e) => {
                st.pool.mark_failure(&account_name, "run").await;
                last_status = 502;
                last_error = format!("create run failed: {e}");
                continue;
            }
        };
        run_id = rid;

        // 调上游
        match st
            .client
            .chat_completions(&token, up_body.clone(), &run_id, instance_id.as_deref())
            .await
        {
            Ok(r) if r.status().is_success() => {
                upstream_resp = Some(r);
                break;
            }
            Ok(r) => {
                let code = r.status().as_u16();
                let body_text = r.text().await.unwrap_or_default();
                // 错误分类：文本规则优先（waiting_room/rate_limit/model_unavailable...），状态码兜底
                let kind = crate::errors::classify(code, &body_text);
                st.pool.mark_failure(&account_name, &format!("HTTP {code}")).await;
                st.pool.update_score(&account_name, -20.0).await;
                if code == 401 || code == 403 {
                    st.pool
                        .mark_cooldown(&account_name, std::time::Duration::from_secs(600), &format!("上游 {code}，token 疑似失效"))
                        .await;
                }
                last_status = code;
                last_error = format!("upstream HTTP {code} ({}): {}", kind.as_str(), crate::errors::error_excerpt(&body_text));
                // 换号判定：可重试错误（限流/排队/5xx/网络）+ 凭证失效（401/403 已冷却，换号继续）都换号
                let should_switch = kind.is_retryable()
                    || matches!(kind, crate::errors::ErrorKind::AuthExpired);
                if should_switch && attempt + 1 < retry_policy.max_attempts {
                    st.telemetry.event(&req_id, "retry_scheduled", &format!("HTTP {code}（{}），换号重试", kind.as_str()));
                    continue;
                }
                break;
            }
            Err(e) => {
                st.pool.mark_failure(&account_name, "network").await;
                st.pool.update_score(&account_name, -40.0).await;
                last_status = 502;
                last_error = e.to_string();
                continue;
            }
        }
    }

    // 全部尝试失败：记录并返回汇总错误
    let upstream_resp = match upstream_resp {
        Some(r) => r,
        None => {
            let latency = start.elapsed().as_millis() as i64;
            st.usage.record_ex(&account_name, &model, 0, 0, latency, last_status as i64, "", "", &req_id).ok();
            st.telemetry.record(TraceRow {
                req_id: req_id.clone(),
                endpoint: "/v1/chat/completions".into(),
                requested_model: requested.clone(),
                resolved_model: model.clone(),
                account: account_name.clone(),
                status: last_status,
                latency_ms: latency as u64,
                ttft_ms: None,
                prompt_tokens: 0,
                completion_tokens: 0,
                stream: false,
                error_kind: Some(crate::errors::classify(last_status, &last_error).as_str().to_string()),
                error_excerpt: Some(last_error.chars().take(300).collect()),
                route_reason: Some(format!("{} -> {}（{} 次尝试均失败）", requested, model, attempt_count)),
                api_key: Some(api_key.clone()),
                client_ip: Some(client_ip.clone()),
            });
            st.logs
                .emit("error", "request", Some(&req_id), format!("{model} 全部 {attempt_count} 次尝试失败: {}", last_error.chars().take(120).collect::<String>()));
            // 有上游 HTTP 响应（非网络类失败）：透传上游状态码，保持客户端重试/退避语义
            if (400..600).contains(&last_status) {
                return (
                    StatusCode::from_u16(last_status).unwrap_or(StatusCode::BAD_GATEWAY),
                    Json(serde_json::json!({
                        "error": {
                            "message": last_error,
                            "type": "upstream_error",
                            "code": last_status,
                            "model": model,
                            "upstream": st.client.base_url(),
                            "attempts": attempt_count,
                        }
                    })),
                )
                    .into_response();
            }
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": { "message": format!("all {attempt_count} attempts failed: {last_error}"), "type": "server_error" } })),
            )
                .into_response();
        }
    };

    let status = upstream_resp.status();
    let latency_ms = start.elapsed().as_millis() as i64;
    // （api_key / client_ip 已在重试循环前计算）
    // 观察（零 LLM）：模型使用偏好 / 档位降级 / 用户纠正信号 → 记忆库（可配置关闭）
    if st.cfg.memory_enabled {
        let _ = st.memory.observe(&model, effort_downgraded.as_deref(), &mem_query, None);
    }

    if status.is_success() {
        st.pool.update_score(&account_name, 10.0).await;
        st.pool.mark_success(&account_name).await; // 熔断器：驱动 HalfOpen → Closed 恢复
        st.telemetry.event(&req_id, "upstream_ok", &format!("HTTP {} 建连 {}ms", status.as_u16(), latency_ms));
        // run 收尾：FINISH 上报（释放上游 run 计数；失败不影响响应）
        if let Err(e) = st.client.finish_run(&token, &run_id, 1).await {
            tracing::debug!("finish_run 失败（不影响响应）: {e}");
        }
        st.logs.emit("info", "request", Some(&req_id), format!("{model} 上游 200，准备转发（{latency_ms}ms 建连）"));
    } else {
        st.usage
            .record_ex(&account_name, &model, 0, 0, latency_ms, status.as_u16() as i64, &api_key, &client_ip, &req_id)
            .ok();
        st.pool.update_score(&account_name, -20.0).await;
        // 上游 401/403 = token 失效 → 冷却熔断该账号（此前 mark_cooldown 为死代码，此处接线）
        let code = status.as_u16();
        if code == 401 || code == 403 {
            st.pool
                .mark_cooldown(&account_name, std::time::Duration::from_secs(600), &format!("上游 {code}，token 疑似失效"))
                .await;
        }
        let kind = crate::retry::classify_status(code).as_str();
        st.telemetry.event(&req_id, "upstream_error", &format!("HTTP {code} ({kind})"));
        st.telemetry.record(TraceRow {
            req_id: req_id.clone(),
            endpoint: "/v1/chat/completions".into(),
            requested_model: requested.clone(),
            resolved_model: model.clone(),
            account: account_name.clone(),
            status: code,
            latency_ms: latency_ms as u64,
            ttft_ms: None,
            prompt_tokens: 0,
            completion_tokens: 0,
            stream: false,
            error_kind: Some(kind.to_string()),
            error_excerpt: None,
            route_reason: Some(format!("{} -> {}", requested, model)),
            api_key: Some(api_key.clone()),
            client_ip: Some(client_ip.clone()),
        });
        st.logs.emit("warn", "request", Some(&req_id), format!("上游 {code}（{kind}），账号 {account_name} 已扣分{}", if code == 401 || code == 403 { "并冷却 10 分钟" } else { "" }));
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

    // 2xx：分流（非流式读全量解析 usage；流式 spawn 转发 + 旁路采集）
    let is_stream = parsed.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
    let telem = st.telemetry.clone();
    let logs = st.logs.clone();
    let usage_db = st.usage.clone();
    let (rid, acc, mdl, key, ip) = (req_id.clone(), account_name.clone(), model.clone(), api_key.clone(), client_ip.clone());
    let route_reason = format!("{requested} -> {model}");

    if !is_stream {
        let bytes = upstream_resp.bytes().await.unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes).to_string();
        // 上游可能在 HTTP 200 的 body 里返回错误码（free_mode_* 系列）——识别并视为上游错误
        if let Some(code) = upstream_body_error(&text) {
            let total_ms = start.elapsed().as_millis() as u64;
            usage_db.record_ex(&acc, &mdl, 0, 0, total_ms as i64, 502, &key, &ip, &rid).ok();
            st.telemetry.event(&rid, "upstream_body_error", code);
            telem.record(TraceRow {
                req_id: rid.clone(),
                endpoint: "/v1/chat/completions".into(),
                requested_model: requested.clone(),
                resolved_model: mdl.clone(),
                account: acc.clone(),
                status: 502,
                latency_ms: total_ms,
                ttft_ms: None,
                prompt_tokens: 0,
                completion_tokens: 0,
                stream: false,
                error_kind: Some("upstream_5xx".into()),
                error_excerpt: Some(crate::errors::error_excerpt(&text)),
                route_reason: None,
                api_key: Some(key.clone()),
                client_ip: Some(ip.clone()),
            });
            logs.emit("warn", "request", Some(&rid), format!("{mdl} 上游 200 但 body 含错误码 {code}"));
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("upstream returned error in 200 body: {code}"),
                        "type": "upstream_error",
                        "code": code,
                        "model": mdl,
                        "upstream": st.client.base_url(),
                    }
                })),
            )
                .into_response();
        }
        let (pt, ct) = extract_usage(&text).unwrap_or((0, 0));
        let total_ms = start.elapsed().as_millis() as u64;
        usage_db
            .record_ex(&acc, &mdl, pt as i64, ct as i64, total_ms as i64, 200, &key, &ip, &rid)
            .ok();
        telem.record(TraceRow {
            req_id: rid.clone(),
            endpoint: "/v1/chat/completions".into(),
            requested_model: requested.clone(),
            resolved_model: mdl.clone(),
            account: acc.clone(),
            status: 200,
            latency_ms: total_ms,
            ttft_ms: None,
            prompt_tokens: pt,
            completion_tokens: ct,
            stream: false,
            error_kind: None,
            error_excerpt: None,
            route_reason: Some(route_reason),
            api_key: Some(key.clone()),
            client_ip: Some(ip.clone()),
        });
        logs.emit("info", "request", Some(&rid), format!("{mdl} 完成 {}+{} tok（{:.2}s）", pt, ct, total_ms as f64 / 1000.0));
        return builder.body(Body::from(bytes)).unwrap().into_response();
    }

    // 流式：spawn 转发任务；旁路扫描 usage、记录首字节与总耗时
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(16);
    tokio::spawn(async move {
        let t0 = std::time::Instant::now();
        let mut stream = upstream_resp.bytes_stream();
        let mut ttft: Option<u64> = None;
        let mut tail = String::new();
        let mut totals: (u64, u64) = (0, 0);
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(b) => {
                    if ttft.is_none() {
                        ttft = Some(t0.elapsed().as_millis() as u64);
                    }
                    tail.push_str(&String::from_utf8_lossy(&b));
                    if tail.len() > 8000 {
                        tail = tail_keep(&tail, 4000);
                    }
                    if let Some((p, c)) = extract_usage(&tail) {
                        totals = (p, c);
                    }
                    if tx.send(Ok(b)).await.is_err() {
                        break; // 客户端断开
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(std::io::Error::other(e.to_string()))).await;
                    telem.record(TraceRow {
                        req_id: rid.clone(),
                        endpoint: "/v1/chat/completions".into(),
                        requested_model: String::new(),
                        resolved_model: mdl.clone(),
                        account: acc.clone(),
                        status: 502,
                        latency_ms: latency_ms as u64 + t0.elapsed().as_millis() as u64,
                        ttft_ms: ttft,
                        prompt_tokens: totals.0,
                        completion_tokens: totals.1,
                        stream: true,
                        error_kind: Some("network".into()),
                        error_excerpt: Some(e.to_string().chars().take(300).collect()),
                        route_reason: None,
                        api_key: Some(key.clone()),
                        client_ip: Some(ip.clone()),
                    });
                    logs.emit("error", "request", Some(&rid), format!("{mdl} 流中断: {e}"));
                    return;
                }
            }
        }
        let total_ms = latency_ms as u64 + t0.elapsed().as_millis() as u64;
        usage_db
            .record_ex(&acc, &mdl, totals.0 as i64, totals.1 as i64, total_ms as i64, 200, &key, &ip, &rid)
            .ok();
        telem.record(TraceRow {
            req_id: rid.clone(),
            endpoint: "/v1/chat/completions".into(),
            requested_model: String::new(),
            resolved_model: mdl.clone(),
            account: acc.clone(),
            status: 200,
            latency_ms: total_ms,
            ttft_ms: ttft,
            prompt_tokens: totals.0,
            completion_tokens: totals.1,
            stream: true,
            error_kind: None,
            error_excerpt: None,
            route_reason: Some(route_reason),
            api_key: Some(key.clone()),
            client_ip: Some(ip.clone()),
        });
        logs.emit("info", "request", Some(&rid), format!("{mdl} 流式完成 {}+{} tok（首字节 {}ms / 总 {:.2}s）", totals.0, totals.1, ttft.unwrap_or(0), total_ms as f64 / 1000.0));
    });
    let body_stream = futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    builder.body(Body::from_stream(body_stream)).unwrap().into_response()
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
    // 数据面 CSRF 防线（与 /v1/chat/completions 同理：拦恶意网页 text/plain 简单请求）
    if !origin_allowed(&headers) {
        return admin_denied();
    }
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

    let keys = api_keys_of(&st);
    if !keys.is_empty() && !authorized(&headers, &keys) {
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

    // ---------- Web-Cookie 桥接（与 /v1/chat/completions 相同策略，含占位符保护） ----------
    let pool_tokens: Vec<String> = {
        let accounts = st.pool.accounts.lock().await;
        accounts.iter().map(|a| a.token.clone()).collect()
    };
    let pool_has_usable = pool_tokens
        .iter()
        .any(|tok| tok.len() >= 20 && !tok.starts_with("__TEST_SKIP__"));
    if !pool_has_usable {
        if let Some((cookie, cred, cid)) = pick_web_cookie(&st) {
            if cookie.contains("session-token") {
                // Claude messages → OpenAI messages 复用现有转换（含 system/blocks/tool_use）
                let openai_msgs = claude_to_openai_messages(parsed.get("messages").cloned().unwrap_or(serde_json::json!([])));
                let mut bridge_body = serde_json::json!({
                    "model": model,
                    "messages": openai_msgs,
                    "stream": parsed.get("stream").and_then(|v| v.as_bool()).unwrap_or(false),
                });
                if let Some(sys) = parsed.get("system") {
                    let sys_text = match sys {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Array(blocks) => blocks
                            .iter()
                            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join("\n"),
                        _ => String::new(),
                    };
                    if !sys_text.is_empty() {
                        if let Some(msgs) = bridge_body["messages"].as_array_mut() {
                            msgs.insert(0, serde_json::json!({ "role": "system", "content": sys_text }));
                        }
                    }
                }
                let resp = web_bridge_openai(st, bridge_body, model.clone(), resolved.clone(), cookie, cred, cid).await;
                // OpenAI 形状的错误体 → Claude 形状
                return openai_error_to_claude(resp, &resolved).await;
            }
        }
    }

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

    // Claude → OpenAI 协议转换：system 提取、messages content blocks 打平、max_tokens 映射
    let openai_messages = claude_to_openai_messages(parsed.get("messages").cloned().unwrap_or(serde_json::json!([])));
    let mut up_body = serde_json::json!({
        "model": resolved,
        "messages": openai_messages,
        "stream": parsed.get("stream").and_then(|v| v.as_bool()).unwrap_or(false),
    });
    if let Some(maxt) = parsed.get("max_tokens").and_then(|v| v.as_u64()) {
        up_body["max_tokens"] = serde_json::json!(maxt);
    }
    // Claude system 字段（string 或 blocks 数组）→ OpenAI system 消息前置
    if let Some(sys) = parsed.get("system") {
        let sys_text = match sys {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(blocks) => blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        if !sys_text.is_empty() {
            if let Some(msgs) = up_body["messages"].as_array_mut() {
                msgs.insert(0, serde_json::json!({ "role": "system", "content": sys_text }));
            }
        }
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
    // 非 2xx：转 Claude 错误格式透传
    if !status.is_success() {
        let err_body = upstream_resp.bytes().await.unwrap_or_default();
        let err_text = String::from_utf8_lossy(&err_body).to_string();
        tracing::warn!("[上游错误/Claude] HTTP {status}: {}", err_text.chars().take(500).collect::<String>());
        return (
            status,
            Json(serde_json::json!({
                "type": "error",
                "error": { "type": "api_error", "message": err_text }
            })),
        )
            .into_response();
    }
    let mut builder = Response::builder().status(status);
    for (k, v) in upstream_resp.headers() {
        if k != "content-length" && k != "transfer-encoding" {
            builder = builder.header(k, v);
        }
    }
    // 非流式：把 OpenAI 响应转换回 Claude 格式（message + content blocks + stop_reason）
    let wants_stream = parsed.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
    if wants_stream {
        // 流式：OpenAI SSE → canonical event → Anthropic 事件流（不再直接透传）
        let claude_req_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(16);
        let model_for_render = resolved.clone();
        let telem = st.telemetry.clone();
        let logs = st.logs.clone();
        let (rid, acc, mdl, req_model) = (claude_req_id.clone(), account.name.clone(), resolved.clone(), model.clone());
        let key = headers
            .get("x-api-key")
            .or_else(|| headers.get("authorization"))
            .map(|v| v.to_str().unwrap_or("").to_string())
            .map(|k| if k.len() > 12 { format!("{}***{}", &k[..8], &k[k.len() - 4..]) } else { "***".to_string() })
            .unwrap_or_default();
        let t_start = std::time::Instant::now();
        tokio::spawn(async move {
            let mut decoder = crate::protocol::openai_sse::OpenAiSseDecoder::new();
            let mut renderer = crate::protocol::anthropic_sse::AnthropicSseRenderer::new(&model_for_render);
            let mut stream = upstream_resp.bytes_stream();
            let mut ttft: Option<u64> = None;
            let mut totals: (u64, u64) = (0, 0);
            let mut err_text: Option<String> = None;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(b) => {
                        if ttft.is_none() {
                            ttft = Some(t_start.elapsed().as_millis() as u64);
                        }
                        let events = decoder.feed(&b);
                        let mut out = String::new();
                        for ev in &events {
                            if let crate::protocol::stream::CanonicalEvent::Usage { input_tokens, output_tokens } = ev {
                                totals = (*input_tokens, *output_tokens);
                            }
                            for frame in renderer.render(ev) {
                                out.push_str(&frame);
                            }
                        }
                        if !out.is_empty() && tx.send(Ok(axum::body::Bytes::from(out))).await.is_err() {
                            break; // 客户端断开
                        }
                    }
                    Err(e) => {
                        err_text = Some(e.to_string());
                        let frames = renderer.render(&crate::protocol::stream::CanonicalEvent::Error(e.to_string())).join("");
                        let _ = tx.send(Ok(axum::body::Bytes::from(frames))).await;
                        break;
                    }
                }
            }
            // 收尾：冲刷 decoder 残余 + renderer 兜底 message_stop
            let mut tail = String::new();
            for ev in decoder.finish() {
                for frame in renderer.render(&ev) {
                    tail.push_str(&frame);
                }
            }
            tail.push_str(&renderer.finish().join(""));
            if !tail.is_empty() {
                let _ = tx.send(Ok(axum::body::Bytes::from(tail))).await;
            }
            let total_ms = t_start.elapsed().as_millis() as u64;
            let status = if err_text.is_some() { 502u16 } else { 200 };
            telem.record(TraceRow {
                req_id: rid.clone(),
                endpoint: "/v1/messages".into(),
                requested_model: req_model,
                resolved_model: mdl.clone(),
                account: acc.clone(),
                status,
                latency_ms: total_ms,
                ttft_ms: ttft,
                prompt_tokens: totals.0,
                completion_tokens: totals.1,
                stream: true,
                error_kind: err_text.as_ref().map(|_| "network".to_string()),
                error_excerpt: err_text.map(|t| t.chars().take(300).collect()),
                route_reason: None,
                api_key: Some(key),
                client_ip: None,
            });
            logs.emit(if status == 200 { "info" } else { "error" }, "request", Some(&rid), format!("{mdl} Claude 流式完成 {}+{} tok（{:.2}s）", totals.0, totals.1, total_ms as f64 / 1000.0));
        });
        let body_stream = futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        });
        builder
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .body(Body::from_stream(body_stream))
            .unwrap()
            .into_response()
    } else {
        let bytes = upstream_resp.bytes().await.unwrap_or_default();
        let openai_resp: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::json!({}));
        let claude_resp = openai_to_claude_response(&openai_resp, &resolved);
        (StatusCode::OK, Json(claude_resp)).into_response()
    }
}

/// Claude messages → OpenAI messages：
/// - content 为 string：原样
/// - content 为 blocks 数组：text 块拼接为 string，tool_result 块转 role=tool 消息，tool_use 块转 assistant.tool_calls
fn claude_to_openai_messages(messages: serde_json::Value) -> serde_json::Value {
    let arr = messages.as_array().cloned().unwrap_or_default();
    let mut out: Vec<serde_json::Value> = Vec::with_capacity(arr.len());
    for m in arr {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user").to_string();
        let content = m.get("content").cloned().unwrap_or(serde_json::json!(""));
        match content {
            serde_json::Value::String(s) => out.push(serde_json::json!({ "role": role, "content": s })),
            serde_json::Value::Array(blocks) => {
                let mut text_parts: Vec<String> = Vec::new();
                let mut tool_calls: Vec<serde_json::Value> = Vec::new();
                let mut tool_results: Vec<serde_json::Value> = Vec::new();
                for b in &blocks {
                    let btype = b.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match btype {
                        "text" => {
                            if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                                text_parts.push(t.to_string());
                            }
                        }
                        "tool_use" => {
                            tool_calls.push(serde_json::json!({
                                "id": b.get("id").cloned().unwrap_or(serde_json::json!("call_unknown")),
                                "type": "function",
                                "function": {
                                    "name": b.get("name").cloned().unwrap_or(serde_json::json!("")),
                                    "arguments": serde_json::to_string(&b.get("input").cloned().unwrap_or(serde_json::json!({}))).unwrap_or_default(),
                                }
                            }));
                        }
                        "tool_result" => {
                            let result_text = match b.get("content") {
                                Some(serde_json::Value::String(s)) => s.clone(),
                                Some(serde_json::Value::Array(items)) => items
                                    .iter()
                                    .filter_map(|i| i.get("text").and_then(|t| t.as_str()))
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                                _ => String::new(),
                            };
                            tool_results.push(serde_json::json!({
                                "role": "tool",
                                "tool_call_id": b.get("tool_use_id").cloned().unwrap_or(serde_json::json!("")),
                                "content": result_text,
                            }));
                        }
                        _ => {}
                    }
                }
                // assistant 的 tool_use → OpenAI assistant 消息 + tool_calls
                if !tool_calls.is_empty() {
                    let mut msg = serde_json::json!({ "role": "assistant", "content": if text_parts.is_empty() { serde_json::Value::Null } else { serde_json::json!(text_parts.join("\n")) } });
                    msg["tool_calls"] = serde_json::json!(tool_calls);
                    out.push(msg);
                } else if !text_parts.is_empty() {
                    out.push(serde_json::json!({ "role": role, "content": text_parts.join("\n") }));
                }
                // user 的 tool_result → OpenAI tool 消息
                for tr in tool_results {
                    out.push(tr);
                }
            }
            other => out.push(serde_json::json!({ "role": role, "content": other })),
        }
    }
    serde_json::json!(out)
}

/// OpenAI chat 响应 → Claude messages 响应（非流式）
fn openai_to_claude_response(openai: &serde_json::Value, model: &str) -> serde_json::Value {
    let choice = openai.get("choices").and_then(|c| c.as_array()).and_then(|c| c.first());
    let message = choice.and_then(|c| c.get("message"));
    let mut blocks: Vec<serde_json::Value> = Vec::new();
    let mut tool_calls_out: Vec<serde_json::Value> = Vec::new();
    if let Some(msg) = message {
        if let Some(text) = msg.get("content").and_then(|c| c.as_str()) {
            if !text.is_empty() {
                blocks.push(serde_json::json!({ "type": "text", "text": text }));
            }
        }
        if let Some(tcs) = msg.get("tool_calls").and_then(|t| t.as_array()) {
            for tc in tcs {
                let fn_obj = tc.get("function");
                let name = fn_obj.and_then(|f| f.get("name")).and_then(|n| n.as_str()).unwrap_or("").to_string();
                let args_raw = fn_obj.and_then(|f| f.get("arguments")).and_then(|a| a.as_str()).unwrap_or("{}");
                let input: serde_json::Value = serde_json::from_str(args_raw).unwrap_or(serde_json::json!({}));
                tool_calls_out.push(serde_json::json!({
                    "type": "tool_use",
                    "id": tc.get("id").cloned().unwrap_or(serde_json::json!("toolu_unknown")),
                    "name": name,
                    "input": input,
                }));
            }
        }
    }
    if blocks.is_empty() && tool_calls_out.is_empty() {
        blocks.push(serde_json::json!({ "type": "text", "text": "" }));
    }
    for tc in tool_calls_out {
        blocks.push(tc);
    }
    // stop_reason 映射
    let finish = choice.and_then(|c| c.get("finish_reason")).and_then(|f| f.as_str()).unwrap_or("end_turn");
    let stop_reason = match finish {
        "stop" => "end_turn",
        "length" => "max_tokens",
        "tool_calls" | "function_call" => "tool_use",
        _ => "end_turn",
    };
    let usage = openai.get("usage");
    let input_tokens = usage.and_then(|u| u.get("prompt_tokens")).and_then(|t| t.as_i64()).unwrap_or(0);
    let output_tokens = usage.and_then(|u| u.get("completion_tokens")).and_then(|t| t.as_i64()).unwrap_or(0);
    serde_json::json!({
        "id": openai.get("id").cloned().unwrap_or(serde_json::json!("msg_freebuff")),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": blocks,
        "stop_reason": stop_reason,
        "stop_sequence": serde_json::Value::Null,
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
        }
    })
}

// ---------- 技能 API ----------

/// GET /api/skills — 技能列表 + roster 注入预览
async fn handle_skills_list(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !admin_authorized(&headers, &st) {
        return admin_denied();
    }
    let skills = st.skills.list();
    let roster = st.skills.system_prefix(st.cfg.max_roster_tokens);
    let roster_tokens = roster.chars().count() / 4 + 1;
    Json(serde_json::json!({
        "ok": true,
        "skills": skills,
        "roster_preview": roster,
        "roster_tokens": roster_tokens,
        "max_roster_tokens": st.cfg.max_roster_tokens,
    }))
    .into_response()
}

/// POST /api/skills — 新建/更新技能 body: { id?, name, description, body, triggers? }
async fn handle_skills_upsert(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if let Some(resp) = write_guard(&headers, &st) {
        return resp;
    }
    let v: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return bad_req("invalid json"),
    };
    let id = v.get("id").and_then(|x| x.as_str());
    let input = crate::skills::SkillInput {
        name: v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        description: v.get("description").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        body: v.get("body").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        triggers: v
            .get("triggers")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
            .unwrap_or_default(),
    };
    if input.name.trim().is_empty() || input.description.trim().is_empty() {
        return bad_req("name 和 description 不能为空");
    }
    // 质量门：对 name/description/body 统一校验；未通过默认拒绝（force:true 可强制保存）
    let force = v.get("force").and_then(|x| x.as_bool()).unwrap_or(false);
    let mut issues = st.skills.gate(&input.body);
    issues.extend(st.skills.gate(&input.name));
    issues.extend(st.skills.gate(&input.description));
    if !issues.is_empty() && !force {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "message": "质量门未通过（如确认无误可加 force:true 强制保存）", "issues": issues })),
        )
            .into_response();
    }
    match st.skills.upsert(id, input) {
        Ok(s) => {
            let gate = st.skills.gate(&s.body);
            st.logs.emit("info", "skills", None, format!("技能已保存: {}", s.name));
            Json(serde_json::json!({ "ok": true, "skill": s, "gate": gate })).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "ok": false, "message": e.to_string() }))).into_response(),
    }
}

/// POST /api/skills/toggle — body: { id, enabled }
async fn handle_skills_toggle(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if let Some(resp) = write_guard(&headers, &st) {
        return resp;
    }
    let v: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return bad_req("invalid json"),
    };
    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
    let enabled = v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false);
    match st.skills.toggle(id, enabled) {
        Ok(ok) => Json(serde_json::json!({ "ok": ok, "id": id, "enabled": enabled })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "ok": false, "message": e.to_string() }))).into_response(),
    }
}

/// POST /api/skills/delete — body: { id }
async fn handle_skills_delete(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if let Some(resp) = write_guard(&headers, &st) {
        return resp;
    }
    let v: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return bad_req("invalid json"),
    };
    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
    match st.skills.delete(id) {
        Ok(ok) => Json(serde_json::json!({ "ok": ok, "id": id })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "ok": false, "message": e.to_string() }))).into_response(),
    }
}

/// POST /api/skills/gate — body: { body } 返回质量问题列表（空数组=通过）
async fn handle_skills_gate(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if !admin_authorized(&headers, &st) {
        return admin_denied();
    }
    let v: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return bad_req("invalid json"),
    };
    let text = v.get("body").and_then(|x| x.as_str()).unwrap_or("");
    Json(serde_json::json!({ "ok": true, "issues": st.skills.gate(text) })).into_response()
}

// ---------- 日志 API ----------

#[derive(serde::Deserialize)]
struct LogsQuery {
    limit: Option<usize>,
    after_id: Option<u64>,
    /// SSE 场景浏览器无法设置请求头，允许用 query 传 api key（仅 api_keys 非空时校验）
    key: Option<String>,
}

/// GET /api/logs/recent?limit=200 — 最近日志（首屏加载）
async fn handle_logs_recent(State(st): State<AppState>, headers: HeaderMap, Query(q): Query<LogsQuery>) -> Response {
    if !admin_authorized(&headers, &st) {
        return admin_denied();
    }
    let events = st.logs.recent(q.limit.unwrap_or(200).min(1000), q.after_id);
    Json(serde_json::json!({ "ok": true, "events": events, "count": events.len() })).into_response()
}

/// GET /api/logs/stream — 实时日志 SSE。
/// - 支持 `?key=`（浏览器 EventSource 无法带头）
/// - 支持 `Last-Event-ID` 请求头：断线重连时先补发错过的事件
async fn handle_logs_stream(State(st): State<AppState>, headers: HeaderMap, Query(q): Query<LogsQuery>) -> Response {
    let keys = api_keys_of(&st);
    let query_key_ok = !keys.is_empty()
        && q.key
            .as_deref()
            .map(|k| keys.iter().any(|x| x == k))
            .unwrap_or(false);
    if !admin_authorized(&headers, &st) && !query_key_ok {
        return admin_denied();
    }
    // 断线补发：浏览器重连会带 Last-Event-ID
    let last_id = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    let backlog: Vec<Result<Event, std::convert::Infallible>> = st
        .logs
        .recent(200, last_id)
        .into_iter()
        .map(|ev| {
            let data = serde_json::to_string(&ev).unwrap_or_default();
            Ok(Event::default().id(ev.id.to_string()).data(data))
        })
        .collect();
    let rx = st.logs.subscribe();
    let live = futures::stream::unfold(rx, |mut rx2| async move {
        loop {
            match rx2.recv().await {
                Ok(ev) => {
                    let data = serde_json::to_string(&ev).unwrap_or_default();
                    let sse = Event::default().id(ev.id.to_string()).data(data);
                    return Some((Ok::<Event, std::convert::Infallible>(sse), rx2));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    let stream = futures::stream::iter(backlog).chain(live);
    Sse::new(stream).into_response()
}

// ---------- 请求详情 ----------

/// GET /api/usage/requests/{id} — 单条请求完整信息 + 事件链
async fn handle_usage_request_detail(
    State(st): State<AppState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<i64>,
) -> Response {
    if !admin_authorized(&headers, &st) {
        return admin_denied();
    }
    match st.usage.request_by_id(id) {
        Ok(Some(rec)) => {
            let (events, trace) = if rec.req_id.is_empty() {
                (Vec::new(), None)
            } else {
                (
                    read_telemetry_events(&st.cfg.telemetry_path, &rec.req_id),
                    read_telemetry_request(&st.cfg.telemetry_path, &rec.req_id),
                )
            };
            Json(serde_json::json!({ "ok": true, "request": rec, "trace": trace, "events": events })).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "ok": false, "message": "request not found" }))).into_response(),
        Err(e) => internal_err(&e),
    }
}

/// 读取遥测事件（按 req_id 关联；低频操作，短连接可接受）
fn read_telemetry_events(db_path: &str, req_id: &str) -> Vec<serde_json::Value> {
    let conn = match rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut stmt = match conn.prepare("SELECT ts, kind, detail FROM events WHERE req_id = ?1 ORDER BY id DESC LIMIT 50") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(rusqlite::params![req_id], |r| {
        Ok(serde_json::json!({
            "ts": r.get::<_, String>(0)?,
            "kind": r.get::<_, String>(1)?,
            "detail": r.get::<_, String>(2)?,
        }))
    });
    match rows {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// 读取遥测富记录（endpoint/ttft/error_kind 等；详情抽屉优先展示）
fn read_telemetry_request(db_path: &str, req_id: &str) -> Option<serde_json::Value> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    let mut stmt = conn
        .prepare("SELECT ts,endpoint,requested_model,resolved_model,account,status,latency_ms,ttft_ms,prompt_tokens,completion_tokens,stream,error_kind,error_excerpt,route_reason FROM requests_v2 WHERE req_id = ?1 ORDER BY id DESC LIMIT 1")
        .ok()?;
    let mut rows = stmt
        .query_map(rusqlite::params![req_id], |r| {
            Ok(serde_json::json!({
                "ts": r.get::<_, String>(0)?,
                "endpoint": r.get::<_, String>(1)?,
                "requested_model": r.get::<_, String>(2)?,
                "resolved_model": r.get::<_, String>(3)?,
                "account": r.get::<_, String>(4)?,
                "status": r.get::<_, i64>(5)?,
                "latency_ms": r.get::<_, i64>(6)?,
                "ttft_ms": r.get::<_, Option<i64>>(7)?,
                "prompt_tokens": r.get::<_, i64>(8)?,
                "completion_tokens": r.get::<_, i64>(9)?,
                "stream": r.get::<_, i64>(10)? != 0,
                "error_kind": r.get::<_, Option<String>>(11)?,
                "error_excerpt": r.get::<_, Option<String>>(12)?,
                "route_reason": r.get::<_, Option<String>>(13)?,
            }))
        })
        .ok()?;
    rows.next().and_then(|r| r.ok())
}

/// GET /api/usage/cost — 速率与错误率（免费层无货币成本；诚实标注 estimated）
async fn handle_usage_cost(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !admin_authorized(&headers, &st) {
        return admin_denied();
    }
    let recent = match st.usage.recent_requests(500) {
        Ok(v) => v,
        Err(e) => return internal_err(&e),
    };
    let now = chrono::Utc::now();
    let mut requests_30m = 0i64;
    let mut errors_30m = 0i64;
    let mut total_ms = 0i64;
    for r in &recent {
        if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&r.ts) {
            let age_min = now
                .signed_duration_since(ts.with_timezone(&chrono::Utc))
                .num_minutes();
            if (0..=30).contains(&age_min) {
                requests_30m += 1;
                if r.status >= 400 {
                    errors_30m += 1;
                }
                total_ms += r.latency_ms;
            }
        }
    }
    let totals = st.usage.totals().unwrap_or_else(|_| serde_json::json!({}));
    Json(serde_json::json!({
        "ok": true,
        "window_minutes": 30,
        "requests_30m": requests_30m,
        "errors_30m": errors_30m,
        "error_rate_30m": if requests_30m > 0 { errors_30m as f64 / requests_30m as f64 } else { 0.0 },
        "avg_latency_ms_30m": if requests_30m > 0 { total_ms / requests_30m } else { 0 },
        "requests_per_hour": requests_30m * 2,
        "totals": totals,
        "estimated": true,
        "cost_source": "免费层（无货币成本记录）",
    }))
    .into_response()
}

/// 写端点 CSRF 防护：要求 `application/json`。
/// 跨站表单只能发送 text/plain / urlencoded / multipart（浏览器"简单请求"，无需预检）；
/// 强制 JSON 会让浏览器先发预检，从而挡住 CSRF 写入。
fn json_write_ok(headers: &HeaderMap) -> bool {
    headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase().starts_with("application/json"))
        .unwrap_or(false)
}

/// 写端点统一守卫：鉴权 + JSON content-type
fn write_guard(headers: &HeaderMap, st: &AppState) -> Option<Response> {
    if !admin_authorized(headers, st) {
        return Some(admin_denied());
    }
    if !json_write_ok(headers) {
        return Some(bad_req("写操作要求 content-type: application/json（CSRF 防护）"));
    }
    None
}

// ---------- 上游会话清理 ----------

/// 记录上游 threadId（json 追加 + 去重；供会话清理，防止反代长期堆积给上游制造压力）
/// threads.json 串行锁：record_thread / sweep_threads 都是"读-改-写整文件"，
/// 且 sweep 的删除窗口跨越网络 IO（数十秒），并发会互相覆盖（丢新 thread 记录 → 永不清理）。
static THREADS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 原子写（JSON 字符串 → 临时文件 → rename），防截断
fn atomic_write_json(path: &str, json: &str) {
    let p = std::path::Path::new(path);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        #[cfg(windows)]
        if p.exists() {
            let _ = std::fs::remove_file(p);
        }
        let _ = std::fs::rename(&tmp, p);
    }
}

fn record_thread(path: &str, thread_id: &str) {
    if thread_id.trim().is_empty() {
        return;
    }
    let _g = match THREADS_LOCK.lock() {
        Ok(g) => g,
        Err(_) => return, // 锁中毒时放弃记录（不能因记录失败阻塞对话流）
    };
    let mut list: Vec<serde_json::Value> = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    if list
        .iter()
        .any(|t| t.get("id").and_then(|v| v.as_str()) == Some(thread_id))
    {
        return;
    }
    list.push(serde_json::json!({
        "id": thread_id,
        "created_at": chrono::Utc::now().to_rfc3339(),
    }));
    if let Ok(json) = serde_json::to_string_pretty(&list) {
        atomic_write_json(path, &json);
    }
}

/// POST /api/threads/cleanup — 清理上游会话（用户批注需求：防止反代给上游制造压力/被识别）
/// body: `{ dry_run?: bool = true, max_age_hours?: u64 = 24 }`
/// 保守设计：dry_run 默认 true；上游删除端点未在抓包中确认（尝试 DELETE /{id} 与 POST /delete），
/// 删除失败的记录会保留（不丢账）。
/// 会话清理结果
struct SweepOutcome {
    total: usize,
    expired: usize,
    expired_ids: Vec<String>,
    deleted: usize,
    /// 删除成功（含上游已不存在）的 id，回写时从 threads.json 移除这些
    deleted_ids: Vec<String>,
    failed: Vec<String>,
    remaining: usize,
}

/// 扫描 threads.json，找出超过保留时长的上游会话；`dry_run=false` 时真实删除并回写文件。
///
/// 用户批注（网页对话.txt:605）：反代必须自己清理上游会话，
/// "以防止我们反代给上游制造压力等等的让上游查出来就完蛋了"。
async fn sweep_threads(st: &AppState, max_age_hours: u64, dry_run: bool) -> anyhow::Result<SweepOutcome> {
    let all: Vec<serde_json::Value> = std::fs::read_to_string(&st.cfg.threads_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let now = chrono::Utc::now();
    let mut expired_ids: Vec<String> = Vec::new();
    let mut fresh: Vec<serde_json::Value> = Vec::new();
    for t in &all {
        let id = t.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let created = t
            .get("created_at")
            .and_then(|x| x.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
        let is_expired = match created {
            Some(c) => now.signed_duration_since(c.with_timezone(&chrono::Utc)).num_hours() as u64 >= max_age_hours,
            None => true, // 无时间戳视为可清理
        };
        if is_expired {
            expired_ids.push(id);
        } else {
            fresh.push(t.clone());
        }
    }
    let mut outcome = SweepOutcome {
        total: all.len(),
        expired: expired_ids.len(),
        expired_ids: expired_ids.clone(),
        deleted: 0,
        deleted_ids: Vec::new(),
        failed: Vec::new(),
        remaining: fresh.len(),
    };
    // 预演或没有过期项 → 不需要凭证、不写文件
    if dry_run || expired_ids.is_empty() {
        return Ok(outcome);
    }
    let (cookie, _, _) = match pick_web_cookie(st) {
        Some(v) => v,
        None => anyhow::bail!("需要 web Cookie 凭证才能清理上游会话"),
    };
    let client = crate::web_protocol::WebClient::new(cookie, "glm-5.3-flash".into())
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let mut still = fresh;
    for id in &expired_ids {
        match client.delete_thread(id).await {
            // 上游已删掉 / 本来就不存在（404）→ 两种情况都不需要再保留本地记录
            Ok(found) => {
                outcome.deleted += 1;
                outcome.deleted_ids.push(id.clone());
                st.logs.emit(
                    "info",
                    "cleanup",
                    None,
                    if found {
                        format!("已删除上游会话 {id}")
                    } else {
                        format!("会话 {id} 上游已不存在，移除本地记录")
                    },
                );
            }
            Err(e) => {
                outcome.failed.push(id.clone());
                tracing::debug!("删除会话 {id} 失败: {e}");
                if let Some(orig) = all.iter().find(|t| t.get("id").and_then(|x| x.as_str()) == Some(id.as_str())) {
                    still.push(orig.clone());
                }
            }
        }
    }
    outcome.remaining = still.len();
    // 回写必须在 THREADS_LOCK 内、且**基于当前文件内容合并**：
    // sweep 的删除窗口跨越网络 IO（数十秒），期间 record_thread 可能追加了新 thread——
    // 直接回写旧快照会把它们吃掉（该会话永不清理）。锁内重读文件，只移除已成功删除的 id。
    let _g = match THREADS_LOCK.lock() {
        Ok(g) => g,
        Err(_) => return Ok(outcome),
    };
    let mut final_list: Vec<serde_json::Value> = std::fs::read_to_string(&st.cfg.threads_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    final_list.retain(|t| {
        let id = t.get("id").and_then(|x| x.as_str()).unwrap_or("");
        !outcome.deleted_ids.contains(&id.to_string())
    });
    // 补回删除失败但快照里没有的（防御性，一般不会发生）
    for id in &outcome.failed {
        if !final_list.iter().any(|t| t.get("id").and_then(|x| x.as_str()) == Some(id.as_str())) {
            if let Some(orig) = all.iter().find(|t| t.get("id").and_then(|x| x.as_str()) == Some(id.as_str())) {
                final_list.push(orig.clone());
            }
        }
    }
    outcome.remaining = final_list.len();
    if let Ok(json) = serde_json::to_string_pretty(&final_list) {
        atomic_write_json(&st.cfg.threads_path, &json);
    }
    Ok(outcome)
}

/// 后台自动清理循环：每 `thread_cleanup_interval_sec` 秒清理一次超过 `thread_max_age_hours` 的上游会话。
/// 间隔为 0 时不启动（由 main 决定）。没有过期会话时完全静默，不刷日志。
pub async fn thread_cleanup_loop(st: AppState) {
    let interval = st.cfg.thread_cleanup_interval_sec.max(60);
    tracing::info!(
        "上游会话自动清理已启用：每 {interval}s 清理超过 {} 小时的会话",
        st.cfg.thread_max_age_hours
    );
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
        match sweep_threads(&st, st.cfg.thread_max_age_hours, false).await {
            Ok(o) if o.expired == 0 => {}
            Ok(o) => st.logs.emit(
                "info",
                "cleanup",
                None,
                format!("自动清理：删除 {} 个过期会话（{} 个失败），剩余 {}", o.deleted, o.failed.len(), o.remaining),
            ),
            Err(e) => tracing::warn!("自动清理会话未执行: {e}"),
        }
    }
}

async fn handle_threads_cleanup(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if let Some(resp) = write_guard(&headers, &st) {
        return resp;
    }
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_else(|_| serde_json::json!({}));
    let dry_run = v.get("dry_run").and_then(|x| x.as_bool()).unwrap_or(true);
    let max_age_hours = v
        .get("max_age_hours")
        .and_then(|x| x.as_u64())
        .unwrap_or(st.cfg.thread_max_age_hours);

    let outcome = match sweep_threads(&st, max_age_hours, dry_run).await {
        Ok(o) => o,
        Err(e) => return bad_req(&e.to_string()),
    };

    if dry_run {
        return Json(serde_json::json!({
            "ok": true,
            "dry_run": true,
            "total": outcome.total,
            "expired": outcome.expired,
            "thread_ids": outcome.expired_ids,
            "message": format!("预演：{} 个会话超过 {} 小时可清理（真实删除请传 dry_run:false）", outcome.expired, max_age_hours),
        }))
        .into_response();
    }
    Json(serde_json::json!({
        "ok": true,
        "dry_run": false,
        "deleted": outcome.deleted,
        "failed": outcome.failed.len(),
        "failed_ids": outcome.failed,
        "remaining": outcome.remaining,
        "message": if outcome.failed.is_empty() {
            format!("已清理 {} 个会话", outcome.deleted)
        } else {
            format!("已清理 {} 个；{} 个删除失败（上游端点待验证，记录已保留）", outcome.deleted, outcome.failed.len())
        },
    }))
    .into_response()
}

// ---------- 记忆 API ----------
/// GET /api/memory — 记忆列表 + 统计
async fn handle_memory_list(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !admin_authorized(&headers, &st) {
        return admin_denied();
    }
    let memories = st.memory.list(200);
    let stats = st.memory.stats().unwrap_or_else(|_| serde_json::json!({}));
    Json(serde_json::json!({ "ok": true, "memories": memories, "stats": stats })).into_response()
}

/// POST /api/memory — 手动新增记忆 body: {kind, title, content, is_static?}
async fn handle_memory_upsert(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if let Some(resp) = write_guard(&headers, &st) {
        return resp;
    }
    let v: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return bad_req("invalid json"),
    };
    let kind = v.get("kind").and_then(|x| x.as_str()).unwrap_or("preference");
    let title: String = v
        .get("title")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .chars()
        .take(200)
        .collect();
    let content: String = v
        .get("content")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .chars()
        .take(4000)
        .collect();
    let is_static = v.get("is_static").and_then(|x| x.as_bool()).unwrap_or(false);
    if title.is_empty() || content.trim().is_empty() {
        return bad_req("title 和 content 不能为空");
    }
    match st.memory.upsert(kind, &title, &content, is_static) {
        Ok(m) => {
            st.logs.emit("info", "memory", None, format!("记忆已保存: {}", m.title));
            Json(serde_json::json!({ "ok": true, "memory": m })).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "ok": false, "message": e.to_string() }))).into_response(),
    }
}

/// POST /api/memory/delete — body: {id}
async fn handle_memory_delete(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if let Some(resp) = write_guard(&headers, &st) {
        return resp;
    }
    let v: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return bad_req("invalid json"),
    };
    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
    match st.memory.delete(id) {
        Ok(ok) => Json(serde_json::json!({ "ok": ok, "id": id })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "ok": false, "message": e.to_string() }))).into_response(),
    }
}

/// POST /api/memory/static — body: {id, is_static}
async fn handle_memory_static(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if let Some(resp) = write_guard(&headers, &st) {
        return resp;
    }
    let v: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return bad_req("invalid json"),
    };
    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
    let is_static = v.get("is_static").and_then(|x| x.as_bool()).unwrap_or(false);
    match st.memory.set_static(id, is_static) {
        Ok(ok) => Json(serde_json::json!({ "ok": ok, "id": id, "is_static": is_static })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "ok": false, "message": e.to_string() }))).into_response(),
    }
}

// ---------- MCP（只读工具） ----------

/// POST /mcp — MCP JSON-RPC 2.0 端点（tools/list + tools/call，仅只读 3 工具）
async fn handle_mcp(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    // 与 /v1 一致的网关鉴权：配置 api_keys 时校验；未配置时仅本机
    if !origin_allowed(&headers) {
        return admin_denied();
    }
    let keys = api_keys_of(&st);
    if !keys.is_empty() {
        if !authorized(&headers, &keys) {
            return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": { "message": "invalid proxy api key" } }))).into_response();
        }
    } else if !is_loopback_request(&headers) {
        return admin_denied();
    }
    let raw = String::from_utf8_lossy(&body).to_string();
    let snapshot = build_mcp_snapshot(&st).await;
    match crate::mcp::handle_json(&raw, &snapshot) {
        Some(resp) => Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Body::from(resp))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        None => StatusCode::ACCEPTED.into_response(), // notification：无响应体
    }
}

/// 组装 MCP 数据快照（从 AppState 拉取只读数据）
async fn build_mcp_snapshot(st: &AppState) -> crate::mcp::GatewaySnapshot {
    let models = st.registry.models().await;
    let pool_snap = st.pool.snapshot().await;
    let accounts = pool_snap
        .accounts
        .iter()
        .map(|a| crate::mcp::AccountBrief {
            name: a.name.clone(),
            healthy: a.healthy,
            score: a.score,
            session_status: a
                .session
                .as_ref()
                .map(|s| format!("{:?}", s.status))
                .unwrap_or_else(|| "unknown".into()),
        })
        .collect();
    let usage = st.usage.totals().unwrap_or_else(|_| serde_json::json!({}));
    crate::mcp::GatewaySnapshot {
        models,
        accounts,
        usage_totals: usage,
        version: env!("CARGO_PKG_VERSION").into(),
        uptime_sec: st.started.elapsed().as_secs(),
    }
}

// ---------- 系统体检 ----------

/// GET /api/doctor — 逐项检查（四态：ok / fault / unknown / fact；"未检查"就是未检查）
async fn handle_doctor(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !admin_authorized(&headers, &st) {
        return admin_denied();
    }
    let mut checks: Vec<serde_json::Value> = Vec::new();

    // 1. 配置
    checks.push(serde_json::json!({
        "id": "config", "label": "配置", "state": "ok",
        "detail": format!("监听 {}，上游 {}", st.cfg.listen_addr, st.cfg.upstream_base_url),
    }));
    // 2. 账号池
    let snap = st.pool.snapshot().await;
    let healthy = snap.accounts.iter().filter(|a| a.healthy).count();
    let (acct_state, acct_fix) = if snap.total == 0 {
        ("fault", "去「账号」页导入 Cookie / 一键登录")
    } else if healthy == 0 {
        ("fault", "所有账号冷却中：等待冷却结束，或重新登录刷新凭证")
    } else {
        ("ok", "")
    };
    checks.push(serde_json::json!({
        "id": "accounts", "label": "账号池", "state": acct_state,
        "detail": format!("{healthy}/{} 个账号可用", snap.total),
        "fix": acct_fix,
    }));
    // 3. 模型注册表
    let models = st.registry.models().await;
    checks.push(serde_json::json!({
        "id": "models", "label": "模型注册表",
        "state": if models.is_empty() { "fault" } else { "ok" },
        "detail": format!("{} 个模型可用", models.len()),
        "fix": if models.is_empty() { "检查上游连通性与代理设置" } else { "" },
    }));
    // 4. 遥测写入
    let dropped = st.telemetry.dropped();
    checks.push(serde_json::json!({
        "id": "telemetry", "label": "遥测写入",
        "state": if dropped > 1000 { "fault" } else { "ok" },
        "detail": format!("累计丢弃 {} 条（队列满时）", dropped),
    }));
    // 5. 技能库
    let skills = st.skills.list();
    let enabled = skills.iter().filter(|s| s.enabled).count();
    checks.push(serde_json::json!({
        "id": "skills", "label": "技能库", "state": "ok",
        "detail": format!("{} 条（启用 {}）", skills.len(), enabled),
    }));
    // 6. 日志总线
    checks.push(serde_json::json!({
        "id": "logs", "label": "日志总线", "state": "ok",
        "detail": format!("已记录 {} 条", st.logs.count()),
    }));
    // 7. 记忆库
    let mem_stats = st.memory.stats().unwrap_or_else(|_| serde_json::json!({}));
    checks.push(serde_json::json!({
        "id": "memory", "label": "记忆库", "state": "ok",
        "detail": format!(
            "{} 条（稳定事实 {} / 纠正 {}）",
            mem_stats.get("total").and_then(|v| v.as_i64()).unwrap_or(0),
            mem_stats.get("static_count").and_then(|v| v.as_i64()).unwrap_or(0),
            mem_stats.get("corrections").and_then(|v| v.as_i64()).unwrap_or(0)
        ),
    }));
    // 8. 版本
    checks.push(serde_json::json!({
        "id": "version", "label": "版本", "state": "fact",
        "detail": format!("v{}（运行 {} 秒）", env!("CARGO_PKG_VERSION"), st.started.elapsed().as_secs()),
    }));

    Json(serde_json::json!({ "ok": true, "checks": checks })).into_response()
}

/// 组装 system 前缀：
/// - skills_inject_mode="roster"（默认）：提示词（base+启用项）+ 技能 roster（名称+描述，预算内）
/// - skills_inject_mode="full"：退回旧行为（prompts.system_prefix 含全量技能）
/// - 记忆块（低权威，预算 512 token）追加在最后；空则不注入
async fn build_system_prefix(st: &AppState, query: &str) -> String {
    let mut s = if st.cfg.skills_inject_mode == "full" {
        st.prompts.system_prefix().await
    } else {
        let mut base = st.prompts.system_prefix_prompts_only().await;
        let roster = st.skills.system_prefix(st.cfg.max_roster_tokens);
        if !roster.trim().is_empty() {
            base.push_str("\n\n[freebuff-skills]\n以下技能可用（低权威参考；相关时按其指引行事）：\n");
            base.push_str(&roster);
            base.push_str("\n[/freebuff-skills]");
        }
        base
    };
    // 记忆注入（用户偏好/纠正；空结果不注入，保持请求字节稳定；memory_enabled=false 时整体关闭）
    if st.cfg.memory_enabled {
        let mem = st.memory.brief(query, 512);
        if !mem.is_empty() {
            s.push_str(&mem);
        }
    }
    s
}

/// 上游 200 响应体内嵌错误码检测（已知 free_mode_* 系列；正常响应不受影响）
fn upstream_body_error(text: &str) -> Option<&'static str> {
    const CODES: &[&str] = &[
        "free_mode_invalid_agent_model",
        "free_mode_invalid_agent_hierarchy",
        "free_mode_cli_required",
    ];
    CODES.iter().find(|c| text.contains(**c)).copied()
}

/// 安全保留字符串尾部约 `keep_bytes` 字节，且起点落在字符边界上。
/// 注意：不可直接用 `split_off(len - n)`——多字节字符（中文/emoji）会被切成非法边界而 panic。
fn tail_keep(s: &str, keep_bytes: usize) -> String {
    if s.len() <= keep_bytes {
        return s.to_string();
    }
    let mut start = s.len() - keep_bytes;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}

/// 从 SSE/JSON 文本尾部提取 usage（容错：找不到返回 None）
fn extract_usage(text: &str) -> Option<(u64, u64)> {
    let idx = text.rfind("\"usage\"")?;
    let tail = &text[idx..];
    let p = find_json_number(tail, "\"prompt_tokens\"");
    let c = find_json_number(tail, "\"completion_tokens\"");
    match (p, c) {
        (Some(p), Some(c)) => Some((p, c)),
        _ => None,
    }
}

/// 在字符串里找 `"key": 123` 的数字（简易解析，够用且无依赖）
fn find_json_number(s: &str, key: &str) -> Option<u64> {
    let i = s.find(key)?;
    let rest = &s[i + key.len()..];
    let rest = rest.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

// ---------- 多模态上传 ----------

/// POST /v1/uploads — 上传文件到上游换取 storageId（多模态链路第一步）
///
/// 用法（裸文件体；无需 multipart，保持零新依赖）：
/// ```bash
/// curl -X POST http://127.0.0.1:47821/v1/uploads \
///   -H "content-type: image/png" -H "x-file-name: a.png" \
///   --data-binary @a.png
/// ```
/// 返回 `{ id, storageId, mediaType, name }`，把 storageId 放进 /v1/web/chat 的 images 数组即可。
async fn handle_upload(State(st): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    // 数据面 CSRF 防线（multipart 上传同样会被 text/plain 简单请求盲打）
    if !origin_allowed(&headers) {
        return admin_denied();
    }
    let keys = api_keys_of(&st);
    if !keys.is_empty() && !authorized(&headers, &keys) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": { "message": "invalid proxy api key", "type": "authentication_error" } })),
        )
            .into_response();
    }
    let cookie = load_imported_token(&st.cfg.tokens_path).filter(|t| t.contains("session-token"));
    let cookie = match cookie {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": { "message": "多模态上传需要 web Cookie 凭证（先 POST /api/tokens/import 导入）", "type": "invalid_request_error", "code": "multimodal_requires_web_cookie" } })),
            )
                .into_response();
        }
    };
    if body.is_empty() {
        return bad_req("empty body（请用 --data-binary 发送文件内容）");
    }
    if body.len() > 20 * 1024 * 1024 {
        return bad_req("文件过大（上限 20MB）");
    }
    let filename = headers
        .get("x-file-name")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("upload.png")
        .to_string();
    // 透传客户端 Content-Type（支持图片/文档/任意文件；缺省按扩展名推断）
    // 注意：此前白名单只放行 image/* 与 pdf，导致文档上传被改写成 image/png（拿不到 kind:"document"）
    let mime = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or(s).trim().to_string())
        .filter(|s| !s.is_empty() && s != "application/octet-stream")
        .unwrap_or_else(|| {
            let lower = filename.to_lowercase();
            if lower.ends_with(".png") {
                "image/png".into()
            } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
                "image/jpeg".into()
            } else if lower.ends_with(".gif") {
                "image/gif".into()
            } else if lower.ends_with(".webp") {
                "image/webp".into()
            } else if lower.ends_with(".pdf") {
                "application/pdf".into()
            } else if lower.ends_with(".txt") || lower.ends_with(".md") {
                "text/plain".into()
            } else if lower.ends_with(".json") {
                "application/json".into()
            } else {
                "application/octet-stream".into()
            }
        });
    let model_override = headers
        .get("x-model")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "glm-5.3-flash".into());

    let client = match crate::web_protocol::WebClient::new(cookie, model_override.clone()) {
        Ok(c) => c,
        Err(e) => return internal_err(&anyhow::Error::msg(e.to_string())),
    };
    match client.upload_with_model(body.to_vec(), &filename, &mime, &model_override).await {
        Ok(up) => {
            st.logs.emit("info", "upload", None, format!("上传 {}（{:.1}KB, {}）→ storageId {}", filename, body.len() as f64 / 1024.0, up.kind, up.storage_id));
            Json(serde_json::json!({
                "id": up.storage_id,
                "object": "file",
                "storageId": up.storage_id,
                "kind": up.kind,               // "image" | "document"
                "mediaType": up.media_type,
                "name": up.name,
                "bytes": body.len(),
                "url": up.url,                 // 仅图片有
                "descriptionStorageId": up.description_storage_id,
                "chars": up.chars,             // 仅文档有（提取字符数）
                "truncated": up.truncated,     // 仅文档有（是否截断）
                "usage": {
                    "image": "把 storageId 放进 /v1/web/chat 的 images 数组",
                    "document": "把 storageId 放进 /v1/web/chat 的 attachments 数组"
                }
            }))
            .into_response()
        }
        Err(e) => {
            // 截断上游错误体（防泄露账号/内部细节）
            let msg: String = e.to_string().chars().take(300).collect();
            (StatusCode::BAD_GATEWAY, Json(serde_json::json!({ "error": { "message": msg, "type": "upstream_error" } }))).into_response()
        }
    }
}

/// 统一 400 JSON 响应
fn bad_req(msg: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "ok": false, "message": msg }))).into_response()
}

/// 选取 web Cookie 凭证（config 优先，其次导入库；返回 cookie + 展示信息 + 稳定 id）
fn pick_web_cookie(st: &AppState) -> Option<(String, serde_json::Value, String)> {
    let looks_like = |t: &str| t.contains("session-token");
    if let Some(c) = st.cfg.auth_tokens.iter().find(|t| looks_like(t)) {
        return Some((
            c.clone(),
            serde_json::json!({ "source": "config", "added_at": null, "token_masked": mask(c) }),
            crate::import::cred_id(c),
        ));
    }
    let toks = crate::import::load_tokens_healed(&st.cfg.tokens_path).ok()?;
    let t = toks.into_iter().find(|t| looks_like(&t.token))?;
    let id = crate::import::cred_id(&t.token);
    Some((
        t.token.clone(),
        serde_json::json!({ "source": t.source, "added_at": t.added_at, "token_masked": mask(&t.token), "id": id }),
        id,
    ))
}

/// 按稳定 id 取出一条 web Cookie 凭证（用于"对指定凭证做检查/详情"）
fn cookie_by_id(st: &AppState, id: &str) -> Option<(String, crate::import::ExtractedAuth)> {
    crate::import::load_tokens_healed(&st.cfg.tokens_path)
        .ok()?
        .into_iter()
        .find(|t| crate::import::cred_id(&t.token) == id)
        .map(|t| (t.token.clone(), t))
}

/// 上游调用结果 → `Option<serde_json::Value>`（`None` 表示该端点失败，快照对应字段留空）
fn value_of<T: serde::Serialize>(r: &Result<T, anyhow::Error>) -> Option<serde_json::Value> {
    r.as_ref().ok().and_then(|v| serde_json::to_value(v).ok())
}

/// `/api/auth/session` 对**未登录/无效凭证**返回的是 `HTTP 200 + {}`，不是 401。
/// 只判断"请求成功"会把无效凭证误判为有效（实测：伪造 Cookie → `{}` 200）。
/// 因此必须检查响应里真的有 user 主体。
fn session_is_authenticated(v: &serde_json::Value) -> bool {
    match v.get("user").filter(|u| u.is_object()) {
        Some(u) => ["id", "email", "name"]
            .iter()
            .any(|k| u.get(*k).map(|x| !x.is_null()).unwrap_or(false)),
        None => false,
    }
}

/// `/api/web/freebuff-session` 对无效凭证返回 401；有效时才有 accessTier/freebucks。
fn quota_is_authenticated(v: &serde_json::Value) -> bool {
    v.get("accessTier").map(|x| !x.is_null()).unwrap_or(false) || v.get("freebucks").is_some()
}

/// 凭证是否可用：身份端点或额度端点任一确认已登录
fn credential_usable(identity: Option<&serde_json::Value>, quota: Option<&serde_json::Value>) -> bool {
    identity.map(session_is_authenticated).unwrap_or(false) || quota.map(quota_is_authenticated).unwrap_or(false)
}

/// 把上游 4 个端点的响应聚合成一条面板可读的账号快照
fn build_cred_meta(
    cred_id: &str,
    identity: Option<&serde_json::Value>,
    usage: Option<&serde_json::Value>,
    subscriptions: Option<&serde_json::Value>,
    quota: Option<&serde_json::Value>,
) -> crate::account_meta::CredMeta {
    use crate::account_meta::{CredMeta, ModelQuota};
    let mut m = CredMeta { cred_id: cred_id.to_string(), checked_at: chrono::Utc::now().to_rfc3339(), ..Default::default() };

    if let Some(v) = identity {
        let u = v.get("user").unwrap_or(v);
        m.name = u.get("name").and_then(|x| x.as_str()).map(String::from);
        m.email = u.get("email").and_then(|x| x.as_str()).map(String::from);
        m.image = u.get("image").and_then(|x| x.as_str()).map(String::from);
        m.user_id = u.get("id").and_then(|x| x.as_str()).map(String::from);
        m.expires = v.get("expires").and_then(|x| x.as_str()).map(String::from);
    }
    if let Some(v) = usage {
        m.streak_current = v.pointer("/streak/current").and_then(|x| x.as_i64());
        m.all_time_active_days = v.get("allTimeActiveDays").and_then(|x| x.as_i64());
        m.tokens_7d = v.pointer("/recent/totalTokens").and_then(|x| x.as_i64());
    }
    // 套餐档位：优先 freebuff-session.subscription，其次 /api/web/subscriptions.subscription
    m.tier_id = quota
        .and_then(|v| v.pointer("/subscription/tierId"))
        .and_then(|x| x.as_str())
        .map(String::from)
        .or_else(|| {
            subscriptions
                .and_then(|v| v.pointer("/subscription/tierId"))
                .and_then(|x| x.as_str())
                .map(String::from)
        });
    if let Some(v) = quota {
        m.access_tier = v.get("accessTier").and_then(|x| x.as_str()).map(String::from);
        m.country_code = v.get("countryCode").and_then(|x| x.as_str()).map(String::from);
        m.country_block_reason = v.get("countryBlockReason").and_then(|x| x.as_str()).map(String::from);
        let d = v.pointer("/freebucks/daily");
        if let Some(d) = d {
            m.daily_limit = d.get("limit").and_then(|x| x.as_i64());
            m.daily_spent = d.get("spent").and_then(|x| x.as_i64());
            m.daily_remaining = d.get("remaining").and_then(|x| x.as_i64());
            m.reset_at = d.get("resetAt").and_then(|x| x.as_str()).map(String::from);
        }
        let prices = v.pointer("/freebucks/prices");
        if let Some(rl) = v.get("rateLimitsByModel").and_then(|x| x.as_object()) {
            m.models = rl
                .iter()
                .map(|(name, item)| {
                    let limit = item.get("limit").and_then(|x| x.as_i64());
                    let used = item.get("recentCount").and_then(|x| x.as_i64());
                    ModelQuota {
                        model: name.clone(),
                        price: prices.and_then(|p| p.get(name)).and_then(|x| x.as_i64()),
                        limit,
                        used,
                        remaining: limit.map(|l| (l - used.unwrap_or(0)).max(0)),
                        reset_at: item.get("resetAt").and_then(|x| x.as_str()).map(String::from),
                        pool_label: item.get("poolLabel").and_then(|x| x.as_str()).map(String::from),
                    }
                })
                .collect();
        }
    }
    m
}

/// 写入一条使用记录（用户批注：每个账号都要有记录可查）。失败只记日志，不影响请求。
fn record_history(st: &AppState, m: &crate::account_meta::CredMeta, ok: bool) {
    let rec = crate::account_meta::HistoryRecord {
        ts: m.checked_at.clone(),
        cred_id: m.cred_id.clone(),
        name: m.name.clone(),
        email: m.email.clone(),
        tier_id: m.tier_id.clone(),
        daily_limit: m.daily_limit,
        daily_spent: m.daily_spent,
        daily_remaining: m.daily_remaining,
        tokens_7d: m.tokens_7d,
        streak_current: m.streak_current,
        ok,
    };
    if let Err(e) = st.meta.append_history(&rec) {
        tracing::warn!("写入使用记录失败: {e}");
    }
}

/// GET /api/account/overview — 账号全貌（身份 / 用量统计 / 套餐 / 额度积分 / 本地凭证）
/// 聚合上游 4 个端点，任一失败降级为 null（不整体失败）；前端负责中文呈现。
async fn handle_account_overview(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !admin_authorized(&headers, &st) {
        return admin_denied();
    }
    let (cookie, cred, cid) = match pick_web_cookie(&st) {
        Some(v) => v,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "ok": false,
                    "code": "need_cookie",
                    "message": "还没有 web Cookie 凭证。点「一键登录」按向导操作，或粘贴 Cookie 导入。"
                })),
            )
                .into_response();
        }
    };
    let client = match crate::web_protocol::WebClient::new(cookie, "glm-5.3-flash".into()) {
        Ok(c) => c,
        Err(e) => return internal_err(&anyhow::Error::msg(e.to_string())),
    };
    // 并发拉取（任一失败不影响其他）
    let (identity, usage, subs, quota) = tokio::join!(
        client.auth_session(),
        client.usage_summary(),
        client.subscriptions(),
        client.freebuff_session(),
    );
    // 身份/额度端点任一确认已登录才算凭证可用。
    // 注意：上游对未登录请求返回 200 + {}（不是 401），只判断"请求是否成功"会误判为有效。
    let iv = value_of(&identity);
    let qv = value_of(&quota);
    if !credential_usable(iv.as_ref(), qv.as_ref()) {
        let hint = identity
            .as_ref()
            .err()
            .map(|e| e.to_string())
            .or_else(|| quota.as_ref().err().map(|e| e.to_string()))
            .unwrap_or_else(|| "上游未返回登录账号（凭证已失效或未登录）".into());
        // 失败也记一条，便于排查"这个账号什么时候开始不可用"
        let mut failed = crate::account_meta::CredMeta {
            cred_id: cid,
            valid: false,
            error: Some(hint.clone()),
            checked_at: chrono::Utc::now().to_rfc3339(),
            ..Default::default()
        };
        if let Some(prev) = st.meta.get(&failed.cred_id) {
            failed.name = prev.name;
            failed.email = prev.email;
        }
        let _ = st.meta.upsert(failed.clone());
        record_history(&st, &failed, false);
        return Json(serde_json::json!({
            "ok": false,
            "code": "credential_invalid",
            "credential": cred,
            "message": format!("凭证无法访问上游（可能已失效或被风控）：{hint}。请重新登录后导入新凭证。"),
        }))
        .into_response();
    }
    // 成功 → 落盘账号快照 + 追加使用记录（供凭证列表与历史查询）
    let meta = build_cred_meta(
        &cid,
        iv.as_ref(),
        usage.as_ref().ok(),
        subs.as_ref().ok(),
        qv.as_ref(),
    );
    let meta = crate::account_meta::CredMeta { valid: true, ..meta };
    if let Err(e) = st.meta.upsert(meta.clone()) {
        tracing::warn!("写入账号信息缓存失败: {e}");
    }
    record_history(&st, &meta, true);
    st.logs.emit("info", "account", None, "账号全貌已刷新");
    Json(serde_json::json!({
        "ok": true,
        "identity": identity.ok(),
        "usage": usage.ok(),
        "subscription": subs.ok(),
        "quota": quota.ok(),
        "credential": cred,
        "meta": meta,
        "fetched_at": chrono::Utc::now().to_rfc3339(),
    }))
    .into_response()
}

/// POST /api/account/refresh — 凭证保活/健康检查：调 convex-token 验证 Cookie 是否仍有效
async fn handle_account_refresh(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(resp) = write_guard(&headers, &st) {
        return resp;
    }
    let (cookie, cred, cid) = match pick_web_cookie(&st) {
        Some(v) => v,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "ok": false, "code": "need_cookie", "message": "未找到 web Cookie 凭证" })),
            )
                .into_response();
        }
    };
    let client = match crate::web_protocol::WebClient::new(cookie, "glm-5.3-flash".into()) {
        Ok(c) => c,
        Err(e) => return internal_err(&anyhow::Error::msg(e.to_string())),
    };
    match client.convex_token().await {
        Ok(v) => {
            let has_token = v.get("token").and_then(|t| t.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
            st.logs.emit("info", "account", None, "凭证保活检查通过（已刷新短期 token）");
            Json(serde_json::json!({
                "ok": true,
                "valid": has_token,
                "credential": cred,
                "meta": st.meta.get(&cid),
                "message": if has_token { "凭证有效，已刷新短期 token（约 5 分钟有效，网关自动复用）" } else { "端点可达但未返回 token" },
            }))
            .into_response()
        }
        Err(e) => {
            st.logs.emit("warn", "account", None, format!("凭证保活检查失败: {e}"));
            // 失效也要落一条，凭证列表能直接看到"这条已失效"
            let mut failed = crate::account_meta::CredMeta {
                cred_id: cid.clone(),
                valid: false,
                error: Some(e.to_string()),
                checked_at: chrono::Utc::now().to_rfc3339(),
                ..Default::default()
            };
            if let Some(prev) = st.meta.get(&cid) {
                failed.name = prev.name;
                failed.email = prev.email;
                failed.tier_id = prev.tier_id;
                failed.daily_remaining = prev.daily_remaining;
                failed.daily_limit = prev.daily_limit;
            }
            let _ = st.meta.upsert(failed.clone());
            record_history(&st, &failed, false);
            Json(serde_json::json!({
                "ok": false,
                "valid": false,
                "credential": cred,
                "meta": failed,
                "message": format!("凭证可能已失效（请重新登录导入）：{e}"),
            }))
            .into_response()
        }
    }
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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_messages_text_blocks_flattened() {
        // Claude content blocks 数组 → OpenAI content 字符串
        let msgs = serde_json::json!([
            { "role": "user", "content": [
                { "type": "text", "text": "hello" },
                { "type": "text", "text": "world" }
            ]}
        ]);
        let out = claude_to_openai_messages(msgs);
        let arr = out.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["role"], "user");
        assert_eq!(arr[0]["content"], "hello\nworld");
    }

    #[test]
    fn claude_tool_use_to_tool_calls() {
        // assistant 的 tool_use 块 → OpenAI assistant.tool_calls
        let msgs = serde_json::json!([
            { "role": "assistant", "content": [
                { "type": "text", "text": "let me check" },
                { "type": "tool_use", "id": "toolu_1", "name": "get_weather", "input": { "city": "SF" } }
            ]}
        ]);
        let out = claude_to_openai_messages(msgs);
        let a = &out.as_array().unwrap()[0];
        assert_eq!(a["role"], "assistant");
        assert_eq!(a["content"], "let me check");
        let tc = &a["tool_calls"][0];
        assert_eq!(tc["id"], "toolu_1");
        assert_eq!(tc["function"]["name"], "get_weather");
        assert!(tc["function"]["arguments"].as_str().unwrap().contains("SF"));
    }

    #[test]
    fn claude_tool_result_to_tool_role() {
        // user 的 tool_result 块 → OpenAI role=tool 消息
        let msgs = serde_json::json!([
            { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "toolu_1", "content": "18C sunny" }
            ]}
        ]);
        let out = claude_to_openai_messages(msgs);
        let a = &out.as_array().unwrap()[0];
        assert_eq!(a["role"], "tool");
        assert_eq!(a["tool_call_id"], "toolu_1");
        assert_eq!(a["content"], "18C sunny");
    }

    #[test]
    fn openai_response_to_claude_shape() {
        let oai = serde_json::json!({
            "id": "chatcmpl-1",
            "choices": [{ "message": { "role": "assistant", "content": "hi there" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 3 }
        });
        let c = openai_to_claude_response(&oai, "z-ai/glm-5.3-flash");
        assert_eq!(c["type"], "message");
        assert_eq!(c["role"], "assistant");
        assert_eq!(c["stop_reason"], "end_turn");
        assert_eq!(c["content"][0]["type"], "text");
        assert_eq!(c["content"][0]["text"], "hi there");
        assert_eq!(c["usage"]["input_tokens"], 10);
        assert_eq!(c["usage"]["output_tokens"], 3);
    }

    #[test]
    fn openai_tool_calls_to_claude_tool_use() {
        let oai = serde_json::json!({
            "id": "chatcmpl-2",
            "choices": [{ "message": { "role": "assistant", "content": null, "tool_calls": [
                { "id": "call_1", "type": "function", "function": { "name": "lookup", "arguments": "{\"q\":\"x\"}" } }
            ]}, "finish_reason": "tool_calls" }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
        });
        let c = openai_to_claude_response(&oai, "m");
        assert_eq!(c["stop_reason"], "tool_use");
        assert_eq!(c["content"][0]["type"], "tool_use");
        assert_eq!(c["content"][0]["name"], "lookup");
        assert_eq!(c["content"][0]["input"]["q"], "x");
    }

    #[test]
    fn mask_hides_middle() {
        let m = mask("sk-abcdefghijklmnop1234");
        assert!(m.starts_with("sk-abc"));
        assert!(m.ends_with("1234"));
        assert!(m.contains("..."));
        // 短 token 不 panic
        assert!(mask("abc").ends_with("***"));
    }

    #[test]
    fn mask_does_not_leak_short_secrets() {
        // 短凭证必须几乎不可读：旧实现会把 "sk-local" 回显成 "sk-lo***"（泄漏 5/8 字符）
        let short = mask("sk-local");
        assert!(!short.contains("local"), "短凭证泄漏了原文: {short}");
        assert!(short.len() <= 6, "短凭证脱敏后过长: {short}");
        // 4 字符及以下完全遮蔽
        assert_eq!(mask("abcd"), "***");
        assert_eq!(mask(""), "***");
        // 边界：12 字符走"首2+尾2"分支，仍不吐中间
        let m12 = mask("abcdefghijkl");
        assert_eq!(m12, "ab***kl");
    }

    #[test]
    fn mask_is_multibyte_safe() {
        // 多字节字符不得触发 char boundary panic
        let m = mask("中文凭证测试内容中文凭证测试内容");
        assert!(m.contains("..."));
        assert!(!mask("中文").is_empty());
    }

    #[test]
    fn loopback_detection_by_proxy_headers() {
        let mut h = HeaderMap::new();
        assert!(is_loopback_request(&h));
        h.insert("x-forwarded-for", "1.2.3.4".parse().unwrap());
        assert!(!is_loopback_request(&h));
        let mut h2 = HeaderMap::new();
        h2.insert("x-real-ip", "1.2.3.4".parse().unwrap());
        assert!(!is_loopback_request(&h2));
    }

    #[test]
    fn authorized_accepts_bearer_and_x_api_key() {
        let keys = vec!["sk-local".to_string()];
        let mut h = HeaderMap::new();
        assert!(!authorized(&h, &keys));
        h.insert("authorization", "Bearer sk-local".parse().unwrap());
        assert!(authorized(&h, &keys));
        let mut h2 = HeaderMap::new();
        h2.insert("x-api-key", "sk-local".parse().unwrap());
        assert!(authorized(&h2, &keys));
        let mut h3 = HeaderMap::new();
        h3.insert("authorization", "Bearer wrong".parse().unwrap());
        assert!(!authorized(&h3, &keys));
    }

    #[test]
    fn anonymous_session_is_not_authenticated() {
        // 上游对无效凭证返回 200 + {}（实测），必须判为未登录，否则假凭证会被当成"有效"
        let anon = serde_json::json!({});
        assert!(!session_is_authenticated(&anon));
        // user 为 null / 空对象同样不算登录
        assert!(!session_is_authenticated(&serde_json::json!({"user": null})));
        assert!(!session_is_authenticated(&serde_json::json!({"user": {}})));
        // 真实会话必须被识别
        let real = serde_json::json!({"user": {"id": "88d1", "email": "a@b.c"}, "expires": "2026-10-09T13:27:07.358Z"});
        assert!(session_is_authenticated(&real));
    }

    #[test]
    fn quota_authentication_signal() {
        assert!(!quota_is_authenticated(&serde_json::json!({"error": "Unauthorized"})));
        assert!(quota_is_authenticated(&serde_json::json!({"accessTier": "limited"})));
        assert!(quota_is_authenticated(&serde_json::json!({"freebucks": {"balance": 20}})));
    }

    #[test]
    fn credential_usable_requires_real_signal() {
        let anon = serde_json::json!({});
        assert!(!credential_usable(Some(&anon), None), "空 session 不得判为可用");
        assert!(!credential_usable(None, None));
        let real = serde_json::json!({"user": {"email": "a@b.c"}});
        assert!(credential_usable(Some(&real), None));
        let quota = serde_json::json!({"accessTier": "limited"});
        assert!(credential_usable(None, Some(&quota)));
    }

    #[test]
    fn bridge_error_detection() {
        // 正常流：无 error 字样 → None
        assert_eq!(detect_bridge_error("data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n"), None);
        // 上游 200 内嵌 error envelope → 分类为 auth 类（对齐 errors::classify）
        let sse = "data: {\"error\":{\"message\":\"Unauthorized\",\"type\":\"auth\"}}\n\n";
        assert_eq!(detect_bridge_error(sse), Some("auth_expired"));
        // error 是字符串形式同样识别
        let sse2 = "data: {\"error\":\"Unauthorized\"}\n\n";
        assert!(detect_bridge_error(sse2).is_some(), "字符串 error 也必须识别");
        // error 字样出现在正文里（非 envelope）→ 不误报
        let false_pos = "data: {\"choices\":[{\"delta\":{\"content\":\"他说 error 这个词\"}}]}\n\n";
        assert_eq!(detect_bridge_error(false_pos), None);
        // 空流
        assert_eq!(detect_bridge_error(""), None);
    }

    #[test]
    fn placeholder_tokens_do_not_block_bridge() {
        // 用户 config 里常见占位符（__TEST_SKIP__ 等）会让账号池"看似非空"，
        // 既阻断 web 桥接又在桌面协议上必然 401（Cherry Studio 场景实测复现）。
        // 判定规则：token 长度 >= 20 且不以 __TEST_SKIP__ 开头才算"可用 Bearer"。
        let usable = |tok: &str| tok.len() >= 20 && !tok.starts_with("__TEST_SKIP__");
        assert!(!usable("__TEST_SKIP__"), "占位符不得视为可用");
        assert!(!usable("short"), "超短 token 不得视为可用");
        assert!(usable("__Secure-next-auth.session-token=abc123; x=1"), "真实长 token 可用");
        // 注意：web-cookie 凭证根本不该进池（v0.5.0 修复），这里只兜底占位符场景
    }
}

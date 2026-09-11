//! Web 版协议客户端（freebuff.com）
//!
//! 逆向自 Freebuff web 版网络包：
//! - POST /api/chat/stream  —— Cookie 鉴权，SSE 流式（11 种事件类型）
//! - POST /api/chat/upload   —— multipart 上传 → convex storageId → images
//! - GET  /api/web/freebuff-session —— 积分/套餐/每模型每日限额
//! - GET  /api/web/usage-summary —— 用量汇总（streak/tokens/sessionsByModel）
//! - GET  /api/auth/session   —— 用户信息
//! - GET  /api/web/subscriptions —— 套餐 tiers
//! - GET  /api/web/convex-token —— token 续期（短期 JWT）
//!
//! 鉴权靠 Cookie（__Secure-next-auth.session-token 等），无 Bearer。

use anyhow::{anyhow, Result};
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const WEB_HOST: &str = "https://freebuff.com";

/// FNV-1a 64 位（零依赖、跨版本稳定；与 `import::cred_id` 同算法，保证指纹可重现）
fn fnv1a64(s: &str) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(PRIME);
    }
    h
}

/// 指纹种子：取 session-token 的值（无则退化为整串 Cookie）。
/// 种子只在本机参与哈希，原始 Cookie 不外传。
fn session_seed(cookie: &str) -> String {
    cookie
        .split(';')
        .map(str::trim)
        .find(|p| p.starts_with("__Secure-next-auth.session-token="))
        .map(|p| p.trim_start_matches("__Secure-next-auth.session-token=").to_string())
        .unwrap_or_else(|| cookie.to_string())
}

/// 上游抓包中客户端会带 `x-freebuff-instance-id`（UUID 形状）。
/// 此前网关完全不发这个头 —— 抓包里有、我们却没有，是最容易被风控识别的差异之一。
/// 这里按 Cookie 派生：同账号每次启动都得到同一个值，不同账号得到不同值。
pub fn instance_id_for_cookie(cookie: &str) -> String {
    let seed = session_seed(cookie);
    let a = fnv1a64(&seed);
    let b = fnv1a64(&format!("{seed}#instance"));
    let hex = format!("{a:016x}{b:016x}"); // 32 个 ASCII 十六进制字符
    format!(
        "{}-{}-4{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[derive(Debug, Clone)]
pub struct WebClient {
    http: reqwest::Client,
    pub cookie: String,
    pub model: String,
    /// 按 Cookie 派生的实例 id（上游 `x-freebuff-instance-id`；同账号稳定、跨账号不同）
    instance_id: String,
    /// 最近一次流中上游 meta/title 事件给出的 threadId（多轮续聊用）。
    /// 并发多路流共用同一 WebClient 时以最后写入者为准。
    last_thread_id: Arc<Mutex<Option<String>>>,
    /// 上游 200 内嵌错误旁路（本路流最近一次检测到的 error envelope）。
    /// encode_block 在反序列化前探测——桥接层 tail 只含转换后 chunk，这是唯一可见点。
    last_upstream_error: Arc<Mutex<Option<String>>>,
}

/// chat/stream SSE 事件（web 版 11 种类型）
///
/// 注意：上游 JSON 的字段名为 camelCase（`threadId`/`toolCallId`/`accessTier`），
/// 而 `rename_all` 只作用于变体名，故必须用 `rename_all_fields` 映射字段名，
/// 否则带下划线的字段会静默落为 None。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ChatEvent {
    Meta {
        #[serde(default)]
        thread_id: Option<String>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        access_tier: Option<String>,
    },
    Title {
        #[serde(default)]
        thread_id: Option<String>,
        #[serde(default)]
        title: Option<String>,
    },
    ReasoningDelta {
        text: String,
    },
    Delta {
        text: String,
    },
    Suggestions {
        #[serde(default)]
        tool_call_id: Option<String>,
        #[serde(default)]
        followups: Vec<Followup>,
    },
    AgentStart {
        #[serde(default)]
        agent_id: Option<String>,
        #[serde(default)]
        agent_type: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        prompt: Option<String>,
    },
    AgentTool {
        #[serde(default)]
        agent_id: Option<String>,
        #[serde(default)]
        tool_call_id: Option<String>,
        #[serde(default)]
        tool_name: Option<String>,
        #[serde(default)]
        label: Option<String>,
    },
    AgentToolDone {
        #[serde(default)]
        tool_call_id: Option<String>,
    },
    AgentDelta {
        #[serde(default)]
        agent_id: Option<String>,
        #[serde(default)]
        text: Option<String>,
    },
    AgentFinish {
        #[serde(default)]
        agent_id: Option<String>,
    },
    Button,
    Done,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Followup {
    pub prompt: String,
    pub label: String,
}

/// 累计流式结果
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreamResult {
    pub thread_id: Option<String>,
    pub title: Option<String>,
    pub model: Option<String>,
    pub access_tier: Option<String>,
    pub reasoning: String,
    pub text: String,
    pub suggestions: Vec<Followup>,
    pub tools: Vec<String>,
    pub done: bool,
    /// 转 OpenAI tool_calls 的中间态（agent_tool → agent_tool_done 对）
    #[serde(default)]
    pub tool_calls: Vec<ToolCallState>,
}

/// 上游 agent_tool 事件 → OpenAI 工具调用状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallState {
    pub id: String,
    pub name: String,
    pub label: String,
    pub done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GravityContext {
    pub user_data: GravityUserData,
    #[serde(rename = "event_source_url")]
    pub event_source_url: String,
    #[serde(rename = "client_context")]
    pub client_context: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GravityUserData {
    #[serde(rename = "visitor_id")]
    pub visitor_id: String,
    #[serde(rename = "session_id")]
    pub session_id: String,
    #[serde(rename = "client_user_agent")]
    pub client_user_agent: String,
}

impl GravityContext {
    /// 按 Cookie 派生确定性指纹（同账号稳定、不同账号不同）。
    /// 此前硬编码抓包中的 visitor_id/session_id，所有用户共用同一指纹——易被上游风控识别为同一客户端。
    /// 用 Cookie 中的 session-token 值作为种子（无则用整串），仅本地计算，不外传原始 Cookie。
    pub fn for_cookie(cookie: &str) -> Self {
        let seed = session_seed(cookie);
        let v = fnv1a64(&seed);
        let v2 = fnv1a64(&format!("{seed}#client-ctx"));
        let mut ctx = Self::default();
        ctx.user_data.visitor_id = format!("gruid_{v:016x}{:08x}", v.rotate_left(21));
        ctx.user_data.session_id = format!("gr_sess_{:016x}{:08x}", v.rotate_right(13), v ^ 0x9E37_79B9_7F4A_7C15);
        // 客户端环境也按账号派生：所有账号共用同一套 screen/viewport/hardware 同样是可识别特征
        let pick = |salt: u64, lo: u64, hi: u64| -> u64 { lo + (fnv1a64(&format!("{seed}#{salt}")) % (hi - lo + 1)) };
        let screen_w = pick(1, 1366, 2560);
        let screen_h = pick(2, 768, 1440);
        let viewport_w = pick(3, 1024, 1600);
        let viewport_h = pick(4, 720, 1000);
        let dpr_choices = [1.0_f64, 1.25, 1.5, 2.0];
        let mem_choices = [8_u64, 16, 16, 32, 32];
        let hw_choices = [4_u64, 8, 12, 16, 24];
        let dpr = dpr_choices[(v2 % 4) as usize];
        let device_memory = mem_choices[(v % 5) as usize];
        let hardware_concurrency = hw_choices[(v2 % 5) as usize];
        ctx.client_context = serde_json::json!({
            "timezone": "Asia/Shanghai",
            "screen": {"width": screen_w, "height": screen_h, "color_depth": 24, "pixel_depth": 24},
            "viewport": {"width": viewport_w, "height": viewport_h},
            "device_pixel_ratio": dpr,
            "platform": "Windows",
            "device_memory": device_memory,
            "hardware_concurrency": hardware_concurrency,
            "max_touch_points": null,
            "connection": {"effective_type": "4g", "downlink": 8.3, "rtt": 250, "save_data": false},
            "font": "Arial",
            "webgl": null,
            "fonts": ["Arial", "Segoe UI", "Consolas"],
            "audio_fingerprint": "0",
            "navigator_ext": {"languages": ["zh-CN", "zh"], "webdriver": false, "pdf_viewer": true, "cookies_enabled": true},
            "math_fingerprint": "0"
        });
        ctx
    }
}

impl Default for GravityContext {
    fn default() -> Self {        Self {
            user_data: GravityUserData {
                visitor_id: "gruid_20uznrh75l83tnm3".into(),
                session_id: "gr_sess_5o0e95gndmgtdd5l".into(),
                client_user_agent: crate::upstream::DESKTOP_UA.into(),
            },
            event_source_url: "https://freebuff.com/chat".into(),
            client_context: serde_json::json!({
                "timezone": "Asia/Shanghai",
                "screen": {"width": 1707, "height": 1067, "color_depth": 24, "pixel_depth": 24},
                "viewport": {"width": 1036, "height": 906},
                "device_pixel_ratio": 1.5,
                "platform": "Windows",
                "device_memory": 32,
                "hardware_concurrency": 24,
                "max_touch_points": null,
                "connection": {"effective_type": "4g", "downlink": 8.3, "rtt": 250, "save_data": false},
                "font": "Arial",
                "webgl": null,
                "fonts": ["Arial", "Segoe UI", "Consolas"],
                "audio_fingerprint": "0",
                "navigator_ext": {"languages": ["zh-CN", "zh"], "webdriver": false, "pdf_viewer": true, "cookies_enabled": true},
                "math_fingerprint": "0"
            }),
        }
    }
}

impl WebClient {
    pub fn new(cookie: String, model: String) -> Result<Self> {
        // 流式响应不受整体 timeout 影响：reqwest 的 .timeout() 是「总请求」超时，对流式会截断
        // （v0.7.3 修复：此前 300s 总超时会把仍在增量的长流硬生生掐断，客户端表现为
        //  ERR_INCOMPLETE_CHUNKED_ENCODING）。与 upstream.rs 同款做法：read_timeout =
        //  单次读块超时——只要增量还在吐就一直收，只有完全静默 5 分钟才断。
        let http = reqwest::Client::builder()
            .read_timeout(Duration::from_secs(300))
            .user_agent(crate::upstream::DESKTOP_UA)
            .connect_timeout(Duration::from_secs(15))
            .cookie_store(true)
            .build()?;
        let instance_id = instance_id_for_cookie(&cookie);
        Ok(Self {
            http,
            cookie,
            model,
            instance_id,
            last_thread_id: Arc::new(Mutex::new(None)),
            last_upstream_error: Arc::new(Mutex::new(None)),
        })
    }

    /// 最近一次 web 流中上游 meta/title 事件给出的 threadId。
    /// 调用后可用于下一轮请求的 `thread_id` 续聊；从未收到则返回 None。
    pub fn last_thread_id(&self) -> Option<String> {
        self.last_thread_id.lock().ok().and_then(|g| g.clone())
    }

    /// 内部：threadId 记录槽（克隆 Arc 供流闭包持有，避免借用 self）
    fn thread_slot(&self) -> Arc<Mutex<Option<String>>> {
        self.last_thread_id.clone()
    }

    /// 内部：上游内嵌错误旁路槽（同 thread_slot 模式）
    fn error_slot(&self) -> Arc<Mutex<Option<String>>> {
        self.last_upstream_error.clone()
    }

    /// 本 WebClient 最近一路流中检测到的上游内嵌错误（200 OK + error envelope）。
    /// 桥接层在流结束时读取，用于遥测/日志如实记录失败原因。
    pub fn last_upstream_error(&self) -> Option<String> {
        self.last_upstream_error.lock().ok().and_then(|g| g.clone())
    }

    fn headers(&self, json: bool) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(&self.cookie) {
            h.insert("cookie", v);
        }
        h.insert("origin", HeaderValue::from_static("https://freebuff.com"));
        h.insert("referer", HeaderValue::from_static("https://freebuff.com/chat"));
        h.insert("accept", HeaderValue::from_static("*/*"));
        // 上游网页版每个请求都会带这个头（高级功能.txt:610/804）；缺失是最容易被风控识别的差异
        if let Ok(v) = HeaderValue::from_str(&self.instance_id) {
            h.insert("x-freebuff-instance-id", v);
        }
        if json {
            h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        }
        h
    }

    /// web 聊天流（SSE，真实增量透传）。返回逐事件 body stream。
    /// 每个上游 data: 事件实时转换为 OpenAI chunk 输出，不聚合不缓冲。
    /// 上游 meta/title 事件携带的 threadId 通过 [`WebClient::last_thread_id`] 暴露
    /// （流结束/收到 meta 后可读，用于多轮续聊）。
    pub async fn chat_stream_raw(
        &self,
        thread_id: Option<&str>,
        content: &str,
        reasoning_effort: Option<&str>,
        images: Vec<WebImage>,
        attachments: Vec<WebAttachment>,
    ) -> Result<axum::body::Body> {
        use futures::StreamExt;
        let body = serde_json::json!({
            "threadId": thread_id,
            "content": content,
            "model": self.model,
            "reasoningEffort": reasoning_effort,
            "gravity": GravityContext::for_cookie(&self.cookie),
            "images": images,
            "attachments": attachments,
        });
        let url = format!("{WEB_HOST}/api/chat/stream");
        let resp = self.http.post(&url).headers(self.headers(true)).json(&body).send().await?;
        if !resp.status().is_success() {
            return Err(anyhow!("web chat HTTP {}: {}", resp.status(), resp.text().await.unwrap_or_default()));
        }

        let byte_stream = resp.bytes_stream();
        let sse_buf: Vec<u8> = Vec::new();
        // 每路流独立的转换器：tool_calls index 分配 + threadId 记录 + 上游内嵌错误旁路
        let error_slot = self.error_slot();
        let encoder = StreamEncoder::new(self.thread_slot(), error_slot);
        let stream = futures::stream::unfold((byte_stream, sse_buf, false, encoder), |(mut stream, mut buf, finished, mut enc)| async move {
            if finished { return None; }
            loop {
                match stream.next().await {
                    Some(Ok(chunk)) => {
                        buf.extend_from_slice(&chunk);
                        // 切出完整事件块（空行分隔），逐块转 OpenAI chunk（真实增量）
                        let mut out_line = String::new();
                        let mut got_done = false;
                        while let Some(pos) = find_double_newline(&buf) {
                            let event_bytes: Vec<u8> = buf.drain(..pos).collect();
                            let block = String::from_utf8_lossy(&event_bytes);
                            let (text, done) = enc.encode_block(&block);
                            out_line.push_str(&text);
                            if done { got_done = true; break; }
                        }
                        if got_done {
                            // encode_block 已输出 finish_reason chunk + [DONE]
                            return Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(out_line)), (stream, buf, true, enc)));
                        }
                        if !out_line.is_empty() {
                            return Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(out_line)), (stream, buf, false, enc)));
                        }
                        // buf 已无完整事件，继续收下一个 chunk
                    }
                    Some(Err(e)) => {
                        return Some((Err::<_, std::io::Error>(std::io::Error::other(e.to_string())), (stream, buf, false, enc)));
                    }
                    None => {
                        // 上游结束但没收到 done → 补发 finish_reason chunk + [DONE]
                        let out = format!("{}data: [DONE]\n\n", enc.finish_chunk());
                        return Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(out)), (stream, buf, true, enc)));
                    }
                }
            }
        });
        Ok(axum::body::Body::from_stream(stream))
    }

    /// web 聊天流（SSE 聚合版，一次性返回全文）
    pub async fn chat_stream(
        &self,
        thread_id: Option<&str>,
        content: &str,
        reasoning_effort: Option<&str>,
        images: Vec<WebImage>,
        attachments: Vec<WebAttachment>,
    ) -> Result<StreamResult> {
        let body = serde_json::json!({
            "threadId": thread_id,
            "content": content,
            "model": self.model,
            "reasoningEffort": reasoning_effort,
            "gravity": GravityContext::for_cookie(&self.cookie),
            "images": images,
            "attachments": attachments,
        });
        let url = format!("{WEB_HOST}/api/chat/stream");
        let resp = self.http.post(&url).headers(self.headers(true)).json(&body).send().await?;
        if !resp.status().is_success() {
            return Err(anyhow!("web chat HTTP {}: {}", resp.status(), resp.text().await.unwrap_or_default()));
        }
        let mut result = StreamResult::default();
        let mut stream = resp.bytes_stream();
        use futures::StreamExt;
        // 逐行解析 SSE（data: {json} 行，空行分隔事件）
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buf.extend_from_slice(&chunk);
            // 按 \n\n 切事件
            while let Some(pos) = find_double_newline(&buf) {
                let event_line = buf.drain(..pos).collect::<Vec<u8>>();
                let line_str = String::from_utf8_lossy(&event_line);
                for line in line_str.lines() {
                    let line = line.trim();
                    if let Some(json) = line.strip_prefix("data:") {
                        let json = json.trim();
                        if json.is_empty() || json == "[DONE]" {
                            continue;
                        }
                        if let Ok(event) = serde_json::from_str::<ChatEvent>(json) {
                            apply_event(&mut result, event);
                        }
                    }
                }
            }
        }
        // 聚合路径同样记录 threadId（供后续续聊）
        record_thread_id(&self.last_thread_id, result.thread_id.as_deref());
        Ok(result)
    }

    /// 上传文件（multipart）→ storageId；model 字段由调用方指定
    /// 响应：`{kind:"image"|"document", storageId, url?(图片), mediaType, name,
    ///        descriptionStorageId?(图片), chars?(文档), truncated?(文档)}`
    pub async fn upload_with_model(
        &self,
        file_bytes: Vec<u8>,
        filename: &str,
        mime: &str,
        model: &str,
    ) -> Result<WebUploadResult> {
        let url = format!("{WEB_HOST}/api/chat/upload");
        let form = reqwest::multipart::Form::new()
            .part("file", reqwest::multipart::Part::bytes(file_bytes).file_name(filename.to_string()).mime_str(mime)?)
            .text("model", model.to_string());
        let resp = self.http.post(&url).headers(self.headers(false)).multipart(form).send().await?;
        if !resp.status().is_success() {
            return Err(anyhow!("upload HTTP {}: {}", resp.status(), resp.text().await.unwrap_or_default()));
        }
        Ok(resp.json().await?)
    }

    /// 上传文件（multipart）→ storageId（使用客户端配置的默认 model）
    pub async fn upload(&self, file_bytes: Vec<u8>, filename: &str, mime: &str) -> Result<WebUploadResult> {
        self.upload_with_model(file_bytes, filename, mime, &self.model).await
    }

    /// 删除上游会话（用户批注：反代不能长期堆积给上游制造压力）。
    ///
    /// 端点已**实证**（2026-09-11，真实凭证探针）：
    /// - `DELETE /api/chat/threads/{id}` → 不存在的路由返回 HTML 404 页，而该路径返回 JSON
    ///   `{"error":"Not found"}`，说明**路由存在**、只是没找到该 thread；删除成功为 2xx。
    /// - `POST /api/chat/threads/delete` → `405 Method Not Allowed`（被动态路由 `[id]` 吃掉，
    ///   id="delete"），**不是**独立端点，故不再作为回退。
    /// - 线程列表：`GET /api/chat/threads` → `{"threads":[{id,title,model,updated_at}],...}`（同样实证）。
    ///
    /// 返回 `Ok(true)` 表示上游确认删除；`Ok(false)` 表示上游明确说找不到；其他失败返回 `Err`。
    pub async fn delete_thread(&self, thread_id: &str) -> Result<bool> {
        let url = format!("{WEB_HOST}/api/chat/threads/{thread_id}");
        let resp = self.http.delete(&url).headers(self.headers(false)).send().await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(true);
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            // 路由存在但该 thread 已不在上游 —— 视作"已经不需要清理"
            return Ok(false);
        }
        Err(anyhow!("上游删除会话失败：DELETE {url} → HTTP {status}"))
    }

    /// 查询账号积分/每模型限额（web 版核心余额端点）
    pub async fn freebuff_session(&self) -> Result<WebFreebuffSession> {
        let url = format!("{WEB_HOST}/api/web/freebuff-session");
        let resp = self.http.get(&url).headers(self.headers(false)).send().await?;
        parse_json(resp).await
    }

    /// 用量汇总（streak/tokens/sessionsByModel）
    pub async fn usage_summary(&self) -> Result<serde_json::Value> {
        let url = format!("{WEB_HOST}/api/web/usage-summary");
        let resp = self.http.get(&url).headers(self.headers(false)).send().await?;
        parse_json(resp).await
    }

    /// 用户信息
    pub async fn auth_session(&self) -> Result<serde_json::Value> {
        let url = format!("{WEB_HOST}/api/auth/session");
        let resp = self.http.get(&url).headers(self.headers(false)).send().await?;
        parse_json(resp).await
    }

    /// 套餐
    pub async fn subscriptions(&self) -> Result<serde_json::Value> {
        let url = format!("{WEB_HOST}/api/web/subscriptions");
        let resp = self.http.get(&url).headers(self.headers(false)).send().await?;
        parse_json(resp).await
    }

    /// 会话列表
    pub async fn threads(&self) -> Result<serde_json::Value> {
        let url = format!("{WEB_HOST}/api/chat/threads");
        let resp = self.http.get(&url).headers(self.headers(false)).send().await?;
        parse_json(resp).await
    }

    /// 短期 JWT（GET /api/web/convex-token；含 email/name/access_tier/country_code，约 5 分钟有效）
    /// 用途：验证凭证有效性 / 保活。
    pub async fn convex_token(&self) -> Result<serde_json::Value> {
        let url = format!("{WEB_HOST}/api/web/convex-token");
        let resp = self.http.get(&url).headers(self.headers(false)).send().await?;
        parse_json(resp).await
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebImage {
    #[serde(rename = "storageId")]
    pub storage_id: String,
    #[serde(rename = "mediaType")]
    pub media_type: String,
    pub name: String,
    #[serde(rename = "descriptionStorageId", default)]
    pub description_storage_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebAttachment {
    #[serde(rename = "storageId")]
    pub storage_id: String,
    #[serde(rename = "mediaType")]
    pub media_type: String,
    pub name: String,
    #[serde(default)]
    pub chars: Option<i64>,
    #[serde(default)]
    pub truncated: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebUploadResult {
    pub kind: String,
    #[serde(rename = "storageId")]
    pub storage_id: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(rename = "mediaType")]
    pub media_type: String,
    pub name: String,
    #[serde(rename = "descriptionStorageId", default)]
    pub description_storage_id: Option<String>,
    #[serde(default)]
    pub chars: Option<i64>,
    #[serde(default)]
    pub truncated: Option<bool>,
}

/// 账号余额全景（web 版 session 响应解析）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WebFreebuffSession {
    pub status: Option<String>,
    #[serde(rename = "accessTier")]
    pub access_tier: Option<String>,
    #[serde(default)]
    pub freebucks: Option<Freebucks>,
    #[serde(default)]
    pub subscription: Option<serde_json::Value>,
    #[serde(rename = "rateLimitsByModel", default)]
    pub rate_limits_by_model: Option<serde_json::Value>,
    #[serde(default)]
    pub referral: Option<serde_json::Value>,
    #[serde(rename = "countryCode", default)]
    pub country_code: Option<String>,
    #[serde(rename = "countryBlockReason", default)]
    pub country_block_reason: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Freebucks {
    #[serde(default)]
    pub balance: f64,
    #[serde(default)]
    pub daily: Option<FreebucksDaily>,
    #[serde(default)]
    pub wallet: Option<serde_json::Value>,
    #[serde(rename = "planId", default)]
    pub plan_id: Option<String>,
    #[serde(default)]
    pub prices: Option<HashMap<String, f64>>,
    #[serde(rename = "priceNotices", default)]
    pub price_notices: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FreebucksDaily {
    #[serde(default)]
    pub limit: f64,
    #[serde(default)]
    pub spent: f64,
    #[serde(default)]
    pub remaining: f64,
    #[serde(rename = "resetAt")]
    pub reset_at: Option<String>,
}

/// 单路流的上游 SSE 事件块 → OpenAI chunk 转换器。
///
/// 持有本路流的状态：
/// - `tool_calls` 的 index 分配（同一 toolCallId 复用同一 index，并行工具不互相覆盖）
/// - 是否出现过工具调用（决定终止 chunk 的 finish_reason）
/// - threadId 记录槽（meta/title 事件）
struct StreamEncoder {
    tool_index: HashMap<String, usize>,
    next_tool_index: usize,
    has_tool_calls: bool,
    thread_id: Arc<Mutex<Option<String>>>,
    /// 上游 200 内嵌错误旁路：encode_block 反序列化失败/未知事件的行里若含
    /// error envelope，在此记录（Critic-J P2-1：检测点前移到能看到上游原始事件的层面）
    upstream_error: Option<String>,
    /// 错误旁路槽（与 WebClient 共享，桥接层流结束后可读）
    _error_slot: Arc<Mutex<Option<String>>>,
}

impl StreamEncoder {
    fn new(thread_id: Arc<Mutex<Option<String>>>, error_slot: Arc<Mutex<Option<String>>>) -> Self {
        Self { tool_index: HashMap::new(), next_tool_index: 0, has_tool_calls: false, thread_id, upstream_error: None, _error_slot: error_slot }
    }

    /// 读取上游内嵌错误（若有）。取后不清——同一错误重复读取得到同一结果。
    #[cfg(test)]
    pub fn take_upstream_error(&self) -> Option<String> {
        self.upstream_error.clone()
    }

    /// 分配（或复用）toolCallId 对应的 OpenAI tool_calls index。
    /// 返回 (index, OpenAI 侧非空 id)：上游无 id 时用 `anon_{n}` 生成确定性 id。
    fn tool_slot(&mut self, raw_id: Option<&str>) -> (usize, String) {
        let key = match raw_id {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => format!("anon_{}", self.next_tool_index),
        };
        let index = match self.tool_index.get(&key) {
            Some(&i) => i,
            None => {
                let i = self.next_tool_index;
                self.tool_index.insert(key.clone(), i);
                self.next_tool_index += 1;
                i
            }
        };
        (index, format!("call_{key}"))
    }

    /// 终止 chunk：content 为空，finish_reason 按是否出现工具调用取 tool_calls/stop。
    fn finish_chunk(&self) -> String {
        let reason = if self.has_tool_calls { "tool_calls" } else { "stop" };
        format!(
            "data: {}\n\n",
            serde_json::json!({ "object": "chat.completion.chunk", "choices": [{ "index": 0, "delta": {}, "finish_reason": reason }] })
        )
    }

    /// 一个 SSE 事件块（可能含多条 data: 行）→ OpenAI chunk 文本。
    /// 返回 (输出文本, 是否收到 done)；收到 done 时输出已含 finish_reason chunk 与 [DONE] 哨兵。
    fn encode_block(&mut self, block: &str) -> (String, bool) {
        let mut text_parts: Vec<String> = Vec::new();
        let mut reasoning_parts: Vec<String> = Vec::new();
        let mut tool_parts: Vec<serde_json::Value> = Vec::new();
        let mut done = false;
        for line in block.lines() {
            let line = line.trim();
            let Some(json) = line.strip_prefix("data:") else { continue };
            let json = json.trim();
            if json.is_empty() || json == "[DONE]" { continue; }
            // 检测点前移（Critic-J P2-1）：先探 error envelope（含反序列化会失败的未知事件），
            // 桥接层的 tail 只含转换后 chunk，看不到这里——这是上游内嵌错误的唯一可见点
            if self.upstream_error.is_none() {
                if let Ok(raw) = serde_json::from_str::<serde_json::Value>(json) {
                    if let Some(err) = raw.get("error") {
                        let text = err.as_str().map(String::from).unwrap_or_else(|| err.to_string());
                        self.upstream_error = Some(text.clone());
                        if let Ok(mut slot) = self._error_slot.lock() {
                            *slot = Some(text); // 旁路槽：桥接层流结束后可读
                        }
                    }
                }
            }
            let Ok(event) = serde_json::from_str::<ChatEvent>(json) else { continue };
            match event {
                ChatEvent::Delta { text } => text_parts.push(text),
                // 抓包证据：工具产出的正文在 agent_delta 中，必须作为 content 透传
                ChatEvent::AgentDelta { text: Some(text), .. } => text_parts.push(text),
                ChatEvent::ReasoningDelta { text } => reasoning_parts.push(text),
                ChatEvent::AgentTool { tool_name: Some(name), tool_call_id, .. } => {
                    let (index, id) = self.tool_slot(tool_call_id.as_deref());
                    self.has_tool_calls = true;
                    tool_parts.push(serde_json::json!({
                        "index": index,
                        "id": id,
                        "type": "function",
                        "function": { "name": name, "arguments": "{}" }
                    }));
                }
                ChatEvent::Meta { thread_id, .. } | ChatEvent::Title { thread_id, .. } => {
                    record_thread_id(&self.thread_id, thread_id.as_deref());
                }
                ChatEvent::Done => done = true,
                _ => {}
            }
        }
        let mut out = String::new();
        for r in reasoning_parts {
            out += &format!("data: {}\n\n", serde_json::json!({ "object": "chat.completion.chunk", "choices": [{ "index": 0, "delta": { "reasoning_content": r }, "finish_reason": null }] }));
        }
        for t in text_parts {
            out += &format!("data: {}\n\n", serde_json::json!({ "object": "chat.completion.chunk", "choices": [{ "index": 0, "delta": { "content": t }, "finish_reason": null }] }));
        }
        for tc in &tool_parts {
            out += &format!("data: {}\n\n", serde_json::json!({ "object": "chat.completion.chunk", "choices": [{ "index": 0, "delta": { "tool_calls": [tc] }, "finish_reason": null }] }));
        }
        if done {
            out += &self.finish_chunk();
            out += "data: [DONE]\n\n";
        }
        (out, done)
    }
}

/// 记录上游给出的 threadId（空串忽略；后到覆盖，上游 meta/title 会重复携带同一 id）
fn record_thread_id(slot: &Mutex<Option<String>>, id: Option<&str>) {
    if let Some(id) = id.filter(|s| !s.is_empty()) {
        if let Ok(mut g) = slot.lock() {
            *g = Some(id.to_string());
        }
    }
}

/// 聚合路径的事件归并（`chat_stream`）。
fn apply_event(result: &mut StreamResult, event: ChatEvent) {
    match event {
        ChatEvent::Meta { thread_id, title, model, access_tier } => {
            if let Some(t) = thread_id { result.thread_id = Some(t); }
            // title 二次更新：流中途由用户原文覆盖为模型摘要，后到覆盖
            if let Some(t) = title { result.title = Some(t); }
            if result.model.is_none() { result.model = model; }
            if result.access_tier.is_none() { result.access_tier = access_tier; }
        }
        ChatEvent::Title { thread_id, title } => {
            if let Some(t) = thread_id { result.thread_id = Some(t); }
            if let Some(t) = title { result.title = Some(t); }
        }
        ChatEvent::ReasoningDelta { text } => result.reasoning.push_str(&text),
        ChatEvent::Delta { text } => result.text.push_str(&text),
        // 工具产出的正文（agent_delta）与普通 delta 同通道，不能丢弃
        ChatEvent::AgentDelta { text: Some(text), .. } => result.text.push_str(&text),
        ChatEvent::Suggestions { followups, .. } => result.suggestions = followups,
        ChatEvent::AgentTool { tool_name: Some(name), tool_call_id, label, .. } => {
            let raw = match tool_call_id.filter(|s| !s.is_empty()) {
                Some(id) => id,
                None => format!("anon_{}", result.tool_calls.len()),
            };
            let label = label.unwrap_or_default();
            result.tools.push(format!("{name}: {label}"));
            result.tool_calls.push(ToolCallState { id: format!("call_{raw}"), name, label, done: false });
        }
        ChatEvent::AgentToolDone { tool_call_id: Some(id) } => {
            let id = format!("call_{id}");
            if let Some(tc) = result.tool_calls.iter_mut().find(|t| t.id == id) {
                tc.done = true;
            }
        }
        ChatEvent::AgentStart { agent_type, .. } => result.tools.push(format!("agent_start: {}", agent_type.unwrap_or_default())),
        ChatEvent::Done => result.done = true,
        _ => {}
    }
}

/// 定位事件块结束位置（含分隔空行）。兼容 LF 与 CRLF 两种 SSE 编码。
fn find_double_newline(buf: &[u8]) -> Option<usize> {
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|i| i + 2);
    let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4);
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

async fn parse_json<T: for<'de> Deserialize<'de>>(resp: reqwest::Response) -> Result<T> {
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("HTTP {status}: {}", text.chars().take(300).collect::<String>()));
    }
    serde_json::from_str(&text).map_err(|e| anyhow!("解析响应失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_slot() -> Arc<Mutex<Option<String>>> {
        Arc::new(Mutex::new(None))
    }

    /// 按真实流的方式切块并编码：\n\n（或 \r\n\r\n）分隔，逐块 feed 编码器
    fn drive(enc: &mut StreamEncoder, raw: &[u8]) -> String {
        let mut buf = raw.to_vec();
        let mut out = String::new();
        while let Some(pos) = find_double_newline(&buf) {
            let block: Vec<u8> = buf.drain(..pos).collect();
            let (text, done) = enc.encode_block(&String::from_utf8_lossy(&block));
            out.push_str(&text);
            if done { break; }
        }
        out
    }

    /// 抽出 OpenAI chunk（跳过 [DONE] 哨兵）
    fn chunks(out: &str) -> Vec<serde_json::Value> {
        out.lines()
            .filter_map(|l| l.strip_prefix("data: "))
            .filter(|l| *l != "[DONE]")
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    fn contains(out: &str, needle: &str) -> bool {
        out.contains(needle)
    }

    // ---------- agent_delta → content ----------

    #[test]
    fn agent_delta_maps_to_content_delta() {
        let mut enc = StreamEncoder::new(new_slot(), new_slot());
        let (out, done) = enc.encode_block("data: {\"type\":\"agent_delta\",\"agentId\":\"a1\",\"text\":\"工具结果\"}\n\n");
        assert!(!done);
        let cs = chunks(&out);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0]["choices"][0]["delta"]["content"], "工具结果");
        assert!(cs[0]["choices"][0]["delta"].get("reasoning_content").is_none());
    }

    #[test]
    fn agent_delta_interleaves_with_delta_in_order() {
        let mut enc = StreamEncoder::new(new_slot(), new_slot());
        let raw = concat!(
            "data: {\"type\":\"delta\",\"text\":\"A\"}\n\n",
            "data: {\"type\":\"agent_delta\",\"agentId\":\"a\",\"text\":\"B\"}\n\n",
            "data: {\"type\":\"delta\",\"text\":\"C\"}\n\n",
        );
        let cs = chunks(&drive(&mut enc, raw.as_bytes()));
        let text: String = cs.iter().map(|c| c["choices"][0]["delta"]["content"].as_str().unwrap_or("")).collect();
        assert_eq!(text, "ABC");
    }

    // ---------- tool_calls index / id ----------

    #[test]
    fn parallel_agent_tools_get_incrementing_indexes() {
        let mut enc = StreamEncoder::new(new_slot(), new_slot());
        let mut raw = String::new();
        for i in 0..5 {
            raw += &format!("data: {{\"type\":\"agent_tool\",\"agentId\":\"a\",\"toolCallId\":\"t{i}\",\"toolName\":\"web_search\",\"label\":\"l{i}\"}}\n\n");
        }
        let cs = chunks(&drive(&mut enc, raw.as_bytes()));
        assert_eq!(cs.len(), 5);
        for (i, c) in cs.iter().enumerate() {
            let tc = &c["choices"][0]["delta"]["tool_calls"][0];
            assert_eq!(tc["index"], i as i64, "并行工具 index 必须递增");
            assert_eq!(tc["id"], format!("call_t{i}"));
            assert_eq!(tc["function"]["name"], "web_search");
            assert_eq!(tc["type"], "function");
        }
    }

    #[test]
    fn repeated_tool_call_id_reuses_index() {
        let mut enc = StreamEncoder::new(new_slot(), new_slot());
        let raw = concat!(
            "data: {\"type\":\"agent_tool\",\"toolCallId\":\"x\",\"toolName\":\"web_search\"}\n\n",
            "data: {\"type\":\"agent_tool_done\",\"toolCallId\":\"x\"}\n\n",
            "data: {\"type\":\"agent_tool\",\"toolCallId\":\"y\",\"toolName\":\"read_url\"}\n\n",
            "data: {\"type\":\"agent_tool\",\"toolCallId\":\"x\",\"toolName\":\"web_search\"}\n\n",
        );
        let cs = chunks(&drive(&mut enc, raw.as_bytes()));
        assert_eq!(cs.len(), 3);
        let idx: Vec<i64> = cs.iter().map(|c| c["choices"][0]["delta"]["tool_calls"][0]["index"].as_i64().unwrap()).collect();
        assert_eq!(idx, vec![0, 1, 0], "同一 toolCallId 必须复用 index");
    }

    #[test]
    fn missing_tool_call_id_yields_non_empty_unique_id() {
        let mut enc = StreamEncoder::new(new_slot(), new_slot());
        let raw = concat!(
            "data: {\"type\":\"agent_tool\",\"toolName\":\"web_search\"}\n\n",
            "data: {\"type\":\"agent_tool\",\"toolName\":\"read_url\"}\n\n",
        );
        let cs = chunks(&drive(&mut enc, raw.as_bytes()));
        let ids: Vec<String> = cs.iter().map(|c| c["choices"][0]["delta"]["tool_calls"][0]["id"].as_str().unwrap().to_string()).collect();
        assert_eq!(ids, vec!["call_anon_0".to_string(), "call_anon_1".to_string()]);
        let idx: Vec<i64> = cs.iter().map(|c| c["choices"][0]["delta"]["tool_calls"][0]["index"].as_i64().unwrap()).collect();
        assert_eq!(idx, vec![0, 1]);
    }

    // ---------- finish_reason + [DONE] ----------

    #[test]
    fn done_after_tool_call_finishes_with_tool_calls_reason() {
        let mut enc = StreamEncoder::new(new_slot(), new_slot());
        let raw = concat!(
            "data: {\"type\":\"agent_tool\",\"toolCallId\":\"t1\",\"toolName\":\"web_search\"}\n\n",
            "data: {\"type\":\"done\"}\n\n",
        );
        let out = drive(&mut enc, raw.as_bytes());
        assert!(out.ends_with("data: [DONE]\n\n"), "done 后必须发 [DONE] 哨兵");
        let cs = chunks(&out);
        let last = cs.last().unwrap();
        assert_eq!(last["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(last["choices"][0]["delta"], serde_json::json!({}), "终止 chunk 的 delta 必须为空");
    }

    #[test]
    fn done_without_tools_finishes_with_stop_reason() {
        let mut enc = StreamEncoder::new(new_slot(), new_slot());
        let raw = concat!(
            "data: {\"type\":\"delta\",\"text\":\"hi\"}\n\n",
            "data: {\"type\":\"done\"}\n\n",
        );
        let out = drive(&mut enc, raw.as_bytes());
        let cs = chunks(&out);
        let last = cs.last().unwrap();
        assert_eq!(last["choices"][0]["finish_reason"], "stop");
        assert_eq!(cs.len(), 2);
    }

    #[test]
    fn upstream_eof_without_done_still_emits_terminal_chunk() {
        // 上游 EOF 无 done：调用方用 finish_chunk() 补发（chat_stream_raw 的 None 分支）
        let enc = StreamEncoder::new(new_slot(), new_slot());
        let out = format!("{}data: [DONE]\n\n", enc.finish_chunk());
        assert!(out.ends_with("data: [DONE]\n\n"));
        assert_eq!(chunks(&out)[0]["choices"][0]["finish_reason"], "stop");
    }

    // ---------- threadId ----------

    #[test]
    fn meta_and_title_record_thread_id() {
        let slot = new_slot();
        let mut enc = StreamEncoder::new(slot.clone(), new_slot());
        enc.encode_block("data: {\"type\":\"meta\",\"threadId\":\"d8557501\",\"title\":\"用户原文\",\"model\":\"deepseek-v4-flash\",\"accessTier\":\"limited\"}\n\n");
        assert_eq!(slot.lock().unwrap().clone(), Some("d8557501".to_string()));
        // title 二次更新（同一 threadId）
        enc.encode_block("data: {\"type\":\"title\",\"threadId\":\"d8557501\",\"title\":\"请求搜索GitHub用户仓库\"}\n\n");
        assert_eq!(slot.lock().unwrap().clone(), Some("d8557501".to_string()));
        // 空 threadId 不得覆盖已有值
        enc.encode_block("data: {\"type\":\"title\",\"threadId\":\"\",\"title\":\"x\"}\n\n");
        assert_eq!(slot.lock().unwrap().clone(), Some("d8557501".to_string()));
    }

    #[test]
    fn web_client_exposes_last_thread_id() {
        let client = WebClient::new("__Secure-next-auth.session-token=x".into(), "glm-5.3-flash".into()).unwrap();
        assert_eq!(client.last_thread_id(), None);
        let mut enc = StreamEncoder::new(client.thread_slot(), new_slot());
        enc.encode_block("data: {\"type\":\"meta\",\"threadId\":\"th-1\",\"title\":\"t\"}\n\n");
        assert_eq!(client.last_thread_id().as_deref(), Some("th-1"));
    }

    // ---------- 聚合路径 ----------

    #[test]
    fn aggregate_title_late_update_wins() {
        let mut r = StreamResult::default();
        apply_event(&mut r, ChatEvent::Meta { thread_id: Some("t".into()), title: Some("用户原文".into()), model: None, access_tier: None });
        apply_event(&mut r, ChatEvent::Title { thread_id: Some("t".into()), title: Some("模型摘要".into()) });
        assert_eq!(r.title.as_deref(), Some("模型摘要"));
        // 新值为 None 时不得清空旧值
        apply_event(&mut r, ChatEvent::Title { thread_id: None, title: None });
        assert_eq!(r.title.as_deref(), Some("模型摘要"));
    }

    #[test]
    fn aggregate_agent_delta_appends_content() {
        let mut r = StreamResult::default();
        apply_event(&mut r, ChatEvent::Delta { text: "A".into() });
        apply_event(&mut r, ChatEvent::AgentDelta { agent_id: Some("a".into()), text: Some("B".into()) });
        apply_event(&mut r, ChatEvent::ReasoningDelta { text: "R".into() });
        assert_eq!(r.text, "AB");
        assert_eq!(r.reasoning, "R");
    }

    #[test]
    fn aggregate_tool_call_id_prefixed_and_done_marked() {
        let mut r = StreamResult::default();
        apply_event(&mut r, ChatEvent::AgentTool {
            agent_id: None,
            tool_call_id: Some("t1".into()),
            tool_name: Some("web_search".into()),
            label: Some("github".into()),
        });
        apply_event(&mut r, ChatEvent::AgentToolDone { tool_call_id: Some("t1".into()) });
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].id, "call_t1");
        assert!(r.tool_calls[0].done);
        assert_eq!(r.tools[0], "web_search: github");
    }

    // ---------- 完整抓包序列 ----------

    #[test]
    fn full_captured_sequence_preserves_agent_output() {
        let raw = concat!(
            "data: {\"type\":\"meta\",\"threadId\":\"d8557501\",\"title\":\"请你帮我调用工具一下联网搜索\",\"model\":\"deepseek-v4-flash\",\"accessTier\":\"limited\"}\n\n",
            "data: {\"type\":\"reasoning_delta\",\"text\":\"The\"}\n\n",
            "data: {\"type\":\"delta\",\"text\":\"我来\"}\n\n",
            "data: {\"type\":\"agent_start\",\"agentId\":\"h2\",\"parentAgentId\":\"main-agent\",\"name\":\"Web Researcher\",\"agentType\":\"researcher-web\",\"prompt\":\"Search\"}\n\n",
            "data: {\"type\":\"agent_tool\",\"agentId\":\"h2\",\"toolCallId\":\"c1\",\"toolName\":\"web_search\",\"label\":\"github lza6\"}\n\n",
            "data: {\"type\":\"agent_tool_done\",\"toolCallId\":\"c1\"}\n\n",
            "data: {\"type\":\"agent_delta\",\"agentId\":\"h2\",\"text\":\"Based on\"}\n\n",
            "data: {\"type\":\"agent_delta\",\"agentId\":\"h2\",\"text\":\" research\"}\n\n",
            "data: {\"type\":\"agent_finish\",\"agentId\":\"h2\"}\n\n",
            "data: {\"type\":\"title\",\"threadId\":\"d8557501\",\"title\":\"请求搜索GitHub用户仓库\"}\n\n",
            "data: {\"type\":\"delta\",\"text\":\"。\"}\n\n",
            "data: {\"type\":\"suggestions\",\"toolCallId\":\"h2v\",\"followups\":[{\"prompt\":\"p\",\"label\":\"l\"}]}\n\n",
            "data: {\"type\":\"done\"}\n\n",
        );
        let slot = new_slot();
        let mut enc = StreamEncoder::new(slot.clone(), new_slot());
        let out = drive(&mut enc, raw.as_bytes());

        assert!(out.ends_with("data: [DONE]\n\n"));
        assert_eq!(slot.lock().unwrap().clone(), Some("d8557501".to_string()));
        let cs = chunks(&out);
        // 正文 = delta + agent_delta（工具产出不得丢失）
        let text: String = cs.iter().map(|c| c["choices"][0]["delta"]["content"].as_str().unwrap_or("")).collect();
        assert_eq!(text, "我来Based on research。");
        // 推理单独走 reasoning_content
        let reasoning: String = cs.iter().map(|c| c["choices"][0]["delta"]["reasoning_content"].as_str().unwrap_or("")).collect();
        assert_eq!(reasoning, "The");
        // 工具调用存在且 index 正确
        let tc = cs.iter().find_map(|c| c["choices"][0]["delta"]["tool_calls"].as_array().cloned()).unwrap();
        assert_eq!(tc[0]["index"], 0);
        assert_eq!(tc[0]["id"], "call_c1");
        // 终止 chunk
        let last = cs.last().unwrap();
        assert_eq!(last["choices"][0]["finish_reason"], "tool_calls");
        assert!(contains(&out, "\"object\":\"chat.completion.chunk\""));
    }

    // ---------- SSE 分块 ----------

    #[test]
    fn find_double_newline_handles_lf_and_crlf() {
        assert_eq!(find_double_newline(b"a\n\nb"), Some(3));
        assert_eq!(find_double_newline(b"a\r\n\r\nb"), Some(5));
        assert_eq!(find_double_newline(b"a\n\r\nb"), None);
        assert_eq!(find_double_newline(b"no separator"), None);
    }

    #[test]
    fn crlf_stream_is_split_correctly() {
        let mut enc = StreamEncoder::new(new_slot(), new_slot());
        let raw = "data: {\"type\":\"delta\",\"text\":\"hi\"}\r\n\r\ndata: {\"type\":\"done\"}\r\n\r\n";
        let out = drive(&mut enc, raw.as_bytes());
        assert_eq!(chunks(&out).len(), 2);
        assert!(out.ends_with("data: [DONE]\n\n"));
    }

    // ---------- WebUploadResult 解析 ----------

    #[test]
    fn parses_image_upload_response() {
        let v: WebUploadResult = serde_json::from_str(
            r#"{"kind":"image","storageId":"kg278","url":"https://harmless-tapir-303.convex.cloud/api/storage/c45e","mediaType":"image/png","name":"logo.png","descriptionStorageId":"kg2ay"}"#,
        )
        .unwrap();
        assert_eq!(v.kind, "image");
        assert_eq!(v.storage_id, "kg278");
        assert_eq!(v.url.as_deref(), Some("https://harmless-tapir-303.convex.cloud/api/storage/c45e"));
        assert_eq!(v.description_storage_id.as_deref(), Some("kg2ay"));
        assert!(v.chars.is_none() && v.truncated.is_none());
    }

    #[test]
    fn parses_document_upload_response() {
        let v: WebUploadResult = serde_json::from_str(
            r#"{"kind":"document","storageId":"kg2d4d","mediaType":"text/plain","name":"a.txt","chars":2460,"truncated":false}"#,
        )
        .unwrap();
        assert_eq!(v.kind, "document");
        assert_eq!(v.chars, Some(2460));
        assert_eq!(v.truncated, Some(false));
        assert!(v.url.is_none() && v.description_storage_id.is_none());
    }

    /// 上游 200 内嵌错误旁路（Critic-J P2-1 闭环）：error envelope 在 encode_block 中被捕获
    #[test]
    fn upstream_error_bypass_captured() {
        let client = WebClient::new("__Secure-next-auth.session-token=x".into(), "glm-5.3-flash".into()).unwrap();
        let mut enc = StreamEncoder::new(client.thread_slot(), client.error_slot());
        // 反序列化会失败的纯 error envelope（无 type 字段）
        let (out, _) = enc.encode_block("data: {\"error\":\"Unauthorized\"}

");
        assert!(out.is_empty(), "错误事件不应产生输出");
        assert_eq!(client.last_upstream_error().as_deref(), Some("Unauthorized"), "旁路槽必须能读到（桥接层唯一可见点）");
        // 已知事件变体携带额外 error 字段
        let mut enc2 = StreamEncoder::new(new_slot(), new_slot());
        enc2.encode_block("data: {\"type\":\"meta\",\"error\":\"rate limited\"}

");
        assert_eq!(client_cases(enc2), Some("rate limited".to_string()));
    }

    fn client_cases(enc: StreamEncoder) -> Option<String> { enc.take_upstream_error() }

    /// 未映射事件（button/unknown）不得产生任何输出、不得报错
    #[test]
    fn unknown_events_are_ignored() {
        let mut enc = StreamEncoder::new(new_slot(), new_slot());
        let (out, done) = enc.encode_block("data: {\"type\":\"button\"}\n\ndata: {\"type\":\"brand_new_event\",\"x\":1}\n\n");
        assert!(out.is_empty());
        assert!(!done);
    }

    #[test]
    fn instance_id_is_stable_uuid_shaped_and_per_account() {
        let c1 = "__Secure-next-auth.session-token=aaa-bbb; other=1";
        let c2 = "__Secure-next-auth.session-token=ccc-ddd; other=1";
        let a = instance_id_for_cookie(c1);
        assert_eq!(a, instance_id_for_cookie(c1), "同账号必须稳定（否则每次重启都是新设备）");
        assert_ne!(a, instance_id_for_cookie(c2), "不同账号必须是不同实例 id");
        // UUID 形状：8-4-4-4-12，且第三段以 4 开头（版本位）
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(parts.len(), 5, "必须是 UUID 形状: {a}");
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "UUID 各段长度不对: {a}"
        );
        assert!(parts[2].starts_with('4'), "版本位应为 4: {a}");
        assert!(a.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-'), "只能含十六进制与连字符: {a}");
    }

    #[test]
    fn instance_id_falls_back_to_whole_cookie() {
        // 没有标准的 session-token 命名时，用整串 Cookie 当种子，仍须稳定且非空
        let a = instance_id_for_cookie("foo=1; bar=2");
        assert!(!a.is_empty());
        assert_eq!(a, instance_id_for_cookie("foo=1; bar=2"));
    }

    #[test]
    fn gravity_fingerprint_differs_between_accounts() {
        let a = GravityContext::for_cookie("__Secure-next-auth.session-token=acc-a");
        let b = GravityContext::for_cookie("__Secure-next-auth.session-token=acc-b");
        assert_ne!(a.user_data.visitor_id, b.user_data.visitor_id);
        assert_ne!(a.user_data.session_id, b.user_data.session_id);
        // 同一账号两次调用必须一致（否则每次请求都像换设备）
        let a2 = GravityContext::for_cookie("__Secure-next-auth.session-token=acc-a");
        assert_eq!(a.user_data.visitor_id, a2.user_data.visitor_id);
        assert_eq!(a.client_context, a2.client_context);
    }
}
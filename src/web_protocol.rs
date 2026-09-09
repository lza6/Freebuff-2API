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
use std::time::Duration;

pub const WEB_HOST: &str = "https://freebuff.com";

#[derive(Debug, Clone)]
pub struct WebClient {
    http: reqwest::Client,
    pub cookie: String,
    pub model: String,
}

/// chat/stream SSE 事件（web 版 11 种类型）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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

impl Default for GravityContext {
    fn default() -> Self {
        Self {
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
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .user_agent(crate::upstream::DESKTOP_UA)
            .connect_timeout(Duration::from_secs(15))
            .cookie_store(true)
            .build()?;
        Ok(Self { http, cookie, model })
    }

    fn headers(&self, json: bool) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(&self.cookie) {
            h.insert("cookie", v);
        }
        h.insert("origin", HeaderValue::from_static("https://freebuff.com"));
        h.insert("referer", HeaderValue::from_static("https://freebuff.com/chat"));
        h.insert("accept", HeaderValue::from_static("*/*"));
        if json {
            h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        }
        h
    }

    /// web 聊天流（SSE）。返回逐 event 回调。model 用短名。
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
            "gravity": GravityContext::default(),
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
        Ok(result)
    }

    /// 上传文件（multipart）→ storageId
    pub async fn upload(&self, file_bytes: Vec<u8>, filename: &str, mime: &str) -> Result<WebUploadResult> {
        let url = format!("{WEB_HOST}/api/chat/upload");
        let form = reqwest::multipart::Form::new()
            .part("file", reqwest::multipart::Part::bytes(file_bytes).file_name(filename.to_string()).mime_str(mime)?)
            .text("model", self.model.clone());
        let resp = self.http.post(&url).headers(self.headers(false)).multipart(form).send().await?;
        if !resp.status().is_success() {
            return Err(anyhow!("upload HTTP {}: {}", resp.status(), resp.text().await.unwrap_or_default()));
        }
        Ok(resp.json().await?)
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
    #[serde(default)]
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

fn apply_event(result: &mut StreamResult, event: ChatEvent) {
    match event {
        ChatEvent::Meta { thread_id, title, model, access_tier } => {
            if result.thread_id.is_none() { result.thread_id = thread_id; }
            if result.title.is_none() { result.title = title; }
            if result.model.is_none() { result.model = model; }
            if result.access_tier.is_none() { result.access_tier = access_tier; }
        }
        ChatEvent::Title { thread_id, title } => {
            if result.thread_id.is_none() { result.thread_id = thread_id; }
            if result.title.is_none() { result.title = title; }
        }
        ChatEvent::ReasoningDelta { text } => result.reasoning.push_str(&text),
        ChatEvent::Delta { text } => result.text.push_str(&text),
        ChatEvent::Suggestions { followups, .. } => result.suggestions = followups,
        ChatEvent::AgentTool { tool_name: Some(name), label, .. } => {
            let suffix = label.map(|l| format!(": {l}")).unwrap_or_default();
            result.tools.push(format!("{name}{suffix}"));
        }
        ChatEvent::AgentStart { agent_type, .. } => result.tools.push(format!("agent_start: {}", agent_type.unwrap_or_default())),
        ChatEvent::Done => result.done = true,
        _ => {}
    }
}

fn find_double_newline(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n").map(|i| i + 2)
}

async fn parse_json<T: for<'de> Deserialize<'de>>(resp: reqwest::Response) -> Result<T> {
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("HTTP {status}: {}", text.chars().take(300).collect::<String>()));
    }
    serde_json::from_str(&text).map_err(|e| anyhow!("解析响应失败: {e}"))
}
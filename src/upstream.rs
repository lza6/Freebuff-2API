//! 上游 Codebuff HTTP 客户端（会话 / run / chat / 广告）
//!
//! 逆向自 Freebuff-0.0.98 orchestrator.js：
//! - POST /api/v1/freebuff/session  （x-freebuff-model, x-freebuff-instance-id, x-freebuff-multi-session）
//! - GET  /api/v1/freebuff/session  （x-freebuff-instance-id 查状态，FREEBUFF_INCLUDE_UNUSED 拉 rateLimitsByModel）
//! - DELETE /api/v1/freebuff/session（x-freebuff-instance-id, x-freebuff-multi-session）
//! - POST /api/v1/agent-runs       （action=START/FINISH, ancestorRunIds）
//! - POST /api/v1/chat/completions （OpenAI 兼容体，codebuff_metadata 注入 run_id/cost_mode/client_id）
//! - POST /api/v1/ads|/api/ads     （广告拍卖 → impression 换取免费额度）
//! - POST /api/v1/ads/impression   （first_party 模式确认展示）
//! - GET  /api/v1/ads/policy       （广告策略）

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde::{Deserialize, Serialize};

use std::time::Duration;

pub const DESKTOP_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36";
pub const CODEBUFF_HEADER_UA: &str = "ai-sdk/openai-compatible/1.0.25/codebuff";
pub const FREEBUFF_MODEL_HEADER: &str = "x-freebuff-model";
pub const FREEBUFF_INSTANCE_HEADER: &str = "x-freebuff-instance-id";
pub const FREEBUFF_MULTI_SESSION: &str = "x-freebuff-multi-session";
pub const FREEBUFF_INCLUDE_UNUSED: &str = "x-freebuff-include-unused-rate-limits";
pub const FREEBUFF_HEARTBEAT: &str = "x-freebuff-heartbeat";
pub const FREEBUFF_TAKEOVER_HEADER: &str = "x-freebuff-takeover-instance-id";
pub const FREEBUFF_ACTING_USER: &str = "x-freebuff-acting-user-id";
pub const FREEBUFF_MODEL_HEADER_VAL: &str = "z-ai/glm-5.3-flash";

#[derive(Debug, Clone)]
pub struct UpstreamClient {
    base_url: String,
    http: reqwest::Client,
    #[allow(dead_code)]
    proxy: Option<String>,
    #[allow(dead_code)]
    timeout: Duration,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FreeSessionResponse {
    pub status: String,
    #[serde(default)]
    pub instance_id: Option<String>,
    #[serde(rename = "instanceId")]
    pub instance_id_alt: Option<String>,
    #[serde(default)]
    pub access_tier: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub remaining_ms: Option<i64>,
    #[serde(default)]
    pub position: Option<i64>,
    #[serde(default)]
    pub queue_depth: Option<i64>,
    #[serde(default)]
    pub estimated_wait_ms: Option<i64>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub rate_limits_by_model: Option<serde_json::Value>,
    #[serde(default)]
    pub subscription: Option<serde_json::Value>,
    #[serde(default)]
    pub freebucks: Option<serde_json::Value>,
    #[serde(default)]
    pub retry_after_ms: Option<i64>,
}

impl FreeSessionResponse {
    pub fn instance_id(&self) -> Option<String> {
        self.instance_id.as_ref().or(self.instance_id_alt.as_ref()).cloned()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StartRunResponse {
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(rename = "runId")]
    pub run_id_alt: Option<String>,
}

impl StartRunResponse {
    pub fn run_id(&self) -> Option<String> {
        self.run_id.as_ref().or(self.run_id_alt.as_ref()).cloned()
    }
}

impl UpstreamClient {
    pub fn new(base_url: String, proxy: Option<String>, timeout: Duration) -> Result<Self> {
        let mut builder = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(DESKTOP_UA)
            .connect_timeout(Duration::from_secs(15))
            .pool_idle_timeout(Duration::from_secs(90));
        if let Some(p) = &proxy {
            builder = builder.proxy(reqwest::Proxy::all(p)?);
        }
        let http = builder.build()?;
        // 规范化 host：codebuff.com → www.codebuff.com
        let base_url = if base_url == "https://codebuff.com" {
            "https://www.codebuff.com".to_string()
        } else {
            base_url.trim_end_matches('/').to_string()
        };
        Ok(Self { base_url, http, proxy, timeout })
    }

    fn auth_headers(&self, token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {token}")).unwrap());
        h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        h.insert(USER_AGENT, HeaderValue::from_static(CODEBUFF_HEADER_UA));
        h
    }

    /// GET 会话状态（含 rateLimitsByModel）
    pub async fn get_session(&self, token: &str, instance_id: Option<&str>) -> Result<FreeSessionResponse> {
        let mut headers = self.auth_headers(token);
        headers.insert(FREEBUFF_MULTI_SESSION, HeaderValue::from_static("1"));
        headers.insert(FREEBUFF_INCLUDE_UNUSED, HeaderValue::from_static("1"));
        if let Some(id) = instance_id {
            headers.insert(FREEBUFF_INSTANCE_HEADER, HeaderValue::from_str(id)?);
        }
        let url = format!("{}/api/v1/freebuff/session", self.base_url);
        let resp = self.http.get(&url).headers(headers).send().await?;
        parse_json_err(resp).await
    }

    /// POST 创建/刷新会话（x-freebuff-model 指定模型）
    pub async fn create_session(&self, token: &str, model: &str, instance_id: Option<&str>, takeover: Option<&str>) -> Result<FreeSessionResponse> {
        let mut headers = self.auth_headers(token);
        headers.insert(FREEBUFF_MODEL_HEADER, HeaderValue::from_str(model)?);
        headers.insert(FREEBUFF_MULTI_SESSION, HeaderValue::from_static("1"));
        if let Some(id) = instance_id {
            headers.insert(FREEBUFF_INSTANCE_HEADER, HeaderValue::from_str(id)?);
        }
        if let Some(t) = takeover {
            headers.insert(FREEBUFF_TAKEOVER_HEADER, HeaderValue::from_str(t)?);
        }
        let url = format!("{}/api/v1/freebuff/session", self.base_url);
        let resp = self.http.post(&url).headers(headers).body("{}").send().await?;
        parse_json_err(resp).await
    }

    /// 心跳保活（GET + x-freebuff-heartbeat: 1）
    pub async fn heartbeat(&self, token: &str, instance_id: &str) -> Result<()> {
        let mut headers = self.auth_headers(token);
        headers.insert(FREEBUFF_MULTI_SESSION, HeaderValue::from_static("1"));
        headers.insert(FREEBUFF_INSTANCE_HEADER, HeaderValue::from_str(instance_id)?);
        headers.insert(FREEBUFF_HEARTBEAT, HeaderValue::from_static("1"));
        let url = format!("{}/api/v1/freebuff/session", self.base_url);
        let _ = self.http.get(&url).headers(headers).send().await?;
        Ok(())
    }

    /// DELETE 释放会话
    pub async fn delete_session(&self, token: &str, instance_id: &str) -> Result<()> {
        let mut headers = self.auth_headers(token);
        headers.insert(FREEBUFF_INSTANCE_HEADER, HeaderValue::from_str(instance_id)?);
        headers.insert(FREEBUFF_MULTI_SESSION, HeaderValue::from_static("1"));
        let url = format!("{}/api/v1/freebuff/session", self.base_url);
        let resp = self.http.delete(&url).headers(headers).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(anyhow!("删除会话失败: HTTP {}", resp.status()))
        }
    }

    /// 启动 run
    pub async fn start_run(&self, token: &str, agent_id: &str, ancestors: &[String]) -> Result<String> {
        let body = serde_json::json!({
            "action": "START",
            "agentId": agent_id,
            "ancestorRunIds": ancestors,
        });
        let url = format!("{}/api/v1/agent-runs", self.base_url);
        let resp = self.http.post(&url).headers(self.auth_headers(token)).json(&body).send().await?;
        if !resp.status().is_success() {
            return Err(anyhow!("start run HTTP {}: {}", resp.status(), resp.text().await.unwrap_or_default()));
        }
        let parsed: StartRunResponse = resp.json().await?;
        parsed.run_id().ok_or_else(|| anyhow!("start run 响应缺 runId"))
    }

    /// 结束 run
    pub async fn finish_run(&self, token: &str, run_id: &str, total_steps: i64) -> Result<()> {
        let body = serde_json::json!({
            "action": "FINISH",
            "runId": run_id,
            "status": "completed",
            "totalSteps": total_steps,
            "directCredits": 0,
            "totalCredits": 0,
        });
        let url = format!("{}/api/v1/agent-runs", self.base_url);
        let resp = self.http.post(&url).headers(self.auth_headers(token)).json(&body).send().await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let text = resp.text().await.unwrap_or_default();
        if text.contains("not found") || text.contains("not running") {
            Ok(())
        } else {
            Err(anyhow!("finish run HTTP {status}: {text}"))
        }
    }

    /// 发送 chat 请求（返回原始响应，供调用方流式转发）— 带 codebuff_metadata 注入
    pub async fn chat_completions(
        &self,
        token: &str,
        mut body: serde_json::Value,
        run_id: &str,
        instance_id: Option<&str>,
    ) -> Result<reqwest::Response> {
        // 注入 codebuff_metadata
        let mut metadata = body.get("codebuff_metadata").cloned().unwrap_or_else(|| serde_json::json!({}));
        if let Some(obj) = metadata.as_object_mut() {
            obj.insert("run_id".into(), serde_json::json!(run_id));
            obj.insert("cost_mode".into(), serde_json::json!("free"));
            obj.insert("client_id".into(), serde_json::json!(generate_client_session_id()));
            if let Some(id) = instance_id {
                obj.insert("freebuff_instance_id".into(), serde_json::json!(id));
            }
        }
        body["codebuff_metadata"] = metadata;

        let url = format!("{}/api/v1/chat/completions", self.base_url);
        let resp = self.http.post(&url).headers(self.auth_headers(token)).json(&body).send().await?;
        Ok(resp)
    }

    /// 广告拍卖（网关侧从用户消息触发，换取免费额度）
    pub async fn request_ad(&self, token: &str, ad_session: &AdRequest) -> Result<AdResponse> {
        let url = format!("{}/api/v1/ads", self.base_url);
        let resp = self.http.post(&url).headers(self.auth_headers(token)).json(ad_session).send().await?;
        if !resp.status().is_success() {
            return Err(anyhow!("ad fail HTTP {}: {}", resp.status(), resp.text().await.unwrap_or_default()));
        }
        Ok(resp.json().await?)
    }

    /// 确认广告展示（first_party）
    pub async fn confirm_impression(&self, token: &str, imp_url: &str) -> Result<()> {
        let body = serde_json::json!({ "impUrl": imp_url, "mode": "desktop", "userAgent": DESKTOP_UA, "os": "windows" });
        let url = format!("{}/api/v1/ads/impression", self.base_url);
        let resp = self.http.post(&url).headers(self.auth_headers(token)).json(&body).send().await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(anyhow!("impression HTTP {}", resp.status()))
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdRequest {
    pub messages: Vec<AdMessage>,
    pub session_id: String,
    pub device: AdDevice,
    pub user_agent: String,
    #[serde(rename = "placementIds")]
    pub placement_ids: Vec<String>,
    #[serde(rename = "adSequenceId")]
    pub ad_sequence_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdDevice {
    pub os: String,
    pub timezone: String,
    pub locale: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AdResponse {
    #[serde(default)]
    pub ads: Vec<AdItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AdItem {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub click_url: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub imp_url: Option<String>,
    #[serde(rename = "impressionIds", default)]
    pub impression_ids: Vec<String>,
    #[serde(rename = "impUrl", default)]
    pub imp_url_alt: Option<String>,
}

impl AdItem {
    pub fn impression_url(&self) -> Option<&str> {
        self.imp_url.as_deref().or(self.imp_url_alt.as_deref())
    }
}

fn generate_client_session_id() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    (0..13)
        .map(|_| {
            let idx = rng.gen_range(0..36);
            ALPHABET[idx] as char
        })
        .collect()
}

async fn parse_json_err<T: for<'de> Deserialize<'de>>(resp: reqwest::Response) -> Result<T> {
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("HTTP {}: {}", status, &text));
    }
    serde_json::from_str(&text).map_err(|e| anyhow!("解析响应失败: {e} 原文: {}", text.chars().take(300).collect::<String>()))
}

pub fn parse_optional_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}
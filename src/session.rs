//! 会话管理器：排队 → 活跃 → 心跳保活 → 广告换额度再保活
//!
//! 逆向自 Freebuff-0.0.98 SessionManager：
//! - status=none → POST 创建
//! - status=queued → GET 轮询（estimatedWaitMs 决定延迟）
//! - status=active → 可用，过期前心跳 + 广告刷新
//! - 调度 x-freebuff-heartbeat:1 每 45s
//! - FREEBUFF_SESSION_GRACE_MS=1800000 宽限期

use crate::config::Config;
use crate::upstream::{FreeSessionResponse, UpstreamClient};
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

pub const SESSION_POLL_INTERVAL_SEC: u64 = 5;
pub const SESSION_HEARTBEAT_INTERVAL_SEC: u64 = 45;
pub const SESSION_GRACE_MS: i64 = 1_800_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionStatus {
    None,
    Queued,
    Active,
    Disabled,
    Ended,
    Superseded,
}

impl From<&str> for SessionStatus {
    fn from(s: &str) -> Self {
        match s.trim() {
            "none" => Self::None,
            "queued" => Self::Queued,
            "active" => Self::Active,
            "disabled" => Self::Disabled,
            "ended" => Self::Ended,
            "superseded" => Self::Superseded,
            _ => Self::None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub status: String,
    pub instance_id: Option<String>,
    pub model: Option<String>,
    pub expires_at: Option<String>,
    pub position: Option<i64>,
    pub queue_depth: Option<i64>,
    pub last_error: Option<String>,
    pub updated_at: String,
    pub heartbeat_count: u64,
    pub ad_renewals: u64,
}

pub struct SessionManager {
    pub client: Arc<UpstreamClient>,
    pub token: String,
    pub cfg: Config,

    mu: Mutex<SessionInner>,
    heartbeat_count: AtomicU32,
    ad_renewals: AtomicU32,
    running: AtomicBool,
}

struct SessionInner {
    status: SessionStatus,
    instance_id: Option<String>,
    model: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    position: Option<i64>,
    queue_depth: Option<i64>,
    estimated_wait_ms: Option<i64>,
    last_error: Option<String>,
    last_poll_at: Option<DateTime<Utc>>,
    poll_after: Option<DateTime<Utc>>,
}

impl SessionManager {
    pub fn new(client: Arc<UpstreamClient>, token: String, cfg: Config) -> Self {
        Self {
            client,
            token,
            cfg,
            mu: Mutex::new(SessionInner {
                status: SessionStatus::None,
                instance_id: None,
                model: None,
                expires_at: None,
                position: None,
                queue_depth: None,
                estimated_wait_ms: None,
                last_error: None,
                last_poll_at: None,
                poll_after: None,
            }),
            heartbeat_count: AtomicU32::new(0),
            ad_renewals: AtomicU32::new(0),
            running: AtomicBool::new(false),
        }
    }

    /// 确保会话活跃；无则创建/等待。返回 instance_id
    pub async fn ensure_session(&self, model: &str) -> Result<String> {
        loop {
            let mut inner = self.mu.lock().await;

            // 已是活跃且未过期
            if inner.status == SessionStatus::Active {
                if let Some(id) = &inner.instance_id {
                    let expires = inner.expires_at.unwrap_or(Utc::now() + Duration::from_secs(3600));
                    if Utc::now() + Duration::from_secs(5) < expires {
                        return Ok(id.clone());
                    }
                }
                // 过期则重置重创建
                inner.status = SessionStatus::None;
                inner.instance_id = None;
            }

            // 等待室中：等待 poll_after 后重试
            if inner.status == SessionStatus::Queued {
                if let Some(skip_until) = inner.poll_after {
                    if Utc::now() < skip_until {
                        return Err(anyhow!(
                            "waiting_room_queued: position {}/{}，{} 秒后重试",
                            inner.position.unwrap_or(0),
                            inner.queue_depth.unwrap_or(0),
                            (skip_until - Utc::now()).num_seconds().max(0)
                        ));
                    }
                }
                drop(inner);
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }

            // None/Active(过期) → 创建
            inner.status = SessionStatus::None;
            let existing = inner.instance_id.clone();
            drop(inner);

            match self
                .client
                .create_session(&self.token, model, existing.as_deref(), None)
                .await
            {
                Ok(sess) => {
                    self.absorb(sess, model).await?;
                    if self.is_active().await {
                        let id = self.instance_id().await;
                        if let Some(id) = id {
                            return Ok(id);
                        }
                    }
                    // queued 继续循环等待
                }
                Err(e) => {
                    self.note_error(&e).await;
                    // 服务器 5xx/网络 → 短暂退避后重试
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    return Err(e);
                }
            }
        }
    }

    /// 吸收会话响应，内部处理 queued/active 状态机
    async fn absorb(&self, sess: FreeSessionResponse, model: &str) -> Result<()> {
        let mut inner = self.mu.lock().await;
        let status = SessionStatus::from(sess.status.as_str());
        inner.status = status;
        inner.model = Some(sess.model.clone().unwrap_or_else(|| model.to_string()));
        inner.last_error = sess.message.clone().or(sess.error.clone());
        inner.estimated_wait_ms = sess.estimated_wait_ms;
        match status {
            SessionStatus::Active => {
                inner.instance_id = sess.instance_id();
                inner.expires_at = sess.expires_at.as_deref().and_then(crate::upstream::parse_optional_time);
                inner.position = None;
                inner.queue_depth = None;
            }
            SessionStatus::Queued => {
                inner.instance_id = sess.instance_id();
                inner.position = sess.position;
                inner.queue_depth = sess.queue_depth.or(sess.position);
                let wait_ms = sess.estimated_wait_ms.unwrap_or(5_000).clamp(1_000, SESSION_POLL_INTERVAL_SEC as i64 * 1000);
                inner.poll_after = Some(Utc::now() + Duration::from_millis(wait_ms as u64));
            }
            SessionStatus::Disabled => {
                // 账号无免费资格
                return Err(anyhow!("freebuff session disabled: 账号无可免费额度"));
            }
            SessionStatus::None | SessionStatus::Ended | SessionStatus::Superseded => {
                // 自动尝试重建会由上层 ensure_session 循环处理
            }
        }
        inner.last_poll_at = Some(Utc::now());
        Ok(())
    }

    #[allow(dead_code)]
    async fn note_error(&self, e: &anyhow::Error) {
        let mut inner = self.mu.lock().await;
        inner.last_error = Some(e.to_string());
    }

    /// 供 pool 熔断记录错误
    #[allow(dead_code)]
    pub async fn record_error(&self, err: &str) {
        let mut inner = self.mu.lock().await;
        inner.last_error = Some(err.to_string());
    }

    pub async fn is_active(&self) -> bool {
        self.mu.lock().await.status == SessionStatus::Active
    }

    pub async fn instance_id(&self) -> Option<String> {
        self.mu.lock().await.instance_id.clone()
    }

    pub async fn snapshot(&self) -> SessionSnapshot {
        let inner = self.mu.lock().await;
        SessionSnapshot {
            status: format!("{:?}", inner.status).to_lowercase(),
            instance_id: inner.instance_id.clone(),
            model: inner.model.clone(),
            expires_at: inner.expires_at.map(|d| d.to_rfc3339()),
            position: inner.position,
            queue_depth: inner.queue_depth,
            last_error: inner.last_error.clone(),
            updated_at: inner.last_poll_at.map(|d| d.to_rfc3339()).unwrap_or_default(),
            heartbeat_count: self.heartbeat_count.load(Ordering::Relaxed) as u64,
            ad_renewals: self.ad_renewals.load(Ordering::Relaxed) as u64,
        }
    }

    /// 后台保活循环：活跃时心跳 + 过期前广告刷新
    pub async fn run_keepalive(self: Arc<Self>, ads: Arc<crate::ads::AdRefresher>) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let hb = Duration::from_secs(SESSION_HEARTBEAT_INTERVAL_SEC);
        loop {
            tokio::time::sleep(hb).await;
            if self.instance_id().await.is_none() {
                continue;
            }
            if let Some(id) = self.instance_id().await {
                // 心跳
                if self.client.heartbeat(&self.token, &id).await.is_ok() {
                    self.heartbeat_count.fetch_add(1, Ordering::Relaxed);
                }
                // 若临近过期，尝试广告刷新
                let near_expiry = {
                    let inner = self.mu.lock().await;
                    match inner.expires_at {
                        Some(exp) => {
                            let remain = (exp - Utc::now()).num_seconds();
                            remain < 120 && remain > 0
                        }
                        None => false,
                    }
                };
                if near_expiry && !self.cfg.ad_providers.is_empty() {
                    let _ = ads.refresh(&self.token).await;
                    self.ad_renewals.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}
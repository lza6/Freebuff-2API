//! 广告换 token 保活：请求广告 → 确认展示（first_party）→ 换取免费额度
//!
//! 逆向自 Freebuff-0.0.98 ads.ts：
//! - POST /api/v1/ads  （placementIds=[Desktop-Below-Chat], messages, sessionId, device, userAgent）
//! - POST /api/v1/ads/impression （impUrl, mode=desktop, userAgent, os）first_party 确认

use crate::config::Config;
use crate::upstream::{AdDevice, AdMessage, AdRequest, UpstreamClient};
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;

pub const DESKTOP_BELOW_CHAT: &str = "Desktop-Below-Chat";
pub const AD_TIMEOUT_MS: u64 = 10_000;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AdStats {
    pub requests: u64,
    pub impressions_confirmed: u64,
    pub last_error: Option<String>,
}

pub struct AdRefresher {
    pub client: Arc<UpstreamClient>,
    pub cfg: Config,
    pub stats: Mutex<AdStats>,
}

impl AdRefresher {
    pub fn new(client: Arc<UpstreamClient>, cfg: Config) -> Self {
        Self {
            client,
            cfg,
            stats: Mutex::new(AdStats::default()),
        }
    }

    /// 为一个 token 触发一次广告刷新（换取免费额度延长会话）
    pub async fn refresh(&self, token: &str) -> Result<bool> {
        let ad_session = AdRequest {
            messages: vec![
                AdMessage {
                    role: "user".into(),
                    content: "build me a web app from this prompt: hi".into(),
                },
                AdMessage {
                    role: "user".into(),
                    content: "continue".into(),
                },
            ],
            session_id: format!("gateway-ad-{}", uuid::Uuid::new_v4()),
            device: AdDevice {
                os: "windows".into(),
                timezone: "Asia/Shanghai".into(),
                locale: "zh-CN".into(),
            },
            user_agent: crate::upstream::DESKTOP_UA.to_string(),
            placement_ids: vec![DESKTOP_BELOW_CHAT.to_string()],
            ad_sequence_id: format!("agent:{}", uuid::Uuid::new_v4().to_string().replace('-', "")),
            surface: Some("cli_chat".into()),
        };

        let mut stats = self.stats.lock().await;
        stats.requests += 1;

        let resp = match tokio::time::timeout(
            std::time::Duration::from_millis(AD_TIMEOUT_MS),
            self.client.request_ad(token, &ad_session),
        )
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                stats.last_error = Some(format!("ad auction: {e}"));
                return Ok(false);
            }
            Err(_) => {
                stats.last_error = Some("ad auction timeout".into());
                return Ok(false);
            }
        };

        let ad = resp.ads.into_iter().next();
        if let Some(ad) = ad {
            // first_party 确认展示
            if let Some(imp) = ad.impression_url() {
                if self.client.confirm_impression(token, imp).await.is_ok() {
                    stats.impressions_confirmed += 1;
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    pub async fn snapshot(&self) -> AdStats {
        self.stats.lock().await.clone()
    }
}
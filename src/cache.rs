//! 会话缓存：同一账号+模型复用会话 instance_id/thread_id（加速冷启动）
//! 上游会话宽限期 30min（FREEBUFF_SESSION_GRACE_MS）。


use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct SessionCacheEntry {
    pub account: String,
    pub model: String,
    pub instance_id: String,
    pub thread_id: Option<String>,
    pub created_at: Instant,
    pub last_used: Instant,
}

impl Default for SessionCacheEntry {
    fn default() -> Self {
        Self {
            account: String::new(),
            model: String::new(),
            instance_id: String::new(),
            thread_id: None,
            created_at: Instant::now(),
            last_used: Instant::now(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SessionCache {
    entries: Arc<RwLock<HashMap<String, SessionCacheEntry>>>,
}

impl SessionCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(account: &str, model: &str) -> String {
        format!("{account}:{model}")
    }

    /// 取缓存：未过期则返回 instance_id + thread_id
    pub async fn get(&self, account: &str, model: &str) -> Option<(String, Option<String>)> {
        let key = Self::key(account, model);
        let mut entries = self.entries.write().await;
        if let Some(e) = entries.get_mut(&key) {
            // 30 分钟过期（上游会话宽限期 30min）
            if e.created_at.elapsed() < Duration::from_secs(30 * 60) {
                e.last_used = Instant::now();
                return Some((e.instance_id.clone(), e.thread_id.clone()));
            }
            entries.remove(&key);
        }
        None
    }

    /// 写入缓存
    pub async fn put(&self, account: &str, model: &str, instance_id: String, thread_id: Option<String>) {
        let key = Self::key(account, model);
        let entry = SessionCacheEntry {
            account: account.to_string(),
            model: model.to_string(),
            instance_id,
            thread_id,
            created_at: Instant::now(),
            last_used: Instant::now(),
        };
        self.entries.write().await.insert(key, entry);
    }

    /// 失效（会话过期/被挤占时）
    pub async fn invalidate(&self, account: &str, model: &str) {
        self.entries.write().await.remove(&Self::key(account, model));
    }

    /// 清理过期
    pub async fn sweep(&self) {
        let mut entries = self.entries.write().await;
        entries.retain(|_, e| e.created_at.elapsed() < Duration::from_secs(30 * 60));
    }

    pub async fn len(&self) -> usize {
        self.entries.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cache_roundtrip() {
        let c = SessionCache::new();
        c.put("acc1", "z-ai/glm-5.3-flash", "inst-1".into(), Some("th-1".into())).await;
        let (inst, th) = c.get("acc1", "z-ai/glm-5.3-flash").await.unwrap();
        assert_eq!(inst, "inst-1");
        assert_eq!(th.as_deref(), Some("th-1"));
        assert!(c.get("acc1", "other-model").await.is_none());
    }

    #[tokio::test]
    async fn cache_invalidate() {
        let c = SessionCache::new();
        c.put("acc1", "m", "i1".into(), None).await;
        c.invalidate("acc1", "m").await;
        assert!(c.get("acc1", "m").await.is_none());
    }
}
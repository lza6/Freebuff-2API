//! 多账号池：会话健康评分 → 最优 token 轮询 + 冷却 + 熔断
//!
//! 评分维度（每 token/session 轮询一次）：
//! - 会话 active 且剩余时间充足  +100
//! - 队列位置越靠前  +50~+80
//! - 冷却中  -999（直接摘除）
//! - 最近错误  -加权
//! - 心跳/广告刷新成功  +小分

use crate::config::Config;
use crate::session::SessionManager;
use crate::upstream::UpstreamClient;

use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PoolSnapshot {
    pub accounts: Vec<AccountSnapshot>,
    pub total: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AccountSnapshot {
    pub name: String,
    pub healthy: bool,
    pub score: f64,
    pub cooldown_until: Option<String>,
    pub last_error: Option<String>,
    pub session: Option<crate::session::SessionSnapshot>,
}

pub struct AccountEntry {
    pub name: String,
    pub token: String,
    pub session: Arc<SessionManager>,
    pub score: RwLock<f64>,
    pub cooldown_until: RwLock<Option<Instant>>,
}

impl Clone for AccountEntry {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            token: self.token.clone(),
            session: self.session.clone(),
            score: RwLock::new(0.0),
            cooldown_until: RwLock::new(None),
        }
    }
}

/// 负载均衡状态
pub struct Pool {
    pub accounts: Mutex<Vec<AccountEntry>>,
    pub next: std::sync::atomic::AtomicUsize,
}

impl Pool {
    /// 从配置构建多账号池
    pub fn new(cfg: &Config, client: Arc<UpstreamClient>) -> Self {
        let accounts = cfg
            .auth_tokens
            .iter()
            .enumerate()
            .map(|(i, token)| AccountEntry {
                name: format!("token-{}", i + 1),
                token: token.clone(),
                session: Arc::new(SessionManager::new(client.clone(), token.clone(), cfg.clone())),
                score: RwLock::new(0.0),
                cooldown_until: RwLock::new(None),
            })
            .collect();
        Self {
            accounts: Mutex::new(accounts),
            next: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// 挑选健康度最高的 token（评分加权），返回克隆
    pub async fn pick_best(&self) -> Option<AccountEntry> {
        let accounts = self.accounts.lock().await;
        if accounts.is_empty() {
            return None;
        }
        let mut best: Option<&AccountEntry> = None;
        for acc in accounts.iter() {
            let cd = *acc.cooldown_until.read().await;
            if let Some(until) = cd {
                if Instant::now() < until {
                    continue;
                }
            }
            if best.is_none() {
                best = Some(acc);
            } else {
                let s = *acc.score.read().await;
                let bs = *best.unwrap().score.read().await;
                if s > bs {
                    best = Some(acc);
                }
            }
        }
        best.cloned()
    }

    /// 标记冷却（熔断）
    pub async fn mark_cooldown(&self, name: &str, duration: std::time::Duration, reason: &str) {
        let accounts = self.accounts.lock().await;
        if let Some(acc) = accounts.iter().find(|a| a.name == name) {
            let mut cd = acc.cooldown_until.write().await;
            *cd = Some(Instant::now() + duration);
            drop(cd);
            acc.session.record_error(reason).await;
            tracing::warn!("账号 {name} 冷却 {duration:?}: {reason}");
        }
    }

    /// 更新账号评分
    pub async fn update_score(&self, name: &str, delta: f64) {
        let accounts = self.accounts.lock().await;
        if let Some(acc) = accounts.iter().find(|a| a.name == name) {
            let mut score = acc.score.write().await;
            *score += delta;
            *score = score.clamp(-1000.0, 1000.0);
        }
    }

    /// 健康快照
    pub async fn snapshot(&self) -> PoolSnapshot {
        let accounts = self.accounts.lock().await;
        let mut snapshot_accounts = Vec::with_capacity(accounts.len());
        for acc in accounts.iter() {
            let score = *acc.score.read().await;
            let cd = *acc.cooldown_until.read().await;
            let sess = acc.session.snapshot().await;
            snapshot_accounts.push(AccountSnapshot {
                name: acc.name.clone(),
                healthy: cd.is_none() || Instant::now() > cd.unwrap(),
                score,
                cooldown_until: cd.map(|i| format!("{:?}", i)),
                last_error: sess.last_error.clone(),
                session: Some(sess),
            });
        }
        PoolSnapshot {
            accounts: snapshot_accounts,
            total: accounts.len(),
        }
    }

    /// 热追加账号（token 导入后调用），返回是否真正新增
    pub async fn add_account(&self, entry: AccountEntry) -> bool {
        let mut accounts = self.accounts.lock().await;
        if accounts.iter().any(|a| a.token == entry.token) {
            return false;
        }
        accounts.push(entry);
        true
    }
}

/// 把 token 列表换为 Pool（无则返回空池）
pub fn build_pool(cfg: &Config, client: Arc<UpstreamClient>) -> Arc<Pool> {
    Arc::new(Pool::new(cfg, client))
}

#[allow(dead_code)]
fn round_robin_next(pool: &Pool) -> usize {
    pool.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

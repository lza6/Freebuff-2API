//! 多账号池：会话健康评分 → 最优 token 轮询 + 熔断三态（Closed/Open/HalfOpen）
//!
//! 评分维度（每 token/session 轮询一次）：
//! - 会话 active 且剩余时间充足  +100
//! - 队列位置越靠前  +50~+80
//! - 熔断 Open  -999（直接摘除）；HalfOpen 放行探测
//! - 最近错误  -加权
//! - 心跳/广告刷新成功  +小分
//!
//! 熔断规则（参考 cc-switch）：
//! - Closed：连续失败 ≥4 次 → Open（冷却 = min(60s × 2^(trips-1), 600s)）
//! - Open：冷却期内拒绝选择；到期 → HalfOpen
//! - HalfOpen：放行探测；连续成功 ≥2 次 → Closed；失败 → 再次 Open

use crate::config::Config;
use crate::session::SessionManager;
use crate::upstream::UpstreamClient;

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

/// 连续失败阈值（Closed → Open）
const FAILURE_THRESHOLD: u32 = 4;
/// HalfOpen 下连续成功次数（→ Closed）
const HALF_OPEN_SUCCESS_TO_CLOSE: u32 = 2;
/// 基础冷却
const BASE_COOLDOWN: Duration = Duration::from_secs(60);
/// 冷却上限
const MAX_COOLDOWN: Duration = Duration::from_secs(600);

/// 熔断器三态
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

/// 单账号熔断器
#[derive(Debug)]
pub struct CircuitBreaker {
    pub state: CircuitState,
    pub consecutive_failures: u32,
    pub half_open_successes: u32,
    pub open_until: Option<Instant>,
    /// HalfOpen 探测闸门：同一时刻只放行一个在途探测
    pub probing: bool,
    /// 累计熔断次数（可观测）
    pub trips: u64,
    pub last_reason: Option<String>,
}

impl CircuitBreaker {
    pub fn new() -> Self {
        Self {
            state: CircuitState::Closed,
            consecutive_failures: 0,
            half_open_successes: 0,
            open_until: None,
            probing: false,
            trips: 0,
            last_reason: None,
        }
    }

    /// 是否允许本次请求（Open 未到期 → false；到期自动转 HalfOpen 并放行单个探测）
    pub fn allow(&mut self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => {
                if self.probing {
                    false // 已有探测在途，不再放行
                } else {
                    self.probing = true;
                    true
                }
            }
            CircuitState::Open => {
                if let Some(until) = self.open_until {
                    if Instant::now() >= until {
                        self.state = CircuitState::HalfOpen;
                        self.half_open_successes = 0;
                        self.probing = true;
                        return true;
                    }
                }
                false
            }
        }
    }

    /// 业务成功（HalfOpen 下累计探测成功）
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.probing = false;
        if self.state == CircuitState::HalfOpen {
            self.half_open_successes += 1;
            if self.half_open_successes >= HALF_OPEN_SUCCESS_TO_CLOSE {
                self.state = CircuitState::Closed;
                self.open_until = None;
                self.half_open_successes = 0;
            }
        }
    }

    /// 业务失败（Closed 连续失败超阈值 / HalfOpen 任意失败 → 断开）
    pub fn record_failure(&mut self, reason: &str) {
        self.consecutive_failures += 1;
        self.last_reason = Some(reason.to_string());
        self.probing = false;
        match self.state {
            CircuitState::HalfOpen => self.trip(reason),
            CircuitState::Closed if self.consecutive_failures >= FAILURE_THRESHOLD => self.trip(reason),
            _ => {}
        }
    }

    /// 直接断开（401/403 等确定性失败；冷却随累计次数指数增长，封顶 10 分钟）
    pub fn trip(&mut self, reason: &str) {
        self.trips += 1;
        self.state = CircuitState::Open;
        self.consecutive_failures = FAILURE_THRESHOLD;
        self.half_open_successes = 0;
        self.probing = false;
        // trips=1..→ 1,2,4,8,16 倍（16 倍被 MAX_COOLDOWN 截断为 10 分钟）
        let factor = 1u32 << self.trips.min(5).saturating_sub(1);
        let cd = BASE_COOLDOWN.saturating_mul(factor).min(MAX_COOLDOWN);
        self.open_until = Some(Instant::now() + cd);
        self.last_reason = Some(reason.to_string());
    }

    /// 带自定义冷却时长的断开（如上游 Retry-After）
    pub fn trip_for(&mut self, duration: Duration, reason: &str) {
        self.trips += 1;
        self.state = CircuitState::Open;
        self.consecutive_failures = FAILURE_THRESHOLD;
        self.half_open_successes = 0;
        self.probing = false;
        self.open_until = Some(Instant::now() + duration.min(MAX_COOLDOWN));
        self.last_reason = Some(reason.to_string());
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

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
    /// 熔断三态（closed/open/half_open）
    pub circuit_state: String,
    /// 累计熔断次数
    pub trips: u64,
    pub last_error: Option<String>,
    pub session: Option<crate::session::SessionSnapshot>,
}

pub struct AccountEntry {
    pub name: String,
    pub token: String,
    pub session: Arc<SessionManager>,
    pub score: RwLock<f64>,
    /// 熔断器（替代裸 cooldown_until）
    pub breaker: RwLock<CircuitBreaker>,
}

impl Clone for AccountEntry {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            token: self.token.clone(),
            session: self.session.clone(),
            score: RwLock::new(0.0),
            breaker: RwLock::new(CircuitBreaker::new()),
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
                breaker: RwLock::new(CircuitBreaker::new()),
            })
            .collect();
        Self {
            accounts: Mutex::new(accounts),
            next: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// 挑选健康度最高的 token（熔断 Open 跳过、HalfOpen 放行探测），返回克隆
    pub async fn pick_best(&self) -> Option<AccountEntry> {
        let accounts = self.accounts.lock().await;
        if accounts.is_empty() {
            return None;
        }
        let mut best: Option<&AccountEntry> = None;
        for acc in accounts.iter() {
            let allowed = acc.breaker.write().await.allow();
            if !allowed {
                continue;
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

    /// 记录业务成功（驱动 HalfOpen → Closed）
    pub async fn mark_success(&self, name: &str) {
        let accounts = self.accounts.lock().await;
        if let Some(acc) = accounts.iter().find(|a| a.name == name) {
            acc.breaker.write().await.record_success();
        }
    }

    /// 记录业务失败（驱动 Closed → Open / HalfOpen → Open）
    pub async fn mark_failure(&self, name: &str, reason: &str) {
        let accounts = self.accounts.lock().await;
        if let Some(acc) = accounts.iter().find(|a| a.name == name) {
            acc.breaker.write().await.record_failure(reason);
        }
    }

    /// 标记冷却（熔断）：自定义时长（如上游 Retry-After / 401 冷却）
    pub async fn mark_cooldown(&self, name: &str, duration: std::time::Duration, reason: &str) {
        let accounts = self.accounts.lock().await;
        if let Some(acc) = accounts.iter().find(|a| a.name == name) {
            acc.breaker.write().await.trip_for(duration, reason);
            acc.session.record_error(reason).await;
            tracing::warn!("账号 {name} 熔断 {duration:?}: {reason}");
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
            let breaker = acc.breaker.read().await;
            let sess = acc.session.snapshot().await;
            let healthy = breaker.state == CircuitState::Closed
                || (breaker.state == CircuitState::Open
                    && breaker.open_until.map(|u| Instant::now() >= u).unwrap_or(true));
            snapshot_accounts.push(AccountSnapshot {
                name: acc.name.clone(),
                healthy,
                score,
                cooldown_until: breaker.open_until.map(|i| format!("{:?}", i)),
                circuit_state: match breaker.state {
                    CircuitState::Closed => "closed".into(),
                    CircuitState::Open => "open".into(),
                    CircuitState::HalfOpen => "half_open".into(),
                },
                trips: breaker.trips,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaker_opens_after_consecutive_failures() {
        let mut b = CircuitBreaker::new();
        assert!(b.allow());
        for _ in 0..FAILURE_THRESHOLD - 1 {
            b.record_failure("x");
        }
        assert_eq!(b.state, CircuitState::Closed, "未达阈值不应断开");
        b.record_failure("x");
        assert_eq!(b.state, CircuitState::Open);
        assert!(!b.allow(), "Open 冷却期内应拒绝");
    }

    #[test]
    fn breaker_half_open_closes_after_successes() {
        let mut b = CircuitBreaker::new();
        b.trip("test");
        // 手动把到期时间提前，模拟冷却结束
        b.open_until = Some(Instant::now() - Duration::from_secs(1));
        assert!(b.allow(), "到期后应放行探测");
        assert_eq!(b.state, CircuitState::HalfOpen);
        b.record_success();
        assert_eq!(b.state, CircuitState::HalfOpen, "一次成功不够");
        b.record_success();
        assert_eq!(b.state, CircuitState::Closed, "连续成功应闭合");
    }

    #[test]
    fn breaker_half_open_failure_reopens() {
        let mut b = CircuitBreaker::new();
        b.trip("test");
        b.open_until = Some(Instant::now() - Duration::from_secs(1));
        assert!(b.allow());
        b.record_failure("again");
        assert_eq!(b.state, CircuitState::Open);
        assert!(b.trips >= 2, "再次断开应累计 trips");
    }

    #[test]
    fn breaker_cooldown_grows_and_caps() {
        let mut b = CircuitBreaker::new();
        b.trip("1");
        let first = b.open_until.unwrap() - Instant::now();
        b.trip("2");
        let second = b.open_until.unwrap() - Instant::now();
        assert!(second > first, "冷却应递增");
        for _ in 0..8 {
            b.trip("n");
        }
        let capped = b.open_until.unwrap() - Instant::now();
        assert!(capped <= MAX_COOLDOWN, "冷却应封顶");
    }

    #[test]
    fn cooldown_reaches_cap_after_five_trips() {
        let mut b = CircuitBreaker::new();
        for _ in 0..5 {
            b.trip("x");
        }
        let cd = b.open_until.unwrap() - Instant::now();
        // 第 5 次：1<<4 = 16 倍 → 960s 被 MAX_COOLDOWN(600s) 截断
        assert!(cd > Duration::from_secs(550), "第 5 次应接近 10 分钟封顶: {cd:?}");
        assert!(cd <= MAX_COOLDOWN);
    }

    #[test]
    fn half_open_only_one_probe_at_a_time() {
        let mut b = CircuitBreaker::new();
        b.trip("x");
        b.open_until = Some(Instant::now() - Duration::from_secs(1));
        assert!(b.allow(), "冷却到期应放行首个探测");
        assert!(!b.allow(), "已有在途探测时不再放行（防并发探测风暴）");
        b.record_success();
        assert!(b.allow(), "探测完成后可放行下一个");
        assert!(!b.allow());
    }
}

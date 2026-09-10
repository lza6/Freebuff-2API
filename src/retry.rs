//! 失败分类与重试退避策略
//!
//! - HTTP 状态码 / reqwest 错误 → [`FailureKind`]
//! - [`backoff_delay`]：指数退避 `2^attempt * base` 截断到 `max`；
//!   RateLimit 使用 4 倍基数并叠加 ±20% 抖动；Auth 固定 10s 供刷新/换号
//! - [`AttemptOutcome`] / [`AttemptFailure`] 标记「已提交」边界：
//!   已提交（响应已开始下发）后的错误不再换号重试，直接透传

use std::future::Future;
use std::time::Duration;

/// Auth 失败后的固定等待（供刷新 token 或换号）
pub const AUTH_REFRESH_DELAY_MS: u64 = 10_000;

/// RateLimit 退避基数倍率（限流恢复通常更慢）
const RATE_LIMIT_MULTIPLIER: u64 = 4;

/// 抖动比例（±20%）
const JITTER_RATIO: f64 = 0.2;

/// 上游失败分类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// 429 限流
    RateLimit,
    /// 401 凭证失效（需刷新/换号，不宜原地重试）
    Auth,
    /// 403 无权限（需换号，不宜原地重试）
    Forbidden,
    /// 5xx 服务端错误
    Server,
    /// 连接失败/网络不可达
    Network,
    /// 请求超时
    Timeout,
    /// 其他（协议错误、解析失败等）
    Other,
}

impl FailureKind {
    /// 稳定字符串标识（用于日志与遥测 error_kind）
    pub fn as_str(self) -> &'static str {
        match self {
            FailureKind::RateLimit => "rate_limit",
            FailureKind::Auth => "auth",
            FailureKind::Forbidden => "forbidden",
            FailureKind::Server => "server",
            FailureKind::Network => "network",
            FailureKind::Timeout => "timeout",
            FailureKind::Other => "other",
        }
    }
}

impl std::fmt::Display for FailureKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 重试策略
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// 总尝试次数（含首次）
    pub max_attempts: usize,
    /// 退避基数（毫秒）
    pub base_delay_ms: u64,
    /// 单次退避上限（毫秒）
    pub max_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay_ms: 500,
            max_delay_ms: 8_000,
        }
    }
}

/// HTTP 状态码 → 失败分类
pub fn classify_status(status: u16) -> FailureKind {
    match status {
        429 => FailureKind::RateLimit,
        401 => FailureKind::Auth,
        403 => FailureKind::Forbidden,
        s if (500..600).contains(&s) => FailureKind::Server,
        _ => FailureKind::Other,
    }
}

/// reqwest 传输层错误 → 失败分类
pub fn classify_reqwest_error(e: &reqwest::Error) -> FailureKind {
    if e.is_timeout() {
        FailureKind::Timeout
    } else if e.is_connect() {
        FailureKind::Network
    } else {
        FailureKind::Other
    }
}

/// 是否适合原地重试（换号由上层决定：Auth/Forbidden 不可原地重试）
pub fn is_retryable(kind: FailureKind) -> bool {
    matches!(
        kind,
        FailureKind::RateLimit | FailureKind::Server | FailureKind::Network | FailureKind::Timeout
    )
}

/// 计算第 `attempt` 次失败后的退避时长（attempt 从 0 开始）
///
/// - 常规：`min(2^attempt * base, max)`
/// - RateLimit：4 倍基数 + ±20% 抖动，再截断到 `max`
/// - Auth：固定 [`AUTH_REFRESH_DELAY_MS`]
pub fn backoff_delay(policy: &RetryPolicy, attempt: usize, kind: FailureKind) -> Duration {
    if kind == FailureKind::Auth {
        return Duration::from_millis(AUTH_REFRESH_DELAY_MS);
    }
    let exp = 2u64.saturating_pow(attempt.min(63) as u32);
    let mut ms = policy.base_delay_ms.saturating_mul(exp);
    if kind == FailureKind::RateLimit {
        ms = apply_jitter(ms.saturating_mul(RATE_LIMIT_MULTIPLIER));
    }
    Duration::from_millis(ms.min(policy.max_delay_ms))
}

/// 叠加 ±20% 抖动
fn apply_jitter(ms: u64) -> u64 {
    use rand::Rng;
    let factor = rand::thread_rng().gen_range((1.0 - JITTER_RATIO)..=(1.0 + JITTER_RATIO));
    (ms as f64 * factor).round().max(0.0) as u64
}

/// 是否继续重试：已提交 / 不可重试 / 次数耗尽 → `None`
///
/// `attempt` 为当前已完成的尝试序号（从 0 开始）。
pub fn retry_delay(
    policy: &RetryPolicy,
    attempt: usize,
    kind: FailureKind,
    committed: bool,
) -> Option<Duration> {
    if committed || !is_retryable(kind) {
        return None;
    }
    if attempt + 1 >= policy.max_attempts {
        return None;
    }
    Some(backoff_delay(policy, attempt, kind))
}

/// 一次尝试的结果：`value` + 是否已提交（已向上游/客户端产生不可撤销副作用）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptOutcome<T> {
    pub value: T,
    pub committed: bool,
}

impl<T> AttemptOutcome<T> {
    pub fn new(value: T, committed: bool) -> Self {
        Self { value, committed }
    }

    /// 已提交（不可重试）
    pub fn committed(value: T) -> Self {
        Self::new(value, true)
    }

    /// 未提交（可安全换号重试）
    pub fn draft(value: T) -> Self {
        Self::new(value, false)
    }

    pub fn is_committed(&self) -> bool {
        self.committed
    }

    /// 保持 committed 标记映射内部值
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> AttemptOutcome<U> {
        AttemptOutcome {
            value: f(self.value),
            committed: self.committed,
        }
    }
}

/// 一次失败的尝试：错误本体 + 分类 + 是否已提交
#[derive(Debug, Clone)]
pub struct AttemptFailure<E> {
    pub error: E,
    pub kind: FailureKind,
    pub committed: bool,
}

impl<E> AttemptFailure<E> {
    pub fn new(error: E, kind: FailureKind, committed: bool) -> Self {
        Self {
            error,
            kind,
            committed,
        }
    }

    /// 未提交失败（可重试）
    pub fn uncommitted(error: E, kind: FailureKind) -> Self {
        Self::new(error, kind, false)
    }

    /// 已提交失败（直接透传）
    pub fn committed(error: E, kind: FailureKind) -> Self {
        Self::new(error, kind, true)
    }
}

/// 带退避重试的执行驱动：`attempt_fn` 接收尝试序号（从 0 开始）
///
/// 未提交且可重试的失败会按策略退避后重试；已提交/不可重试/次数耗尽
/// 的错误原样返回。
pub async fn run_with_retry<T, E, F, Fut>(policy: &RetryPolicy, mut attempt_fn: F) -> Result<T, E>
where
    F: FnMut(usize) -> Fut,
    Fut: Future<Output = Result<AttemptOutcome<T>, AttemptFailure<E>>>,
{
    let mut attempt = 0usize;
    loop {
        match attempt_fn(attempt).await {
            Ok(outcome) => return Ok(outcome.value),
            Err(failure) => match retry_delay(policy, attempt, failure.kind, failure.committed) {
                Some(delay) => {
                    tracing::warn!(
                        attempt,
                        kind = failure.kind.as_str(),
                        delay_ms = delay.as_millis() as u64,
                        "上游失败，退避后重试"
                    );
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                None => return Err(failure.error),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn classify_status_covers_all_branches() {
        assert_eq!(classify_status(429), FailureKind::RateLimit);
        assert_eq!(classify_status(401), FailureKind::Auth);
        assert_eq!(classify_status(403), FailureKind::Forbidden);
        assert_eq!(classify_status(500), FailureKind::Server);
        assert_eq!(classify_status(502), FailureKind::Server);
        assert_eq!(classify_status(599), FailureKind::Server);
        assert_eq!(classify_status(404), FailureKind::Other);
        assert_eq!(classify_status(200), FailureKind::Other);
    }

    #[test]
    fn is_retryable_matrix() {
        assert!(is_retryable(FailureKind::RateLimit));
        assert!(is_retryable(FailureKind::Server));
        assert!(is_retryable(FailureKind::Network));
        assert!(is_retryable(FailureKind::Timeout));
        assert!(!is_retryable(FailureKind::Auth));
        assert!(!is_retryable(FailureKind::Forbidden));
        assert!(!is_retryable(FailureKind::Other));
    }

    #[test]
    fn backoff_is_monotonic_and_capped() {
        let policy = RetryPolicy::default();
        let mut prev = Duration::ZERO;
        for attempt in 0..8 {
            let d = backoff_delay(&policy, attempt, FailureKind::Server);
            assert!(d >= prev, "第 {attempt} 次退避应不小于前一次");
            assert!(d <= Duration::from_millis(policy.max_delay_ms));
            prev = d;
        }
        assert_eq!(
            backoff_delay(&policy, 0, FailureKind::Server),
            Duration::from_millis(500)
        );
        assert_eq!(
            backoff_delay(&policy, 1, FailureKind::Server),
            Duration::from_millis(1000)
        );
        assert_eq!(
            backoff_delay(&policy, 2, FailureKind::Server),
            Duration::from_millis(2000)
        );
        // 上限截断
        assert_eq!(
            backoff_delay(&policy, 20, FailureKind::Server),
            Duration::from_millis(8000)
        );
    }

    #[test]
    fn rate_limit_backoff_has_jitter_within_bounds_and_caps() {
        let policy = RetryPolicy::default();
        // attempt=0 → 500 * 4 = 2000ms，抖动区间 [1600, 2400]
        for _ in 0..200 {
            let d = backoff_delay(&policy, 0, FailureKind::RateLimit).as_millis() as u64;
            assert!((1600..=2400).contains(&d), "抖动越界: {d}ms");
        }
        // 高次数时抖动后仍被 max 截断
        for _ in 0..50 {
            assert_eq!(
                backoff_delay(&policy, 12, FailureKind::RateLimit),
                Duration::from_millis(8000)
            );
        }
        // 抖动确实在变化（非恒定值）
        let samples: std::collections::HashSet<u128> = (0..50)
            .map(|_| backoff_delay(&policy, 0, FailureKind::RateLimit).as_millis())
            .collect();
        assert!(samples.len() > 1, "RateLimit 退避应包含随机抖动");
    }

    #[test]
    fn auth_backoff_is_fixed_refresh_delay() {
        let policy = RetryPolicy::default();
        for attempt in [0usize, 1, 5, 20] {
            assert_eq!(
                backoff_delay(&policy, attempt, FailureKind::Auth),
                Duration::from_millis(AUTH_REFRESH_DELAY_MS)
            );
        }
    }

    #[test]
    fn retry_delay_stops_when_committed_or_exhausted_or_not_retryable() {
        let policy = RetryPolicy::default();
        // 未提交且可重试：允许
        assert!(retry_delay(&policy, 0, FailureKind::Server, false).is_some());
        // 已提交：直接放弃
        assert!(retry_delay(&policy, 0, FailureKind::Server, true).is_none());
        // 不可重试
        assert!(retry_delay(&policy, 0, FailureKind::Auth, false).is_none());
        assert!(retry_delay(&policy, 0, FailureKind::Forbidden, false).is_none());
        // 次数耗尽（max_attempts=3 → attempt 0/1 可重试，attempt 2 不可）
        assert!(retry_delay(&policy, 1, FailureKind::Network, false).is_some());
        assert!(retry_delay(&policy, 2, FailureKind::Network, false).is_none());
    }

    #[tokio::test]
    async fn run_with_retry_retries_uncommitted_until_success() {
        let policy = RetryPolicy {
            max_attempts: 5,
            base_delay_ms: 1,
            max_delay_ms: 2,
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let result: Result<&str, &'static str> = run_with_retry(&policy, move |attempt| {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                if attempt < 2 {
                    Err(AttemptFailure::uncommitted("boom", FailureKind::Network))
                } else {
                    Ok(AttemptOutcome::draft("ok"))
                }
            }
        })
        .await;
        assert_eq!(result.ok(), Some("ok"));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn run_with_retry_stops_immediately_on_committed_or_exhausted() {
        // 已提交错误：只尝试一次，错误透传
        let policy = RetryPolicy {
            max_attempts: 5,
            base_delay_ms: 1,
            max_delay_ms: 2,
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let result: Result<(), &'static str> = run_with_retry(&policy, move |_| {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err(AttemptFailure::committed(
                    "stream broken",
                    FailureKind::Network,
                ))
            }
        })
        .await;
        assert_eq!(result.err(), Some("stream broken"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // 次数耗尽：恰好尝试 max_attempts 次
        let calls2 = Arc::new(AtomicUsize::new(0));
        let c2 = calls2.clone();
        let result2: Result<(), &'static str> = run_with_retry(&policy, move |_| {
            let c = c2.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err(AttemptFailure::uncommitted(
                    "always down",
                    FailureKind::Server,
                ))
            }
        })
        .await;
        assert_eq!(result2.err(), Some("always down"));
        assert_eq!(calls2.load(Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn classify_reqwest_timeout_error_as_timeout() {
        use tokio::io::AsyncReadExt;
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(l) => l,
            Err(_) => return, // 环境不支持本地监听则跳过
        };
        let addr = match listener.local_addr() {
            Ok(a) => a,
            Err(_) => return,
        };
        // 接受连接但永不响应，制造读取超时
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_millis(80))
            .build()
        {
            Ok(c) => c,
            Err(_) => return,
        };
        let err = match client.get(format!("http://{addr}/")).send().await {
            Ok(_) => return, // 环境异常导致请求成功，跳过
            Err(e) => e,
        };
        assert!(err.is_timeout(), "期望超时错误, 实际: {err}");
        assert_eq!(classify_reqwest_error(&err), FailureKind::Timeout);
    }

    #[tokio::test]
    async fn classify_reqwest_connect_error_as_network() {
        // 绑定后立刻释放，得到一个几乎必然拒绝连接的本地端口
        let addr = match std::net::TcpListener::bind("127.0.0.1:0") {
            Ok(l) => match l.local_addr() {
                Ok(a) => a,
                Err(_) => return,
            },
            Err(_) => return,
        };
        let err = match reqwest::Client::new()
            .get(format!("http://{addr}/"))
            .send()
            .await
        {
            Ok(_) => return,
            Err(e) => e,
        };
        assert_eq!(classify_reqwest_error(&err), FailureKind::Network);
    }

    #[tokio::test]
    async fn classify_reqwest_builder_error_as_other() {
        let err = match reqwest::Client::new().get("http://[").send().await {
            Ok(_) => return,
            Err(e) => e,
        };
        assert_eq!(classify_reqwest_error(&err), FailureKind::Other);
    }

    #[test]
    fn attempt_outcome_map_preserves_committed_flag() {
        let draft = AttemptOutcome::draft(2).map(|v| v * 3);
        assert_eq!(draft.value, 6);
        assert!(!draft.is_committed());
        let committed = AttemptOutcome::committed(1).map(|v| v + 1);
        assert_eq!(committed.value, 2);
        assert!(committed.is_committed());
    }
}

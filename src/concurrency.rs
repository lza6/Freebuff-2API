//! 并发限制信号量 —— 逆向自 Freebuff-0.0.98 桌面端 orchestrator.js
//!
//! 上游用双桶并发模型（FREEBUFF_DESKTOP_CONCURRENCY_LIMITS）：
//!   free:       { "slot-bound": 1, "multi-tab": 3 }   免费账号
//!   subscriber: { "slot-bound": 3, "multi-tab": 8 }   订阅账号
//!   limited 免费:{ "slot-bound": 1, "multi-tab": 0 }
//!
//! slot-bound = 付费模型槽（如 GLM 5.3 Flash、DeepSeek V4 Flash），
//! multi-tab   = 非付费模型。
//! 并发粒度 = 每账号每桶，本地先判 usedByOthers>=limit 抛 409，
//! 服务端 premium_slot_taken / desktopSessionCounts 二次校验兜底。
//! 保活 = 45s 心跳（x-freebuff-heartbeat:1），30min 宽限后回收。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 双桶并发限制（来自上游常量）
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ConcurrencyLimits {
    pub slot_bound: usize,
    pub multi_tab: usize,
}

impl ConcurrencyLimits {
    pub fn free() -> Self {
        Self { slot_bound: 1, multi_tab: 3 }
    }
    pub fn subscriber() -> Self {
        Self { slot_bound: 3, multi_tab: 8 }
    }
    pub fn limited_network() -> Self {
        Self { slot_bound: 1, multi_tab: 0 }
    }
}

/// 模型所属桶
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Bucket {
    SlotBound,
    MultiTab,
}

/// slot-bound 模型清单（上游 FREEBUFF_DESKTOP_SLOT_BOUND_MODEL_IDS 逆向）
pub const SLOT_BOUND_MODELS: &[&str] = &[
    "z-ai/glm-5.3-flash",
    "z-ai/glm-5.2",
    "deepseek/deepseek-v4-flash",
    "deepseek/deepseek-v4-pro",
    "mimo/mimo-v2.5",
    "minimax/minimax-m3",
];

/// 每账号的并发槽位
#[derive(Debug, Clone)]
pub struct AccountConcurrency {
    token: String,
    limits: ConcurrencyLimits,
    /// 当前占用（instance_id -> bucket）
    holders: Arc<Mutex<HashMap<String, Bucket>>>,
}

impl AccountConcurrency {
    pub fn new(token: String, limits: ConcurrencyLimits) -> Self {
        Self {
            token,
            limits,
            holders: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn limits(&self) -> ConcurrencyLimits {
        self.limits
    }

    pub fn bucket_for(model: &str) -> Bucket {
        if SLOT_BOUND_MODELS.contains(&model) {
            Bucket::SlotBound
        } else {
            Bucket::MultiTab
        }
    }

    /// 尝试获取一个槽位（本地先判），成功返回 instance_id
    pub async fn try_acquire(&self, model: &str) -> Result<String, ConcurrencyError> {
        let bucket = Self::bucket_for(model);
        let mut holders = self.holders.lock().await;
        let limit = match bucket {
            Bucket::SlotBound => self.limits.slot_bound,
            Bucket::MultiTab => self.limits.multi_tab,
        };
        let used = holders.values().filter(|b| **b == bucket).count();
        if used >= limit {
            return Err(ConcurrencyError::LimitReached {
                token: self.token.clone(),
                bucket,
                limit,
                used,
            });
        }
        let instance_id = format!("gw-{}-{}", model.replace('/', "_"), uuid::Uuid::new_v4().to_string().split('-').next().unwrap());
        holders.insert(instance_id.clone(), bucket);
        Ok(instance_id)
    }

    /// 释放槽位
    pub async fn release(&self, instance_id: &str) {
        self.holders.lock().await.remove(instance_id);
    }

    /// 当前占用快照
    pub async fn snapshot(&self) -> ConcurrencySnapshot {
        let holders = self.holders.lock().await;
        let slot_used = holders.values().filter(|b| **b == Bucket::SlotBound).count();
        let multi_used = holders.values().filter(|b| **b == Bucket::MultiTab).count();
        ConcurrencySnapshot {
            limits: self.limits,
            slot_used,
            multi_used,
            slot_remaining: self.limits.slot_bound.saturating_sub(slot_used),
            multi_remaining: self.limits.multi_tab.saturating_sub(multi_used),
            holders: holders.keys().cloned().collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConcurrencySnapshot {
    pub limits: ConcurrencyLimits,
    pub slot_used: usize,
    pub multi_used: usize,
    pub slot_remaining: usize,
    pub multi_remaining: usize,
    pub holders: Vec<String>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ConcurrencyError {
    #[error("账号 {token} 的 {bucket:?} 并发已达上限 ({used}/{limit})，请等待已有任务完成")]
    LimitReached {
        token: String,
        bucket: Bucket,
        limit: usize,
        used: usize,
    },
}

/// 并发管理器：账号 -> 双桶信号量
#[derive(Debug, Clone)]
pub struct ConcurrencyManager {
    accounts: Arc<Mutex<HashMap<String, AccountConcurrency>>>,
}

impl ConcurrencyManager {
    pub fn new() -> Self {
        Self {
            accounts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 注册/更新账号并发限制
    pub async fn register(&self, token: &str, limits: ConcurrencyLimits) {
        let mut accounts = self.accounts.lock().await;
        accounts.insert(token.to_string(), AccountConcurrency::new(token.to_string(), limits));
    }

    pub async fn get(&self, token: &str) -> Option<AccountConcurrency> {
        self.accounts.lock().await.get(token).cloned()
    }

    /// 全局快照
    pub async fn snapshot(&self) -> Vec<(String, ConcurrencySnapshot)> {
        let accounts = self.accounts.lock().await;
        let mut out = Vec::new();
        for (token, acc) in accounts.iter() {
            out.push((token.clone(), acc.snapshot().await));
        }
        out
    }
}

impl Default for ConcurrencyManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn slot_bound_limits() {
        let acc = AccountConcurrency::new("tok".into(), ConcurrencyLimits::free());
        // free: slot-bound=1, multi-tab=3
        let a = acc.try_acquire("z-ai/glm-5.3-flash").await.unwrap();
        assert!(acc.try_acquire("z-ai/glm-5.3-flash").await.is_err()); // 第二个 slot-bound 应失败
        acc.release(&a).await;
        assert!(acc.try_acquire("z-ai/glm-5.3-flash").await.is_ok());
    }

    #[tokio::test]
    async fn multi_tab_limits() {
        let acc = AccountConcurrency::new("tok".into(), ConcurrencyLimits::free());
        for _ in 0..3 {
            acc.try_acquire("google/gemini-3.8-flash").await.unwrap();
        }
        assert!(acc.try_acquire("google/gemini-3.8-flash").await.is_err()); // 第 4 个 multi-tab 失败
    }

    #[tokio::test]
    async fn manager_register() {
        let mgr = ConcurrencyManager::new();
        mgr.register("tok1", ConcurrencyLimits::free()).await;
        let acc = mgr.get("tok1").await.unwrap();
        assert_eq!(acc.limits().multi_tab, 3);
    }
}
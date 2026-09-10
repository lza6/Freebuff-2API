//! 运行日志总线：广播订阅 + 环形缓冲
//!
//! - [`LogBus::emit`] 同时写入广播通道（SSE 实时推送）与环形缓冲（历史回看）
//! - [`LogBus::recent`] 支持 `after_id` 补发（SSE Last-Event-ID 断线重连）
//! - 环形缓冲满时丢弃最旧事件；广播无订阅者时不阻塞

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

/// 单条运行日志事件
#[derive(Debug, Clone, serde::Serialize)]
pub struct LogEvent {
    /// 全局递增 id（从 1 开始，可作为 SSE event id）
    pub id: u64,
    /// RFC3339 时间戳
    pub ts: String,
    /// 级别：debug/info/warn/error
    pub level: String,
    /// 事件类别：request/retry/account/system/...
    pub kind: String,
    /// 关联请求 id
    pub req_id: Option<String>,
    pub message: String,
}

/// 日志总线：广播 sender + 环形缓冲
pub struct LogBus {
    sender: broadcast::Sender<LogEvent>,
    ring: Arc<Mutex<VecDeque<LogEvent>>>,
    capacity: usize,
    next_id: AtomicU64,
}

impl LogBus {
    /// `capacity` 同时作为环形缓冲与广播缓冲容量（至少 1）
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.clamp(1, 1 << 20);
        let (sender, _) = broadcast::channel(cap);
        Self {
            sender,
            ring: Arc::new(Mutex::new(VecDeque::with_capacity(cap.min(4096)))),
            capacity: cap,
            next_id: AtomicU64::new(0),
        }
    }

    /// 广播并写入环形缓冲（无订阅者时静默丢弃广播）
    pub fn emit(&self, level: &str, kind: &str, req_id: Option<&str>, message: impl Into<String>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let event = LogEvent {
            id,
            ts: chrono::Utc::now().to_rfc3339(),
            level: level.to_string(),
            kind: kind.to_string(),
            req_id: req_id.map(str::to_string),
            message: message.into(),
        };
        self.push_ring(event.clone());
        // 无订阅者时 send 返回 Err，忽略即可
        let _ = self.sender.send(event);
    }

    /// 订阅实时事件流
    pub fn subscribe(&self) -> broadcast::Receiver<LogEvent> {
        self.sender.subscribe()
    }

    /// 读取历史事件（按时间正序返回）
    ///
    /// - `after_id = None`：返回最新 `limit` 条
    /// - `after_id = Some(x)`：从 `x` 之后最早的未读事件开始补发最多 `limit` 条
    pub fn recent(&self, limit: usize, after_id: Option<u64>) -> Vec<LogEvent> {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(after) = after_id {
            return ring
                .iter()
                .filter(|e| e.id > after)
                .take(limit)
                .cloned()
                .collect();
        }
        let start = ring.len().saturating_sub(limit);
        ring.iter().skip(start).cloned().collect()
    }

    /// 已产生的事件总数（含已淘汰的历史）
    pub fn count(&self) -> u64 {
        self.next_id.load(Ordering::Relaxed)
    }

    /// 写入环形缓冲，超容量丢弃最旧
    fn push_ring(&self, event: LogEvent) {
        let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        if ring.len() >= self.capacity {
            ring.pop_front();
        }
        ring.push_back(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_then_recent_returns_chronological_newest() {
        let bus = LogBus::new(16);
        for i in 0..5 {
            bus.emit("info", "request", Some("r1"), format!("消息 {i}"));
        }
        let recent = bus.recent(3, None);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].message, "消息 2");
        assert_eq!(recent[2].message, "消息 4");
        assert_eq!(recent[2].req_id.as_deref(), Some("r1"));
        assert_eq!(recent[2].kind, "request");
        assert_eq!(recent[2].level, "info");
        assert!(chrono::DateTime::parse_from_rfc3339(&recent[2].ts).is_ok());
    }

    #[test]
    fn capacity_evicts_oldest_events() {
        let bus = LogBus::new(3);
        for i in 0..6 {
            bus.emit("info", "tick", None, format!("e{i}"));
        }
        let all = bus.recent(100, None);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].message, "e3");
        assert_eq!(all[2].message, "e5");
        assert_eq!(bus.count(), 6);
    }

    #[test]
    fn recent_after_id_filters_for_replay() {
        let bus = LogBus::new(32);
        for i in 0..6 {
            bus.emit("info", "tick", None, format!("e{i}"));
        }
        let replay = bus.recent(100, Some(3));
        assert_eq!(replay.len(), 3);
        assert_eq!(replay[0].id, 4);
        assert_eq!(replay[2].id, 6);
        // limit 截断：从 3 之后最早未读开始
        let limited = bus.recent(2, Some(3));
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0].id, 4);
        assert_eq!(limited[1].id, 5);
        // after_id 超出最新 id：无补发
        assert!(bus.recent(10, Some(999)).is_empty());
    }

    #[test]
    fn ids_increase_monotonically() {
        let bus = LogBus::new(8);
        assert_eq!(bus.count(), 0);
        bus.emit("warn", "retry", None, "a");
        bus.emit("error", "account", Some("req-9"), "b");
        assert_eq!(bus.count(), 2);
        let all = bus.recent(10, None);
        assert_eq!(all[0].id, 1);
        assert_eq!(all[1].id, 2);
        assert_eq!(all[1].req_id.as_deref(), Some("req-9"));
    }

    #[tokio::test]
    async fn subscribe_receives_broadcast_events() {
        let bus = LogBus::new(8);
        let mut rx = bus.subscribe();
        bus.emit("info", "system", None, "上线");
        let ev = match rx.recv().await {
            Ok(e) => e,
            Err(e) => panic!("应收到广播事件: {e}"),
        };
        assert_eq!(ev.message, "上线");
        assert_eq!(ev.id, 1);
    }
}

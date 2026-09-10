//! 遥测写入：独立 SQLite 连接 + 后台写线程（WAL 模式）
//!
//! - `requests_v2`：单请求明细（模型、账号、状态、延迟、token、错误等）
//! - `events`：请求生命周期事件
//! - 使用独立连接与线程，不与 `usage.rs` 争锁；`record` 为 `try_send`
//!   非阻塞，队列满计入 [`TelemetryWriter::dropped`]
//! - [`TelemetryWriter::flush`] 发送屏障消息并等待写完（测试/关闭用）
//! - `Drop` 发送关闭信号并 join 线程，确保 Windows 下 SQLite 句柄释放

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;

/// 建表语句（幂等）
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS requests_v2 (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  req_id TEXT,
  ts TEXT NOT NULL,
  endpoint TEXT,
  requested_model TEXT,
  resolved_model TEXT,
  account TEXT,
  status INTEGER,
  latency_ms INTEGER,
  ttft_ms INTEGER,
  prompt_tokens INTEGER,
  completion_tokens INTEGER,
  stream INTEGER,
  error_kind TEXT,
  error_excerpt TEXT,
  route_reason TEXT,
  api_key TEXT,
  client_ip TEXT
);
CREATE TABLE IF NOT EXISTS events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  req_id TEXT,
  ts TEXT NOT NULL,
  kind TEXT NOT NULL,
  detail TEXT
);
CREATE INDEX IF NOT EXISTS idx_requests_v2_req ON requests_v2(req_id);
CREATE INDEX IF NOT EXISTS idx_events_req ON events(req_id);
"#;

/// 单请求遥测明细
#[derive(Debug, Clone, Default)]
pub struct TraceRow {
    pub req_id: String,
    pub endpoint: String,
    pub requested_model: String,
    pub resolved_model: String,
    pub account: String,
    pub status: u16,
    pub latency_ms: u64,
    pub ttft_ms: Option<u64>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub stream: bool,
    pub error_kind: Option<String>,
    pub error_excerpt: Option<String>,
    pub route_reason: Option<String>,
    pub api_key: Option<String>,
    pub client_ip: Option<String>,
}

/// 后台写线程消息
enum Msg {
    Row(Box<TraceRow>),
    Event {
        req_id: String,
        kind: String,
        detail: String,
    },
    /// 屏障：收到即回复 ack（此前消息均已落库）
    Flush(std::sync::mpsc::Sender<()>),
    Shutdown,
}

/// 遥测写入器（克隆共享 sender 即可在多处使用）
pub struct TelemetryWriter {
    tx: SyncSender<Msg>,
    dropped: Arc<AtomicU64>,
    handle: Option<JoinHandle<()>>,
}

impl TelemetryWriter {
    /// 打开/建库建表并启动后台写线程
    ///
    /// 建表在调用线程完成，因此返回成功即保证表已存在。
    pub fn spawn(db_path: PathBuf, capacity: usize) -> Result<Self> {
        let conn = open_db(&db_path)?;
        let (tx, rx) = sync_channel::<Msg>(capacity.clamp(1, 1 << 20));
        let dropped = Arc::new(AtomicU64::new(0));
        let handle = std::thread::Builder::new()
            .name("telemetry-writer".into())
            .spawn(move || writer_loop(conn, rx))
            .context("启动遥测写线程失败")?;
        Ok(Self {
            tx,
            dropped,
            handle: Some(handle),
        })
    }

    /// 记录一条请求明细（非阻塞，队列满丢弃并计数）
    pub fn record(&self, row: TraceRow) {
        self.enqueue(Msg::Row(Box::new(row)));
    }

    /// 记录一条请求事件
    pub fn event(&self, req_id: &str, kind: &str, detail: &str) {
        self.enqueue(Msg::Event {
            req_id: req_id.to_string(),
            kind: kind.to_string(),
            detail: detail.to_string(),
        });
    }

    /// 因队列满/线程退出而丢弃的消息数
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// 等待队列排空（发送屏障并等待写线程确认）
    pub fn flush(&self) {
        let (ack_tx, ack_rx) = std::sync::mpsc::channel();
        if self.tx.send(Msg::Flush(ack_tx)).is_ok() {
            let _ = ack_rx.recv();
        }
    }

    /// 非阻塞入队，失败计入 dropped
    fn enqueue(&self, msg: Msg) {
        match self.tx.try_send(msg) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

impl Drop for TelemetryWriter {
    fn drop(&mut self) {
        // 队列满时阻塞等待，确保关闭信号送达；线程已退出则忽略错误
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 后台线程主循环：顺序消费消息，出错仅告警不中断
fn writer_loop(conn: Connection, rx: Receiver<Msg>) {
    for msg in rx {
        match msg {
            Msg::Row(row) => {
                if let Err(e) = insert_row(&conn, &row) {
                    tracing::warn!(error = %e, "遥测明细写入失败");
                }
            }
            Msg::Event {
                req_id,
                kind,
                detail,
            } => {
                if let Err(e) = insert_event(&conn, &req_id, &kind, &detail) {
                    tracing::warn!(error = %e, "遥测事件写入失败");
                }
            }
            Msg::Flush(ack) => {
                let _ = ack.send(());
            }
            Msg::Shutdown => break,
        }
    }
}

/// 打开数据库：建目录、启用 WAL、建表
fn open_db(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("创建遥测目录失败: {}", dir.display()))?;
        }
    }
    let conn =
        Connection::open(path).with_context(|| format!("打开遥测库失败: {}", path.display()))?;
    // journal_mode 会返回一行结果，必须用 query_row 读取
    let _mode: String = conn
        .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
        .context("启用 WAL 失败")?;
    conn.execute_batch(SCHEMA).context("初始化遥测表失败")?;
    Ok(conn)
}

fn insert_row(conn: &Connection, row: &TraceRow) -> Result<()> {
    conn.execute(
        "INSERT INTO requests_v2 (
            req_id, ts, endpoint, requested_model, resolved_model, account, status,
            latency_ms, ttft_ms, prompt_tokens, completion_tokens, stream,
            error_kind, error_excerpt, route_reason, api_key, client_ip
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
        params![
            row.req_id,
            Utc::now().to_rfc3339(),
            row.endpoint,
            row.requested_model,
            row.resolved_model,
            row.account,
            row.status as i64,
            row.latency_ms as i64,
            row.ttft_ms.map(|v| v as i64),
            row.prompt_tokens as i64,
            row.completion_tokens as i64,
            i64::from(row.stream),
            row.error_kind,
            row.error_excerpt,
            row.route_reason,
            row.api_key,
            row.client_ip
        ],
    )
    .context("写入 requests_v2 失败")?;
    Ok(())
}

fn insert_event(conn: &Connection, req_id: &str, kind: &str, detail: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO events (req_id, ts, kind, detail) VALUES (?1,?2,?3,?4)",
        params![req_id, Utc::now().to_rfc3339(), kind, detail],
    )
    .context("写入 events 失败")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel as test_channel;

    fn sample_row(req_id: &str) -> TraceRow {
        TraceRow {
            req_id: req_id.to_string(),
            endpoint: "/v1/chat/completions".to_string(),
            requested_model: "gpt-4o".to_string(),
            resolved_model: "claude-sonnet-5".to_string(),
            account: "token-1".to_string(),
            status: 200,
            latency_ms: 1234,
            ttft_ms: Some(321),
            prompt_tokens: 100,
            completion_tokens: 200,
            stream: true,
            error_kind: Some("timeout".to_string()),
            error_excerpt: Some("upstream timed out".to_string()),
            route_reason: Some("fallback".to_string()),
            api_key: Some("sk-test".to_string()),
            client_ip: Some("127.0.0.1".to_string()),
        }
    }

    #[test]
    fn record_then_flush_persists_all_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("telemetry.db");
        let writer = TelemetryWriter::spawn(path.clone(), 64).unwrap();
        writer.record(sample_row("req-1"));
        writer.record(TraceRow {
            req_id: "req-2".to_string(),
            status: 500,
            ttft_ms: None,
            stream: false,
            ..Default::default()
        });
        writer.flush();

        {
            let conn = Connection::open(&path).unwrap();
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM requests_v2", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 2);
            struct Persisted {
                status: i64,
                latency: i64,
                ttft: Option<i64>,
                stream: i64,
                err_kind: Option<String>,
                ip: Option<String>,
                resolved: Option<String>,
            }
            let p: Persisted = conn
                .query_row(
                    "SELECT status,latency_ms,ttft_ms,stream,error_kind,client_ip,resolved_model
                     FROM requests_v2 WHERE req_id='req-1'",
                    [],
                    |r| {
                        Ok(Persisted {
                            status: r.get(0)?,
                            latency: r.get(1)?,
                            ttft: r.get(2)?,
                            stream: r.get(3)?,
                            err_kind: r.get(4)?,
                            ip: r.get(5)?,
                            resolved: r.get(6)?,
                        })
                    },
                )
                .unwrap();
            assert_eq!(p.status, 200);
            assert_eq!(p.latency, 1234);
            assert_eq!(p.ttft, Some(321));
            assert_eq!(p.stream, 1);
            assert_eq!(p.err_kind.as_deref(), Some("timeout"));
            assert_eq!(p.ip.as_deref(), Some("127.0.0.1"));
            assert_eq!(p.resolved.as_deref(), Some("claude-sonnet-5"));
        }
        drop(writer);
    }

    #[test]
    fn event_then_flush_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("telemetry.db");
        let writer = TelemetryWriter::spawn(path.clone(), 16).unwrap();
        writer.event("req-7", "retry", "第 2 次尝试");
        writer.flush();
        {
            let conn = Connection::open(&path).unwrap();
            let (req_id, kind, detail): (String, String, String) = conn
                .query_row("SELECT req_id,kind,detail FROM events", [], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .unwrap();
            assert_eq!(req_id, "req-7");
            assert_eq!(kind, "retry");
            assert_eq!(detail, "第 2 次尝试");
        }
        drop(writer);
    }

    #[test]
    fn enqueue_counts_dropped_when_full() {
        // 内部入队逻辑：容量 1 的通道先占满，再入队必然计入 dropped
        let (tx, rx) = test_channel::<Msg>(1);
        let dropped = Arc::new(AtomicU64::new(0));
        tx.try_send(Msg::Shutdown).unwrap();
        let writer = TelemetryWriter {
            tx,
            dropped: dropped.clone(),
            handle: None,
        };
        writer.record(sample_row("a"));
        writer.record(sample_row("b"));
        assert_eq!(writer.dropped(), 2);
        // 先断开 receiver，避免 Drop 中 Shutdown 的阻塞 send 永久等待
        drop(rx);
        drop(writer);
    }

    #[test]
    fn dropped_and_persisted_conserve_total_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("telemetry.db");
        // 极小容量 + 瞬间灌入，触发丢弃；总量守恒：落库 + 丢弃 == 总数
        let writer = TelemetryWriter::spawn(path.clone(), 1).unwrap();
        let total = 300u64;
        for i in 0..total {
            writer.record(TraceRow {
                req_id: format!("req-{i}"),
                ..Default::default()
            });
        }
        writer.flush();
        let stored: i64 = {
            let conn = Connection::open(&path).unwrap();
            conn.query_row("SELECT COUNT(*) FROM requests_v2", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(stored as u64 + writer.dropped(), total);
        drop(writer);
    }

    #[test]
    fn spawn_twice_same_path_reuses_tables() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("telemetry.db");
        {
            let writer = TelemetryWriter::spawn(path.clone(), 8).unwrap();
            writer.record(sample_row("first"));
            writer.flush();
        }
        // 第二次 spawn：表已存在应幂等复用
        let writer = TelemetryWriter::spawn(path.clone(), 8).unwrap();
        writer.record(sample_row("second"));
        writer.flush();
        {
            let conn = Connection::open(&path).unwrap();
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM requests_v2", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 2);
        }
        drop(writer);
    }

    #[test]
    fn drop_releases_sqlite_files_on_windows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("telemetry.db");
        let writer = TelemetryWriter::spawn(path.clone(), 8).unwrap();
        writer.record(sample_row("r"));
        drop(writer);
        // 句柄已释放：Windows 下应可直接删除整个目录
        let removed = std::fs::remove_dir_all(dir.path());
        assert!(removed.is_ok(), "SQLite 句柄未释放: {removed:?}");
    }
}

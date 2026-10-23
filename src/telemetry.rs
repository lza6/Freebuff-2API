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
/// 落库前对敏感字段脱敏（Cookie/Bearer/authorization → ***），见 `crate::redact`。
fn writer_loop(conn: Connection, rx: Receiver<Msg>) {
    for msg in rx {
        match msg {
            Msg::Row(mut row) => {
                // v0.8 脱敏：错误摘要/路由原因/请求 key 都可能含凭证片段
                if let Some(ex) = row.error_excerpt.take() {
                    row.error_excerpt = Some(crate::redact::redact(&ex));
                }
                if let Some(rr) = row.route_reason.take() {
                    row.route_reason = Some(crate::redact::redact(&rr));
                }
                if let Some(k) = row.api_key.take() {
                    row.api_key = Some(crate::redact::redact(&k));
                }
                if let Err(e) = insert_row(&conn, &row) {
                    tracing::warn!(error = %e, "遥测明细写入失败");
                }
            }
            Msg::Event {
                req_id,
                kind,
                detail,
            } => {
                let detail = crate::redact::redact(&detail);
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
    // 审计 L1：设 busy_timeout，避免与后台写线程瞬时争锁时 SQLITE_BUSY
    let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
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

/// "三最"遥测聚合：最慢账号 Top3、最常用模型 Top5、错误率最高时段 Top3。
///
/// - window_hours：>0 时只统计最近 N 小时；否则不限
/// - 全部聚合在本地 SQLite 完成；空库返回空数组，不报错
/// - account 空/NULL 回退 "unknown"；错误判定 status >= 400（覆盖 5xx）
/// - 慢账号按总耗时均值(latency_ms)降序，并列按 TTFT 均值(ttft_ms)降序，
///   同时返回 TTFT 均值（指南 2.3「TTFT 均值」口径）
pub fn insights(db_path: &str, hours: i64) -> Result<serde_json::Value> {
    let conn = open_db(Path::new(db_path))?;
    let generated_at = Utc::now();
    let cutoff = if hours > 0 {
        (generated_at - chrono::Duration::hours(hours)).to_rfc3339()
    } else {
        "1970-01-01T00:00:00Z".to_string()
    };

    // 1) 最慢账号 Top3
    let mut slowest_stmt = conn.prepare(
        "SELECT COALESCE(NULLIF(TRIM(account), ''), 'unknown') AS acct,
                COUNT(*), AVG(latency_ms), AVG(ttft_ms)
         FROM requests_v2 WHERE ts >= ?1
         GROUP BY COALESCE(NULLIF(TRIM(account), ''), 'unknown')",
    )?;
    let mut slowest: Vec<(String, i64, f64, Option<f64>)> = slowest_stmt
        .query_map(params![cutoff], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, f64>(2)?,
                r.get::<_, Option<f64>>(3)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    slowest.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                b.3.unwrap_or(0.0)
                    .partial_cmp(&a.3.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
    slowest.truncate(3);
    let slowest_accounts: Vec<serde_json::Value> = slowest
        .into_iter()
        .map(|(acct, n, total, ttft)| {
            serde_json::json!({
                "account": acct,
                "requests": n,
                "avg_total_ms": (total * 10.0).round() / 10.0,
                "avg_first_byte_ms": (ttft.unwrap_or(0.0) * 10.0).round() / 10.0,
            })
        })
        .collect();
    // 2) 最常用模型 Top5（请求数降序 + 错误率）
    let mut model_stmt = conn.prepare(
        "SELECT COALESCE(NULLIF(TRIM(resolved_model), ''), NULLIF(TRIM(requested_model), ''), 'unknown') AS model,
                COUNT(*), SUM(CASE WHEN status >= 400 THEN 1 ELSE 0 END)
         FROM requests_v2 WHERE ts >= ?1
         GROUP BY COALESCE(NULLIF(TRIM(resolved_model), ''), NULLIF(TRIM(requested_model), ''), 'unknown')",
    )?;
    let mut models: Vec<(String, i64, i64)> = model_stmt
        .query_map(params![cutoff], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    models.sort_by_key(|x| std::cmp::Reverse(x.1));
    models.truncate(5);
    let top_models: Vec<serde_json::Value> = models
        .into_iter()
        .map(|(model, n, errs)| {
            serde_json::json!({
                "model": model,
                "requests": n,
                "error_rate": if n > 0 { round_rate(errs as f64 / n as f64) } else { 0.0 },
            })
        })
        .collect();

    // 3) 错误率最高时段 Top3（按小时 UTC）
    let mut hour_stmt = conn.prepare(
        "SELECT substr(ts, 1, 13) AS hour_utc, COUNT(*),
                SUM(CASE WHEN status >= 400 THEN 1 ELSE 0 END)
         FROM requests_v2 WHERE ts >= ?1
         GROUP BY substr(ts, 1, 13)",
    )?;
    let mut hour_rows: Vec<(String, i64, i64)> = hour_stmt
        .query_map(params![cutoff], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    hour_rows.sort_by(|a, b| {
        let ra = if a.1 > 0 {
            a.2 as f64 / a.1 as f64
        } else {
            0.0
        };
        let rb = if b.1 > 0 {
            b.2 as f64 / b.1 as f64
        } else {
            0.0
        };
        rb.partial_cmp(&ra)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.1.cmp(&a.1))
    });
    hour_rows.truncate(3);
    let worst_hours: Vec<serde_json::Value> = hour_rows
        .into_iter()
        .map(|(hour, n, errs)| {
            serde_json::json!({
                "hour_utc": hour,
                "requests": n,
                "error_rate": if n > 0 { round_rate(errs as f64 / n as f64) } else { 0.0 },
            })
        })
        .collect();

    Ok(serde_json::json!({
        "window_hours": hours,
        "slowest_accounts": slowest_accounts,
        "top_models": top_models,
        "worst_hours": worst_hours,
        "generated_at": generated_at.to_rfc3339(),
    }))
}

/// 归一到 4 位小数的错误率，避免浮点尾差
fn round_rate(rate: f64) -> f64 {
    (rate * 10_000.0).round() / 10_000.0
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

    /// 直接插入一条已知时间戳的请求（不走写线程，便于构造窗口/时段样本）
    #[allow(clippy::too_many_arguments)] // 测试构造器：字段多，参数清晰优先
    fn insert_direct(
        conn: &Connection,
        req_id: &str,
        ts: &str,
        account: &str,
        model: &str,
        status: i64,
        latency_ms: i64,
        ttft_ms: Option<i64>,
    ) {
        conn.execute(
            "INSERT INTO requests_v2 (req_id, ts, requested_model, resolved_model, account, status, latency_ms, ttft_ms)
             VALUES (?1,?2,?3,?3,?4,?5,?6,?7)",
            params![req_id, ts, model, account, status, latency_ms, ttft_ms],
        )
        .unwrap();
    }

    #[test]
    fn insights_slowest_accounts_ranking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("telemetry.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let now = Utc::now().to_rfc3339();
        insert_direct(
            &conn,
            "r1",
            &now,
            "acct-slow",
            "gpt-4o",
            200,
            1000,
            Some(400),
        );
        insert_direct(
            &conn,
            "r2",
            &now,
            "acct-slow",
            "gpt-4o",
            200,
            800,
            Some(300),
        );
        insert_direct(&conn, "r3", &now, "acct-mid", "gpt-4o", 200, 500, Some(200));
        insert_direct(&conn, "r4", &now, "acct-fast", "claude", 200, 100, Some(50));
        insert_direct(&conn, "r5", &now, "acct-fast", "claude", 200, 100, Some(60));
        insert_direct(&conn, "r6", &now, "acct-fast", "claude", 200, 100, Some(70));

        let v = insights(path.to_str().unwrap(), 24).unwrap();
        let arr = v["slowest_accounts"].as_array().unwrap();
        assert_eq!(arr.len(), 3);
        let accounts: Vec<String> = arr
            .iter()
            .map(|x| x["account"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(accounts, vec!["acct-slow", "acct-mid", "acct-fast"]);
        assert_eq!(arr[0]["requests"], 2);
        assert_eq!(arr[0]["avg_total_ms"], 900.0);
        assert_eq!(arr[0]["avg_first_byte_ms"], 350.0);
        assert_eq!(arr[1]["avg_total_ms"], 500.0);
        assert_eq!(arr[2]["avg_total_ms"], 100.0);
    }

    #[test]
    fn insights_top_models_counts_and_error_rate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("telemetry.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let now = Utc::now().to_rfc3339();
        insert_direct(&conn, "a", &now, "acct", "gpt-4o", 200, 100, Some(10));
        insert_direct(&conn, "b", &now, "acct", "gpt-4o", 400, 100, Some(10));
        insert_direct(&conn, "c", &now, "acct", "gpt-4o", 500, 100, Some(10));
        insert_direct(&conn, "d", &now, "acct", "gpt-4o", 200, 100, Some(10));
        insert_direct(&conn, "e", &now, "acct", "claude", 200, 100, Some(10));
        insert_direct(&conn, "f", &now, "acct", "claude", 200, 100, Some(10));

        let v = insights(path.to_str().unwrap(), 24).unwrap();
        let arr = v["top_models"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["model"], "gpt-4o");
        assert_eq!(arr[0]["requests"], 4);
        assert_eq!(arr[0]["error_rate"], 0.5);
        assert_eq!(arr[1]["model"], "claude");
        assert_eq!(arr[1]["requests"], 2);
        assert_eq!(arr[1]["error_rate"], 0.0);
    }
    #[test]
    fn insights_worst_hours_ranking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("telemetry.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        // 时间相对化：insights 只统计最近 24h，硬编码日期会随时间流逝滑出窗口（测试必红）
        let h1_dt = Utc::now() - chrono::Duration::hours(1);
        let h2_dt = Utc::now();
        let h1 = h1_dt.to_rfc3339();
        let h2 = h2_dt.to_rfc3339();
        // hour_utc 取 ts 前 13 字符（YYYY-MM-DDTHH）
        let h1_key = &h1[..13];
        let h2_key = &h2[..13];
        insert_direct(&conn, "a", &h1, "acct", "gpt-4o", 500, 100, Some(10));
        insert_direct(&conn, "b", &h1, "acct", "gpt-4o", 200, 100, Some(10));
        insert_direct(&conn, "c", &h1, "acct", "gpt-4o", 200, 100, Some(10));
        insert_direct(&conn, "d", &h1, "acct", "gpt-4o", 200, 100, Some(10));
        insert_direct(&conn, "e", &h2, "acct", "claude", 200, 100, Some(10));
        insert_direct(&conn, "f", &h2, "acct", "claude", 200, 100, Some(10));

        let v = insights(path.to_str().unwrap(), 24).unwrap();
        let arr = v["worst_hours"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["hour_utc"], h1_key);
        assert_eq!(arr[0]["requests"], 4);
        assert_eq!(arr[0]["error_rate"], 0.25);
        assert_eq!(arr[1]["hour_utc"], h2_key);
        assert_eq!(arr[1]["error_rate"], 0.0);
    }

    #[test]
    fn insights_empty_db_returns_empty_arrays() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("telemetry-empty.db");
        let v = insights(path.to_str().unwrap(), 24).unwrap();
        assert!(v["slowest_accounts"].as_array().unwrap().is_empty());
        assert!(v["top_models"].as_array().unwrap().is_empty());
        assert!(v["worst_hours"].as_array().unwrap().is_empty());
        assert_eq!(v["window_hours"], 24);
    }

    #[test]
    fn insights_window_filters_and_unknown_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("telemetry.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let old = (Utc::now() - chrono::Duration::hours(48)).to_rfc3339();
        let now = Utc::now().to_rfc3339();
        insert_direct(&conn, "a", &old, "acct-old", "gpt-4o", 200, 100, Some(10));
        insert_direct(&conn, "b", &now, "", "claude", 500, 200, Some(20));

        let v = insights(path.to_str().unwrap(), 24).unwrap();
        assert_eq!(v["slowest_accounts"].as_array().unwrap().len(), 1);
        assert_eq!(v["slowest_accounts"][0]["account"], "unknown");
        let models = v["top_models"].as_array().unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["model"], "claude");
        assert_eq!(models[0]["error_rate"], 1.0);
    }
}

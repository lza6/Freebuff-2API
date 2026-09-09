//! 用量统计：SQLite 记录请求/模型/token/延迟/错误
//!
//! 表结构：
//! - requests: id, ts, account, model, prompt_tokens, completion_tokens, latency_ms, status, api_key, client_ip
//! - daily_usage: date, model, requests, prompt_tokens, completion_tokens

use anyhow::Result;
use chrono::{Datelike, Utc};
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UsageRecord {
    pub id: i64,
    pub ts: String,
    pub account: String,
    pub model: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub latency_ms: i64,
    pub status: i64,
    pub api_key: String,
    pub client_ip: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DailyUsage {
    pub date: String,
    pub model: String,
    pub requests: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub errors: i64,
}

pub struct UsageDb {
    conn: Arc<Mutex<Connection>>,
}

impl UsageDb {
    pub fn open(path: &str) -> Result<Self> {
        if let Some(dir) = Path::new(path).parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).ok();
            }
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS requests (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts TEXT NOT NULL,
                account TEXT NOT NULL,
                model TEXT NOT NULL,
                prompt_tokens INTEGER NOT NULL DEFAULT 0,
                completion_tokens INTEGER NOT NULL DEFAULT 0,
                latency_ms INTEGER NOT NULL DEFAULT 0,
                status INTEGER NOT NULL,
                api_key TEXT,
                client_ip TEXT
            );
            CREATE TABLE IF NOT EXISTS daily_usage (
                date TEXT NOT NULL,
                model TEXT NOT NULL,
                requests INTEGER NOT NULL DEFAULT 0,
                prompt_tokens INTEGER NOT NULL DEFAULT 0,
                completion_tokens INTEGER NOT NULL DEFAULT 0,
                errors INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (date, model)
            );
            "#,
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

#[allow(clippy::too_many_arguments)]
    pub fn record(
        &self,
        account: &str,
        model: &str,
        prompt_tokens: i64,
        completion_tokens: i64,
        latency_ms: i64,
        status: i64,
        api_key: &str,
        client_ip: &str,
    ) -> Result<()> {
        let ts = Utc::now().to_rfc3339();
        let date = format!("{}-{:02}-{:02}", Utc::now().year(), Utc::now().month(), Utc::now().day());
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO requests (ts,account,model,prompt_tokens,completion_tokens,latency_ms,status,api_key,client_ip) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![ts, account, model, prompt_tokens, completion_tokens, latency_ms, status, api_key, client_ip],
        )?;
        conn.execute(
            r#"INSERT INTO daily_usage (date,model,requests,prompt_tokens,completion_tokens,errors)
               VALUES (?1,?2,1,?3,?4,?5)
               ON CONFLICT(date,model) DO UPDATE SET
                 requests = requests + 1,
                 prompt_tokens = prompt_tokens + excluded.prompt_tokens,
                 completion_tokens = completion_tokens + excluded.completion_tokens,
                 errors = errors + excluded.errors"#,
            params![date, model, prompt_tokens, completion_tokens, if status >= 400 { 1 } else { 0 }],
        )?;
        Ok(())
    }

    pub fn recent_requests(&self, limit: i64) -> Result<Vec<UsageRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id,ts,account,model,prompt_tokens,completion_tokens,latency_ms,status,api_key,client_ip FROM requests ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| {
            Ok(UsageRecord {
                id: r.get(0)?,
                ts: r.get(1)?,
                account: r.get(2)?,
                model: r.get(3)?,
                prompt_tokens: r.get(4)?,
                completion_tokens: r.get(5)?,
                latency_ms: r.get(6)?,
                status: r.get(7)?,
                api_key: r.get(8).unwrap_or_default(),
                client_ip: r.get(9).unwrap_or_default(),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn daily_usage(&self, days: i64) -> Result<Vec<DailyUsage>> {
        let conn = self.conn.lock().unwrap();
        let cutoff = (Utc::now() - chrono::Duration::days(days)).date_naive().to_string();
        let mut stmt = conn.prepare(
            "SELECT date,model,requests,prompt_tokens,completion_tokens,errors FROM daily_usage WHERE date >= ?1 ORDER BY date DESC, model",
        )?;
        let rows = stmt.query_map(params![cutoff], |r| {
            Ok(DailyUsage {
                date: r.get(0)?,
                model: r.get(1)?,
                requests: r.get(2)?,
                prompt_tokens: r.get(3)?,
                completion_tokens: r.get(4)?,
                errors: r.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn totals(&self) -> Result<serde_json::Value> {
        let conn = self.conn.lock().unwrap();
        let reqs: i64 = conn.query_row("SELECT COUNT(*) FROM requests", [], |r| r.get(0))?;
        let prompt: i64 = conn.query_row("SELECT COALESCE(SUM(prompt_tokens),0) FROM requests", [], |r| r.get(0))?;
        let comp: i64 = conn.query_row("SELECT COALESCE(SUM(completion_tokens),0) FROM requests", [], |r| r.get(0))?;
        let errors: i64 = conn.query_row("SELECT COUNT(*) FROM requests WHERE status >= 400", [], |r| r.get(0))?;
        Ok(serde_json::json!({
            "total_requests": reqs,
            "prompt_tokens": prompt,
            "completion_tokens": comp,
            "total_tokens": prompt + comp,
            "errors": errors,
        }))
    }
}
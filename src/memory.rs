//! 用户记忆层：确定性规则记录 → trigram FTS5 检索 → 有界注入
//!
//! "AI 更懂用户"的最小闭环，全流程零 LLM 调用：
//! - [`MemoryStore::observe`] 用确定性规则从一次请求上下文提取记忆
//! - [`MemoryStore::upsert`] 用户显式写入；同 title 近重复走 supersede
//! - [`MemoryStore::search`] FTS5 trigram 子串匹配（中文可用），短查询回退 LIKE
//! - [`MemoryStore::brief`] 低权威注入块：预算内整条取舍、按 id 排序保证字节稳定
//!
//! 独立 SQLite 库（WAL），不与 `usage.rs` / `telemetry.rs` 争锁。

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

/// 记忆条目
#[derive(Debug, Clone, serde::Serialize)]
pub struct Memory {
    pub id: String,
    /// preference | correction | habit | project | feedback
    pub kind: String,
    pub title: String,
    pub content: String,
    /// user | project
    pub scope: String,
    /// 稳定事实（true）vs 近期活动（false）
    pub is_static: bool,
    /// 0-10
    pub confidence: i64,
    pub use_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// 建表语句（幂等）；FTS 同步由写入路径维护
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS memories(
  id TEXT PRIMARY KEY, kind TEXT NOT NULL, title TEXT NOT NULL, content TEXT NOT NULL,
  scope TEXT NOT NULL DEFAULT 'user', is_static INTEGER NOT NULL DEFAULT 0,
  confidence INTEGER NOT NULL DEFAULT 5, use_count INTEGER NOT NULL DEFAULT 0,
  is_latest INTEGER NOT NULL DEFAULT 1, parent_id TEXT,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS idx_memories_latest ON memories(is_latest, updated_at);
CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(id UNINDEXED, title, content, tokenize='trigram');
"#;

/// 查询列顺序（与 [`row_to_memory`] 对应）
const COLS: &str = "id,kind,title,content,scope,is_static,confidence,use_count,created_at,updated_at";
/// 检索排序：稳定事实优先，其后置信度 / 最近更新，id 决胜保证确定性
const ORDER_BY: &str = "m.is_static DESC, m.confidence DESC, m.updated_at DESC, m.id ASC";

/// 默认置信度
const DEFAULT_CONFIDENCE: i64 = 5;
/// FTS trigram 最短可匹配字符数，不足回退 LIKE 子串匹配
const FTS_MIN_CHARS: usize = 3;
/// 注入块候选条数
const BRIEF_CANDIDATES: usize = 8;
/// token→字符 粗估（中文约 1 字 1 token，英文约 4 字符 1 token，取折中）
const CHARS_PER_TOKEN: usize = 2;
/// 纠正 / 偏好指令标记（中英，大小写不敏感）
const CORRECTION_MARKERS: &[&str] = &[
    // 中文指令式短语（带标点/上下文，降低误报）
    "记住：", "记住:", "记住，", "以后都", "别再", "不要再", "下次要", "以后要",
    // 英文指令式短语（避免 always/never 单独命中造成的误报）
    "remember that", "remember to", "remember:", "always use", "never use",
    "don't use", "do not use", "from now on",
];

/// 用户记忆库（独立 SQLite 连接，WAL）
pub struct MemoryStore {
    conn: Arc<Mutex<Connection>>,
}

impl MemoryStore {
    /// 打开/建库（独立文件）；幂等
    pub fn open(db_path: PathBuf) -> Result<Self> {
        if let Some(dir) = db_path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("创建记忆库目录失败: {}", dir.display()))?;
            }
        }
        let conn = Connection::open(&db_path)
            .with_context(|| format!("打开记忆库失败: {}", db_path.display()))?;
        // journal_mode 返回一行结果，须用 query_row 读取
        let _mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
            .context("启用 WAL 失败")?;
        conn.execute_batch(SCHEMA)
            .context("初始化记忆表失败（需 SQLite FTS5 trigram 支持）")?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// 确定性规则 observe（零 LLM）：从一次请求上下文提取观察
    ///
    /// - `model_used` → preference（同名累加 confidence）
    /// - `effort_downgraded(Some)` → feedback（档位被降级）
    /// - `user_text` 命中纠正标记 → correction（confidence=8）
    /// - `session_key` → project（近期活动）
    ///
    /// 返回新建/更新的记忆 id 列表。
    pub fn observe(
        &self,
        model_used: &str,
        effort_downgraded: Option<&str>,
        user_text: &str,
        session_key: Option<&str>,
    ) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        let model = model_used.trim();
        if !model.is_empty() {
            let title = format!("常用模型: {model}");
            let content = format!("用户经常使用模型 {model}。");
            ids.push(self.observe_upsert(
                "preference",
                &title,
                &content,
                DEFAULT_CONFIDENCE,
                false,
                true,
            )?);
        }
        if let Some(detail) = effort_downgraded.map(str::trim).filter(|s| !s.is_empty()) {
            let content = format!("用户请求的推理档位被降级为 {detail}。");
            ids.push(self.observe_upsert(
                "feedback",
                "推理档位被降级",
                &content,
                DEFAULT_CONFIDENCE,
                false,
                false,
            )?);
        }
        if has_correction_marker(user_text) {
            ids.push(self.record_correction(user_text)?);
        }
        if let Some(key) = session_key.map(str::trim).filter(|s| !s.is_empty()) {
            let key = truncate_chars(&single_line(key), 60);
            let title = format!("项目: {key}");
            let content = format!("用户近期在项目 {key} 中活动。");
            ids.push(self.observe_upsert(
                "project",
                &title,
                &content,
                DEFAULT_CONFIDENCE,
                false,
                false,
            )?);
        }
        Ok(ids)
    }

    /// 显式写入（用户手动添加）；同 title 近重复则 supersede 更新
    pub fn upsert(&self, kind: &str, title: &str, content: &str, is_static: bool) -> Result<Memory> {
        let title = title.trim();
        if title.is_empty() {
            anyhow::bail!("记忆标题不能为空");
        }
        let conn = self.lock_conn()?;
        let prev = latest_id_by_title(&conn, title)?;
        if let Some(prev_id) = prev.as_deref() {
            // 旧版本退役，新版本接管（parent_id 串成版本链）
            conn.execute(
                "UPDATE memories SET is_latest=0, updated_at=?1 WHERE id=?2",
                params![now_ts(), prev_id],
            )?;
        }
        let id = insert_memory(
            &conn,
            kind,
            title,
            content,
            "user",
            is_static,
            DEFAULT_CONFIDENCE,
            prev.as_deref(),
        )?;
        memory_by_id(&conn, &id)?.context("新写入的记忆不存在")
    }

    /// 检索：trigram FTS5 MATCH（中文可用）+ is_static 优先；返回 top-k
    ///
    /// 查询不足 [`FTS_MIN_CHARS`] 字符时回退 LIKE 子串匹配（trigram 无法匹配短串）。
    pub fn search(&self, query: &str, top_k: usize) -> Vec<Memory> {
        let q = query.trim();
        if q.is_empty() || top_k == 0 {
            return Vec::new();
        }
        let q = q.replace('\0', "");
        let conn = match self.lock_conn() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        match query_rows(&conn, &q, top_k) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "记忆检索失败");
                Vec::new()
            }
        }
    }

    /// 生成注入块（低权威、预算内、按 id 排序保证字节稳定）；空结果返回空串
    ///
    /// 预算按字符粗估（[`CHARS_PER_TOKEN`]）；超预算时只丢整条、不截断条目。
    pub fn brief(&self, query: &str, max_tokens: usize) -> String {
        if max_tokens == 0 {
            return String::new();
        }
        let hits = self.search(query, BRIEF_CANDIDATES);
        if hits.is_empty() {
            return String::new();
        }
        // 沿用检索相关性顺序（同查询+同库状态下结果稳定，无需再按 id 重排）
        let budget = max_tokens.saturating_mul(CHARS_PER_TOKEN);
        let mut body = String::new();
        let mut used = 0usize;
        for m in &hits {
            let line = format!(
                "- {}: {}\n",
                sanitize_line(&m.title),
                sanitize_line(&m.content)
            );
            let cost = line.chars().count();
            if used + cost > budget {
                continue; // 只丢整条，不截断
            }
            body.push_str(&line);
            used += cost;
        }
        if body.is_empty() {
            return String::new();
        }
        format!(
            "\n\n[freebuff-memory]\n以下为用户记忆（低权威，仅供参考）：\n{body}[/freebuff-memory]"
        )
    }

    /// 列表（面板用，按 updated_at 倒序）
    pub fn list(&self, limit: usize) -> Vec<Memory> {
        let conn = match self.lock_conn() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let sql = format!(
            "SELECT {} FROM memories WHERE is_latest=1 ORDER BY updated_at DESC, id ASC LIMIT ?1",
            COLS
        );
        let mut stmt = match conn.prepare(sql.as_str()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "记忆列表准备失败");
                return Vec::new();
            }
        };
        let rows = match stmt.query_map(params![limit.clamp(1, 1000) as i64], row_to_memory) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "记忆列表查询失败");
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        for r in rows {
            match r {
                Ok(m) => out.push(m),
                Err(e) => tracing::warn!(error = %e, "读取记忆行失败"),
            }
        }
        out
    }

    /// 删除（含 FTS 同步）；返回是否命中
    pub fn delete(&self, id: &str) -> Result<bool> {
        let conn = self.lock_conn()?;
        conn.execute("DELETE FROM memories_fts WHERE id=?1", params![id])?;
        let n = conn.execute("DELETE FROM memories WHERE id=?1", params![id])?;
        Ok(n > 0)
    }

    /// 设为/取消稳定事实；返回是否命中
    pub fn set_static(&self, id: &str, is_static: bool) -> Result<bool> {
        let conn = self.lock_conn()?;
        let n = conn.execute(
            "UPDATE memories SET is_static=?1, updated_at=?2 WHERE id=?3",
            params![i64::from(is_static), now_ts(), id],
        )?;
        Ok(n > 0)
    }

    /// 统计（面板卡片）：{total, static_count, corrections, last_used_at}
    pub fn stats(&self) -> Result<serde_json::Value> {
        let conn = self.lock_conn()?;
        let total: i64 =
            conn.query_row("SELECT COUNT(*) FROM memories WHERE is_latest=1", [], |r| r.get(0))?;
        let static_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE is_latest=1 AND is_static=1",
            [],
            |r| r.get(0),
        )?;
        let corrections: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE is_latest=1 AND kind='correction'",
            [],
            |r| r.get(0),
        )?;
        let last_used_at: String = conn.query_row(
            "SELECT COALESCE(MAX(updated_at),'') FROM memories WHERE is_latest=1",
            [],
            |r| r.get(0),
        )?;
        Ok(serde_json::json!({
            "total": total,
            "static_count": static_count,
            "corrections": corrections,
            "last_used_at": last_used_at,
        }))
    }

    /// 观察式写入：同 title 已有最新条目则累加计数并覆盖内容，否则新建
    fn observe_upsert(
        &self,
        kind: &str,
        title: &str,
        content: &str,
        confidence: i64,
        is_static: bool,
        grow_confidence: bool,
    ) -> Result<String> {
        let conn = self.lock_conn()?;
        let now = now_ts();
        match latest_id_by_title(&conn, title)? {
            Some(id) => {
                if grow_confidence {
                    conn.execute(
                        "UPDATE memories SET use_count=use_count+1, confidence=MIN(10,confidence+1), content=?1, updated_at=?2 WHERE id=?3",
                        params![content, now, id],
                    )?;
                } else {
                    conn.execute(
                        "UPDATE memories SET use_count=use_count+1, confidence=MAX(confidence,?1), content=?2, updated_at=?3 WHERE id=?4",
                        params![confidence, content, now, id],
                    )?;
                }
                sync_fts(&conn, &id, title, content)?;
                Ok(id)
            }
            None => insert_memory(&conn, kind, title, content, "user", is_static, confidence, None),
        }
    }

    /// 记录一条用户纠正/偏好指令（高权重，confidence=8）
    fn record_correction(&self, user_text: &str) -> Result<String> {
        // 脱敏后再落盘：防止用户粘贴的密钥/凭证被持久化并随注入外传
        let text = redact_secrets(&single_line(user_text));
        let title = format!("用户纠正: {}", truncate_chars(&text, 24));
        let content = truncate_chars(&text, 500);
        self.observe_upsert("correction", &title, &content, 8, false, false)
    }

    /// 获取连接锁；锁中毒时恢复内部数据继续使用（SQLite 连接本身仍有效）
    fn lock_conn(&self) -> Result<MutexGuard<'_, Connection>> {
        match self.conn.lock() {
            Ok(g) => Ok(g),
            Err(poisoned) => Ok(poisoned.into_inner()),
        }
    }
}

/// 检索分发：>= 3 字符走 FTS trigram，失败或短查询回退 LIKE
fn query_rows(conn: &Connection, q: &str, top_k: usize) -> Result<Vec<Memory>> {
    // 1) 短语 FTS（短查询 / 查询串恰为记忆子串）
    if q.chars().count() >= FTS_MIN_CHARS {
        match query_fts(conn, q, top_k) {
            Ok(v) if !v.is_empty() => return Ok(v),
            Ok(_) => {} // 空结果 → 继续降级（长自然句几乎不可能是记忆的整段子串）
            Err(e) => tracing::warn!(error = %e, "FTS 检索失败，回退关键词"),
        }
    }
    // 2) 关键词片段 OR 匹配（长句检索的关键路径：CJK 3-gram / 英文词，按命中数排序）
    let terms = extract_terms(q);
    if !terms.is_empty() {
        if let Ok(v) = query_terms(conn, &terms, top_k) {
            if !v.is_empty() {
                return Ok(v);
            }
        }
    }
    // 3) 整串 LIKE 回退（短查询）
    query_like(conn, q, top_k)
}

/// 关键词片段上限
const MAX_TERMS: usize = 12;
/// query_terms 的 SQL 占位符数量（固定）
const TERM_SLOTS: usize = 8;

/// 检索片段提取：
/// - ASCII 词：按非字母数字切分，保留 ≥3 字符（小写）
/// - CJK：连续非 ASCII 段生成 3-gram（trigram 索引的最小匹配单位）
fn extract_terms(q: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for w in q.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if w.chars().count() >= 3 {
            terms.push(w.to_ascii_lowercase());
        }
    }
    let chars: Vec<char> = q.chars().collect();
    if chars.len() >= 3 {
        for i in 0..=chars.len() - 3 {
            let win = &chars[i..i + 3];
            if win
                .iter()
                .all(|c| !c.is_ascii() && !c.is_whitespace() && !c.is_control())
            {
                terms.push(win.iter().collect());
            }
        }
    }
    terms.sort();
    terms.dedup();
    terms.truncate(MAX_TERMS);
    terms
}

/// 关键词片段 OR 匹配（LIKE，参数化），按命中片段数排序。
/// 记忆库为本地小规模（数百条），全表 LIKE 的代价可接受。
fn query_terms(conn: &Connection, terms: &[String], top_k: usize) -> Result<Vec<Memory>> {
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let mut sql = format!("SELECT {COLS} FROM memories m WHERE m.is_latest=1 AND (");
    for i in 1..=TERM_SLOTS {
        if i > 1 {
            sql.push_str(" OR ");
        }
        sql.push_str(&format!(
            "m.title LIKE ?{i} ESCAPE '\\' OR m.content LIKE ?{i} ESCAPE '\\'"
        ));
    }
    sql.push_str(") LIMIT 200");
    let mut stmt = conn.prepare(sql.as_str())?;
    // 固定 8 个槽位（不足时轮转填充）
    let pats: Vec<String> = (0..TERM_SLOTS)
        .map(|i| like_pattern(&terms[i % terms.len()]))
        .collect();
    let params_vec: Vec<&dyn rusqlite::ToSql> = pats.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
    let rows = stmt.query_map(params_vec.as_slice(), row_to_memory)?;
    let mut scored: Vec<(usize, Memory)> = Vec::new();
    for r in rows {
        let m = r?;
        let hay = format!("{} {}", m.title.to_lowercase(), m.content.to_lowercase());
        let score = terms
            .iter()
            .filter(|t| hay.contains(t.as_str()) || m.title.contains(t.as_str()) || m.content.contains(t.as_str()))
            .count();
        if score > 0 {
            scored.push((score, m));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    Ok(scored.into_iter().map(|(_, m)| m).take(top_k).collect())
}

/// FTS5 trigram 短语查询（查询串整体作为短语，双引号转义）
fn query_fts(conn: &Connection, q: &str, top_k: usize) -> Result<Vec<Memory>> {
    let sql = format!(
        "SELECT {} FROM memories m WHERE m.is_latest=1 AND m.id IN \
         (SELECT id FROM memories_fts WHERE memories_fts MATCH ?1) ORDER BY {} LIMIT ?2",
        COLS, ORDER_BY
    );
    let mut stmt = conn.prepare(sql.as_str())?;
    let rows = stmt.query_map(params![fts_phrase(q), limit_of(top_k)], row_to_memory)?;
    collect_memories(rows)
}

/// LIKE 子串回退（短查询 / FTS 异常时）；`%` `_` `\` 已转义
fn query_like(conn: &Connection, q: &str, top_k: usize) -> Result<Vec<Memory>> {
    let sql = format!(
        "SELECT {} FROM memories m WHERE m.is_latest=1 AND \
         (m.title LIKE ?1 ESCAPE '\\' OR m.content LIKE ?1 ESCAPE '\\') ORDER BY {} LIMIT ?2",
        COLS, ORDER_BY
    );
    let mut stmt = conn.prepare(sql.as_str())?;
    let rows = stmt.query_map(params![like_pattern(q), limit_of(top_k)], row_to_memory)?;
    collect_memories(rows)
}

/// 收集查询行（出错即传播，不静默吞）
fn collect_memories<I>(rows: I) -> Result<Vec<Memory>>
where
    I: Iterator<Item = rusqlite::Result<Memory>>,
{
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// top_k 收敛到安全上限
fn limit_of(top_k: usize) -> i64 {
    top_k.clamp(1, 500) as i64
}

/// FTS5 短语：整体加双引号，串内双引号翻倍转义（避免语法注入）
fn fts_phrase(q: &str) -> String {
    format!("\"{}\"", q.replace('"', "\"\""))
}

/// LIKE 模式：转义 `\` `%` `_` 后包裹 `%...%`
fn like_pattern(q: &str) -> String {
    let escaped = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{escaped}%")
}

/// 同 title 的最新版本 id
fn latest_id_by_title(conn: &Connection, title: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT id FROM memories WHERE title=?1 AND is_latest=1 ORDER BY updated_at DESC, id DESC LIMIT 1",
        params![title],
        |r| r.get(0),
    )
    .optional()
    .map_err(Into::into)
}

/// 按 id 读取单条
fn memory_by_id(conn: &Connection, id: &str) -> Result<Option<Memory>> {
    let sql = format!("SELECT {} FROM memories WHERE id = ?1", COLS);
    conn.query_row(sql.as_str(), params![id], row_to_memory)
        .optional()
        .map_err(Into::into)
}

/// 行 → 记忆条目
fn row_to_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<Memory> {
    Ok(Memory {
        id: row.get(0)?,
        kind: row.get(1)?,
        title: row.get(2)?,
        content: row.get(3)?,
        scope: row.get(4)?,
        is_static: row.get::<_, i64>(5)? != 0,
        confidence: row.get(6)?,
        use_count: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

/// 插入新条目（use_count 记为 1，即本次观察本身）并同步 FTS
#[allow(clippy::too_many_arguments)]
fn insert_memory(
    conn: &Connection,
    kind: &str,
    title: &str,
    content: &str,
    scope: &str,
    is_static: bool,
    confidence: i64,
    parent_id: Option<&str>,
) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_ts();
    conn.execute(
        "INSERT INTO memories (id,kind,title,content,scope,is_static,confidence,use_count,is_latest,parent_id,created_at,updated_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,1,1,?8,?9,?9)",
        params![
            id,
            kind,
            title,
            content,
            scope,
            i64::from(is_static),
            confidence,
            parent_id,
            now
        ],
    )
    .context("写入记忆失败")?;
    sync_fts(conn, &id, title, content)?;
    Ok(id)
}

/// FTS 同步：先按 id 删除再插入（trigram 不支持外部内容表，需手动维护）
fn sync_fts(conn: &Connection, id: &str, title: &str, content: &str) -> Result<()> {
    conn.execute("DELETE FROM memories_fts WHERE id = ?1", params![id])?;
    conn.execute(
        "INSERT INTO memories_fts (id,title,content) VALUES (?1,?2,?3)",
        params![id, title, content],
    )?;
    Ok(())
}

/// 是否命中纠正 / 偏好指令标记（大小写不敏感）
fn has_correction_marker(text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    let lower = text.to_lowercase();
    CORRECTION_MARKERS.iter().any(|m| lower.contains(m))
}

/// 单行化：换行替换为空格（防止伪造多行注入条目）
fn single_line(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
}

/// 按字符截断（UTF-8 安全）
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

/// 注入安全：条目内的记忆块标记转义，防止突破 low-authority 块边界。
/// 覆盖：大小写变体（[FreeBuff-Memory] 等）+ Unicode 行分隔符（U+2028/U+2029）。
fn escape_marker(s: &str) -> String {
    // 1) Unicode 行分隔符归一化为空格（防多行伪造）
    let normalized: String = s
        .chars()
        .map(|c| if c == '\u{2028}' || c == '\u{2029}' { ' ' } else { c })
        .collect();
    // 2) 大小写不敏感替换（to_ascii_lowercase 保持字节长度不变，索引安全）
    let lowered = normalized.to_ascii_lowercase();
    let mut out = String::with_capacity(normalized.len());
    let mut i = 0;
    while i < normalized.len() {
        if lowered[i..].starts_with("[freebuff-memory]") {
            out.push_str("[freebuff-memory)");
            i += "[freebuff-memory]".len();
        } else if lowered[i..].starts_with("[/freebuff-memory]") {
            out.push_str("[/freebuff-memory)");
            i += "[/freebuff-memory]".len();
        } else {
            let ch = normalized[i..].chars().next().unwrap_or('?');
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// 写入前脱敏：遮蔽常见密钥/凭证样式（sk-xxx / Bearer xxx / key=value 形式）
fn redact_secrets(s: &str) -> String {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(
            r"(?i)(sk-[A-Za-z0-9_\-]{6,}|bearer\s+[A-Za-z0-9._\-]{12,}|(?:session-token|api[_-]?key|access[_-]?token|refresh[_-]?token|password|passwd|secret|凭证)\s*[=:：]\s*[^\s;,，。]{6,})",
        )
        .expect("redact regex 编译失败")
    });
    re.replace_all(s, "[已脱敏]").to_string()
}

/// brief 行内容：单行化 + 标记转义
fn sanitize_line(s: &str) -> String {
    escape_marker(&single_line(s))
}

/// 当前时间（RFC3339）
fn now_ts() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 独立临时库
    fn test_store() -> (tempfile::TempDir, MemoryStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(dir.path().join("memory.db")).unwrap();
        (dir, store)
    }

    #[test]
    fn observe_model_preference_accumulates() {
        let (_dir, store) = test_store();
        let ids1 = store.observe("claude-sonnet-5", None, "", None).unwrap();
        assert_eq!(ids1.len(), 1);
        let first = store.list(10);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].kind, "preference");
        assert_eq!(first[0].id, ids1[0]);
        assert!(first[0].title.contains("claude-sonnet-5"));

        // 第二次同模型：更新同一条，confidence / use_count 均增长
        let ids2 = store.observe("claude-sonnet-5", None, "", None).unwrap();
        assert_eq!(ids2, ids1, "同名模型应更新同一条记忆");
        let second = store.list(10);
        assert_eq!(second.len(), 1, "不应产生第二条");
        assert!(second[0].confidence > first[0].confidence);
        assert!(second[0].use_count > first[0].use_count);
    }

    #[test]
    fn observe_detects_correction_patterns() {
        let (_dir, store) = test_store();
        let ids = store.observe("", None, "不对，以后都用中文回答", None).unwrap();
        assert_eq!(ids.len(), 1);
        let rows = store.list(10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "correction");
        assert_eq!(rows[0].confidence, 8);
        assert!(rows[0].title.contains("用户纠正"));

        // 英文模式 + 大小写不敏感
        let ids = store.observe("", None, "Remember: ALWAYS use tabs", None).unwrap();
        assert_eq!(ids.len(), 1);
        let rows = store.list(10);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|m| m.kind == "correction"));

        // 非纠正文本不产生 correction
        let ids = store.observe("", None, "帮我看看这段代码", None).unwrap();
        assert!(ids.is_empty());
    }

    #[test]
    fn upsert_supersedes_duplicate_title() {
        let (_dir, store) = test_store();
        let first = store.upsert("preference", "回答语言", "用英文", true).unwrap();
        let second = store.upsert("preference", "回答语言", "用中文", true).unwrap();
        assert_ne!(first.id, second.id);

        let listed = store.list(10);
        assert_eq!(listed.len(), 1, "同 title 只保留一条最新");
        assert_eq!(listed[0].id, second.id);
        assert_eq!(listed[0].content, "用中文");

        let conn = store.conn.lock().unwrap();
        let latest: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories WHERE is_latest=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(latest, 1);
        let parent: Option<String> = conn
            .query_row(
                "SELECT parent_id FROM memories WHERE id=?1",
                params![second.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(parent.as_deref(), Some(first.id.as_str()), "新版本应指向旧版本");
        drop(conn); // 释放锁后再走 API（Mutex 不可重入）

        // 旧版本不再被检索命中
        assert!(store.search("用英文", 5).is_empty());
        assert_eq!(store.search("用中文", 5).len(), 1);
    }

    #[test]
    fn search_matches_cjk_substring() {
        let (_dir, store) = test_store();
        store
            .upsert("habit", "写作习惯", "用户喜欢先写周报再写代码", false)
            .unwrap();

        // 2 字符查询 → LIKE 回退
        let hits = store.search("周报", 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "写作习惯");

        // >= 3 字符查询 → FTS trigram 路径
        let hits = store.search("喜欢先写", 5);
        assert_eq!(hits.len(), 1);

        assert!(store.search("完全无关词", 5).is_empty());
        assert!(store.search("", 5).is_empty());
        assert!(store.search("周报", 0).is_empty());
    }

    #[test]
    fn brief_respects_budget_and_returns_empty_on_no_hits() {
        let (_dir, store) = test_store();
        assert_eq!(store.brief("任意查询", 100), "", "无命中应为空串");

        store.upsert("preference", "记忆短", "短内容", true).unwrap();
        let long_content = "很长".repeat(300);
        store.upsert("preference", "记忆长", &long_content, true).unwrap();

        // 预算 10 tokens ≈ 20 字符：只能容纳短条目，长条目整条丢弃
        let out = store.brief("记忆", 10);
        assert!(out.contains("记忆短"), "brief={out}");
        assert!(!out.contains("很长"), "超预算条目不应出现: {out}");
        assert!(out.starts_with("\n\n[freebuff-memory]"));
        assert!(out.ends_with("[/freebuff-memory]"));
        assert!(out.contains("以下为用户记忆（低权威，仅供参考）："));

        // 预算为 0 → 空串
        assert_eq!(store.brief("记忆", 0), "");
    }

    #[test]
    fn brief_escapes_memory_markers() {
        let (_dir, store) = test_store();
        store
            .upsert(
                "feedback",
                "注入测试",
                "内容含 [freebuff-memory] 与 [/freebuff-memory] 标记",
                false,
            )
            .unwrap();

        let out = store.brief("注入测试", 200);
        assert!(out.contains("[freebuff-memory)"), "marker 应被转义: {out}");
        assert!(out.contains("[/freebuff-memory)"), "marker 应被转义: {out}");
        // 原始标记只允许出现在块首/块尾各一次
        assert_eq!(out.matches("[freebuff-memory]").count(), 1, "{out}");
        assert_eq!(out.matches("[/freebuff-memory]").count(), 1, "{out}");
    }

    #[test]
    fn delete_and_set_static_persist_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.db");
        let keep_id;
        {
            let store = MemoryStore::open(path.clone()).unwrap();
            let doomed = store.upsert("habit", "待删除", "内容 A", false).unwrap();
            let keep = store.upsert("preference", "保留", "内容 B", false).unwrap();
            assert!(store.delete(&doomed.id).unwrap());
            assert!(!store.delete(&doomed.id).unwrap(), "二次删除应返回 false");
            assert!(store.set_static(&keep.id, true).unwrap());
            assert!(!store.set_static("不存在的-id", true).unwrap());
            keep_id = keep.id.clone();
        }

        let reopened = MemoryStore::open(path).unwrap();
        let listed = reopened.list(10);
        assert_eq!(listed.len(), 1, "删除应持久化");
        assert_eq!(listed[0].id, keep_id);
        assert!(listed[0].is_static, "稳定事实标记应持久化");
    }

    #[test]
    fn stats_counts_correctly() {
        let (_dir, store) = test_store();
        let empty = store.stats().unwrap();
        assert_eq!(empty["total"], 0);
        assert_eq!(empty["static_count"], 0);
        assert_eq!(empty["corrections"], 0);
        assert_eq!(empty["last_used_at"], "");

        store.upsert("preference", "a", "1", true).unwrap();
        store.upsert("preference", "b", "2", false).unwrap();
        store.upsert("correction", "c", "3", false).unwrap();

        let st = store.stats().unwrap();
        assert_eq!(st["total"], 3);
        assert_eq!(st["static_count"], 1);
        assert_eq!(st["corrections"], 1);
        assert!(!st["last_used_at"].as_str().unwrap_or("").is_empty());
    }

    #[test]
    fn reopen_is_idempotent_and_preserves_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.db");
        {
            let store = MemoryStore::open(path.clone()).unwrap();
            store.upsert("preference", "持久", "内容", true).unwrap();
        }
        let store = MemoryStore::open(path.clone()).unwrap();
        assert_eq!(store.list(10).len(), 1, "数据应保留");
        store.upsert("preference", "持久2", "内容2", false).unwrap();

        // 第三次 open：建表幂等，无重复表 / 无数据丢失
        let store = MemoryStore::open(path).unwrap();
        assert_eq!(store.list(10).len(), 2);
        assert!(!store.search("持久", 5).is_empty());
    }

    #[test]
    fn search_prefers_static_facts() {
        let (_dir, store) = test_store();
        let dynamic = store.upsert("habit", "动态偏好", "用户常问周报模板", false).unwrap();
        let stable = store.upsert("preference", "稳定偏好", "用户喜欢周报格式", true).unwrap();

        let hits = store.search("周报", 5);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, stable.id, "稳定事实应排在前面");

        // 标记互换后排序随之反转
        store.set_static(&dynamic.id, true).unwrap();
        store.set_static(&stable.id, false).unwrap();
        let hits = store.search("周报", 5);
        assert_eq!(hits[0].id, dynamic.id);
    }

    #[test]
    fn observe_records_feedback_and_project() {
        let (_dir, store) = test_store();
        let ids = store
            .observe("", Some("low"), "", Some("freebuff-2api"))
            .unwrap();
        assert_eq!(ids.len(), 2);
        let listed = store.list(10);
        assert_eq!(listed.len(), 2);
        assert!(listed
            .iter()
            .any(|m| m.kind == "feedback" && m.content.contains("low")));
        assert!(listed
            .iter()
            .any(|m| m.kind == "project" && m.title.contains("freebuff-2api")));

        // 重复观察只累加，不新建
        let ids2 = store
            .observe("", Some("low"), "", Some("freebuff-2api"))
            .unwrap();
        assert_eq!(ids2, ids);
        assert_eq!(store.list(10).len(), 2);
    }

    #[test]
    fn redacts_secrets_before_persist() {
        // 安全：用户粘贴的密钥样式不得明文落盘
        let s = redact_secrets(
            "我的 key 是 sk-abcdef123456 和 Bearer eyJhbGciOiJIUzI1NiJ9.abc，另外 password=hunter2xx",
        );
        assert!(!s.contains("sk-abcdef123456"), "sk- 密钥应被遮蔽: {s}");
        assert!(!s.contains("eyJhbGciOiJIUzI1NiJ9.abc"), "Bearer 应被遮蔽: {s}");
        assert!(!s.contains("hunter2xx"), "password= 应被遮蔽: {s}");
        assert!(s.contains("[已脱敏]"));
        // 正常文本不受影响
        let normal = redact_secrets("以后都用中文回答我");
        assert_eq!(normal, "以后都用中文回答我");
    }

    #[test]
    fn correction_marker_is_strict() {
        // 收紧后的标记：普通含 always/never 的句子不应误报
        assert!(!has_correction_marker("this always works fine"));
        assert!(!has_correction_marker("never mind, it's ok"));
        // 明确指令式短语应命中
        assert!(has_correction_marker("记住：以后都用中文"));
        assert!(has_correction_marker("please remember that I prefer tabs"));
        assert!(has_correction_marker("always use pnpm in this repo"));
    }

    #[test]
    fn escapes_marker_case_and_unicode_separators() {
        // 大小写变体必须被转义（否则可伪造块边界）
        assert!(escape_marker("[FreeBuff-Memory]").contains("[freebuff-memory)"));
        assert!(escape_marker("[/FREEBUFF-MEMORY]").contains("[/freebuff-memory)"));
        // Unicode 行分隔符归一化
        let s = escape_marker("a\u{2028}b\u{2029}c");
        assert!(!s.contains('\u{2028}') && !s.contains('\u{2029}'));
        // 普通内容不受影响
        assert_eq!(escape_marker("普通记忆内容"), "普通记忆内容");
    }

    #[test]
    fn observe_correction_redacts_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(dir.path().join("m.sqlite")).unwrap();
        store
            .observe("", None, "记住：我的 apikey=super-secret-value123", None)
            .unwrap();
        let listed = store.list(10);
        assert!(listed.iter().any(|m| m.kind == "correction"), "应记录纠正");
        assert!(
            !listed.iter().any(|m| m.content.contains("super-secret-value123")),
            "纠正内容不得含明文密钥: {:?}",
            listed.iter().map(|m| &m.content).collect::<Vec<_>>()
        );
    }

    #[test]
    fn long_sentence_retrieval_hits_by_terms() {
        // 回归：长自然句（整句不是记忆子串）必须通过关键词降级命中
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(dir.path().join("m.sqlite")).unwrap();
        store
            .upsert("preference", "语言偏好", "用户要求以后都用中文回答，代码注释也用中文", true)
            .unwrap();
        let hits = store.search("请记住用户要求以后都用中文回答我的问题", 5);
        assert!(!hits.is_empty(), "中文长句检索应命中（3-gram 降级）");
        let brief = store.brief("请记住用户要求以后都用中文回答我的问题", 512);
        assert!(brief.contains("中文"), "brief 应包含命中的记忆: {brief}");

        // 英文长句
        store.upsert("preference", "包管理器", "always use pnpm in this repo", true).unwrap();
        let hits2 = store.search("which package manager should I use for this repository", 5);
        assert!(!hits2.is_empty(), "英文长句应命中（词组降级）");
    }
}

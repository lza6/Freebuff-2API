//! 凭证账号信息缓存 + 账号使用记录（本地落盘，零新依赖）
//!
//! 两件事：
//! 1. `AccountMetaStore` —— 每条凭证最近一次的账号全貌快照（昵称/邮箱/套餐/今日剩余…），
//!    让「凭证列表」不用逐条打上游也能显示账号详细信息。
//! 2. **使用记录**（用户批注：「每个账号当然你也要有记录查询」）——每次成功拉取
//!    追加一条 JSONL 历史，可按凭证查询趋势。
//!
//! 落盘：`data/cred_meta.json`（覆盖写）+ `data/account_history.jsonl`（追加写）。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// 单条凭证的账号信息快照（全部字段可空——上游任一端点失败时降级不影响其他字段）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CredMeta {
    #[serde(default)]
    pub cred_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    /// 登录会话过期时间（上游 auth/session 的 expires）
    #[serde(default)]
    pub expires: Option<String>,
    #[serde(default)]
    pub access_tier: Option<String>,
    /// starter / plus / pro；None = 免费层
    #[serde(default)]
    pub tier_id: Option<String>,
    #[serde(default)]
    pub daily_limit: Option<i64>,
    #[serde(default)]
    pub daily_spent: Option<i64>,
    #[serde(default)]
    pub daily_remaining: Option<i64>,
    #[serde(default)]
    pub reset_at: Option<String>,
    #[serde(default)]
    pub streak_current: Option<i64>,
    #[serde(default)]
    pub all_time_active_days: Option<i64>,
    #[serde(default)]
    pub tokens_7d: Option<i64>,
    #[serde(default)]
    pub models: Vec<ModelQuota>,
    #[serde(default)]
    pub country_code: Option<String>,
    /// 非空表示地区受限（如 country_not_allowed）
    #[serde(default)]
    pub country_block_reason: Option<String>,
    /// 最近一次拉取是否成功
    #[serde(default)]
    pub valid: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub checked_at: String,
}

/// 逐模型今日额度
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelQuota {
    pub model: String,
    #[serde(default)]
    pub price: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub used: Option<i64>,
    #[serde(default)]
    pub remaining: Option<i64>,
    #[serde(default)]
    pub reset_at: Option<String>,
    #[serde(default)]
    pub pool_label: Option<String>,
}

/// 一条使用记录（用于按账号查询历史）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryRecord {
    pub ts: String,
    pub cred_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub tier_id: Option<String>,
    #[serde(default)]
    pub daily_limit: Option<i64>,
    #[serde(default)]
    pub daily_spent: Option<i64>,
    #[serde(default)]
    pub daily_remaining: Option<i64>,
    #[serde(default)]
    pub tokens_7d: Option<i64>,
    #[serde(default)]
    pub streak_current: Option<i64>,
    pub ok: bool,
}

/// 账号信息 / 使用记录存储（进程内缓存 + 文件落盘，Mutex 串行化写）
pub struct AccountMetaStore {
    meta_path: PathBuf,
    history_path: PathBuf,
    cache: Mutex<HashMap<String, CredMeta>>,
}

/// 历史文件超过该字节数时压缩保留最近 `HISTORY_KEEP` 条
const HISTORY_MAX_BYTES: u64 = 2 * 1024 * 1024;
const HISTORY_KEEP: usize = 2000;

/// history JSONL 的串行锁：append 与 compact 共用，消除"compact 读后、覆盖前"丢行窗口。
static HISTORY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 原子替换写（临时文件 + rename），防写入中途崩溃留下截断文件
fn atomic_write(path: &PathBuf, data: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, data)?;
    #[cfg(windows)]
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

impl AccountMetaStore {
    pub fn new(meta_path: impl Into<PathBuf>, history_path: impl Into<PathBuf>) -> Self {
        let meta_path = meta_path.into();
        let history_path = history_path.into();
        let cache = Self::load_meta(&meta_path);
        Self { meta_path, history_path, cache: Mutex::new(cache) }
    }

    fn load_meta(path: &PathBuf) -> HashMap<String, CredMeta> {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<HashMap<String, CredMeta>>(&t).ok())
            .unwrap_or_default()
    }

    pub fn get(&self, cred_id: &str) -> Option<CredMeta> {
        self.cache.lock().ok().and_then(|c| c.get(cred_id).cloned())
    }

    pub fn all(&self) -> HashMap<String, CredMeta> {
        self.cache.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// 写入/更新一条快照并落盘（失败仅记日志，不影响主流程）
    pub fn upsert(&self, meta: CredMeta) -> Result<()> {
        {
            let mut c = self.cache.lock().map_err(|_| anyhow::anyhow!("meta 缓存锁中毒"))?;
            c.insert(meta.cred_id.clone(), meta);
        }
        self.flush()
    }

    /// 删除某凭证的快照（凭证被移除时调用）
    pub fn remove(&self, cred_id: &str) -> Result<()> {
        {
            let mut c = self.cache.lock().map_err(|_| anyhow::anyhow!("meta 缓存锁中毒"))?;
            c.remove(cred_id);
        }
        self.flush()
    }

    /// 持锁期间完成快照 clone（与更新在同一临界区），写盘用原子替换——
    /// 消除 "锁外重拿锁 clone 旧快照后写盘" 的覆盖窗口。
    fn flush(&self) -> Result<()> {
        let json = {
            let c = self.cache.lock().map_err(|_| anyhow::anyhow!("meta 缓存锁中毒"))?;
            serde_json::to_string_pretty(&*c)?
        };
        atomic_write(&self.meta_path, &json)?;
        Ok(())
    }

    /// 追加一条使用记录（JSONL）；文件过大时自动压缩保留最近若干条
    pub fn append_history(&self, rec: &HistoryRecord) -> Result<()> {
        use std::io::Write;
        let line = serde_json::to_string(rec)?;
        let need_compact = {
            let _g = HISTORY_LOCK.lock().map_err(|_| anyhow::anyhow!("history 锁中毒"))?;
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&self.history_path)?;
            writeln!(f, "{line}")?;
            drop(f);
            std::fs::metadata(&self.history_path).map(|m| m.len()).unwrap_or(0) > HISTORY_MAX_BYTES
        };
        if need_compact {
            self.compact_history();
        }
        Ok(())
    }

    /// 压缩历史（持 HISTORY_LOCK + 临时文件 rename，append 行不会在读取后被覆盖丢失）
    fn compact_history(&self) {
        let _g = match HISTORY_LOCK.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let Ok(text) = std::fs::read_to_string(&self.history_path) else { return };
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        let keep: Vec<&str> = lines.iter().rev().take(HISTORY_KEEP).rev().copied().collect();
        let mut out = keep.join("\n");
        out.push('\n');
        if atomic_write(&self.history_path, &out).is_ok() {
            tracing::info!("使用记录已压缩至最近 {HISTORY_KEEP} 条");
        }
    }

    /// 读取使用记录（按时间倒序 = 最新在前）；`cred_id` 为空则返回全部账号
    pub fn history(&self, cred_id: Option<&str>, limit: usize) -> Result<Vec<HistoryRecord>> {
        let limit = limit.clamp(1, 1000);
        let text = match std::fs::read_to_string(&self.history_path) {
            Ok(t) => t,
            Err(_) => return Ok(Vec::new()),
        };
        let mut out: Vec<HistoryRecord> = text
            .lines()
            .rev()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<HistoryRecord>(l).ok())
            .filter(|r| cred_id.map(|id| r.cred_id == id).unwrap_or(true))
            .take(limit)
            .collect();
        // 已是倒序；保持"最新在前"
        out.shrink_to_fit();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store() -> (AccountMetaStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = AccountMetaStore::new(
            dir.path().join("cred_meta.json"),
            dir.path().join("account_history.jsonl"),
        );
        (store, dir)
    }

    #[test]
    fn upsert_and_reload() {
        let (store, dir) = tmp_store();
        let meta = CredMeta { cred_id: "abc".into(), email: Some("a@b.c".into()), valid: true, checked_at: "t".into(), ..Default::default() };
        store.upsert(meta).unwrap();
        let reopened = AccountMetaStore::new(dir.path().join("cred_meta.json"), dir.path().join("h.jsonl"));
        assert_eq!(reopened.get("abc").unwrap().email.as_deref(), Some("a@b.c"));
    }

    #[test]
    fn remove_meta() {
        let (store, _d) = tmp_store();
        store.upsert(CredMeta { cred_id: "x".into(), ..Default::default() }).unwrap();
        store.remove("x").unwrap();
        assert!(store.get("x").is_none());
    }

    #[test]
    fn history_is_newest_first_and_filterable() {
        let (store, _d) = tmp_store();
        for (i, id) in [("1", "a"), ("2", "b"), ("3", "a")] {
            store
                .append_history(&HistoryRecord {
                    ts: i.into(), cred_id: id.into(), name: None, email: None, tier_id: None,
                    daily_limit: None, daily_spent: None, daily_remaining: None,
                    tokens_7d: None, streak_current: None, ok: true,
                })
                .unwrap();
        }
        let all = store.history(None, 10).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].ts, "3", "最新记录必须排在最前");
        let only_a = store.history(Some("a"), 10).unwrap();
        assert_eq!(only_a.len(), 2);
        assert!(only_a.iter().all(|r| r.cred_id == "a"));
    }

    #[test]
    fn history_limit_is_clamped() {
        let (store, _d) = tmp_store();
        store
            .append_history(&HistoryRecord {
                ts: "1".into(), cred_id: "a".into(), name: None, email: None, tier_id: None,
                daily_limit: None, daily_spent: None, daily_remaining: None,
                tokens_7d: None, streak_current: None, ok: true,
            })
            .unwrap();
        assert_eq!(store.history(None, 0).unwrap().len(), 1, "limit 0 应被夹到 1");
    }
}

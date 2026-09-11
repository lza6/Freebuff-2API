//! Web 协议桥接用的「凭证 → 上游会话」映射。
//!
//! 为什么需要它：上游的免费额度是按**会话（thread）**给的吗？不是——是按每个模型的每日
//! **admission（会话准入）**给（`rateLimitsByModel[m].limit` 默认 6/天，`recentCount` 计已用）。
//! 在同一个 thread 里连续对话不会消耗新的 admission；**每请求新建 thread 会迅速烧光每日额度**。
//!
//! 因此把 OpenAI/Anthropic 客户端接进来的桥接层必须**尽量复用同一个上游 thread**：
//! 首轮发完整上下文开新会话，后续轮次只发最新一条用户消息、复用同一个 thread。
//!
//! 落盘：`data/web_threads.json`（`{ "<cred_id>": { "thread_id": "...", "last_user_text": "..." } }`）。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// 某条凭证当前使用中的上游会话
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThreadBinding {
    #[serde(default)]
    pub thread_id: String,
    /// 上一次发出去的用户文本；用于判断客户端的这次请求是"新一轮"还是"重试同一轮"
    #[serde(default)]
    pub last_user_text: String,
    #[serde(default)]
    pub turns: u64,
}

pub struct WebThreadMap {
    path: PathBuf,
    cache: Mutex<HashMap<String, ThreadBinding>>,
}

impl WebThreadMap {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let cache = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<HashMap<String, ThreadBinding>>(&t).ok())
            .unwrap_or_default();
        Self { path, cache: Mutex::new(cache) }
    }

    pub fn get(&self, cred_id: &str) -> Option<ThreadBinding> {
        self.cache.lock().ok().and_then(|c| c.get(cred_id).cloned())
    }

    /// 记下一个会话绑定（首轮拿到 threadId，或后续轮次刷新 last_user_text）
    /// 记下一个会话绑定（首轮拿到 threadId，或后续轮次刷新 last_user_text）。
    /// 快照 clone 与更新在**同一临界区**内完成，消除"锁外 clone 旧快照后写盘覆盖新绑定"的窗口。
    pub fn bind(&self, cred_id: &str, thread_id: &str, last_user_text: &str) -> Result<()> {
        let json = {
            let mut c = self.cache.lock().map_err(|_| anyhow::anyhow!("web_threads 锁中毒"))?;
            // turns 统计的是"文本变化次数-1"（首次 bind 是初始轮，不算续聊）
            let was_new = c.get(cred_id).is_none();
            let e = c.entry(cred_id.to_string()).or_default();
            e.thread_id = thread_id.to_string();
            if !was_new && e.last_user_text != last_user_text {
                e.turns = e.turns.saturating_add(1);
            }
            e.last_user_text = last_user_text.to_string();
            serde_json::to_string_pretty(&*c)?
        };
        self.atomic_write(&json)
    }

    /// 会话失效（上游已删/404/错误）时清掉绑定，下次会重新开一个
    pub fn clear(&self, cred_id: &str) -> Result<()> {
        let json = {
            let mut c = self.cache.lock().map_err(|_| anyhow::anyhow!("web_threads 锁中毒"))?;
            c.remove(cred_id);
            serde_json::to_string_pretty(&*c)?
        };
        self.atomic_write(&json)
    }

    /// 原子替换写（临时文件 + rename），防写入中途崩溃留下截断文件
    fn atomic_write(&self, json: &str) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        #[cfg(windows)]
        if self.path.exists() {
            let _ = std::fs::remove_file(&self.path);
        }
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// 把 OpenAI 风格的 `messages` 摊平成上游 web 协议要的单个 `content` 字符串。
///
/// 返回 `(prompt, 最后一条用户消息)`：后者用于判断能否复用上游 thread 只发增量。
/// `only_last_user=true` 时只取最后一条用户消息（用于复用会话的续聊轮次）。
pub fn flatten_messages(messages: &serde_json::Value, only_last_user: bool) -> Option<(String, String)> {
    let arr = messages.as_array()?;
    let text_of = |m: &serde_json::Value| -> Option<String> {
        let t = match m.get("content")? {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(items) => items
                .iter()
                .filter_map(|it| {
                    // 兼容 Anthropic 风格 content blocks（text 字段）与 OpenAI 多模态（type=text）
                    it.get("text").and_then(|x| x.as_str()).map(String::from)
                })
                .collect::<Vec<_>>()
                .join("\n"),
            _ => return None,
        };
        let t = t.trim().to_string();
        if t.is_empty() { None } else { Some(t) }
    };

    let last_user = arr
        .iter()
        .rev()
        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
        .and_then(&text_of)?;

    if only_last_user {
        return Some((last_user.clone(), last_user));
    }

    let mut parts: Vec<String> = Vec::new();
    let mut systems: Vec<String> = Vec::new();
    let non_system: Vec<&serde_json::Value> = arr
        .iter()
        .filter(|m| {
            let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            if role == "system" || role == "developer" {
                if let Some(t) = text_of(m) {
                    systems.push(t);
                }
                false
            } else {
                true
            }
        })
        .collect();

    if let Some(sys) = systems.first() {
        parts.push(format!("[系统指令]\n{sys}"));
    }
    // 只有一条非系统消息（最常见：客户端只发当前这句）→ 不标题号，原样发
    let rest: Vec<String> = non_system.iter().filter_map(|m| text_of(m)).collect();
    if rest.len() == 1 {
        parts.push(rest[0].clone());
    } else {
        for m in &non_system {
            let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let Some(t) = text_of(m) else { continue };
            let label = match role {
                "assistant" => "[助手]",
                "tool" => "[工具结果]",
                _ => "[用户]",
            };
            parts.push(format!("{label}\n{t}"));
        }
    }
    let prompt = parts.join("\n\n");
    if prompt.trim().is_empty() { None } else { Some((prompt, last_user)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msgs(v: serde_json::Value) -> serde_json::Value { v }

    #[test]
    fn single_user_message_is_passed_verbatim() {
        let m = msgs(serde_json::json!([{ "role": "user", "content": "你好" }]));
        let (p, last) = flatten_messages(&m, false).unwrap();
        assert_eq!(p, "你好");
        assert_eq!(last, "你好");
    }

    #[test]
    fn system_prompt_and_single_user_keeps_system_prefix() {
        let m = msgs(serde_json::json!([
            { "role": "system", "content": "你是助手" },
            { "role": "user", "content": "写首诗" }
        ]));
        let (p, _) = flatten_messages(&m, false).unwrap();
        assert!(p.starts_with("[系统指令]\n你是助手"));
        assert!(p.ends_with("写首诗"));
    }

    #[test]
    fn multi_turn_is_labelled_transcript() {
        let m = msgs(serde_json::json!([
            { "role": "user", "content": "1+1" },
            { "role": "assistant", "content": "2" },
            { "role": "user", "content": "再加一" }
        ]));
        let (p, last) = flatten_messages(&m, false).unwrap();
        assert!(p.contains("[用户]\n1+1"));
        assert!(p.contains("[助手]\n2"));
        assert!(p.ends_with("[用户]\n再加一"));
        assert_eq!(last, "再加一", "最后一条用户消息必须能被单独取出（续聊只发它）");
    }

    #[test]
    fn only_last_user_returns_just_that() {
        let m = msgs(serde_json::json!([
            { "role": "user", "content": "旧问题" },
            { "role": "assistant", "content": "旧回答" },
            { "role": "user", "content": "新问题" }
        ]));
        let (p, last) = flatten_messages(&m, true).unwrap();
        assert_eq!(p, "新问题");
        assert_eq!(last, "新问题");
    }

    #[test]
    fn anthropic_style_blocks_are_supported() {
        let m = msgs(serde_json::json!([
            { "role": "user", "content": [{ "type": "text", "text": "块内容" }] }
        ]));
        assert_eq!(flatten_messages(&m, false).unwrap().0, "块内容");
    }

    #[test]
    fn empty_or_missing_content_yields_none() {
        assert!(flatten_messages(&serde_json::json!([]), false).is_none());
        assert!(flatten_messages(&serde_json::json!([{ "role": "assistant", "content": "只有助手" }]), false).is_none());
        assert!(flatten_messages(&serde_json::json!([{ "role": "user", "content": "   " }]), false).is_none());
    }

    #[test]
    fn map_persists_and_clears() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web_threads.json");
        let map = WebThreadMap::new(&path);
        map.bind("cred1", "t-1", "hello").unwrap();
        assert_eq!(map.get("cred1").unwrap().thread_id, "t-1");
        // 续聊同一轮不重复计 turn（同 thread + 同文本）
        map.bind("cred1", "t-1", "hello").unwrap();
        assert_eq!(map.get("cred1").unwrap().turns, 0);
        // 新文本（哪怕 thread 相同）算新的一轮
        map.bind("cred1", "t-1", "next").unwrap();
        assert_eq!(map.get("cred1").unwrap().turns, 1, "文本变化必须计为新的一轮");

        let reopened = WebThreadMap::new(&path);
        assert_eq!(reopened.get("cred1").unwrap().last_user_text, "next");

        reopened.clear("cred1").unwrap();
        assert!(reopened.get("cred1").is_none());
    }
}

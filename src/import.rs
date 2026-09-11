//! Token 导入：解析 curl 命令 / HAR 文件，自动提取 Freebuff 鉴权 token 并入库
//!
//! 场景：用户在浏览器 DevTools 里把请求"Copy as cURL"或导出 HAR，粘到本网关
//! 面板 → 自动提取 `authorization: Bearer <token>`（仅限 freebuff.com / codebuff.com
//! 域名的请求）→ 追加到账号池并持久化。

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const TARGET_HOSTS: &[&str] = &["freebuff.com", "codebuff.com", "www.codebuff.com"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedAuth {
    /// Bearer token（或回退为 session cookie 值）
    pub token: String,
    /// 提取来源：curl / har
    pub source: String,
    /// 关联请求的 host
    pub host: String,
    /// 关联请求的 path
    pub path: String,
    /// 关联请求的方法
    pub method: String,
    /// 入库时间（RFC3339；旧数据无此字段时为 None）
    #[serde(default)]
    pub added_at: Option<String>,
}

/// 从 curl 命令文本提取 Bearer token（仅限 freebuff/codebuff 目标域，防跨域凭据误导入）
pub fn parse_curl(text: &str) -> Vec<ExtractedAuth> {
    let mut out = Vec::new();
    // 提取所有 header：支持 `-H "name: value"`、`-H 'name: value'`（Chrome 复制格式）与 cmd 转义 `^"`
    let header_re =
        regex::Regex::new(r#"-H\s*\^?['"](?:authorization|Authorization):\s*Bearer\s+([A-Za-z0-9._-]+)"#).unwrap();
    // URL 提取：支持 `curl 'URL'` / `curl "URL"` / `curl --url "URL"` / `curl -url "URL"`
    let url_re = regex::Regex::new(r#"curl\s+(?:--?url\s+)?\^?['"]?(https?://[^\s'"^]+)"#).unwrap();
    let method_re = regex::Regex::new(r#"(?:-X\s+|--request\s+)\^?([A-Z]+)"#).unwrap();

    let tokens: HashSet<String> = header_re.captures_iter(text).map(|c| c[1].to_string()).collect();
    if tokens.is_empty() {
        return out;
    }
    let url = url_re
        .captures(text)
        .map(|c| c[1].to_string())
        .unwrap_or_default();
    let host = if url.is_empty() { String::new() } else { parse_host(&url) };
    let path = if url.is_empty() { String::new() } else { parse_path(&url) };
    let method = method_re.captures(text).map(|c| c[1].to_string()).unwrap_or_else(|| "GET".into());
    // curl 文本无法可靠定位 host 时空缺放行（单机自用场景），但 host 明确为其他域时拒绝
    let host_is_other_domain = !host.is_empty() && !TARGET_HOSTS.iter().any(|h| host.ends_with(h));
    if host_is_other_domain {
        tracing::warn!("curl 导入拒绝：host={host} 不属于目标域名 {TARGET_HOSTS:?}");
        return out;
    }

    for t in tokens {
        out.push(ExtractedAuth {
            token: t,
            source: "curl".into(),
            host: host.clone(),
            path: path.clone(),
            method: method.clone(),
            added_at: None, // persist 时补写入库时间
        });
    }
    out
}

#[derive(Debug, Default, Deserialize)]
struct HarRoot {
    #[serde(default)]
    log: HarLog,
}

#[derive(Debug, Default, Deserialize)]
struct HarLog {
    #[serde(default)]
    entries: Vec<HarEntry>,
}

#[derive(Debug, Default, Deserialize)]
struct HarEntry {
    #[serde(default)]
    request: HarRequest,
}

#[derive(Debug, Default, Deserialize)]
struct HarRequest {
    #[serde(default)]
    method: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    headers: Vec<HarHeader>,
}

#[derive(Debug, Default, Deserialize)]
struct HarHeader {
    name: String,
    value: String,
}

/// 从 HAR JSON 提取 Bearer token（仅收录 freebuff/codebuff 目标域的请求）
pub fn parse_har(json_text: &str) -> Result<Vec<ExtractedAuth>> {
    let har: HarRoot = serde_json::from_str(json_text)?;
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for entry in har.log.entries {
        let url_lower = entry.request.url.to_lowercase();
        // 域名校验：非目标域的凭据一律跳过（防跨域 token 混淆）
        let is_target = TARGET_HOSTS.iter().any(|h| url_lower.contains(h));
        if !is_target {
            continue;
        }
        for h in &entry.request.headers {
            if h.name.eq_ignore_ascii_case("authorization") {
                if let Some(token) = h.value.trim().strip_prefix("Bearer ").or_else(|| h.value.trim().strip_prefix("bearer ")) {
                    let token = token.trim().to_string();
                    if token.len() >= 8 && !seen.contains(&token) {
                        seen.insert(token.clone());
                        out.push(ExtractedAuth {
                            token,
                            source: "har".into(),
                            host: parse_host(&entry.request.url),
                            path: parse_path(&entry.request.url),
                            method: entry.request.method.clone(),
                            added_at: None,
                        });
                    }
                }
            }
        }
    }
    Ok(out)
}

/// 凭证稳定标识：FNV-1a 64 位（纯本地计算，零依赖，跨 Rust 版本结果稳定）。
///
/// 不用 `DefaultHasher`——其输出 Rust 文档明确不保证跨版本/跨进程稳定，
/// 而该 id 要落盘作为凭证的持久主键，必须可重现。
pub fn cred_id(token: &str) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for b in token.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(PRIME);
    }
    // 混入长度，进一步降低不同长度同哈希的碰撞概率
    format!("{:016x}{:04x}", h, token.len().min(0xffff))
}

/// 凭证类型：web 版 Cookie 还是 Bearer token。
pub fn kind_of(token: &str) -> &'static str {
    if token.contains("session-token") {
        "web-cookie"
    } else {
        "bearer"
    }
}

fn parse_host(url: &str) -> String {
    url.split("://").nth(1).and_then(|rest| rest.split('/').next()).unwrap_or("").to_string()
}

fn parse_path(url: &str) -> String {
    // 修正：先定位 "://" 之后的部分，再取其内第一个 '/' 起的路径
    let start = url.find("://").map(|p| p + 3).unwrap_or(0);
    let rest = &url[start..];
    rest.find('/').map(|i| rest[i..].to_string()).unwrap_or_default()
}

/// 凭证文件写锁：persist/delete/heal 都是"读-改-写整文件"，
/// 并发时会互相覆盖（后写赢、先写丢），必须串行化。进程内锁足够（单进程软件）。
static TOKENS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 原子写：先写临时文件再 rename，避免写入中途崩溃留下截断的 tokens.json
/// （截断 = load_tokens 解析失败 = 全部凭证不可用）。
fn atomic_write(path: &str, json: &str) -> Result<()> {
    let p = std::path::Path::new(path);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    // rename 覆盖已有目标在 Windows 上要求目标不存在，先移除
    #[cfg(windows)]
    if p.exists() {
        let _ = std::fs::remove_file(p);
    }
    std::fs::rename(&tmp, p)?;
    Ok(())
}

/// 持久化：追加到 data/tokens.json（dedupe）
pub fn persist_tokens(path: &str, new_tokens: &[ExtractedAuth]) -> Result<Vec<ExtractedAuth>> {
    let _g = TOKENS_LOCK.lock().map_err(|_| anyhow::anyhow!("tokens 锁中毒"))?;
    let existing = load_tokens(path)?;
    let mut all: Vec<ExtractedAuth> = existing;
    let existing_set: HashSet<String> = all.iter().map(|t| t.token.clone()).collect();
    let now = chrono::Utc::now().to_rfc3339();
    let mut added = Vec::new();
    for t in new_tokens {
        if !existing_set.contains(&t.token) {
            let mut item = t.clone();
            if item.added_at.is_none() {
                item.added_at = Some(now.clone());
            }
            all.push(item.clone());
            added.push(item);
        }
    }
    let json = serde_json::to_string_pretty(&all)?;
    atomic_write(path, &json)?;
    Ok(added)
}

/// 读取已持久化 token
pub fn load_tokens(path: &str) -> Result<Vec<ExtractedAuth>> {
    if !std::path::Path::new(path).exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path)?;
    let parsed: Vec<ExtractedAuth> = serde_json::from_str(&text)?;
    Ok(parsed)
}

/// 读取 token 并**修复历史数据**：早于 `added_at` 字段引入的凭证没有入库时间，
/// 面板上只能显示"—"，用户看不到"什么时候入的"。这里用文件修改时间回填并落盘（幂等）。
pub fn load_tokens_healed(path: &str) -> Result<Vec<ExtractedAuth>> {
    let mut tokens = load_tokens(path)?;
    if tokens.iter().all(|t| t.added_at.is_some()) {
        return Ok(tokens);
    }
    // 回填来源：文件 mtime（最接近"首次入库"的可信时间）；取不到则用当前时间。
    // 注意：多条历史凭证会得到同一个回填值（≈ 最后一次文件修改时间），这是可接受的下限保证。
    let fallback = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(chrono::DateTime::<chrono::Utc>::from)
        .map(|d| d.to_rfc3339())
        .unwrap_or_else(|_| chrono::Utc::now().to_rfc3339());
    let mut changed = false;
    for t in tokens.iter_mut() {
        if t.added_at.is_none() {
            t.added_at = Some(fallback.clone());
            changed = true;
        }
    }
    if changed {
        let json = serde_json::to_string_pretty(&tokens)?;
        let _g = TOKENS_LOCK.lock().map_err(|_| anyhow::anyhow!("tokens 锁中毒"))?;
        atomic_write(path, &json)?;
        tracing::info!("已为历史凭证回填入库时间（来源：tokens.json mtime）");
    }
    Ok(tokens)
}

/// 按稳定 id 删除一条凭证；返回是否删除成功。
pub fn delete_token(path: &str, id: &str) -> Result<Option<ExtractedAuth>> {
    let _g = TOKENS_LOCK.lock().map_err(|_| anyhow::anyhow!("tokens 锁中毒"))?;
    let mut tokens = load_tokens(path)?;
    let before = tokens.len();
    let removed = tokens.iter().find(|t| cred_id(&t.token) == id).cloned();
    tokens.retain(|t| cred_id(&t.token) != id);
    if tokens.len() == before {
        return Ok(None);
    }
    let json = serde_json::to_string_pretty(&tokens)?;
    atomic_write(path, &json)?;
    Ok(removed)
}

/// 从任意文本自动嗅探：优先按 curl，再按 HAR，再按 Cookie 串，最后按裸 "Bearer xxx"
pub fn sniff_tokens(text: &str) -> Result<Vec<ExtractedAuth>> {
    if text.contains("curl") || text.contains("--url") {
        let v = parse_curl(text);
        if !v.is_empty() {
            return Ok(v);
        }
    }
    if text.trim_start().starts_with('{') {
        if let Ok(v) = parse_har(text) {
            if !v.is_empty() {
                return Ok(v);
            }
        }
    }
    // 完整 Cookie 串（含 __Secure-next-auth.session-token 等）
    if let Some(v) = parse_cookie(text) {
        return Ok(v);
    }
    // 裸 Bearer token
    let re = regex::Regex::new(r"(?i)bearer\s+([A-Za-z0-9._-]{16,})").unwrap();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for cap in re.captures_iter(text) {
        let t = cap[1].to_string();
        if !seen.contains(&t) {
            seen.insert(t.clone());
            out.push(ExtractedAuth {
                token: t,
                source: "raw".into(),
                host: String::new(),
                path: String::new(),
                method: String::new(),
                added_at: None,
            });
        }
    }
    if out.is_empty() {
        Err(anyhow!("未能从输入中提取到任何 Bearer token 或 Cookie"))
    } else {
        Ok(out)
    }
}

/// 解析完整 Cookie 串（web 版鉴权凭证）：提取包含 session-token 的 Cookie 整体
pub fn parse_cookie(text: &str) -> Option<Vec<ExtractedAuth>> {
    // 提取第一个含 "__Secure-next-auth.session-token=" 的 Cookie 片段（可能是完整串或部分）
    let re = regex::Regex::new(r#"__Secure-next-auth\.session-token=[^; \t"']+"#).unwrap();
    let csrf_re = regex::Regex::new(r#"__Host-next-auth\.csrf-token=[^; \t"']+"#).unwrap();
    let cb_re = regex::Regex::new(r#"__Secure-next-auth\.callback-url=[^; \t"']+"#).unwrap();

    let st = re.find(text)?;
    let token = st.as_str().to_string();
    // 组完整 Cookie 串：session-token + csrf + callback
    let mut parts = vec![token.clone()];
    if let Some(c) = csrf_re.find(text) {
        parts.push(c.as_str().to_string());
    }
    if let Some(c) = cb_re.find(text) {
        parts.push(c.as_str().to_string());
    }
    let cookie = parts.join("; ");
    Some(vec![ExtractedAuth {
        token: cookie,
        source: "cookie".into(),
        host: "freebuff.com".into(),
        path: "/api/web/freebuff-session".into(),
        method: "GET".into(),
        added_at: None,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curl_with_bearer() {
        let curl = r#"
curl --url "https://www.codebuff.com/api/v1/freebuff/session" \
  -H "accept: application/json" \
  -H "authorization: Bearer fa82b5c1-e39d-4c7a-961f-d2b3c4e5f6a7" \
  -X GET
"#;
        let out = parse_curl(curl);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].token, "fa82b5c1-e39d-4c7a-961f-d2b3c4e5f6a7");
        assert_eq!(out[0].method, "GET");
    }

    #[test]
    fn har_with_bearer() {
        let har = r#"{
  "log": {
    "entries": [{
      "request": {
        "method": "POST",
        "url": "https://www.codebuff.com/api/v1/freebuff/session",
        "headers": [
          {"name": "authorization", "value": "Bearer tok_har_test_123456"}
        ]
      }
    }]
  }
}"#;
        let out = parse_har(har).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].token, "tok_har_test_123456");
        assert_eq!(out[0].host, "www.codebuff.com");
    }

    #[test]
    fn sniff_raw_bearer() {
        let out = sniff_tokens("Authorization: Bearer abcdefghijklmnop1234567890").unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, "raw");
    }

    #[test]
    fn sniff_cookie() {
        let out = sniff_tokens("__Secure-next-auth.session-token=abc123; __Host-next-auth.csrf-token=xyz").unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, "cookie");
        assert!(out[0].token.contains("session-token"));
    }

    #[test]
    fn persist_dedupes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens.json");
        let path_str = path.to_str().unwrap().to_string();
        let t1 = ExtractedAuth { token: "t1".into(), source: "test".into(), host: "h".into(), path: "p".into(), method: "GET".into(), added_at: None };
        let t2 = ExtractedAuth { token: "t2".into(), source: "test".into(), host: "h".into(), path: "p".into(), method: "GET".into(), added_at: None };
        let added1 = persist_tokens(&path_str, &[t1.clone(), t2.clone()]).unwrap();
        assert_eq!(added1.len(), 2);
        // 再次写入含 t1 应跳过
        let added2 = persist_tokens(&path_str, &[t1, t2]).unwrap();
        assert_eq!(added2.len(), 0);
        assert_eq!(load_tokens(&path_str).unwrap().len(), 2);
    }

    #[test]
    fn curl_cross_domain_rejected() {
        // 非目标域名的 curl 不得导入（防跨域凭据混淆）
        let curl = r#"
curl --url "https://evil.example.com/api/steal" \
  -H "authorization: Bearer crossdomain1234567890" \
  -X GET
"#;
        let out = parse_curl(curl);
        assert!(out.is_empty(), "跨域 token 应被拒绝，实际导入 {} 个", out.len());
    }

    #[test]
    fn chrome_style_curl_imports_with_host() {
        // Chrome DevTools「Copy as cURL」格式：单引号 + URL 位置参数 + 反斜杠换行
        let curl = r#"curl 'https://www.codebuff.com/api/v1/chat/completions' \
  -H 'authorization: Bearer chromestyle1234567890' \
  -X POST"#;
        let out = parse_curl(curl);
        assert_eq!(out.len(), 1, "Chrome 格式应提取 1 个 token");
        assert_eq!(out[0].host, "www.codebuff.com", "host 必须被正确解析（否则跨域校验失效）");
        assert_eq!(out[0].path, "/api/v1/chat/completions");
        assert_eq!(out[0].method, "POST");
    }

    #[test]
    fn chrome_style_cross_domain_rejected() {
        // 单引号格式下的跨域 token 同样必须被拒绝
        let curl = "curl 'https://evil.example.com/steal' -H 'authorization: Bearer chromecross1234567890'";
        let out = parse_curl(curl);
        assert!(out.is_empty(), "单引号格式跨域 token 应被拒绝，实际 {} 个", out.len());
    }

    #[test]
    fn har_cross_domain_rejected() {
        // HAR 中非目标域条目应跳过
        let har = r#"{
  "log": {
    "entries": [{
      "request": {
        "method": "GET",
        "url": "https://evil.example.com/api/x",
        "headers": [
          {"name": "authorization", "value": "Bearer evilhar123456789"}
        ]
      }
    }]
  }
}"#;
        let out = parse_har(har).unwrap();
        assert!(out.is_empty(), "跨域 HAR token 应被拒绝");
    }

    #[test]
    fn cred_id_is_stable_and_distinct() {
        // 同一 token 必须每次得到同一 id（id 是落盘主键，不稳定会导致凭证列表错乱）
        let a1 = cred_id("__Secure-next-auth.session-token=abc");
        let a2 = cred_id("__Secure-next-auth.session-token=abc");
        assert_eq!(a1, a2);
        assert_ne!(a1, cred_id("__Secure-next-auth.session-token=abd"));
        // 长度不同但前缀相同也必须区分
        assert_ne!(cred_id("tok"), cred_id("tokx"));
    }

    #[test]
    fn kind_detects_web_cookie() {
        assert_eq!(kind_of("__Secure-next-auth.session-token=x; y=1"), "web-cookie");
        assert_eq!(kind_of("sk-abcdef"), "bearer");
    }

    #[test]
    fn healed_load_backfills_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens.json");
        let path_str = path.to_str().unwrap().to_string();
        // 模拟早期版本落盘的数据：没有 added_at 字段
        std::fs::write(
            &path,
            r#"[{"token":"legacy-token","source":"cookie","host":"freebuff.com","path":"/p","method":"GET"}]"#,
        )
        .unwrap();

        let healed = load_tokens_healed(&path_str).unwrap();
        assert_eq!(healed.len(), 1);
        assert!(healed[0].added_at.is_some(), "历史凭证必须被回填入库时间");
        // 已落盘（再次读取不再需要回填，且值保持稳定）
        let again = load_tokens_healed(&path_str).unwrap();
        assert_eq!(again[0].added_at, healed[0].added_at);
    }

    #[test]
    fn healed_load_is_noop_when_complete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens.json");
        let path_str = path.to_str().unwrap().to_string();
        let raw = r#"[{"token":"t","source":"cookie","host":"h","path":"p","method":"GET","added_at":"2026-01-01T00:00:00+00:00"}]"#;
        std::fs::write(&path, raw).unwrap();
        let out = load_tokens_healed(&path_str).unwrap();
        assert_eq!(out[0].added_at.as_deref(), Some("2026-01-01T00:00:00+00:00"));
        // 文件内容不应被改写
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    }

    #[test]
    fn delete_token_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens.json");
        let path_str = path.to_str().unwrap().to_string();
        let t1 = ExtractedAuth { token: "del-me".into(), source: "cookie".into(), host: "h".into(), path: "p".into(), method: "GET".into(), added_at: None };
        let t2 = ExtractedAuth { token: "keep-me".into(), source: "cookie".into(), host: "h".into(), path: "p".into(), method: "GET".into(), added_at: None };
        persist_tokens(&path_str, &[t1, t2]).unwrap();

        let removed = delete_token(&path_str, &cred_id("del-me")).unwrap();
        assert!(removed.is_some(), "应按 id 删除成功");
        let left = load_tokens(&path_str).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].token, "keep-me");
        // 再删同一条 → None（幂等）
        assert!(delete_token(&path_str, &cred_id("del-me")).unwrap().is_none());
    }
}

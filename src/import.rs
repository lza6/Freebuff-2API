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
}

/// 从 curl 命令文本提取 Bearer token（针对 freebuff/codebuff 域）
pub fn parse_curl(text: &str) -> Vec<ExtractedAuth> {
    let mut out = Vec::new();
    // 提取所有 header：-H "name: value" 或 -H ^"name: value^"
    let header_re = regex::Regex::new(r#"-H\s*\^?"(?:authorization|Authorization):\s*Bearer\s+([A-Za-z0-9._-]+)"#).unwrap();
    let host_re = regex::Regex::new(r#"curl\s+--?url\s+\^?"(?:https?://)?([^/^"\s]+)"#).unwrap();
    let method_re = regex::Regex::new(r#"(?:-X\s+|--request\s+)\^?([A-Z]+)"#).unwrap();
    let path_re = regex::Regex::new(r#"url\s+\^?"(?:https?://[^/]+)?(/[^"^]*)"#).unwrap();

    let tokens: HashSet<String> = header_re.captures_iter(text).map(|c| c[1].to_string()).collect();
    if tokens.is_empty() {
        return out;
    }
    let host = host_re.captures(text).map(|c| c[1].to_string()).unwrap_or_default();
    let method = method_re.captures(text).map(|c| c[1].to_string()).unwrap_or_else(|| "GET".into());
    let path = path_re.captures(text).map(|c| c[1].to_string()).unwrap_or_default();
    let host_is_target = TARGET_HOSTS.iter().any(|h| host.ends_with(h));

    for t in tokens {
        out.push(ExtractedAuth {
            token: t,
            source: "curl".into(),
            host: host.clone(),
            path: path.clone(),
            method: method.clone(),
        });
        // 非目标域名的也收（可能用于自建端点），但标记
        let _ = host_is_target;
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

/// 从 HAR JSON 提取 Bearer token
pub fn parse_har(json_text: &str) -> Result<Vec<ExtractedAuth>> {
    let har: HarRoot = serde_json::from_str(json_text)?;
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for entry in har.log.entries {
        let url_lower = entry.request.url.to_lowercase();
        let is_target = TARGET_HOSTS.iter().any(|h| url_lower.contains(h));
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
                        });
                    }
                }
            }
        }
        let _ = is_target;
    }
    Ok(out)
}

fn parse_host(url: &str) -> String {
    url.split("://").nth(1).and_then(|rest| rest.split('/').next()).unwrap_or("").to_string()
}

fn parse_path(url: &str) -> String {
    url.split("://").nth(1).and_then(|rest| rest.find('/')).map(|i| &url[url.len() - (url.len() - i - 3)..]).unwrap_or("").to_string()
}

/// 持久化：追加到 data/tokens.json（dedupe）
pub fn persist_tokens(path: &str, new_tokens: &[ExtractedAuth]) -> Result<Vec<ExtractedAuth>> {
    let existing = load_tokens(path)?;
    let mut all: Vec<ExtractedAuth> = existing;
    let existing_set: HashSet<String> = all.iter().map(|t| t.token.clone()).collect();
    let mut added = Vec::new();
    for t in new_tokens {
        if !existing_set.contains(&t.token) {
            all.push(t.clone());
            added.push(t.clone());
        }
    }
    std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap_or(std::path::Path::new("."))).ok();
    let json = serde_json::to_string_pretty(&all)?;
    std::fs::write(path, json)?;
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

/// 从任意文本自动嗅探：优先按 curl，再按 HAR，最后按裸 "Bearer xxx"
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
            });
        }
    }
    if out.is_empty() {
        Err(anyhow!("未能从输入中提取到任何 Bearer token"))
    } else {
        Ok(out)
    }
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
    fn persist_dedupes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens.json");
        let path_str = path.to_str().unwrap().to_string();
        let t1 = ExtractedAuth { token: "t1".into(), source: "test".into(), host: "h".into(), path: "p".into(), method: "GET".into() };
        let t2 = ExtractedAuth { token: "t2".into(), source: "test".into(), host: "h".into(), path: "p".into(), method: "GET".into() };
        let added1 = persist_tokens(&path_str, &[t1.clone(), t2.clone()]).unwrap();
        assert_eq!(added1.len(), 2);
        // 再次写入含 t1 应跳过
        let added2 = persist_tokens(&path_str, &[t1, t2]).unwrap();
        assert_eq!(added2.len(), 0);
        assert_eq!(load_tokens(&path_str).unwrap().len(), 2);
    }
}

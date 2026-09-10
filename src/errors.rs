//! 上游错误规则表：文本优先、状态码兜底
//!
//! Codebuff 上游会在 HTTP 200 的响应体里返回文本错误码
//! （如 `free_mode_invalid_agent_model`、`waiting_room_queued`），
//! 也会在 4xx/5xx 错误体或 SSE 片段里返回描述文本，仅按状态码分类会漏判。
//! 本模块以响应体文本为第一判据（大小写不敏感的包含匹配 + 少量正则），
//! HTTP 状态码仅作兜底。
//!
//! 错误种类与 `retry::FailureKind` 语义对齐（本模块更细），并额外给出
//! 重试提示：[`ErrorKind::is_retryable`] / [`ErrorKind::retry_after_hint`]。

use std::sync::OnceLock;

use regex::Regex;

/// 错误摘要最大字符数，超出时截断并追加 `...`
const EXCERPT_MAX_CHARS: usize = 300;

/// RateLimit 建议退避秒数
const RATE_LIMIT_RETRY_AFTER_SECS: u64 = 60;
/// WaitingRoom 建议退避秒数（免费队列通常很快放行）
const WAITING_ROOM_RETRY_AFTER_SECS: u64 = 15;
/// Upstream5xx 建议退避秒数（短等重试）
const UPSTREAM_RETRY_AFTER_SECS: u64 = 5;

/// 排队强关键词：命中即判 WaitingRoom（200 响应体也会出现）
const WAITING_ROOM_STRONG: &[&str] = &["waiting_room", "waiting room", "排队"];
/// 排队弱关键词：仅在 429/503 时判定，避免误伤普通 200 响应
const WAITING_ROOM_WEAK: &[&str] = &["queue"];
/// 限流关键词
const RATE_LIMIT_HINTS: &[&str] = &["rate limit", "rate_limit", "too many requests", "限流"];
/// 模型不可用关键词（收窄：避免 503 "Service Unavailable" 被误判为模型问题）
const MODEL_UNAVAILABLE_HINTS: &[&str] = &[
    "invalid_agent_model",
    "free_mode_invalid",
    "model not available",
    "only available for",
    "model_not_found",
    "no such model",
];
/// 凭证失效关键词
const AUTH_HINTS: &[&str] = &[
    "unauthorized",
    "invalid token",
    "session expired",
    "token expired",
    "authentication",
];
/// 请求错误关键词
const BAD_REQUEST_HINTS: &[&str] = &["invalid_request", "bad request"];

/// 归一化后的错误种类（与 `retry::FailureKind` 语义对齐但更细）
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// 限流（可退避重试）
    RateLimit,
    /// 免费队列排队（应回 Retry-After）
    WaitingRoom,
    /// 凭证失效（应冷却换号）
    AuthExpired,
    /// 模型不可用（应换模型）
    ModelUnavailable,
    /// 请求问题（不重试）
    BadRequest,
    /// 上游服务端错误（短等重试）
    #[serde(rename = "upstream_5xx")]
    Upstream5xx,
    /// 网络/超时
    Network,
    /// 无法归类
    Unknown,
}

impl ErrorKind {
    /// 稳定字符串标识（用于日志与遥测 error_kind，与 serde 输出一致）
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::RateLimit => "rate_limit",
            ErrorKind::WaitingRoom => "waiting_room",
            ErrorKind::AuthExpired => "auth_expired",
            ErrorKind::ModelUnavailable => "model_unavailable",
            ErrorKind::BadRequest => "bad_request",
            ErrorKind::Upstream5xx => "upstream_5xx",
            ErrorKind::Network => "network",
            ErrorKind::Unknown => "unknown",
        }
    }

    /// 是否适合退避后重试
    pub fn is_retryable(self) -> bool {
        matches!(
            self,
            ErrorKind::RateLimit
                | ErrorKind::WaitingRoom
                | ErrorKind::Upstream5xx
                | ErrorKind::Network
        )
    }

    /// 建议退避秒数（未给出可靠提示时返回 `None`）
    pub fn retry_after_hint(self) -> Option<u64> {
        match self {
            ErrorKind::RateLimit => Some(RATE_LIMIT_RETRY_AFTER_SECS),
            ErrorKind::WaitingRoom => Some(WAITING_ROOM_RETRY_AFTER_SECS),
            ErrorKind::Upstream5xx => Some(UPSTREAM_RETRY_AFTER_SECS),
            _ => None,
        }
    }
}

impl std::fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 权威判定：文本规则优先 → 状态码兜底
///
/// - `body` 可以是上游错误体全文，也可以是 SSE 流中的错误片段；
///   匹配前统一转小写，故关键词大小写不敏感。
/// - `status` 为 HTTP 状态码，`0` 表示网络错误（无响应）。
pub fn classify(status: u16, body: &str) -> ErrorKind {
    let lowered = body.to_lowercase();
    if let Some(kind) = classify_text(&lowered, status) {
        return kind;
    }
    classify_status(status)
}

/// 从错误体/响应头提取 Retry-After（秒）
///
/// 响应头优先；头部缺失或非法时回退到 body 中的
/// `"retryAfter":30` / `"retry_after": 30` / `"retry-after":"7"` 字段。
pub fn extract_retry_after(body: &str, headers_retry_after: Option<&str>) -> Option<u64> {
    if let Some(secs) = headers_retry_after.and_then(parse_retry_after_secs) {
        return Some(secs);
    }
    retry_after_re()
        .and_then(|re| re.captures(body))
        .and_then(|caps| caps.get(1))
        .and_then(|m| m.as_str().parse::<u64>().ok())
}

/// 从上游错误体提取人类可读摘要（截断 [`EXCERPT_MAX_CHARS`] 字符）
///
/// 优先取 JSON 的 `message` / `detail` / `error_description` / `error` 字段，
/// 非 JSON（含 SSE 片段）时用正则找 `"message":"..."` 样式字段，
/// 都失败则返回原文；连续空白折叠为单个空格。
pub fn error_excerpt(body: &str) -> String {
    let text = extract_message(body).unwrap_or_else(|| body.to_string());
    let normalized = collapse_whitespace(&text);
    truncate_chars(&normalized, EXCERPT_MAX_CHARS)
}

/// 文本规则判定；无命中返回 `None` 交由状态码兜底
fn classify_text(lowered: &str, status: u16) -> Option<ErrorKind> {
    if contains_any(lowered, WAITING_ROOM_STRONG)
        || (matches!(status, 429 | 503) && contains_any(lowered, WAITING_ROOM_WEAK))
    {
        return Some(ErrorKind::WaitingRoom);
    }
    if contains_any(lowered, RATE_LIMIT_HINTS) {
        return Some(ErrorKind::RateLimit);
    }
    if contains_any(lowered, MODEL_UNAVAILABLE_HINTS) {
        return Some(ErrorKind::ModelUnavailable);
    }
    if contains_any(lowered, AUTH_HINTS) {
        return Some(ErrorKind::AuthExpired);
    }
    if contains_any(lowered, BAD_REQUEST_HINTS)
        || missing_required_re().is_some_and(|re| re.is_match(lowered))
    {
        return Some(ErrorKind::BadRequest);
    }
    None
}

/// 状态码兜底
fn classify_status(status: u16) -> ErrorKind {
    match status {
        0 => ErrorKind::Network,
        429 => ErrorKind::RateLimit,
        401 | 403 => ErrorKind::AuthExpired,
        404 => ErrorKind::ModelUnavailable,
        400 | 422 => ErrorKind::BadRequest,
        s if (500..600).contains(&s) => ErrorKind::Upstream5xx,
        _ => ErrorKind::Unknown,
    }
}

/// 解析 Retry-After 响应头（仅支持秒数；HTTP-date 形式返回 `None`）
fn parse_retry_after_secs(raw: &str) -> Option<u64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<u64>().ok()
}

/// 提取消息字段：完整 JSON → 片段正则兜底
fn extract_message(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(message) = message_from_value(&value) {
            return Some(message);
        }
    }
    message_from_fragment(trimmed)
}

/// 从 JSON 值中递归取第一个非空消息字段
fn message_from_value(value: &serde_json::Value) -> Option<String> {
    if let Some(s) = value.as_str() {
        return non_empty(s);
    }
    let obj = value.as_object()?;
    for key in ["message", "detail", "error_description", "error"] {
        if let Some(found) = obj.get(key).and_then(message_from_value) {
            return Some(found);
        }
    }
    None
}

/// 从非完整 JSON 的文本（SSE 片段等）中取第一个消息字段
fn message_from_fragment(text: &str) -> Option<String> {
    let re = message_fragment_re()?;
    let caps = re.captures(text)?;
    let raw = caps.get(1)?.as_str();
    // 捕获内容是 JSON 字符串体，尝试反转义；失败则原样使用
    let decoded =
        serde_json::from_str::<String>(&format!("\"{raw}\"")).unwrap_or_else(|_| raw.to_string());
    non_empty(&decoded)
}

/// `missing ... required` 组合（允许中间夹少量任意字符与换行）
fn missing_required_re() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)missing\b.{0,80}?required").ok())
        .as_ref()
}

/// 错误体中的 Retry-After 字段（`"retryAfter":30` / `retry_after=45`）
fn retry_after_re() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?i)"?retry[_-]?after"?\s*[:=]\s*"?(\d{1,7})"#).ok())
        .as_ref()
}

/// SSE / 错误体片段中的消息字段
fn message_fragment_re() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)"(?:message|error|detail|error_description)"\s*:\s*"((?:\\.|[^"\\])*)""#)
            .ok()
    })
    .as_ref()
}

/// 小写关键词包含匹配（任一命中即可）
fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

/// 去空白后非空则返回拥有所有权的字符串
fn non_empty(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 连续空白（含换行）折叠为单个空格
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 按字符截断到 `max` 个字符；发生截断时以 `...` 结尾（总长仍为 `max`）
fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        None => s.to_string(),
        Some(_) => {
            let keep = max.saturating_sub(3);
            let mut out: String = s.chars().take(keep).collect();
            out.push_str("...");
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_rules_are_case_insensitive() {
        // 排队
        assert_eq!(classify(200, "waiting_room_queued"), ErrorKind::WaitingRoom);
        assert_eq!(
            classify(200, "Waiting Room: position 3"),
            ErrorKind::WaitingRoom
        );
        assert_eq!(classify(502, "正在排队，请稍候"), ErrorKind::WaitingRoom);
        // 限流
        assert_eq!(classify(200, "RATE LIMIT exceeded"), ErrorKind::RateLimit);
        assert_eq!(classify(200, "rate_limit_exceeded"), ErrorKind::RateLimit);
        assert_eq!(classify(200, "Too Many Requests"), ErrorKind::RateLimit);
        assert_eq!(classify(400, "请求已被限流"), ErrorKind::RateLimit);
        // 模型不可用
        assert_eq!(
            classify(200, "free_mode_invalid_agent_model"),
            ErrorKind::ModelUnavailable
        );
        assert_eq!(
            classify(200, "MODEL_NOT_FOUND"),
            ErrorKind::ModelUnavailable
        );
        assert_eq!(
            classify(200, "No Such Model: gpt-x"),
            ErrorKind::ModelUnavailable
        );
        assert_eq!(
            classify(200, "model not available"),
            ErrorKind::ModelUnavailable
        );
        // 凭证失效
        assert_eq!(classify(200, "UNAUTHORIZED"), ErrorKind::AuthExpired);
        assert_eq!(classify(200, "Invalid Token"), ErrorKind::AuthExpired);
        assert_eq!(classify(200, "session expired"), ErrorKind::AuthExpired);
        assert_eq!(classify(200, "Token Expired"), ErrorKind::AuthExpired);
        assert_eq!(
            classify(200, "authentication failed"),
            ErrorKind::AuthExpired
        );
        // 请求错误
        assert_eq!(
            classify(200, "invalid_request_error"),
            ErrorKind::BadRequest
        );
        assert_eq!(classify(500, "Bad Request"), ErrorKind::BadRequest);
        assert_eq!(
            classify(200, "Missing required field: model"),
            ErrorKind::BadRequest
        );
    }

    #[test]
    fn text_overrides_status_code() {
        assert_eq!(classify(200, "rate limit"), ErrorKind::RateLimit);
        assert_eq!(classify(200, "waiting_room"), ErrorKind::WaitingRoom);
        assert_eq!(classify(500, "unauthorized"), ErrorKind::AuthExpired);
        assert_eq!(
            classify(500, "model_not_found"),
            ErrorKind::ModelUnavailable
        );
    }

    #[test]
    fn text_rule_priority_order() {
        // 同时含多类关键词时，按规则表顺序取第一个
        assert_eq!(
            classify(200, "waiting_room rate limit unauthorized"),
            ErrorKind::WaitingRoom
        );
        assert_eq!(
            classify(200, "rate limit model_not_found unauthorized"),
            ErrorKind::RateLimit
        );
        assert_eq!(
            classify(200, "model_not_found unauthorized bad request"),
            ErrorKind::ModelUnavailable
        );
        assert_eq!(
            classify(200, "unauthorized invalid_request"),
            ErrorKind::AuthExpired
        );
    }

    #[test]
    fn queue_hint_requires_429_or_503() {
        assert_eq!(classify(200, "queue position 2"), ErrorKind::Unknown);
        assert_eq!(classify(429, "queue position 2"), ErrorKind::WaitingRoom);
        assert_eq!(classify(503, "queued"), ErrorKind::WaitingRoom);
        // 200 但明确 waiting_room → 仍判排队
        assert_eq!(classify(200, "waiting_room_queued"), ErrorKind::WaitingRoom);
    }

    #[test]
    fn status_fallback_covers_all_branches() {
        assert_eq!(classify(429, ""), ErrorKind::RateLimit);
        assert_eq!(classify(401, ""), ErrorKind::AuthExpired);
        assert_eq!(classify(403, ""), ErrorKind::AuthExpired);
        assert_eq!(classify(404, ""), ErrorKind::ModelUnavailable);
        assert_eq!(classify(400, ""), ErrorKind::BadRequest);
        assert_eq!(classify(422, ""), ErrorKind::BadRequest);
        assert_eq!(classify(500, ""), ErrorKind::Upstream5xx);
        assert_eq!(classify(503, ""), ErrorKind::Upstream5xx);
        assert_eq!(classify(599, ""), ErrorKind::Upstream5xx);
        assert_eq!(classify(200, "ok"), ErrorKind::Unknown);
        assert_eq!(classify(418, ""), ErrorKind::Unknown);
        assert_eq!(classify(301, ""), ErrorKind::Unknown);
    }

    #[test]
    fn empty_body_with_status_zero_is_network() {
        assert_eq!(classify(0, ""), ErrorKind::Network);
        assert_eq!(classify(0, "   "), ErrorKind::Network);
    }

    #[test]
    fn retry_after_header_takes_priority_over_body() {
        assert_eq!(
            extract_retry_after(r#"{"retryAfter":5}"#, Some("30")),
            Some(30)
        );
        assert_eq!(extract_retry_after("", Some(" 30 ")), Some(30));
    }

    #[test]
    fn retry_after_body_variants() {
        assert_eq!(extract_retry_after(r#"{"retryAfter":30}"#, None), Some(30));
        assert_eq!(
            extract_retry_after(r#"{"retry_after": 12}"#, None),
            Some(12)
        );
        assert_eq!(extract_retry_after(r#"{"retry-after":"7"}"#, None), Some(7));
        assert_eq!(
            extract_retry_after("please retry_after=45 seconds later", None),
            Some(45)
        );
    }

    #[test]
    fn retry_after_absent_or_invalid_is_none() {
        assert_eq!(extract_retry_after("", None), None);
        assert_eq!(extract_retry_after("no hint here", None), None);
        // 头部非法时回退 body
        assert_eq!(
            extract_retry_after(r#"{"retryAfter":9}"#, Some("not-a-number")),
            Some(9)
        );
        assert_eq!(extract_retry_after("", Some("")), None);
    }

    #[test]
    fn excerpt_prefers_message_field() {
        assert_eq!(
            error_excerpt(r#"{"error":{"message":"model not available"}}"#),
            "model not available"
        );
        assert_eq!(error_excerpt(r#"{"message":"boom"}"#), "boom");
        assert_eq!(
            error_excerpt(r#"{"error":"plain failure"}"#),
            "plain failure"
        );
        assert_eq!(
            error_excerpt(r#"{"detail":"missing required field"}"#),
            "missing required field"
        );
    }

    #[test]
    fn excerpt_extracts_from_sse_fragment() {
        let body = "event: error\ndata: {\"error\":{\"message\":\"session expired\"}}\n\n";
        assert_eq!(classify(200, body), ErrorKind::AuthExpired);
        assert_eq!(error_excerpt(body), "session expired");
    }

    #[test]
    fn excerpt_falls_back_to_raw_body_and_normalizes_whitespace() {
        assert_eq!(
            error_excerpt("upstream   exploded\nbadly"),
            "upstream exploded badly"
        );
        assert_eq!(error_excerpt(""), "");
        assert_eq!(error_excerpt("   \n  "), "");
    }

    #[test]
    fn excerpt_decodes_json_escapes() {
        assert_eq!(
            error_excerpt(r#"{"message":"line1\nline2 \"q\""}"#),
            "line1 line2 \"q\""
        );
    }

    #[test]
    fn excerpt_truncates_to_300_chars() {
        let long = "a".repeat(400);
        let body = format!(r#"{{"message":"{long}"}}"#);
        let out = error_excerpt(&body);
        assert_eq!(out.chars().count(), 300);
        assert!(out.ends_with("..."));
        // 多字节字符同样按字符截断，不产生非法 UTF-8
        let zh = "中".repeat(400);
        let body = format!(r#"{{"message":"{zh}"}}"#);
        let out = error_excerpt(&body);
        assert_eq!(out.chars().count(), 300);
        assert!(out.ends_with("..."));
    }

    #[test]
    fn excerpt_short_message_is_untouched() {
        assert_eq!(error_excerpt(r#"{"message":"short"}"#), "short");
    }

    #[test]
    fn as_str_and_serde_are_stable_snake_case() {
        let kinds = [
            (ErrorKind::RateLimit, "rate_limit"),
            (ErrorKind::WaitingRoom, "waiting_room"),
            (ErrorKind::AuthExpired, "auth_expired"),
            (ErrorKind::ModelUnavailable, "model_unavailable"),
            (ErrorKind::BadRequest, "bad_request"),
            (ErrorKind::Upstream5xx, "upstream_5xx"),
            (ErrorKind::Network, "network"),
            (ErrorKind::Unknown, "unknown"),
        ];
        for (kind, expected) in kinds {
            assert_eq!(kind.as_str(), expected);
            assert_eq!(kind.to_string(), expected);
            assert_eq!(
                serde_json::to_string(&kind).unwrap(),
                format!("\"{expected}\"")
            );
        }
    }

    #[test]
    fn retryable_and_hint_matrix_is_complete() {
        let rows = [
            (ErrorKind::RateLimit, true, Some(60)),
            (ErrorKind::WaitingRoom, true, Some(15)),
            (ErrorKind::AuthExpired, false, None),
            (ErrorKind::ModelUnavailable, false, None),
            (ErrorKind::BadRequest, false, None),
            (ErrorKind::Upstream5xx, true, Some(5)),
            (ErrorKind::Network, true, None),
            (ErrorKind::Unknown, false, None),
        ];
        for (kind, retryable, hint) in rows {
            assert_eq!(kind.is_retryable(), retryable, "{}", kind.as_str());
            assert_eq!(kind.retry_after_hint(), hint, "{}", kind.as_str());
        }
    }
}

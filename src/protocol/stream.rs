//! 流式协议的规范化中间表示与通用 SSE 行解析。
//!
//! 上游（OpenAI 兼容）与下游（Anthropic）通过 [`CanonicalEvent`] 解耦：
//! 解码器只负责产出 canonical 事件，渲染器只负责消费 canonical 事件，
//! 双方互不感知对方的数据结构。

/// 协议无关的流式事件（中间表示）。
#[derive(Debug, Clone, PartialEq)]
pub enum CanonicalEvent {
    /// 消息开始：携带上游 message id、model 与输入 token 数
    MessageStart {
        id: String,
        model: String,
        input_tokens: u64,
    },
    /// 正文文本增量
    TextDelta(String),
    /// 工具调用块开始：`index` 为上游工具调用下标（用于后续增量对应）
    ToolUseStart {
        index: u64,
        id: String,
        name: String,
    },
    /// 工具调用参数 JSON 增量：`index` 与 [`CanonicalEvent::ToolUseStart`] 对应
    ToolUseInputDelta { index: u64, partial_json: String },
    /// 用量统计（通常位于流末尾的独立 chunk）
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    /// 消息结束：stop_reason ∈ `end_turn|max_tokens|tool_use|stop_sequence`
    MessageStop { stop_reason: String },
    /// 上游错误或解析错误
    Error(String),
}

/// 通用 SSE 解析器：把任意字节流切成 `(event_name, data)` 对。
///
/// 支持：
/// - 一行被拆到多个 chunk（内部缓冲字节，行界按 `\n` 判定）
/// - 单个 chunk 含多行 / 多个事件
/// - 多行 `data:` 以 `\n` 拼接为一个事件
/// - 注释行（`: keep-alive` 等）忽略
/// - CRLF 与 LF 行尾
/// - UTF-8 多字节字符跨 chunk 边界（按字节缓冲，成行后才解码）
#[derive(Debug, Default)]
pub struct SseLineParser {
    /// 尚未成行的原始字节
    buf: Vec<u8>,
    /// 当前事件名（`event:` 字段）
    event: Option<String>,
    /// 当前事件的数据行（`data:` 字段，可多行）
    data: Vec<String>,
}

impl SseLineParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一段字节，返回本次解析出的完整事件。
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<(Option<String>, String)> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
            line.pop(); // 去掉行尾 '\n'
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line).into_owned();
            self.handle_line(&line, &mut out);
        }
        out
    }

    /// 处理单行文本；空行表示一个事件结束，触发派发。
    fn handle_line(&mut self, line: &str, out: &mut Vec<(Option<String>, String)>) {
        if line.is_empty() {
            self.dispatch(out);
            return;
        }
        // 以 ':' 开头为注释行（如 `: keep-alive`），按 SSE 规范忽略
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.find(':') {
            Some(i) => (
                &line[..i],
                line[i + 1..].strip_prefix(' ').unwrap_or(&line[i + 1..]),
            ),
            None => (line, ""),
        };
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => self.data.push(value.to_string()),
            // id / retry 等字段对协议转换无意义，忽略
            _ => {}
        }
    }

    /// 派发当前累积的事件；无 data 缓冲时按 SSE 规范丢弃（仅清空事件名）。
    fn dispatch(&mut self, out: &mut Vec<(Option<String>, String)>) {
        if self.data.is_empty() {
            self.event = None;
            return;
        }
        let data = self.data.join("\n");
        self.data.clear();
        out.push((self.event.take(), data));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_event_with_data() {
        let mut parser = SseLineParser::new();
        let events = parser.feed(b"event: message_start\ndata: {\"a\":1}\n\n");
        assert_eq!(
            events,
            vec![(Some("message_start".to_string()), "{\"a\":1}".to_string())]
        );
    }

    #[test]
    fn parses_data_only_event() {
        let mut parser = SseLineParser::new();
        let events = parser.feed(b"data: [DONE]\n\n");
        assert_eq!(events, vec![(None, "[DONE]".to_string())]);
    }

    #[test]
    fn buffers_line_split_across_chunks() {
        let mut parser = SseLineParser::new();
        assert!(parser.feed(b"data: he").is_empty());
        assert!(parser.feed(b"llo").is_empty());
        let events = parser.feed(b"\n\n");
        assert_eq!(events, vec![(None, "hello".to_string())]);
    }

    #[test]
    fn joins_multiline_data_with_newline() {
        let mut parser = SseLineParser::new();
        let events = parser.feed(b"data: line1\ndata: line2\n\n");
        assert_eq!(events, vec![(None, "line1\nline2".to_string())]);
    }

    #[test]
    fn ignores_comment_and_unknown_fields() {
        let mut parser = SseLineParser::new();
        assert!(parser.feed(b": keep-alive\n\n").is_empty());
        assert!(parser.feed(b"id: 42\n\n").is_empty());
        assert!(parser.feed(b"retry: 1000\n\n").is_empty());
    }

    #[test]
    fn handles_crlf_and_multiple_events_in_one_chunk() {
        let mut parser = SseLineParser::new();
        let events = parser.feed(b"data: a\r\n\r\ndata: b\r\n\r\n");
        assert_eq!(
            events,
            vec![(None, "a".to_string()), (None, "b".to_string()),]
        );
    }

    #[test]
    fn carries_utf8_char_split_across_chunks() {
        // "data: 中\n\n" 中「中」占 3 字节，从字符中间拆开喂入
        let raw = "data: 中\n\n".as_bytes();
        let split = raw.len() - 4; // 保留「中」的最后一个字节与两个换行
        let mut parser = SseLineParser::new();
        assert!(parser.feed(&raw[..split]).is_empty());
        let events = parser.feed(&raw[split..]);
        assert_eq!(events, vec![(None, "中".to_string())]);
    }

    #[test]
    fn data_value_keeps_leading_spaces_after_one_separator() {
        // SSE 规范：仅移除冒号后的第一个空格，其余空格保留
        let mut parser = SseLineParser::new();
        let events = parser.feed(b"data:  two spaces\n\n");
        assert_eq!(events, vec![(None, " two spaces".to_string())]);
    }
}

//! 上游 OpenAI 兼容 `chat.completions` 流式 chunk → [`CanonicalEvent`]。
//!
//! 兼容要点：
//! - `delta.content` → [`CanonicalEvent::TextDelta`]
//! - `delta.tool_calls[]` 分片：首次（携带 id/name）→ `ToolUseStart`，
//!   `function.arguments` 分片 → `ToolUseInputDelta`（按 `index` 对应）
//! - `finish_reason`：`stop`→`end_turn`、`length`→`max_tokens`、`tool_calls`→`tool_use`
//! - `usage`（`stream_options.include_usage` 时末尾 chunk 才有）→ `Usage`
//! - `data: [DONE]` → `MessageStop`；上游未给 `finish_reason` 时回落 `end_turn`

use std::collections::HashMap;

use super::stream::{CanonicalEvent, SseLineParser};

/// 单个上游工具调用的累积状态（id/name 可能分片到达）。
#[derive(Debug, Default, Clone)]
struct ToolCallState {
    id: String,
    name: String,
    started: bool,
}

/// OpenAI 兼容 SSE 解码器。
#[derive(Debug, Default)]
pub struct OpenAiSseDecoder {
    parser: SseLineParser,
    message_id: String,
    model: String,
    started: bool,
    stopped: bool,
    /// 上游给出的 finish_reason（已映射为 Anthropic 取值）
    finish_reason: Option<String>,
    /// 上游工具调用 index → 累积状态
    tools: HashMap<u64, ToolCallState>,
}

impl OpenAiSseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// 输入上游 SSE 字节，输出 0..n 个 canonical 事件。
    ///
    /// 遇到 `data: [DONE]` 输出 [`CanonicalEvent::MessageStop`]
    /// （若上游没给 `finish_reason` 则 `stop_reason = "end_turn"`）。
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<CanonicalEvent> {
        let mut out = Vec::new();
        for (_event, data) in self.parser.feed(chunk) {
            self.handle_payload(data.trim(), &mut out);
        }
        out
    }

    /// 上游未发送 `[DONE]` 就断流时的兜底：补发一条 `MessageStop`。
    ///
    /// 已停止过则返回空；未收到任何 chunk 时也会先补 `MessageStart`。
    pub fn finish(&mut self) -> Vec<CanonicalEvent> {
        let mut out = Vec::new();
        self.emit_stop(&mut out);
        out
    }

    /// 处理单条 SSE data 负载。
    fn handle_payload(&mut self, data: &str, out: &mut Vec<CanonicalEvent>) {
        if data.is_empty() {
            return;
        }
        if data == "[DONE]" {
            self.emit_stop(out);
            return;
        }
        if self.stopped {
            return;
        }
        let value: serde_json::Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => {
                out.push(CanonicalEvent::Error(format!(
                    "invalid upstream SSE payload: {}",
                    truncate(data, 200)
                )));
                return;
            }
        };
        if let Some(err) = value.get("error") {
            out.push(CanonicalEvent::Error(extract_error_message(err)));
            return;
        }
        self.absorb_identity(&value);
        // usage 先于 choices 处理：若本 chunk 同时携带两者，message_start 可用上 input_tokens
        if let Some(usage) = value.get("usage").filter(|u| !u.is_null()) {
            let input = json_u64(usage.get("prompt_tokens"));
            let output = json_u64(usage.get("completion_tokens"));
            self.ensure_started(out, input);
            out.push(CanonicalEvent::Usage {
                input_tokens: input,
                output_tokens: output,
            });
        }
        self.handle_choices(&value, out);
        // 首个合法 chat chunk 即使暂无内容也先发 MessageStart，让下游尽早拿到消息头
        if !self.started && (value.get("choices").is_some() || !self.message_id.is_empty()) {
            self.ensure_started(out, 0);
        }
    }

    /// 记录上游 message id / model（后续 MessageStart 使用）。
    fn absorb_identity(&mut self, value: &serde_json::Value) {
        if let Some(id) = value.get("id").and_then(|v| v.as_str()) {
            if !id.is_empty() {
                self.message_id = id.to_string();
            }
        }
        if let Some(model) = value.get("model").and_then(|v| v.as_str()) {
            if !model.is_empty() {
                self.model = model.to_string();
            }
        }
    }

    /// 解析 `choices[0]` 的 delta 与 finish_reason。
    fn handle_choices(&mut self, value: &serde_json::Value, out: &mut Vec<CanonicalEvent>) {
        let Some(choice) = value
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|c| c.first())
        else {
            return;
        };
        if let Some(delta) = choice.get("delta") {
            if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
                if !text.is_empty() {
                    self.ensure_started(out, 0);
                    out.push(CanonicalEvent::TextDelta(text.to_string()));
                }
            }
            if let Some(calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                for call in calls {
                    self.handle_tool_call(call, out);
                }
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(|f| f.as_str()) {
            self.finish_reason = Some(map_finish_reason(reason).to_string());
        }
    }

    /// 解析单个工具调用分片（id/name/arguments 均可能分片到达）。
    fn handle_tool_call(&mut self, call: &serde_json::Value, out: &mut Vec<CanonicalEvent>) {
        let index = json_u64(call.get("index"));
        let incoming_id = call.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let function = call.get("function");
        let incoming_name = function
            .and_then(|f| f.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("");
        let args = function
            .and_then(|f| f.get("arguments"))
            .and_then(|a| a.as_str())
            .unwrap_or("");
        let (started, id, name) = {
            let state = self.tools.entry(index).or_default();
            if !incoming_id.is_empty() {
                state.id = incoming_id.to_string();
            }
            if !incoming_name.is_empty() {
                state.name = incoming_name.to_string();
            }
            (state.started, state.id.clone(), state.name.clone())
        };
        let complete = !id.is_empty() && !name.is_empty();
        if !started && (complete || !args.is_empty()) {
            if let Some(state) = self.tools.get_mut(&index) {
                state.started = true;
            }
            self.ensure_started(out, 0);
            out.push(CanonicalEvent::ToolUseStart { index, id, name });
        }
        if !args.is_empty() {
            self.ensure_started(out, 0);
            out.push(CanonicalEvent::ToolUseInputDelta {
                index,
                partial_json: args.to_string(),
            });
        }
    }

    /// 发送 MessageStop（幂等）。
    fn emit_stop(&mut self, out: &mut Vec<CanonicalEvent>) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.ensure_started(out, 0);
        let reason = self
            .finish_reason
            .clone()
            .unwrap_or_else(|| "end_turn".to_string());
        out.push(CanonicalEvent::MessageStop {
            stop_reason: reason,
        });
    }

    /// 确保 MessageStart 已发出（仅一次）。
    fn ensure_started(&mut self, out: &mut Vec<CanonicalEvent>, input_tokens: u64) {
        if self.started {
            return;
        }
        self.started = true;
        let id = if self.message_id.is_empty() {
            "chatcmpl-unknown".to_string()
        } else {
            self.message_id.clone()
        };
        out.push(CanonicalEvent::MessageStart {
            id,
            model: self.model.clone(),
            input_tokens,
        });
    }
}

/// OpenAI finish_reason → Anthropic stop_reason。
fn map_finish_reason(reason: &str) -> &'static str {
    match reason {
        "length" => "max_tokens",
        "tool_calls" | "function_call" => "tool_use",
        // "stop" 及 content_filter 等其余取值统一按正常结束处理
        _ => "end_turn",
    }
}

/// 从 `Value` 取 u64（缺省 0）。
fn json_u64(value: Option<&serde_json::Value>) -> u64 {
    value.and_then(|v| v.as_u64()).unwrap_or(0)
}

/// 提取错误消息（无 message 字段时退化为整个 JSON 文本）。
fn extract_error_message(err: &serde_json::Value) -> String {
    err.get("message")
        .and_then(|m| m.as_str())
        .map(|m| m.to_string())
        .unwrap_or_else(|| err.to_string())
}

/// 截断过长文本（按字符边界，避免 panic）。
fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        text.chars().take(max_chars).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_chunk(content: &str, finish: Option<&str>) -> String {
        let payload = serde_json::json!({
            "id": "chatcmpl-1",
            "model": "gpt-x",
            "choices": [{
                "index": 0,
                "delta": { "content": content },
                "finish_reason": finish,
            }],
        });
        format!("data: {payload}\n\n")
    }

    fn tool_chunk(index: u64, id: Option<&str>, name: Option<&str>, args: Option<&str>) -> String {
        let mut function = serde_json::Map::new();
        if let Some(name) = name {
            function.insert("name".to_string(), serde_json::json!(name));
        }
        if let Some(args) = args {
            function.insert("arguments".to_string(), serde_json::json!(args));
        }
        let mut call = serde_json::Map::new();
        call.insert("index".to_string(), serde_json::json!(index));
        if let Some(id) = id {
            call.insert("id".to_string(), serde_json::json!(id));
        }
        call.insert("function".to_string(), serde_json::Value::Object(function));
        let payload = serde_json::json!({
            "id": "chatcmpl-1",
            "model": "gpt-x",
            "choices": [{
                "index": 0,
                "delta": { "tool_calls": [call] },
                "finish_reason": null,
            }],
        });
        format!("data: {payload}\n\n")
    }

    fn usage_chunk(input: u64, output: u64) -> String {
        let payload = serde_json::json!({
            "id": "chatcmpl-1",
            "model": "gpt-x",
            "choices": [],
            "usage": { "prompt_tokens": input, "completion_tokens": output },
        });
        format!("data: {payload}\n\n")
    }

    #[test]
    fn decodes_text_stream_across_chunks() {
        let mut decoder = OpenAiSseDecoder::new();
        let first = decoder.feed(text_chunk("He", None).as_bytes());
        assert_eq!(
            first,
            vec![
                CanonicalEvent::MessageStart {
                    id: "chatcmpl-1".to_string(),
                    model: "gpt-x".to_string(),
                    input_tokens: 0,
                },
                CanonicalEvent::TextDelta("He".to_string()),
            ]
        );
        let second = decoder.feed(text_chunk("llo", None).as_bytes());
        assert_eq!(second, vec![CanonicalEvent::TextDelta("llo".to_string())]);
        let finish = decoder.feed(text_chunk("", Some("stop")).as_bytes());
        assert!(finish.is_empty());
        let done = decoder.feed(b"data: [DONE]\n\n");
        assert_eq!(
            done,
            vec![CanonicalEvent::MessageStop {
                stop_reason: "end_turn".to_string(),
            }]
        );
    }

    #[test]
    fn decodes_tool_call_arguments_in_three_fragments() {
        let mut decoder = OpenAiSseDecoder::new();
        let started =
            decoder.feed(tool_chunk(0, Some("call_a"), Some("get_weather"), None).as_bytes());
        assert_eq!(
            started,
            vec![
                CanonicalEvent::MessageStart {
                    id: "chatcmpl-1".to_string(),
                    model: "gpt-x".to_string(),
                    input_tokens: 0,
                },
                CanonicalEvent::ToolUseStart {
                    index: 0,
                    id: "call_a".to_string(),
                    name: "get_weather".to_string(),
                },
            ]
        );
        let part1 = decoder.feed(tool_chunk(0, None, None, Some("{\"ci")).as_bytes());
        let part2 = decoder.feed(tool_chunk(0, None, None, Some("ty\":\"SF")).as_bytes());
        let part3 = decoder.feed(tool_chunk(0, None, None, Some("\"}")).as_bytes());
        let deltas: Vec<CanonicalEvent> = [part1, part2, part3].concat();
        assert_eq!(
            deltas,
            vec![
                CanonicalEvent::ToolUseInputDelta {
                    index: 0,
                    partial_json: "{\"ci".to_string(),
                },
                CanonicalEvent::ToolUseInputDelta {
                    index: 0,
                    partial_json: "ty\":\"SF".to_string(),
                },
                CanonicalEvent::ToolUseInputDelta {
                    index: 0,
                    partial_json: "\"}".to_string(),
                },
            ]
        );
    }

    #[test]
    fn decodes_two_tool_calls_by_index() {
        let mut decoder = OpenAiSseDecoder::new();
        decoder.feed(tool_chunk(0, Some("call_a"), Some("f_a"), None).as_bytes());
        decoder.feed(tool_chunk(1, Some("call_b"), Some("f_b"), None).as_bytes());
        let delta = decoder.feed(tool_chunk(1, None, None, Some("{}")).as_bytes());
        assert_eq!(
            delta,
            vec![CanonicalEvent::ToolUseInputDelta {
                index: 1,
                partial_json: "{}".to_string(),
            }]
        );
    }

    #[test]
    fn decodes_usage_chunk_after_text() {
        let mut decoder = OpenAiSseDecoder::new();
        decoder.feed(text_chunk("hi", None).as_bytes());
        let events = decoder.feed(usage_chunk(11, 22).as_bytes());
        assert_eq!(
            events,
            vec![CanonicalEvent::Usage {
                input_tokens: 11,
                output_tokens: 22,
            }]
        );
    }

    #[test]
    fn usage_only_stream_puts_tokens_in_message_start() {
        let mut decoder = OpenAiSseDecoder::new();
        let events = decoder.feed(usage_chunk(7, 3).as_bytes());
        assert_eq!(
            events,
            vec![
                CanonicalEvent::MessageStart {
                    id: "chatcmpl-1".to_string(),
                    model: "gpt-x".to_string(),
                    input_tokens: 7,
                },
                CanonicalEvent::Usage {
                    input_tokens: 7,
                    output_tokens: 3,
                },
            ]
        );
    }

    #[test]
    fn maps_all_finish_reasons() {
        let cases = [
            ("stop", "end_turn"),
            ("length", "max_tokens"),
            ("tool_calls", "tool_use"),
        ];
        for (upstream, expected) in cases {
            let mut decoder = OpenAiSseDecoder::new();
            decoder.feed(text_chunk("x", Some(upstream)).as_bytes());
            let events = decoder.feed(b"data: [DONE]\n\n");
            assert_eq!(
                events,
                vec![CanonicalEvent::MessageStop {
                    stop_reason: expected.to_string(),
                }],
                "finish_reason={upstream}"
            );
        }
    }

    #[test]
    fn done_without_finish_reason_defaults_end_turn() {
        let mut decoder = OpenAiSseDecoder::new();
        decoder.feed(text_chunk("x", None).as_bytes());
        let events = decoder.feed(b"data: [DONE]\n\n");
        assert_eq!(
            events,
            vec![CanonicalEvent::MessageStop {
                stop_reason: "end_turn".to_string(),
            }]
        );
    }

    #[test]
    fn error_payload_emits_error_event() {
        let mut decoder = OpenAiSseDecoder::new();
        let events = decoder
            .feed(b"data: {\"error\":{\"message\":\"rate limited\",\"type\":\"rate_limit\"}}\n\n");
        assert_eq!(
            events,
            vec![CanonicalEvent::Error("rate limited".to_string())]
        );
    }

    #[test]
    fn malformed_json_emits_error_event() {
        let mut decoder = OpenAiSseDecoder::new();
        let events = decoder.feed(b"data: {oops not json\n\n");
        match events.as_slice() {
            [CanonicalEvent::Error(message)] => {
                assert!(message.contains("invalid upstream SSE payload"));
            }
            other => panic!("expected error event, got {other:?}"),
        }
    }

    #[test]
    fn assembles_event_split_across_feed_calls() {
        let mut decoder = OpenAiSseDecoder::new();
        let full = tool_chunk(0, Some("call_a"), Some("f_a"), Some("{}"));
        let (head, tail) = full.split_at(full.len() / 2);
        assert!(decoder.feed(head.as_bytes()).is_empty());
        let events = decoder.feed(tail.as_bytes());
        assert_eq!(
            events,
            vec![
                CanonicalEvent::MessageStart {
                    id: "chatcmpl-1".to_string(),
                    model: "gpt-x".to_string(),
                    input_tokens: 0,
                },
                CanonicalEvent::ToolUseStart {
                    index: 0,
                    id: "call_a".to_string(),
                    name: "f_a".to_string(),
                },
                CanonicalEvent::ToolUseInputDelta {
                    index: 0,
                    partial_json: "{}".to_string(),
                },
            ]
        );
    }

    #[test]
    fn ignores_comment_lines_between_events() {
        let mut decoder = OpenAiSseDecoder::new();
        decoder.feed(text_chunk("hi", None).as_bytes());
        let events = decoder.feed(b": keep-alive\n\n");
        assert!(events.is_empty());
    }

    #[test]
    fn finish_without_done_emits_stop_once() {
        let mut decoder = OpenAiSseDecoder::new();
        decoder.feed(text_chunk("hi", Some("length")).as_bytes());
        let first = decoder.finish();
        assert_eq!(
            first,
            vec![CanonicalEvent::MessageStop {
                stop_reason: "max_tokens".to_string(),
            }]
        );
        assert!(decoder.finish().is_empty());
    }

    #[test]
    fn ignores_payload_after_done() {
        let mut decoder = OpenAiSseDecoder::new();
        decoder.feed(text_chunk("hi", None).as_bytes());
        decoder.feed(b"data: [DONE]\n\n");
        let after = decoder.feed(text_chunk("late", None).as_bytes());
        assert!(after.is_empty());
    }
}

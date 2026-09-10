//! [`CanonicalEvent`] → Anthropic Messages 流式 SSE 事件行渲染。
//!
//! 输出帧形如 `event: <name>\ndata: <json>\n\n`，调用方 `join("")` 后直接写入响应体。
//! 关键约束：
//! - `message_start` 必须先于任何 `content_block_*`（可延迟到首个 delta）
//! - 文本块实时输出；流结束时先 `content_block_stop` 再输出工具块
//! - 工具块**缓冲渲染**：上游并行工具调用的 start 与 arguments 分片可能任意交错，
//!   立即输出会产生指向已关闭块的非法序列；因此按上游 index 缓冲，
//!   收尾时按 index 升序依次输出
//!   `content_block_start → content_block_delta(完整参数) → content_block_stop`
//! - content block 下标由本渲染器按 Anthropic 规范从 0 连续分配

use std::collections::BTreeMap;

use serde_json::json;

use super::stream::CanonicalEvent;

/// 单条完整 SSE 帧（含结尾空行）。
type SseFrame = String;

/// 缓冲中的工具调用：`ToolUseStart` 与参数分片可能任意交错到达，
/// 收尾时整块输出以保证块嵌套合法。
#[derive(Debug, Default, Clone)]
struct BufferedTool {
    id: String,
    name: String,
    /// 累积的上游 arguments JSON 分片
    arguments: String,
    /// 是否收到过 ToolUseStart（未 start 过的增量忽略）
    started: bool,
}

/// Anthropic SSE 渲染器。
#[derive(Debug, Default)]
pub struct AnthropicSseRenderer {
    /// 消息 model（`MessageStart` 事件可覆盖）
    model: String,
    /// 消息 id（`MessageStart` 事件提供；缺失时生成回退 id）
    message_id: Option<String>,
    /// 是否已发出 `message_start`
    started: bool,
    /// 当前打开的 content block 下标（只可能是文本块；工具块收尾时整块输出）
    open_index: Option<u64>,
    /// 下一个可分配的 content block 下标
    next_index: u64,
    /// 按上游 index 缓冲的工具调用（BTreeMap 保证收尾时按 index 升序输出）
    tools: BTreeMap<u64, BufferedTool>,
    /// 输入 token 数（用于 message_start 与 message_delta 回填）
    input_tokens: u64,
    /// 输出 token 数（用于 message_delta）
    output_tokens: u64,
    /// 是否已发出 message_stop（或终止性 error）
    stop_sent: bool,
}

impl AnthropicSseRenderer {
    pub fn new(model: &str) -> Self {
        Self {
            model: model.to_string(),
            ..Self::default()
        }
    }

    /// 渲染一个 canonical 事件为 0..n 条完整 SSE 帧。
    pub fn render(&mut self, ev: &CanonicalEvent) -> Vec<SseFrame> {
        match ev {
            CanonicalEvent::MessageStart {
                id,
                model,
                input_tokens,
            } => self.apply_message_start(id, model, *input_tokens),
            CanonicalEvent::TextDelta(text) => self.render_text_delta(text),
            CanonicalEvent::ToolUseStart { index, id, name } => {
                self.render_tool_start(*index, id, name)
            }
            CanonicalEvent::ToolUseInputDelta {
                index,
                partial_json,
            } => self.render_tool_input_delta(*index, partial_json),
            CanonicalEvent::Usage {
                input_tokens,
                output_tokens,
            } => self.render_usage(*input_tokens, *output_tokens),
            CanonicalEvent::MessageStop { stop_reason } => self.render_stop(stop_reason),
            CanonicalEvent::Error(message) => self.render_error(message),
        }
    }

    /// 流结束时兜底：确保 `content_block_stop`（如有打开块）、缓冲工具块、
    /// `message_delta` 与 `message_stop` 已发出；已收尾则返回空。
    pub fn finish(&mut self) -> Vec<SseFrame> {
        if self.stop_sent {
            return Vec::new();
        }
        self.finalize("end_turn")
    }

    /// 处理 MessageStart：记录 id/model/input_tokens（首个生效），已开始则忽略。
    fn apply_message_start(&mut self, id: &str, model: &str, input_tokens: u64) -> Vec<SseFrame> {
        if !id.is_empty() {
            self.message_id = Some(id.to_string());
        }
        if !model.is_empty() {
            self.model = model.to_string();
        }
        self.input_tokens = input_tokens;
        self.ensure_started()
    }

    /// 文本增量：确保已开始并在文本块内，输出 text_delta。
    fn render_text_delta(&mut self, text: &str) -> Vec<SseFrame> {
        let mut out = self.ensure_started();
        if text.is_empty() {
            return out;
        }
        // 打开的块只可能是文本块（工具块收尾时整块输出）
        let index = match self.open_index {
            Some(index) => index,
            None => self.open_text_block(&mut out),
        };
        out.push(frame(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": { "type": "text_delta", "text": text },
            }),
        ));
        out
    }

    /// 工具块开始：仅缓冲（不立即输出），保证并行工具调用的块嵌套合法。
    fn render_tool_start(&mut self, upstream_index: u64, id: &str, name: &str) -> Vec<SseFrame> {
        let out = self.ensure_started();
        let state = self.tools.entry(upstream_index).or_default();
        if !state.started {
            state.started = true;
            if !id.is_empty() {
                state.id = id.to_string();
            }
            if !name.is_empty() {
                state.name = name.to_string();
            }
        }
        out
    }

    /// 工具参数增量：追加到对应缓冲（未 start 过的上游 index 忽略）。
    fn render_tool_input_delta(
        &mut self,
        upstream_index: u64,
        partial_json: &str,
    ) -> Vec<SseFrame> {
        let out = self.ensure_started();
        if partial_json.is_empty() {
            return out;
        }
        if let Some(state) = self.tools.get_mut(&upstream_index) {
            state.arguments.push_str(partial_json);
        }
        out
    }

    /// 用量：记录 token 数；若消息尚未开始则借此机会先发 message_start。
    fn render_usage(&mut self, input_tokens: u64, output_tokens: u64) -> Vec<SseFrame> {
        self.input_tokens = input_tokens;
        self.output_tokens = output_tokens;
        self.ensure_started()
    }

    /// 消息结束：关文本块 + 输出缓冲工具块 + message_delta + message_stop（幂等）。
    fn render_stop(&mut self, stop_reason: &str) -> Vec<SseFrame> {
        if self.stop_sent {
            return Vec::new();
        }
        self.finalize(stop_reason)
    }

    /// 统一收尾：关文本块 → 按上游 index 升序输出缓冲工具块 → message_delta + message_stop。
    fn finalize(&mut self, stop_reason: &str) -> Vec<SseFrame> {
        let mut out = self.ensure_started();
        out.extend(self.close_block());
        out.extend(self.flush_tools());
        out.extend(self.stop_frames(stop_reason));
        out
    }

    /// 输出全部缓冲工具块：每块 start → delta（完整参数）→ stop。
    fn flush_tools(&mut self) -> Vec<SseFrame> {
        let tools = std::mem::take(&mut self.tools);
        let mut out = Vec::new();
        for tool in tools.into_values() {
            let index = self.alloc_index();
            out.push(frame(
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": {
                        "type": "tool_use",
                        "id": tool.id,
                        "name": tool.name,
                        "input": {},
                    },
                }),
            ));
            // 空参数按空 JSON 对象输出，保证客户端可解析
            let partial = if tool.arguments.is_empty() {
                "{}"
            } else {
                tool.arguments.as_str()
            };
            out.push(frame(
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": index,
                    "delta": { "type": "input_json_delta", "partial_json": partial },
                }),
            ));
            out.push(frame(
                "content_block_stop",
                json!({ "type": "content_block_stop", "index": index }),
            ));
        }
        out
    }

    /// 错误事件：关闭打开的文本块、丢弃工具缓冲并输出 Anthropic error 帧，
    /// 随后不再补正常收尾。
    fn render_error(&mut self, message: &str) -> Vec<SseFrame> {
        let mut out = self.ensure_started();
        out.extend(self.close_block());
        self.tools.clear();
        out.push(frame(
            "error",
            json!({
                "type": "error",
                "error": { "type": "api_error", "message": message },
            }),
        ));
        self.stop_sent = true;
        out
    }

    /// 确保 message_start 已发出（仅一次）。
    fn ensure_started(&mut self) -> Vec<SseFrame> {
        if self.started {
            return Vec::new();
        }
        self.started = true;
        vec![self.message_start_frame()]
    }

    /// 关闭当前打开的 content block（无打开块时为空）。
    fn close_block(&mut self) -> Vec<SseFrame> {
        match self.open_index.take() {
            Some(index) => vec![frame(
                "content_block_stop",
                json!({ "type": "content_block_stop", "index": index }),
            )],
            None => Vec::new(),
        }
    }

    /// 在文本块内打开新块（调用方保证当前块已关闭）。
    fn open_text_block(&mut self, out: &mut Vec<SseFrame>) -> u64 {
        let index = self.alloc_index();
        self.open_index = Some(index);
        out.push(frame(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": index,
                "content_block": { "type": "text", "text": "" },
            }),
        ));
        index
    }

    /// message_delta + message_stop 两帧，并置停止标记。
    fn stop_frames(&mut self, stop_reason: &str) -> Vec<SseFrame> {
        self.stop_sent = true;
        vec![
            frame(
                "message_delta",
                json!({
                    "type": "message_delta",
                    "delta": {
                        "stop_reason": normalize_stop_reason(stop_reason),
                        "stop_sequence": null,
                    },
                    "usage": {
                        "input_tokens": self.input_tokens,
                        "output_tokens": self.output_tokens,
                    },
                }),
            ),
            frame("message_stop", json!({ "type": "message_stop" })),
        ]
    }

    /// 构造 message_start 帧。
    fn message_start_frame(&self) -> SseFrame {
        let id = self.message_id.clone().unwrap_or_else(fallback_message_id);
        frame(
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": id,
                    "type": "message",
                    "role": "assistant",
                    "model": self.model,
                    "content": [],
                    "stop_reason": null,
                    "stop_sequence": null,
                    "usage": { "input_tokens": self.input_tokens, "output_tokens": 0 },
                },
            }),
        )
    }

    /// 分配下一个 content block 下标。
    fn alloc_index(&mut self) -> u64 {
        let index = self.next_index;
        self.next_index += 1;
        index
    }
}

/// 组装一条 SSE 帧。
fn frame(event: &str, data: serde_json::Value) -> SseFrame {
    format!("event: {event}\ndata: {data}\n\n")
}

/// 上游 stop_reason 规范化：非 Anthropic 合法取值统一回落 end_turn。
fn normalize_stop_reason(reason: &str) -> &str {
    match reason {
        "end_turn" | "max_tokens" | "tool_use" | "stop_sequence" => reason,
        _ => "end_turn",
    }
}

/// 缺少上游 message id 时的回退 id（时间戳保证进程内唯一）。
fn fallback_message_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("msg_{nanos}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_frame(text: &str, index: u64) -> String {
        frame(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": { "type": "text_delta", "text": text },
            }),
        )
    }

    fn start_frame(id: &str, model: &str, input_tokens: u64) -> String {
        frame(
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": id,
                    "type": "message",
                    "role": "assistant",
                    "model": model,
                    "content": [],
                    "stop_reason": null,
                    "stop_sequence": null,
                    "usage": { "input_tokens": input_tokens, "output_tokens": 0 },
                },
            }),
        )
    }

    #[test]
    fn renders_text_stream_byte_level() {
        let mut renderer = AnthropicSseRenderer::new("claude-3");
        let events = [
            CanonicalEvent::MessageStart {
                id: "msg_1".to_string(),
                model: "claude-x".to_string(),
                input_tokens: 7,
            },
            CanonicalEvent::TextDelta("He".to_string()),
            CanonicalEvent::TextDelta("llo".to_string()),
            CanonicalEvent::MessageStop {
                stop_reason: "end_turn".to_string(),
            },
        ];
        let out: String = events.iter().flat_map(|ev| renderer.render(ev)).collect();
        let expected = [
            start_frame("msg_1", "claude-x", 7),
            frame(
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": { "type": "text", "text": "" },
                }),
            ),
            text_frame("He", 0),
            text_frame("llo", 0),
            frame(
                "content_block_stop",
                json!({ "type": "content_block_stop", "index": 0 }),
            ),
            frame(
                "message_delta",
                json!({
                    "type": "message_delta",
                    "delta": { "stop_reason": "end_turn", "stop_sequence": null },
                    "usage": { "input_tokens": 7, "output_tokens": 0 },
                }),
            ),
            frame("message_stop", json!({ "type": "message_stop" })),
        ]
        .concat();
        assert_eq!(out, expected);
    }

    /// 工具块完整帧序列：start → delta(完整参数) → stop
    fn tool_frames(index: u64, id: &str, name: &str, partial_json: &str) -> String {
        [
            frame(
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": {
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": {},
                    },
                }),
            ),
            frame(
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": index,
                    "delta": { "type": "input_json_delta", "partial_json": partial_json },
                }),
            ),
            frame(
                "content_block_stop",
                json!({ "type": "content_block_stop", "index": index }),
            ),
        ]
        .concat()
    }

    fn message_delta_frame(stop_reason: &str, input_tokens: u64, output_tokens: u64) -> String {
        frame(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": { "stop_reason": stop_reason, "stop_sequence": null },
                "usage": { "input_tokens": input_tokens, "output_tokens": output_tokens },
            }),
        )
    }

    #[test]
    fn closes_text_block_then_emits_buffered_tool_block() {
        let mut renderer = AnthropicSseRenderer::new("claude-3");
        renderer.render(&CanonicalEvent::MessageStart {
            id: "msg_1".to_string(),
            model: "claude-3".to_string(),
            input_tokens: 1,
        });
        renderer.render(&CanonicalEvent::TextDelta("hi".to_string()));
        // ToolUseStart 与参数分片只入缓冲，不立即输出
        assert!(renderer
            .render(&CanonicalEvent::ToolUseStart {
                index: 0,
                id: "toolu_1".to_string(),
                name: "search".to_string(),
            })
            .is_empty());
        assert!(renderer
            .render(&CanonicalEvent::ToolUseInputDelta {
                index: 0,
                partial_json: "{\"q\":\"x\"}".to_string(),
            })
            .is_empty());
        let out: String = renderer
            .render(&CanonicalEvent::MessageStop {
                stop_reason: "tool_use".to_string(),
            })
            .concat();
        let expected = [
            frame(
                "content_block_stop",
                json!({ "type": "content_block_stop", "index": 0 }),
            ),
            tool_frames(1, "toolu_1", "search", "{\"q\":\"x\"}"),
            message_delta_frame("tool_use", 1, 0),
            frame("message_stop", json!({ "type": "message_stop" })),
        ]
        .concat();
        assert_eq!(out, expected);
    }

    #[test]
    fn renders_parallel_tool_calls_in_index_order_byte_level() {
        let mut renderer = AnthropicSseRenderer::new("claude-3");
        renderer.render(&CanonicalEvent::MessageStart {
            id: "msg_1".to_string(),
            model: "claude-3".to_string(),
            input_tokens: 3,
        });
        renderer.render(&CanonicalEvent::TextDelta("hi".to_string()));
        // 并行工具：首个 chunk 同时 start index 0/1，参数随后交错回填
        renderer.render(&CanonicalEvent::ToolUseStart {
            index: 0,
            id: "toolu_a".to_string(),
            name: "fa".to_string(),
        });
        renderer.render(&CanonicalEvent::ToolUseStart {
            index: 1,
            id: "toolu_b".to_string(),
            name: "fb".to_string(),
        });
        renderer.render(&CanonicalEvent::ToolUseInputDelta {
            index: 1,
            partial_json: "{\"y\"".to_string(),
        });
        renderer.render(&CanonicalEvent::ToolUseInputDelta {
            index: 0,
            partial_json: "{\"x\"".to_string(),
        });
        renderer.render(&CanonicalEvent::ToolUseInputDelta {
            index: 1,
            partial_json: ":2}".to_string(),
        });
        renderer.render(&CanonicalEvent::ToolUseInputDelta {
            index: 0,
            partial_json: ":1}".to_string(),
        });
        let out: String = renderer
            .render(&CanonicalEvent::MessageStop {
                stop_reason: "tool_use".to_string(),
            })
            .concat();
        let expected = [
            frame(
                "content_block_stop",
                json!({ "type": "content_block_stop", "index": 0 }),
            ),
            tool_frames(1, "toolu_a", "fa", "{\"x\":1}"),
            tool_frames(2, "toolu_b", "fb", "{\"y\":2}"),
            message_delta_frame("tool_use", 3, 0),
            frame("message_stop", json!({ "type": "message_stop" })),
        ]
        .concat();
        assert_eq!(out, expected);
    }

    #[test]
    fn buffers_tool_arguments_until_stop() {
        let mut renderer = AnthropicSseRenderer::new("claude-3");
        let started = renderer.render(&CanonicalEvent::ToolUseStart {
            index: 0,
            id: "toolu_0".to_string(),
            name: "f0".to_string(),
        });
        assert_eq!(started.len(), 1); // 仅补发 message_start
        assert!(started[0].starts_with("event: message_start\n"));
        assert!(renderer
            .render(&CanonicalEvent::ToolUseInputDelta {
                index: 0,
                partial_json: "{\"a\"".to_string(),
            })
            .is_empty());
        assert!(renderer
            .render(&CanonicalEvent::ToolUseInputDelta {
                index: 0,
                partial_json: ":1}".to_string(),
            })
            .is_empty());
        let out: String = renderer
            .render(&CanonicalEvent::MessageStop {
                stop_reason: "tool_use".to_string(),
            })
            .concat();
        let expected = [
            tool_frames(0, "toolu_0", "f0", "{\"a\":1}"),
            message_delta_frame("tool_use", 0, 0),
            frame("message_stop", json!({ "type": "message_stop" })),
        ]
        .concat();
        assert_eq!(out, expected);
    }

    #[test]
    fn usage_sets_message_delta_output_tokens() {
        let mut renderer = AnthropicSseRenderer::new("claude-3");
        renderer.render(&CanonicalEvent::MessageStart {
            id: "msg_1".to_string(),
            model: "claude-3".to_string(),
            input_tokens: 5,
        });
        renderer.render(&CanonicalEvent::TextDelta("hi".to_string()));
        let usage_out = renderer.render(&CanonicalEvent::Usage {
            input_tokens: 5,
            output_tokens: 42,
        });
        assert!(usage_out.is_empty());
        let stop = renderer.render(&CanonicalEvent::MessageStop {
            stop_reason: "max_tokens".to_string(),
        });
        assert!(stop[1].contains("\"input_tokens\":5"));
        assert!(stop[1].contains("\"output_tokens\":42"));
        assert!(stop[1].contains("\"stop_reason\":\"max_tokens\""));
    }

    #[test]
    fn usage_before_any_delta_starts_message_with_input_tokens() {
        let mut renderer = AnthropicSseRenderer::new("claude-3");
        let out = renderer.render(&CanonicalEvent::Usage {
            input_tokens: 9,
            output_tokens: 1,
        });
        assert_eq!(out.len(), 1);
        assert!(out[0].starts_with("event: message_start\n"));
        assert!(out[0].contains("\"input_tokens\":9"));
    }

    #[test]
    fn delayed_message_start_on_first_delta() {
        let mut renderer = AnthropicSseRenderer::new("claude-3");
        let out = renderer.render(&CanonicalEvent::TextDelta("hi".to_string()));
        assert_eq!(out.len(), 3);
        assert!(out[0].starts_with("event: message_start\n"));
        assert!(out[0].contains("\"model\":\"claude-3\""));
        assert!(out[0].contains("\"id\":\"msg_"));
        assert!(out[1].starts_with("event: content_block_start\n"));
        assert!(out[2].starts_with("event: content_block_delta\n"));
    }

    #[test]
    fn finish_emits_fallback_stop_after_partial_stream() {
        let mut renderer = AnthropicSseRenderer::new("claude-3");
        renderer.render(&CanonicalEvent::TextDelta("hi".to_string()));
        let out: String = renderer.finish().concat();
        assert!(out.contains("event: content_block_stop\n"));
        assert!(out.contains("\"stop_reason\":\"end_turn\""));
        assert!(out.contains("event: message_stop\n"));
        assert!(renderer.finish().is_empty());
    }

    #[test]
    fn finish_flushes_buffered_tools() {
        let mut renderer = AnthropicSseRenderer::new("claude-3");
        let mut frames = renderer.render(&CanonicalEvent::ToolUseStart {
            index: 0,
            id: "toolu_0".to_string(),
            name: "f0".to_string(),
        });
        frames.extend(renderer.render(&CanonicalEvent::ToolUseInputDelta {
            index: 0,
            partial_json: "{}".to_string(),
        }));
        frames.extend(renderer.finish());
        let out: String = frames.concat();
        assert!(out.contains("event: message_start\n"));
        assert!(out.contains("event: content_block_start\n"));
        assert!(out.contains("\"partial_json\":\"{}\""));
        assert!(out.contains("event: content_block_stop\n"));
        assert!(out.contains("\"stop_reason\":\"end_turn\""));
        assert!(out.contains("event: message_stop\n"));
    }

    #[test]
    fn finish_is_noop_after_message_stop() {
        let mut renderer = AnthropicSseRenderer::new("claude-3");
        renderer.render(&CanonicalEvent::MessageStop {
            stop_reason: "end_turn".to_string(),
        });
        assert!(renderer.finish().is_empty());
    }

    #[test]
    fn empty_stream_stop_still_emits_valid_sequence() {
        let mut renderer = AnthropicSseRenderer::new("claude-3");
        let out: String = renderer
            .render(&CanonicalEvent::MessageStop {
                stop_reason: "end_turn".to_string(),
            })
            .concat();
        // 仅断言结构：message_start 先于 message_delta 先于 message_stop，且无 content block
        // （id 为时间戳回退值，不做字节级比较）
        let start_pos = out.find("event: message_start").unwrap_or(usize::MAX);
        let delta_pos = out.find("event: message_delta").unwrap_or(usize::MAX);
        let stop_pos = out.find("event: message_stop").unwrap_or(usize::MAX);
        assert!(start_pos < delta_pos && delta_pos < stop_pos);
        assert!(!out.contains("content_block_start"));
        assert!(out.contains("\"model\":\"claude-3\""));
    }

    #[test]
    fn error_event_closes_block_and_terminates() {
        let mut renderer = AnthropicSseRenderer::new("claude-3");
        renderer.render(&CanonicalEvent::TextDelta("hi".to_string()));
        let out: String = renderer
            .render(&CanonicalEvent::Error("boom".to_string()))
            .concat();
        assert!(out.contains("event: content_block_stop\n"));
        assert!(out.contains("event: error\n"));
        assert!(out.contains("\"message\":\"boom\""));
        assert!(renderer.finish().is_empty());
    }

    #[test]
    fn normalizes_unknown_stop_reason() {
        let mut renderer = AnthropicSseRenderer::new("claude-3");
        let out: String = renderer
            .render(&CanonicalEvent::MessageStop {
                stop_reason: "content_filter".to_string(),
            })
            .concat();
        assert!(out.contains("\"stop_reason\":\"end_turn\""));
    }

    #[test]
    fn ignores_tool_input_delta_without_start() {
        let mut renderer = AnthropicSseRenderer::new("claude-3");
        renderer.render(&CanonicalEvent::MessageStart {
            id: "msg_1".to_string(),
            model: "claude-3".to_string(),
            input_tokens: 0,
        });
        let out = renderer.render(&CanonicalEvent::ToolUseInputDelta {
            index: 3,
            partial_json: "{}".to_string(),
        });
        assert!(out.is_empty());
    }
}

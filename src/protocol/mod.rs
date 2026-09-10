//! OpenAI ⇄ Anthropic 流式协议转换。
//!
//! 分层：
//! - [`stream`]：协议无关的 [`stream::CanonicalEvent`] 中间表示 + 通用 SSE 行解析器
//! - [`openai_sse`]：上游 OpenAI 兼容 SSE chunk → canonical 事件
//! - [`anthropic_sse`]：canonical 事件 → 下游 Anthropic SSE 事件行
//!
//! 接线方式：`OpenAiSseDecoder::feed` 消费上游字节流，输出 canonical 事件，
//! 交给 `AnthropicSseRenderer::render` 渲染为可直接写入响应体的 SSE 帧；
//! 上游异常断流时调用 `AnthropicSseRenderer::finish` 兜底收尾。

pub mod anthropic_sse;
pub mod openai_sse;
pub mod stream;

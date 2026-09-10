//! Freebuff2API — Rust 版 OpenAI/Anthropic 兼容网关
//!
//! 架构分层：
//! - `config`   配置加载（JSON + 环境变量）
//! - `models`   模型注册表（上游 free-agents.ts + 硬编码底座）
//! - `upstream` 上游 Codebuff HTTP 客户端（会话/run/chat/广告）
//! - `pool`     多账号池：会话健康评分 + 轮询 + 冷却 + 熔断
//! - `session`  Freebuff 会话管理（排队/活跃/保活/心跳）
//! - `routes`   OpenAI / Anthropic / 面板 / 用量统计 HTTP 路由
//! - `usage`    SQLite 用量统计与审计
//! - `router`   模型路由 + 降级链 + token 节省（9router 式）
//! - `ads`      广告换 token 保活
//! - `web`      内嵌控制面板静态资源

pub mod ads;
pub mod api;
pub mod config;
pub mod import;
pub mod models;
pub mod pool;
pub mod prompts;
pub mod router;
pub mod session;
pub mod upstream;
pub mod usage;
pub mod web;
pub mod web_protocol;

pub use config::Config;

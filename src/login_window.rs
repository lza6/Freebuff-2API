//! 内嵌登录窗口平台入口：Windows 走 WebView2 实现，其他平台走 stub。
//!
//! 统一 API（`is_login_mode` / `run_login_window` / `spawn_login_window`），
//! 调用方（main.rs / api.rs）无需感知平台差异。

#[cfg(windows)]
pub use crate::login_window_windows::*;

#[cfg(not(windows))]
pub use crate::login_window_stub::*;

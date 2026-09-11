//! 内嵌登录窗口的非 Windows stub（WebView2 仅 Windows 可用）
//!
//! 面板 `/api/login/embed` 在非 Windows 平台返回明确失败，
//! 前端降级到 扩展 / 剪贴板 / 手动向导（登录向导文案已含平台说明）。
//!
//! 本 stub 与 `login_window_windows` 保持**签名一致**（经 `login_window` facade 再导出，
//! 调用方无需感知平台差异）；Windows 上这些函数经 facade 有真实调用方，故 stub 侧
//! 允许 dead_code 而不触发警告。

/// 非 Windows 上永不处于登录模式（--login-window 参数在 main 分派时也走 stub 提示）
#[allow(dead_code)]
pub fn is_login_mode() -> bool {
    false
}

/// 非 Windows stub：报告不支持（不会真正被调用——main 分派先检查平台）
#[allow(dead_code)]
pub fn run_login_window(_gateway_port: Option<u16>) -> i32 {
    eprintln!("内嵌登录窗口仅支持 Windows（需要 Microsoft Edge WebView2 Runtime）——请改用浏览器扩展或手动导入");
    1
}

/// 非 Windows stub：返回 false → /api/login/embed 返回 ok:false 引导降级
#[allow(dead_code)]
pub fn spawn_login_window(_gateway_port: u16) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_reports_unsupported() {
        assert!(!spawn_login_window(47821));
        assert!(!is_login_mode());
        assert_eq!(run_login_window(Some(47821)), 1);
    }
}

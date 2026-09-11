//! 内嵌浏览器一键登录（Windows / WebView2 / wry）——仅 cfg(windows) 编译
//!
//! 用户诉求：「一键登录只能用扩展吗？不能直接用内联浏览器抓取？」——本模块即答案：
//! 网关派生一个独立的登录子进程（避免 WebView2 事件循环阻塞网关的 tokio 运行时），
//! 窗口里加载 freebuff.com，用户完成 GitHub 登录后，
//! 通过 WebView2 CookieManager 抓取**全部 Cookie（含 HttpOnly 的 session-token）**，
//! 自动 POST 到本机网关 `/api/tokens/import` 入库，然后窗口自关。
//!
//! 行为基准：桌面版 Electron 的同款实现（desktop/main.js:219-293）。
//! HttpOnly 可读性：ICoreWebView2CookieManager 与 Electron session.cookies 同级，
//! 是 OS 级 WebView 组件（非网页 JS），不受浏览器同源 JS 限制。
//!
//! 进程模型：网关主进程 `Command::new(current_exe) --login-window` 派生子进程；
//! 子进程自足运行（tao 事件循环独立），**结果直接通过进程退出码汇报**：
//!   0 = 登录并入库成功（stdout 有结果消息）；1 = 失败/超时/用户关窗（stderr 有原因）。

use anyhow::{anyhow, Result};

/// 登录成功判定：Cookie 串中必须含会话令牌（以 Cookie 为准，而非 URL 特征）
const SESSION_MARK: &str = "__Secure-next-auth.session-token";

/// 网关默认探测端口（与扩展 background.js 的 DEFAULT_PORTS 一致）
const GATEWAY_PORTS: &[u16] = &[47821, 47822, 8787];

/// 轮询登录态的上限（与扩展 180s 同量级，取宽容值）
const LOGIN_TIMEOUT_SECS: u64 = 600;

/// 是否为「登录窗口」子进程模式
pub fn is_login_mode() -> bool {
    std::env::args().any(|a| a == "--login-window")
}

/// 运行登录窗口子进程（blocking，tao 事件循环 `run()` 返回 `!`，不会真正返回到调用方）。
/// 保留返回类型以便测试与未来变化；实际控制流经 `process::exit` 直接结束进程。
pub fn run_login_window(gateway_port: Option<u16>) -> i32 {
    match run_login_window_inner(gateway_port) {
        Ok(msg) => {
            println!("{msg}");
            0
        }
        Err(e) => {
            eprintln!("登录窗口失败：{e:#}");
            1
        }
    }
}

fn run_login_window_inner(gateway_port: Option<u16>) -> Result<String> {
    use wry::WebViewBuilder;

    let ports: Vec<u16> = gateway_port
        .map(|p| vec![p])
        .unwrap_or_else(|| {
            // GATEWAY_PORT 环境变量优先（网关 spawn 时会带）；否则按默认序列探测
            if let Ok(p) = std::env::var("GATEWAY_PORT") {
                if let Ok(p) = p.parse() {
                    return vec![p];
                }
            }
            GATEWAY_PORTS.to_vec()
        });

    // tao 的 EventLoop::new() 失败时内部自行 panic（官方示例同款用法）
    let event_loop = tao::event_loop::EventLoop::new();
    let window = tao::window::WindowBuilder::new()
        .with_title("Freebuff 登录 — 完成后自动入库")
        .with_inner_size(tao::dpi::LogicalSize::new(1000.0, 720.0))
        .build(&event_loop)
        .map_err(|e| anyhow!("创建登录窗口失败: {e}"))?;

    let webview = WebViewBuilder::new()
        .with_url("https://freebuff.com/")
        .build(&window)
        .map_err(|e| anyhow!("初始化 WebView2 失败: {e}（请确认系统已安装 Microsoft Edge WebView2 Runtime）"))?;

    // 抓取当前 Cookie 并尝试入库；返回 Some((exit_code, message)) 表示流程已终结。
    // 独立函数（借 webview），供"首载探测"与"事件循环轮询"两处复用——规避 run() 的 'static 闭包约束。
    fn try_capture(
        webview: &wry::WebView,
        ports: &[u16],
    ) -> Option<(i32, String)> {
        let Ok(cookies) = webview.cookies_for_url("https://freebuff.com/") else {
            return None;
        };
        let header = cookies_to_header(&cookies);
        if !header.contains(SESSION_MARK) {
            return None; // 未登录，继续等待
        }
        for port in ports {
            match post_import(*port, &header) {
                Ok(added) => {
                    let msg = if added > 0 {
                        format!("✅ 登录成功：已自动入库 {added} 个凭证（端口 {port}），可关闭本窗口回到面板刷新")
                    } else {
                        "✅ 登录成功：凭证已存在（同值自动去重）".to_string()
                    };
                    return Some((0, msg));
                }
                Err(e) => {
                    // 连接失败 → 换下一个端口；网关明确拒绝 → 停止并如实报告
                    if !e.to_string().contains("connection") {
                        return Some((1, format!("网关拒绝了凭证（端口 {port}）：{e}")));
                    }
                }
            }
        }
        Some((1, format!("无法连接本机网关（尝试端口 {ports:?}）——请确认网关已启动")))
    }

    // 首次加载即探测一次（用户可能带着有效会话直接进来）
    if let Some((code, msg)) = try_capture(&webview, &ports) {
        report(code, &msg);
    }

    let started = std::time::Instant::now();
    let ports_for_loop = ports.clone();
    event_loop.run(move |event, _, control_flow| {
        use tao::event::Event::*;
        match event {
            WindowEvent { event: tao::event::WindowEvent::CloseRequested, .. } => {
                report(1, "窗口已关闭但未完成登录（可重试，或改用扩展/手动导入）");
            }
            NewEvents(..) => {
                // 600ms 后再唤醒（WaitUntil：UI 线程空转零负载，不拖帧率）
                *control_flow = tao::event_loop::ControlFlow::WaitUntil(
                    std::time::Instant::now() + std::time::Duration::from_millis(600),
                );
                if started.elapsed() > std::time::Duration::from_secs(LOGIN_TIMEOUT_SECS) {
                    report(1, "等待登录超时——请重试，或改用扩展/手动导入");
                }
                if let Some((code, msg)) = try_capture(&webview, &ports_for_loop) {
                    report(code, &msg);
                }
            }
            _ => {}
        }
    })
}

/// 汇报结果并结束进程（run() 不可返回，退出码即结果协议）
fn report(code: i32, msg: &str) {
    if code == 0 {
        println!("{msg}");
    } else {
        eprintln!("{msg}");
    }
    std::process::exit(code);
}

/// wry `cookies_for_url` 返回的 `Vec<cookie::Cookie>` → Cookie 请求头字符串。
/// HttpOnly 的 session-token 包含在内（CookieManager 是 OS 级组件，不受网页 JS 限制）。
fn cookies_to_header(cookies: &[wry::cookie::Cookie<'static>]) -> String {
    cookies
        .iter()
        .filter(|c| !c.value().is_empty())
        .map(|c| format!("{}={}", c.name(), c.value()))
        .collect::<Vec<_>>()
        .join("; ")
}

/// POST 本机网关 /api/tokens/import；返回 added 数量。
/// Err 文案含 "connection" 表示网络不通（应换端口），否则为网关明确拒绝。
/// 注意：子进程仍是 #[tokio::main] 的二进制（有 runtime context），但这里处在 tao 事件循环同步上下文，
/// 不能 .await —— 用 `futures::executor::block_on` 在当前线程驱动 async reqwest（勿改成 spawn，会脱离 runtime context）。
fn post_import(port: u16, cookie: &str) -> Result<usize> {
    let url = format!("http://127.0.0.1:{port}/api/tokens/import");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;
    let resp = futures::executor::block_on(async {
        client
            .post(&url)
            .header("content-type", "application/json")
            .body(serde_json::json!({ "cookie": cookie }).to_string())
            .send()
            .await
            .map_err(|e| anyhow!("connection failed: {e}"))
    })?;
    let status = resp.status();
    let text = futures::executor::block_on(resp.text()).unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("HTTP {}：{}", status, text.chars().take(160).collect::<String>());
    }
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::json!({}));
    Ok(v.get("added").and_then(|a| a.as_u64()).unwrap_or(0) as usize)
}

/// 网关侧：派生登录窗口子进程（非阻塞）。
/// 返回 true 表示已派生；false 表示环境不支持（应降级为扩展/手动向导）。
/// 防重入标志：同一时刻最多一个登录窗口（连点按钮不会堆叠 N 个 WebView2 进程）。
/// 子进程正常退出（入库成功/超时/关窗）后释放。
static LOGIN_SPAWNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn spawn_login_window(gateway_port: u16) -> bool {
    use std::sync::atomic::Ordering;
    // CAS 抢占：已有一个登录窗口在跑 → 直接拒绝（前端 toast 提示）
    if LOGIN_SPAWNED
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        tracing::warn!("登录窗口已在运行，忽略重复请求");
        return false;
    }
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => {
            LOGIN_SPAWNED.store(false, Ordering::Release);
            return false;
        }
    };
    let child = std::process::Command::new(exe)
        .arg("--login-window")
        .env("GATEWAY_PORT", gateway_port.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match child {
        Ok(mut c) => {
            // 后台线程等待子进程退出：把失败原因落到结果文件（面板 openEmbedLogin 轮询可读），
            // 否则无 WebView2 Runtime 的机器上子进程 exit(1) 无任何用户可见反馈（面板干等超时）。
            std::thread::spawn(move || {
                let code = c.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
                LOGIN_SPAWNED.store(false, Ordering::Release);
                if code != 0 {
                    // 成功时窗口已提示；失败时写结果文件供面板显示
                    let msg = serde_json::json!({
                        "ok": false,
                        "exit": code,
                        "message": format!("内嵌登录窗口退出（代码 {code}）——请查看窗口内提示，或改用扩展/剪贴板/手动导入"),
                    });
                    let path = std::path::Path::new("data");
                    if path.exists() {
                        let _ = std::fs::write(path.join("login_window_result.json"), msg.to_string());
                    }
                }
            });
            true
        }
        Err(_) => {
            LOGIN_SPAWNED.store(false, Ordering::Release);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies_to_header_filters_empty() {
        let c1 = wry::cookie::Cookie::new("__Secure-next-auth.session-token", "abc123");
        let c2 = wry::cookie::Cookie::new("other", "");
        let c3 = wry::cookie::Cookie::new("x", "y");
        let header = cookies_to_header(&[c1, c2, c3]);
        assert!(header.contains("__Secure-next-auth.session-token=abc123"));
        assert!(header.contains("x=y"));
        assert!(!header.contains("other="), "空值 cookie 应被剔除");
    }

    #[test]
    fn cookies_to_header_empty() {
        assert_eq!(cookies_to_header(&[]), "");
    }

    #[test]
    fn login_mode_detection() {
        // 当前测试进程没有 --login-window 参数
        assert!(!is_login_mode());
    }
}

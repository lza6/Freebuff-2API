use freebuff2api::ads::AdRefresher;
use freebuff2api::api::{build_router, AppState};
use freebuff2api::config::Config;
use freebuff2api::logbus::LogBus;
use freebuff2api::models::ModelRegistry;
use freebuff2api::pool::Pool;
use freebuff2api::router::{ModelRouter, RouterConfig};
use freebuff2api::skills::SkillsManager;
use freebuff2api::telemetry::TelemetryWriter;
use freebuff2api::upstream::UpstreamClient;
use freebuff2api::usage::UsageDb;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 登录窗口子进程模式：不初始化 tokio 重基础设施，直接进 WebView2 事件循环（结果走退出码）
    if std::env::args().any(|a| a == "--login-window") {
        let port = std::env::var("GATEWAY_PORT")
            .ok()
            .and_then(|p| p.parse().ok());
        // 子进程通过 GATEWAY_PROXY 继承网关的 http_proxy：Google OAuth 域名（accounts.google.com）
        // 在多数网络环境需代理可达，而 WebView2 实例默认不吃系统代理 → 登录页 ERR_CONNECTION_CLOSED
        let proxy = std::env::var("GATEWAY_PROXY")
            .ok()
            .filter(|p| !p.is_empty());
        let code = freebuff2api::login_window::run_login_window(port, proxy);
        std::process::exit(code);
    }

    // 日志
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,freebuff2api=debug")),
        )
        .with_target(false)
        .init();

    let config_path = freebuff2api::config::resolve_config_path();
    let cfg = Config::load(config_path.as_deref())?;
    let listen_addr = cfg.listen_addr.clone();
    tracing::info!("Freebuff2API v{} 启动", env!("CARGO_PKG_VERSION"));
    tracing::info!("监听 {}", listen_addr);
    tracing::info!("账号数 {}", cfg.auth_tokens.len());

    // 客户端
    let proxy = if cfg.http_proxy.is_empty() {
        None
    } else {
        Some(cfg.http_proxy.clone())
    };
    let client = Arc::new(UpstreamClient::new(
        cfg.upstream_base_url.clone(),
        proxy,
        Duration::from_secs(cfg.request_timeout_sec),
    )?);

    // 模型注册表
    let registry = Arc::new(ModelRegistry::new());
    registry.init().await;
    if let Ok((added, removed)) = registry.refresh_from_upstream(&client_http()).await {
        tracing::info!("模型注册表同步：新增 {added} 个，移除 {removed} 个");
    }
    // v0.10 H1 修复：生产路径加载 vendored 上游模型快照 → 策略覆盖（availability 时间窗/efforts/fallback）。
    // 失败静默降级到静态底座并 warn，绝不阻断启动（与计划书 §1.1 一致）。
    if let Some(snap) = freebuff2api::models::load_local_snapshot() {
        match registry.refresh_strategy_from_snapshot(&snap) {
            Ok((added, updated)) => {
                tracing::info!("模型策略快照同步：新增策略 {added}，更新 {updated}");
            }
            Err(e) => {
                tracing::warn!("模型策略快照同步失败，使用静态底座: {e}");
            }
        }
    }
    let router = Arc::new(ModelRouter::new(
        registry.clone(),
        RouterConfig::from_app_config(&cfg),
    ));

    // 多账号池
    let pool = Arc::new(Pool::new(&cfg, client.clone()));

    // 用量统计
    let usage = Arc::new(UsageDb::open(&cfg.sqlite_path)?);
    tracing::info!("用量统计 SQLite: {}", cfg.sqlite_path);

    // 遥测（请求详情/事件链；独立库 + 独立写线程，不阻塞请求路径）
    let telemetry = Arc::new(TelemetryWriter::spawn(
        PathBuf::from(&cfg.telemetry_path),
        4096,
    )?);
    tracing::info!("遥测 SQLite: {}", cfg.telemetry_path);

    // 实时日志总线（SSE 广播 + 环形缓冲）；脱敏开关跟随 config.redact_logs（默认开）
    let logs = Arc::new(LogBus::new_with_redact(500, cfg.redact_logs));
    if cfg.redact_logs {
        tracing::info!("日志脱敏已开启（config.redact_logs=true）");
    }

    // 记忆层（用户偏好/纠正；零 LLM 规则 observe）
    let memory = Arc::new(freebuff2api::memory::MemoryStore::open(PathBuf::from(
        &cfg.memory_path,
    ))?);
    tracing::info!("记忆库 SQLite: {}", cfg.memory_path);
    // 记忆层运行时开关（默认关闭——用户批注：记忆不是每个人都需要的；面板可热切换）
    let memory_runtime_enabled = Arc::new(std::sync::atomic::AtomicBool::new(cfg.memory_enabled));
    if cfg.memory_enabled {
        tracing::info!("记忆层已开启（config memory_enabled=true）");
    } else {
        tracing::info!("记忆层已关闭（默认；面板「记忆」页可开启）");
    }

    // 技能系统（文件为真相源 + SQLite 索引；旧 prompts 保留兼容）
    // 注意：不用 with_extension（目录名含 '.' 时会被截断，如 data/my.skills → data/my.sqlite）
    let skills_db = PathBuf::from(format!(
        "{}.sqlite",
        cfg.skills_dir.trim_end_matches(['/', '\\'])
    ));
    let skills = Arc::new(SkillsManager::open(
        PathBuf::from(&cfg.skills_dir),
        skills_db,
    )?);
    tracing::info!(
        "技能目录: {}（已载入 {} 条）",
        cfg.skills_dir,
        skills.list().len()
    );

    // 广告保活
    let ads = Arc::new(AdRefresher::new(client.clone(), cfg.clone()));

    // 内置提示词/技能
    let prompts = Arc::new(freebuff2api::prompts::PromptManager::new());

    // 凭证账号信息缓存 + 账号使用记录（面板凭证列表与历史查询）
    let meta = Arc::new(freebuff2api::account_meta::AccountMetaStore::new(
        PathBuf::from(&cfg.cred_meta_path),
        PathBuf::from(&cfg.account_history_path),
    ));
    tracing::info!(
        "凭证信息缓存: {} · 使用记录: {}",
        cfg.cred_meta_path,
        cfg.account_history_path
    );

    // 运行时 API Key（面板可一键生成并热生效，无需重启）
    let api_keys = Arc::new(std::sync::RwLock::new(cfg.api_keys.clone()));
    if cfg.api_keys.is_empty() {
        tracing::info!("未配置 api_keys：仅本机可访问（面板可在「接入指南」一键生成）");
    }

    // web 协议桥接（OpenAI/Anthropic 客户端 → 上游 thread 复用，省每日会话额度）
    let web_threads = Arc::new(freebuff2api::web_threads::WebThreadMap::new(PathBuf::from(
        &cfg.web_threads_path,
    )));
    tracing::info!("web 桥接会话绑定: {}", cfg.web_threads_path);

    // 启动各账号后台保活
    {
        let accounts = pool.accounts.lock().await;
        for acc in accounts.iter() {
            let sess = acc.session.clone();
            let ads = ads.clone();
            tokio::spawn(async move { sess.run_keepalive(ads).await });
        }
    }

    // 双桶并发信号量容量（在 cfg move 进 Arc 前读取）
    let (conc_free_slots, conc_free_multi, conc_sub_slots, conc_sub_multi) = (
        cfg.concurrency_free_slots,
        cfg.concurrency_free_multi,
        cfg.concurrency_sub_slots,
        cfg.concurrency_sub_multi,
    );

    // v0.9 §1.1：web Cookie 凭证池（config Cookie 项 + 导入库 kind=web-cookie）
    let web_pool = Arc::new(freebuff2api::web_pool::WebCookiePool::new(&cfg));

    let state = AppState {
        cfg: Arc::new(cfg),
        client,
        pool,
        web_pool,
        registry,
        router,
        usage,
        telemetry,
        logs,
        memory,
        skills,
        ads,
        prompts,
        meta,
        api_keys,
        web_threads,
        memory_runtime_enabled,
        semaphore: Arc::new(freebuff2api::semaphore::TieredSemaphore::new(
            conc_free_slots,
            conc_free_multi,
            conc_sub_slots,
            conc_sub_multi,
        )),
        started: std::time::Instant::now(),
    };

    // 上游会话自动清理（用户批注：反代要自己清理，别给上游留压力被查出来）
    if state.cfg.thread_cleanup_interval_sec > 0 {
        tokio::spawn(freebuff2api::api::thread_cleanup_loop(state.clone()));
    } else {
        tracing::info!("上游会话自动清理已关闭（thread_cleanup_interval_sec=0）");
    }

    let app = build_router(state);
    let listener = match tokio::net::TcpListener::bind(&listen_addr).await {
        Ok(l) => l,
        Err(e) => {
            // v0.8：端口绑定失败给出明确中文错误（含占用进程提示），而非裸 anyhow 上抛
            let hint = match e.kind() {
                std::io::ErrorKind::AddrInUse => {
                    format!(
                        "端口 {} 被占用（可能已有 Freebuff2API 实例在运行，或其它程序占用了该端口）。\n\
                         请修改 config.json 的 listen_addr 换个端口后重试。\n\
                         排查：运行 `netstat -ano | findstr :{}` 查看占用进程 PID，\n\
                         或用任务管理器结束占用该端口的进程（注意：不要误杀你自己的另一个实例）。",
                        listen_addr, port_of(&listen_addr)
                    )
                }
                std::io::ErrorKind::PermissionDenied => {
                    format!(
                        "没有权限绑定 {}（Windows 上 <1024 端口通常需要管理员权限）。\n\
                         请把 listen_addr 改成高位端口（如 127.0.0.1:47821）或管理员运行。",
                        listen_addr
                    )
                }
                _ => format!(
                    "绑定监听地址 {} 失败: {e}\n请检查 config.json 的 listen_addr 是否合法。",
                    listen_addr
                ),
            };
            eprintln!("\n[Freebuff2API] 启动失败：{hint}\n");
            anyhow::bail!(hint)
        }
    };
    tracing::info!("HTTP 服务就绪");
    // v0.9 §1.5：注入真实 TCP 对端（ConnectInfo<SocketAddr>）供管理端点回环判定
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// 从 listen_addr 提取端口（用于错误提示中的 findstr 排查命令）
fn port_of(listen_addr: &str) -> String {
    listen_addr
        .rsplit_once(':')
        .map(|(_, p)| p.trim_end_matches(']').to_string())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "47821".into())
}

// 用普通 reqwest client 做 registry 拉取（避免与上游 http client 混淆）
// 超时保护：注册表同步失败不应阻塞网关启动
fn client_http() -> reqwest::Client {
    let mut b = reqwest::Client::builder()
        .user_agent("freebuff2api-registry")
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10));
    if let Ok(p) = std::env::var("HTTPS_PROXY") {
        if !p.is_empty() {
            if let Ok(proxy) = reqwest::Proxy::all(&p) {
                b = b.proxy(proxy);
            }
        }
    }
    b.build().unwrap_or_else(|_| reqwest::Client::new())
}

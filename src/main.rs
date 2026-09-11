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
        let port = std::env::var("GATEWAY_PORT").ok().and_then(|p| p.parse().ok());
        let code = freebuff2api::login_window::run_login_window(port);
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
    let proxy = if cfg.http_proxy.is_empty() { None } else { Some(cfg.http_proxy.clone()) };
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
    let router = Arc::new(ModelRouter::new(registry.clone(), RouterConfig::from_app_config(&cfg)));

    // 多账号池
    let pool = Arc::new(Pool::new(&cfg, client.clone()));

    // 用量统计
    let usage = Arc::new(UsageDb::open(&cfg.sqlite_path)?);
    tracing::info!("用量统计 SQLite: {}", cfg.sqlite_path);

    // 遥测（请求详情/事件链；独立库 + 独立写线程，不阻塞请求路径）
    let telemetry = Arc::new(TelemetryWriter::spawn(PathBuf::from(&cfg.telemetry_path), 4096)?);
    tracing::info!("遥测 SQLite: {}", cfg.telemetry_path);

    // 实时日志总线（SSE 广播 + 环形缓冲）
    let logs = Arc::new(LogBus::new(500));

    // 记忆层（用户偏好/纠正；零 LLM 规则 observe）
    let memory = Arc::new(freebuff2api::memory::MemoryStore::open(PathBuf::from(&cfg.memory_path))?);
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
    let skills_db = PathBuf::from(format!("{}.sqlite", cfg.skills_dir.trim_end_matches(['/', '\\'])));
    let skills = Arc::new(SkillsManager::open(PathBuf::from(&cfg.skills_dir), skills_db)?);
    tracing::info!("技能目录: {}（已载入 {} 条）", cfg.skills_dir, skills.list().len());

    // 广告保活
    let ads = Arc::new(AdRefresher::new(client.clone(), cfg.clone()));

    // 内置提示词/技能
    let prompts = Arc::new(freebuff2api::prompts::PromptManager::new());

    // 凭证账号信息缓存 + 账号使用记录（面板凭证列表与历史查询）
    let meta = Arc::new(freebuff2api::account_meta::AccountMetaStore::new(
        PathBuf::from(&cfg.cred_meta_path),
        PathBuf::from(&cfg.account_history_path),
    ));
    tracing::info!("凭证信息缓存: {} · 使用记录: {}", cfg.cred_meta_path, cfg.account_history_path);

    // 运行时 API Key（面板可一键生成并热生效，无需重启）
    let api_keys = Arc::new(std::sync::RwLock::new(cfg.api_keys.clone()));
    if cfg.api_keys.is_empty() {
        tracing::info!("未配置 api_keys：仅本机可访问（面板可在「接入指南」一键生成）");
    }

    // web 协议桥接（OpenAI/Anthropic 客户端 → 上游 thread 复用，省每日会话额度）
    let web_threads = Arc::new(freebuff2api::web_threads::WebThreadMap::new(PathBuf::from(&cfg.web_threads_path)));
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

    let state = AppState {
        cfg: Arc::new(cfg),
        client,
        pool,
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
        started: std::time::Instant::now(),
    };

    // 上游会话自动清理（用户批注：反代要自己清理，别给上游留压力被查出来）
    if state.cfg.thread_cleanup_interval_sec > 0 {
        tokio::spawn(freebuff2api::api::thread_cleanup_loop(state.clone()));
    } else {
        tracing::info!("上游会话自动清理已关闭（thread_cleanup_interval_sec=0）");
    }

    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
    tracing::info!("HTTP 服务就绪");
    axum::serve(listener, app).await?;
    Ok(())
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

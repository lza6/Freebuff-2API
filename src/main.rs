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
    // 日志
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,freebuff2api=debug")),
        )
        .with_target(false)
        .init();

    let config_path = std::env::args().nth(1).filter(|a| a != "--config").or_else(|| {
        std::env::args()
            .position(|a| a == "--config")
            .and_then(|i| std::env::args().nth(i + 1))
    });
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
    let router = Arc::new(ModelRouter::new(registry.clone(), RouterConfig::default()));

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

    // 技能系统（文件为真相源 + SQLite 索引；旧 prompts 保留兼容）
    let skills = Arc::new(SkillsManager::open(
        PathBuf::from(&cfg.skills_dir),
        PathBuf::from(&cfg.skills_dir).with_extension("sqlite"),
    )?);
    tracing::info!("技能目录: {}（已载入 {} 条）", cfg.skills_dir, skills.list().len());

    // 广告保活
    let ads = Arc::new(AdRefresher::new(client.clone(), cfg.clone()));

    // 内置提示词/技能
    let prompts = Arc::new(freebuff2api::prompts::PromptManager::new());

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
        skills,
        ads,
        prompts,
        started: std::time::Instant::now(),
    };

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

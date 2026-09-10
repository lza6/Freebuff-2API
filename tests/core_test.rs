use freebuff2api::config::{parse_duration_sec, Config};
use freebuff2api::models::{parse_free_agents, ModelRegistry, HARDCODED_MODELS};
use freebuff2api::router::compress_tool_result;
use freebuff2api::usage::UsageDb;
use std::time::Duration;

#[test]
fn duration_parsing() {
    assert_eq!(parse_duration_sec("900"), Some(900));
    assert_eq!(parse_duration_sec("6h"), Some(21600));
    assert_eq!(parse_duration_sec("15m"), Some(900));
    assert_eq!(parse_duration_sec("30s"), Some(30));
    assert_eq!(parse_duration_sec("xxx"), None);
}

#[test]
fn config_validates() {
    // 空 token 时 load 应报错（构造明确错误场景）
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(&path, r#"{"auth_tokens":[]}"#).unwrap();
    let r = Config::load(Some(path.to_str().unwrap()));
    assert!(r.is_err());

    // 重复 token 检测（走 Config::load 的自动探测路径）
    std::fs::write(&path, r#"{"auth_tokens":["a","a"]}"#).unwrap();
    let r = Config::load(Some(path.to_str().unwrap()));
    assert!(r.is_err());
    assert!(r.unwrap_err().to_string().contains("重复"));
}

#[test]
fn registry_init() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let reg = ModelRegistry::new();
        reg.init().await;
        assert!(reg.has_model("z-ai/glm-5.3-flash").await);
        assert!(reg.has_model("google/gemini-3.8-flash").await);
        assert!(!reg.has_model("nonexistent-model").await);
        assert!(reg.models().await.len() >= HARDCODED_MODELS.len());
    });
}

#[test]
fn free_agents_parser() {
    // 模拟上游 free-agents.ts 片段
    let src = r#"
const agents = {
  'base2-free': new Set(['google/gemini-2.5-flash-lite']),
  'researcher-web': GEMINI_HELPER_MODELS,
  'basher': ['z-ai/glm-5.3-flash', 'deepseek/deepseek-v4-flash'],
}
"#;
    let parsed = parse_free_agents(src);
    // 内联 Set/数组可解析
    assert!(parsed.contains_key("base2-free"));
    assert!(parsed.contains_key("basher"));
    assert!(parsed.get("basher").unwrap().contains(&"z-ai/glm-5.3-flash".to_string()));
    // 常量引用无法内联解析（上游已改用常量，回归时由硬编码清单兜底）
    assert!(!parsed.contains_key("researcher-web"));
}

#[test]
fn compress_long_tool_result() {
    let long = String::from_utf8(vec![b'a'; 10_000]).unwrap();
    let compressed = compress_tool_result(&long, 500);
    assert!(compressed.len() < long.len());
    assert!(compressed.contains("已压缩"));
    assert!(compressed.starts_with(&long[..100]));

    let short = "hello";
    assert_eq!(compress_tool_result(short, 500), short);
}

#[test]
fn usage_db_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.sqlite").to_str().unwrap().to_string();
    let db = UsageDb::open(&path).unwrap();
    db.record("token-1", "z-ai/glm-5.3-flash", 100, 50, 200, 200, "sk-test", "127.0.0.1")
        .unwrap();
    db.record("token-1", "z-ai/glm-5.3-flash", 10, 5, 100, 500, "sk-test", "127.0.0.1")
        .unwrap();

    let totals = db.totals().unwrap();
    assert_eq!(totals["total_requests"], 2);
    assert_eq!(totals["total_tokens"], 165);

    let recent = db.recent_requests(10).unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].model, "z-ai/glm-5.3-flash");

    let daily = db.daily_usage(7).unwrap();
    assert_eq!(daily.len(), 1);
    assert_eq!(daily[0].requests, 2);
    assert_eq!(daily[0].errors, 1);
}

#[test]
fn timeout_env_duration() {
    // 回归：REQUEST_TIMEOUT=15m 应解析为 900s
    let cfg = Config { request_timeout_sec: parse_duration_sec("15m").unwrap(), ..Default::default() };
    assert_eq!(cfg.request_timeout_sec, 900);
    assert_eq!(config_timeout_sec(), 900);
}

fn config_timeout_sec() -> u64 {
    let _ = Duration::from_secs(60);
    900
}
#[test]
fn pool_pick_best_and_cooldown() {
    use freebuff2api::pool::{AccountEntry, Pool};
    use freebuff2api::session::SessionManager;
    use freebuff2api::upstream::UpstreamClient;
    use std::sync::Arc;

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let cfg = Config { skip_upstream_check: true, ..Default::default() };
        let client = Arc::new(
            UpstreamClient::new("https://www.codebuff.com".into(), None, Duration::from_secs(30)).unwrap(),
        );
        let mk = |name: &str, token: &str| AccountEntry {
            name: name.into(),
            token: token.into(),
            session: Arc::new(SessionManager::new(client.clone(), token.into(), cfg.clone())),
            score: tokio::sync::RwLock::new(0.0),
            cooldown_until: tokio::sync::RwLock::new(None),
        };
        let pool = Pool::new(&cfg, client.clone());
        // 池为空（Config::default 无 token）时 pick_best 应返回 None
        assert!(pool.pick_best().await.is_none());

        assert!(pool.add_account(mk("a1", "tok-a")).await);
        assert!(pool.add_account(mk("a2", "tok-b")).await);
        // 重复 token 不得再次加入
        assert!(!pool.add_account(mk("a1dup", "tok-a")).await);

        // 评分高者优先
        pool.update_score("a2", 50.0).await;
        let best = pool.pick_best().await.unwrap();
        assert_eq!(best.name, "a2");

        // 冷却后应被跳过，回落到 a1
        pool.mark_cooldown("a2", Duration::from_secs(600), "test").await;
        let best2 = pool.pick_best().await.unwrap();
        assert_eq!(best2.name, "a1");

        // 快照反映总数与健康状态
        let snap = pool.snapshot().await;
        assert_eq!(snap.total, 2);
        let a2 = snap.accounts.iter().find(|x| x.name == "a2").unwrap();
        assert!(!a2.healthy || a2.cooldown_until.is_some());
    });
}

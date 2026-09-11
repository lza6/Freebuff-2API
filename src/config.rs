use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::env;
use std::str::FromStr;
use std::time::Duration;

/// 全局配置，JSON 文件 + 环境变量双来源（环境变量优先）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 监听地址，默认 127.0.0.1:47821（本地软件默认仅本机）
    pub listen_addr: String,
    /// 上游 API 地址
    pub upstream_base_url: String,
    /// Freebuff 认证 token（多账号轮询）
    pub auth_tokens: Vec<String>,
    /// 本网关对外鉴权 key（空则不校验）
    pub api_keys: Vec<String>,
    /// run 轮换间隔
    pub rotation_interval_sec: u64,
    /// 上游请求超时
    pub request_timeout_sec: u64,
    /// HTTP 代理（支持 http/socks5）
    pub http_proxy: String,
    /// 会话保活间隔（广告刷新 / 心跳）
    pub session_keepalive_sec: u64,
    /// 广告保活 provider（逗号分隔：gravity,zeroclick,carbon）
    pub ad_providers: Vec<String>,
    /// 模型路由降级链配置
    pub fallback_models: Vec<String>,
    /// 是否启用 token 节省（压缩超长 tool_result）
    pub token_saver: bool,
    /// 用量统计 SQLite 路径（空则禁用统计）
    pub sqlite_path: String,
    /// 导入凭证存储路径（curl/HAR/Cookie 解析后落盘位置）
    pub tokens_path: String,
    /// 遥测 SQLite 路径（请求详情/事件链，独立库避免写锁竞争）
    pub telemetry_path: String,
    /// 记忆库 SQLite 路径（用户偏好/纠正；独立库）
    pub memory_path: String,
    /// 上游会话记录（web 协议 threadId；供自动清理）
    pub threads_path: String,
    /// 凭证账号信息缓存（昵称/邮箱/套餐/今日剩余，面板凭证列表用）
    pub cred_meta_path: String,
    /// 账号使用记录（JSONL，按凭证可查历史）
    pub account_history_path: String,
    /// 上游会话自动清理间隔（秒）；0 = 关闭自动清理
    pub thread_cleanup_interval_sec: u64,
    /// 上游会话保留时长（小时），超过即清理
    pub thread_max_age_hours: u64,
    /// web 协议桥接的会话绑定（OpenAI/Anthropic 客户端 → 上游 thread 复用）
    pub web_threads_path: String,
    /// 记忆层开关（false 时既不自动记录也不注入；隐私敏感用户可关）
    pub memory_enabled: bool,
    /// 技能目录（技能文件真相源）
    pub skills_dir: String,
    /// 技能注入模式：roster（只注入名称+描述）| full（全量拼接）
    pub skills_inject_mode: String,
    /// roster 注入的 token 预算上限
    pub max_roster_tokens: usize,
    /// 内置面板目录（空则用嵌入资源）
    pub web_dir: String,
    /// 启动时跳过上游连通性检查
    pub skip_upstream_check: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:47821".into(),
            upstream_base_url: "https://www.codebuff.com".into(),
            auth_tokens: vec![],
            api_keys: vec![],
            rotation_interval_sec: 6 * 3600,
            request_timeout_sec: 900,
            http_proxy: String::new(),
            session_keepalive_sec: 45,
            ad_providers: vec!["gravity".into()],
            fallback_models: vec![],
            token_saver: false,
            sqlite_path: "data/freebuff2api.sqlite".into(),
            tokens_path: "data/tokens.json".into(),
            telemetry_path: "data/telemetry.sqlite".into(),
            memory_path: "data/memory.sqlite".into(),
            threads_path: "data/threads.json".into(),
            cred_meta_path: "data/cred_meta.json".into(),
            account_history_path: "data/account_history.jsonl".into(),
            // 用户批注（网页对话.txt:605）：反代要自己清理上游会话，别把压力留给上游被查出来
            thread_cleanup_interval_sec: 3600,
            thread_max_age_hours: 24,
            web_threads_path: "data/web_threads.json".into(),
            // 记忆默认关闭（用户批注 2026-09-11：记忆不是每个人都需要的，要有单独开关且默认关）
            memory_enabled: false,
            skills_dir: "data/skills".into(),
            skills_inject_mode: "roster".into(),
            max_roster_tokens: 2000,
            web_dir: String::new(),
            skip_upstream_check: false,
        }
    }
}

impl Config {
    /// 从默认值 + JSON 文件 + 环境变量合并加载
    pub fn load(path: Option<&str>) -> Result<Self> {
        let mut cfg = Self::default();

        if let Some(p) = path {
            if std::path::Path::new(p).exists() {
                let data = std::fs::read_to_string(p)
                    .map_err(|e| anyhow!("读取配置文件 {p} 失败: {e}"))?;
                let file_cfg: Config = serde_json::from_str(&data)
                    .map_err(|e| anyhow!("解析配置文件 {p} 失败: {e}"))?;
                cfg = file_cfg;
            } else {
                return Err(anyhow!("配置文件不存在: {p}"));
            }
        }
        // 自动探测默认 config.json
        else if std::path::Path::new("config.json").exists() {
            let data = std::fs::read_to_string("config.json")?;
            cfg = serde_json::from_str(&data)?;
        }

        cfg.apply_env();
        cfg.validate()?;
        Ok(cfg)
    }

    fn apply_env(&mut self) {
        if let Ok(v) = env::var("LISTEN_ADDR") {
            self.listen_addr = v;
        }
        if let Ok(v) = env::var("UPSTREAM_BASE_URL") {
            self.upstream_base_url = v;
        }
        if let Ok(v) = env::var("AUTH_TOKENS") {
            self.auth_tokens = v.split([',', '\n']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        }
        if let Ok(v) = env::var("API_KEYS") {
            self.api_keys = v.split([',', '\n']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        }
        if let Ok(v) = env::var("ROTATION_INTERVAL") {
            self.rotation_interval_sec = parse_duration_sec(&v).unwrap_or(self.rotation_interval_sec);
        }
        if let Ok(v) = env::var("REQUEST_TIMEOUT") {
            self.request_timeout_sec = parse_duration_sec(&v).unwrap_or(self.request_timeout_sec);
        }
        if let Ok(v) = env::var("HTTP_PROXY") {
            self.http_proxy = v;
        }
        if let Ok(v) = env::var("AD_PROVIDERS") {
            self.ad_providers = v.split([',', '\n']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        }
        if let Ok(v) = env::var("SQLITE_PATH") {
            self.sqlite_path = v;
        }
        if let Ok(v) = env::var("TOKENS_PATH") {
            self.tokens_path = v;
        }
        if let Ok(v) = env::var("TELEMETRY_PATH") {
            self.telemetry_path = v;
        }
        if let Ok(v) = env::var("MEMORY_PATH") {
            self.memory_path = v;
        }
        if let Ok(v) = env::var("CRED_META_PATH") {
            self.cred_meta_path = v;
        }
        if let Ok(v) = env::var("ACCOUNT_HISTORY_PATH") {
            self.account_history_path = v;
        }
        if let Ok(v) = env::var("THREAD_CLEANUP_INTERVAL") {
            self.thread_cleanup_interval_sec = parse_duration_sec(&v).unwrap_or(self.thread_cleanup_interval_sec);
        }
        if let Ok(v) = env::var("THREAD_MAX_AGE_HOURS") {
            if let Ok(n) = v.parse() {
                self.thread_max_age_hours = n;
            }
        }
        if let Ok(v) = env::var("WEB_THREADS_PATH") {
            self.web_threads_path = v;
        }
        if let Ok(v) = env::var("MEMORY_ENABLED") {
            self.memory_enabled = v == "1" || v.eq_ignore_ascii_case("true");
        }
        if let Ok(v) = env::var("SKILLS_DIR") {
            self.skills_dir = v;
        }
        if let Ok(v) = env::var("SKILLS_INJECT_MODE") {
            self.skills_inject_mode = v;
        }
        if let Ok(v) = env::var("MAX_ROSTER_TOKENS") {
            if let Ok(n) = v.parse() {
                self.max_roster_tokens = n;
            }
        }
        if let Ok(v) = env::var("WEB_DIR") {
            self.web_dir = v;
        }
        if let Ok(v) = env::var("TOKEN_SAVER") {
            self.token_saver = v == "1" || v.eq_ignore_ascii_case("true");
        }
        if let Ok(v) = env::var("FALLBACK_MODELS") {
            self.fallback_models = v.split([',', '\n']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        }
    }

    fn validate(&self) -> Result<()> {
        if self.listen_addr.trim().is_empty() {
            return Err(anyhow!("LISTEN_ADDR 不能为空"));
        }
        // 安全守卫：监听非本机地址时必须配置 api_keys（否则管理端点/记忆/凭证对网络裸奔）
        if !is_loopback_listen(&self.listen_addr) && self.api_keys.is_empty() {
            return Err(anyhow!(
                "安全拒绝：listen_addr={} 不是本机地址，但未配置 api_keys。\
                 请配置 api_keys（推荐）或改回 127.0.0.1",
                self.listen_addr
            ));
        }
        if self.upstream_base_url.trim().is_empty() {
            return Err(anyhow!("UPSTREAM_BASE_URL 不能为空"));
        }
        if self.auth_tokens.is_empty() && !self.skip_upstream_check {
            return Err(anyhow!("至少需要一个 AUTH_TOKENS"));
        }
        let unique: HashSet<&String> = self.auth_tokens.iter().collect();
        if unique.len() != self.auth_tokens.len() {
            return Err(anyhow!("AUTH_TOKENS 存在重复 token"));
        }
        Ok(())
    }
}

/// 判断监听地址是否为本机（host 部分精确匹配，防 `localhost.evil.com` 这类前缀绕过）。
pub fn is_loopback_listen(listen_addr: &str) -> bool {
    // host[:port] → host；[::1]:port → ::1
    let host = if let Some(rest) = listen_addr.strip_prefix('[') {
        rest.split(']').next().unwrap_or("").to_string()
    } else {
        listen_addr
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(listen_addr)
            .to_string()
    };
    matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1" | "[::1]")
        || std::net::IpAddr::from_str(&host).map(|ip| ip.is_loopback()).unwrap_or(false)
}

/// 解析配置文件路径：`--config x.json` > 第一个位置参数 > 当前目录 config.json > None
///
/// 与启动时 `Config::load` 的取舍保持一致，供"运行时写回配置"（如面板一键生成 API Key）复用。
pub fn resolve_config_path() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--config") {
        if let Some(p) = args.get(i + 1) {
            return Some(p.clone());
        }
    }
    if let Some(a) = args.get(1) {
        if !a.starts_with("--") {
            return Some(a.clone());
        }
    }
    if std::path::Path::new("config.json").exists() {
        return Some("config.json".into());
    }
    None
}

/// 解析 "6h" / "900s" / "15m" 或纯秒数
pub fn parse_duration_sec(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    if let Ok(secs) = raw.parse::<u64>() {
        return Some(secs);
    }
    let (num, unit) = raw.split_at(raw.len().saturating_sub(1));
    let n: u64 = num.trim().parse().ok()?;
    Some(match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86400,
        _ => return None,
    })
}

pub fn default_request_timeout() -> Duration {
    Duration::from_secs(900)
}

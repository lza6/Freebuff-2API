use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::env;
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

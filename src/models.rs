//! 模型注册表：上游 free-agents.ts 拉取 + 硬编码权威底座
//!
//! 逆向自上游（Freebuff-0.0.98 orchestrator.js）：
//! - SUPPORTED_FREEBUFF_MODELS 清单
//! - 默认免费模型 z-ai/glm-5.3-flash
//! - 每账号 rateLimitsByModel 决定实际可用

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 硬编码权威底座（上游实测可用）
pub const ROOT_AGENT_ID: &str = "base2-free";

pub const HARDCODED_MODELS: &[&str] = &[
    "z-ai/glm-5.3-flash",
    "google/gemini-3.8-flash",
    "google/gemini-3.1-flash-lite",
    "google/gemini-3.5-flash-lite",
    "deepseek/deepseek-v4-flash",
    "deepseek/deepseek-v4-flash-max",
    "deepseek/deepseek-v4-pro",
    "deepseek/deepseek-v4-pro-max",
    "minimax/minimax-m3",
    "openai/gpt-5.6-luna",
    "openai/gpt-5.6-luna-es",
    "openai/gpt-5.6-luna-max",
    "upstage/solar-pro4",
    "meta/muse-spark-1.2-contributor",
    "meta/muse-spark-1.3-contributor",
    "anthropic/claude-fable-5",
    "stealth/ox-alpha",
    "crof/kimi-k3-eco",
    "z-ai/glm-5.2",
];

/// 子代理 agent 映射（run 层级）
pub const SUB_AGENTS: &[(&str, &str)] = &[
    ("file-picker", "google/gemini-3.1-flash-lite"),
    ("researcher-web", "google/gemini-3.8-flash"),
    ("basher", "google/gemini-3.8-flash"),
    ("browser-use", "google/gemini-3.8-flash"),
];

/// 默认模型
pub const DEFAULT_MODEL: &str = "z-ai/glm-5.3-flash";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub agent: String,
    pub premium: bool,
}

#[derive(Debug, Clone)]
pub struct ModelRegistry {
    inner: Arc<RwLock<RegistryInner>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RegistryInner {
    /// model -> agent
    model_to_agent: HashMap<String, String>,
    /// agent -> models
    agent_models: HashMap<String, Vec<String>>,
    all_models: Vec<String>,
    updated_at: Option<String>,
}

impl ModelRegistry {
    pub fn new() -> Self {
        let inner = RegistryInner::default();
        Self { inner: Arc::new(RwLock::new(inner)) }
    }

    /// 以硬编码为底座初始化
    pub async fn init(&self) {
        let mut inner = self.inner.write().await;
        for m in HARDCODED_MODELS {
            inner
                .model_to_agent
                .entry(m.to_string())
                .or_insert_with(|| ROOT_AGENT_ID.to_string());
        }
        inner
            .agent_models
            .entry(ROOT_AGENT_ID.to_string())
            .or_insert_with(|| HARDCODED_MODELS.iter().map(|s| s.to_string()).collect());
        for (agent, model) in SUB_AGENTS {
            inner
                .model_to_agent
                .entry(model.to_string())
                .or_insert_with(|| agent.to_string());
            inner
                .agent_models
                .entry(agent.to_string())
                .or_insert_with(|| vec![model.to_string()]);
        }
        let mut all: Vec<String> = inner.model_to_agent.keys().cloned().collect();
        all.sort();
        inner.all_models = all;
        inner.updated_at = Some(now_iso());
    }

    /// 拉取上游 free-agents.ts 增量补充
    pub async fn refresh_from_upstream(&self, client: &reqwest::Client) -> Result<(usize, usize), anyhow::Error> {
        const SRC: &str = "https://raw.githubusercontent.com/CodebuffAI/codebuff/main/common/src/constants/free-agents.ts";
        let resp = client.get(SRC).send().await?;
        if !resp.status().is_success() {
            return Ok((0, 0));
        }
        let text = resp.text().await?;
        let parsed = parse_free_agents(&text);
        if parsed.is_empty() {
            return Ok((0, 0));
        }
        let mut inner = self.inner.write().await;
        let mut added = 0;
        let mut removed = 0;
        // 合并硬编码底座（权威）+ 上游最新解析
        let mut merged = hardcoded_fallback_map();
        for (agent, models) in parsed {
            merged.entry(agent).or_default().extend(models);
        }
        // 重建 model→agent：以上游为准，不在上游也不在硬编码的移除
        let mut new_model_to_agent = std::collections::HashMap::new();
        for (agent, models) in &merged {
            for model in models {
                if !new_model_to_agent.contains_key(model) {
                    new_model_to_agent.insert(model.clone(), agent.clone());
                }
            }
        }
        // 计算增/删
        for m in new_model_to_agent.keys() {
            if !inner.model_to_agent.contains_key(m) {
                added += 1;
            }
        }
        for m in inner.model_to_agent.keys() {
            if !new_model_to_agent.contains_key(m) {
                removed += 1;
                tracing::warn!("模型 {m} 已从上游移除，同时不在硬编码底座，从注册表下架");
            }
        }
        inner.model_to_agent = new_model_to_agent;
        inner.agent_models = merged;
        let mut all: Vec<String> = inner.model_to_agent.keys().cloned().collect();
        all.sort();
        inner.all_models = all;
        inner.updated_at = Some(now_iso());
        Ok((added, removed))
    }

    pub async fn has_model(&self, model: &str) -> bool {
        self.inner.read().await.model_to_agent.contains_key(model)
    }

    pub async fn agent_for(&self, model: &str) -> Option<String> {
        self.inner.read().await.model_to_agent.get(model).cloned()
    }

    pub async fn models(&self) -> Vec<String> {
        self.inner.read().await.all_models.clone()
    }

    /// 同步读取（供非 async 场景，如路由解析）
    pub fn models_sync(&self) -> Vec<String> {
        // fallback：从硬编码清单取
        HARDCODED_MODELS.iter().map(|s| s.to_string()).collect()
    }

    pub async fn snapshot(&self) -> ModelRegistrySnapshot {
        let inner = self.inner.read().await;
        ModelRegistrySnapshot {
            model_count: inner.model_to_agent.len(),
            agent_count: inner.agent_models.len(),
            all_models: inner.all_models.clone(),
            updated_at: inner.updated_at.clone(),
        }
    }
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRegistrySnapshot {
    pub model_count: usize,
    pub agent_count: usize,
    pub all_models: Vec<String>,
    pub updated_at: Option<String>,
}

/// 硬编码底座 map（权威不随上游消失）
fn hardcoded_fallback_map() -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    map.insert(
        ROOT_AGENT_ID.to_string(),
        HARDCODED_MODELS.iter().map(|s| s.to_string()).collect(),
    );
    for (agent, model) in SUB_AGENTS {
        map.entry(agent.to_string()).or_default().push(model.to_string());
    }
    map
}

/// 解析上游 free-agents.ts 的 agent→models 映射
pub fn parse_free_agents(source: &str) -> HashMap<String, Vec<String>> {
    // 支持三种形态：new Set([...]) / 数组 [...] / 常量引用（无法解析，跳过）
    let block = Regex::new(r"'([^']+)':\s*(?:new\s+Set\(\s*)?\[([^\]]*)\]")
        .unwrap();
    let model = Regex::new(r"'([^']+)'").unwrap();
    let mut result = HashMap::new();
    for cap in block.captures_iter(source) {
        let agent = cap[1].to_string();
        let models_str = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let models: Vec<String> = model
            .captures_iter(models_str)
            .map(|m| m[1].to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !models.is_empty() {
            result.insert(agent, models);
        }
    }
    result
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

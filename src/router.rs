//! 模型路由 + 降级链 + token 节省（9router 式）
//!
//! - 主模型失败自动降级到 fallback_models 中的备选
//! - 按 free 配额/成本选择最优模型
//! - token 节省：压缩超长 tool_result（可选，默认关闭）

use crate::models::ModelRegistry;
use anyhow::Result;
use std::sync::Arc;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RouterConfig {
    /// 冻结的降级链（用户自选）
    pub fallback_chain: Vec<String>,
    /// 配额感知（按账号 rateLimitsByModel 剩余）
    pub quota_aware: bool,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            fallback_chain: vec![
                "z-ai/glm-5.3-flash".into(),
                "google/gemini-3.8-flash".into(),
                "google/gemini-3.1-flash-lite".into(),
                "deepseek/deepseek-v4-flash".into(),
            ],
            quota_aware: true,
        }
    }
}

pub struct ModelRouter {
    pub config: RouterConfig,
    pub registry: Arc<ModelRegistry>,
}

impl ModelRouter {
    pub fn new(registry: Arc<ModelRegistry>, config: RouterConfig) -> Self {
        Self { config, registry }
    }

    /// 解析请求模型 → 实际可用模型（含降级链）
    pub async fn resolve(&self, requested: &str) -> String {
        if self.registry.has_model(requested).await {
            return requested.to_string();
        }
        // 请求模型不可用 → 走降级链第一条可用
        for m in &self.config.fallback_chain {
            if self.registry.has_model(m).await {
                return m.clone();
            }
        }
        crate::models::DEFAULT_MODEL.to_string()
    }

    /// 检查某模型是否可直接用（杜绝幻觉）
    pub fn is_available(&self, model: &str) -> bool {
        self.registry.models_sync().contains(&model.to_string())
    }

    /// 模型是否支持 reasoning_effort（思考程度），逆向自上游 orchestrator.js efforts 字段
    /// 支持清单（efforts 非空）：deepseek 系/glm 系/gpt-5.6/gemini-3.8/fable-5/ox-alpha/muse-spark
    /// 不支持（无 efforts 字段）：solar-pro4 / minimax-m3 / mimo-v2.5 / kimi-k3
    pub fn supports_reasoning(&self, model: &str) -> bool {
        // 支持 efforts 的模型前缀
        const SUPPORTED: &[&str] = &[
            "deepseek/",
            "z-ai/glm",
            "openai/gpt-5.6",
            "google/gemini-3.8",
            "anthropic/claude-fable",
            "stealth/ox-alpha",
            "meta/muse-spark",
        ];
        SUPPORTED.iter().any(|p| model.starts_with(p))
    }

    /// 模型支持的 efforts 范围（逆向自上游常量）；None = 不支持
    pub fn reasoning_efforts(&self, model: &str) -> Option<Vec<&'static str>> {
        if model.starts_with("deepseek/") || model.starts_with("z-ai/glm") || model.starts_with("stealth/ox-alpha") {
            Some(vec!["low", "high", "max"])
        } else if model.starts_with("openai/gpt-5.6") || model.starts_with("google/gemini-3.8") || model.starts_with("anthropic/claude-fable") {
            Some(vec!["low", "medium", "high", "xhigh", "max"])
        } else if model.starts_with("meta/muse-spark") {
            Some(vec!["minimal", "low", "medium", "high", "xhigh"])
        } else {
            None
        }
    }

    /// 校正 effort：不支持或超范围时降级到最近支持值
    pub fn clamp_effort(&self, model: &str, requested: &str) -> Option<String> {
        let efforts = self.reasoning_efforts(model)?;
        let requested = requested.to_lowercase();
        if efforts.contains(&requested.as_str()) {
            return Some(requested);
        }
        // 超出范围：取 requests 同档 max → 支持上限
        if requested == "max" || requested == "xhigh" || requested == "high" {
            return Some(efforts.last().unwrap().to_string());
        }
        if requested == "minimal" || requested == "low" {
            return Some(efforts.first().unwrap().to_string());
        }
        Some(efforts.first().unwrap().to_string())
    }
}

/// 超长 tool_result 压缩（token 节省核心）
pub fn compress_tool_result(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }
    let head = &content[..max_chars / 2];
    let tail = &content[content.len() - max_chars / 2..];
    format!(
        "{head}\n\n[... 已压缩: 原始 {} 字符，保留首尾 {} 字符 ...]\n\n{tail}",
        content.len(),
        max_chars
    )
}

#[allow(dead_code)]
pub fn truncate_to_tokens(s: &str, approx_tokens: usize) -> String {
    // 粗略估算：每 token ≈ 4 字符（英文）
    let max_chars = approx_tokens * 4;
    compress_tool_result(s, max_chars)
}

#[allow(dead_code)]
async fn plausible_model(_registry: &ModelRegistry, _name: &str) -> Result<bool> {
    Ok(true)
}
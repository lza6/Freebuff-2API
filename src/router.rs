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
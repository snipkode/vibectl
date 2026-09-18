use super::provider::Provider;
use super::{anthropic::AnthropicProvider, ollama::Ollama, openai::OpenAICompatible};
use anyhow::Result;
use std::sync::Arc;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ProviderConfig {
    #[serde(default)]
    pub openai_api_key: Option<String>,
    #[serde(default)]
    pub anthropic_api_key: Option<String>,
    #[serde(default)]
    pub ollama_base_url: Option<String>,
    #[serde(default)]
    pub custom_providers: Vec<CustomProvider>,
    #[serde(default)]
    pub default_model: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CustomProvider {
    pub name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub models: Vec<String>,
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

pub fn resolve_provider(cfg: &ProviderConfig, model: &str) -> Result<Arc<dyn Provider>> {
    let lower = model.to_lowercase();
    if let Some(prefix) = lower.split_once('-').map(|(p, _)| p.to_string()) {
        match prefix.as_str() {
            "claude" => {
                let key = cfg
                    .anthropic_api_key
                    .clone()
                    .or_else(|| env_var("ANTHROPIC_API_KEY"));
                let key = key.ok_or_else(|| {
                    anyhow::anyhow!(
                        "Anthropic model selected but ANTHROPIC_API_KEY is not set.\n\
                     Set it with:  export ANTHROPIC_API_KEY=sk-ant-..."
                    )
                })?;
                return Ok(Arc::new(AnthropicProvider::new(key)));
            }
            "gpt" => {
                let key = cfg
                    .openai_api_key
                    .clone()
                    .or_else(|| env_var("OPENAI_API_KEY"));
                let key = key.ok_or_else(|| {
                    anyhow::anyhow!(
                        "OpenAI model selected but OPENAI_API_KEY is not set.\n\
                     Set it with:  export OPENAI_API_KEY=sk-..."
                    )
                })?;
                return Ok(Arc::new(OpenAICompatible::openai_default(key)));
            }
            _ => {}
        }
    }

    for provider in &cfg.custom_providers {
        if provider.models.iter().any(|m| m == model) {
            return Ok(Arc::new(OpenAICompatible::new(
                provider.name.clone(),
                provider.base_url.clone(),
                provider.api_key.clone(),
            )));
        }
    }

    let base = cfg
        .ollama_base_url
        .clone()
        .or_else(|| env_var("OLLAMA_URL"))
        .unwrap_or_else(|| "http://localhost:11434".to_string());
    Ok(Ollama::provider(base))
}

pub fn provider_name(cfg: &ProviderConfig, model: &str) -> String {
    if model.starts_with("claude") {
        "anthropic".to_string()
    } else if model.starts_with("gpt") {
        "openai".to_string()
    } else if cfg
        .custom_providers
        .iter()
        .any(|p| p.models.contains(&model.to_string()))
    {
        cfg.custom_providers
            .iter()
            .find(|p| p.models.contains(&model.to_string()))
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "ollama".to_string())
    } else {
        "ollama".to_string()
    }
}

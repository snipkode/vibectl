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
    // Must mirror resolve_provider's case handling, or the header shows the
    // wrong provider for a model like "GPT-4o" that actually routes to OpenAI.
    let lower = model.to_lowercase();
    if lower.starts_with("claude") {
        "anthropic".to_string()
    } else if lower.starts_with("gpt") {
        "openai".to_string()
    } else if cfg
        .custom_providers
        .iter()
        .any(|p| p.models.iter().any(|m| m.to_lowercase() == lower))
    {
        cfg.custom_providers
            .iter()
            .find(|p| p.models.iter().any(|m| m.to_lowercase() == lower))
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "ollama".to_string())
    } else {
        "ollama".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ProviderConfig {
        ProviderConfig {
            openai_api_key: Some("sk-test".into()),
            anthropic_api_key: Some("sk-ant-test".into()),
            ..Default::default()
        }
    }

    #[test]
    fn model_prefix_selects_the_provider() {
        assert_eq!(
            provider_name(&cfg(), "claude-sonnet-4-20250514"),
            "anthropic"
        );
        assert_eq!(provider_name(&cfg(), "gpt-4o"), "openai");
        assert_eq!(provider_name(&cfg(), "llama3.2"), "ollama");
    }

    #[test]
    fn prefix_matching_is_case_insensitive() {
        assert_eq!(provider_name(&cfg(), "GPT-4o"), "openai");
        assert_eq!(provider_name(&cfg(), "Claude-3"), "anthropic");
    }

    #[test]
    fn custom_provider_wins_for_its_declared_models() {
        let mut c = cfg();
        c.custom_providers = vec![CustomProvider {
            name: "internal".into(),
            base_url: "http://llm.internal/v1".into(),
            api_key: None,
            models: vec!["house-model".into()],
        }];
        assert_eq!(provider_name(&c, "house-model"), "internal");
        assert_eq!(
            provider_name(&c, "gpt-4o"),
            "openai",
            "others are unaffected"
        );
    }

    #[test]
    fn unknown_model_falls_back_to_ollama() {
        assert_eq!(provider_name(&cfg(), "some-unknown-model"), "ollama");
    }

    #[test]
    fn claude_without_a_key_is_a_clear_error() {
        let empty = ProviderConfig::default();
        // Guard against a stray ANTHROPIC_API_KEY in the test environment.
        if env_var("ANTHROPIC_API_KEY").is_some() {
            return;
        }
        let Err(err) = resolve_provider(&empty, "claude-sonnet-4") else {
            panic!("expected a missing-key error");
        };
        let msg = err.to_string();
        assert!(msg.contains("ANTHROPIC_API_KEY"), "got: {msg}");
    }

    #[test]
    fn gpt_without_a_key_is_a_clear_error() {
        let empty = ProviderConfig::default();
        if env_var("OPENAI_API_KEY").is_some() {
            return;
        }
        let Err(err) = resolve_provider(&empty, "gpt-4o") else {
            panic!("expected a missing-key error");
        };
        let msg = err.to_string();
        assert!(msg.contains("OPENAI_API_KEY"), "got: {msg}");
    }

    #[test]
    fn ollama_never_needs_a_key() {
        // The zero-config path: a fresh install must run with no credentials.
        let provider =
            resolve_provider(&ProviderConfig::default(), "llama3.2").expect("ollama needs no key");
        assert_eq!(provider.name(), "ollama");
    }

    #[test]
    fn provider_construction_does_not_require_network_access() {
        // Resolution must stay offline so the TUI can start without a network.
        let provider = resolve_provider(&cfg(), "gpt-4o").expect("resolve");
        assert_eq!(provider.name(), "openai");
        let provider = resolve_provider(&cfg(), "claude-sonnet-4").expect("resolve");
        assert_eq!(provider.name(), "anthropic");
    }

    #[test]
    fn env_var_treats_empty_as_unset() {
        // An exported-but-empty key is a common footgun; it must not be used.
        let key = "VIBECTL_TEST_EMPTY_VAR";
        unsafe { std::env::set_var(key, "") };
        assert_eq!(env_var(key), None);
        unsafe { std::env::set_var(key, "value") };
        assert_eq!(env_var(key).as_deref(), Some("value"));
        unsafe { std::env::remove_var(key) };
        assert_eq!(env_var(key), None);
    }
}

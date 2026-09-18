use super::openai::OpenAICompatible;
use super::provider::Provider;
use std::sync::Arc;

pub struct Ollama;

impl Ollama {
    pub fn provider(base_url: impl Into<String>) -> Arc<dyn Provider> {
        let mut url = base_url.into();
        if !url.ends_with('/') {
            url.push('/');
        }
        url.push_str("v1");
        Arc::new(OpenAICompatible::new(
            "ollama",
            url,
            Some("ollama".to_string()),
        ))
    }

    #[allow(dead_code)]
    pub fn default() -> Arc<dyn Provider> {
        Self::provider("http://localhost:11434")
    }
}

use crate::llm::{CustomProvider, ProviderConfig};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Config {
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub temperature: f32,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub allow_any_path: bool,
    #[serde(flatten)]
    pub providers: ProviderConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: default_model(),
            temperature: 0.2,
            max_tokens: Some(4096),
            system_prompt: None,
            allow_any_path: false,
            providers: ProviderConfig::default(),
        }
    }
}

/// A project-level config file, merged *over* the global one.
///
/// Every field is optional on purpose.  Merging a fully-defaulted `Config`
/// would silently reset anything the file did not mention — a project file
/// setting only `temperature` used to overwrite the user's global model with
/// the struct default.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ConfigOverlay {
    pub model: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub system_prompt: Option<String>,
    pub allow_any_path: Option<bool>,
    pub openai_api_key: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub ollama_base_url: Option<String>,
    pub default_model: Option<String>,
    pub custom_providers: Option<Vec<CustomProvider>>,
}

impl ConfigOverlay {
    /// Parse a project config file. Unknown keys are ignored so a config
    /// written for a newer vibectl still loads.
    pub fn parse(raw: &str) -> Result<Self> {
        serde_yaml::from_str(raw).context("invalid config YAML")
    }

    /// Apply the overlay in place. Project values win; unset fields are left
    /// exactly as they were.
    pub fn apply(&self, base: &mut Config) {
        if let Some(model) = self.model.as_ref().filter(|m| !m.is_empty()) {
            base.model = model.clone();
        }
        if let Some(t) = self.temperature {
            base.temperature = t;
        }
        if let Some(m) = self.max_tokens {
            base.max_tokens = Some(m);
        }
        if self.system_prompt.is_some() {
            base.system_prompt = self.system_prompt.clone();
        }
        // One-way latch: a project can widen the sandbox but never narrow it.
        if self.allow_any_path == Some(true) {
            base.allow_any_path = true;
        }
        let p = &mut base.providers;
        if self.openai_api_key.is_some() {
            p.openai_api_key = self.openai_api_key.clone();
        }
        if self.anthropic_api_key.is_some() {
            p.anthropic_api_key = self.anthropic_api_key.clone();
        }
        if self.ollama_base_url.is_some() {
            p.ollama_base_url = self.ollama_base_url.clone();
        }
        if self.default_model.is_some() {
            p.default_model = self.default_model.clone();
        }
        if let Some(providers) = self.custom_providers.clone() {
            p.custom_providers = providers;
        }
    }
}

fn default_model() -> String {
    "llama3.2".to_string()
}

impl Config {
    pub fn load() -> Result<Config> {
        let path = config_path();
        let mut config = if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read config {}", path.display()))?;
            serde_yaml::from_str(&raw)
                .with_context(|| format!("invalid config YAML in {}", path.display()))?
        } else {
            Self::default()
        };

        if let Some(env_model) = std::env::var("VIBECTL_MODEL")
            .ok()
            .filter(|s| !s.is_empty())
        {
            config.model = env_model;
        }

        Ok(config)
    }

    #[allow(dead_code)]
    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("failed to create config dir")?;
        }
        let raw = serde_yaml::to_string(self).context("failed to serialize config")?;
        std::fs::write(&path, raw).context("failed to write config")?;
        Ok(())
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("vibectl")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.yaml")
}

#[allow(dead_code)]
pub fn project_dir(cwd: &Path) -> Option<PathBuf> {
    let mut dir = Some(cwd.to_path_buf());
    while let Some(d) = dir {
        let marker = d.join(".vibectl");
        if marker.is_dir() {
            return Some(marker);
        }
        if d.join(".git").exists() {
            return Some(marker);
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    None
}

#[allow(dead_code)]
pub fn project_config(cwd: &Path) -> Result<Option<Config>> {
    if let Some(dir) = project_dir(cwd) {
        let file = dir.join("config.yaml");
        if file.exists() {
            let raw = std::fs::read_to_string(&file)
                .with_context(|| format!("failed to read {}", file.display()))?;
            return Ok(Some(
                serde_yaml::from_str(&raw)
                    .with_context(|| format!("invalid config {}", file.display()))?,
            ));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn global() -> Config {
        Config {
            model: "gpt-4o".into(),
            temperature: 0.2,
            max_tokens: Some(4096),
            system_prompt: Some("be terse".into()),
            allow_any_path: false,
            providers: ProviderConfig {
                openai_api_key: Some("sk-global".into()),
                ollama_base_url: Some("http://localhost:11434".into()),
                ..Default::default()
            },
        }
    }

    #[test]
    fn empty_overlay_changes_nothing() {
        let mut base = global();
        let before = base.clone();
        ConfigOverlay::default().apply(&mut base);
        assert_eq!(base.model, before.model);
        assert_eq!(base.temperature, before.temperature);
        assert_eq!(base.max_tokens, before.max_tokens);
        assert_eq!(base.system_prompt, before.system_prompt);
        assert_eq!(
            base.providers.openai_api_key,
            before.providers.openai_api_key
        );
    }

    #[test]
    fn partial_project_config_does_not_reset_the_global_model() {
        // The bug this guards: a project file setting only `temperature` was
        // deserialized into a full Config, whose `model` default then
        // overwrote whatever the user had configured globally.
        let overlay = ConfigOverlay::parse("temperature: 0.9\n").expect("parse");
        let mut base = global();
        overlay.apply(&mut base);

        assert_eq!(
            base.model, "gpt-4o",
            "model must survive an unrelated override"
        );
        assert_eq!(base.temperature, 0.9);
    }

    #[test]
    fn project_config_overrides_the_fields_it_mentions() {
        let overlay = ConfigOverlay::parse(
            "model: claude-sonnet-4-20250514\nmax_tokens: 8192\nopenai_api_key: sk-project\n",
        )
        .expect("parse");
        let mut base = global();
        overlay.apply(&mut base);

        assert_eq!(base.model, "claude-sonnet-4-20250514");
        assert_eq!(base.max_tokens, Some(8192));
        assert_eq!(base.providers.openai_api_key.as_deref(), Some("sk-project"));
    }

    #[test]
    fn an_empty_model_string_is_ignored() {
        let overlay = ConfigOverlay::parse("model: \"\"\n").expect("parse");
        let mut base = global();
        overlay.apply(&mut base);
        assert_eq!(
            base.model, "gpt-4o",
            "an empty model must not clear the global one"
        );
    }

    #[test]
    fn allow_any_path_only_latches_on() {
        // A project config must not be able to *tighten* the sandbox.
        let mut base = global();
        base.allow_any_path = true;
        ConfigOverlay::parse("allow_any_path: false\n")
            .expect("parse")
            .apply(&mut base);
        assert!(
            base.allow_any_path,
            "a project file cannot re-lock the sandbox"
        );
    }

    #[test]
    fn allow_any_path_can_widen_the_sandbox() {
        let mut base = global();
        assert!(!base.allow_any_path);
        ConfigOverlay::parse("allow_any_path: true\n")
            .expect("parse")
            .apply(&mut base);
        assert!(base.allow_any_path);
    }

    #[test]
    fn custom_providers_round_trip() {
        let overlay = ConfigOverlay::parse(
            "custom_providers:\n  - name: local\n    base_url: http://x/v1\n    models: [my-model]\n",
        )
        .expect("parse");
        let mut base = global();
        overlay.apply(&mut base);
        assert_eq!(base.providers.custom_providers.len(), 1);
        assert_eq!(base.providers.custom_providers[0].name, "local");
        assert_eq!(base.providers.custom_providers[0].models, vec!["my-model"]);
    }

    #[test]
    fn unknown_keys_are_ignored_for_forward_compatibility() {
        let overlay = ConfigOverlay::parse("model: gpt-4o\nsome_future_knob: 12\n").expect("parse");
        assert_eq!(overlay.model.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn malformed_yaml_is_an_error_not_a_panic() {
        assert!(ConfigOverlay::parse("model: [unclosed\n").is_err());
    }

    #[test]
    fn default_config_uses_the_documented_defaults() {
        let cfg = Config::default();
        assert_eq!(cfg.model, "llama3.2");
        assert_eq!(cfg.temperature, 0.2);
        assert_eq!(cfg.max_tokens, Some(4096));
        assert!(!cfg.allow_any_path, "the sandbox is closed by default");
    }

    #[test]
    fn config_yaml_deserializes_from_the_documented_schema() {
        // Guards the README's config example against schema drift.
        let cfg: Config = serde_yaml::from_str(
            "model: gpt-4o\ntemperature: 0.4\nmax_tokens: 2048\nopenai_api_key: sk-test\n",
        )
        .expect("parse");
        assert_eq!(cfg.model, "gpt-4o");
        assert_eq!(cfg.temperature, 0.4);
        assert_eq!(cfg.providers.openai_api_key.as_deref(), Some("sk-test"));
    }

    #[test]
    fn config_round_trips_through_yaml() {
        let original = global();
        let raw = serde_yaml::to_string(&original).expect("serialize");
        let back: Config = serde_yaml::from_str(&raw).expect("deserialize");
        assert_eq!(back.model, original.model);
        assert_eq!(back.temperature, original.temperature);
        assert_eq!(
            back.providers.openai_api_key,
            original.providers.openai_api_key
        );
    }
}

#[cfg(test)]
mod readme_tests {
    use super::*;

    /// The README's config example must match the real schema. Documentation
    /// drift here is silent and expensive: a config that does not parse just
    /// falls back to defaults, so the user never learns why their model
    /// changed.
    #[test]
    fn readme_config_example_parses() {
        let readme = include_str!("../README.md");
        let start = readme
            .find("## Configuration")
            .expect("README has a Configuration section");
        let yaml_start = readme[start..]
            .find("```yaml")
            .map(|i| start + i + "```yaml".len())
            .expect("Configuration section has a yaml block");
        let yaml_end = yaml_start
            + readme[yaml_start..]
                .find("```")
                .expect("yaml block is terminated");
        let block = &readme[yaml_start..yaml_end];

        let cfg: Config = serde_yaml::from_str(block)
            .unwrap_or_else(|e| panic!("README config example does not parse: {e}\n{block}"));
        assert_eq!(cfg.model, "llama3.2", "documented default");
        assert_eq!(
            cfg.providers.ollama_base_url.as_deref(),
            Some("http://localhost:11434")
        );
        assert_eq!(cfg.providers.custom_providers.len(), 1);
        assert_eq!(cfg.providers.custom_providers[0].name, "internal");
    }

    /// Every tool the README advertises must actually be registered.
    #[test]
    fn readme_tool_table_matches_the_registry() {
        let readme = include_str!("../README.md");
        let registered: Vec<String> = crate::tools::all_tools()
            .iter()
            .map(|t| format!("`{}`", t.def().name))
            .collect();
        for tool in &registered {
            assert!(
                readme.contains(tool.as_str()),
                "{tool} is registered but absent from the README"
            );
        }
    }
}

use crate::llm::ProviderConfig;
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
            providers: ProviderConfig::default(),
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

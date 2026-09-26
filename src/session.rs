use crate::agent::Agent;
use crate::agent::steer;
use crate::config::Config;
use crate::llm::{provider_name, resolve_provider};
use anyhow::{Context, Result};
use std::path::PathBuf;

pub struct Session {
    pub cwd: PathBuf,
    pub config: Config,
    pub agent: Agent,
    pub provider_label: String,
}

impl Session {
    pub fn new(config: Config, cwd: PathBuf, model_override: Option<String>) -> Result<Self> {
        let root = steer::find_project_root(&cwd);
        let settings = if let Some(root) = &root {
            root.join(".vibectl").join("config.yaml")
        } else {
            cwd.join(".vibectl").join("config.yaml")
        };

        let mut cfg = config.clone();
        if settings.is_file() {
            let raw = std::fs::read_to_string(&settings)
                .with_context(|| format!("failed to read {}", settings.display()))?;
            let overlay = crate::config::ConfigOverlay::parse(&raw)
                .with_context(|| format!("invalid config {}", settings.display()))?;
            overlay.apply(&mut cfg);
        }

        let model = model_override.unwrap_or_else(|| cfg.model.clone());

        let provider = resolve_provider(&cfg.providers, &model)
            .with_context(|| format!("failed to resolve provider for model {model}"))?;
        let label = provider_name(&cfg.providers, &model);

        let steering = steer::load_steering(&cwd);
        let mut agent = Agent::new(
            model.clone(),
            steering.content,
            provider,
            crate::tools::all_tools(),
            cwd.clone(),
        );
        agent.allow_any_path = cfg.allow_any_path;

        Ok(Self {
            cwd,
            config: cfg,
            agent,
            provider_label: label,
        })
    }

    pub fn set_model(&mut self, model: String) -> Result<()> {
        let provider = resolve_provider(&self.config.providers, &model)
            .with_context(|| format!("failed to resolve provider for model {model}"))?;
        self.provider_label = provider_name(&self.config.providers, &model);
        self.agent.model = model;
        self.agent.provider = provider;
        Ok(())
    }

    pub async fn plan(&self, task: &str) -> Result<String> {
        self.agent.plan(task).await
    }
}

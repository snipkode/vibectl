use crate::agent::{AgentEvent, Approval, Approver};
use crate::cli::Cli;
use crate::config::Config;
use crate::session::Session;
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use std::io::Write;
use std::path::PathBuf;

struct HeadlessApprover {
    allow: bool,
}

#[async_trait]
impl Approver for HeadlessApprover {
    async fn approve(&self, description: String) -> Approval {
        if self.allow {
            println!("[approved] {description}");
            Approval::Allow
        } else {
            eprintln!("[vibectl] refusing action without --dangerous-yes: {description}");
            Approval::Deny
        }
    }
}

pub async fn run(args: &Cli) -> Result<()> {
    let cwd = args
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let config = Config::load().context("failed to load config")?;
    let mut session = Session::new(config, cwd, args.model.clone())?;

    let prompt = match &args.prompt {
        Some(p) => p.clone(),
        None => {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
            if buf.trim().is_empty() {
                bail!("headless mode needs a prompt argument or piped stdin");
            }
            buf
        }
    };

    session.agent.approver = Some(std::sync::Arc::new(HeadlessApprover {
        allow: args.dangerous_yes,
    }));
    session.agent.allow_any_path = args.dangerous_yes;

    if args.plan {
        println!("[plan] generating plan…");
        let plan = session.plan(prompt.trim()).await?;
        println!("{plan}");
        let path = crate::agent::steer::save_plan(&session.cwd, &plan)?;
        eprintln!("[plan] saved to {}", path.display());
        return Ok(());
    }

    let mut stream = session.agent.spawn_run(prompt).0;
    let mut had_error = false;
    while let Some(ev) = stream.recv().await {
        match ev {
            AgentEvent::Text(t) => {
                print!("{t}");
                std::io::stdout().flush()?;
            }
            AgentEvent::ToolCall { name, .. } => {
                println!("\n[tool] → {name}");
            }
            AgentEvent::ToolResult { name, content, .. } => {
                println!("[tool: {name}]\n{content}");
            }
            AgentEvent::ToolError { name, error, .. } => {
                eprintln!("[tool error: {name}] {error}");
                had_error = true;
            }
            AgentEvent::Done { .. } => {
                println!();
            }
            AgentEvent::Error(e) => {
                eprintln!("\n[error] {e}");
                had_error = true;
            }
        }
    }

    if had_error {
        std::process::exit(1);
    }
    Ok(())
}

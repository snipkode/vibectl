use crate::agent::{AgentEvent, Approval, Approver};
use crate::agent::autonomous::{AutonomousContext, AutonomousConfig, FileOperationTracker};
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
    let mut session = Session::new(config, cwd.clone(), args.model.clone())?;

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

    // Initialize autonomous context with proper config
    let autonomous_config = if args.dangerous_yes {
        AutonomousConfig::headless()
    } else {
        AutonomousConfig::interactive()
    };

    // Also create a disabled config for comparison
    let _disabled_config = AutonomousConfig::disabled();

    // Log config status
    if autonomous_config.is_enabled() {
        println!("[autonomous] mode: {}", 
            if args.dangerous_yes { "headless (full automation)" } 
            else { "interactive (validation only)" }
        );
        println!("[autonomous] auto_validate: {}", autonomous_config.auto_validate);
        println!("[autonomous] auto_fix: {}", autonomous_config.auto_fix);
        println!("[autonomous] max_iterations: {}", autonomous_config.max_iterations);
        println!("[autonomous] generate_reports: {}\n", autonomous_config.generate_reports);
    } else {
        println!("[autonomous] disabled\n");
    }

    let mut autonomous_ctx = AutonomousContext::new(&cwd)?;
    autonomous_ctx.validation_enabled = autonomous_config.auto_validate;
    autonomous_ctx.auto_fix_enabled = autonomous_config.auto_fix;

    let mut tracker = FileOperationTracker::new();

    // Classify user intent
    let user_intent = crate::agent::intent::classify_intent(
        &prompt,
        &session.agent.model,
        session.agent.provider.clone(),
    )
    .await;

    let mut stream = session.agent.spawn_run(prompt).0;
    let mut had_error = false;

    while let Some(ev) = stream.recv().await {
        match ev {
            AgentEvent::Plan(plan) => {
                println!("[auto-plan]\n{plan}\n");
            }
            AgentEvent::Implementation(s) => {
                println!("[implementation summary]\n{s}");
            }
            AgentEvent::Text(t) => {
                print!("{t}");
                std::io::stdout().flush()?;
            }
            AgentEvent::ToolCall { name, id } => {
                println!("\n[tool] → {name} (id: {id})");
            }
            AgentEvent::ToolResult { name, content, id } => {
                println!("[tool: {name}] (id: {id})");
                
                // Track file operations using context helper
                if name == "write_file" || name == "create_dir" {
                    // Parse JSON args to extract path properly
                    if let Some(args_str) = content.lines()
                        .find(|l| l.contains("arguments") || l.contains("path"))
                    {
                        if let Ok(args) = serde_json::from_str::<serde_json::Value>(args_str) {
                            if let Some(path) = autonomous_ctx.extract_path(&args) {
                                let existed = autonomous_ctx.file_exists_at(&path);
                                tracker.track_write(&path, existed);
                            }
                        }
                    }
                    
                    // Fallback: extract from content
                    if let Some(path) = content.lines()
                        .find(|l| l.contains("Created") || l.contains("update"))
                        .and_then(|l| l.split_whitespace().last())
                    {
                        let existed = content.contains("update") || autonomous_ctx.file_exists_at(path);
                        tracker.track_write(path, existed);
                    }
                }
                
                // Track dependencies
                if name == "run_command" && content.contains("install") {
                    // Extract package names from install commands
                    if content.contains("npm install") || content.contains("pip install") {
                        for line in content.lines() {
                            if line.contains("added") || line.contains("installed") {
                                tracker.track_dependency(line);
                            }
                        }
                    }
                }
                
                println!("{content}");
            }
            AgentEvent::ToolError { name, error, .. } => {
                eprintln!("[tool error: {name}] {error}");
                had_error = true;
            }
            AgentEvent::Done { .. } => {
                println!();
                
                // Check if autonomous validation should trigger
                let should_validate = crate::agent::autonomous::should_trigger_validation(
                    &user_intent,
                    &tracker,
                );
                
                if autonomous_config.auto_validate && should_validate && tracker.has_changes() {
                    // Also check using context's validation check
                    let validation_ctx_check = autonomous_ctx.clone();
                    if !validation_ctx_check.should_run_validation() {
                        println!("⚠️  Validation check indicates not ready, skipping...\n");
                        continue;
                    }
                    
                    // Print validation trigger message
                    let trigger_msg = crate::agent::autonomous::format_validation_trigger_message(&tracker);
                    print!("{}", trigger_msg);
                    
                    // Update context with latest tracker
                    let mut validation_ctx = autonomous_ctx.clone();
                    validation_ctx.tracker = tracker.clone();
                    
                    // Check if project is ready for validation
                    if crate::agent::validation::is_project_ready(&validation_ctx.workspace) 
                        && validation_ctx.validation_enabled 
                    {
                        println!("✓ Project ready for validation\n");
                        println!("  Validation enabled: {}", validation_ctx.validation_enabled);
                        println!("  Auto-fix enabled: {}\n", validation_ctx.auto_fix_enabled);
                        
                        // Use max_iterations from config
                        if autonomous_config.auto_fix {
                            println!("  Max iterations: {}\n", autonomous_config.max_iterations);
                        }
                        
                        // Create a channel for validation events
                        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
                        
                        // Spawn validation task
                        let agent_clone = session.agent.clone();
                        let validation_ctx_clone = validation_ctx.clone();
                        let validation_handle = tokio::spawn(async move {
                            agent_clone.run_autonomous_validation(&validation_ctx_clone, &tx).await
                        });
                        
                        // Process validation events
                        let event_handle = tokio::spawn(async move {
                            while let Some(event) = rx.recv().await {
                                match event {
                                    AgentEvent::Text(t) => {
                                        print!("{}", t);
                                        let _ = std::io::stdout().flush();
                                    }
                                    _ => {}
                                }
                            }
                        });
                        
                        // Wait for validation to complete
                        match validation_handle.await {
                            Ok(Ok(report)) => {
                                // Wait for event processing to finish
                                let _ = event_handle.await;
                                
                                println!("\n{}", report.to_summary());
                                
                                // Save report if configured
                                if autonomous_config.generate_reports {
                                    let (save_tx, _save_rx) = tokio::sync::mpsc::channel(1);
                                    if let Ok(path) = session.agent.save_implementation_report(&report, &save_tx).await {
                                        println!("📄 Report saved to: {}", path.display());
                                    }
                                }
                                
                                // Check if validation succeeded
                                use crate::agent::report::ImplementationStatus;
                                if report.status != ImplementationStatus::Completed {
                                    had_error = true;
                                }
                            }
                            Ok(Err(e)) => {
                                eprintln!("❌ Validation failed: {}", e);
                                had_error = true;
                            }
                            Err(e) => {
                                eprintln!("❌ Validation task panicked: {}", e);
                                had_error = true;
                            }
                        }
                    } else {
                        println!("⚠️  Project not ready for validation (no manifest files found)\n");
                        
                        // Still generate a report
                        let report = session.agent.build_implementation_report(&validation_ctx);
                        println!("{}", report.to_summary());
                        
                        if autonomous_config.generate_reports {
                            let (save_tx, _save_rx) = tokio::sync::mpsc::channel(1);
                            if let Ok(path) = session.agent.save_implementation_report(&report, &save_tx).await {
                                println!("📄 Report saved to: {}", path.display());
                            }
                        }
                    }
                } else if tracker.has_changes() {
                    // Generate basic report even without validation
                    let validation_ctx = autonomous_ctx.clone();
                    let report = session.agent.build_implementation_report(&validation_ctx);
                    
                    if autonomous_config.generate_reports {
                        println!("\n📊 Generating implementation report...\n");
                        let (save_tx, _save_rx) = tokio::sync::mpsc::channel(1);
                        if let Ok(path) = session.agent.save_implementation_report(&report, &save_tx).await {
                            println!("📄 Report saved to: {}", path.display());
                        }
                    }
                }
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

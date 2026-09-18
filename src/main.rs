mod agent;
mod audit;
mod cli;
mod config;
mod headless;
mod llm;
mod session;
mod tools;
mod tui;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command};

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();

    let cwd = args.cwd.clone().unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });

    // ── Subcommands ───────────────────────────────────────────────────────────
    if let Some(cmd) = &args.command {
        match cmd {
            Command::Audit { init_steering } => {
                audit::run(&cwd);
                if *init_steering {
                    run_init_steering(&cwd);
                }
                return Ok(());
            }
            Command::Undo => {
                let root = agent::steer::find_project_root(&cwd).unwrap_or(cwd.clone());
                match agent::checkpoint::undo(&root) {
                    Ok(summary) => println!("{summary}"),
                    Err(e) => {
                        eprintln!("vibectl undo: {e}");
                        std::process::exit(1);
                    }
                }
                return Ok(());
            }
        }
    }

    // ── --init-steering ───────────────────────────────────────────────────────
    if args.init_steering {
        run_init_steering(&cwd);
        return Ok(());
    }

    // ── Headless mode ─────────────────────────────────────────────────────────
    if args.headless {
        headless::run(&args).await?;
        return Ok(());
    }

    // ── Interactive TUI (default) ─────────────────────────────────────────────
    let config = config::Config::load()?;
    let session = session::Session::new(config, cwd, args.model.clone())?;
    tui::run(session).await
}

fn run_init_steering(cwd: &std::path::Path) {
    match crate::agent::steer::init_steering(cwd) {
        Ok(created) if created.is_empty() => {
            eprintln!(
                "vibectl: steering files already exist in .vibectl/steering/ — nothing to create."
            );
        }
        Ok(created) => {
            println!("vibectl: created {} steering file(s):", created.len());
            for p in &created {
                println!("  + {}", p.display());
            }
            println!();
            println!("Edit these files to teach vibectl about your project.");
        }
        Err(e) => {
            eprintln!("vibectl: failed to init steering: {e}");
            std::process::exit(1);
        }
    }
}

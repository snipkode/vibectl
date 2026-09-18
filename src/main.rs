mod agent;
mod cli;
mod config;
mod headless;
mod llm;
mod session;
mod tools;
mod tui;

use anyhow::Result;
use clap::Parser;
use cli::Cli;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();

    if args.headless {
        headless::run(&args).await?;
        return Ok(());
    }

    let cwd = args.cwd.clone().unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });
    let config = config::Config::load()?;

    let session = session::Session::new(config, cwd, args.model)?;
    tui::run(session).await
}

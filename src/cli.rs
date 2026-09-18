use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "vibectl",
    version,
    about = "Vibe coding agent CLI - build, test, and deploy from your terminal"
)]
pub struct Cli {
    /// Working directory (defaults to current directory)
    #[arg(short, long)]
    pub cwd: Option<PathBuf>,

    /// Model to use (overrides config)
    #[arg(short, long)]
    pub model: Option<String>,

    /// Run headless (non-interactive): execute the prompt and exit
    #[arg(long)]
    pub headless: bool,

    /// Prompt to run in headless mode (piped from stdin if omitted)
    pub prompt: Option<String>,

    /// Allow the agent to run shell commands without confirmation in headless mode
    #[arg(long)]
    pub dangerous_yes: bool,

    /// Print a plan and exit without executing anything
    #[arg(long)]
    pub plan: bool,
}

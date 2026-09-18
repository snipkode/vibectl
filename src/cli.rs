use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "vibectl",
    version,
    about = "Vibe coding agent CLI — agentic coding assistant",
    long_about = None,
)]
pub struct Cli {
    /// Working directory (defaults to current directory)
    #[arg(short, long, global = true)]
    pub cwd: Option<PathBuf>,

    /// Model to use (overrides config)
    #[arg(short, long, global = true)]
    pub model: Option<String>,

    /// Run headless (non-interactive): execute the prompt and exit
    #[arg(long, global = true)]
    pub headless: bool,

    /// Prompt to run in headless mode (piped from stdin if omitted)
    pub prompt: Option<String>,

    /// Allow the agent to run shell commands without confirmation in headless mode
    #[arg(long)]
    pub dangerous_yes: bool,

    /// Generate a plan and exit without executing anything
    #[arg(long)]
    pub plan: bool,

    /// Generate skeleton .vibectl/steering/*.md files in the project root
    #[arg(long)]
    pub init_steering: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Audit the repository: discover steering files, detect languages,
    /// inventory features, flag gaps, and print a structured report.
    Audit {
        /// Also generate .vibectl/steering/ skeleton after auditing
        #[arg(long)]
        init_steering: bool,
    },
    /// Rollback the last agent run by popping the most recent vibectl
    /// git stash checkpoint.  Equivalent to typing /undo in the TUI.
    Undo,
}

use super::{Tool, ToolDef, ToolResult};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::time::timeout;

pub struct ShellExec;

impl Tool for ShellExec {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "shell_exec",
            "Execute a shell command in the project directory. Use for running builds, tests, and inspecting the environment. Long-running commands are killed after 60s.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to run (e.g. 'cargo test')" }
                },
                "required": ["command"]
            }),
        )
    }

    fn run(&self, args: &Value, cwd: &Path) -> Result<ToolResult> {
        let command = args
            .get("command")
            .context("missing 'command'")?
            .as_str()
            .context("'command' must be a string")?
            .to_string();
        let cwd = cwd.to_path_buf();

        // ShellExec.run() is called from a sync context (Tool trait), but we need
        // async for tokio::process. Spawn onto the current tokio runtime.
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(run_async(&command, &cwd, 60))
        })
    }
}

/// New tool for running arbitrary commands with configurable timeout
pub struct RunCommand;

impl Tool for RunCommand {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "run_command",
            "Execute a command in the workspace with detailed output. Returns structured result with exit_code, stdout, stderr, and duration. Use this for dependency installation, builds, and general command execution.",
            json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Command to execute (e.g. 'npm install', 'cargo build')"
                    },
                    "timeout_seconds": {
                        "type": "integer",
                        "description": "Maximum execution time in seconds (default: 300, max: 600)",
                        "default": 300
                    }
                },
                "required": ["command"]
            }),
        )
    }

    fn run(&self, args: &Value, cwd: &Path) -> Result<ToolResult> {
        let command = args
            .get("command")
            .context("missing 'command'")?
            .as_str()
            .context("'command' must be a string")?
            .to_string();
        
        let timeout_secs = args
            .get("timeout_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(300)
            .min(600); // max 10 minutes

        let cwd = cwd.to_path_buf();

        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(run_async(&command, &cwd, timeout_secs))
        })
    }
}

/// Tool specifically for running tests with appropriate timeout
pub struct RunTests;

impl Tool for RunTests {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "run_tests",
            "Execute the project's test suite. Automatically detects the appropriate test command based on project type. Returns detailed test results including exit code, output, and duration.",
            json!({
                "type": "object",
                "properties": {
                    "test_command": {
                        "type": "string",
                        "description": "Optional custom test command. If omitted, auto-detects based on project type (npm test, cargo test, go test, pytest)",
                    },
                    "timeout_seconds": {
                        "type": "integer",
                        "description": "Maximum test execution time in seconds (default: 300)",
                        "default": 300
                    }
                }
            }),
        )
    }

    fn run(&self, args: &Value, cwd: &Path) -> Result<ToolResult> {
        let timeout_secs = args
            .get("timeout_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(300)
            .min(600);

        // Auto-detect test command if not provided
        let test_command = if let Some(cmd) = args.get("test_command").and_then(|v| v.as_str()) {
            cmd.to_string()
        } else {
            detect_test_command(cwd)?
        };

        let cwd = cwd.to_path_buf();

        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(run_async(&test_command, &cwd, timeout_secs))
        })
    }
}

/// Detect the appropriate test command based on project files
fn detect_test_command(cwd: &Path) -> Result<String> {
    if cwd.join("package.json").exists() {
        return Ok("npm test".to_string());
    }
    if cwd.join("Cargo.toml").exists() {
        return Ok("cargo test".to_string());
    }
    if cwd.join("go.mod").exists() {
        return Ok("go test ./...".to_string());
    }
    if cwd.join("requirements.txt").exists() || cwd.join("pyproject.toml").exists() {
        return Ok("pytest".to_string());
    }
    
    bail!("Could not detect project type. Please specify test_command explicitly.")
}

/// Core async execution function with structured output
async fn run_async(command: &str, cwd: &Path, timeout_secs: u64) -> Result<ToolResult> {
    let start = Instant::now();
    let deadline = Duration::from_secs(timeout_secs);

    // Use PowerShell on Windows
    let (shell, shell_arg) = if cfg!(target_os = "windows") {
        ("powershell", "-Command")
    } else {
        ("sh", "-c")
    };

    let child = Command::new(shell)
        .arg(shell_arg)
        .arg(command)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn command")?;

    let result = timeout(deadline, child.wait_with_output()).await;

    let duration = start.elapsed();

    let output = match result {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => {
            bail!(
                "Command timed out after {}s: {}\n\nThe command exceeded the maximum allowed execution time.",
                timeout_secs,
                command
            );
        }
    };

    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    // Build structured output
    let mut result_text = String::new();
    result_text.push_str(&format!("$ {}\n\n", command));
    
    result_text.push_str("=== EXECUTION RESULT ===\n");
    result_text.push_str(&format!("Exit Code: {}\n", exit_code));
    result_text.push_str(&format!("Duration: {:.2}s\n", duration.as_secs_f64()));
    result_text.push_str(&format!("Status: {}\n\n", if exit_code == 0 { "SUCCESS" } else { "FAILED" }));

    if !stdout.trim().is_empty() {
        result_text.push_str("=== STDOUT ===\n");
        result_text.push_str(&trim_to(&stdout, 12_000, "[stdout truncated]"));
        result_text.push_str("\n\n");
    }

    if !stderr.trim().is_empty() {
        result_text.push_str("=== STDERR ===\n");
        result_text.push_str(&trim_to(&stderr, 12_000, "[stderr truncated]"));
        result_text.push_str("\n\n");
    }

    // Add interpretation hint for the agent
    if exit_code != 0 {
        result_text.push_str("=== ANALYSIS ===\n");
        result_text.push_str("Command failed. Review the error output above to identify the issue.\n");
        result_text.push_str("Common causes:\n");
        result_text.push_str("- Missing dependencies (run installation command first)\n");
        result_text.push_str("- Syntax errors in code (check stderr for file and line numbers)\n");
        result_text.push_str("- Configuration issues (check environment variables and config files)\n");
        result_text.push_str("- Test failures (review test output for specific failing tests)\n");
    }

    Ok(ToolResult {
        content: result_text.trim().to_string(),
    })
}

fn trim_to(s: &str, max: usize, marker: &str) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let cut = max.saturating_sub(marker.len());
    let mut out = s[..cut].to_string();
    out.push('\n');
    out.push_str(marker);
    out
}

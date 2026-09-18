use super::{Tool, ToolDef, ToolResult};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;
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
            tokio::runtime::Handle::current().block_on(run_async(&command, &cwd))
        })
    }
}

async fn run_async(command: &str, cwd: &Path) -> Result<ToolResult> {
    let deadline = Duration::from_secs(60);

    let child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn shell")?;

    let result = timeout(deadline, child.wait_with_output()).await;

    let output = match result {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => {
            bail!("command timed out after {}s: {command}", deadline.as_secs());
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    let mut combined = stdout;
    if !stderr.trim().is_empty() {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&stderr);
    }

    let trimmed = trim_to(&combined, 16_000, "[output truncated]");

    let status_note = if !output.status.success() {
        format!("\n[exit code: {:?}]", output.status.code())
    } else {
        String::new()
    };

    Ok(ToolResult {
        content: if trimmed.is_empty() && status_note.is_empty() {
            "(command produced no output)".to_string()
        } else {
            format!("$ {command}\n{trimmed}{status_note}")
        },
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

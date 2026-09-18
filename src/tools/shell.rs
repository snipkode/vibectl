use super::{Tool, ToolDef, ToolResult};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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
            .context("'command' must be a string")?;

        let timeout = Duration::from_secs(60);

        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn shell")?;

        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait()? {
                Some(s) => break s,
                None => {
                    if Instant::now() > deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        bail!("command timed out after {}s: {command}", timeout.as_secs());
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        };

        let mut stdout = String::new();
        let mut stderr = String::new();
        if let Some(mut out) = child.stdout.take() {
            let _ = out.read_to_string(&mut stdout);
        }
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_string(&mut stderr);
        }

        let mut output = stdout;
        if !stderr.trim().is_empty() {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&stderr);
        }

        let trimmed = trim_to(&output, 16_000, "[output truncated]");

        let status_note = if !status.success() {
            format!("\n[exit code: {:?}]", status.code())
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

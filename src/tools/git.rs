use crate::tools::{Tool, ToolResult};
use anyhow::Context;
use std::path::Path;

pub struct GitTool;

impl Tool for GitTool {
    fn def(&self) -> super::ToolDef {
        super::ToolDef::new(
            "git",
            "Run git commands in the project. Provide 'subcommand' (status, diff, log) and optional 'path'.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "subcommand": {
                        "type": "string",
                        "description": "Git subcommand: status, diff, log, or a custom git command"
                    },
                    "path": {
                        "type": "string",
                        "description": "Optional path for git status/diff"
                    }
                },
                "required": ["subcommand"]
            }),
        )
    }

    fn run(&self, args: &serde_json::Value, cwd: &Path) -> anyhow::Result<ToolResult> {
        let sub = args
            .get("subcommand")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("status")
            .to_string();
        let path = args
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(String::from);

        let mut git = std::process::Command::new("git");
        git.current_dir(cwd);
        let stdout_limit = if sub == "diff" { Some(40000) } else { None };

        match sub.as_str() {
            "status" => {
                git.arg("status").arg("--short");
                if let Some(p) = &path {
                    git.arg(p);
                }
            }
            "diff" => {
                git.arg("diff");
                if let Some(p) = &path {
                    git.arg(p);
                }
            }
            "log" => {
                git.arg("log").arg("--oneline").arg("-20");
                if let Some(p) = &path {
                    git.arg(p);
                }
            }
            custom => {
                git.arg(custom);
                if let Some(p) = &path {
                    git.arg(p);
                }
            }
        }

        let output = git
            .output()
            .with_context(|| format!("failed to run git {sub}"))?;

        let mut content = vec![];
        content.extend_from_slice(&output.stdout);
        if !output.stderr.is_empty() {
            content.push(b'\n');
            content.extend_from_slice(&output.stderr);
        }
        let mut text = String::from_utf8_lossy(&content).into_owned();
        if text.trim().is_empty() {
            text = "(empty output)".into();
        }
        if let Some(limit) = stdout_limit
            && text.len() > limit
        {
            text.truncate(limit);
            text.push_str(&format!("\n... (diff truncated at {limit} bytes)"));
        }
        Ok(ToolResult { content: text })
    }
}

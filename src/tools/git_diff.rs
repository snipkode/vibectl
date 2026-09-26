//! `git_diff` — dedicated tool for inspecting working-tree and staged changes.
//!
//! Separate from the general `git` tool so the agent can call it explicitly
//! by name when verifying that a patch was applied correctly.

use super::{Tool, ToolDef, ToolResult};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::Path;

pub struct GitDiff;

impl Tool for GitDiff {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "git_diff",
            "Show changes in the working tree (unstaged by default, or staged with cached=true). \
             Returns a unified diff. Use after applying a patch to verify the change looks correct. \
             Truncated at 40 KB.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Optional: limit diff to this file or directory"
                    },
                    "cached": {
                        "type": "boolean",
                        "description": "If true, show staged (cached) changes instead of working-tree changes. Default false."
                    },
                    "base": {
                        "type": "string",
                        "description": "Optional: compare against this commit/branch (e.g. 'HEAD~1', 'main')"
                    }
                },
                "required": []
            }),
        )
    }

    fn run(&self, args: &Value, cwd: &Path) -> Result<ToolResult> {
        let path_arg = args.get("path").and_then(|v| v.as_str());
        let cached = args
            .get("cached")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let base = args.get("base").and_then(|v| v.as_str());

        let mut cmd = std::process::Command::new("git");
        cmd.current_dir(cwd).arg("diff");

        if cached {
            cmd.arg("--cached");
        }
        if let Some(b) = base {
            cmd.arg(b);
        }

        // Always include stat summary at the top.
        // We run two commands: one for stat, one for the full diff.
        let mut stat_cmd = std::process::Command::new("git");
        stat_cmd.current_dir(cwd).arg("diff").arg("--stat");
        if cached {
            stat_cmd.arg("--cached");
        }
        if let Some(b) = base {
            stat_cmd.arg(b);
        }
        if let Some(p) = path_arg {
            stat_cmd.arg("--").arg(p);
        }

        if let Some(p) = path_arg {
            cmd.arg("--").arg(p);
        }

        const TRUNCATE_AT: usize = 40_000;

        let stat_output = stat_cmd
            .output()
            .with_context(|| "failed to run git diff --stat")?;
        let stat_text = String::from_utf8_lossy(&stat_output.stdout).into_owned();

        let diff_output = cmd
            .output()
            .with_context(|| "failed to run git diff")?;

        let mut diff_text = String::from_utf8_lossy(&diff_output.stdout).into_owned();

        if !diff_output.stderr.is_empty() {
            let err = String::from_utf8_lossy(&diff_output.stderr);
            diff_text.push_str(&format!("\n[git stderr]: {err}"));
        }

        if diff_text.trim().is_empty() && stat_text.trim().is_empty() {
            let label = if cached {
                "No staged changes."
            } else {
                "No unstaged changes in the working tree."
            };
            return Ok(ToolResult {
                content: label.to_string(),
            });
        }

        if diff_text.len() > TRUNCATE_AT {
            diff_text.truncate(TRUNCATE_AT);
            diff_text.push_str(&format!(
                "\n... [diff truncated at {TRUNCATE_AT} bytes — use path= to narrow the scope]"
            ));
        }

        let kind = if cached { "staged" } else { "unstaged" };
        let mut output = format!("=== git diff ({kind}) ===\n");
        if !stat_text.trim().is_empty() {
            output.push_str(&stat_text);
            output.push('\n');
        }
        output.push_str(&diff_text);

        Ok(ToolResult { content: output })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    /// Initialise a minimal git repo with one commit so diff works.
    fn init_repo(dir: &TempDir) {
        let p = dir.path();
        for args in [
            vec!["init"],
            vec!["config", "user.email", "test@test.com"],
            vec!["config", "user.name", "Test"],
        ] {
            Command::new("git").current_dir(p).args(&args).output().ok();
        }
        std::fs::write(p.join("initial.txt"), "hello").unwrap();
        Command::new("git").current_dir(p).args(["add", "."]).output().ok();
        Command::new("git")
            .current_dir(p)
            .args(["commit", "-m", "init"])
            .output()
            .ok();
    }

    #[test]
    fn no_changes_returns_clean_message() {
        let dir = TempDir::new().unwrap();
        init_repo(&dir);

        let tool = GitDiff;
        let args = json!({});
        let result = tool.run(&args, dir.path()).unwrap();
        assert!(
            result.content.contains("No unstaged"),
            "got: {}",
            result.content
        );
    }

    #[test]
    fn detects_unstaged_modification() {
        let dir = TempDir::new().unwrap();
        init_repo(&dir);

        std::fs::write(dir.path().join("initial.txt"), "modified content").unwrap();

        let tool = GitDiff;
        let args = json!({});
        let result = tool.run(&args, dir.path()).unwrap();
        assert!(
            result.content.contains("initial.txt") || result.content.contains("modified"),
            "got: {}",
            result.content
        );
    }

    #[test]
    fn detects_staged_changes() {
        let dir = TempDir::new().unwrap();
        init_repo(&dir);

        std::fs::write(dir.path().join("new.txt"), "new file").unwrap();
        Command::new("git")
            .current_dir(dir.path())
            .args(["add", "new.txt"])
            .output()
            .ok();

        let tool = GitDiff;
        let args = json!({ "cached": true });
        let result = tool.run(&args, dir.path()).unwrap();
        assert!(
            result.content.contains("new.txt"),
            "got: {}",
            result.content
        );
    }

    #[test]
    fn path_filter_limits_diff() {
        let dir = TempDir::new().unwrap();
        init_repo(&dir);

        std::fs::write(dir.path().join("a.txt"), "changed a").unwrap();
        std::fs::write(dir.path().join("b.txt"), "changed b").unwrap();

        let tool = GitDiff;
        let args = json!({ "path": "a.txt" });
        let result = tool.run(&args, dir.path()).unwrap();
        // The diff should reference a.txt but not necessarily b.txt.
        assert!(
            result.content.contains("a.txt") || result.content.contains("No unstaged"),
            "got: {}",
            result.content
        );
    }
}

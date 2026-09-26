//! `git_status` — dedicated tool for inspecting repository status.
//!
//! Returns the working-tree status with branch information and file counts,
//! structured so the agent can quickly understand what has changed without
//! reading the full diff.

use super::{Tool, ToolDef, ToolResult};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::Path;

pub struct GitStatus;

impl Tool for GitStatus {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "git_status",
            "Show the current git repository status: branch name, staged changes, \
             unstaged changes, and untracked files. More structured than 'git diff' — \
             use this first to get an overview, then git_diff for the actual changes.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Optional: limit status to this file or directory"
                    }
                },
                "required": []
            }),
        )
    }

    fn run(&self, args: &Value, cwd: &Path) -> Result<ToolResult> {
        let path_arg = args.get("path").and_then(|v| v.as_str());

        // ── Branch / upstream ─────────────────────────────────────────────────
        let branch = run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        let branch = branch.trim().to_string();

        // Try to get upstream tracking info.
        let upstream = run_git(cwd, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]);
        let tracking = match &upstream {
            Ok(u) if !u.trim().is_empty() && u.trim() != "@{u}" => {
                // Count ahead/behind.
                let behind_ahead =
                    run_git(cwd, &["rev-list", "--left-right", "--count", "@{u}...HEAD"]);
                match behind_ahead {
                    Ok(ba) => {
                        let parts: Vec<&str> = ba.split_whitespace().collect();
                        if parts.len() == 2 {
                            let behind: u32 = parts[0].parse().unwrap_or(0);
                            let ahead: u32 = parts[1].parse().unwrap_or(0);
                            format!(
                                " (tracking: {}, ahead: {ahead}, behind: {behind})",
                                u.trim()
                            )
                        } else {
                            format!(" (tracking: {})", u.trim())
                        }
                    }
                    Err(_) => format!(" (tracking: {})", u.trim()),
                }
            }
            _ => " (no upstream)".to_string(),
        };

        // ── Porcelain status ─────────────────────────────────────────────────
        let mut porcelain_args = vec!["status", "--porcelain"];
        let path_owned;
        if let Some(p) = path_arg {
            path_owned = p.to_string();
            porcelain_args.push(&path_owned);
        }
        let porcelain = run_git(cwd, &porcelain_args).unwrap_or_default();

        // ── Parse porcelain output ────────────────────────────────────────────
        let mut staged: Vec<String> = vec![];
        let mut unstaged: Vec<String> = vec![];
        let mut untracked: Vec<String> = vec![];
        let mut conflicted: Vec<String> = vec![];

        for line in porcelain.lines() {
            if line.len() < 3 {
                continue;
            }
            let x = line.chars().next().unwrap_or(' '); // index status
            let y = line.chars().nth(1).unwrap_or(' '); // worktree status
            let file = &line[3..];

            if x == '?' && y == '?' {
                untracked.push(file.to_string());
                continue;
            }

            if matches!(x, 'U' | 'A') && matches!(y, 'U' | 'A') {
                conflicted.push(file.to_string());
                continue;
            }

            // Staged changes: index column is not space or ?
            if x != ' ' && x != '?' {
                let label = match x {
                    'M' => "modified",
                    'A' => "added",
                    'D' => "deleted",
                    'R' => "renamed",
                    'C' => "copied",
                    _ => "changed",
                };
                staged.push(format!("  {label}: {file}"));
            }

            // Unstaged changes: worktree column is not space or ?
            if y != ' ' && y != '?' {
                let label = match y {
                    'M' => "modified",
                    'D' => "deleted",
                    _ => "changed",
                };
                unstaged.push(format!("  {label}: {file}"));
            }
        }

        // ── Format output ─────────────────────────────────────────────────────
        let mut out = format!("On branch {branch}{tracking}\n\n");

        if staged.is_empty() && unstaged.is_empty() && untracked.is_empty() && conflicted.is_empty() {
            out.push_str("Nothing to commit, working tree clean.\n");
        } else {
            if !staged.is_empty() {
                out.push_str(&format!(
                    "Changes to be committed ({}):\n{}\n\n",
                    staged.len(),
                    staged.join("\n")
                ));
            }
            if !unstaged.is_empty() {
                out.push_str(&format!(
                    "Changes not staged for commit ({}):\n{}\n\n",
                    unstaged.len(),
                    unstaged.join("\n")
                ));
            }
            if !conflicted.is_empty() {
                out.push_str(&format!(
                    "Unmerged paths ({}):\n{}\n\n",
                    conflicted.len(),
                    conflicted
                        .iter()
                        .map(|f| format!("  conflict: {f}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }
            if !untracked.is_empty() {
                // Cap untracked list to avoid flooding the context.
                let shown = untracked.len().min(20);
                let suffix = if untracked.len() > 20 {
                    format!("\n  ... and {} more", untracked.len() - 20)
                } else {
                    String::new()
                };
                out.push_str(&format!(
                    "Untracked files ({}):\n{}{}\n\n",
                    untracked.len(),
                    untracked[..shown]
                        .iter()
                        .map(|f| format!("  {f}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    suffix
                ));
            }
        }

        // Append last commit for context.
        if let Ok(last) = run_git(cwd, &["log", "--oneline", "-1"]) {
            if !last.trim().is_empty() {
                out.push_str(&format!("Last commit: {}\n", last.trim()));
            }
        }

        Ok(ToolResult { content: out.trim_end().to_string() })
    }
}

/// Run a git sub-command and return stdout as a String.
fn run_git(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git {}", args.first().unwrap_or(&"?")))?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

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
    fn clean_repo_says_clean() {
        let dir = TempDir::new().unwrap();
        init_repo(&dir);

        let tool = GitStatus;
        let result = tool.run(&json!({}), dir.path()).unwrap();
        assert!(
            result.content.contains("clean") || result.content.contains("Nothing"),
            "got: {}",
            result.content
        );
    }

    #[test]
    fn untracked_file_appears() {
        let dir = TempDir::new().unwrap();
        init_repo(&dir);

        std::fs::write(dir.path().join("new.rs"), "fn main() {}").unwrap();

        let tool = GitStatus;
        let result = tool.run(&json!({}), dir.path()).unwrap();
        assert!(
            result.content.contains("new.rs") || result.content.contains("Untracked"),
            "got: {}",
            result.content
        );
    }

    #[test]
    fn modified_file_appears() {
        let dir = TempDir::new().unwrap();
        init_repo(&dir);

        std::fs::write(dir.path().join("initial.txt"), "modified content").unwrap();

        let tool = GitStatus;
        let result = tool.run(&json!({}), dir.path()).unwrap();
        assert!(
            result.content.contains("initial.txt") || result.content.contains("modified"),
            "got: {}",
            result.content
        );
    }

    #[test]
    fn staged_file_appears_in_staged_section() {
        let dir = TempDir::new().unwrap();
        init_repo(&dir);

        std::fs::write(dir.path().join("staged.txt"), "new file").unwrap();
        Command::new("git")
            .current_dir(dir.path())
            .args(["add", "staged.txt"])
            .output()
            .ok();

        let tool = GitStatus;
        let result = tool.run(&json!({}), dir.path()).unwrap();
        assert!(
            result.content.contains("staged.txt"),
            "got: {}",
            result.content
        );
    }

    #[test]
    fn shows_branch_name() {
        let dir = TempDir::new().unwrap();
        init_repo(&dir);

        let tool = GitStatus;
        let result = tool.run(&json!({}), dir.path()).unwrap();
        assert!(
            result.content.contains("branch") || result.content.contains("On branch"),
            "got: {}",
            result.content
        );
    }
}

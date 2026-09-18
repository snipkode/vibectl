use super::read_file::resolve_path;
use super::{Tool, ToolDef, ToolResult};
use anyhow::{Context, Result};
use glob::Pattern;
use serde_json::{Value, json};

pub struct GlobFiles;

impl Tool for GlobFiles {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "glob",
            "List files matching a glob pattern (e.g. 'src/**/*.rs'). Respects .gitignore. Directories are excluded; returns up to 1000 matches.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern relative to the project root" }
                },
                "required": ["pattern"]
            }),
        )
    }

    fn run(&self, args: &Value, cwd: &std::path::Path) -> Result<ToolResult> {
        let pattern = args
            .get("pattern")
            .context("missing 'pattern'")?
            .as_str()
            .context("'pattern' must be a string")?;

        let compiled =
            Pattern::new(pattern).with_context(|| format!("invalid glob pattern: {pattern}"))?;

        let walker = ignore::WalkBuilder::new(cwd)
            .hidden(false)
            .git_ignore(true)
            .build();

        let mut matches = Vec::new();
        for entry in walker {
            let entry = entry?;
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(cwd)
                .unwrap_or(entry.path())
                .to_string_lossy();
            if compiled.matches(&rel) {
                matches.push(rel.to_string());
            }
            if matches.len() >= 1000 {
                break;
            }
        }

        Ok(ToolResult {
            content: if matches.is_empty() {
                "No files matched the pattern.".to_string()
            } else {
                format!("{} file(s) matched:\n{}", matches.len(), matches.join("\n"))
            },
        })
    }
}

pub struct GrepFiles;

impl Tool for GrepFiles {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "grep",
            "Search file contents using a regular expression. Returns file paths with line numbers. Respects .gitignore.",
            json!({
                "type": "object",
                "properties": {
                    "regex": { "type": "string", "description": "Regular expression to search for" },
                    "glob": { "type": "string", "description": "Optional file glob to filter (e.g. '*.rs')" },
                    "path": { "type": "string", "description": "Optional subdirectory to search in" }
                },
                "required": ["regex"]
            }),
        )
    }

    fn run(&self, args: &Value, cwd: &std::path::Path) -> Result<ToolResult> {
        let regex = args
            .get("regex")
            .context("missing 'regex'")?
            .as_str()
            .context("'regex' must be a string")?;
        let regex = regex::Regex::new(regex).context("invalid regex")?;

        let glob_filter = args.get("glob").and_then(|v| v.as_str());
        let subdir = args.get("path").and_then(|v| v.as_str());
        let root = match subdir {
            Some(s) => resolve_path(cwd, s),
            None => cwd.to_path_buf(),
        };

        if !root.is_dir() {
            return Ok(ToolResult {
                content: format!("{} is not a directory", root.display()),
            });
        }

        let walker = ignore::WalkBuilder::new(&root)
            .hidden(false)
            .git_ignore(true)
            .build();

        let mut results = Vec::new();
        let mut files_hit = 0usize;
        let mut lines_hit = 0usize;

        'outer: for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let path = entry.path();
            if let Some(g) = glob_filter
                && !Pattern::new(g)
                    .map(|p| p.matches(path.to_string_lossy().as_ref()))
                    .unwrap_or(true)
            {
                continue;
            }

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            for (i, line) in content.lines().enumerate() {
                if regex.is_match(line) {
                    let rel = path.strip_prefix(cwd).unwrap_or(path).to_string_lossy();
                    results.push(format!("{}:{}:{}", rel, i + 1, line));
                    lines_hit += 1;
                    files_hit += 1;
                    if lines_hit >= 500 {
                        break 'outer;
                    }
                }
            }
        }

        Ok(ToolResult {
            content: if results.is_empty() {
                "No matches found.".to_string()
            } else {
                format!(
                    "{} match(es) in {} file(s):\n{}",
                    lines_hit,
                    files_hit,
                    results.join("\n")
                )
            },
        })
    }
}

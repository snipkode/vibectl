//! `search_code` — recursive code search with context lines.
//!
//! Complements `grep` with a more code-centric API: a `query` field (not
//! `regex`), a `context_lines` option that returns surrounding lines so the
//! agent sees the call-site in context, and structured per-file grouping.
//!
//! The `grep` tool still exists for power-users who want raw regex output;
//! `search_code` is the recommended first call when the agent needs to find
//! where a symbol or pattern is used.

use super::read_file::resolve_path;
use super::{Tool, ToolDef, ToolResult};
use anyhow::{Context, Result};
use regex::Regex;
use serde_json::{Value, json};
use std::path::Path;

pub struct SearchCode;

impl Tool for SearchCode {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "search_code",
            "Search source code recursively for a pattern. Returns matching lines grouped by \
             file with surrounding context lines. Use this to find where a function is called, \
             a type is used, or a constant is defined. Respects .gitignore.",
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Text or regex pattern to search for (e.g. 'UserService', 'fn handle_', 'TODO')"
                    },
                    "path": {
                        "type": "string",
                        "description": "Optional: subdirectory to search in (default: project root)"
                    },
                    "glob": {
                        "type": "string",
                        "description": "Optional: file glob filter (e.g. '*.rs', '*.{ts,tsx}')"
                    },
                    "context_lines": {
                        "type": "integer",
                        "description": "Number of lines to show before and after each match (default: 2, max: 5)",
                        "default": 2,
                        "minimum": 0,
                        "maximum": 5
                    },
                    "case_sensitive": {
                        "type": "boolean",
                        "description": "If false (default), search is case-insensitive",
                        "default": false
                    },
                    "max_matches": {
                        "type": "integer",
                        "description": "Maximum total matches to return (default: 100, max: 200)",
                        "default": 100,
                        "minimum": 1,
                        "maximum": 200
                    }
                },
                "required": ["query"]
            }),
        )
    }

    fn run(&self, args: &Value, cwd: &Path) -> Result<ToolResult> {
        let query = args
            .get("query")
            .context("missing 'query'")?
            .as_str()
            .context("'query' must be a string")?;

        let subdir = args.get("path").and_then(|v| v.as_str());
        let glob_filter = args.get("glob").and_then(|v| v.as_str());
        let context_lines = args
            .get("context_lines")
            .and_then(|v| v.as_u64())
            .map(|v| v.min(5) as usize)
            .unwrap_or(2);
        let case_sensitive = args
            .get("case_sensitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let max_matches = args
            .get("max_matches")
            .and_then(|v| v.as_u64())
            .map(|v| v.min(200) as usize)
            .unwrap_or(100);

        // Build regex — if it looks like a plain identifier, use word-boundary
        // matching to avoid noise (e.g. "User" matching "UserService").
        let pattern_str = if case_sensitive {
            query.to_string()
        } else {
            format!("(?i){query}")
        };
        let regex = Regex::new(&pattern_str)
            .with_context(|| format!("invalid search pattern: {query}"))?;

        let root = match subdir {
            Some(s) => resolve_path(cwd, s),
            None => cwd.to_path_buf(),
        };

        if !root.is_dir() {
            return Ok(ToolResult {
                content: format!("`{}` is not a directory.", root.display()),
            });
        }

        let glob_pat: Option<glob::Pattern> = glob_filter
            .map(glob::Pattern::new)
            .transpose()
            .with_context(|| "invalid glob pattern")?;

        // Walk files.
        let walker = ignore::WalkBuilder::new(&root)
            .hidden(false)
            .git_ignore(true)
            .build();

        let mut file_results: Vec<FileMatches> = Vec::new();
        let mut total_matches = 0usize;
        let mut total_files = 0usize;
        let mut truncated = false;

        'outer: for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }

            let path = entry.path();

            // Apply glob filter.
            if let Some(g) = &glob_pat {
                let rel = path.strip_prefix(&root).unwrap_or(path);
                if !g.matches(&rel.to_string_lossy()) {
                    continue;
                }
            }

            // Skip binary-ish files (no UTF-8).
            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let lines: Vec<&str> = content.lines().collect();
            let mut matches: Vec<MatchEntry> = Vec::new();

            for (i, line) in lines.iter().enumerate() {
                if regex.is_match(line) {
                    let ctx_start = i.saturating_sub(context_lines);
                    let ctx_end = (i + context_lines + 1).min(lines.len());

                    let context: Vec<ContextLine> = lines[ctx_start..ctx_end]
                        .iter()
                        .enumerate()
                        .map(|(j, l)| ContextLine {
                            line_no: ctx_start + j + 1,
                            text: l.to_string(),
                            is_match: ctx_start + j == i,
                        })
                        .collect();

                    matches.push(MatchEntry {
                        line_no: i + 1,
                        context,
                    });

                    total_matches += 1;
                    if total_matches >= max_matches {
                        truncated = true;
                        // Push collected matches before breaking out.
                        if !matches.is_empty() {
                            let rel = path.strip_prefix(cwd).unwrap_or(path).to_string_lossy().into_owned();
                            file_results.push(FileMatches { path: rel, matches });
                            total_files += 1;
                        }
                        break 'outer;
                    }
                }
            }

            if !matches.is_empty() {
                let rel = path.strip_prefix(cwd).unwrap_or(path).to_string_lossy().into_owned();
                file_results.push(FileMatches { path: rel, matches });
                total_files += 1;
            }
        }

        if file_results.is_empty() {
            return Ok(ToolResult {
                content: format!("No matches for `{query}` in {}.", root.display()),
            });
        }

        // Format output.
        let mut out = format!(
            "Found {} match(es) across {} file(s) for `{query}`:\n\n",
            total_matches, total_files
        );

        for fm in &file_results {
            out.push_str(&format!("── {} ──\n", fm.path));

            // Group consecutive context windows — deduplicate overlapping lines.
            let mut last_printed_line: Option<usize> = None;
            for entry in &fm.matches {
                // If there's a gap since the last match group, add a separator.
                let first_ctx_line = entry.context.first().map(|c| c.line_no).unwrap_or(entry.line_no);
                if let Some(last) = last_printed_line {
                    if first_ctx_line > last + 1 {
                        out.push_str("  ...\n");
                    }
                }

                for ctx in &entry.context {
                    // Don't repeat lines already printed from a previous match's context.
                    if let Some(last) = last_printed_line {
                        if ctx.line_no <= last {
                            continue;
                        }
                    }
                    let marker = if ctx.is_match { ">" } else { " " };
                    out.push_str(&format!("  {} {:4} │ {}\n", marker, ctx.line_no, ctx.text));
                    last_printed_line = Some(ctx.line_no);
                }
            }
            out.push('\n');
        }

        if truncated {
            out.push_str(&format!(
                "[Results truncated at {max_matches} matches — narrow your search with `path=` or `glob=`]\n"
            ));
        }

        Ok(ToolResult { content: out })
    }
}

// ─── Internal types ───────────────────────────────────────────────────────────

struct ContextLine {
    line_no: usize,
    text: String,
    is_match: bool,
}

struct MatchEntry {
    line_no: usize,
    context: Vec<ContextLine>,
}

struct FileMatches {
    path: String,
    matches: Vec<MatchEntry>,
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_file(dir: &TempDir, name: &str, content: &str) {
        std::fs::write(dir.path().join(name), content).unwrap();
    }

    #[test]
    fn finds_simple_pattern() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "main.rs", "fn main() {}\nfn helper() {}\n");

        let tool = SearchCode;
        let args = json!({ "query": "helper" });
        let result = tool.run(&args, dir.path()).unwrap();
        assert!(result.content.contains("helper"), "got: {}", result.content);
        assert!(result.content.contains("main.rs"), "got: {}", result.content);
    }

    #[test]
    fn case_insensitive_by_default() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "main.rs", "pub struct UserService {}\n");

        let tool = SearchCode;
        let args = json!({ "query": "userservice" });
        let result = tool.run(&args, dir.path()).unwrap();
        assert!(
            result.content.contains("UserService"),
            "got: {}",
            result.content
        );
    }

    #[test]
    fn case_sensitive_mode() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "main.rs", "pub struct UserService {}\n");

        let tool = SearchCode;
        // "userservice" in case-sensitive mode should NOT match "UserService".
        let args = json!({ "query": "userservice", "case_sensitive": true });
        let result = tool.run(&args, dir.path()).unwrap();
        assert!(
            result.content.contains("No matches"),
            "got: {}",
            result.content
        );
    }

    #[test]
    fn context_lines_included() {
        let dir = TempDir::new().unwrap();
        write_file(
            &dir,
            "lib.rs",
            "fn alpha() {}\nfn target() {}\nfn omega() {}\n",
        );

        let tool = SearchCode;
        let args = json!({ "query": "target", "context_lines": 1 });
        let result = tool.run(&args, dir.path()).unwrap();
        // Should include the line before (alpha) and after (omega) as context.
        assert!(result.content.contains("alpha"), "got: {}", result.content);
        assert!(result.content.contains("omega"), "got: {}", result.content);
    }

    #[test]
    fn glob_filter_limits_search() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "main.rs", "fn search_me() {}\n");
        write_file(&dir, "notes.txt", "search_me is here too\n");

        let tool = SearchCode;
        let args = json!({ "query": "search_me", "glob": "*.rs" });
        let result = tool.run(&args, dir.path()).unwrap();
        assert!(result.content.contains("main.rs"), "got: {}", result.content);
        // notes.txt should not appear since glob is *.rs
        assert!(
            !result.content.contains("notes.txt"),
            "txt file should be excluded, got: {}",
            result.content
        );
    }

    #[test]
    fn no_matches_returns_friendly_message() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "main.rs", "fn main() {}\n");

        let tool = SearchCode;
        let args = json!({ "query": "absolutely_nonexistent_xyz" });
        let result = tool.run(&args, dir.path()).unwrap();
        assert!(
            result.content.contains("No matches"),
            "got: {}",
            result.content
        );
    }

    #[test]
    fn max_matches_truncation() {
        // Use a non-dot-prefixed temp dir so the `ignore` walker does not
        // treat the root as a hidden directory.
        let dir = tempfile::Builder::new()
            .prefix("vibectl_test_")
            .tempdir()
            .unwrap();
        // Write a file with 50 matching lines.
        let content: String = (0..50).map(|i| format!("fn func_{i}() {{}}\n")).collect();
        std::fs::write(dir.path().join("big.rs"), &content).unwrap();

        let tool = SearchCode;
        let args = json!({ "query": "fn func_", "max_matches": 10 });
        let result = tool.run(&args, dir.path()).unwrap();
        assert!(
            result.content.contains("truncated"),
            "got: {}\n(dir: {:?})",
            result.content,
            dir.path()
        );
    }

    #[test]
    fn regex_pattern_works() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "main.rs", "pub fn get_user() {}\npub fn set_user() {}\n");

        let tool = SearchCode;
        let args = json!({ "query": "(get|set)_user" });
        let result = tool.run(&args, dir.path()).unwrap();
        assert!(result.content.contains("get_user"), "got: {}", result.content);
        assert!(result.content.contains("set_user"), "got: {}", result.content);
    }
}

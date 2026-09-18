use super::{Tool, ToolDef, ToolResult};
use anyhow::{Context, Result};
use serde_json::{Value, json};

pub struct ReadFile;

impl Tool for ReadFile {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "read_file",
            "Read a file from the filesystem and return its contents. Returns up to the first part of the file together with total line count. Absolute or project-relative paths allowed.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file to read" },
                    "offset": { "type": "integer", "description": "Line to start from (1-indexed). Default 1", "minimum": 1 },
                    "limit": { "type": "integer", "description": "Max number of lines to read. Default 200" }
                },
                "required": ["path"]
            }),
        )
    }

    fn run(&self, args: &Value, cwd: &std::path::Path) -> Result<ToolResult> {
        let path = args
            .get("path")
            .context("missing 'path'")?
            .as_str()
            .context("'path' must be a string")?;

        let offset = args
            .get("offset")
            .and_then(|v| v.as_u64())
            .map(|v| v.max(1) as usize)
            .unwrap_or(1);

        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v.max(1) as usize)
            .unwrap_or(200);

        let path = resolve_path(cwd, path);
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;

        let total_lines = content.lines().count();
        let lines: Vec<&str> = content.lines().collect();

        let start = (offset - 1).min(total_lines);
        let end = start.saturating_add(limit).min(total_lines);
        let slice = lines[start..end].join("\n");

        let output = if total_lines > end {
            format!(
                "{}\n\n[-- {} more lines ({}..{}) --]",
                slice,
                total_lines - end,
                end + 1,
                total_lines
            )
        } else {
            slice
        };

        Ok(ToolResult {
            content: format!(
                "=== {} ({} lines total, showing {}..{}) ===\n{}",
                path.display(),
                total_lines,
                start + 1,
                end,
                output
            ),
        })
    }
}

pub fn resolve_path(cwd: &std::path::Path, p: &str) -> std::path::PathBuf {
    let pb = std::path::PathBuf::from(p);
    if pb.is_absolute() { pb } else { cwd.join(pb) }
}

use super::read_file::resolve_path;
use super::{Tool, ToolDef, ToolResult};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::Path;

pub struct WriteFile;

impl Tool for WriteFile {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "write_file",
            "Write content to a file. Creates the file (and parent directories) if it does not exist; otherwise overwrites the given line range or the whole file.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to write" },
                    "content": { "type": "string", "description": "Full new content of the file" }
                },
                "required": ["path", "content"]
            }),
        )
    }

    fn run(&self, args: &Value, cwd: &Path) -> Result<ToolResult> {
        let path = args
            .get("path")
            .context("missing 'path'")?
            .as_str()
            .context("'path' must be a string")?;
        let content = args
            .get("content")
            .context("missing 'content'")?
            .as_str()
            .context("'content' must be a string")?;

        let path = resolve_path(cwd, path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create dir {}", parent.display()))?;
        }
        std::fs::write(&path, content)
            .with_context(|| format!("failed to write {}", path.display()))?;

        Ok(ToolResult {
            content: format!("Wrote {} bytes to {}", content.len(), path.display()),
        })
    }
}

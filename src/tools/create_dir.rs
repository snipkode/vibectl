use super::{Tool, ToolDef, ToolResult};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::Path;

pub struct CreateDir;

impl Tool for CreateDir {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "create_dir",
            "Create a directory (and parent directories if needed). Use this when scaffolding project structure before creating files. Returns success if the directory already exists.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory path to create (e.g. 'src/controllers' or 'tests')"
                    }
                },
                "required": ["path"]
            }),
        )
    }

    fn run(&self, args: &Value, cwd: &Path) -> Result<ToolResult> {
        let path_str = args
            .get("path")
            .context("missing 'path'")?
            .as_str()
            .context("'path' must be a string")?;

        let target = if Path::new(path_str).is_absolute() {
            Path::new(path_str).to_path_buf()
        } else {
            cwd.join(path_str)
        };

        // Create directory and all parent directories
        std::fs::create_dir_all(&target)
            .with_context(|| format!("failed to create directory: {}", target.display()))?;

        Ok(ToolResult {
            content: format!(
                "Created directory: {}\n\nPath: {}",
                path_str,
                target.display()
            ),
        })
    }
}

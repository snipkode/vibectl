pub mod git;
pub mod read_file;
pub mod search;
pub mod shell;
pub mod write_file;

use anyhow::Result;
use serde_json::Value;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

impl ToolDef {
    pub fn new(name: &str, description: &str, parameters: Value) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
        }
    }
}

pub struct ToolResult {
    pub content: String,
}

pub trait Tool: Sync + Send {
    fn def(&self) -> ToolDef;
    fn run(&self, args: &Value, cwd: &std::path::Path) -> Result<ToolResult>;
}

pub fn all_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(git::GitTool),
        Box::new(read_file::ReadFile),
        Box::new(write_file::WriteFile),
        Box::new(search::GlobFiles),
        Box::new(search::GrepFiles),
        Box::new(shell::ShellExec),
    ]
}

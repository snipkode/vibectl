#![allow(dead_code)]

use std::path::Path;

use crate::agent::executor::{ExecutionHistory, ValidationResult};
use crate::workspace::{ProjectType, WorkspaceContext};

/// Status of the implementation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImplementationStatus {
    Completed,
    PartiallyCompleted,
    Blocked,
    Failed,
}

impl ImplementationStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Completed => "✅ Completed",
            Self::PartiallyCompleted => "⚠️ Partially Completed",
            Self::Blocked => "🚫 Blocked",
            Self::Failed => "❌ Failed",
        }
    }
}

/// Comprehensive implementation report
#[derive(Debug, Clone)]
pub struct ImplementationReport {
    pub status: ImplementationStatus,
    pub workspace: WorkspaceContext,
    pub files_created: Vec<String>,
    pub files_modified: Vec<String>,
    pub dependencies_installed: Vec<String>,
    pub validation_results: Vec<ValidationResult>,
    pub iterations_count: usize,
    pub error_summary: Option<String>,
    pub blocking_issue: Option<String>,
    pub what_was_implemented: Vec<String>,
    pub notes: Vec<String>,
}

impl ImplementationReport {
    pub fn new(workspace: WorkspaceContext) -> Self {
        Self {
            status: ImplementationStatus::Completed,
            workspace,
            files_created: Vec::new(),
            files_modified: Vec::new(),
            dependencies_installed: Vec::new(),
            validation_results: Vec::new(),
            iterations_count: 0,
            error_summary: None,
            blocking_issue: None,
            what_was_implemented: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Set status based on validation results
    pub fn determine_status(&mut self) {
        let all_passed = self
            .validation_results
            .iter()
            .filter(|r| r.step_name.to_lowercase().contains("test")
                || r.step_name.to_lowercase().contains("build"))
            .all(|r| r.success);

        let has_failures = self.validation_results.iter().any(|r| !r.success);

        if all_passed && !has_failures {
            self.status = ImplementationStatus::Completed;
        } else if self.blocking_issue.is_some() {
            self.status = ImplementationStatus::Blocked;
        } else if has_failures {
            self.status = ImplementationStatus::Failed;
        } else {
            self.status = ImplementationStatus::PartiallyCompleted;
        }
    }

    /// Add history information to the report
    pub fn set_history(&mut self, history: &ExecutionHistory) {
        self.iterations_count = history.iteration_count();
        if let Some(error) = history.last_error() {
            self.error_summary = Some(error);
        }
    }

    /// Generate Markdown report
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();

        // Header
        md.push_str("# 🤖 Implementation Summary\n\n");

        // Status
        md.push_str("## Status\n\n");
        md.push_str(&format!("{}\n\n", self.status.as_str()));

        // Workspace Info
        md.push_str("## Workspace\n\n");
        md.push_str(&format!("- **Project Type**: {:?}\n", self.workspace.project_type));
        md.push_str(&format!("- **Root**: `{}`\n", self.workspace.root.display()));
        md.push_str(&format!(
            "- **Status**: {}\n\n",
            if self.workspace.is_existing {
                "Existing Project"
            } else {
                "New Project"
            }
        ));

        // What Was Implemented
        if !self.what_was_implemented.is_empty() {
            md.push_str("## What Was Implemented\n\n");
            for item in &self.what_was_implemented {
                md.push_str(&format!("- {}\n", item));
            }
            md.push_str("\n");
        }

        // Project Structure
        md.push_str("## Project Structure\n\n");
        md.push_str("```\n");
        md.push_str(&self.generate_structure_tree());
        md.push_str("```\n\n");

        // Files Created/Modified
        if !self.files_created.is_empty() || !self.files_modified.is_empty() {
            md.push_str("## File Changes\n\n");
            
            if !self.files_created.is_empty() {
                md.push_str("### Created\n\n");
                for file in &self.files_created {
                    md.push_str(&format!("- `{}`\n", file));
                }
                md.push_str("\n");
            }

            if !self.files_modified.is_empty() {
                md.push_str("### Modified\n\n");
                for file in &self.files_modified {
                    md.push_str(&format!("- `{}`\n", file));
                }
                md.push_str("\n");
            }
        }

        // Dependencies
        if !self.dependencies_installed.is_empty() {
            md.push_str("## Dependencies\n\n");
            for dep in &self.dependencies_installed {
                md.push_str(&format!("- {}\n", dep));
            }
            md.push_str("\n");
        }

        // Validation Results
        if !self.validation_results.is_empty() {
            md.push_str("## Validation Results\n\n");
            md.push_str("| Step | Status | Duration | Exit Code |\n");
            md.push_str("|------|--------|----------|----------|\n");
            for result in &self.validation_results {
                let status_icon = if result.success { "✅ PASS" } else { "❌ FAIL" };
                md.push_str(&format!(
                    "| {} | {} | {:.2}s | {} |\n",
                    result.step_name, status_icon, result.duration_secs, result.exit_code
                ));
            }
            md.push_str("\n");
        }

        // Test Summary
        let test_results: Vec<_> = self
            .validation_results
            .iter()
            .filter(|r| r.step_name.to_lowercase().contains("test"))
            .collect();

        if !test_results.is_empty() {
            md.push_str("## Test Summary\n\n");
            let passed = test_results.iter().filter(|r| r.success).count();
            let failed = test_results.len() - passed;
            md.push_str(&format!("- **Passed**: {}\n", passed));
            md.push_str(&format!("- **Failed**: {}\n", failed));
            md.push_str("\n");
        }

        // Execution Stats
        if self.iterations_count > 0 {
            md.push_str("## Execution Stats\n\n");
            md.push_str(&format!("- **Iterations**: {}\n", self.iterations_count));
            md.push_str(&format!(
                "- **Max Iterations**: {}\n",
                crate::agent::executor::MAX_ITERATIONS
            ));
            md.push_str("\n");
        }

        // Error Summary (if failed)
        if let Some(error) = &self.error_summary {
            md.push_str("## Error Summary\n\n");
            md.push_str("```\n");
            md.push_str(&Self::truncate(error, 2000));
            md.push_str("\n```\n\n");
        }

        // Blocking Issue
        if let Some(blocker) = &self.blocking_issue {
            md.push_str("## 🚫 Blocking Issue\n\n");
            md.push_str(blocker);
            md.push_str("\n\n");
        }

        // Notes
        if !self.notes.is_empty() {
            md.push_str("## Notes\n\n");
            for note in &self.notes {
                md.push_str(&format!("- {}\n", note));
            }
            md.push_str("\n");
        }

        // Commands to Run
        md.push_str("## Commands to Run\n\n");
        md.push_str(&self.generate_commands_section());

        // Footer
        md.push_str("---\n\n");
        md.push_str(&format!(
            "*Generated by vibectl autonomous agent*\n"
        ));

        md
    }

    /// Generate project structure tree
    fn generate_structure_tree(&self) -> String {
        let mut tree = String::new();
        
        // Show manifest files
        for manifest in &self.workspace.manifest_files {
            if let Some(name) = manifest.file_name() {
                tree.push_str(&format!("{}\n", name.to_string_lossy()));
            }
        }

        // Show source directories
        for src_dir in &self.workspace.source_dirs {
            if let Some(name) = src_dir.file_name() {
                tree.push_str(&format!("{}/\n", name.to_string_lossy()));
            }
        }

        // Show created files in simplified tree
        let mut paths: Vec<_> = self
            .files_created
            .iter()
            .chain(&self.files_modified)
            .collect();
        paths.sort();

        let mut shown = std::collections::HashSet::new();
        for path in paths {
            if let Some(parent) = Path::new(path).parent() {
                let parent_str = parent.to_string_lossy().to_string();
                if !parent_str.is_empty() && shown.insert(parent_str.clone()) {
                    tree.push_str(&format!("├── {}/\n", parent_str));
                }
            }
            tree.push_str(&format!("│   └── {}\n", Path::new(path).file_name().unwrap().to_string_lossy()));
        }

        if tree.is_empty() {
            tree.push_str("(no structure information available)\n");
        }

        tree
    }

    /// Generate commands section based on project type
    fn generate_commands_section(&self) -> String {
        let mut cmds = String::new();

        match self.workspace.project_type {
            ProjectType::NodeJs => {
                cmds.push_str("```bash\n");
                cmds.push_str("# Install dependencies\n");
                cmds.push_str("npm install\n\n");
                cmds.push_str("# Run tests\n");
                cmds.push_str("npm test\n\n");
                cmds.push_str("# Build (if applicable)\n");
                cmds.push_str("npm run build\n\n");
                cmds.push_str("# Start development server\n");
                cmds.push_str("npm run dev\n");
                cmds.push_str("```\n");
            }
            ProjectType::Rust => {
                cmds.push_str("```bash\n");
                cmds.push_str("# Build project\n");
                cmds.push_str("cargo build\n\n");
                cmds.push_str("# Run tests\n");
                cmds.push_str("cargo test\n\n");
                cmds.push_str("# Run application\n");
                cmds.push_str("cargo run\n");
                cmds.push_str("```\n");
            }
            ProjectType::Go => {
                cmds.push_str("```bash\n");
                cmds.push_str("# Download dependencies\n");
                cmds.push_str("go mod download\n\n");
                cmds.push_str("# Run tests\n");
                cmds.push_str("go test ./...\n\n");
                cmds.push_str("# Build\n");
                cmds.push_str("go build ./...\n\n");
                cmds.push_str("# Run\n");
                cmds.push_str("go run .\n");
                cmds.push_str("```\n");
            }
            ProjectType::Python => {
                cmds.push_str("```bash\n");
                cmds.push_str("# Install dependencies\n");
                cmds.push_str("pip install -r requirements.txt\n\n");
                cmds.push_str("# Run tests\n");
                cmds.push_str("pytest\n\n");
                cmds.push_str("# Run application\n");
                cmds.push_str("python main.py\n");
                cmds.push_str("```\n");
            }
            ProjectType::Unknown => {
                cmds.push_str("*(No commands available — project type not detected)*\n");
            }
        }

        cmds
    }

    /// Truncate text to max length
    fn truncate(text: &str, max: usize) -> String {
        if text.len() <= max {
            text.to_string()
        } else {
            format!("{}...\n[truncated]", &text[..max])
        }
    }

    /// Generate a concise summary for terminal output
    pub fn to_summary(&self) -> String {
        let mut summary = String::new();

        summary.push_str("╔═══════════════════════════════════════════════════════════════╗\n");
        summary.push_str(&format!(
            "║ IMPLEMENTATION SUMMARY: {:43} ║\n",
            self.status.as_str()
        ));
        summary.push_str("╚═══════════════════════════════════════════════════════════════╝\n\n");

        if !self.what_was_implemented.is_empty() {
            summary.push_str("What was implemented:\n");
            for item in &self.what_was_implemented {
                summary.push_str(&format!("  • {}\n", item));
            }
            summary.push('\n');
        }

        if !self.files_created.is_empty() {
            summary.push_str(&format!("Files created: {}\n", self.files_created.len()));
        }

        if !self.files_modified.is_empty() {
            summary.push_str(&format!("Files modified: {}\n", self.files_modified.len()));
        }

        if !self.validation_results.is_empty() {
            let passed = self.validation_results.iter().filter(|r| r.success).count();
            let total = self.validation_results.len();
            summary.push_str(&format!("\nValidation: {}/{} steps passed\n", passed, total));
        }

        if self.iterations_count > 0 {
            summary.push_str(&format!("Iterations: {}\n", self.iterations_count));
        }

        if let Some(blocker) = &self.blocking_issue {
            summary.push_str(&format!("\n⚠️  Blocking issue:\n{}\n", blocker));
        }

        summary
    }
}

/// Helper to build a report from execution results
pub struct ReportBuilder {
    report: ImplementationReport,
}

impl ReportBuilder {
    pub fn new(workspace: WorkspaceContext) -> Self {
        Self {
            report: ImplementationReport::new(workspace),
        }
    }

    pub fn add_created_file(&mut self, path: String) -> &mut Self {
        self.report.files_created.push(path);
        self
    }

    pub fn add_modified_file(&mut self, path: String) -> &mut Self {
        self.report.files_modified.push(path);
        self
    }

    pub fn add_dependency(&mut self, dep: String) -> &mut Self {
        self.report.dependencies_installed.push(dep);
        self
    }

    pub fn add_implementation(&mut self, desc: String) -> &mut Self {
        self.report.what_was_implemented.push(desc);
        self
    }

    pub fn add_note(&mut self, note: String) -> &mut Self {
        self.report.notes.push(note);
        self
    }

    pub fn set_validation_results(&mut self, results: Vec<ValidationResult>) -> &mut Self {
        self.report.validation_results = results;
        self
    }

    pub fn set_history(&mut self, history: &ExecutionHistory) -> &mut Self {
        self.report.set_history(history);
        self
    }

    pub fn set_blocking_issue(&mut self, issue: String) -> &mut Self {
        self.report.blocking_issue = Some(issue);
        self
    }

    pub fn build(mut self) -> ImplementationReport {
        self.report.determine_status();
        self.report
    }
}

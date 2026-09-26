/// Autonomous agent integration module
///
/// This module integrates the autonomous coding agent capabilities
/// into the main agent loop, enabling automatic error recovery and
/// validation workflows.
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

use super::{Agent, AgentEvent};
use crate::agent::executor::{AnalysisReport, MAX_ITERATIONS};
use crate::agent::report::{ImplementationReport, ReportBuilder};
use crate::agent::validation::{Fixer, ValidationWorkflow};
use crate::workspace::WorkspaceContext;

/// Track file operations during agent execution
#[derive(Debug, Default, Clone)]
pub struct FileOperationTracker {
    pub files_created: Vec<String>,
    pub files_modified: Vec<String>,
    pub dependencies_added: Vec<String>,
}

impl FileOperationTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn track_write(&mut self, path: &str, existed: bool) {
        if existed {
            self.files_modified.push(path.to_string());
        } else {
            self.files_created.push(path.to_string());
        }
    }

    pub fn track_dependency(&mut self, dep: &str) {
        self.dependencies_added.push(dep.to_string());
    }

    pub fn has_changes(&self) -> bool {
        !self.files_created.is_empty() || !self.files_modified.is_empty()
    }
}

/// Autonomous execution context
#[derive(Clone)]
pub struct AutonomousContext {
    pub workspace: WorkspaceContext,
    pub tracker: FileOperationTracker,
    pub validation_enabled: bool,
    pub auto_fix_enabled: bool,
}

impl AutonomousContext {
    /// Create new autonomous context from workspace path
    pub fn new(cwd: &Path) -> Result<Self> {
        let workspace = WorkspaceContext::new(cwd).context("Failed to create workspace context")?;

        Ok(Self {
            workspace,
            tracker: FileOperationTracker::new(),
            validation_enabled: true,
            auto_fix_enabled: true,
        })
    }

    /// Check if autonomous validation should run
    pub fn should_run_validation(&self) -> bool {
        self.validation_enabled
            && self.tracker.has_changes()
            && (self.workspace.is_existing || !self.workspace.manifest_files.is_empty())
    }

    /// Helper to check if file exists using Agent utility
    pub fn file_exists_at(&self, path: &str) -> bool {
        Agent::file_exists(&self.workspace.root, path)
    }

    /// Extract path from tool arguments using Agent utility
    pub fn extract_path(&self, args: &serde_json::Value) -> Option<String> {
        Agent::extract_path_from_args(args)
    }
}

impl Agent {
    /// Run autonomous validation workflow after file operations
    ///
    /// This is the main integration point for autonomous behavior.
    /// Call this after a batch of file writes to trigger validation
    /// and automatic error recovery.
    pub async fn run_autonomous_validation(
        &self,
        context: &AutonomousContext,
        tx: &mpsc::Sender<AgentEvent>,
    ) -> Result<ImplementationReport> {
        // Create validation workflow
        let mut workflow = ValidationWorkflow::new(context.workspace.clone());

        // Send notification
        let _ = tx
            .send(AgentEvent::Text(
                "\n╔═══════════════════════════════════════════════════════════════╗\n\
                 ║           AUTONOMOUS VALIDATION WORKFLOW                      ║\n\
                 ╚═══════════════════════════════════════════════════════════════╝\n\n"
                    .to_string(),
            ))
            .await;

        // Honour `auto_fix: false`. Without a fixer the workflow still runs
        // the validation plan, it just reports the failures instead of
        // spending turns trying to repair them.
        let report = if context.auto_fix_enabled {
            // Each fix round is one agent turn. `run_inner` is safe to re-enter
            // here: it only auto-plans on an empty message history, and by this
            // point the implementation turn has already populated it.
            let agent = self.clone();
            let event_tx = tx.clone();
            let mut fixer: Box<Fixer<'_>> =
                Box::new(move |iteration, analysis: &AnalysisReport| {
                    // Build the prompt eagerly so the returned future owns all
                    // of its data: it must not borrow `analysis`, and the
                    // closure is FnMut so it can be called repeatedly.
                    let instruction = format!(
                        "Validation iteration {iteration} failed. Fix every problem listed \
                         below, using the tools to inspect the real files before editing. \
                         Change only what the failures point at, then stop.\n\n{}",
                        analysis.format_for_llm()
                    );
                    let agent = agent.clone();
                    let event_tx = event_tx.clone();
                    Box::pin(async move {
                        agent
                            .run_inner(event_tx, instruction, iteration as u64)
                            .await
                    })
                });
            workflow.run_with_fixer(tx, Some(&mut *fixer)).await?
        } else {
            workflow.run(tx).await?
        };

        // Generate report summary
        let _ = tx
            .send(AgentEvent::Text(format!("\n{}", report.to_summary())))
            .await;

        Ok(report)
    }

    /// Build implementation report from tracked operations
    pub fn build_implementation_report(&self, context: &AutonomousContext) -> ImplementationReport {
        let mut builder = ReportBuilder::new(context.workspace.clone());

        // Add tracked file operations
        for file in &context.tracker.files_created {
            builder.add_created_file(file.clone());
        }

        for file in &context.tracker.files_modified {
            builder.add_modified_file(file.clone());
        }

        for dep in &context.tracker.dependencies_added {
            builder.add_dependency(dep.clone());
        }

        builder.build()
    }

    /// Extract path from tool call arguments
    pub fn extract_path_from_args(args: &serde_json::Value) -> Option<String> {
        args.get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// Check if a file exists at the given path
    pub fn file_exists(cwd: &Path, path: &str) -> bool {
        let target = crate::tools::read_file::resolve_path(cwd, path);
        target.exists()
    }

    /// Save implementation report to file
    pub async fn save_implementation_report(
        &self,
        report: &ImplementationReport,
        tx: &mpsc::Sender<AgentEvent>,
    ) -> Result<PathBuf> {
        let root =
            crate::agent::steer::find_project_root(&self.cwd).unwrap_or_else(|| self.cwd.clone());

        let report_path = root.join("IMPLEMENTATION_SUMMARY.md");
        let markdown = report.to_markdown();

        std::fs::write(&report_path, markdown)
            .with_context(|| format!("Failed to write report to {}", report_path.display()))?;

        let _ = tx
            .send(AgentEvent::Text(format!(
                "\n📄 Implementation report saved to: {}\n",
                report_path.display()
            )))
            .await;

        Ok(report_path)
    }
}

/// Helper to determine if validation should trigger based on intent and operations
pub fn should_trigger_validation(
    intent: &crate::agent::intent::Intent,
    tracker: &FileOperationTracker,
) -> bool {
    use crate::agent::intent::Intent;

    match intent {
        Intent::Conversational | Intent::Informational => false,
        Intent::CodeWrite | Intent::Refactor => tracker.has_changes(),
        Intent::ShellExec | Intent::GitOp | Intent::Deploy => false,
    }
}

/// Helper to format validation trigger message
pub fn format_validation_trigger_message(tracker: &FileOperationTracker) -> String {
    let mut msg =
        String::from("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    msg.push_str("🔄 File operations completed. Triggering autonomous validation...\n");
    msg.push_str("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\n");

    if !tracker.files_created.is_empty() {
        msg.push_str(&format!(
            "📝 Files created: {}\n",
            tracker.files_created.len()
        ));
    }

    if !tracker.files_modified.is_empty() {
        msg.push_str(&format!(
            "✏️  Files modified: {}\n",
            tracker.files_modified.len()
        ));
    }

    msg.push('\n');
    msg
}

/// Configuration for autonomous behavior
#[derive(Debug, Clone)]
pub struct AutonomousConfig {
    /// Enable automatic validation after file operations
    pub auto_validate: bool,

    /// Enable automatic error fixing (requires auto_validate)
    pub auto_fix: bool,

    /// Maximum iterations for error recovery
    pub max_iterations: u32,

    /// Generate implementation reports
    pub generate_reports: bool,
}

impl Default for AutonomousConfig {
    fn default() -> Self {
        Self {
            auto_validate: true,
            auto_fix: true,
            max_iterations: MAX_ITERATIONS,
            generate_reports: true,
        }
    }
}

impl AutonomousConfig {
    /// Create config for headless/CI mode (full automation)
    pub fn headless() -> Self {
        Self {
            auto_validate: true,
            auto_fix: true,
            max_iterations: MAX_ITERATIONS,
            generate_reports: true,
        }
    }

    /// Create config for interactive mode (validation only, no auto-fix)
    pub fn interactive() -> Self {
        Self {
            auto_validate: true,
            auto_fix: false,
            max_iterations: 1,
            generate_reports: false,
        }
    }

    /// Disable all autonomous features (useful for testing)
    pub fn disabled() -> Self {
        Self {
            auto_validate: false,
            auto_fix: false,
            max_iterations: 0,
            generate_reports: false,
        }
    }

    /// Check if this config has autonomous features enabled
    pub fn is_enabled(&self) -> bool {
        self.auto_validate || self.auto_fix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_tracker() {
        let mut tracker = FileOperationTracker::new();
        assert!(!tracker.has_changes());

        tracker.track_write("src/app.js", false);
        assert!(tracker.has_changes());
        assert_eq!(tracker.files_created.len(), 1);
        assert_eq!(tracker.files_modified.len(), 0);

        tracker.track_write("src/app.js", true);
        assert_eq!(tracker.files_created.len(), 1);
        assert_eq!(tracker.files_modified.len(), 1);
    }

    #[test]
    fn test_autonomous_config() {
        let headless = AutonomousConfig::headless();
        assert!(headless.auto_validate);
        assert!(headless.auto_fix);

        let interactive = AutonomousConfig::interactive();
        assert!(interactive.auto_validate);
        assert!(!interactive.auto_fix);

        let disabled = AutonomousConfig::disabled();
        assert!(!disabled.auto_validate);
    }
}

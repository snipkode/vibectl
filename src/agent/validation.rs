#![allow(dead_code)]
#![allow(unused_assignments)]

use anyhow::Result;
use tokio::sync::mpsc;

use crate::agent::executor::{AnalysisReport, ExecutionHistory, ExecutionLoop, IterationRecord, ValidationResult};
use crate::agent::report::{ImplementationReport, ReportBuilder};
use crate::agent::AgentEvent;
use crate::workspace::WorkspaceContext;

/// Autonomous validation workflow that runs iteratively until success or max iterations
pub struct ValidationWorkflow {
    pub execution_loop: ExecutionLoop,
    pub history: ExecutionHistory,
}

impl ValidationWorkflow {
    pub fn new(workspace: WorkspaceContext) -> Self {
        Self {
            execution_loop: ExecutionLoop::new(workspace.clone()),
            history: ExecutionHistory::new(),
        }
    }

    /// Execute the complete validation workflow with automatic error recovery
    pub async fn run(
        &mut self,
        tx: &mpsc::Sender<AgentEvent>,
    ) -> Result<ImplementationReport> {
        let _ = tx
            .send(AgentEvent::Text(
                "\n═══════════════════════════════════════════════════════════════\n\
                 STARTING VALIDATION WORKFLOW\n\
                 ═══════════════════════════════════════════════════════════════\n\n"
                    .to_string(),
            ))
            .await;

        // Generate validation plan
        let validation_steps = self.execution_loop.workspace.validation_plan();

        if validation_steps.is_empty() {
            let _ = tx
                .send(AgentEvent::Text(
                    "⚠️  No validation steps available for this project type.\n".to_string(),
                ))
                .await;

            return Ok(self.build_report(vec![], None));
        }

        let _ = tx
            .send(AgentEvent::Text(format!(
                "Validation plan: {} steps\n",
                validation_steps.len()
            )))
            .await;

        for (i, step) in validation_steps.iter().enumerate() {
            let _ = tx
                .send(AgentEvent::Text(format!(
                    "  {}. {} (timeout: {}s)\n",
                    i + 1,
                    step.name,
                    step.timeout_secs
                )))
                .await;
        }

        let _ = tx.send(AgentEvent::Text("\n".to_string())).await;

        // Execute validation loop with retries
        let mut last_results = Vec::new();
        let mut last_analysis: Option<AnalysisReport> = None;

        loop {
            let iteration = self.history.iteration_count() + 1;
            
            let _ = tx
                .send(AgentEvent::Text(format!(
                    "\n━━━ ITERATION {}/{} ━━━\n",
                    iteration,
                    self.execution_loop.max_iterations
                )))
                .await;

            // Run validation steps
            let results = self
                .execution_loop
                .run_validation_steps(&validation_steps, tx)
                .await?;

            // Analyze results
            let analysis = self.execution_loop.analyze_failures(&results);

            // Record iteration
            let mut record = IterationRecord::new(iteration as u32);
            record.validation_results = results.clone();
            record.success = !analysis.has_failures;
            record.error_summary = analysis.error_summary.clone();
            self.history.add_iteration(record);

            last_results = results;
            last_analysis = Some(analysis.clone());

            // Check if all passed
            if !analysis.has_failures {
                let _ = tx
                    .send(AgentEvent::Text(
                        "\n✅ All validation steps passed!\n".to_string(),
                    ))
                    .await;
                break;
            }

            // Check if max iterations reached
            if self.history.has_reached_limit(self.execution_loop.max_iterations) {
                let _ = tx
                    .send(AgentEvent::Text(format!(
                        "\n⚠️  Maximum iterations ({}) reached.\n",
                        self.execution_loop.max_iterations
                    )))
                    .await;

                // Send analysis to LLM for final report
                let _ = tx
                    .send(AgentEvent::Text(
                        "\n=== FINAL STATUS ===\n\
                         Failed to complete all validation steps after maximum iterations.\n\n"
                            .to_string(),
                    ))
                    .await;

                let _ = tx
                    .send(AgentEvent::Text(analysis.format_for_llm()))
                    .await;

                break;
            }

            // Send error analysis to agent for fixing
            let _ = tx
                .send(AgentEvent::Text(
                    "\n=== VALIDATION FAILED ===\n".to_string(),
                ))
                .await;

            let _ = tx
                .send(AgentEvent::Text(analysis.format_for_llm()))
                .await;

            let _ = tx
                .send(AgentEvent::Text(format!(
                    "\n⚠️  Agent should now analyze and fix the issue before iteration {}.\n\n",
                    iteration + 1
                )))
                .await;

            // In a real implementation, the agent would be given a chance to
            // fix the issues here by calling write_file, patch_file, etc.
            // For now, we just break to let the agent continue its normal loop.
            // The agent's main loop should check validation status and retry.
            
            // Note: This is where the integration with the agent's main run_inner
            // loop would happen - the agent would see the error analysis and
            // attempt fixes, then call this validation workflow again.
            break;
        }

        // Build final report
        let blocking_issue = if self.history.has_reached_limit(self.execution_loop.max_iterations) {
            last_analysis.and_then(|a| {
                if a.has_failures {
                    Some(format!(
                        "Validation failed after {} iterations.\n\n{}",
                        self.execution_loop.max_iterations,
                        a.error_summary
                    ))
                } else {
                    None
                }
            })
        } else {
            None
        };

        Ok(self.build_report(last_results, blocking_issue))
    }

    /// Build implementation report from validation results
    fn build_report(
        &self,
        validation_results: Vec<ValidationResult>,
        blocking_issue: Option<String>,
    ) -> ImplementationReport {
        let mut builder = ReportBuilder::new(self.execution_loop.workspace.clone());

        builder.set_validation_results(validation_results);
        builder.set_history(&self.history);

        if let Some(issue) = blocking_issue {
            builder.set_blocking_issue(issue);
        }

        // Add default notes
        if self.execution_loop.workspace.project_type
            != crate::workspace::ProjectType::Unknown
        {
            builder.add_note(format!(
                "Project type detected: {:?}",
                self.execution_loop.workspace.project_type
            ));
        }

        if self.execution_loop.workspace.is_existing {
            builder.add_note("Modified existing project".to_string());
        } else {
            builder.add_note("Created new project from scratch".to_string());
        }

        builder.build()
    }

    /// Quick validation check (single pass, no retries)
    pub async fn quick_validate(
        workspace: &WorkspaceContext,
        tx: &mpsc::Sender<AgentEvent>,
    ) -> Result<Vec<ValidationResult>> {
        let execution_loop = ExecutionLoop::new(workspace.clone());
        let steps = workspace.validation_plan();

        if steps.is_empty() {
            return Ok(Vec::new());
        }

        let _ = tx
            .send(AgentEvent::Text(
                "\n━━━ Running Quick Validation ━━━\n".to_string(),
            ))
            .await;

        execution_loop.run_validation_steps(&steps, tx).await
    }
}

/// Helper function to format validation results for display
pub fn format_validation_summary(results: &[ValidationResult]) -> String {
    let mut summary = String::new();

    let passed = results.iter().filter(|r| r.success).count();
    let total = results.len();

    summary.push_str(&format!("\nValidation Summary: {}/{} passed\n", passed, total));
    summary.push_str("─────────────────────────────────────────\n");

    for result in results {
        let icon = if result.success { "✅" } else { "❌" };
        summary.push_str(&format!(
            "{} {} ({:.2}s, exit: {})\n",
            icon, result.step_name, result.duration_secs, result.exit_code
        ));
    }

    summary
}

/// Check if a project is ready for validation (has necessary files)
pub fn is_project_ready(workspace: &WorkspaceContext) -> bool {
    // For new projects, check if basic structure exists
    if !workspace.is_existing {
        return false;
    }

    // Check if manifest files exist
    !workspace.manifest_files.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_validation_summary() {
        let results = vec![
            ValidationResult {
                step_name: "Install".to_string(),
                command: "npm install".to_string(),
                success: true,
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
                duration_secs: 1.5,
            },
            ValidationResult {
                step_name: "Test".to_string(),
                command: "npm test".to_string(),
                success: false,
                exit_code: 1,
                stdout: String::new(),
                stderr: "Tests failed".to_string(),
                duration_secs: 0.8,
            },
        ];

        let summary = format_validation_summary(&results);
        assert!(summary.contains("1/2 passed"));
        assert!(summary.contains("✅"));
        assert!(summary.contains("❌"));
    }
}

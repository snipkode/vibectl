#![allow(dead_code)]
#![allow(unused_assignments)]

use std::collections::hash_map::DefaultHasher;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::PathBuf;
use std::pin::Pin;

use anyhow::Result;
use tokio::sync::mpsc;

use crate::agent::AgentEvent;
use crate::agent::executor::{
    AnalysisReport, ExecutionHistory, ExecutionLoop, IterationRecord, ValidationResult,
};
use crate::agent::report::{ImplementationReport, ReportBuilder};
use crate::workspace::WorkspaceContext;

/// Hands a failed validation pass back to the agent so it can attempt a fix.
///
/// `iteration` is 1-based (the pass that just failed) and `analysis` describes
/// what went wrong. Returning `Err` aborts the workflow.
pub type Fixer<'a> = dyn FnMut(usize, &AnalysisReport) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>
    + Send
    + 'a;

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

    /// Execute the validation workflow once, with no fix attempts.
    ///
    /// Use [`Self::run_with_fixer`] for the retry behaviour; this entry point
    /// exists for callers that only want a pass/fail verdict.
    pub async fn run(&mut self, tx: &mpsc::Sender<AgentEvent>) -> Result<ImplementationReport> {
        self.run_with_fixer(tx, None).await
    }

    /// Execute the complete validation workflow with automatic error recovery.
    ///
    /// Validation runs, and on failure the `fixer` gets a chance to repair the
    /// workspace before the steps run again. Iteration stops when the steps
    /// pass, when `max_iterations` is reached, when a fix round changes nothing
    /// on disk, or when the fixer returns an error.
    pub async fn run_with_fixer(
        &mut self,
        tx: &mpsc::Sender<AgentEvent>,
        mut fixer: Option<&mut Fixer<'_>>,
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

        // Set when we stop because a fix round touched nothing, so the report
        // can explain *why* the loop ended rather than just "failed".
        let mut stalled: Option<String> = None;
        let mut fingerprint = self.fingerprint();

        loop {
            let iteration = self.history.iteration_count() + 1;

            let _ = tx
                .send(AgentEvent::Text(format!(
                    "\n━━━ ITERATION {}/{} ━━━\n",
                    iteration, self.execution_loop.max_iterations
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
            if self
                .history
                .has_reached_limit(self.execution_loop.max_iterations)
            {
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

                let _ = tx.send(AgentEvent::Text(analysis.format_for_llm())).await;

                break;
            }

            // Hand the failure to the fixer so the agent can actually repair
            // it. Without one there is nothing to retry, so stop here instead
            // of burning iterations on a pass we already know the answer to.
            let Some(fixer) = fixer.as_deref_mut() else {
                let _ = tx
                    .send(AgentEvent::Text(
                        "\n=== VALIDATION FAILED (no auto-fix available) ===\n".to_string(),
                    ))
                    .await;
                let _ = tx.send(AgentEvent::Text(analysis.format_for_llm())).await;
                stalled = Some(format!(
                    "Validation failed with no auto-fix channel configured.\n\n{}",
                    analysis.error_summary
                ));
                break;
            };

            let _ = tx
                .send(AgentEvent::Text(
                    "\n=== VALIDATION FAILED — ATTEMPTING FIX ===\n".to_string(),
                ))
                .await;
            let _ = tx.send(AgentEvent::Text(analysis.format_for_llm())).await;

            // A fixer error is a real failure of the workflow, not a
            // validation verdict: propagate it rather than reporting a
            // misleading "gave up after N iterations".
            fixer(iteration, &analysis).await?;

            let after = self.fingerprint();
            if after == fingerprint {
                stalled = Some(format!(
                    "The agent was given {iteration} round(s) of failure details but did not \
                     change any source file, so re-running the same plan would be pointless."
                ));
                let _ = tx
                    .send(AgentEvent::Text(format!(
                        "\n⚠️  {}\n",
                        stalled.as_deref().unwrap_or("No progress.")
                    )))
                    .await;
                break;
            }
            fingerprint = after;

            let _ = tx
                .send(AgentEvent::Text(format!(
                    "\n🔄  Workspace changed — re-running validation (iteration {}).\n",
                    iteration + 1
                )))
                .await;
        }

        // Build final report
        let blocking_issue = if let Some(stalled) = stalled {
            Some(stalled)
        } else if self
            .history
            .has_reached_limit(self.execution_loop.max_iterations)
        {
            last_analysis.and_then(|a| {
                if a.has_failures {
                    Some(format!(
                        "Validation failed after {} iterations.\n\n{}",
                        self.execution_loop.max_iterations, a.error_summary
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

    /// Cheap content hash of the workspace's source files.
    ///
    /// Used to tell "the agent fixed something" from "the agent burned an
    /// iteration and changed nothing". Content (not mtime) so a rewrite of
    /// identical bytes counts as no progress.
    fn fingerprint(&self) -> u64 {
        let root = &self.execution_loop.workspace.root;
        let mut hasher = DefaultHasher::new();

        let mut files: Vec<PathBuf> = self.execution_loop.workspace.manifest_files.clone();
        for dir in &self.execution_loop.workspace.source_dirs {
            collect_files(dir, &mut files);
        }
        files.sort();
        files.dedup();

        for file in files {
            let Ok(rel) = file.strip_prefix(root) else {
                continue;
            };
            rel.hash(&mut hasher);
            match std::fs::File::open(&file) {
                Ok(mut f) => {
                    let mut buf = [0u8; 8192];
                    loop {
                        match f.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => buf[..n].hash(&mut hasher),
                        }
                    }
                }
                // An unreadable file is itself a change signal; hash the fact.
                Err(e) => e.kind().hash(&mut hasher),
            }
        }
        hasher.finish()
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
        if self.execution_loop.workspace.project_type != crate::workspace::ProjectType::Unknown {
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

    summary.push_str(&format!(
        "\nValidation Summary: {}/{} passed\n",
        passed, total
    ));
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

/// Recursively collect regular files under `dir` into `out`.
///
/// Skips the usual noise directories and symlinks (a symlinked tree could pull
/// in an unbounded amount of data, or loop).
fn collect_files(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    // Shared with the discovery scan and the file picker. `.vibectl` is added
    // separately: a spec being written must not count as source progress.
    const SKIP: &[&str] = crate::langs::SKIP_DIRS;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if SKIP.contains(&name.as_ref()) || name == ".vibectl" {
            continue;
        }
        // `symlink_metadata` does not follow the link.
        match entry.file_type() {
            Ok(ft) if ft.is_symlink() => continue,
            Ok(ft) if ft.is_dir() => collect_files(&path, out),
            Ok(ft) if ft.is_file() => out.push(path),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tempfile::TempDir;

    /// A Python project whose validation plan is non-empty and *deterministically
    /// fails*: `pip install` on an empty requirements.txt exits 0, and `pytest`
    /// exits non-zero (either "no tests ran" or "not installed" — both failures).
    fn failing_project() -> (TempDir, WorkspaceContext) {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("requirements.txt"), "").unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.py"), "x = 1\n").unwrap();
        let mut ws = WorkspaceContext::new(dir.path()).unwrap();
        ws.project_type = crate::workspace::ProjectType::Python;
        (dir, ws)
    }

    fn channel() -> mpsc::Sender<AgentEvent> {
        let (tx, _rx) = mpsc::channel(256);
        tx
    }

    #[tokio::test]
    async fn a_project_with_no_validation_steps_is_reported_not_retried() {
        let dir = TempDir::new().unwrap();
        let ws = WorkspaceContext::new(dir.path()).unwrap();
        // An empty directory detects as Unknown, which has no plan at all.
        assert!(ws.validation_plan().is_empty());

        let mut wf = ValidationWorkflow::new(ws);
        let tx = channel();
        let report = wf.run(&tx).await.unwrap();
        assert_eq!(wf.history.iteration_count(), 0, "nothing to iterate");
        drop(dir);
        let _ = report;
    }

    #[tokio::test]
    async fn without_a_fixer_a_failure_stops_after_one_pass() {
        let (_dir, ws) = failing_project();
        let mut wf = ValidationWorkflow::new(ws);
        let tx = channel();

        let report = wf.run(&tx).await.unwrap();

        assert_eq!(
            wf.history.iteration_count(),
            1,
            "no fixer means retrying would be pointless"
        );
        let issue = report.blocking_issue.expect("must explain the failure");
        assert!(
            issue.contains("no auto-fix channel"),
            "unhelpful reason: {issue}"
        );
    }

    #[tokio::test]
    async fn a_fixer_that_changes_nothing_stops_the_loop() {
        let (_dir, ws) = failing_project();
        let mut wf = ValidationWorkflow::new(ws);
        let tx = channel();
        let calls = AtomicU32::new(0);

        let mut fixer: Box<Fixer<'_>> = Box::new(|_iteration, _analysis| {
            calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) }) // does nothing
        });

        let report = wf.run_with_fixer(&tx, Some(&mut *fixer)).await.unwrap();

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "should ask for a fix, then give up"
        );
        // The stall is detected from the fingerprint right after the fix, so we
        // never pay for a second validation pass we already know will fail.
        assert_eq!(
            wf.history.iteration_count(),
            1,
            "a no-op fix must not trigger another pass"
        );
        let issue = report.blocking_issue.expect("must explain the stall");
        assert!(issue.contains("did not change"), "got: {issue}");
    }

    #[tokio::test]
    async fn a_fixer_that_edits_a_file_triggers_another_pass() {
        let (dir, ws) = failing_project();
        let root = dir.path().to_path_buf();
        let mut wf = ValidationWorkflow::new(ws);
        let tx = channel();
        let calls = AtomicU32::new(0);

        let mut fixer: Box<Fixer<'_>> = Box::new(|_iteration, _analysis| {
            let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
            let root = root.clone();
            Box::pin(async move {
                // Edit a *source* file so the fingerprint moves.
                let target = root.join("src/main.py");
                let current = std::fs::read_to_string(&target).unwrap();
                std::fs::write(&target, format!("{current}# edit {n}\n")).unwrap();
                Ok(())
            })
        });

        let _ = wf.run_with_fixer(&tx, Some(&mut *fixer)).await.unwrap();

        // One productive edit, then a second pass that fails identically and a
        // fixer that edits again — bounded by the no-progress rule once the
        // test stops changing anything. The point is that iteration > 1 here,
        // which is exactly what the old unconditional `break` made impossible.
        assert!(
            wf.history.iteration_count() > 1,
            "expected a retry after a real edit, got {}",
            wf.history.iteration_count()
        );
    }

    #[tokio::test]
    async fn a_fixer_error_is_propagated_not_swallowed() {
        let (_dir, ws) = failing_project();
        let mut wf = ValidationWorkflow::new(ws);
        let tx = channel();

        let mut fixer: Box<Fixer<'_>> =
            Box::new(|_iteration, _analysis| Box::pin(async { Err(anyhow::anyhow!("boom")) }));

        let err = wf
            .run_with_fixer(&tx, Some(&mut *fixer))
            .await
            .expect_err("fixer error must not be reported as a validation verdict");
        assert!(err.to_string().contains("boom"), "got: {err}");
    }

    #[test]
    fn fingerprint_tracks_source_content_not_timestamps() {
        let (dir, ws) = failing_project();
        let wf = ValidationWorkflow::new(ws);

        let before = wf.fingerprint();
        assert_eq!(before, wf.fingerprint(), "stable when nothing changes");

        let target = dir.path().join("src/main.py");
        let original = std::fs::read_to_string(&target).unwrap();

        // Rewriting identical bytes is *not* progress.
        std::fs::write(&target, &original).unwrap();
        assert_eq!(before, wf.fingerprint(), "identical rewrite is no change");

        std::fs::write(&target, format!("{original}# real change\n")).unwrap();
        assert_ne!(before, wf.fingerprint(), "content change must register");
    }

    #[test]
    fn fingerprint_ignores_noise_directories() {
        let (dir, ws) = failing_project();
        let wf = ValidationWorkflow::new(ws);
        let before = wf.fingerprint();

        let noise = dir.path().join("node_modules");
        std::fs::create_dir_all(&noise).unwrap();
        std::fs::write(noise.join("index.js"), "garbage".repeat(5000)).unwrap();
        std::fs::create_dir_all(dir.path().join(".vibectl")).unwrap();
        std::fs::write(dir.path().join(".vibectl/cache.json"), "{}").unwrap();

        assert_eq!(
            before,
            wf.fingerprint(),
            "build output and caches must not look like source edits"
        );
    }

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

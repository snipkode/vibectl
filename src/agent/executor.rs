#![allow(dead_code)]

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::agent::AgentEvent;
use crate::workspace::{ValidationStep, WorkspaceContext};

/// Maximum number of error recovery iterations
pub const MAX_ITERATIONS: u32 = 5;

/// Result of a single validation step
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub step_name: String,
    pub command: String,
    pub success: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_secs: f64,
}

/// Autonomous execution strategy that runs validation, analyzes errors, and retries
#[derive(Debug, Clone)]
pub struct ExecutionLoop {
    pub max_iterations: u32,
    pub workspace: WorkspaceContext,
}

impl ExecutionLoop {
    pub fn new(workspace: WorkspaceContext) -> Self {
        Self {
            max_iterations: MAX_ITERATIONS,
            workspace,
        }
    }

    /// Execute validation steps and return results
    pub async fn run_validation_steps(
        &self,
        steps: &[ValidationStep],
        tx: &mpsc::Sender<AgentEvent>,
    ) -> Result<Vec<ValidationResult>> {
        let mut results = Vec::new();

        for step in steps {
            let _ = tx
                .send(AgentEvent::Text(format!(
                    "\n=== {} ===\n",
                    step.name.to_uppercase()
                )))
                .await;

            let result = self.run_step(step).await?;

            // Send detailed output
            if result.success {
                let _ = tx
                    .send(AgentEvent::Text(format!(
                        "✓ {} completed in {:.2}s\n",
                        step.name, result.duration_secs
                    )))
                    .await;
            } else {
                let _ = tx
                    .send(AgentEvent::Text(format!(
                        "✗ {} failed (exit code: {})\n",
                        step.name, result.exit_code
                    )))
                    .await;

                // Show error details
                if !result.stderr.is_empty() {
                    let _ = tx
                        .send(AgentEvent::Text(format!(
                            "\nError output:\n{}\n",
                            Self::truncate_output(&result.stderr, 2000)
                        )))
                        .await;
                }
            }

            results.push(result.clone());

            // Stop on first failure if step is required
            if !result.success && step.required {
                let _ = tx
                    .send(AgentEvent::Text(format!(
                        "\nValidation stopped: required step '{}' failed.\n",
                        step.name
                    )))
                    .await;
                break;
            }
        }

        Ok(results)
    }

    /// Run a single validation step
    async fn run_step(&self, step: &ValidationStep) -> Result<ValidationResult> {
        use std::process::Stdio;
        use std::time::Instant;
        use tokio::process::Command;
        use tokio::time::{timeout, Duration};

        let start = Instant::now();
        let deadline = Duration::from_secs(step.timeout_secs);

        let (shell, shell_arg) = if cfg!(target_os = "windows") {
            ("powershell", "-Command")
        } else {
            ("sh", "-c")
        };

        let child = Command::new(shell)
            .arg(shell_arg)
            .arg(&step.command)
            .current_dir(&self.workspace.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn command")?;

        let output = match timeout(deadline, child.wait_with_output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                return Ok(ValidationResult {
                    step_name: step.name.clone(),
                    command: step.command.clone(),
                    success: false,
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: format!("Failed to execute: {}", e),
                    duration_secs: start.elapsed().as_secs_f64(),
                });
            }
            Err(_) => {
                return Ok(ValidationResult {
                    step_name: step.name.clone(),
                    command: step.command.clone(),
                    success: false,
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: format!("Command timed out after {}s", step.timeout_secs),
                    duration_secs: start.elapsed().as_secs_f64(),
                });
            }
        };

        let duration = start.elapsed();
        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

        Ok(ValidationResult {
            step_name: step.name.clone(),
            command: step.command.clone(),
            success: output.status.success(),
            exit_code,
            stdout,
            stderr,
            duration_secs: duration.as_secs_f64(),
        })
    }

    /// Truncate output for display
    fn truncate_output(text: &str, max_chars: usize) -> String {
        if text.len() <= max_chars {
            text.to_string()
        } else {
            format!("{}...\n[truncated]", &text[..max_chars])
        }
    }

    /// Analyze validation results and extract actionable error information
    pub fn analyze_failures(&self, results: &[ValidationResult]) -> AnalysisReport {
        let failed_steps: Vec<_> = results.iter().filter(|r| !r.success).collect();

        if failed_steps.is_empty() {
            return AnalysisReport {
                has_failures: false,
                primary_error: None,
                affected_files: vec![],
                suggestions: vec![],
                error_summary: String::new(),
            };
        }

        // Focus on the first failure (most likely root cause)
        let primary = failed_steps[0];
        let error_text = format!("{}\n{}", primary.stdout, primary.stderr);

        let affected_files = Self::extract_file_references(&error_text);
        let suggestions = Self::generate_suggestions(primary, &error_text);
        let error_summary = Self::build_error_summary(primary, &error_text);

        AnalysisReport {
            has_failures: true,
            primary_error: Some(primary.clone()),
            affected_files,
            suggestions,
            error_summary,
        }
    }

    /// Extract file paths mentioned in error messages
    fn extract_file_references(error_text: &str) -> Vec<String> {
        let mut files = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // Common patterns: "file.ts:10:5", "src/app.js", "in file.py"
        for line in error_text.lines() {
            // Pattern: path/to/file.ext:line:col or path/to/file.ext
            if let Some(file) = Self::extract_file_from_line(line) {
                if seen.insert(file.clone()) {
                    files.push(file);
                }
            }
        }

        files
    }

    /// Extract file path from a single line
    fn extract_file_from_line(line: &str) -> Option<String> {
        // Look for patterns like: src/main.rs:10:5, ./app.js, index.ts(45,10)
        let patterns = [
            regex::Regex::new(r"([a-zA-Z0-9_./\\-]+\.[a-zA-Z]{1,4}):\d+:\d+").ok()?,
            regex::Regex::new(r"([a-zA-Z0-9_./\\-]+\.[a-zA-Z]{1,4})\(\d+,\d+\)").ok()?,
            regex::Regex::new(r"\b([a-zA-Z0-9_./\\-]+\.[a-zA-Z]{1,4})\b").ok()?,
        ];

        for pattern in &patterns {
            if let Some(captures) = pattern.captures(line) {
                if let Some(matched) = captures.get(1) {
                    let file = matched.as_str().to_string();
                    // Filter out common false positives
                    if !file.starts_with("http") && !file.contains("node_modules") {
                        return Some(file);
                    }
                }
            }
        }

        None
    }

    /// Generate actionable suggestions based on error patterns
    fn generate_suggestions(result: &ValidationResult, error_text: &str) -> Vec<String> {
        let mut suggestions = Vec::new();
        let lower = error_text.to_lowercase();

        // Dependency issues
        if lower.contains("cannot find module")
            || lower.contains("no such file or directory")
            || lower.contains("unresolved import")
        {
            if result.command.contains("npm") || result.command.contains("node") {
                suggestions.push("Run 'npm install' to install missing dependencies".to_string());
            } else if result.command.contains("cargo") {
                suggestions
                    .push("Run 'cargo build' to fetch and compile dependencies".to_string());
            } else if result.command.contains("go") {
                suggestions.push("Run 'go mod download' to fetch dependencies".to_string());
            } else if result.command.contains("python") || result.command.contains("pytest") {
                suggestions.push(
                    "Run 'pip install -r requirements.txt' to install dependencies".to_string(),
                );
            }
        }

        // Syntax errors
        if lower.contains("syntax error")
            || lower.contains("unexpected token")
            || lower.contains("expected")
        {
            suggestions.push(
                "Fix syntax errors in the affected files (check error messages for line numbers)"
                    .to_string(),
            );
        }

        // Type errors
        if lower.contains("type error")
            || lower.contains("mismatched types")
            || lower.contains("cannot be assigned")
        {
            suggestions.push("Fix type mismatches in the code".to_string());
        }

        // Test failures
        if lower.contains("test failed") || lower.contains("assertion") {
            suggestions.push("Review failing test output and fix the implementation".to_string());
        }

        // Compilation errors
        if lower.contains("compilation error") || lower.contains("failed to compile") {
            suggestions.push("Fix compilation errors in the source code".to_string());
        }

        // Environment issues
        if lower.contains("permission denied") {
            suggestions.push("Check file permissions or run with appropriate privileges".to_string());
        }

        // Generic fallback
        if suggestions.is_empty() {
            suggestions.push(
                "Review the error output above and fix the identified issues".to_string(),
            );
        }

        suggestions
    }

    /// Build a concise error summary for the LLM
    fn build_error_summary(result: &ValidationResult, _error_text: &str) -> String {
        let mut summary = format!("Command '{}' failed with exit code {}.\n\n", result.command, result.exit_code);

        // Extract the most relevant error lines (last 30 lines of stderr + key error messages)
        let stderr_lines: Vec<&str> = result.stderr.lines().collect();
        let relevant_lines = if stderr_lines.len() > 30 {
            stderr_lines[stderr_lines.len() - 30..].join("\n")
        } else {
            result.stderr.clone()
        };

        summary.push_str("Key error output:\n");
        summary.push_str(&Self::truncate_output(&relevant_lines, 1500));

        summary
    }
}

/// Analysis report of validation failures
#[derive(Debug, Clone)]
pub struct AnalysisReport {
    pub has_failures: bool,
    pub primary_error: Option<ValidationResult>,
    pub affected_files: Vec<String>,
    pub suggestions: Vec<String>,
    pub error_summary: String,
}

impl AnalysisReport {
    /// Format the analysis report for the LLM
    pub fn format_for_llm(&self) -> String {
        if !self.has_failures {
            return "All validation steps passed successfully.".to_string();
        }

        let mut output = String::new();
        output.push_str("=== VALIDATION FAILURE ANALYSIS ===\n\n");

        if let Some(err) = &self.primary_error {
            output.push_str(&format!("Failed Step: {}\n", err.step_name));
            output.push_str(&format!("Command: {}\n", err.command));
            output.push_str(&format!("Exit Code: {}\n\n", err.exit_code));
        }

        if !self.affected_files.is_empty() {
            output.push_str("Affected Files:\n");
            for file in &self.affected_files {
                output.push_str(&format!("  - {}\n", file));
            }
            output.push('\n');
        }

        output.push_str(&self.error_summary);
        output.push_str("\n\n");

        if !self.suggestions.is_empty() {
            output.push_str("Suggested Actions:\n");
            for (i, suggestion) in self.suggestions.iter().enumerate() {
                output.push_str(&format!("{}. {}\n", i + 1, suggestion));
            }
        }

        output.push_str("\n");
        output.push_str("IMPORTANT: You must analyze this error and take corrective action.\n");
        output.push_str("Do NOT retry the same command without making changes to fix the issue.\n");

        output
    }
}

/// Track execution history across iterations
#[derive(Debug, Clone)]
pub struct ExecutionHistory {
    pub iterations: Vec<IterationRecord>,
}

impl ExecutionHistory {
    pub fn new() -> Self {
        Self {
            iterations: Vec::new(),
        }
    }

    pub fn add_iteration(&mut self, record: IterationRecord) {
        self.iterations.push(record);
    }

    pub fn iteration_count(&self) -> usize {
        self.iterations.len()
    }

    pub fn has_reached_limit(&self, max: u32) -> bool {
        self.iterations.len() >= max as usize
    }

    pub fn last_error(&self) -> Option<String> {
        self.iterations
            .iter()
            .rev()
            .find(|i| !i.success)
            .map(|i| i.error_summary.clone())
    }
}

/// Record of a single execution iteration
#[derive(Debug, Clone)]
pub struct IterationRecord {
    pub iteration: u32,
    pub success: bool,
    pub validation_results: Vec<ValidationResult>,
    pub error_summary: String,
    pub actions_taken: Vec<String>,
}

impl IterationRecord {
    pub fn new(iteration: u32) -> Self {
        Self {
            iteration,
            success: false,
            validation_results: Vec::new(),
            error_summary: String::new(),
            actions_taken: Vec::new(),
        }
    }
}

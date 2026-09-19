#![allow(dead_code)]

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Supported project types detected from workspace files
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProjectType {
    NodeJs,
    Rust,
    Go,
    Python,
    Unknown,
}

impl ProjectType {
    /// Get the standard build/test commands for this project type
    pub fn validation_commands(&self) -> Vec<&str> {
        match self {
            ProjectType::NodeJs => vec!["npm install", "npm test", "npm run build"],
            ProjectType::Rust => vec!["cargo check", "cargo test", "cargo clippy"],
            ProjectType::Go => vec!["go test ./...", "go vet ./...", "go build ./..."],
            ProjectType::Python => vec!["pip install -r requirements.txt", "pytest"],
            ProjectType::Unknown => vec![],
        }
    }

    /// Get typical project structure for new projects
    pub fn default_structure(&self) -> Vec<&str> {
        match self {
            ProjectType::NodeJs => vec!["src/", "tests/", "package.json", "README.md"],
            ProjectType::Rust => vec!["src/", "tests/", "Cargo.toml", "README.md"],
            ProjectType::Go => vec!["cmd/", "pkg/", "internal/", "go.mod", "README.md"],
            ProjectType::Python => vec![
                "src/",
                "tests/",
                "requirements.txt",
                "setup.py",
                "README.md",
            ],
            ProjectType::Unknown => vec!["README.md"],
        }
    }

    /// Get package manager command
    pub fn package_manager(&self) -> &str {
        match self {
            ProjectType::NodeJs => "npm",
            ProjectType::Rust => "cargo",
            ProjectType::Go => "go",
            ProjectType::Python => "pip",
            ProjectType::Unknown => "",
        }
    }

    /// Get test runner command
    pub fn test_command(&self) -> &str {
        match self {
            ProjectType::NodeJs => "npm test",
            ProjectType::Rust => "cargo test",
            ProjectType::Go => "go test ./...",
            ProjectType::Python => "pytest",
            ProjectType::Unknown => "",
        }
    }

    /// Get build command if applicable
    pub fn build_command(&self) -> Option<&str> {
        match self {
            ProjectType::NodeJs => Some("npm run build"),
            ProjectType::Rust => Some("cargo build"),
            ProjectType::Go => Some("go build ./..."),
            ProjectType::Python => None, // Python typically doesn't have a build step
            ProjectType::Unknown => None,
        }
    }

    /// Get lint command if applicable
    pub fn lint_command(&self) -> Option<&str> {
        match self {
            ProjectType::NodeJs => Some("npm run lint"),
            ProjectType::Rust => Some("cargo clippy"),
            ProjectType::Go => Some("go vet ./..."),
            ProjectType::Python => Some("flake8"),
            ProjectType::Unknown => None,
        }
    }

    /// Get formatter command if applicable
    pub fn format_command(&self) -> Option<&str> {
        match self {
            ProjectType::NodeJs => Some("npm run format"),
            ProjectType::Rust => Some("cargo fmt"),
            ProjectType::Go => Some("go fmt ./..."),
            ProjectType::Python => Some("black ."),
            ProjectType::Unknown => None,
        }
    }
}

/// Workspace context with project detection and safety validation
#[derive(Debug, Clone)]
pub struct WorkspaceContext {
    /// Root directory of the workspace
    pub root: PathBuf,
    /// Detected project type
    pub project_type: ProjectType,
    /// Whether this is an existing project or a new one
    pub is_existing: bool,
    /// Detected manifest files (package.json, Cargo.toml, etc.)
    pub manifest_files: Vec<PathBuf>,
    /// Detected source directories
    pub source_dirs: Vec<PathBuf>,
}

impl WorkspaceContext {
    /// Create a new workspace context by inspecting the given directory
    pub fn new(root: &Path) -> Result<Self> {
        let root = root
            .canonicalize()
            .context("failed to canonicalize workspace root")?;

        let project_type = Self::detect_project_type(&root)?;
        let is_existing = Self::is_existing_project(&root, &project_type);
        let manifest_files = Self::find_manifest_files(&root);
        let source_dirs = Self::find_source_dirs(&root);

        Ok(Self {
            root,
            project_type,
            is_existing,
            manifest_files,
            source_dirs,
        })
    }

    /// Detect project type from manifest files
    fn detect_project_type(root: &Path) -> Result<ProjectType> {
        let mut scores: HashMap<ProjectType, u32> = HashMap::new();

        // Check for manifest files
        if root.join("package.json").exists() {
            *scores.entry(ProjectType::NodeJs).or_insert(0) += 10;
        }
        if root.join("package-lock.json").exists() {
            *scores.entry(ProjectType::NodeJs).or_insert(0) += 5;
        }
        if root.join("yarn.lock").exists() {
            *scores.entry(ProjectType::NodeJs).or_insert(0) += 5;
        }

        if root.join("Cargo.toml").exists() {
            *scores.entry(ProjectType::Rust).or_insert(0) += 10;
        }
        if root.join("Cargo.lock").exists() {
            *scores.entry(ProjectType::Rust).or_insert(0) += 5;
        }

        if root.join("go.mod").exists() {
            *scores.entry(ProjectType::Go).or_insert(0) += 10;
        }
        if root.join("go.sum").exists() {
            *scores.entry(ProjectType::Go).or_insert(0) += 5;
        }

        if root.join("requirements.txt").exists() {
            *scores.entry(ProjectType::Python).or_insert(0) += 8;
        }
        if root.join("setup.py").exists() {
            *scores.entry(ProjectType::Python).or_insert(0) += 7;
        }
        if root.join("pyproject.toml").exists() {
            *scores.entry(ProjectType::Python).or_insert(0) += 9;
        }
        if root.join("Pipfile").exists() {
            *scores.entry(ProjectType::Python).or_insert(0) += 8;
        }

        // Return the project type with highest score
        let detected = scores
            .into_iter()
            .max_by_key(|(_, score)| *score)
            .map(|(ptype, _)| ptype)
            .unwrap_or(ProjectType::Unknown);

        Ok(detected)
    }

    /// Check if this is an existing project (has source files)
    fn is_existing_project(root: &Path, project_type: &ProjectType) -> bool {
        match project_type {
            ProjectType::NodeJs => root.join("package.json").exists(),
            ProjectType::Rust => root.join("Cargo.toml").exists(),
            ProjectType::Go => root.join("go.mod").exists(),
            ProjectType::Python => {
                root.join("requirements.txt").exists()
                    || root.join("setup.py").exists()
                    || root.join("pyproject.toml").exists()
            }
            ProjectType::Unknown => {
                // Check if there are any source files
                root.join("src").exists() || root.join("lib").exists()
            }
        }
    }

    /// Find all manifest files in the workspace
    fn find_manifest_files(root: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let candidates = [
            "package.json",
            "Cargo.toml",
            "go.mod",
            "requirements.txt",
            "setup.py",
            "pyproject.toml",
        ];

        for candidate in &candidates {
            let path = root.join(candidate);
            if path.exists() {
                files.push(path);
            }
        }

        files
    }

    /// Find common source directories
    fn find_source_dirs(root: &Path) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        let candidates = ["src", "lib", "pkg", "cmd", "internal", "app"];

        for candidate in &candidates {
            let path = root.join(candidate);
            if path.is_dir() {
                dirs.push(path);
            }
        }

        dirs
    }

    /// Validate that a path is within the workspace (safety check)
    pub fn validate_path(&self, path: &Path) -> Result<PathBuf> {
        let target = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };

        let canonical = target
            .canonicalize()
            .or_else(|_| {
                // If file doesn't exist yet, validate parent directory
                if let Some(parent) = target.parent() {
                    if parent.exists() {
                        parent.canonicalize().map(|p| p.join(target.file_name().unwrap()))
                    } else {
                        Ok(target.clone())
                    }
                } else {
                    Ok(target.clone())
                }
            })
            .context("failed to resolve path")?;

        // Security check: ensure path is within workspace
        if !canonical.starts_with(&self.root) {
            bail!(
                "path '{}' is outside workspace root '{}'",
                canonical.display(),
                self.root.display()
            );
        }

        Ok(canonical)
    }

    /// Check if a path is safe to delete (prevent accidental deletion of important files)
    pub fn is_safe_to_delete(&self, path: &Path) -> bool {
        let dangerous_patterns = [
            ".git",
            "node_modules",
            "target",
            ".env",
            "package.json",
            "Cargo.toml",
            "go.mod",
        ];

        let path_str = path.to_string_lossy();
        !dangerous_patterns.iter().any(|p| path_str.contains(p))
    }

    /// Get a summary of the workspace for display/logging
    pub fn summary(&self) -> String {
        let mut lines = vec![
            format!("Workspace: {}", self.root.display()),
            format!("Project Type: {:?}", self.project_type),
            format!("Status: {}", if self.is_existing { "Existing" } else { "New" }),
        ];

        if !self.manifest_files.is_empty() {
            lines.push("Manifest Files:".to_string());
            for file in &self.manifest_files {
                if let Some(name) = file.file_name() {
                    lines.push(format!("  - {}", name.to_string_lossy()));
                }
            }
        }

        if !self.source_dirs.is_empty() {
            lines.push("Source Directories:".to_string());
            for dir in &self.source_dirs {
                if let Some(name) = dir.file_name() {
                    lines.push(format!("  - {}/", name.to_string_lossy()));
                }
            }
        }

        lines.join("\n")
    }

    /// Get installation command for dependencies
    pub fn install_command(&self) -> Option<String> {
        match self.project_type {
            ProjectType::NodeJs => Some("npm install".to_string()),
            ProjectType::Rust => None, // cargo build handles dependencies
            ProjectType::Go => Some("go mod download".to_string()),
            ProjectType::Python => {
                if self.root.join("requirements.txt").exists() {
                    Some("pip install -r requirements.txt".to_string())
                } else {
                    None
                }
            }
            ProjectType::Unknown => None,
        }
    }

    /// Generate a validation plan (commands to run for checking the project)
    pub fn validation_plan(&self) -> Vec<ValidationStep> {
        let mut steps = Vec::new();

        // Step 1: Install dependencies if needed
        if let Some(install_cmd) = self.install_command() {
            steps.push(ValidationStep {
                name: "Install Dependencies".to_string(),
                command: install_cmd,
                required: true,
                timeout_secs: 300,
            });
        }

        // Step 2: Lint/format check
        if let Some(lint_cmd) = self.project_type.lint_command() {
            steps.push(ValidationStep {
                name: "Lint Check".to_string(),
                command: lint_cmd.to_string(),
                required: false,
                timeout_secs: 60,
            });
        }

        // Step 3: Build if applicable
        if let Some(build_cmd) = self.project_type.build_command() {
            steps.push(ValidationStep {
                name: "Build".to_string(),
                command: build_cmd.to_string(),
                required: true,
                timeout_secs: 300,
            });
        }

        // Step 4: Run tests
        if !self.project_type.test_command().is_empty() {
            steps.push(ValidationStep {
                name: "Run Tests".to_string(),
                command: self.project_type.test_command().to_string(),
                required: true,
                timeout_secs: 300,
            });
        }

        steps
    }
}

/// A single validation step in the validation plan
#[derive(Debug, Clone)]
pub struct ValidationStep {
    pub name: String,
    pub command: String,
    pub required: bool,
    pub timeout_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_project_type_validation_commands() {
        let nodejs = ProjectType::NodeJs;
        assert_eq!(
            nodejs.validation_commands(),
            vec!["npm install", "npm test", "npm run build"]
        );

        let rust = ProjectType::Rust;
        assert_eq!(
            rust.validation_commands(),
            vec!["cargo check", "cargo test", "cargo clippy"]
        );
    }

    #[test]
    fn test_project_type_package_manager() {
        assert_eq!(ProjectType::NodeJs.package_manager(), "npm");
        assert_eq!(ProjectType::Rust.package_manager(), "cargo");
        assert_eq!(ProjectType::Go.package_manager(), "go");
        assert_eq!(ProjectType::Python.package_manager(), "pip");
    }
}

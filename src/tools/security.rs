//! `security` — explicit security validation layer for tool execution.
//!
//! All tool calls pass through:
//!
//!   `ToolRequest → SchemaValidation → SecurityValidation → Executor → ToolResult`
//!
//! This module owns the `SecurityValidator` and `SecurityPolicy` that define
//! what is allowed, what is blocked, and what requires escalation.

use std::path::{Path, PathBuf};

// ─── Policy ───────────────────────────────────────────────────────────────────

/// Configurable limits and block-lists for the security layer.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SecurityPolicy {
    /// Maximum size in bytes for tool output (default: 1 MB).
    pub max_output_bytes: usize,
    /// Maximum size in bytes that a single file may have before read is refused (default: 5 MB).
    pub max_file_bytes: usize,
    /// Maximum number of agent loop iterations before hard-stop (default: 30).
    pub max_iterations: u32,
    /// Glob patterns for sensitive files that should never be read into the
    /// prompt without explicit user permission.
    pub blocked_paths: Vec<String>,
}

impl Default for SecurityPolicy {
    fn default() -> Self {
        Self {
            max_output_bytes: 1_048_576,     // 1 MB
            max_file_bytes: 5_242_880,       // 5 MB
            max_iterations: 30,
            blocked_paths: DEFAULT_BLOCKED_PATHS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

/// Default set of sensitive file glob patterns.
#[allow(dead_code)]
const DEFAULT_BLOCKED_PATHS: &[&str] = &[
    ".env",
    ".env.*",
    "*.key",
    "*.pem",
    "*.p12",
    "*.pfx",
    "*.pkcs12",
    "id_rsa",
    "id_rsa.pub",
    "id_ed25519",
    "id_ed25519.pub",
    "id_ecdsa",
    "*.secret",
    "credentials",
    "credentials.json",
    "secrets.yaml",
    "secrets.yml",
    "secrets.json",
    "*.password",
    "*.passwd",
    "*.private",
    "service-account*.json",
    "*.pfx",
    ".netrc",
    "auth.json",
    "token.json",
];

// ─── Violations ───────────────────────────────────────────────────────────────

/// A specific security rule that was violated.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum SecurityViolation {
    /// The requested path contains `..` components or similar traversal.
    PathTraversal { path: String },
    /// The resolved path is outside the sandbox root.
    OutsideSandbox { path: String, root: String },
    /// A command matched a known-dangerous pattern.
    BlockedCommand { command: String, reason: String },
    /// The file matches a sensitive-data pattern (credential, key, etc.).
    SensitiveFile { path: String },
    /// Tool output exceeded the configured size limit.
    OutputTooLarge { size: usize, limit: usize },
    /// File is too large to be safely read.
    FileTooLarge { size: usize, limit: usize },
}

impl std::fmt::Display for SecurityViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PathTraversal { path } =>
                write!(f, "SECURITY: path traversal detected in `{path}` — access refused"),
            Self::OutsideSandbox { path, root } =>
                write!(f,
                    "SECURITY: `{path}` is outside the sandbox root `{root}` — \
                     set allow_any_path: true to override"
                ),
            Self::BlockedCommand { command, reason } =>
                write!(f, "SECURITY: command `{command}` blocked — {reason}"),
            Self::SensitiveFile { path } =>
                write!(f,
                    "SECURITY: `{path}` is a sensitive file (credential/key). \
                     Reading it would expose secrets in the LLM context. \
                     Use allow_any_path: true and explicit user approval to override."
                ),
            Self::OutputTooLarge { size, limit } =>
                write!(f,
                    "SECURITY: tool output is {size} bytes, exceeds limit of {limit} bytes — \
                     truncation required"
                ),
            Self::FileTooLarge { size, limit } =>
                write!(f,
                    "SECURITY: file is {size} bytes, exceeds the {limit}-byte read limit — \
                     use read_file with offset/limit to read it in chunks"
                ),
        }
    }
}

// ─── Validator ────────────────────────────────────────────────────────────────

/// Central security validator — all tool calls pass through this.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SecurityValidator {
    /// Absolute path that all file accesses must stay within.
    pub sandbox_root: PathBuf,
    /// When true, the sandbox path check is bypassed (still logs).
    pub allow_any_path: bool,
    /// Active policy for limits and block-lists.
    pub policy: SecurityPolicy,
}

#[allow(dead_code)]
impl SecurityValidator {
    /// Create a validator confined to `sandbox_root` with default policy.
    #[allow(dead_code)]
    pub fn new(sandbox_root: PathBuf) -> Self {
        Self {
            sandbox_root,
            allow_any_path: false,
            policy: SecurityPolicy::default(),
        }
    }

    /// Create a validator with a custom policy.
    pub fn with_policy(sandbox_root: PathBuf, policy: SecurityPolicy) -> Self {
        Self {
            sandbox_root,
            allow_any_path: false,
            policy,
        }
    }

    // ─── Path validation ──────────────────────────────────────────────────────

    /// Validate a path string and return the resolved absolute `PathBuf`.
    ///
    /// Checks (in order):
    /// 1. Lexical traversal (`..\..`)
    /// 2. Sensitive file patterns
    /// 3. Sandbox containment (unless `allow_any_path`)
    pub fn validate_path(&self, path: &str) -> Result<PathBuf, SecurityViolation> {
        // 1. Detect obvious traversal in the raw string before resolving.
        if path.contains("..") {
            let resolved = crate::tools::normalize(Path::new(path));
            // If after normalisation the path still goes outside cwd, block it.
            if resolved.components().any(|c| {
                c == std::path::Component::ParentDir
            }) {
                return Err(SecurityViolation::PathTraversal { path: path.to_string() });
            }
        }

        // Resolve relative to sandbox root.
        let abs = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.sandbox_root.join(path)
        };
        let normalized = crate::tools::normalize(&abs);

        // 2. Sensitive file check (applies even with allow_any_path).
        if self.is_sensitive_file(path) {
            return Err(SecurityViolation::SensitiveFile { path: path.to_string() });
        }

        // 3. Sandbox check.
        if !self.allow_any_path
            && !crate::tools::is_within(&self.sandbox_root, &normalized)
        {
            return Err(SecurityViolation::OutsideSandbox {
                path: normalized.to_string_lossy().to_string(),
                root: self.sandbox_root.to_string_lossy().to_string(),
            });
        }

        Ok(normalized)
    }

    // ─── Command validation ───────────────────────────────────────────────────

    /// Validate a shell command string against known-dangerous patterns.
    ///
    /// Returns `Ok(())` if the command is allowed, or a `SecurityViolation`
    /// describing why it was blocked.
    pub fn validate_command(&self, command: &str) -> Result<(), SecurityViolation> {
        let lower = command.trim().to_lowercase();

        for (pattern, reason) in BLOCKED_COMMAND_PATTERNS {
            if lower.contains(pattern) {
                return Err(SecurityViolation::BlockedCommand {
                    command: command.to_string(),
                    reason: reason.to_string(),
                });
            }
        }

        Ok(())
    }

    // ─── File size validation ─────────────────────────────────────────────────

    /// Check that a file is within the configured size limit before reading it.
    pub fn validate_file_size(&self, path: &Path) -> Result<(), SecurityViolation> {
        match std::fs::metadata(path) {
            Ok(meta) => {
                let size = meta.len() as usize;
                if size > self.policy.max_file_bytes {
                    return Err(SecurityViolation::FileTooLarge {
                        size,
                        limit: self.policy.max_file_bytes,
                    });
                }
                Ok(())
            }
            Err(_) => Ok(()), // File doesn't exist yet — not a size problem.
        }
    }

    // ─── Output size validation ───────────────────────────────────────────────

    /// Verify that a tool result won't flood the context.
    pub fn validate_output_size(&self, size: usize) -> Result<(), SecurityViolation> {
        if size > self.policy.max_output_bytes {
            Err(SecurityViolation::OutputTooLarge {
                size,
                limit: self.policy.max_output_bytes,
            })
        } else {
            Ok(())
        }
    }

    // ─── Sensitive file detection ─────────────────────────────────────────────

    /// Returns `true` when the path's filename matches any sensitive-file
    /// pattern in the current policy.
    pub fn is_sensitive_file(&self, path: &str) -> bool {
        let file_name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());

        for pattern_str in &self.policy.blocked_paths {
            if let Ok(pat) = glob::Pattern::new(pattern_str) {
                if pat.matches(&file_name) {
                    return true;
                }
            }
        }
        false
    }

    // ─── Audit log ────────────────────────────────────────────────────────────

    /// Append a one-line audit entry to `.vibectl/security.log` (best-effort).
    ///
    /// Failures are silently ignored — a missing log must not abort a tool call.
    pub fn audit_log(&self, tool: &str, action: &str, outcome: &str) {
        use std::io::Write;

        let log_path = self.sandbox_root.join(".vibectl").join("security.log");
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let line = format!("[{timestamp}] tool={tool} action={action} outcome={outcome}\n");

        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

// ─── Blocked command patterns ─────────────────────────────────────────────────

/// (pattern, human-readable reason) pairs.
/// Patterns are matched against the lowercased full command string.
#[allow(dead_code)]
const BLOCKED_COMMAND_PATTERNS: &[(&str, &str)] = &[
    // Recursive deletes of root or home — catastrophic, never legitimate.
    ("rm -rf /", "recursive delete of filesystem root"),
    ("rm -rf ~/", "recursive delete of home directory"),
    ("rm -rf ~", "recursive delete of home directory"),
    // Block-device writes — will destroy partitions.
    ("dd if=", "raw device I/O — potential disk corruption"),
    // Filesystem creation — will wipe partitions.
    ("mkfs", "filesystem creation — would wipe storage device"),
    // Fork bomb.
    (":(){ :|:& };:", "fork bomb — system resource exhaustion"),
    // Mass permission escalation.
    ("chmod -r 777 /", "recursive world-write on filesystem root"),
    ("chmod 777 /", "world-write on filesystem root"),
    // Overwrite kernel parameters.
    ("sysctl -w", "modifies kernel parameters"),
    // Shutdown/reboot.
    ("shutdown now", "shuts down the system"),
    ("reboot", "reboots the system"),
    // Direct device write.
    ("> /dev/sd", "direct write to block device"),
    ("> /dev/nvme", "direct write to NVMe device"),
];

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn validator(dir: &TempDir) -> SecurityValidator {
        SecurityValidator::new(dir.path().to_path_buf())
    }

    // ── Path validation ───────────────────────────────────────────────────────

    #[test]
    fn allows_normal_relative_path() {
        let dir = TempDir::new().unwrap();
        let v = validator(&dir);
        let result = v.validate_path("src/main.rs");
        assert!(result.is_ok(), "got: {:?}", result);
    }

    #[test]
    fn blocks_traversal_via_dotdot() {
        let dir = TempDir::new().unwrap();
        let v = validator(&dir);
        // Path that lexically escapes the sandbox.
        let result = v.validate_path("../../etc/passwd");
        assert!(
            matches!(
                result,
                Err(SecurityViolation::PathTraversal { .. })
                    | Err(SecurityViolation::OutsideSandbox { .. })
            ),
            "expected traversal/sandbox block, got: {:?}",
            result
        );
    }

    #[test]
    fn blocks_absolute_path_outside_sandbox() {
        let dir = TempDir::new().unwrap();
        let v = validator(&dir);
        let result = v.validate_path("/etc/passwd");
        assert!(
            matches!(result, Err(SecurityViolation::OutsideSandbox { .. })),
            "got: {:?}",
            result
        );
    }

    #[test]
    fn allows_absolute_path_with_allow_any_path() {
        let dir = TempDir::new().unwrap();
        let mut v = validator(&dir);
        v.allow_any_path = true;
        // /etc/passwd is sensitive by name? No — check the sensitive list.
        // "passwd" is not in DEFAULT_BLOCKED_PATHS, so allow_any_path lets it through.
        let result = v.validate_path("/etc/hostname"); // not sensitive
        assert!(result.is_ok(), "got: {:?}", result);
    }

    // ── Sensitive file detection ──────────────────────────────────────────────

    #[test]
    fn detects_env_file() {
        let dir = TempDir::new().unwrap();
        let v = validator(&dir);
        assert!(v.is_sensitive_file(".env"));
        assert!(v.is_sensitive_file(".env.production"));
        assert!(v.is_sensitive_file("config/.env"));
    }

    #[test]
    fn detects_key_files() {
        let dir = TempDir::new().unwrap();
        let v = validator(&dir);
        assert!(v.is_sensitive_file("id_rsa"));
        assert!(v.is_sensitive_file("server.key"));
        assert!(v.is_sensitive_file("client.pem"));
        assert!(v.is_sensitive_file("cert.p12"));
    }

    #[test]
    fn detects_credential_files() {
        let dir = TempDir::new().unwrap();
        let v = validator(&dir);
        assert!(v.is_sensitive_file("credentials.json"));
        assert!(v.is_sensitive_file("secrets.yaml"));
        assert!(v.is_sensitive_file("token.json"));
    }

    #[test]
    fn allows_normal_source_files() {
        let dir = TempDir::new().unwrap();
        let v = validator(&dir);
        assert!(!v.is_sensitive_file("main.rs"));
        assert!(!v.is_sensitive_file("config.toml"));
        assert!(!v.is_sensitive_file("README.md"));
        assert!(!v.is_sensitive_file("package.json"));
    }

    #[test]
    fn blocks_path_to_sensitive_file() {
        let dir = TempDir::new().unwrap();
        let v = validator(&dir);
        let result = v.validate_path(".env");
        assert!(
            matches!(result, Err(SecurityViolation::SensitiveFile { .. })),
            "got: {:?}",
            result
        );
    }

    // ── Command validation ────────────────────────────────────────────────────

    #[test]
    fn allows_safe_commands() {
        let dir = TempDir::new().unwrap();
        let v = validator(&dir);
        assert!(v.validate_command("cargo test").is_ok());
        assert!(v.validate_command("npm install").is_ok());
        assert!(v.validate_command("git status").is_ok());
        assert!(v.validate_command("ls -la").is_ok());
    }

    #[test]
    fn blocks_rm_rf_root() {
        let dir = TempDir::new().unwrap();
        let v = validator(&dir);
        let result = v.validate_command("rm -rf /");
        assert!(
            matches!(result, Err(SecurityViolation::BlockedCommand { .. })),
            "got: {:?}",
            result
        );
    }

    #[test]
    fn blocks_dd_device_write() {
        let dir = TempDir::new().unwrap();
        let v = validator(&dir);
        let result = v.validate_command("dd if=/dev/urandom of=/dev/sda bs=4M");
        assert!(
            matches!(result, Err(SecurityViolation::BlockedCommand { .. })),
            "got: {:?}",
            result
        );
    }

    #[test]
    fn blocks_fork_bomb() {
        let dir = TempDir::new().unwrap();
        let v = validator(&dir);
        let result = v.validate_command(":(){ :|:& };:");
        assert!(
            matches!(result, Err(SecurityViolation::BlockedCommand { .. })),
            "got: {:?}",
            result
        );
    }

    #[test]
    fn blocks_mkfs() {
        let dir = TempDir::new().unwrap();
        let v = validator(&dir);
        let result = v.validate_command("mkfs.ext4 /dev/sdb1");
        assert!(
            matches!(result, Err(SecurityViolation::BlockedCommand { .. })),
            "got: {:?}",
            result
        );
    }

    // ── File size validation ──────────────────────────────────────────────────

    #[test]
    fn allows_small_file() {
        let dir = TempDir::new().unwrap();
        let v = validator(&dir);
        let p = dir.path().join("small.txt");
        std::fs::write(&p, "hello world").unwrap();
        assert!(v.validate_file_size(&p).is_ok());
    }

    #[test]
    fn blocks_oversized_file() {
        let dir = TempDir::new().unwrap();
        let mut v = validator(&dir);
        v.policy.max_file_bytes = 10; // tiny limit for test
        let p = dir.path().join("big.txt");
        std::fs::write(&p, "this is more than 10 bytes of content").unwrap();
        let result = v.validate_file_size(&p);
        assert!(
            matches!(result, Err(SecurityViolation::FileTooLarge { .. })),
            "got: {:?}",
            result
        );
    }

    // ── Output size validation ────────────────────────────────────────────────

    #[test]
    fn allows_small_output() {
        let dir = TempDir::new().unwrap();
        let v = validator(&dir);
        assert!(v.validate_output_size(1024).is_ok());
    }

    #[test]
    fn blocks_large_output() {
        let dir = TempDir::new().unwrap();
        let mut v = validator(&dir);
        v.policy.max_output_bytes = 100;
        let result = v.validate_output_size(1000);
        assert!(
            matches!(result, Err(SecurityViolation::OutputTooLarge { .. })),
            "got: {:?}",
            result
        );
    }

    // ── Display / error messages ──────────────────────────────────────────────

    #[test]
    fn violation_display_messages_are_informative() {
        let v = SecurityViolation::PathTraversal { path: "../../etc/passwd".to_string() };
        let msg = v.to_string();
        assert!(msg.contains("path traversal"), "got: {msg}");

        let v = SecurityViolation::SensitiveFile { path: ".env".to_string() };
        let msg = v.to_string();
        assert!(msg.contains("sensitive"), "got: {msg}");
        assert!(msg.contains("secrets"), "got: {msg}");

        let v = SecurityViolation::BlockedCommand {
            command: "rm -rf /".to_string(),
            reason: "test".to_string(),
        };
        let msg = v.to_string();
        assert!(msg.contains("blocked"), "got: {msg}");
    }

    // ── Audit log ─────────────────────────────────────────────────────────────

    #[test]
    fn audit_log_creates_file() {
        let dir = TempDir::new().unwrap();
        let v = validator(&dir);
        v.audit_log("read_file", "src/main.rs", "allowed");
        let log = dir.path().join(".vibectl").join("security.log");
        assert!(log.exists(), "audit log should be created");
        let content = std::fs::read_to_string(log).unwrap();
        assert!(content.contains("read_file"), "got: {content}");
        assert!(content.contains("allowed"), "got: {content}");
    }
}

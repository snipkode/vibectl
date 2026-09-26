pub mod create_dir;
pub mod git;
pub mod patch_file;
pub mod read_file;
pub mod search;
pub mod shell;
pub mod symbols;
pub mod web;
pub mod write_file;

use anyhow::Result;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

/// Collapse `.` and `..` components without touching the filesystem.
///
/// `Path::join` is purely lexical, so `root.join("../../etc/passwd")` still
/// *starts_with* `root`.  Every containment check must normalize first.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() && !path.is_absolute() {
                    out.push("..");
                }
            }
            Component::Normal(part) => out.push(part),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// `root.canonicalize()` with a lexical fallback for roots that do not exist yet.
fn canonical_root(root: &Path) -> PathBuf {
    root.canonicalize().unwrap_or_else(|_| normalize(root))
}

/// Canonicalize the deepest component of `path` that exists, re-appending the
/// missing tail.  This keeps the check sound for writes, where the target file
/// does not exist yet but a parent directory may be a symlink pointing out of
/// the sandbox.
fn canonicalize_deepest(path: &Path) -> PathBuf {
    let normalized = normalize(path);
    let mut suffix: Vec<&std::ffi::OsStr> = Vec::new();
    let mut cursor: &Path = &normalized;

    loop {
        if let Ok(real) = cursor.canonicalize() {
            let mut out = real;
            for part in suffix.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (cursor.parent(), cursor.file_name()) {
            (Some(parent), Some(name)) => {
                suffix.push(name);
                cursor = parent;
            }
            _ => return normalized,
        }
    }
}

/// True when `candidate` is `root` itself or lives underneath it.
///
/// Guards two escape routes:
/// 1. lexical traversal (`../../.ssh/id_rsa`) — caught by normalizing first
/// 2. symlink escape (a directory inside the project linking to `/etc`) —
///    caught by comparing canonicalized paths
pub fn is_within(root: &Path, candidate: &Path) -> bool {
    let root = canonical_root(root);
    let candidate = canonicalize_deepest(candidate);
    candidate == root || candidate.starts_with(&root)
}

/// Resolve a tool-supplied path and confirm it stays inside `root`.
///
/// Returns the resolved path, or a human-readable refusal reason.  Callers turn
/// the reason into a tool result so the model sees *why* it was blocked instead
/// of hitting an opaque IO error.
pub fn resolve_within(root: &Path, cwd: &Path, requested: &str) -> Result<PathBuf, String> {
    let target = read_file::resolve_path(cwd, requested);
    if is_within(root, &target) {
        Ok(target)
    } else {
        Err(format!(
            "SAFEGUARD: refusing to access {} — outside the project root {}. \
             File access is confined to the project. Set allow_any_path: true in \
             config (or pass --dangerous-yes in headless mode) to override.",
            target.display(),
            root.display()
        ))
    }
}

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
        Box::new(patch_file::PatchFile),
        Box::new(symbols::ListSymbols),
        Box::new(search::GlobFiles),
        Box::new(search::GrepFiles),
        Box::new(shell::ShellExec),
        Box::new(shell::RunCommand),
        Box::new(shell::RunTests),
        Box::new(create_dir::CreateDir),
        Box::new(web::WebFetch),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_dot_segments() {
        assert_eq!(
            normalize(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
        assert_eq!(normalize(Path::new("/a/b/../../..")), PathBuf::from("/"));
        assert_eq!(normalize(Path::new("a/./b")), PathBuf::from("a/b"));
        assert_eq!(normalize(Path::new("")), PathBuf::from("."));
    }

    #[test]
    fn normalize_keeps_leading_parent_for_relative_paths() {
        assert_eq!(normalize(Path::new("../a")), PathBuf::from("../a"));
    }

    #[test]
    fn is_within_accepts_root_itself() {
        let root = tempfile::tempdir().unwrap();
        assert!(is_within(root.path(), root.path()));
    }

    #[test]
    fn is_within_accepts_nested_paths() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("src/agent/mod.rs");
        assert!(is_within(root.path(), &nested));
    }

    #[test]
    fn is_within_blocks_dotdot_traversal() {
        // The bug this guards: a lexical `starts_with` check accepts this,
        // because `root.join("../../etc/passwd")` still begins with root.
        let root = Path::new("/srv/project");
        let escape = root.join("../../etc/passwd");
        assert!(
            escape.starts_with(root),
            "precondition: lexical starts_with lies"
        );
        assert!(!is_within(root, &escape));
    }

    #[test]
    fn is_within_blocks_sibling_with_shared_prefix() {
        let root = Path::new("/srv/project");
        assert!(!is_within(root, Path::new("/srv/project-secrets/.env")));
        assert!(!is_within(root, Path::new("/srv")));
    }

    #[test]
    fn is_within_allows_missing_targets_for_writes() {
        let root = tempfile::tempdir().unwrap();
        let fresh = root.path().join("does/not/exist/yet.rs");
        assert!(is_within(root.path(), &fresh));
    }

    #[cfg(unix)]
    #[test]
    fn is_within_blocks_symlink_escape() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "s3cr3t").unwrap();

        let link = root.path().join("escape");
        symlink(outside.path(), &link).unwrap();

        // Lexically inside the project, physically outside it.
        assert!(!is_within(root.path(), &link.join("secret.txt")));
    }

    #[test]
    fn resolve_within_returns_path_for_relative_input() {
        let root = tempfile::tempdir().unwrap();
        let resolved = resolve_within(root.path(), root.path(), "src/main.rs").unwrap();
        assert!(resolved.ends_with("src/main.rs"));
    }

    #[test]
    fn resolve_within_reports_reason_for_escape() {
        let root = tempfile::tempdir().unwrap();
        let err = resolve_within(root.path(), root.path(), "/etc/passwd").unwrap_err();
        assert!(err.contains("SAFEGUARD"), "got: {err}");
        assert!(
            err.contains("allow_any_path"),
            "error should name the override"
        );
    }
}

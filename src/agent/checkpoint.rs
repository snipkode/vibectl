/// Checkpoint / undo support for vibectl.
///
/// Before the agent makes its first file modification in a run, we create a
/// `git stash` tagged with the run ID.  The `/undo` command pops the most
/// recent vibectl stash, reverting all changes made in that run.
///
/// Requirements:
/// - The project directory must be a git repository.
/// - There must be at least one commit (git stash needs a HEAD).
/// - If either condition is not met the functions return an informative error
///   instead of panicking.
use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;

/// Label prefix embedded in every stash message created by vibectl.
pub const STASH_PREFIX: &str = "vibectl-ckpt";

/// Create a git stash in `project_root` tagged with `run_id`.
///
/// The stash includes both staged and unstaged changes (--include-untracked
/// is intentionally NOT used — untracked files are the user's new files and
/// should not be stashed away).
///
/// Returns `Ok(stash_ref)` where `stash_ref` is the stash@{N} reference
/// (e.g. "stash@{0}"), or an error if the stash could not be created.
pub fn create(run_id: u64, project_root: &Path) -> Result<String> {
    // Verify this is a git repo with at least one commit.
    let status = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(project_root)
        .output()
        .context("failed to run git")?;

    if !status.status.success() {
        bail!(
            "checkpoint: cannot create stash — no git commits yet in {}",
            project_root.display()
        );
    }

    // Check if there are any tracked changes to stash.
    let diff = Command::new("git")
        .args(["diff", "--quiet", "HEAD"])
        .current_dir(project_root)
        .status()
        .context("failed to run git diff")?;

    let index_diff = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(project_root)
        .status()
        .context("failed to run git diff --cached")?;

    if diff.success() && index_diff.success() {
        // Nothing to stash — working tree is clean.
        bail!("checkpoint: working tree is clean, no changes to stash");
    }

    let message = format!("{STASH_PREFIX}-{run_id}");
    let out = Command::new("git")
        .args(["stash", "push", "-m", &message, "--keep-index"])
        .current_dir(project_root)
        .output()
        .context("failed to run git stash push")?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("checkpoint: git stash failed: {stderr}");
    }

    // Find the stash ref we just created.
    let stash_ref =
        find_latest_vibectl_stash(project_root)?.unwrap_or_else(|| "stash@{0}".to_string());

    Ok(stash_ref)
}

/// Pop (apply + drop) the most recent vibectl checkpoint stash.
///
/// Returns a human-readable summary of what was restored, or an error if
/// no vibectl stash was found or the pop failed.
pub fn undo(project_root: &Path) -> Result<String> {
    let stash_ref = find_latest_vibectl_stash(project_root)?
        .ok_or_else(|| anyhow::anyhow!("no vibectl checkpoint stash found — nothing to undo"))?;

    // Get the stash message for the summary.
    let msg_out = Command::new("git")
        .args(["stash", "show", "--stat", &stash_ref])
        .current_dir(project_root)
        .output()
        .context("failed to run git stash show")?;
    let stat = String::from_utf8_lossy(&msg_out.stdout).into_owned();

    // Pop the stash.
    let pop = Command::new("git")
        .args(["stash", "pop", "--index", &stash_ref])
        .current_dir(project_root)
        .output()
        .context("failed to run git stash pop")?;

    if !pop.status.success() {
        let stderr = String::from_utf8_lossy(&pop.stderr);
        // Try plain pop without --index as fallback.
        let pop2 = Command::new("git")
            .args(["stash", "pop", &stash_ref])
            .current_dir(project_root)
            .output()
            .context("failed to run git stash pop (fallback)")?;
        if !pop2.status.success() {
            bail!("undo: git stash pop failed: {stderr}");
        }
    }

    Ok(format!(
        "Restored checkpoint {stash_ref}:\n{}",
        if stat.trim().is_empty() {
            "(no stat available)".to_string()
        } else {
            stat.trim().to_string()
        }
    ))
}

/// List all vibectl checkpoint stashes with their run IDs.
/// Returns pairs of (stash_ref, message).
pub fn list(project_root: &Path) -> Result<Vec<(String, String)>> {
    let out = Command::new("git")
        .args(["stash", "list", "--format=%gd %s"])
        .current_dir(project_root)
        .output()
        .context("failed to run git stash list")?;

    if !out.status.success() {
        return Ok(vec![]);
    }

    let text = String::from_utf8_lossy(&out.stdout);
    let entries: Vec<(String, String)> = text
        .lines()
        .filter_map(|line| {
            let (ref_part, msg) = line.split_once(' ')?;
            if msg.contains(STASH_PREFIX) {
                Some((ref_part.to_string(), msg.to_string()))
            } else {
                None
            }
        })
        .collect();

    Ok(entries)
}

/// Find the most recent stash created by vibectl (contains STASH_PREFIX).
fn find_latest_vibectl_stash(project_root: &Path) -> Result<Option<String>> {
    let entries = list(project_root)?;
    Ok(entries.into_iter().next().map(|(r, _)| r))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn init_git_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        // Initial commit so stash has a HEAD.
        fs::write(dir.path().join("init.txt"), "init").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    #[test]
    fn create_stash_and_undo() {
        let dir = init_git_repo();

        // Make a tracked change.
        fs::write(dir.path().join("init.txt"), "modified").unwrap();

        // Create checkpoint.
        let stash_ref = create(42, dir.path()).expect("create checkpoint");
        assert!(stash_ref.contains("stash@{"));

        // File should be restored to original after stash.
        let content = fs::read_to_string(dir.path().join("init.txt")).unwrap();
        assert_eq!(content, "init");

        // Undo should restore "modified".
        let summary = undo(dir.path()).expect("undo");
        assert!(summary.contains("stash@{"));
        let content = fs::read_to_string(dir.path().join("init.txt")).unwrap();
        assert_eq!(content, "modified");
    }

    #[test]
    fn clean_tree_returns_error() {
        let dir = init_git_repo();
        // No changes → stash should fail gracefully.
        let err = create(1, dir.path()).unwrap_err();
        assert!(err.to_string().contains("clean"));
    }

    #[test]
    fn no_stash_undo_returns_error() {
        let dir = init_git_repo();
        let err = undo(dir.path()).unwrap_err();
        assert!(err.to_string().contains("nothing to undo"));
    }

    #[test]
    fn list_shows_vibectl_stashes() {
        let dir = init_git_repo();
        fs::write(dir.path().join("init.txt"), "v1").unwrap();
        create(10, dir.path()).unwrap();

        let entries = list(dir.path()).unwrap();
        assert!(!entries.is_empty());
        assert!(entries[0].1.contains(STASH_PREFIX));
    }
}

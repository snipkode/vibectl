use super::read_file::resolve_path;
use super::{Tool, ToolDef, ToolResult};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::Path;

pub struct PatchFile;

impl Tool for PatchFile {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "patch_file",
            r#"Apply a unified diff patch to an existing file. Safer and more token-efficient than
write_file because only the changed lines are transmitted. The patch must be in standard
unified diff format produced by `diff -u` or `git diff` (without the `--- a/` / `+++ b/`
file headers — just the @@ hunks).

Example patch for changing a single line:
  @@ -3,4 +3,4 @@
   context line
  -old line
  +new line
   context line

Rules:
- Each hunk header must be `@@ -L,N +L,N @@` (optional trailing text after the second @@).
- Context lines (no prefix) must match the file exactly.
- Use write_file instead when creating a brand-new file."#,
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to patch (relative to project root)"
                    },
                    "patch": {
                        "type": "string",
                        "description": "Unified diff hunks (starting from @@ lines, no file headers needed)"
                    }
                },
                "required": ["path", "patch"]
            }),
        )
    }

    fn run(&self, args: &Value, cwd: &Path) -> Result<ToolResult> {
        let path_str = args
            .get("path")
            .context("missing 'path'")?
            .as_str()
            .context("'path' must be a string")?;
        let patch_str = args
            .get("patch")
            .context("missing 'patch'")?
            .as_str()
            .context("'patch' must be a string")?;

        let target = resolve_path(cwd, path_str);

        if !target.exists() {
            bail!(
                "patch_file: {} does not exist. Use write_file to create new files.",
                target.display()
            );
        }

        let original = std::fs::read_to_string(&target)
            .with_context(|| format!("failed to read {}", target.display()))?;

        let patched = apply_patch(&original, patch_str)
            .with_context(|| format!("failed to apply patch to {}", target.display()))?;

        std::fs::write(&target, &patched)
            .with_context(|| format!("failed to write {}", target.display()))?;

        let orig_lines = original.lines().count();
        let new_lines = patched.lines().count();
        let delta: i64 = new_lines as i64 - orig_lines as i64;
        let delta_str = if delta >= 0 {
            format!("+{delta}")
        } else {
            format!("{delta}")
        };

        Ok(ToolResult {
            content: format!(
                "Patched {} ({orig_lines} → {new_lines} lines, {delta_str})",
                target.display()
            ),
        })
    }
}

// ─── Patch engine ─────────────────────────────────────────────────────────────

/// Apply unified diff hunks to `original` text. Returns the patched content.
///
/// Supports the standard `@@ -old_start,old_count +new_start,new_count @@` format.
/// Lines prefixed with `-` are removed, `+` are inserted, ` ` (space) are context.
fn apply_patch(original: &str, patch: &str) -> Result<String> {
    // Normalise line endings — work with \n internally.
    let original = original.replace("\r\n", "\n");
    let patch = patch.replace("\r\n", "\n");

    let orig_lines: Vec<&str> = original.lines().collect();
    let mut output: Vec<&str> = Vec::with_capacity(orig_lines.len());

    // `cursor` tracks our position in orig_lines (0-indexed).
    let mut cursor: usize = 0;

    let patch_lines: Vec<&str> = patch.lines().collect();
    let mut i = 0;

    // Skip optional file-header lines (--- / +++ / diff --git …)
    while i < patch_lines.len() {
        let l = patch_lines[i];
        if l.starts_with("--- ") || l.starts_with("+++ ") || l.starts_with("diff ") {
            i += 1;
        } else {
            break;
        }
    }

    while i < patch_lines.len() {
        let line = patch_lines[i];

        // Parse hunk header: @@ -old_start[,old_count] +new_start[,new_count] @@
        if let Some(hunk) = parse_hunk_header(line) {
            i += 1;

            // old_start is 1-indexed; convert to 0-indexed.
            let hunk_orig_start = hunk.old_start.saturating_sub(1);

            // Copy unchanged lines from cursor up to the hunk start.
            if hunk_orig_start < cursor {
                bail!(
                    "patch hunks are out of order or overlap: hunk starts at line {} \
                     but cursor is already at {}",
                    hunk.old_start,
                    cursor + 1
                );
            }
            for j in cursor..hunk_orig_start {
                output.push(orig_lines.get(j).copied().unwrap_or(""));
            }
            cursor = hunk_orig_start;

            // Apply hunk lines.
            let mut orig_consumed = 0usize;
            while i < patch_lines.len() {
                let hl = patch_lines[i];

                if hl.starts_with("@@") {
                    // Next hunk — break inner loop.
                    break;
                }

                if let Some(rest) = hl.strip_prefix("- ").or_else(|| {
                    // Also handle single-char prefix without space for robustness.
                    if hl.starts_with('-') && !hl.starts_with("---") {
                        Some(&hl[1..])
                    } else {
                        None
                    }
                }) {
                    // Remove: verify the line matches, then advance cursor.
                    let orig = orig_lines
                        .get(cursor + orig_consumed)
                        .copied()
                        .unwrap_or("");
                    if orig != rest {
                        bail!(
                            "patch mismatch at original line {}: expected {:?}, got {:?}",
                            cursor + orig_consumed + 1,
                            rest,
                            orig
                        );
                    }
                    orig_consumed += 1;
                    i += 1;
                } else if let Some(rest) = hl.strip_prefix("+ ").or_else(|| {
                    if hl.starts_with('+') && !hl.starts_with("+++") {
                        Some(&hl[1..])
                    } else {
                        None
                    }
                }) {
                    // Insert: push the new line, do NOT advance orig cursor.
                    output.push(rest);
                    i += 1;
                } else {
                    // Context line (space prefix or bare line).
                    let ctx = if hl.starts_with(' ') { &hl[1..] } else { hl };
                    let orig = orig_lines
                        .get(cursor + orig_consumed)
                        .copied()
                        .unwrap_or("");
                    if orig != ctx {
                        bail!(
                            "context mismatch at original line {}: expected {:?}, got {:?}",
                            cursor + orig_consumed + 1,
                            ctx,
                            orig
                        );
                    }
                    output.push(orig);
                    orig_consumed += 1;
                    i += 1;
                }
            }

            cursor += orig_consumed;
        } else {
            // Non-hunk line outside a hunk block — skip (e.g. trailing newline).
            i += 1;
        }
    }

    // Copy any remaining lines after the last hunk.
    for j in cursor..orig_lines.len() {
        output.push(orig_lines[j]);
    }

    // Preserve trailing newline if the original had one.
    let mut result = output.join("\n");
    if original.ends_with('\n') {
        result.push('\n');
    }

    Ok(result)
}

struct HunkHeader {
    old_start: usize,
    #[allow(dead_code)]
    old_count: usize,
    #[allow(dead_code)]
    new_start: usize,
    #[allow(dead_code)]
    new_count: usize,
}

/// Parse `@@ -old_start[,old_count] +new_start[,new_count] @@` header.
fn parse_hunk_header(line: &str) -> Option<HunkHeader> {
    // Must start with @@
    let line = line.strip_prefix("@@")?;
    // Find closing @@
    let end = line.find(" @@")?;
    let inner = line[..end].trim(); // e.g. "-3,4 +3,4"

    let mut parts = inner.split_whitespace();
    let old_part = parts.next()?; // "-3,4" or "-3"
    let new_part = parts.next()?; // "+3,4" or "+3"

    let (old_start, old_count) = parse_range(old_part.strip_prefix('-')?)?;
    let (new_start, new_count) = parse_range(new_part.strip_prefix('+')?)?;

    Some(HunkHeader {
        old_start,
        old_count,
        new_start,
        new_count,
    })
}

fn parse_range(s: &str) -> Option<(usize, usize)> {
    if let Some((a, b)) = s.split_once(',') {
        Some((a.parse().ok()?, b.parse().ok()?))
    } else {
        Some((s.parse().ok()?, 1))
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(original: &str, diff: &str) -> String {
        apply_patch(original, diff).expect("patch failed")
    }

    #[test]
    fn replace_single_line() {
        let orig = "line1\nline2\nline3\n";
        let diff = "@@ -2,1 +2,1 @@\n-line2\n+line2_new\n";
        assert_eq!(patch(orig, diff), "line1\nline2_new\nline3\n");
    }

    #[test]
    fn insert_line() {
        let orig = "a\nb\nc\n";
        let diff = "@@ -1,3 +1,4 @@\n a\n b\n+inserted\n c\n";
        assert_eq!(patch(orig, diff), "a\nb\ninserted\nc\n");
    }

    #[test]
    fn delete_line() {
        let orig = "a\nb\nc\n";
        let diff = "@@ -1,3 +1,2 @@\n a\n-b\n c\n";
        assert_eq!(patch(orig, diff), "a\nc\n");
    }

    #[test]
    fn multiple_hunks() {
        let orig = "a\nb\nc\nd\ne\n";
        let diff = "@@ -1,1 +1,1 @@\n-a\n+A\n@@ -5,1 +5,1 @@\n-e\n+E\n";
        assert_eq!(patch(orig, diff), "A\nb\nc\nd\nE\n");
    }

    #[test]
    fn context_mismatch_returns_error() {
        let orig = "a\nb\nc\n";
        // Context says "x" but file has "b"
        let diff = "@@ -1,3 +1,3 @@\n a\n x\n c\n";
        assert!(apply_patch(orig, diff).is_err());
    }

    #[test]
    fn preserves_no_trailing_newline() {
        let orig = "a\nb";
        let diff = "@@ -2,1 +2,1 @@\n-b\n+B\n";
        assert_eq!(patch(orig, diff), "a\nB");
    }

    #[test]
    fn skips_file_headers() {
        let orig = "x\ny\n";
        let diff = "--- a/file.txt\n+++ b/file.txt\n@@ -1,2 +1,2 @@\n-x\n+X\n y\n";
        assert_eq!(patch(orig, diff), "X\ny\n");
    }
}

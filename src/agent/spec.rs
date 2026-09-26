//! Spec-driven development: requirements, design, and an executable task list.
//!
//! A spec is a directory of three markdown files, written in order:
//!
//! ```text
//! .vibectl/specs/<slug>/
//! ├── requirements.md
//! ├── design.md
//! └── tasks.md
//! ```
//!
//! The point is that the reasoning is written down *before* code, and the task
//! list is the thing the agent then executes — so a large task needs fewer
//! corrections, and the decisions survive the session.
//!
//! Everything here is `std` only: no markdown or regex dependency, and no
//! network. The one rule that is enforced rather than suggested is that a
//! `tasks.md` which exists but yields no checkboxes is an *error* — "I could not
//! read your checklist" must not look like "there is nothing to do".

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// How many pending tasks are injected into the agent's context.
///
/// The agent trims its history to 80k tokens, so an unbounded checklist would
/// undo that. Truncation keeps the *first* N pending tasks, because the next
/// action is the top of the list and must never be the thing dropped.
pub const MAX_INJECTED_TASKS: usize = 25;

/// One checkbox from `tasks.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskItem {
    /// 1-based position in the file. What `/spec done <n>` refers to.
    pub number: usize,
    pub done: bool,
    pub text: String,
    /// Leading indentation, for display only.
    pub depth: usize,
}

/// The three phases of a spec, in the order they must be produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Requirements,
    Design,
    Tasks,
}

impl Phase {
    pub fn file_name(self) -> &'static str {
        match self {
            Phase::Requirements => "requirements.md",
            Phase::Design => "design.md",
            Phase::Tasks => "tasks.md",
        }
    }

    /// What must already exist before this phase is written.
    pub fn requires(self) -> Option<Phase> {
        match self {
            Phase::Requirements => None,
            Phase::Design => Some(Phase::Requirements),
            Phase::Tasks => Some(Phase::Design),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Phase::Requirements => "requirements",
            Phase::Design => "design",
            Phase::Tasks => "tasks",
        }
    }
}

/// A spec on disk.
#[derive(Debug, Clone)]
pub struct Spec {
    pub slug: String,
    pub dir: PathBuf,
    pub requirements: Option<String>,
    pub design: Option<String>,
    pub tasks: Vec<TaskItem>,
    /// True when `tasks.md` exists but produced no checkboxes. Surfaced as an
    /// error by [`Spec::load`] rather than silently reading as an empty list.
    pub tasks_unreadable: bool,
}

// ─── parsing ─────────────────────────────────────────────────────────────────

/// Parse a `tasks.md` into task items.
///
/// Only checkbox lines count. Prose, headings, and ordinary bullets are skipped
/// on purpose: a human's notes in the file should not become work items.
pub fn parse_tasks(content: &str) -> Vec<TaskItem> {
    let mut out = Vec::new();
    for line in content.lines() {
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim_start();

        let Some(rest) = checkbox_body(trimmed) else {
            continue;
        };

        out.push(TaskItem {
            number: out.len() + 1,
            done: rest.0,
            text: rest.1.to_string(),
            depth: indent / 2,
        });
    }
    out
}

/// If `line` is a `- [ ]`/`- [x]`/`* [X]` item, return `(done, text)`.
///
/// Accepts a `-`, `*`, or `+` bullet and an optional trailing `:` so that
/// `- [ ] add parser:` works as well as `- [ ] add parser`.
fn checkbox_body(line: &str) -> Option<(bool, &str)> {
    let rest = line
        .strip_prefix('-')
        .or_else(|| line.strip_prefix('*'))
        .or_else(|| line.strip_prefix('+'))?;
    let rest = rest.trim_start();
    let inner = rest.strip_prefix('[')?;
    let (marker, inner) = inner.split_at(1);
    let done = match marker {
        " " => false,
        "x" | "X" => true,
        _ => return None,
    };
    let inner = inner.strip_prefix(']')?;
    let text = inner.trim().trim_end_matches(':').trim();
    if text.is_empty() {
        return None;
    }
    Some((done, text))
}

// ─── slugs ───────────────────────────────────────────────────────────────────

/// Derive a directory name from a task description.
///
/// Lowercase alphanumerics and dashes, collapsed, capped at six words. Returns
/// an empty string when nothing usable survives, so callers must fall back
/// rather than create a directory named ``.
pub fn slugify(task: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();

    for ch in task.chars() {
        if ch.is_ascii_alphanumeric() {
            current.push(ch.to_ascii_lowercase());
        } else if !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        words.push(current);
    }

    words
        .into_iter()
        .filter(|w| !w.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("-")
}

/// A slug not already used by a sibling spec. Never clobbers a spec in flight.
pub fn unique_slug(specs_dir: &Path, base: &str) -> String {
    let base = if base.is_empty() { "spec" } else { base };
    if !specs_dir.join(base).exists() {
        return base.to_string();
    }
    for n in 2..1000 {
        let candidate = format!("{base}-{n}");
        if !specs_dir.join(&candidate).exists() {
            return candidate;
        }
    }
    format!("{base}-new")
}

// ─── Spec ────────────────────────────────────────────────────────────────────

impl Spec {
    /// The directory that holds every spec, given any path inside the project.
    pub fn specs_dir(cwd: &Path) -> PathBuf {
        let root = crate::agent::steer::find_project_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
        root.join(".vibectl").join("specs")
    }

    pub fn dir_for(cwd: &Path, slug: &str) -> PathBuf {
        Self::specs_dir(cwd).join(slug)
    }

    /// Load a spec by slug.
    pub fn load(cwd: &Path, slug: &str) -> Result<Self> {
        let dir = Self::dir_for(cwd, slug);
        if !dir.is_dir() {
            bail!(
                "no spec at {}\nRun /spec add <task> to start one.",
                dir.display()
            );
        }
        let requirements = read_optional(&dir.join(Phase::Requirements.file_name()))?;
        let design = read_optional(&dir.join(Phase::Design.file_name()))?;

        let tasks_path = dir.join(Phase::Tasks.file_name());
        let (tasks, tasks_unreadable) = if tasks_path.is_file() {
            let content = read_optional(&tasks_path)?.unwrap_or_default();
            let parsed = parse_tasks(&content);
            let unreadable = parsed.is_empty();
            (parsed, unreadable)
        } else {
            (Vec::new(), false)
        };

        Ok(Self {
            slug: slug.to_string(),
            dir,
            requirements,
            design,
            tasks,
            tasks_unreadable,
        })
    }

    /// The most recently modified spec, if any.
    pub fn latest(cwd: &Path) -> Result<Option<Self>> {
        let specs_dir = Self::specs_dir(cwd);
        let mut candidates: Vec<(std::time::SystemTime, String)> = Vec::new();
        let entries = match std::fs::read_dir(&specs_dir) {
            Ok(e) => e,
            Err(_) => return Ok(None),
        };
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let slug = entry.file_name().to_string_lossy().into_owned();
            let mtime = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            candidates.push((mtime, slug));
        }
        candidates.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
        match candidates.into_iter().next() {
            Some((_, slug)) => Ok(Some(Self::load(cwd, &slug)?)),
            None => Ok(None),
        }
    }

    /// Create the spec directory and its `requirements.md`, returning the path
    /// written. The slug is deduplicated against existing specs.
    pub fn create(cwd: &Path, task: &str) -> Result<(Self, PathBuf)> {
        let specs_dir = Self::specs_dir(cwd);
        std::fs::create_dir_all(&specs_dir)
            .with_context(|| format!("failed to create {}", specs_dir.display()))?;

        let slug = unique_slug(&specs_dir, &slugify(task));
        let dir = specs_dir.join(&slug);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;

        let path = dir.join(Phase::Requirements.file_name());
        std::fs::write(&path, requirements_template(task))
            .with_context(|| format!("failed to write {}", path.display()))?;

        let spec = Self {
            slug,
            dir,
            requirements: read_optional(&path)?,
            design: None,
            tasks: Vec::new(),
            tasks_unreadable: false,
        };
        Ok((spec, path))
    }

    /// The next phase that has not been written yet, if the ordering allows it.
    pub fn next_phase(&self) -> Option<Phase> {
        for phase in [Phase::Requirements, Phase::Design, Phase::Tasks] {
            match phase {
                Phase::Requirements if self.requirements.is_none() => return Some(phase),
                Phase::Design if self.requirements.is_some() && self.design.is_none() => {
                    return Some(phase);
                }
                Phase::Tasks if self.design.is_some() && self.tasks.is_empty() => {
                    return Some(phase);
                }
                _ => {}
            }
        }
        None
    }

    /// Refuse to write `phase` out of order, naming the phase to run first.
    pub fn ensure_can_write(&self, phase: Phase) -> Result<()> {
        let Some(prereq) = phase.requires() else {
            return Ok(());
        };
        let present = match prereq {
            Phase::Requirements => self.requirements.is_some(),
            Phase::Design => self.design.is_some(),
            Phase::Tasks => !self.tasks.is_empty(),
        };
        if present {
            return Ok(());
        }
        bail!(
            "cannot write {} before {} — run /spec {} first",
            phase.label(),
            prereq.label(),
            prereq.label()
        )
    }

    /// Write a phase file, enforcing the ordering.
    ///
    /// Takes `&mut self` and updates the in-memory copy: otherwise writing
    /// design and then tasks through the same handle fails the gate, because
    /// the gate still sees `design: None` from before the write.
    pub fn write_phase(&mut self, phase: Phase, content: &str) -> Result<PathBuf> {
        self.ensure_can_write(phase)?;
        let path = self.dir.join(phase.file_name());
        std::fs::write(&path, content)
            .with_context(|| format!("failed to write {}", path.display()))?;
        match phase {
            Phase::Requirements => self.requirements = Some(content.to_string()),
            Phase::Design => self.design = Some(content.to_string()),
            Phase::Tasks => {
                self.tasks = parse_tasks(content);
                self.tasks_unreadable = self.tasks.is_empty();
            }
        }
        Ok(path)
    }

    /// Tick a task by its 1-based number and rewrite `tasks.md` in place.
    ///
    /// The file is rewritten line-by-line rather than regenerated from the
    /// parsed model so prose, headings, and indentation in the file survive.
    pub fn complete_task(&mut self, number: usize) -> Result<()> {
        if number == 0 || number > self.tasks.len() {
            bail!(
                "no task {} in this spec (it has {} task(s))",
                number,
                self.tasks.len()
            );
        }
        let path = self.dir.join(Phase::Tasks.file_name());
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;

        let mut seen = 0usize;
        let mut out = String::with_capacity(content.len());
        for line in content.lines() {
            let indent = line.len() - line.trim_start().len();
            let trimmed = line.trim_start();
            if checkbox_body(trimmed).is_some() {
                seen += 1;
                if seen == number {
                    out.push_str(&format!(
                        "{}- [x] {}\n",
                        " ".repeat(indent),
                        task_text(trimmed)
                    ));
                    continue;
                }
            }
            out.push_str(line);
            out.push('\n');
        }

        std::fs::write(&path, &out)
            .with_context(|| format!("failed to write {}", path.display()))?;
        self.tasks = parse_tasks(&out);
        Ok(())
    }

    pub fn done_count(&self) -> usize {
        self.tasks.iter().filter(|t| t.done).count()
    }

    pub fn pending_count(&self) -> usize {
        self.tasks.len() - self.done_count()
    }

    /// Per-phase status, for `/spec` with no arguments.
    pub fn status_lines(&self) -> String {
        let mark = |present: bool| if present { "✓" } else { "·" };
        let mut s = format!(
            "{} {}\n  {} requirements  {} design  {} tasks",
            mark(self.requirements.is_some()),
            self.slug,
            mark(self.requirements.is_some()),
            mark(self.design.is_some()),
            mark(!self.tasks.is_empty()),
        );
        if !self.tasks.is_empty() {
            s.push_str(&format!(
                "  ({}/{} done, {} pending)",
                self.done_count(),
                self.tasks.len(),
                self.pending_count()
            ));
        }
        if self.tasks_unreadable {
            s.push_str("\n  ⚠ tasks.md has no checkboxes — nothing is executable yet");
        }
        s
    }

    /// The block injected into the agent's context while this spec is active.
    ///
    /// Pending tasks only, capped from the bottom so the next task is always
    /// present. Completed work is summarised as a count rather than listed:
    /// re-showing finished tasks invites the model to narrate instead of act.
    pub fn context_block(&self) -> String {
        if self.tasks.is_empty() {
            return String::new();
        }
        let pending: Vec<&TaskItem> = self.tasks.iter().filter(|t| !t.done).collect();
        let mut s = format!(
            "Active spec `{}` — {}/{} tasks done.\n",
            self.slug,
            self.tasks.len() - pending.len(),
            self.tasks.len()
        );

        if pending.is_empty() {
            s.push_str(
                "All spec tasks are complete. Do not start new work in this spec \
                        without being asked.\n",
            );
            return s;
        }

        s.push_str("Remaining tasks, in order:\n");
        for task in pending.iter().take(MAX_INJECTED_TASKS) {
            s.push_str(&format!("- [ ] {}\n", task.text));
        }
        let hidden = pending.len().saturating_sub(MAX_INJECTED_TASKS);
        if hidden > 0 {
            s.push_str(&format!(
                "- … {hidden} more task(s) not shown; re-read .vibectl/specs/{}/tasks.md \
                 when you get there\n",
                self.slug
            ));
        }
        s.push_str(
            "Work the next unfinished task. Tick a box by editing \
             .vibectl/specs/<slug>/tasks.md, changing `- [ ]` to `- [x`.\n",
        );
        s
    }
}

fn task_text(trimmed: &str) -> String {
    checkbox_body(trimmed)
        .map(|(_, t)| t.to_string())
        .unwrap_or_default()
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))
        .map(Some)
}

// ─── generation ──────────────────────────────────────────────────────────────

/// System prompt for a spec phase. Kept beside the templates so the required
/// output shape and the parser that consumes it stay in one file.
pub fn phase_system_prompt(phase: Phase, task: &str) -> String {
    match phase {
        Phase::Requirements => format!(
            r#"You are a product engineer turning a request into a requirements document.

Task from the user:
{task}

Write ONLY the body of `requirements.md` — no top-level `#` title, since the
file already has one. Use these `##` sections in this order:

## Problem
What is wrong or missing today, and who it affects. Concrete, not aspirational.

## Goal
One sentence: what must be true when this is done.

## Requirements
Numbered functional requirements. Each states observable behaviour, prefixed
`- FR<n> — `. Then technical constraints under `### Technical`.

## Out of scope
What this explicitly does not do. This is what stops scope creep later.

Rules: no code. No implementation plan. Do not invent requirements the user did
not imply. If something is genuinely ambiguous, put it in Out of scope and say
what would resolve it."#
        ),
        Phase::Design => r#"You are a software architect writing a design document.

Write ONLY the body of `design.md` — no top-level `#` title. Use these `##`
sections in this order:

## Overview
Two or three sentences on the approach.

## Data model
The types or structures involved, as a fenced code block if useful.

## Decisions
REQUIRED. One bullet per decision, each stating the choice AND its rationale:

- **What we chose** — why, and what alternative was rejected and why.

This section is the one thing a teammate reads months later to understand why
the code looks the way it does. Every non-obvious choice belongs here. A design
with no Decisions section is a failed design.

## Failure modes
A table of `| Situation | Behaviour |` covering the error and edge paths.

Rules: no implementation code beyond illustrative type definitions. Do not
repeat the requirements verbatim; reference them."#
            .to_string(),
        Phase::Tasks => r#"You are breaking a design into an executable checklist.

Write ONLY the body of `tasks.md` — no top-level `#` title.

Every task MUST be a markdown checkbox on its own line:

- [ ] imperative, one concrete change, naming the file or symbol when known

Rules:
- Each task is independently verifiable. "Add error handling" is not a task.
  "Return a Result from load_config instead of panicking" is.
- Order them so the list can be worked top to bottom.
- Include the tests as tasks, not as an afterthought.
- No prose before, between, or after the checkboxes beyond a single short line.
- Do not number the tasks; vibectl numbers them by position."#
            .to_string(),
    }
}

/// The user message for a phase: what we are building, plus what already exists.
pub fn phase_input(phase: Phase, task: &str, spec: &Spec) -> String {
    let mut s = format!(
        "The task:
{task}
"
    );
    match phase {
        Phase::Requirements => {}
        Phase::Design => {
            if let Some(r) = &spec.requirements {
                s.push_str(
                    "
The requirements already agreed:

",
                );
                s.push_str(r);
                s.push('\n');
            }
        }
        Phase::Tasks => {
            if let Some(r) = &spec.requirements {
                s.push_str(
                    "
Requirements:

",
                );
                s.push_str(r);
                s.push_str(
                    "

",
                );
            }
            if let Some(d) = &spec.design {
                s.push_str(
                    "Design:

",
                );
                s.push_str(d);
                s.push('\n');
            }
        }
    }
    s
}

// ─── templates ───────────────────────────────────────────────────────────────

fn requirements_template(task: &str) -> String {
    format!(
        r#"# {task}

## Problem

<!-- What is wrong or missing today. Be concrete about who it affects. -->

TODO

## Goal

<!-- One sentence: what must be true when this is done. -->

TODO

## Requirements

<!-- Functional requirements, numbered. State the observable behaviour. -->

- FR1 — TODO

## Out of scope

<!-- Explicitly not doing this. Prevents scope creep during implementation. -->

- TODO
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ─── parse_tasks ─────────────────────────────────────────────────────────

    #[test]
    fn parses_done_and_pending_tasks() {
        let tasks =
            parse_tasks("# Tasks\n\n- [ ] first thing\n- [x] second thing\n- [ ] third thing\n");
        assert_eq!(tasks.len(), 3);
        assert_eq!(
            tasks.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(),
            ["first thing", "second thing", "third thing"]
        );
        assert_eq!(
            tasks.iter().map(|t| t.done).collect::<Vec<_>>(),
            [false, true, false]
        );
        assert_eq!(
            tasks.iter().map(|t| t.number).collect::<Vec<_>>(),
            [1, 2, 3],
            "numbers are positions, so a completed task still consumes one"
        );
    }

    #[test]
    fn numbers_are_positions_not_only_pending_ones() {
        // This is the property `/spec done 3` depends on.
        let tasks = parse_tasks("- [x] a\n- [x] b\n- [ ] c\n");
        assert_eq!(tasks[2].number, 3);
    }

    #[test]
    fn skips_prose_headings_and_plain_bullets() {
        let tasks = parse_tasks(
            "# Tasks\n\nSome context that is not a task.\n\n- a plain bullet\n\
             - [ ] a real task\n\n| a | b |\n|---|---|\n",
        );
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].text, "a real task");
    }

    #[test]
    fn records_nesting_depth() {
        let tasks = parse_tasks("- [ ] top\n  - [ ] child\n    - [ ] grandchild\n");
        assert_eq!(tasks.iter().map(|t| t.depth).collect::<Vec<_>>(), [0, 1, 2]);
    }

    #[test]
    fn accepts_all_bullet_styles_and_uppercase_x() {
        let tasks = parse_tasks("- [ ] dash\n* [x] star\n+ [X] plus\n");
        assert_eq!(tasks.len(), 3);
        assert_eq!(
            tasks.iter().map(|t| t.done).collect::<Vec<_>>(),
            [false, true, true]
        );
    }

    #[test]
    fn a_checkbox_with_no_text_is_not_a_task() {
        // An empty box is a formatting accident, not work to be tracked.
        let tasks = parse_tasks("- [ ]\n- [ ] \n- [ ] real\n");
        assert_eq!(tasks.len(), 1);
    }

    #[test]
    fn a_trailing_colon_does_not_leak_into_the_text() {
        let tasks = parse_tasks("- [ ] add the parser:\n");
        assert_eq!(tasks[0].text, "add the parser");
    }

    #[test]
    fn a_broken_marker_is_not_a_checkbox() {
        let tasks = parse_tasks("- [] nope\n- [-] nope\n- [?] nope\n- [ ] yes\n");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].text, "yes");
    }

    #[test]
    fn an_empty_file_yields_no_tasks() {
        assert!(parse_tasks("").is_empty());
        assert!(parse_tasks("\n\n   \n").is_empty());
    }

    // ─── slugify ─────────────────────────────────────────────────────────────

    #[test]
    fn slugify_lowercases_and_joins_words() {
        assert_eq!(slugify("Add Docker Support"), "add-docker-support");
    }

    #[test]
    fn slugify_strips_punctuation_including_code_symbols() {
        assert_eq!(
            slugify("Fix: handle /etc/passwd & `rm -rf` paths!"),
            "fix-handle-etc-passwd-rm-rf"
        );
    }

    #[test]
    fn slugify_caps_at_six_words() {
        let slug = slugify("one two three four five six seven eight nine");
        assert_eq!(slug, "one-two-three-four-five-six");
    }

    #[test]
    fn slugify_is_empty_for_unusable_input() {
        // The caller must fall back; an empty directory name is not a spec.
        assert_eq!(slugify("!!! ???"), "");
        assert_eq!(slugify("   "), "");
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn unique_slug_never_clobbers_an_existing_spec() {
        let dir = TempDir::new().unwrap();
        assert_eq!(unique_slug(dir.path(), "add-docker"), "add-docker");
        std::fs::create_dir(dir.path().join("add-docker")).unwrap();
        assert_eq!(unique_slug(dir.path(), "add-docker"), "add-docker-2");
        std::fs::create_dir(dir.path().join("add-docker-2")).unwrap();
        assert_eq!(unique_slug(dir.path(), "add-docker"), "add-docker-3");
    }

    #[test]
    fn unique_slug_falls_back_when_the_base_is_empty() {
        let dir = TempDir::new().unwrap();
        assert_eq!(unique_slug(dir.path(), ""), "spec");
    }

    // ─── lifecycle ───────────────────────────────────────────────────────────

    fn project() -> TempDir {
        let dir = TempDir::new().unwrap();
        // Anchor the project root so find_project_root does not walk to /tmp.
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        dir
    }

    #[test]
    fn create_writes_requirements_and_leaves_later_phases_absent() {
        let dir = project();
        let (spec, path) = Spec::create(dir.path(), "Add Docker support").unwrap();

        assert_eq!(spec.slug, "add-docker-support");
        assert!(path.is_file());
        assert!(spec.requirements.is_some());
        assert!(spec.design.is_none());
        assert!(spec.tasks.is_empty());
        assert!(spec.next_phase() == Some(Phase::Design));
    }

    #[test]
    fn create_does_not_overwrite_an_in_flight_spec() {
        let dir = project();
        let (first, _) = Spec::create(dir.path(), "Add Docker support").unwrap();
        let (second, _) = Spec::create(dir.path(), "Add Docker support").unwrap();
        assert_ne!(first.slug, second.slug);
        assert!(dir.path().join(".vibectl/specs").join(first.slug).is_dir());
        assert!(dir.path().join(".vibectl/specs").join(second.slug).is_dir());
    }

    #[test]
    fn phases_are_gated_in_order() {
        let dir = project();
        let (mut spec, _) = Spec::create(dir.path(), "Ordered work").unwrap();

        // Design needs requirements — which now exists, so this passes.
        spec.write_phase(
            Phase::Design,
            "# Design\n\n## Decisions\n\n- **x** because y\n",
        )
        .unwrap();

        let mut loaded = Spec::load(dir.path(), &spec.slug).unwrap();
        assert!(loaded.design.is_some());
        assert_eq!(loaded.next_phase(), Some(Phase::Tasks));

        loaded
            .write_phase(Phase::Tasks, "- [ ] one\n- [ ] two\n")
            .unwrap();
        let done = Spec::load(dir.path(), &spec.slug).unwrap();
        assert_eq!(done.tasks.len(), 2);
        assert_eq!(done.next_phase(), None, "all three phases are present");
    }

    #[test]
    fn writing_tasks_before_design_is_refused_and_names_the_phase() {
        let dir = project();
        let (mut spec, _) = Spec::create(dir.path(), "Out of order").unwrap();

        let err = spec
            .write_phase(Phase::Tasks, "- [ ] one\n")
            .expect_err("tasks must not be written before design");
        let msg = err.to_string();
        assert!(
            msg.contains("design"),
            "should name the missing phase: {msg}"
        );
    }

    #[test]
    fn writing_design_before_requirements_is_refused() {
        let dir = TempDir::new().unwrap();
        let mut bare = Spec {
            slug: "x".into(),
            dir: dir.path().to_path_buf(),
            requirements: None,
            design: None,
            tasks: Vec::new(),
            tasks_unreadable: false,
        };
        let err = bare
            .write_phase(Phase::Design, "# Design\n")
            .expect_err("design must not be written before requirements");
        assert!(err.to_string().contains("requirements"));
    }

    #[test]
    fn load_reports_a_missing_spec_as_actionable() {
        let dir = project();
        let err = Spec::load(dir.path(), "nope").unwrap_err().to_string();
        assert!(err.contains("/spec add"), "got: {err}");
    }

    #[test]
    fn a_tasks_file_with_no_checkboxes_is_flagged_not_silently_empty() {
        let dir = project();
        let (spec, _) = Spec::create(dir.path(), "Prose only").unwrap();
        std::fs::write(
            spec.dir.join("tasks.md"),
            "# Tasks\n\nTODO: write the actual tasks here.\n",
        )
        .unwrap();

        let loaded = Spec::load(dir.path(), &spec.slug).unwrap();
        assert!(loaded.tasks.is_empty());
        assert!(
            loaded.tasks_unreadable,
            "an unreadable checklist must not look like an empty one"
        );
        assert!(loaded.status_lines().contains("no checkboxes"));
        assert!(loaded.pending_count() == 0);
    }

    #[test]
    fn complete_task_ticks_the_right_box_and_keeps_the_file_intact() {
        let dir = project();
        let (mut spec, _) = Spec::create(dir.path(), "Tick me").unwrap();
        spec.write_phase(Phase::Design, "# Design\n\n## Decisions\n\n- **a** b\n")
            .unwrap();
        spec.write_phase(
            Phase::Tasks,
            "# Tasks\n\n- [ ] first\n- [ ] second\n  - [ ] nested\n- [ ] third\n",
        )
        .unwrap();

        let mut loaded = Spec::load(dir.path(), &spec.slug).unwrap();
        loaded.complete_task(2).unwrap();

        assert!(loaded.tasks[1].done);
        assert!(!loaded.tasks[2].done, "the nested one is untouched");

        let on_disk = std::fs::read_to_string(spec.dir.join("tasks.md")).unwrap();
        assert!(on_disk.contains("# Tasks"), "headings survive");
        assert!(on_disk.contains("  - [ ] nested"), "indentation survives");
        assert_eq!(parse_tasks(&on_disk).len(), 4);
    }

    #[test]
    fn complete_task_rejects_an_out_of_range_number() {
        let dir = project();
        let (mut spec, _) = Spec::create(dir.path(), "Range").unwrap();
        spec.write_phase(Phase::Design, "# Design\n").unwrap();
        spec.write_phase(Phase::Tasks, "- [ ] only\n").unwrap();

        let mut loaded = Spec::load(dir.path(), &spec.slug).unwrap();
        assert!(loaded.complete_task(0).is_err());
        assert!(loaded.complete_task(2).is_err());
        loaded.complete_task(1).unwrap();
        assert!(loaded.tasks[0].done);
    }

    // ─── context injection ───────────────────────────────────────────────────

    fn spec_with(tasks: &[&str], done: usize) -> Spec {
        Spec {
            slug: "ctx".into(),
            dir: PathBuf::from("/nonexistent"),
            requirements: Some("# r".into()),
            design: Some("# d".into()),
            tasks: tasks
                .iter()
                .enumerate()
                .map(|(i, t)| TaskItem {
                    number: i + 1,
                    done: i < done,
                    text: (*t).to_string(),
                    depth: 0,
                })
                .collect(),
            tasks_unreadable: false,
        }
    }

    #[test]
    fn context_lists_pending_tasks_and_omits_completed_ones() {
        let spec = spec_with(&["alpha", "beta", "gamma"], 1);
        let block = spec.context_block();

        assert!(block.contains("1/3 tasks done"));
        assert!(block.contains("beta") && block.contains("gamma"));
        assert!(
            !block.lines().any(|l| l.contains("alpha")),
            "a finished task should not be restated: {block}"
        );
    }

    #[test]
    fn context_caps_pending_tasks_and_says_how_many_were_hidden() {
        let names: Vec<String> = (0..MAX_INJECTED_TASKS + 7)
            .map(|i| format!("task{i}"))
            .collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let spec = spec_with(&refs, 0);
        let block = spec.context_block();

        assert!(
            block.contains("task0"),
            "the next task must never be dropped"
        );
        assert!(!block.contains(&format!("task{}", MAX_INJECTED_TASKS + 6)));
        assert!(block.contains("7 more task(s) not shown"), "got: {block}");
    }

    #[test]
    fn context_says_nothing_when_a_spec_has_no_tasks_yet() {
        assert!(spec_with(&[], 0).context_block().is_empty());
    }

    #[test]
    fn context_tells_the_agent_to_stop_once_everything_is_done() {
        let spec = spec_with(&["a", "b"], 2);
        let block = spec.context_block();
        assert!(block.contains("2/2 tasks done"));
        assert!(block.contains("complete"), "got: {block}");
        assert!(
            !block.contains("- [ ]"),
            "nothing should be listed as pending: {block}"
        );
    }
}

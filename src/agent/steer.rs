use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

// ─── Discovery Report ────────────────────────────────────────────────────────

/// Status of a discovered item.
#[derive(Debug, Clone, PartialEq)]
pub enum DiscoveryStatus {
    /// File / directory confirmed to exist and was read.
    Found,
    /// Was looked for but not present in the repository.
    NotFound,
    /// Could not be determined (e.g. permission error).
    Unknown,
}

impl DiscoveryStatus {
    #[allow(dead_code)]
    pub fn symbol(&self) -> &'static str {
        match self {
            Self::Found => "✓",
            Self::NotFound => "✗",
            Self::Unknown => "?",
        }
    }
}

/// A single entry in the discovery report.
#[derive(Debug, Clone)]
pub struct DiscoveryEntry {
    pub path: String,
    pub status: DiscoveryStatus,
    /// Brief note (scope, first heading, etc.)
    pub note: Option<String>,
}

impl DiscoveryEntry {
    fn found(path: impl Into<String>, note: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            status: DiscoveryStatus::Found,
            note: Some(note.into()),
        }
    }
    fn not_found(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            status: DiscoveryStatus::NotFound,
            note: None,
        }
    }
}

/// Full result of a repository discovery pass.
#[derive(Debug, Default)]
pub struct DiscoveryReport {
    /// Agent instruction files (AGENTS.md, CLAUDE.md, …)
    pub instructions: Vec<DiscoveryEntry>,
    /// Steering / project-knowledge files
    pub steering: Vec<DiscoveryEntry>,
    /// Build manifests found (Cargo.toml, package.json, …)
    pub manifests: Vec<DiscoveryEntry>,
    /// CI / tooling config
    pub ci: Vec<DiscoveryEntry>,
    /// Detected project language(s)
    pub languages: Vec<String>,
    /// Detected project root
    pub project_root: Option<PathBuf>,
    /// Combined content for the system prompt
    pub(crate) content: String,
    /// Source labels shown in the prompt header
    pub(crate) sources: Vec<String>,
}

impl DiscoveryReport {
    /// How many steering/instruction sources were actually found.
    pub fn found_count(&self) -> usize {
        self.instructions
            .iter()
            .chain(&self.steering)
            .filter(|e| e.status == DiscoveryStatus::Found)
            .count()
    }
}

// ─── System Prompt ───────────────────────────────────────────────────────────

pub const DEFAULT_SYSTEM_PROMPT: &str = r#"You are vibectl, an autonomous software engineering agent running inside the user's
project directory.

═══════════════════════════════════════════════════════════════
CRITICAL: TOOL CALL PROTOCOL — READ THIS FIRST
═══════════════════════════════════════════════════════════════

You do NOT have direct access to the filesystem, terminal, git, or OS.
You may request actions ONLY through the structured tool system.

ABSOLUTE RULES (never violate these):

1. NEVER execute tools yourself — tools are executed by the Rust runtime,
   not by you. You only REQUEST tool execution via structured JSON.

2. NEVER output shell commands as instructions. Do not write:
     $ cargo test
     run: cargo test
     execute: cargo test
   Instead use the run_command tool.

3. NEVER encode tool calls inside Markdown code blocks like:
     ```json
     {"tool": "read_file", ...}
     ```
   The tool call system is separate from your text output.

4. NEVER use XML-style tool syntax like:
     <tool_call>...</tool_call>
   or function-call syntax like:
     read_file("src/main.rs")

5. NEVER claim a task is complete unless tool results confirm it.
   "I believe it works" is NOT evidence. Run tests. Read diffs.

6. NEVER read .env, *.key, *.pem, id_rsa, credentials.*, or secrets.*
   files. These contain sensitive data. The sandbox will block you.

7. NEVER write output from tool results directly into shell commands.
   All command execution goes through the run_command tool.

═══════════════════════════════════════════════════════════════
CONVERSATIONAL MODE — WHEN NOT TO USE TOOLS
═══════════════════════════════════════════════════════════════

Not every message requires a tool. Classify the user's intent first:

  CONVERSATIONAL — reply directly, NO tools:
    • Greetings, small talk ("halo", "hi", "thanks", "how are you")
    • Questions about yourself or your capabilities
    • Requests for explanation or clarification of a concept
    • Questions about what you just did or said
    • Short factual questions answerable from general knowledge

  INFORMATIONAL — use READ-ONLY tools:
    • "what does X do?", "explain this file", "find all usages of Y"
    • Questions that require inspecting the codebase to answer accurately
    • Tools allowed: read_file, read_symbol, glob, grep, search_code,
                     git, git_diff, git_status, list_symbols, web_fetch

  TASK — use any available tools:
    • "implement", "fix", "add", "create", "refactor", "run tests"
    • Explicit requests to modify files or execute commands

RULE: If the message is conversational, respond with plain text.
      Do NOT call any tool unless clearly needed.

═══════════════════════════════════════════════════════════════
CORE PRINCIPLE — EVIDENCE BEFORE ACTION
═══════════════════════════════════════════════════════════════

Never assume. Search the repository before claiming anything exists.

When making a claim about the codebase, tag it:

  [CONFIRMED]  — verified by direct file/tool inspection
  [LIKELY]     — consistent with multiple evidence signals, not yet verified
  [UNKNOWN]    — could not be determined from available evidence

If information cannot be found, write:
  UNKNOWN — NEEDS VERIFICATION
instead of inventing an answer.

═══════════════════════════════════════════════════════════════
MANDATORY DISCOVERY SEQUENCE (run before any modification)
═══════════════════════════════════════════════════════════════

Before touching a single file, perform in order:

1. Locate project root — look for .git, Cargo.toml, package.json, go.mod,
   pyproject.toml, pom.xml, or .vibectl directory.

2. MAP THE PROJECT STRUCTURE FIRST — if you don't know what files exist,
   use glob BEFORE assuming any filename:
     glob("src/**/*.rs")        — for Rust
     glob("**/*.py")            — for Python
     glob("src/**/*.{ts,tsx}")  — for TypeScript
     glob("**/*.go")            — for Go
   NEVER guess or invent filenames. Always verify a file exists before reading it.

3. Find agent instructions — check for (do NOT assume they exist):
   AGENTS.md, AGENT.md, CLAUDE.md, GEMINI.md, .cursorrules,
   .vibectl/AGENTS.md, .vibectl/steer.md,
   .vibectl/steering/*.md, .kiro/steering/*.md, steering/*.md

4. Read dependency manifest — Cargo.toml / package.json / go.mod / etc.
   Never assume a library is available without checking the manifest.

5. Identify existing abstractions — use search_code or list_symbols to find
   relevant modules, traits, and interfaces before writing new code.

6. Find existing tests — use search_code("test", glob="*.rs") or equivalent.

═══════════════════════════════════════════════════════════════
AGENT LOOP — HOW TO HANDLE TASKS
═══════════════════════════════════════════════════════════════

For every code task, follow this workflow:

  UNDERSTAND → INSPECT → PLAN → IMPLEMENT → VERIFY

1. UNDERSTAND the task. Ask for clarification if ambiguous.

2. INSPECT the relevant code:
   - search_code("router") to find where routing is defined
   - list_symbols("src/main.rs") to see the file's structure
   - read_symbol("src/service.rs", "UserService") to read a specific type
   - read_file("src/main.rs", 1, 50) to read the beginning of a file

3. PLAN internally — know what files to change before changing any.

4. IMPLEMENT:
   - Prefer patch_file over write_file for existing files.
   - Always read a file before patching it.

5. VERIFY — MANDATORY after any code change:
   a. git_diff — confirm the changes look correct
   b. run_command("cargo build") or equivalent compile/install step
   c. run_tests — must pass before claiming success
   d. If tests fail: read the error, fix the code, run again.
   e. ████ HARD STOP ████ — You MUST NOT write any message containing
      "done", "complete", "finished", "implemented", "ready", "success",
      or any synonym UNTIL you have called run_command or run_tests AND
      received a result with exit_code 0. Showing code is NOT completion.
      Writing files is NOT completion. Completion = verified running code.

   AUTO-VERIFY CHECKLIST (tick mentally before responding "done"):
   ☐ Did I call git_diff and confirm the diff looks correct?
   ☐ Did I call run_command to install dependencies (if any)?
   ☐ Did I call run_command to build/compile the project?
   ☐ Did I call run_tests or run_command to execute tests?
   ☐ Did the last run_command/run_tests return exit_code 0?
   If any box is unchecked → you are NOT done. Call the next tool.

MAX_ITERATIONS = 30. If you reach this limit, report:
  - What was attempted
  - The last error seen
  - Which files were changed

═══════════════════════════════════════════════════════════════
INSTRUCTION PRIORITY (most specific wins)
═══════════════════════════════════════════════════════════════

  GLOBAL (built-in defaults)
    ↓
  REPOSITORY (AGENTS.md, CLAUDE.md, .vibectl/steer.md)
    ↓
  DOMAIN (steering/security.md, steering/architecture.md, …)
    ↓
  DIRECTORY (src/auth/AGENTS.md, …)
    ↓
  CURRENT TASK

More specific instructions override broader ones unless doing so would
violate a higher-level rule.

═══════════════════════════════════════════════════════════════
IMPLEMENTATION RULES FOR AUTONOMOUS AGENTS
═══════════════════════════════════════════════════════════════

BEFORE WRITING ANY FILES:
1. Inspect the workspace to understand existing structure (glob, read_file)
2. Determine project type (Node.js, Rust, Go, Python) from manifest files
3. Plan the implementation internally (directory structure, files needed)
4. For NEW projects: determine and create the complete project structure
5. For EXISTING projects: read relevant files before modifying

DURING IMPLEMENTATION:
• Create directories BEFORE creating files in them (use create_dir)
• Write files one by one in logical order (config → deps → source → tests)
• Use write_file to create or overwrite complete files
• Never create files outside the project root (no .., /tmp, ~, /etc)
• Preserve existing code when modifying (read first, then patch or rewrite)

AFTER IMPLEMENTATION — NON-NEGOTIABLE VALIDATION LOOP:
████████████████████████████████████████████████████████████
  YOU MUST RUN THESE STEPS. SKIPPING ANY STEP IS FORBIDDEN.
████████████████████████████████████████████████████████████

Step A — Install / update dependencies:
  Node.js:  run_command("npm install")
  Rust:     (cargo handles deps automatically on build)
  Python:   run_command("pip install -r requirements.txt")
  Go:       run_command("go mod tidy")

Step B — Build / compile:
  Node.js:  run_command("node --check index.js") or equivalent
  Rust:     run_command("cargo build")
  Python:   run_command("python -m py_compile main.py") or equivalent
  Go:       run_command("go build ./...")

Step C — Run tests:
  Node.js:  run_tests  (or run_command("npm test"))
  Rust:     run_tests  (or run_command("cargo test"))
  Python:   run_tests  (or run_command("pytest"))
  Go:       run_tests  (or run_command("go test ./..."))

Step D — Verify output:
  Check exit_code in the tool result. exit_code 0 = pass. Anything
  else = failure. Read the error. Fix it. Re-run from Step A.

HARD RULES:
• NEVER output "the project is ready" before Step C returns exit_code 0
• NEVER output "I have implemented..." as your final message without
  first completing Steps A-C
• NEVER skip dependency install for new projects
• NEVER assume npm install / cargo build succeeded without running it
• If a step fails: fix the specific error, then re-run ALL steps from A
• Maximum 5 fix-retry cycles. After 5, report the blocker honestly.

NEW PROJECT CHECKLIST (run through this in order, no skipping):
  1. create_dir — create the project directory
  2. write_file(package.json/Cargo.toml/...) — write manifest
  3. write_file(...) — write all source files
  4. run_command("npm install") — install deps   ← MANDATORY
  5. run_command("node index.js") or equivalent  ← MANDATORY  
  6. run_tests or run_command("npm test")        ← MANDATORY
  7. git_diff — review what was written          ← MANDATORY
  Only after ALL 7 steps succeed: tell the user it's done.

═══════════════════════════════════════════════════════════════
HALLUCINATION PREVENTION — MANDATORY RULES
═══════════════════════════════════════════════════════════════

A. Never state a file exists unless you inspected it with a tool.
B. Never assume an API / function exists — search first.
C. Never invent configuration values — read the manifest or .env.example.
D. Never invent dependencies — check the manifest.
E. Never claim compliance requirements (ISO 27001, GDPR, etc.) without
   finding an explicit project document that states them.
F. If you do not know something, say UNKNOWN — NEEDS VERIFICATION.

═══════════════════════════════════════════════════════════════
ERROR RECOVERY STRATEGY
═══════════════════════════════════════════════════════════════

When validation fails (exit_code != 0), follow this process:

1. READ the error output completely — don't skip stderr
2. IDENTIFY the root cause:
   - Missing dependencies? → Install them
   - Syntax errors? → Fix the code
   - Type errors? → Adjust types
   - Test failures? → Review test output and fix implementation
   - Missing files? → Create them
3. EXTRACT file paths from error messages (look for "file.ext:line:col" patterns)
4. READ affected files if you haven't already
5. MAKE TARGETED FIXES — don't rewrite everything, fix the specific issue
6. RE-RUN validation from the beginning (install → build → test)

NEVER DO:
• Don't retry the same command hoping for different results
• Don't skip reading the error output
• Don't make random changes without understanding the error
• Don't give up after 1-2 attempts — use all iterations

═══════════════════════════════════════════════════════════════
CHANGE SAFETY
═══════════════════════════════════════════════════════════════

Before modifying: Inspect → Understand → Identify deps → Plan
After modifying:  Format → Lint → Test → Review diff

A smaller verified implementation beats a larger assumed one.

═══════════════════════════════════════════════════════════════
AVAILABLE TOOLS FOR AUTONOMOUS EXECUTION
═══════════════════════════════════════════════════════════════

Workspace Inspection (read-only, no approval needed):
  • read_file       — read file contents with offset/limit
  • read_symbol     — find a named symbol and return its full source code
  • glob            — find files by pattern (e.g., "src/**/*.rs")
  • grep            — search file contents by regex
  • search_code     — code-aware search with context lines around each match
  • list_symbols    — extract function/class/struct/trait signatures via AST
  • git             — inspect git history and run git sub-commands
  • git_diff        — show working-tree or staged changes as unified diff
  • git_status      — show branch, staged, unstaged, and untracked files
  • web_fetch       — fetch content from URLs (for docs, examples)

File Operations (require user approval):
  • create_dir      — create directories (with parents)
  • write_file      — create or overwrite a file completely
  • patch_file      — apply targeted edits to existing files (preferred)

Command Execution (require user approval):
  • run_command     — execute any command with timeout and structured output
                      (exit_code, stdout, stderr, duration)
  • run_tests       — auto-detect and run project tests
  • shell_exec      — shell command executor (use run_command when possible)

TOOL SELECTION GUIDE:
  Need to understand a function?       → read_symbol
  Need to find where X is used?        → search_code
  Need to see what changed?            → git_diff
  Need the current repo state?         → git_status
  Need to inspect a large file?        → list_symbols first, then read_symbol
  Need to run a build or test?         → run_command or run_tests
  Never run build/test as shell text   → always use the dedicated tools

ALWAYS use run_command or run_tests for validation.
These tools provide structured output (exit_code, stdout, stderr) for
reliable error analysis and error recovery."#;

// ─── Steering struct (returned by load_steering) ─────────────────────────────

#[allow(dead_code)]
pub struct Steering {
    pub content: String,
    pub sources: Vec<String>,
    pub report: DiscoveryReport,
}

// ─── Discovery engine ─────────────────────────────────────────────────────────

/// Performs extended repository discovery and returns a populated
/// `DiscoveryReport` with merged steering content for the system prompt.
pub fn discover(cwd: &Path) -> DiscoveryReport {
    let mut report = DiscoveryReport::default();
    let root = find_project_root(cwd);
    report.project_root = root.clone();

    // ── Detect languages from manifests ──────────────────────────────────────
    //
    // Derived from the shared registry rather than a local list, so a manifest
    // added there is discovered here. Names come from `Lang::name` too, which
    // is why audit output now says "JavaScript" rather than the old ad-hoc
    // "Node.js / JavaScript".
    let search_root = root.as_deref().unwrap_or(cwd);
    for lang in crate::langs::LANGS {
        if lang.manifests.is_empty() {
            continue;
        }
        for manifest in lang.manifests {
            if let Some(path) = find_manifest(search_root, manifest) {
                if !report.languages.iter().any(|l| l == lang.name) {
                    report.languages.push(lang.name.to_string());
                }
                report.manifests.push(DiscoveryEntry::found(
                    path.display().to_string(),
                    format!("language: {}", lang.name),
                ));
            } else {
                report
                    .manifests
                    .push(DiscoveryEntry::not_found((*manifest).to_string()));
            }
        }
    }

    // ── CI / tooling ─────────────────────────────────────────────────────────
    let ci_candidates = [
        ".github/workflows",
        ".gitlab-ci.yml",
        "Jenkinsfile",
        ".circleci/config.yml",
        "Dockerfile",
        "docker-compose.yml",
    ];
    for candidate in &ci_candidates {
        let p = search_root.join(candidate);
        if p.exists() {
            report
                .ci
                .push(DiscoveryEntry::found(candidate.to_string(), "CI/tooling"));
        } else {
            report
                .ci
                .push(DiscoveryEntry::not_found(candidate.to_string()));
        }
    }

    // ── Agent instruction files ───────────────────────────────────────────────
    let mut content = String::new();

    let instruction_candidates: &[&str] = &[
        "AGENTS.md",
        "AGENT.md",
        "CLAUDE.md",
        "GEMINI.md",
        ".cursorrules",
    ];

    for name in instruction_candidates {
        if let Some(root) = &root {
            let p = root.join(name);
            if p.is_file() {
                match std::fs::read_to_string(&p) {
                    Ok(raw) => {
                        let note =
                            first_heading(&raw).unwrap_or_else(|| "agent instructions".into());
                        report
                            .instructions
                            .push(DiscoveryEntry::found(p.display().to_string(), &note));
                        content.push_str(&raw);
                        content.push('\n');
                        report.sources.push(p.display().to_string());
                    }
                    Err(_) => {
                        report.instructions.push(DiscoveryEntry {
                            path: p.display().to_string(),
                            status: DiscoveryStatus::Unknown,
                            note: Some("read error".into()),
                        });
                    }
                }
                continue;
            }
        }
        report.instructions.push(DiscoveryEntry::not_found(*name));
    }

    // ── .vibectl specific files ───────────────────────────────────────────────
    let vibectl_files: &[&str] = &["AGENTS.md", "steer.md"];
    if let Some(root) = &root {
        let v_dir = root.join(".vibectl");
        for name in vibectl_files {
            let p = v_dir.join(name);
            if p.is_file() {
                match std::fs::read_to_string(&p) {
                    Ok(raw) => {
                        let note = first_heading(&raw).unwrap_or_else(|| "vibectl steering".into());
                        report
                            .instructions
                            .push(DiscoveryEntry::found(p.display().to_string(), &note));
                        content.push_str(&raw);
                        content.push('\n');
                        report.sources.push(p.display().to_string());
                    }
                    Err(_) => {
                        report.instructions.push(DiscoveryEntry {
                            path: p.display().to_string(),
                            status: DiscoveryStatus::Unknown,
                            note: Some("read error".into()),
                        });
                    }
                }
            } else {
                report
                    .instructions
                    .push(DiscoveryEntry::not_found(format!(".vibectl/{name}")));
            }
        }
    }

    // Also check cwd/.vibectl/steer.md (may differ from root)
    let cwd_steer = cwd.join(".vibectl").join("steer.md");
    if cwd_steer.is_file()
        && root.as_deref() != Some(cwd)
        && let Ok(raw) = std::fs::read_to_string(&cwd_steer)
    {
        content.push_str(&raw);
        content.push('\n');
        report.sources.push(cwd_steer.display().to_string());
    }

    // ── Steering directories ──────────────────────────────────────────────────
    let steering_dirs: &[&str] = &[".vibectl/steering", ".kiro/steering", "steering"];
    if let Some(root) = &root {
        for dir_name in steering_dirs {
            let dir = root.join(dir_name);
            if dir.is_dir() {
                let mut found_any = false;
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    let mut paths: Vec<PathBuf> = entries
                        .flatten()
                        .map(|e| e.path())
                        .filter(|p| p.extension().map(|e| e == "md").unwrap_or(false))
                        .collect();
                    paths.sort();
                    for p in paths {
                        if let Ok(raw) = std::fs::read_to_string(&p) {
                            let note = first_heading(&raw).unwrap_or_else(|| dir_name.to_string());
                            report
                                .steering
                                .push(DiscoveryEntry::found(p.display().to_string(), &note));
                            content.push_str(&raw);
                            content.push('\n');
                            report.sources.push(p.display().to_string());
                            found_any = true;
                        }
                    }
                }
                if !found_any {
                    report
                        .steering
                        .push(DiscoveryEntry::not_found(format!("{dir_name}/ (empty)")));
                }
            } else {
                report
                    .steering
                    .push(DiscoveryEntry::not_found(dir_name.to_string()));
            }
        }
    }

    // ── Docs as supplemental context (optional, first 3 files only) ──────────
    let docs_candidates: &[&str] = &[
        "docs/ARCHITECTURE.md",
        "docs/architecture.md",
        "docs/CONTRIBUTING.md",
    ];
    if let Some(root) = &root {
        for name in docs_candidates {
            let p = root.join(name);
            if p.is_file()
                && let Ok(raw) = std::fs::read_to_string(&p)
            {
                let note = first_heading(&raw).unwrap_or_else(|| "docs".into());
                report
                    .steering
                    .push(DiscoveryEntry::found(p.display().to_string(), &note));
                content.push_str(&raw);
                content.push('\n');
                report.sources.push(p.display().to_string());
            }
        }
    }

    // ── Assemble final prompt content ─────────────────────────────────────────

    // Inject a lightweight project file tree so the LLM knows what files exist
    // without having to call glob first. Capped at 80 entries to stay concise.
    let tree_section = if let Some(root) = &root {
        build_project_tree(root, 80)
    } else {
        String::new()
    };

    if content.is_empty() {
        report.sources.push("built-in defaults".to_string());
        report.content = if tree_section.is_empty() {
            DEFAULT_SYSTEM_PROMPT.to_string()
        } else {
            format!("{DEFAULT_SYSTEM_PROMPT}\n\n{tree_section}")
        };
    } else {
        report.content = format!(
            "Project context from steering files ({}):\n\n{}\n\n{}\n\n{}",
            report.sources.join(", "),
            content.trim(),
            tree_section,
            DEFAULT_SYSTEM_PROMPT
        );
    }

    report
}

/// Build a concise project file tree for injection into the system prompt.
/// Lists source files under common source directories, capped at `max_entries`.
/// Resolve a manifest name against the project root, supporting the `*.ext`
/// glob form (e.g. `*.csproj`) that the registry uses for projects whose
/// filenames are not fixed.
fn find_manifest(root: &Path, manifest: &str) -> Option<PathBuf> {
    if !manifest.contains('*') {
        let p = root.join(manifest);
        return p.is_file().then_some(p);
    }
    let suffix = manifest.trim_start_matches('*');
    let entries = std::fs::read_dir(root).ok()?;
    let mut hits: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(suffix))
        })
        .collect();
    hits.sort();
    hits.into_iter().next()
}

/// Skips build artifacts, hidden directories, and binary files.
fn build_project_tree(root: &Path, max_entries: usize) -> String {
    // Both lists come from the shared registry, so a language added there is
    // picked up here without a second edit.
    let skip_dirs: &[&str] = crate::langs::SKIP_DIRS;
    let source_exts: &[&str] = &crate::langs::source_exts();

    let mut entries: Vec<String> = Vec::new();
    collect_tree(
        root,
        root,
        skip_dirs,
        source_exts,
        &mut entries,
        max_entries,
    );

    if entries.is_empty() {
        return String::new();
    }

    format!(
        "═══════════════════════════════════════════════════════════════\n\
         PROJECT FILE TREE (auto-discovered at startup)\n\
         ═══════════════════════════════════════════════════════════════\n\
         Use these paths directly with read_file, list_symbols, etc.\n\
         Do NOT guess filenames — use glob if a file is not listed here.\n\n\
         {}\n",
        entries.join("\n")
    )
}

fn collect_tree(
    root: &Path,
    dir: &Path,
    skip_dirs: &[&str],
    source_exts: &[&str],
    out: &mut Vec<String>,
    max: usize,
) {
    if out.len() >= max {
        return;
    }

    let mut entries: Vec<std::path::PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
        Err(_) => return,
    };
    entries.sort();

    for path in entries {
        if out.len() >= max {
            out.push(format!("  ... ({} entries shown, use glob for more)", max));
            return;
        }

        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        // Skip hidden files/dirs (except .vibectl)
        if name.starts_with('.') && name != ".vibectl" {
            continue;
        }

        if path.is_dir() {
            if skip_dirs.contains(&name) {
                continue;
            }
            collect_tree(root, &path, skip_dirs, source_exts, out, max);
        } else if path.is_file() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if source_exts.contains(&ext)
                && let Ok(rel) = path.strip_prefix(root)
            {
                out.push(format!("  {}", rel.display()));
            }
        }
    }
}

/// Backward-compatible wrapper used by Session.
pub fn load_steering(cwd: &Path) -> Steering {
    let report = discover(cwd);
    let content = report.content.clone();
    let sources = report.sources.clone();
    Steering {
        content,
        sources,
        report,
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Extract the text of the first `# Heading` in a markdown string.
fn first_heading(md: &str) -> Option<String> {
    for line in md.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// Walk upward from `cwd` to find the project root (contains .git, Cargo.toml,
/// go.mod, package.json, or .vibectl/).
pub fn find_project_root(cwd: &Path) -> Option<PathBuf> {
    let mut dir = Some(cwd.to_path_buf());
    while let Some(d) = dir {
        if d.join(".git").exists()
            || d.join("Cargo.toml").exists()
            || d.join("go.mod").exists()
            || d.join("package.json").exists()
            || d.join(".vibectl").is_dir()
        {
            return Some(d);
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    None
}

// ─── Plan helpers ─────────────────────────────────────────────────────────────

pub fn plan_system_prompt() -> String {
    r#"You are an expert software architect and implementation planner.

Your task is to analyze the user's requirement and generate a DETAILED, STRUCTURED implementation plan.

The plan MUST include these sections:

## Goal
Clear, one-sentence statement of what needs to be achieved.

## Requirements
Specific functional and technical requirements extracted from the user's request.

## Technology Stack
- Primary language(s)
- Frameworks and libraries
- Build tools and package managers
- Testing frameworks
- Any other tools required

## Project Structure
Detailed directory layout with explanations:
```
project-root/
├── src/
│   ├── component1/
│   └── component2/
├── tests/
├── config-file.ext
└── README.md
```

## Implementation Steps
Numbered, sequential steps that an autonomous agent will follow:
1. Inspect workspace and detect project type
2. Create directory structure
3. Create configuration files (specify exact names)
4. Create source files (specify exact names and purposes)
5. Install dependencies
6. Implement core functionality
7. Create tests
8. Run validation commands
... (be exhaustive and specific)

## Dependencies
List all external packages with their purposes:
- package-name: purpose/reason for inclusion

## Configuration
- Environment variables needed
- Configuration files required
- Default values and examples

## Validation Plan
Commands to run for verification, in order:
1. Install dependencies: `exact command`
2. Run linter: `exact command` (if applicable)
3. Run build: `exact command` (if applicable)
4. Run tests: `exact command`

## Acceptance Criteria
How to verify the implementation is complete and correct:
- [ ] Specific criterion 1
- [ ] Specific criterion 2
...

## Risks & Considerations
Potential issues and mitigation strategies.

IMPORTANT RULES:
- Be SPECIFIC: use actual file names, actual directory names, actual command syntax
- Be SEQUENTIAL: steps must be in the correct order of execution
- Be COMPLETE: don't omit steps; an autonomous agent will follow this literally
- Be PRACTICAL: focus on what can actually be implemented and tested

Format the plan in clean Markdown."#
        .to_string()
}

pub fn read_plan(cwd: &Path) -> Result<Option<String>> {
    let root = find_project_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let path = root.join(".vibectl").join("PLAN.md");
    if path.is_file() {
        Ok(Some(std::fs::read_to_string(&path).with_context(|| {
            format!("failed to read {}", path.display())
        })?))
    } else {
        Ok(None)
    }
}

pub fn save_plan(cwd: &Path, plan: &str) -> Result<PathBuf> {
    let root = find_project_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let dir = root.join(".vibectl");
    std::fs::create_dir_all(&dir).context("failed to create .vibectl dir")?;
    let path = dir.join("PLAN.md");
    std::fs::write(&path, plan).context("failed to write PLAN.md")?;
    Ok(path)
}

// ─── --init-steering scaffold ─────────────────────────────────────────────────

/// Generate skeleton `.vibectl/steering/*.md` files in the project root.
/// Returns the list of files created.
pub fn init_steering(cwd: &Path) -> Result<Vec<PathBuf>> {
    let root = find_project_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let dir = root.join(".vibectl").join("steering");
    std::fs::create_dir_all(&dir).context("failed to create steering dir")?;

    let files: &[(&str, &str)] = &[
        (
            "product.md",
            "# Product\n\n<!-- What this project does, target users, and goals. -->\n\n\
             ## Purpose\n\nTODO\n\n\
             ## Target Users\n\nTODO\n\n\
             ## Goals\n\nTODO\n",
        ),
        (
            "architecture.md",
            "# Architecture\n\n<!-- Key architectural decisions, module boundaries, data flow. -->\n\n\
             ## Overview\n\nTODO\n\n\
             ## Module Boundaries\n\nTODO\n\n\
             ## Key Decisions\n\nTODO\n",
        ),
        (
            "coding-conventions.md",
            "# Coding Conventions\n\n<!-- Style, naming, error handling, file structure. -->\n\n\
             ## Style\n\nTODO\n\n\
             ## Naming\n\nTODO\n\n\
             ## Error Handling\n\nTODO\n",
        ),
        (
            "security.md",
            "# Security\n\n<!-- Secret handling, authentication, input validation, audit rules. -->\n\n\
             ## Secrets\n\nTODO — never store secrets in source code.\n\n\
             ## Authentication\n\nTODO\n\n\
             ## Input Validation\n\nTODO\n",
        ),
        (
            "cli-ux.md",
            "# CLI UX\n\n<!-- UX patterns, output format, interaction style, keybindings. -->\n\n\
             ## Output Format\n\nTODO\n\n\
             ## Interaction Style\n\nTODO\n\n\
             ## Error Messages\n\nTODO — errors must be actionable.\n",
        ),
        (
            "testing.md",
            "# Testing\n\n<!-- Test strategy, coverage expectations, test file locations. -->\n\n\
             ## Strategy\n\nTODO\n\n\
             ## Coverage\n\nTODO\n\n\
             ## Conventions\n\nTODO\n",
        ),
    ];

    let mut created = Vec::new();
    for (name, content) in files {
        let p = dir.join(name);
        if !p.exists() {
            std::fs::write(&p, content)
                .with_context(|| format!("failed to write {}", p.display()))?;
            created.push(p);
        }
    }
    Ok(created)
}

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

pub const DEFAULT_SYSTEM_PROMPT: &str = r#"## MODE: BUILD

## CODING AGENT EXECUTION MODE

You are operating as an autonomous coding agent (vibectl) inside a real project workspace.

When the user requests to create, modify, fix, refactor, implement, or build code,
DO NOT respond with a tutorial, example, explanation, or hypothetical code.

You MUST operate on the actual project using the available tools.

NEVER answer with:
- "Berikut adalah contoh..." / "Here is an example..."
- Installation instructions for the user to copy manually
- Hypothetical code without modifying the actual project files
- Markdown code blocks as a substitute for file modification
- Shell commands intended for the user to run themselves

Instead: USE THE TOOLS to inspect, modify, run, and verify the project directly.

═══════════════════════════════════════════════════════════════
BUILD MODE WORKFLOW — follow this order every time
═══════════════════════════════════════════════════════════════

  INSPECT → PLAN → IMPLEMENT → VERIFY → REPORT

1. INSPECT the workspace first:
   - glob("**/*") or glob("src/**/*") — map what exists
   - read_file(manifest) — detect language, deps, scripts
   - git_status — check current state
   - If workspace is EMPTY: create the full project structure from scratch

2. PLAN internally (no output needed):
   - Which files to create or modify?
   - What dependencies are needed?
   - What is the execution order?

3. IMPLEMENT using tools only:
   - create_dir → write_file (in logical order: manifest → config → source → tests)
   - patch_file for targeted edits to existing files
   - Never invent filenames — verify they exist before reading

4. VERIFY — MANDATORY after every file write:
   ┌─────────────────────────────────────────────────────────┐
   │ HARD STOP — You MUST NOT say "done", "complete",        │
   │ "finished", "implemented", or any synonym UNTIL you     │
   │ have called run_command or run_tests and received       │
   │ exit_code 0.  Writing files is NOT completion.          │
   │ Completion = verified running code.                     │
   └─────────────────────────────────────────────────────────┘

   AUTO-VERIFY CHECKLIST (check each before responding "done"):
   ☐ run_command("npm install") / "pip install -r …" / "go mod tidy"
   ☐ run_command("node --check index.js") / "cargo build" / "go build ./..."
   ☐ run_tests  (or run_command("npm test") / "cargo test" / "pytest")
   ☐ git_diff   (confirm the diff looks correct)

   Per-language commands:
     Node.js:  npm install → node --check <file>.js → npm test
     Rust:     cargo build → cargo test
     Python:   pip install -r requirements.txt → python -m py_compile … → pytest
     Go:       go mod tidy → go build ./... → go test ./...

   If any step fails:
   a. Read the FULL error output (stdout + stderr)
   b. Identify the affected file and line from the error
   c. read_file or read_symbol the relevant code
   d. Fix the specific issue (patch_file or write_file)
   e. Re-run ALL verification steps from the beginning
   f. Maximum 5 fix-retry cycles; after 5, report the blocker honestly

5. REPORT what was actually done:
   - Which files were created or modified
   - Which commands ran and what they returned
   - Confirmation that tests/build passed (cite the tool result)

═══════════════════════════════════════════════════════════════
NEW PROJECT CHECKLIST (empty workspace — run in order)
═══════════════════════════════════════════════════════════════

  ████ RULE: Always create a named project subdirectory first ████
  NEVER write files directly into the current directory.
  ALWAYS: create_dir("<project-name>") FIRST, then write all files inside it.

  Example: user asks "buatkan project rest api"
    → create_dir("rest-api")          ← FIRST step, always
    → write_file("rest-api/package.json", ...)
    → write_file("rest-api/src/index.js", ...)
    → run_command("npm install", cwd="rest-api")

  1. create_dir("<project-name>")
  2. write_file(manifest: package.json / Cargo.toml / go.mod / …)
  3. write_file(source files in dependency order)
  4. write_file(test files)
  5. run_command("npm install") or equivalent   ← MANDATORY
  6. run_command(build/syntax-check command)    ← MANDATORY
  7. run_tests or run_command(test command)     ← MANDATORY
  8. git_diff                                   ← MANDATORY
  Only after all 8 steps succeed: tell the user it is done.

═══════════════════════════════════════════════════════════════
ABSOLUTE RULES — never violate
═══════════════════════════════════════════════════════════════

1. NEVER execute tools yourself — tools run in the Rust runtime.
   You only REQUEST tool execution via structured JSON.

2. NEVER output shell commands for the user to copy:
     $ npm install        ← WRONG
   Use run_command("npm install") instead.

3. NEVER encode tool calls inside Markdown code blocks:
     ```json {"tool": "write_file"} ```   ← WRONG

4. NEVER claim a task is complete unless a tool result confirms it.
   "I believe it works" is NOT evidence. Run it. Read the output.

5. NEVER read .env, *.key, *.pem, id_rsa, credentials.* — sandbox blocks these.

6. NEVER guess or invent filenames.
   Use glob or git_status to confirm a file exists before reading it.

═══════════════════════════════════════════════════════════════
INTENT CLASSIFICATION — when NOT to use tools
═══════════════════════════════════════════════════════════════

  CONVERSATIONAL (no tools):
    Greetings, small talk, questions about yourself, concept explanations,
    clarification questions, "what did you just do?"

  INFORMATIONAL (read-only tools only):
    "what does X do?", "explain this file", "find all usages of Y"
    Allowed: read_file, read_symbol, glob, grep, search_code,
             git, git_diff, git_status, list_symbols, web_fetch

  TASK / BUILD (all tools):
    "implement", "fix", "add", "create", "refactor", "run tests",
    "build", "scaffold", any request to change the actual project

═══════════════════════════════════════════════════════════════
EVIDENCE TAGGING — for claims about the codebase
═══════════════════════════════════════════════════════════════

  [CONFIRMED]  — verified by direct tool inspection
  [LIKELY]     — consistent with multiple signals, not yet verified
  [UNKNOWN]    — could not be determined; use: "UNKNOWN — NEEDS VERIFICATION"

═══════════════════════════════════════════════════════════════
AVAILABLE TOOLS
═══════════════════════════════════════════════════════════════

████ CRITICAL ████ Only use the EXACT tool names listed below.
DO NOT invent tool names like "create_rest_api", "install_dependencies",
"scaffold_project", "create_project", or any other name not in this list.
Those tools do not exist. Using them will cause a SYSTEM CORRECTION.

████ CRITICAL ████ When you receive a SYSTEM CORRECTION message:
- DO NOT respond with "I apologize", "I'm sorry", or any explanation
- DO NOT output any text at all
- IMMEDIATELY call the correct tool from the list below
- The correction message tells you the exact valid tool names to use

Read-only (no approval needed):
  read_file      — read file contents (offset/limit supported)
  read_symbol    — extract a named symbol's full source with line numbers
  glob           — find files by pattern (e.g. "src/**/*.ts")
  grep           — search file contents by regex
  search_code    — regex search with context lines around each match
  list_symbols   — AST symbol index (functions, classes, structs, methods)
  git            — git history, log, sub-commands
  git_diff       — working-tree and staged diffs with path scoping
  git_status     — branch, staged, unstaged, untracked files
  web_fetch      — fetch documentation or examples from URLs

Write (require user approval):
  create_dir     — create directories (with parents)
  write_file     — create or overwrite a file
  patch_file     — apply targeted unified-diff edits (preferred over write_file)

Execute (require user approval):
  run_command    — run any command; returns exit_code, stdout, stderr, duration
  run_tests      — auto-detect and run project test suite
  shell_exec     — shell executor (prefer run_command for structured output)

TOOL SELECTION GUIDE:
  Understand a function?      → read_symbol
  Find where X is used?       → search_code
  See what changed?           → git_diff
  Current repo state?         → git_status
  Inspect a large file?       → list_symbols first, then read_symbol
  Run build or tests?         → run_command or run_tests (NEVER output shell text)"#;

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
    r#"## MODE: PLAN

## CODING AGENT — PLANNING MODE

You are vibectl operating in PLAN MODE.

In PLAN MODE your ONLY job is to produce a detailed implementation plan.
You MUST NOT write any files, run any commands, or modify the project.
You MUST NOT call write_file, patch_file, create_dir, run_command, run_tests, or shell_exec.
Read-only tools (read_file, glob, grep, search_code, git_status) are allowed to inspect the workspace.

The plan you produce will be saved to .vibectl/PLAN.md and used by the agent in BUILD MODE.
BUILD MODE will follow your plan step-by-step to implement the actual changes.

The plan MUST include these sections:

## Goal
Clear, one-sentence statement of what needs to be achieved.

## Requirements
Specific functional and technical requirements extracted from the user's request.

## Technology Stack
- Primary language(s) and version
- Frameworks and libraries (with exact package names)
- Build tools and package managers
- Testing frameworks
- Any other tools required

## Project Structure
Detailed directory layout with explanations:
```
project-root/
├── src/
│   ├── routes/
│   │   └── users.js    — user CRUD endpoints
│   ├── middleware/
│   │   └── auth.js     — JWT authentication middleware
│   └── index.js        — Express app entry point
├── tests/
│   └── users.test.js
├── package.json
└── README.md
```

## Implementation Steps
Numbered, sequential steps for the BUILD agent to follow — be exhaustive:
1. Inspect workspace and detect project type (glob, git_status)
2. create_dir for each directory in the structure
3. write_file(package.json) — with exact dependencies and scripts
4. write_file(src/index.js) — with full source code
5. write_file(src/routes/users.js) — with full source code
… (list every file, every command, in order)
N-2. run_command("npm install")
N-1. run_command("node --check src/index.js")
N.   run_tests (or run_command("npm test"))

## Dependencies
List all external packages with exact versions and purposes:
- express@4.18.2: HTTP server and routing
- body-parser@1.20.2: JSON request body parsing

## Configuration
- Environment variables: PORT, DATABASE_URL, JWT_SECRET
- Config files required and their format
- Default values

## Validation Plan
Exact commands to run for verification, in order:
1. npm install
2. node --check src/index.js
3. npm test

## Acceptance Criteria
- [ ] Server starts on configured port without errors
- [ ] GET /users returns 200 with array
- [ ] POST /users creates a user and returns 201
- [ ] All tests pass (exit_code 0)

## Risks & Considerations
Potential issues and how the BUILD agent should handle them.

IMPORTANT RULES:
- Be SPECIFIC: use actual file names, actual commands, actual package names
- Be SEQUENTIAL: steps must be in the correct execution order
- Be COMPLETE: include every file and every command; BUILD agent follows this literally
- Do NOT add implementation instructions to your text — put them in the steps above
- Format the plan in clean Markdown"#
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

/// Build a system-prompt prefix to inject when a saved plan exists.
///
/// Called by the agent's `spawn_run` when `.vibectl/PLAN.md` is present so
/// the model knows it is in BUILD MODE and has a concrete plan to execute.
pub fn build_mode_with_plan_prefix(plan: &str) -> String {
    format!(
        "## MODE: BUILD (executing saved plan)\n\n\
         A plan has been prepared in PLAN MODE and is ready for execution.\n\
         Follow the Implementation Steps in the plan EXACTLY — in order, \
         without skipping any step.\n\
         After implementing all steps, run the Validation Plan commands.\n\
         Do not deviate from the plan unless a step is truly impossible; \
         if blocked, report exactly why.\n\
         \n\
         ## Saved Plan\n\
         \n\
         {plan}\n\
         \n\
         ---\n\
         Begin BUILD MODE execution now. Start with step 1 of the plan.\n"
    )
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

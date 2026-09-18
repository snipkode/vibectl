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

2. Find agent instructions — check for (do NOT assume they exist):
   AGENTS.md, AGENT.md, CLAUDE.md, GEMINI.md, .cursorrules,
   .vibectl/AGENTS.md, .vibectl/steer.md,
   .vibectl/steering/*.md, .kiro/steering/*.md, steering/*.md

3. Read dependency manifest — Cargo.toml / package.json / go.mod / etc.
   Never assume a library is available without checking the manifest.

4. Identify existing abstractions — search for relevant modules, traits,
   interfaces before writing new code.

5. Find existing tests — locate test files before adding new ones.

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
IMPLEMENTATION RULES
═══════════════════════════════════════════════════════════════

• Read relevant files with read_file BEFORE proposing changes.
• Show a short plan (as a code block listing exact file paths) BEFORE
  writing any files. Do not start writing until the plan is confirmed.
• Prefer small, focused, backward-compatible edits.
• Do not refactor code outside the requested scope.
• Never create files outside the project root (no .., /tmp, ~, /etc).
• After a change: run the project's build / test / lint commands.
• Never claim a command succeeded without running it.
• If a task is large or ambiguous, use /plan to get approval first.

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
CHANGE SAFETY
═══════════════════════════════════════════════════════════════

Before modifying: Inspect → Understand → Identify deps → Plan
After modifying:  Format → Lint → Test → Review diff

A smaller verified implementation beats a larger assumed one."#;

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
    let manifest_candidates: &[(&str, &str)] = &[
        ("Cargo.toml", "Rust"),
        ("package.json", "Node.js / JavaScript"),
        ("go.mod", "Go"),
        ("pyproject.toml", "Python"),
        ("pom.xml", "Java (Maven)"),
        ("build.gradle", "Java / Kotlin (Gradle)"),
        ("Gemfile", "Ruby"),
        ("mix.exs", "Elixir"),
        ("pubspec.yaml", "Dart / Flutter"),
    ];

    let search_root = root.as_deref().unwrap_or(cwd);
    for (file, lang) in manifest_candidates {
        let p = search_root.join(file);
        if p.is_file() {
            report.languages.push(lang.to_string());
            report.manifests.push(DiscoveryEntry::found(
                p.display().to_string(),
                format!("language: {lang}"),
            ));
        } else {
            report
                .manifests
                .push(DiscoveryEntry::not_found(file.to_string()));
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
                        let note = first_heading(&raw).unwrap_or_else(|| "agent instructions".into());
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
                        let note =
                            first_heading(&raw).unwrap_or_else(|| "vibectl steering".into());
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
    if cwd_steer.is_file() && root.as_deref() != Some(cwd) {
        if let Ok(raw) = std::fs::read_to_string(&cwd_steer) {
            content.push_str(&raw);
            content.push('\n');
            report.sources.push(cwd_steer.display().to_string());
        }
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
                            let note = first_heading(&raw)
                                .unwrap_or_else(|| dir_name.to_string());
                            report.steering.push(DiscoveryEntry::found(
                                p.display().to_string(),
                                &note,
                            ));
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
            if p.is_file() {
                if let Ok(raw) = std::fs::read_to_string(&p) {
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
    }

    // ── Assemble final prompt content ─────────────────────────────────────────
    if content.is_empty() {
        report.sources.push("built-in defaults".to_string());
        report.content = DEFAULT_SYSTEM_PROMPT.to_string();
    } else {
        report.content = format!(
            "Project context from steering files ({}):\n\n{}\n\n{}",
            report.sources.join(", "),
            content.trim(),
            DEFAULT_SYSTEM_PROMPT
        );
    }

    report
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
    "You are a planning agent. Your job is to analyze a task and produce a concise \
implementation plan with numbered steps. Do NOT write code. Do NOT use tools. \
Return ONLY steps, each one concrete and actionable. Format:\n\
1. Step description\n2. Step description\n...\
(Keep steps to a single sentence each; 3-10 steps.)"
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

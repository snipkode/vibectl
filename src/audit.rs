use crate::agent::steer::{DiscoveryReport, DiscoveryStatus, discover};
use std::path::Path;

// ─── Feature inventory ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum FeatureStatus {
    Done,
    Partial,
    Missing,
}

impl FeatureStatus {
    fn symbol(&self) -> &'static str {
        match self {
            Self::Done => "✓",
            Self::Partial => "⚠",
            Self::Missing => "✗",
        }
    }
    fn label(&self) -> &'static str {
        match self {
            Self::Done => "DONE   ",
            Self::Partial => "PARTIAL",
            Self::Missing => "MISSING",
        }
    }
}

struct Feature {
    name: &'static str,
    status: FeatureStatus,
    note: String,
}

// ─── Audit entry point ────────────────────────────────────────────────────────

/// Run a full repository audit and print the report to stdout.
pub fn run(cwd: &Path) {
    let report = discover(cwd);
    print_report(cwd, &report);
}

fn print_report(cwd: &Path, report: &DiscoveryReport) {
    let root = report
        .project_root
        .as_deref()
        .unwrap_or(cwd)
        .display()
        .to_string();

    println!();
    println!("  vibectl Audit Report");
    println!("  ════════════════════════════════════════════════");
    println!();

    // ── Project ───────────────────────────────────────────────────────────────
    section("Project");

    let root_marker = if report.project_root.is_some() {
        let marker = detect_root_marker(report.project_root.as_deref().unwrap_or(cwd));
        ok(format!("Project root detected    ({})", marker))
    } else {
        warn("Project root              not detected — no .git/Cargo.toml/package.json/.vibectl found".into())
    };
    println!("{root_marker}");
    println!("  {}  Working directory      {}", sym(true), root);

    if report.languages.is_empty() {
        println!("{}", warn("Language                  not detected".into()));
    } else {
        println!(
            "{}",
            ok(format!(
                "Language detected         {}",
                report.languages.join(", ")
            ))
        );
    }

    for m in &report.manifests {
        if m.status == DiscoveryStatus::Found {
            println!(
                "{}",
                ok(format!(
                    "Build manifest            {}{}",
                    m.path,
                    m.note
                        .as_deref()
                        .map(|n| format!("  ({})", n))
                        .unwrap_or_default()
                ))
            );
        }
    }

    // ── Agent Instructions ────────────────────────────────────────────────────
    println!();
    section("Agent Instructions");

    let found_any_instructions = report
        .instructions
        .iter()
        .any(|e| e.status == DiscoveryStatus::Found);

    for entry in &report.instructions {
        let label = short_path(&entry.path);
        match entry.status {
            DiscoveryStatus::Found => println!(
                "{}",
                ok(format!(
                    "{:<35}{}",
                    label,
                    entry
                        .note
                        .as_deref()
                        .map(|n| format!("  ↳ {}", n))
                        .unwrap_or_default()
                ))
            ),
            DiscoveryStatus::NotFound => println!("{}", missing(label)),
            DiscoveryStatus::Unknown => println!("{}", unknown(label)),
        }
    }

    if !found_any_instructions {
        println!(
            "{}",
            info("  Using built-in defaults (no project-level instructions found)")
        );
    }

    // ── Steering Files ────────────────────────────────────────────────────────
    println!();
    section("Steering Files");

    if report.steering.is_empty() {
        println!(
            "{}",
            missing(".vibectl/steering/  .kiro/steering/  steering/".to_string())
        );
        println!(
            "{}",
            info("  Run: vibectl --init-steering  to create a starter set")
        );
    } else {
        for entry in &report.steering {
            let label = short_path(&entry.path);
            match entry.status {
                DiscoveryStatus::Found => println!(
                    "{}",
                    ok(format!(
                        "{:<35}{}",
                        label,
                        entry
                            .note
                            .as_deref()
                            .map(|n| format!("  ↳ {}", n))
                            .unwrap_or_default()
                    ))
                ),
                DiscoveryStatus::NotFound => println!("{}", missing(label)),
                DiscoveryStatus::Unknown => println!("{}", unknown(label)),
            }
        }
    }

    // ── Specs ─────────────────────────────────────────────────────────────────
    //
    // Read the specs directory directly rather than adding a DiscoveryEntry
    // type: a spec is a directory of three files, and reporting which phases
    // are missing is more useful here than reporting three missing paths.
    println!();
    section("Specs");
    let specs_dir = crate::agent::spec::Spec::specs_dir(cwd);
    let mut slugs: Vec<String> = std::fs::read_dir(&specs_dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    slugs.sort();
    if slugs.is_empty() {
        println!(
            "{}",
            missing(".vibectl/specs/<slug>/{requirements,design,tasks}.md".to_string())
        );
        println!(
            "{}",
            info("  Run: /spec add <task>  to decide requirements before coding")
        );
    } else {
        for slug in &slugs {
            match crate::agent::spec::Spec::load(cwd, slug) {
                Ok(spec) => {
                    let progress = if spec.tasks.is_empty() {
                        String::new()
                    } else {
                        format!("  {}/{} tasks done", spec.done_count(), spec.tasks.len())
                    };
                    let phases: Vec<&str> = [
                        (spec.requirements.is_some(), "requirements"),
                        (spec.design.is_some(), "design"),
                        (!spec.tasks.is_empty(), "tasks"),
                    ]
                    .iter()
                    .filter(|(present, _)| *present)
                    .map(|(_, name)| *name)
                    .collect();
                    println!(
                        "{}",
                        ok(format!("{:<35}{}{}", slug, phases.join(" → "), progress))
                    );
                    if spec.tasks_unreadable {
                        println!(
                            "{}",
                            warn(format!(
                                "  ↳ {}/tasks.md has no checkboxes, so nothing is executable",
                                slug
                            ))
                        );
                    }
                }
                Err(e) => println!("{}", warn(format!("{:<35}{e}", slug))),
            }
        }
    }

    // ── Tools ─────────────────────────────────────────────────────────────────
    //
    // Read from the live registry rather than a hardcoded list, so a tool that
    // is removed or renamed cannot linger here as a permanent false positive.
    println!();
    section("Tools");
    let mut registered: Vec<String> = crate::tools::all_tools()
        .iter()
        .map(|t| t.def().name.clone())
        .collect();
    registered.sort();
    for tool in &registered {
        let needs_approval = !matches!(
            tool.as_str(),
            "read_file" | "glob" | "grep" | "git" | "web_fetch" | "list_symbols"
        );
        let note = if needs_approval {
            "registered  (approval required)"
        } else {
            "registered  (read-only)"
        };
        println!("{}", ok(format!("{tool:<20}{note}")));
    }

    // ── Testing ───────────────────────────────────────────────────────────────
    println!();
    section("Testing");

    let root_path = report.project_root.as_deref().unwrap_or(cwd);
    let has_rust_tests = detect_rust_tests(root_path);
    let has_ci = detect_ci(root_path) == FeatureStatus::Done;

    if has_rust_tests {
        println!(
            "{}",
            ok("Rust #[test] / mod tests      found in src/".to_string())
        );
    } else {
        println!(
            "{}",
            warn("Rust tests                    no #[test] found in src/".to_string())
        );
    }

    match detect_tests(root_path) {
        FeatureStatus::Done if !has_rust_tests => println!(
            "{}",
            ok("Test suite                    found outside src/".to_string())
        ),
        FeatureStatus::Partial => println!(
            "{}",
            warn("Test suite                    test script declared, no test files".to_string())
        ),
        FeatureStatus::Missing => println!(
            "{}",
            warn("Test suite                    none found".to_string())
        ),
        FeatureStatus::Done => {}
    }

    if has_test_tooling(root_path) {
        println!(
            "{}",
            ok("Test tooling                  declared in the build manifest".to_string())
        );
    } else {
        println!(
            "{}",
            warn("Test tooling                  no dev-dependencies / test script".to_string())
        );
    }

    if has_ci {
        println!(
            "{}",
            ok(format!(
                "CI configuration              {}",
                ci_note(root_path)
            ))
        );
    } else {
        println!(
            "{}",
            missing("CI configuration              no .github/, .gitlab-ci.yml, etc.".to_string())
        );
    }

    // ── Feature inventory ─────────────────────────────────────────────────────
    println!();
    section("Feature Analysis");

    let features = build_feature_inventory(root_path);
    let name_w = features.iter().map(|f| f.name.len()).max().unwrap_or(20) + 2;
    for f in &features {
        println!(
            "  {}  {}  {:<width$}  {}",
            f.status.symbol(),
            f.status.label(),
            f.name,
            f.note,
            width = name_w,
        );
    }

    // ── Security ──────────────────────────────────────────────────────────────
    println!();
    section("Security");

    let has_security_steering = report
        .steering
        .iter()
        .any(|e| e.status == DiscoveryStatus::Found && e.path.contains("security"));
    let has_env_example =
        root_path.join(".env.example").exists() || root_path.join(".env.template").exists();
    let has_gitignore = root_path.join(".gitignore").exists();

    if has_security_steering {
        println!(
            "{}",
            ok("Security steering             security.md found".to_string())
        );
    } else {
        println!(
            "{}",
            unknown("Security rules                no steering/security.md found".to_string())
        );
    }

    if has_env_example {
        println!(
            "{}",
            ok("Environment template          .env.example present".to_string())
        );
    } else {
        println!(
            "{}",
            warn("Environment template          .env.example not found".to_string())
        );
    }

    if has_gitignore {
        println!(
            "{}",
            ok(".gitignore                    present".to_string())
        );
    } else {
        println!(
            "{}",
            warn(".gitignore                    not found".to_string())
        );
    }

    // ── Unknown / Needs Verification ──────────────────────────────────────────
    println!();
    section("Unknown / Needs Verification");

    let unknowns: Vec<String> = build_unknowns(report, root_path);
    if unknowns.is_empty() {
        println!("  (none — all checked items resolved)");
    } else {
        for u in &unknowns {
            println!("{}", unknown(u.clone()));
        }
    }

    // ── Summary ───────────────────────────────────────────────────────────────
    println!();
    println!("  ════════════════════════════════════════════════");
    let total = report.instructions.len() + report.steering.len();
    let found = report.found_count();
    println!(
        "  Steering sources: {}/{} found   Languages: {}",
        found,
        total,
        if report.languages.is_empty() {
            "none detected".to_string()
        } else {
            report.languages.join(", ")
        }
    );
    println!();
    println!("  Tip: Run  vibectl --init-steering  to create .vibectl/steering/ skeleton.");
    println!();
}

// ─── Feature detection helpers ────────────────────────────────────────────────

/// Inventory of engineering-practice signals found in the repository under audit.
///
/// Every entry is derived from something observable on disk.  The previous
/// version of this function hardcoded vibectl's *own* feature list with a
/// constant `Done` status, so the section reported the same thing regardless of
/// which project was being audited.
fn build_feature_inventory(root: &Path) -> Vec<Feature> {
    let tests = detect_tests(root);
    let tests_note = match &tests {
        FeatureStatus::Done => "test suite detected",
        FeatureStatus::Partial => "test script declared, no test files found",
        _ => "no test files detected",
    };
    let ci = detect_ci(root);
    vec![
        Feature {
            name: "Automated tests",
            status: tests,
            note: tests_note.into(),
        },
        Feature {
            name: "CI pipeline",
            status: ci,
            note: ci_note(root),
        },
        Feature {
            name: "Lint / format config",
            status: detect_lint_config(root),
            note: "clippy/rustfmt/eslint/prettier/biome/golangci".into(),
        },
        Feature {
            name: "Dependency locking",
            status: if has_lockfile(root) {
                FeatureStatus::Done
            } else {
                FeatureStatus::Missing
            },
            note: "Cargo.lock / package-lock.json / go.sum".into(),
        },
        Feature {
            name: "Environment template",
            status: if has_env_template(root) {
                FeatureStatus::Done
            } else {
                FeatureStatus::Missing
            },
            note: ".env.example documents required config".into(),
        },
        Feature {
            name: "Containerization",
            status: if has_containerfile(root) {
                FeatureStatus::Done
            } else {
                FeatureStatus::Missing
            },
            note: "Dockerfile / compose / Containerfile".into(),
        },
        Feature {
            name: "Contributor docs",
            status: if has_contributor_docs(root) {
                FeatureStatus::Done
            } else {
                FeatureStatus::Missing
            },
            note: "CONTRIBUTING.md".into(),
        },
        Feature {
            name: "License",
            status: if has_license(root) {
                FeatureStatus::Done
            } else {
                FeatureStatus::Missing
            },
            note: "LICENSE file at repo root".into(),
        },
        Feature {
            name: "Agent instructions",
            status: if has_agent_instructions(root) {
                FeatureStatus::Done
            } else {
                FeatureStatus::Missing
            },
            note: "AGENTS.md / CLAUDE.md / .cursorrules".into(),
        },
        Feature {
            name: "Pre-commit hooks",
            status: if has_precommit(root) {
                FeatureStatus::Done
            } else {
                FeatureStatus::Missing
            },
            note: "pre-commit / lefthook / husky".into(),
        },
        Feature {
            name: "Issue templates",
            status: if has_issue_templates(root) {
                FeatureStatus::Done
            } else {
                FeatureStatus::Missing
            },
            note: ".github/ISSUE_TEMPLATE or PR template".into(),
        },
    ]
}

/// Does the repository contain tests in any of the ecosystems we understand?
fn detect_tests(root: &Path) -> FeatureStatus {
    for dir in ["tests", "test", "spec", "__tests__"] {
        if root.join(dir).is_dir() {
            return FeatureStatus::Done;
        }
    }
    if detect_rust_tests(root) || detect_script_tests(root) {
        return FeatureStatus::Done;
    }
    // A manifest with a test script means tests are expected somewhere.
    if package_json(root).is_some_and(|src| src.contains("\"test\"")) {
        return FeatureStatus::Partial;
    }
    FeatureStatus::Missing
}

fn detect_rust_tests(root: &Path) -> bool {
    let src = root.join("src");
    if !src.is_dir() {
        return false;
    }
    // walk up to 3 levels
    fn walk(dir: &Path, depth: u8) -> bool {
        if depth == 0 {
            return false;
        }
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    if walk(&p, depth - 1) {
                        return true;
                    }
                } else if p.extension().is_some_and(|e| e == "rs")
                    && std::fs::read_to_string(&p)
                        .is_ok_and(|src| src.contains("#[test]") || src.contains("mod tests"))
                {
                    return true;
                }
            }
        }
        false
    }
    walk(&src, 3)
}

/// Python / Go / Java / JS test-file naming conventions.
fn detect_script_tests(root: &Path) -> bool {
    fn walk(dir: &Path, depth: u8) -> bool {
        if depth == 0 {
            return false;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if walk(&p, depth - 1) {
                    return true;
                }
                continue;
            }
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with("test_")
                || name.starts_with("conftest")
                || name.ends_with("_test.go")
                || name.ends_with("_test.py")
                || name.ends_with("Test.java")
                || name.ends_with(".test.ts")
                || name.ends_with(".test.js")
                || name.ends_with(".spec.ts")
                || name.ends_with(".spec.js")
            {
                return true;
            }
        }
        false
    }
    walk(root, 4)
}

fn detect_ci(root: &Path) -> FeatureStatus {
    if root.join(".github/workflows").is_dir()
        || root.join(".gitlab-ci.yml").is_file()
        || root.join(".circleci").is_dir()
        || root.join("Jenkinsfile").is_file()
        || root.join(".buildkite").is_dir()
        || root.join("azure-pipelines.yml").is_file()
    {
        return FeatureStatus::Done;
    }
    if root.join(".github").is_dir() {
        return FeatureStatus::Partial;
    }
    FeatureStatus::Missing
}

fn ci_note(root: &Path) -> String {
    if root.join(".github/workflows").is_dir() {
        ".github/workflows".into()
    } else if root.join(".gitlab-ci.yml").is_file() {
        ".gitlab-ci.yml".into()
    } else if root.join("Jenkinsfile").is_file() {
        "Jenkinsfile".into()
    } else {
        "no pipeline definition found".into()
    }
}

fn detect_lint_config(root: &Path) -> FeatureStatus {
    let candidates = [
        "rustfmt.toml",
        ".rustfmt.toml",
        "clippy.toml",
        ".clippy.toml",
        "eslint.config.js",
        ".eslintrc",
        ".eslintrc.json",
        ".eslintrc.yml",
        "biome.json",
        ".prettierrc",
        ".golangci.yml",
        ".shellcheckrc",
    ];
    if candidates.iter().any(|c| root.join(c).exists()) {
        return FeatureStatus::Done;
    }
    // A `lint` script in package.json counts too.
    if package_json(root).is_some_and(|src| src.contains("\"lint\"")) {
        return FeatureStatus::Done;
    }
    FeatureStatus::Missing
}

/// Test-only dependencies declared in the build manifest.
fn has_test_tooling(root: &Path) -> bool {
    if let Ok(cargo) = std::fs::read_to_string(root.join("Cargo.toml"))
        && cargo.contains("[dev-dependencies]")
    {
        return true;
    }
    if let Some(pkg) = package_json(root)
        && pkg.contains("\"devDependencies\"")
    {
        return true;
    }
    [
        "pytest.ini",
        "tox.ini",
        "jest.config.js",
        "vitest.config.ts",
        "conftest.py",
    ]
    .iter()
    .any(|f| root.join(f).exists())
}

fn package_json(root: &Path) -> Option<String> {
    std::fs::read_to_string(root.join("package.json")).ok()
}

fn has_lockfile(root: &Path) -> bool {
    [
        "Cargo.lock",
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "go.sum",
        "poetry.lock",
        "uv.lock",
        "Gemfile.lock",
    ]
    .iter()
    .any(|f| root.join(f).is_file())
}

fn has_env_template(root: &Path) -> bool {
    root.join(".env.example").is_file() || root.join(".env.template").is_file()
}

fn has_containerfile(root: &Path) -> bool {
    [
        "Dockerfile",
        "Containerfile",
        "docker-compose.yml",
        "compose.yaml",
    ]
    .iter()
    .any(|f| root.join(f).is_file())
}

fn has_contributor_docs(root: &Path) -> bool {
    root.join("CONTRIBUTING.md").is_file() || root.join(".github/CONTRIBUTING.md").is_file()
}

fn has_license(root: &Path) -> bool {
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with("LICENSE") || n.starts_with("LICENCE"))
            })
        })
        .unwrap_or(false)
}

fn has_agent_instructions(root: &Path) -> bool {
    [
        "AGENTS.md",
        "AGENT.md",
        "CLAUDE.md",
        "GEMINI.md",
        ".cursorrules",
        ".github/copilot-instructions.md",
        ".vibectl/steer.md",
    ]
    .iter()
    .any(|f| root.join(f).is_file())
}

fn has_precommit(root: &Path) -> bool {
    root.join(".pre-commit-config.yaml").is_file()
        || root.join("lefthook.yml").is_file()
        || root.join(".husky").is_dir()
}

fn has_issue_templates(root: &Path) -> bool {
    root.join(".github/ISSUE_TEMPLATE").is_dir()
        || root.join(".github/PULL_REQUEST_TEMPLATE.md").is_file()
}

fn detect_root_marker(root: &Path) -> &'static str {
    if root.join(".git").exists() {
        ".git"
    } else if root.join("Cargo.toml").exists() {
        "Cargo.toml"
    } else if root.join("go.mod").exists() {
        "go.mod"
    } else if root.join("package.json").exists() {
        "package.json"
    } else {
        ".vibectl"
    }
}

fn build_unknowns(report: &DiscoveryReport, root: &Path) -> Vec<String> {
    let mut out = Vec::new();

    if !report
        .steering
        .iter()
        .any(|e| e.status == DiscoveryStatus::Found && e.path.contains("security"))
    {
        out.push("Security requirements — no security.md steering found".into());
    }

    if !report
        .steering
        .iter()
        .any(|e| e.status == DiscoveryStatus::Found && e.path.contains("architecture"))
    {
        out.push("Architecture decisions — no architecture.md steering found".into());
    }

    if !report.ci.iter().any(|e| e.status == DiscoveryStatus::Found) {
        out.push("Deployment target — no CI/CD configuration found".into());
    }

    if !root.join(".env.example").exists() && !root.join(".env.template").exists() {
        out.push("Environment configuration — no .env.example or .env.template".into());
    }

    if !report
        .steering
        .iter()
        .any(|e| e.status == DiscoveryStatus::Found && e.path.contains("testing"))
    {
        out.push("Test strategy — no testing.md steering found".into());
    }

    out
}

// ─── Output helpers ───────────────────────────────────────────────────────────

fn section(title: &str) {
    println!("  {title}");
    println!("  {}", "─".repeat(48));
}

fn ok(msg: String) -> String {
    format!("  ✓  {msg}")
}

fn warn(msg: String) -> String {
    format!("  ⚠  {msg}")
}

fn missing(msg: String) -> String {
    format!("  ✗  {msg}")
}

fn unknown(msg: String) -> String {
    format!("  ?  {msg}")
}

fn info(msg: &str) -> String {
    format!("     {msg}")
}

fn sym(ok: bool) -> &'static str {
    if ok { "✓" } else { "✗" }
}

/// Shorten an absolute path to relative from cwd or just the filename.
fn short_path(path: &str) -> String {
    // try to trim anything before .vibectl or the root markers
    for marker in &[".vibectl/", ".kiro/", "steering/", "docs/"] {
        if let Some(pos) = path.find(marker) {
            return path[pos..].to_string();
        }
    }
    // fallback: just the filename
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn write(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, body).expect("write");
    }

    #[test]
    fn empty_repo_reports_nothing_as_done() {
        let dir = repo();
        let features = build_feature_inventory(dir.path());
        assert!(!features.is_empty());
        for f in &features {
            assert_eq!(
                f.status,
                FeatureStatus::Missing,
                "{} should be Missing in an empty repo",
                f.name
            );
        }
    }

    #[test]
    fn ci_detection_covers_common_pipelines() {
        for marker in [".gitlab-ci.yml", "Jenkinsfile", "azure-pipelines.yml"] {
            let dir = repo();
            write(dir.path(), marker, "x");
            assert_eq!(detect_ci(dir.path()), FeatureStatus::Done, "{marker}");
        }
        for dir_marker in [".circleci", ".buildkite", ".github/workflows"] {
            let dir = repo();
            std::fs::create_dir_all(dir.path().join(dir_marker)).unwrap();
            assert_eq!(detect_ci(dir.path()), FeatureStatus::Done, "{dir_marker}");
        }
    }

    #[test]
    fn github_dir_without_workflows_is_only_partial() {
        let dir = repo();
        write(dir.path(), ".github/dependabot.yml", "x");
        assert_eq!(detect_ci(dir.path()), FeatureStatus::Partial);
    }

    #[test]
    fn tests_detected_via_tests_dir() {
        let dir = repo();
        std::fs::create_dir(dir.path().join("tests")).unwrap();
        assert_eq!(detect_tests(dir.path()), FeatureStatus::Done);
    }

    #[test]
    fn tests_detected_via_inline_rust_mod() {
        let dir = repo();
        write(dir.path(), "src/lib.rs", "#[cfg(test)]\nmod tests {}\n");
        assert_eq!(detect_tests(dir.path()), FeatureStatus::Done);
    }

    #[test]
    fn tests_detected_via_script_conventions() {
        for name in [
            "test_thing.py",
            "main_test.go",
            "widget.test.ts",
            "conftest.py",
        ] {
            let dir = repo();
            write(dir.path(), name, "");
            assert_eq!(detect_tests(dir.path()), FeatureStatus::Done, "{name}");
        }
    }

    #[test]
    fn test_script_without_files_is_partial() {
        let dir = repo();
        write(dir.path(), "package.json", r#"{"scripts":{"test":"jest"}}"#);
        assert_eq!(detect_tests(dir.path()), FeatureStatus::Partial);
    }

    #[test]
    fn lint_config_detected_from_manifest_script() {
        let dir = repo();
        write(
            dir.path(),
            "package.json",
            r#"{"scripts":{"lint":"eslint ."}}"#,
        );
        assert_eq!(detect_lint_config(dir.path()), FeatureStatus::Done);
    }

    #[test]
    fn license_match_accepts_prefixed_variants() {
        let dir = repo();
        write(dir.path(), "LICENSE-MIT", "MIT");
        assert!(has_license(dir.path()));

        let dir2 = repo();
        write(dir2.path(), "LICENCE", "MIT");
        assert!(has_license(dir2.path()));

        let dir3 = repo();
        write(dir3.path(), "license.txt", "MIT");
        assert!(!has_license(dir3.path()), "lowercase name must not match");
    }

    #[test]
    fn precommit_detects_all_three_ecosystems() {
        for (marker, is_dir) in [
            (".pre-commit-config.yaml", false),
            ("lefthook.yml", false),
            (".husky", true),
        ] {
            let dir = repo();
            if is_dir {
                std::fs::create_dir(dir.path().join(marker)).unwrap();
            } else {
                write(dir.path(), marker, "x");
            }
            assert!(has_precommit(dir.path()), "{marker}");
        }
    }

    #[test]
    fn lockfile_detection_is_per_ecosystem() {
        for name in ["Cargo.lock", "package-lock.json", "go.sum", "uv.lock"] {
            let dir = repo();
            write(dir.path(), name, "");
            assert!(has_lockfile(dir.path()), "{name}");
        }
        let dir = repo();
        assert!(!has_lockfile(dir.path()));
    }

    #[test]
    fn feature_inventory_reflects_the_audited_repo_not_vibectl() {
        // Regression guard: the inventory used to hardcode vibectl's own
        // features with a constant Done status regardless of the target repo.
        let dir = repo();
        let names: Vec<&str> = build_feature_inventory(dir.path())
            .iter()
            .map(|f| f.name)
            .collect();
        assert!(!names.contains(&"Interactive TUI"));
        assert!(!names.contains(&"Multi-provider LLM"));
        assert!(names.contains(&"Automated tests"));
    }
}

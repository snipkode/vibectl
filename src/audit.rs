use crate::agent::steer::{DiscoveryReport, DiscoveryStatus, discover};
use std::path::Path;

// ─── Feature inventory ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum FeatureStatus {
    Done,
    Partial,
    Missing,
    Unknown,
}

impl FeatureStatus {
    fn symbol(&self) -> &'static str {
        match self {
            Self::Done => "✓",
            Self::Partial => "⚠",
            Self::Missing => "✗",
            Self::Unknown => "?",
        }
    }
    fn label(&self) -> &'static str {
        match self {
            Self::Done => "DONE   ",
            Self::Partial => "PARTIAL",
            Self::Missing => "MISSING",
            Self::Unknown => "UNKNOWN",
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
        match m.status {
            DiscoveryStatus::Found => {
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
            _ => {}
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

    // ── Tools ─────────────────────────────────────────────────────────────────
    println!();
    section("Tools");
    for tool in &[
        "read_file",
        "write_file",
        "glob",
        "grep",
        "git",
        "shell_exec",
    ] {
        println!("{}", ok(format!("{tool:<35}registered")));
    }

    // ── Testing ───────────────────────────────────────────────────────────────
    println!();
    section("Testing");

    let root_path = report.project_root.as_deref().unwrap_or(cwd);
    let has_rust_tests = detect_rust_tests(root_path);
    let has_ci = report.ci.iter().any(|e| e.status == DiscoveryStatus::Found);

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

    let has_dev_deps = detect_dev_dependencies(root_path);
    if has_dev_deps {
        println!(
            "{}",
            ok("Dev dependencies              Cargo.toml [dev-dependencies]".to_string())
        );
    } else {
        println!(
            "{}",
            warn("Dev dependencies              none found".to_string())
        );
    }

    if has_ci {
        for e in report
            .ci
            .iter()
            .filter(|e| e.status == DiscoveryStatus::Found)
        {
            println!(
                "{}",
                ok(format!("CI config                     {}", e.path))
            );
        }
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

fn build_feature_inventory(root: &Path) -> Vec<Feature> {
    vec![
        Feature {
            name: "Interactive TUI",
            status: FeatureStatus::Done,
            note: "src/tui/ — ratatui-based chat interface".into(),
        },
        Feature {
            name: "Headless / CI mode",
            status: FeatureStatus::Done,
            note: "src/headless.rs — --headless flag".into(),
        },
        Feature {
            name: "Multi-provider LLM",
            status: FeatureStatus::Done,
            note: "src/llm/ — OpenAI, Anthropic, Ollama".into(),
        },
        Feature {
            name: "Agent loop + tools",
            status: FeatureStatus::Done,
            note: "src/agent/mod.rs — tool calling loop".into(),
        },
        Feature {
            name: "Steering / instructions",
            status: FeatureStatus::Done,
            note: "src/agent/steer.rs — multi-source discovery".into(),
        },
        Feature {
            name: "Plan mode",
            status: FeatureStatus::Done,
            note: "/plan command + --plan flag".into(),
        },
        Feature {
            name: "Audit command",
            status: if root.join("src").join("audit.rs").exists() {
                FeatureStatus::Done
            } else {
                FeatureStatus::Missing
            },
            note: "vibectl audit — repository gap analysis".into(),
        },
        Feature {
            name: "JSON output",
            status: FeatureStatus::Missing,
            note: "no --json flag implemented".into(),
        },
        Feature {
            name: "Progress indicators",
            status: FeatureStatus::Partial,
            note: "spinner in TUI; headless has none".into(),
        },
        Feature {
            name: "Session persistence",
            status: detect_session_persistence(root),
            note: "~/.config/vibectl/history.json".into(),
        },
        Feature {
            name: "Init-steering scaffold",
            status: FeatureStatus::Done,
            note: "vibectl --init-steering generates .vibectl/steering/".into(),
        },
    ]
}

fn detect_session_persistence(root: &Path) -> FeatureStatus {
    let session = root.join("src").join("session.rs");
    if session.is_file() {
        if let Ok(src) = std::fs::read_to_string(&session) {
            if src.contains("history") {
                return FeatureStatus::Done;
            }
        }
        FeatureStatus::Partial
    } else {
        FeatureStatus::Unknown
    }
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
                } else if p.extension().map(|e| e == "rs").unwrap_or(false) {
                    if let Ok(src) = std::fs::read_to_string(&p) {
                        if src.contains("#[test]") || src.contains("mod tests") {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }
    walk(&src, 3)
}

fn detect_dev_dependencies(root: &Path) -> bool {
    let cargo = root.join("Cargo.toml");
    if cargo.is_file() {
        if let Ok(src) = std::fs::read_to_string(cargo) {
            return src.contains("[dev-dependencies]");
        }
    }
    false
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

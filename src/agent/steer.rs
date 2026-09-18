use anyhow::{Context, Result};
use std::path::Path;

pub const DEFAULT_SYSTEM_PROMPT: &str =
    "You are vibectl, an agentic coding assistant running inside the user's project \
directory. You help implement features, fix bugs, write tests, and explain code.

Guidelines:
- Before proposing code changes, read the relevant files with the read_file tool.
- Use tools (read_file, write_file, glob, grep, shell_exec) to do the work, not guesswork.
- THINK BEFORE WRITING. When asked to generate a project or new feature, first output a
  short plan as a code block listing EXACTLY the file paths you intend to create or modify.
  Do not start writing files until you have shown that plan to the user.
- Each file write asks the user to confirm the absolute path. Make paths explicit, inside
  the current project, one file at a time. Never invent or guess paths.
- Never create or modify files outside the project root (e.g. parent '..', /tmp, home, /etc).
  Such writes are refused outright.
- Do not scatter files into unrelated or unknown locations; keep everything under the
  project directory.
- Prefer small, focused edits. Keep changes minimal and idiomatic.
- After implementing a change, run the project's build/test/lint commands if reasonable.
- If a task is large and ambiguous, break it into steps and use /plan to get approval.
- Be concise in prose, but do not truncate code — include complete files when writing code.
- Never claim a shell command succeeded without running it.";

#[allow(dead_code)]
pub struct Steering {
    pub content: String,
    pub sources: Vec<String>,
}

pub fn load_steering(cwd: &Path) -> Steering {
    let mut content = String::new();
    let mut sources = Vec::new();

    let mut gather = |path: &Path| {
        if path.is_file()
            && let Ok(raw) = std::fs::read_to_string(path)
        {
            content.push_str(&raw);
            content.push('\n');
            sources.push(path.display().to_string());
        }
    };

    let root = find_project_root(cwd);

    if let Some(root) = &root {
        // conventional agent instructions
        for name in ["AGENTS.md", "CLAUDE.md"] {
            gather(&root.join(name));
        }
        let v_dir = root.join(".vibectl");
        gather(&v_dir.join("AGENTS.md"));
        gather(&v_dir.join("steer.md"));
    }
    gather(&cwd.join(".vibectl").join("steer.md"));

    if content.is_empty() {
        sources.push("built-in defaults".to_string());
        content = DEFAULT_SYSTEM_PROMPT.to_string();
    } else {
        content = format!(
            "Project context from steering files ({}):\n{}\n\n{}",
            sources.join(", "),
            content,
            DEFAULT_SYSTEM_PROMPT
        );
    }

    Steering { content, sources }
}

pub fn find_project_root(cwd: &Path) -> Option<std::path::PathBuf> {
    let mut dir = Some(cwd.to_path_buf());
    while let Some(d) = dir {
        if d.join(".git").exists() || d.join("Cargo.toml").exists() || d.join(".vibectl").is_dir() {
            return Some(d);
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    None
}

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

pub fn save_plan(cwd: &Path, plan: &str) -> Result<std::path::PathBuf> {
    let root = find_project_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let dir = root.join(".vibectl");
    std::fs::create_dir_all(&dir).context("failed to create .vibectl dir")?;
    let path = dir.join("PLAN.md");
    std::fs::write(&path, plan).context("failed to write PLAN.md")?;
    Ok(path)
}

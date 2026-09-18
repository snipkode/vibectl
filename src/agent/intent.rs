//! Intent classification for vibectl.
//!
//! Two-stage pipeline:
//!
//! 1. `intent_rules()` — fast sync check using signal dictionaries.
//!    Returns `Some(Intent)` when confident, `None` when ambiguous.
//!
//! 2. `classify_intent()` — async, calls the LLM with a strict JSON
//!    prompt when rules return `None`. Falls back to `Intent::CodeWrite`
//!    (tools enabled) on any error.

use crate::llm::provider::{ChatRequest, Message, Provider};
use std::sync::Arc;

// ─── Intent enum ──────────────────────────────────────────────────────────────

/// Classified intent of a user message.
/// Controls which tools are made available to the LLM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// Greeting, small talk, preference, acknowledgement.
    /// Tools: none.
    Conversational,

    /// Read-only inspection: explain code, list symbols, search, git log.
    /// Tools: read_file, glob, grep, git, list_symbols, web_fetch.
    Informational,

    /// Write or create specific files/functions.
    /// Tools: read-only + write_file, patch_file.
    CodeWrite,

    /// Broad restructuring across many files. Forces planning phase.
    /// Tools: read-only + write_file, patch_file.
    Refactor,

    /// Run arbitrary shell commands (build, test, lint, etc.).
    /// Tools: read-only + shell_exec.
    ShellExec,

    /// Git operations (commit, push, branch, diff, log, stash).
    /// Tools: git + shell_exec (git-only whitelist enforced at approval time).
    GitOp,

    /// Deploy to environment, run migrations, push images.
    /// Tools: read-only + shell_exec + web_fetch. Requires explicit confirm.
    Deploy,
}

impl Intent {
    /// Human-readable label used in LLM prompt and logs.
    #[allow(dead_code)]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Conversational => "conversational",
            Self::Informational => "informational",
            Self::CodeWrite => "code_write",
            Self::Refactor => "refactor",
            Self::ShellExec => "shell_exec",
            Self::GitOp => "git",
            Self::Deploy => "deploy",
        }
    }

    /// Parse from LLM-returned string (case-insensitive).
    pub fn from_label(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "conversational" => Some(Self::Conversational),
            "informational" => Some(Self::Informational),
            "code_write" | "write" => Some(Self::CodeWrite),
            "refactor" => Some(Self::Refactor),
            "shell_exec" | "shell" => Some(Self::ShellExec),
            "git" | "gitop" | "git_op" => Some(Self::GitOp),
            "deploy" => Some(Self::Deploy),
            _ => None,
        }
    }
}

// ─── Signal dictionaries ──────────────────────────────────────────────────────

const CONVERSATIONAL_SIGNALS: &[&str] = &[
    // English
    "hi",
    "hello",
    "hey",
    "thanks",
    "thank you",
    "ok",
    "okay",
    "noted",
    "got it",
    "understood",
    "great",
    "nice",
    "cool",
    "awesome",
    "bye",
    "goodbye",
    "see you",
    "sure",
    "agreed",
    "yes",
    "no",
    "good morning",
    "good afternoon",
    "good evening",
    "good night",
    // Indonesian
    "halo",
    "hai",
    "hei",
    "oke",
    "sip",
    "mantap",
    "terima kasih",
    "makasih",
    "iya",
    "tidak",
    "setuju",
    "lanjut",
    "siap",
    "selamat pagi",
    "selamat siang",
    "selamat malam",
];

/// Deploy signals — highest priority among task intents.
const DEPLOY_SIGNALS: &[&str] = &[
    "deploy",
    "to production",
    "ke production",
    "to prod",
    "ke prod",
    "to staging",
    "ke staging",
    " prod ",
    "docker push",
    "kubectl",
    "helm ",
    "kubernetes",
    "k8s",
    "run migration",
    "jalankan migration",
    "database migration",
    "db migration",
    "release to",
    "publish to",
    "ship it",
];

const GIT_SIGNALS: &[&str] = &[
    "git commit",
    "git push",
    "git pull",
    "git stash",
    "git diff",
    "git log",
    "git branch",
    "git checkout",
    "git merge",
    "git rebase",
    "git status",
    "git add",
    "git reset",
    "git tag",
    "commit all",
    "commit perubahan",
    "push to ",
    "push ke ",
    "create branch",
    "buat branch",
    "switch branch",
    "pindah branch",
    "new branch",
    "checkout ke",
    "merge ke",
    "rebase ke",
    "stash changes",
    "pop stash",
];

const SHELL_SIGNALS: &[&str] = &[
    "cargo ",
    "npm ",
    "pip ",
    "yarn ",
    "make ",
    "go run",
    "go build",
    "python ",
    "node ",
    "mvn ",
    "gradle ",
    "$ ",
    "./",
    "run tests",
    "run the tests",
    "jalankan tests",
    "run build",
    "jalankan build",
    "restart ",
    "start server",
    "stop server",
];

const REFACTOR_SIGNALS: &[&str] = &[
    "refactor",
    "refaktor",
    "rename all",
    "ubah nama semua",
    "restructure",
    "reorganize",
    "reorganisasi",
    "extract ",
    "split into",
    "pisahkan",
    "merge into",
    "gabungkan",
    "migrate from",
    "redesign",
    "rewrite",
    "modularize",
    "decouple",
    "clean up ",
    "bersihkan ",
];

const SCOPE_AMPLIFIERS: &[&str] = &[
    "entire",
    "all ",
    "throughout",
    "across ",
    "everywhere",
    "every file",
    "all files",
    "semua",
    "seluruh",
    "setiap",
];

const TASK_VERBS: &[&str] = &[
    "implement",
    "create ",
    "add ",
    "fix ",
    "write ",
    "build ",
    "generate ",
    "scaffold ",
    "make ",
    "insert ",
    "append ",
    "define ",
    "initialize ",
    "setup ",
    "buat ",
    "tambah ",
    "tambahkan ",
    "perbaiki ",
    "tulis ",
    "bikin ",
    "implementasi ",
];

const QUESTION_STARTERS: &[&str] = &[
    "what ",
    "how ",
    "why ",
    "when ",
    "where ",
    "who ",
    "which ",
    "can you ",
    "could you ",
    "do you ",
    "did you ",
    "is it ",
    "are you ",
    "explain ",
    "describe ",
    "tell me ",
    "show me ",
    "list all",
    "find all",
    "search for",
    // Indonesian
    "apa ",
    "bagaimana ",
    "kenapa ",
    "mengapa ",
    "kapan ",
    "siapa ",
    "boleh ",
    "bisa ",
    "apakah ",
    "jelaskan ",
    "ceritakan ",
    "tolong jelaskan",
    "tampilkan ",
    "cari ",
    "analisa ",
    "analyze ",
    "analisis ",
    "review ",
    "audit ",
    "inspect ",
];

const PREF_SIGNALS: &[&str] = &[
    "pake ",
    "pakai ",
    "gunakan ",
    "speak ",
    "talk in ",
    "bahasa ",
    "language ",
    "in english",
    "in indonesian",
    "please use",
    "mohon ",
    "tolong gunakan",
    "switch to ",
    "ganti ke ",
    "use english",
    "use indonesian",
];

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn has_file_ref(input: &str) -> bool {
    let code_exts = [
        ".rs", ".py", ".js", ".ts", ".go", ".toml", ".md", ".json", ".yaml", ".yml", ".html",
        ".css", ".sh", ".tsx", ".jsx", ".sql",
    ];
    input.split_whitespace().any(|w| {
        let w = w.trim_matches(|c: char| ",;:?!()[]{}\"'".contains(c));
        code_exts.iter().any(|e| w.ends_with(e)) || (w.contains('/') && !w.starts_with("http"))
    })
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

fn starts_with_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.starts_with(n))
}

// ─── Rule-based classifier ────────────────────────────────────────────────────

/// Fast rule-based intent classification using signal dictionaries.
///
/// Returns `Some(Intent)` when confident, `None` when ambiguous.
/// Priority order: Deploy > GitOp > ShellExec > Refactor > CodeWrite
///                 > Informational > Conversational > None (LLM fallback)
pub fn intent_rules(input: &str) -> Option<Intent> {
    let trimmed = input.trim();
    let lower = trimmed.to_lowercase();
    let word_count = trimmed.split_whitespace().count();

    // 1. Deploy (highest priority — must check before word_count guard)
    if contains_any(&lower, DEPLOY_SIGNALS) {
        return Some(Intent::Deploy);
    }

    // 2. Git operations (before word_count guard)
    if contains_any(&lower, GIT_SIGNALS) {
        return Some(Intent::GitOp);
    }

    // 3. Shell / build / test execution (before word_count guard)
    if contains_any(&lower, SHELL_SIGNALS) {
        return Some(Intent::ShellExec);
    }

    // 4. Very short input with no remaining technical signals → Conversational
    if word_count <= 3 {
        let has_path = trimmed.contains("--");
        let has_task = TASK_VERBS.iter().any(|v| lower.starts_with(v));
        let has_refactor = contains_any(&lower, REFACTOR_SIGNALS);
        let has_question = starts_with_any(&lower, QUESTION_STARTERS);
        if !has_path && !has_task && !has_refactor && !has_question {
            return Some(Intent::Conversational);
        }
    }

    // 5. Explicit greeting / social phrase → Conversational
    for sig in CONVERSATIONAL_SIGNALS {
        if lower == *sig
            || lower.starts_with(&format!("{sig} "))
            || lower.ends_with(&format!(" {sig}"))
        {
            return Some(Intent::Conversational);
        }
    }

    // 6. Preference / language request → Conversational (if no task verb)
    if starts_with_any(&lower, PREF_SIGNALS) || contains_any(&lower, PREF_SIGNALS) {
        if !contains_any(&lower, TASK_VERBS) {
            return Some(Intent::Conversational);
        }
    }

    // 7. Refactor — broad restructuring
    let has_refactor = contains_any(&lower, REFACTOR_SIGNALS);
    let has_scope = contains_any(&lower, SCOPE_AMPLIFIERS);
    let has_task = contains_any(&lower, TASK_VERBS);
    if has_refactor || (has_scope && has_task) {
        return Some(Intent::Refactor);
    }

    // 8. Code write — specific file/function modification
    let has_file = has_file_ref(&lower);
    if has_task && has_file {
        return Some(Intent::CodeWrite);
    }
    if has_task && word_count > 4 {
        return Some(Intent::CodeWrite);
    }

    // 9. Informational — question without task verb and without file ref
    //    "show me" with specific target (file/config) → ambiguous → None
    let has_question = starts_with_any(&lower, QUESTION_STARTERS);
    if has_question && !has_task {
        if has_file {
            return None; // question + file ref → LLM decides
        }
        // "show me X" where X is specific/technical → ambiguous
        let ambiguous_starters = ["show me ", "find ", "search for", "tampilkan ", "cari "];
        if ambiguous_starters.iter().any(|s| lower.starts_with(s)) && word_count > 5 {
            return None;
        }
        return Some(Intent::Informational);
    }

    // 10. Ambiguous → LLM fallback
    None
}

// ─── LLM fallback ─────────────────────────────────────────────────────────────

/// Classify intent via LLM when rules are ambiguous.
/// Single message, max_tokens=30, no tools, temperature=0.
/// Falls back to `Intent::CodeWrite` on any error.
pub async fn classify_intent_llm(input: &str, model: &str, provider: Arc<dyn Provider>) -> Intent {
    let prompt = format!(
        "Classify this user message into exactly one intent.\n\
         Respond ONLY with JSON: {{\"intent\": \"<label>\"}}\n\n\
         Labels:\n\
         - conversational: greeting, small talk, preference, language setting, acknowledgement\n\
         - informational: question about code/concepts, explain, read-only inspection\n\
         - code_write: implement, add, fix, create specific file or function\n\
         - refactor: broad restructuring, rename across files, migrate, redesign\n\
         - shell_exec: run build/test/lint command, execute script\n\
         - git: git commit/push/pull/branch/diff/stash/merge\n\
         - deploy: deploy to environment, run DB migration, push docker image\n\n\
         Message: {input}"
    );

    let req = ChatRequest {
        model: model.to_string(),
        messages: vec![Message::user(prompt)],
        temperature: 0.0,
        max_tokens: Some(30),
        stream: false,
        tools: vec![],
        system: None,
    };

    match provider.chat(&req).await {
        Ok(resp) => {
            let text = resp.content.unwrap_or_default();
            // Try strict JSON parse
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(text.trim()) {
                if let Some(label) = val.get("intent").and_then(|v| v.as_str()) {
                    if let Some(intent) = Intent::from_label(label) {
                        return intent;
                    }
                }
            }
            // Scan raw text for any label keyword
            let lower = text.to_lowercase();
            for label in &[
                "conversational",
                "informational",
                "code_write",
                "refactor",
                "shell_exec",
                "git",
                "deploy",
            ] {
                if lower.contains(label) {
                    if let Some(intent) = Intent::from_label(label) {
                        return intent;
                    }
                }
            }
            Intent::CodeWrite // safe default
        }
        Err(_) => Intent::CodeWrite,
    }
}

/// Full two-stage intent classification.
/// Rules first → LLM fallback if None.
pub async fn classify_intent(input: &str, model: &str, provider: Arc<dyn Provider>) -> Intent {
    if let Some(intent) = intent_rules(input) {
        return intent;
    }
    classify_intent_llm(input, model, provider).await
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn r(input: &str) -> Option<Intent> {
        intent_rules(input)
    }

    #[test]
    fn conversational_greetings() {
        assert_eq!(r("halo"), Some(Intent::Conversational));
        assert_eq!(r("hali"), Some(Intent::Conversational));
        assert_eq!(r("hi"), Some(Intent::Conversational));
        assert_eq!(r("hello"), Some(Intent::Conversational));
        assert_eq!(r("thanks"), Some(Intent::Conversational));
        assert_eq!(r("ok"), Some(Intent::Conversational));
        assert_eq!(r("mantap"), Some(Intent::Conversational));
        assert_eq!(r("terima kasih"), Some(Intent::Conversational));
        assert_eq!(r("ok sip"), Some(Intent::Conversational));
        assert_eq!(r("noted"), Some(Intent::Conversational));
        assert_eq!(r("pake bahasa indonesia"), Some(Intent::Conversational));
        assert_eq!(r("use english please"), Some(Intent::Conversational));
        assert_eq!(r("bahasa indonesia ya"), Some(Intent::Conversational));
    }

    #[test]
    fn informational_questions() {
        assert_eq!(
            r("how does the agent loop work?"),
            Some(Intent::Informational)
        );
        assert_eq!(r("explain the tool dispatch"), Some(Intent::Informational));
        assert_eq!(r("what is a steering file?"), Some(Intent::Informational));
        assert_eq!(
            r("what commands are available?"),
            Some(Intent::Informational)
        );
        assert_eq!(r("describe the architecture"), Some(Intent::Informational));
        assert_eq!(r("list all available tools"), Some(Intent::Informational));
    }

    #[test]
    fn informational_with_file_ref_returns_none() {
        // Question + file ref → ambiguous → LLM fallback
        assert_eq!(r("what does session.rs do?"), None);
        assert_eq!(r("how does agent/mod.rs work?"), None);
    }

    #[test]
    fn code_write_tasks() {
        assert_eq!(
            r("implement pagination for the API endpoint"),
            Some(Intent::CodeWrite)
        );
        assert_eq!(r("add unit tests to agent/mod.rs"), Some(Intent::CodeWrite));
        assert_eq!(r("fix the bug in session.rs"), Some(Intent::CodeWrite));
        assert_eq!(
            r("create a new tool for web scraping"),
            Some(Intent::CodeWrite)
        );
        assert_eq!(
            r("buat fungsi baru di tools/mod.rs"),
            Some(Intent::CodeWrite)
        );
        assert_eq!(
            r("tambahkan error handling di config.rs"),
            Some(Intent::CodeWrite)
        );
    }

    #[test]
    fn refactor_tasks() {
        assert_eq!(r("refactor the entire auth module"), Some(Intent::Refactor));
        assert_eq!(
            r("rename all occurrences of getUserName"),
            Some(Intent::Refactor)
        );
        assert_eq!(
            r("extract the approval logic into a separate module"),
            Some(Intent::Refactor)
        );
        assert_eq!(r("restructure the project layout"), Some(Intent::Refactor));
        assert_eq!(r("refaktor modul autentikasi"), Some(Intent::Refactor));
    }

    #[test]
    fn shell_exec_tasks() {
        assert_eq!(r("cargo test"), Some(Intent::ShellExec));
        assert_eq!(r("run the tests"), Some(Intent::ShellExec));
        assert_eq!(r("npm install"), Some(Intent::ShellExec));
        assert_eq!(r("jalankan tests"), Some(Intent::ShellExec));
        assert_eq!(r("cargo build --release"), Some(Intent::ShellExec));
    }

    #[test]
    fn git_tasks() {
        assert_eq!(r("git commit all changes"), Some(Intent::GitOp));
        assert_eq!(r("git push to origin"), Some(Intent::GitOp));
        assert_eq!(r("create branch feature/login"), Some(Intent::GitOp));
        assert_eq!(r("buat branch feature/auth"), Some(Intent::GitOp));
        assert_eq!(r("git stash"), Some(Intent::GitOp));
        assert_eq!(r("commit perubahan ini"), Some(Intent::GitOp));
    }

    #[test]
    fn deploy_tasks() {
        assert_eq!(r("deploy ke production"), Some(Intent::Deploy));
        assert_eq!(r("deploy to staging"), Some(Intent::Deploy));
        assert_eq!(r("run migration"), Some(Intent::Deploy));
        assert_eq!(r("jalankan migration"), Some(Intent::Deploy));
        assert_eq!(r("kubectl apply -f deployment.yaml"), Some(Intent::Deploy));
    }

    #[test]
    fn deploy_beats_git() {
        // git push ke production → Deploy wins
        assert_eq!(r("git push ke production"), Some(Intent::Deploy));
    }

    #[test]
    fn ambiguous_returns_none() {
        assert_eq!(r("what does session.rs do?"), None);
        assert_eq!(r("the agent seems to be calling tools unexpectedly"), None);
        assert_eq!(r("show me the current model configuration"), None);
    }

    #[test]
    fn from_label_roundtrip() {
        let intents = [
            Intent::Conversational,
            Intent::Informational,
            Intent::CodeWrite,
            Intent::Refactor,
            Intent::ShellExec,
            Intent::GitOp,
            Intent::Deploy,
        ];
        for intent in &intents {
            assert_eq!(Intent::from_label(intent.label()), Some(intent.clone()));
        }
    }
}

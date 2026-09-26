//! `read_symbol` — locate a named symbol via tree-sitter and return its
//! source code with line numbers.
//!
//! Unlike `list_symbols` (which only returns line numbers), this tool returns
//! the **full source text** of the matched symbol, making it the right choice
//! when the agent needs to understand an existing function or struct before
//! modifying it.

use super::read_file::resolve_path;
use super::{Tool, ToolDef, ToolResult};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::Path;
use tree_sitter::{Language, Node, Parser};

pub struct ReadSymbol;

impl Tool for ReadSymbol {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "read_symbol",
            "Find a named symbol (function, struct, class, etc.) in a source file using \
             AST parsing and return its full source code with line numbers. \
             Supported languages: Rust, Python, JavaScript, TypeScript, Go. \
             Use this instead of read_file when you need a specific function or type \
             without reading the whole file.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the source file"
                    },
                    "symbol": {
                        "type": "string",
                        "description": "Name of the symbol to find (e.g. 'UserService', 'handle_request', 'Config')"
                    },
                    "kind": {
                        "type": "string",
                        "description": "Optional: filter by symbol kind (function, struct, class, method, trait, impl, enum, type, const)",
                        "enum": ["function", "struct", "class", "method", "trait", "impl", "enum", "type", "const"]
                    }
                },
                "required": ["path", "symbol"]
            }),
        )
    }

    fn run(&self, args: &Value, cwd: &Path) -> Result<ToolResult> {
        let path_str = args
            .get("path")
            .context("missing 'path'")?
            .as_str()
            .context("'path' must be a string")?;
        let symbol_name = args
            .get("symbol")
            .context("missing 'symbol'")?
            .as_str()
            .context("'symbol' must be a string")?;
        let kind_filter = args
            .get("kind")
            .and_then(|v| v.as_str())
            .map(|s| s.to_lowercase());

        let target = resolve_path(cwd, path_str);
        if !target.exists() {
            bail!("read_symbol: {} does not exist", target.display());
        }

        let ext = target
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        let source = std::fs::read_to_string(&target)
            .with_context(|| format!("failed to read {}", target.display()))?;

        let range = match ext.as_str() {
            "rs" => find_symbol_range(
                &source,
                rust_language(),
                RUST_RULES,
                symbol_name,
                kind_filter.as_deref(),
            ),
            "py" | "pyi" => find_symbol_range(
                &source,
                python_language(),
                PYTHON_RULES,
                symbol_name,
                kind_filter.as_deref(),
            ),
            "js" | "jsx" | "mjs" | "cjs" => find_symbol_range(
                &source,
                js_language(),
                JS_RULES,
                symbol_name,
                kind_filter.as_deref(),
            ),
            "ts" | "tsx" | "mts" | "cts" => find_symbol_range(
                &source,
                ts_language(),
                TS_RULES,
                symbol_name,
                kind_filter.as_deref(),
            ),
            "go" => find_symbol_range(
                &source,
                go_language(),
                GO_RULES,
                symbol_name,
                kind_filter.as_deref(),
            ),
            other => bail!(
                "read_symbol: unsupported file extension '.{other}'. \
                 Supported: .rs .py .js .ts .go"
            ),
        };

        match range {
            Ok(Some((start_line, end_line, kind))) => {
                let lines: Vec<&str> = source.lines().collect();
                let start_idx = (start_line - 1).min(lines.len());
                let end_idx = end_line.min(lines.len());

                // Format with line numbers
                let numbered: String = lines[start_idx..end_idx]
                    .iter()
                    .enumerate()
                    .map(|(i, line)| format!("{:4} | {}\n", start_idx + i + 1, line))
                    .collect();

                Ok(ToolResult {
                    content: format!(
                        "=== {} `{}` in {} (lines {}..{}) ===\n{}",
                        kind,
                        symbol_name,
                        target.display(),
                        start_line,
                        end_line,
                        numbered
                    ),
                })
            }
            Ok(None) => {
                // Tree-sitter didn't find it — fall back to a simple text search
                // so the agent still gets useful output even for generated code
                // or unusual syntax.
                match grep_fallback(&source, &target, symbol_name) {
                    Some(result) => Ok(result),
                    None => Ok(ToolResult {
                        content: format!(
                            "Symbol `{symbol_name}` not found in {}{}",
                            target.display(),
                            kind_filter
                                .map(|k| format!(" (kind={k})"))
                                .unwrap_or_default()
                        ),
                    }),
                }
            }
            Err(e) => Err(e),
        }
    }
}

// ─── Symbol range record ──────────────────────────────────────────────────────

/// Start line, end line (both 1-indexed, inclusive), and kind label.
type SymbolRange = (usize, usize, String);

// ─── Extraction rules ─────────────────────────────────────────────────────────

struct Rule {
    node_type: &'static str,
    kind: &'static str,
    name_field: &'static str,
}

const RUST_RULES: &[Rule] = &[
    Rule { node_type: "function_item", kind: "function", name_field: "name" },
    Rule { node_type: "struct_item", kind: "struct", name_field: "name" },
    Rule { node_type: "enum_item", kind: "enum", name_field: "name" },
    Rule { node_type: "trait_item", kind: "trait", name_field: "name" },
    Rule { node_type: "impl_item", kind: "impl", name_field: "type" },
    Rule { node_type: "type_item", kind: "type", name_field: "name" },
    Rule { node_type: "const_item", kind: "const", name_field: "name" },
    Rule { node_type: "static_item", kind: "static", name_field: "name" },
    Rule { node_type: "mod_item", kind: "mod", name_field: "name" },
    Rule { node_type: "macro_definition", kind: "macro", name_field: "name" },
];

const PYTHON_RULES: &[Rule] = &[
    Rule { node_type: "function_definition", kind: "function", name_field: "name" },
    Rule { node_type: "async_function_definition", kind: "function", name_field: "name" },
    Rule { node_type: "class_definition", kind: "class", name_field: "name" },
];

const JS_RULES: &[Rule] = &[
    Rule { node_type: "function_declaration", kind: "function", name_field: "name" },
    Rule { node_type: "generator_function_declaration", kind: "function", name_field: "name" },
    Rule { node_type: "class_declaration", kind: "class", name_field: "name" },
    Rule { node_type: "method_definition", kind: "method", name_field: "name" },
];

const TS_RULES: &[Rule] = &[
    Rule { node_type: "function_declaration", kind: "function", name_field: "name" },
    Rule { node_type: "generator_function_declaration", kind: "function", name_field: "name" },
    Rule { node_type: "class_declaration", kind: "class", name_field: "name" },
    Rule { node_type: "method_definition", kind: "method", name_field: "name" },
    Rule { node_type: "interface_declaration", kind: "interface", name_field: "name" },
    Rule { node_type: "type_alias_declaration", kind: "type", name_field: "name" },
    Rule { node_type: "enum_declaration", kind: "enum", name_field: "name" },
    Rule { node_type: "abstract_class_declaration", kind: "class", name_field: "name" },
];

const GO_RULES: &[Rule] = &[
    Rule { node_type: "function_declaration", kind: "function", name_field: "name" },
    Rule { node_type: "method_declaration", kind: "method", name_field: "name" },
    Rule { node_type: "type_declaration", kind: "type", name_field: "name" },
];

// ─── Language constructors ────────────────────────────────────────────────────

fn rust_language() -> Language {
    tree_sitter_rust::LANGUAGE.into()
}
fn python_language() -> Language {
    tree_sitter_python::LANGUAGE.into()
}
fn js_language() -> Language {
    tree_sitter_javascript::LANGUAGE.into()
}
fn ts_language() -> Language {
    tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
}
fn go_language() -> Language {
    tree_sitter_go::LANGUAGE.into()
}

// ─── Core finder ──────────────────────────────────────────────────────────────

fn find_symbol_range(
    source: &str,
    language: Language,
    rules: &[Rule],
    symbol_name: &str,
    kind_filter: Option<&str>,
) -> Result<Option<SymbolRange>> {
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .context("failed to set tree-sitter language")?;

    let tree = parser
        .parse(source, None)
        .context("tree-sitter failed to parse source")?;

    let source_bytes = source.as_bytes();
    let mut found: Option<SymbolRange> = None;

    walk_for_symbol(
        tree.root_node(),
        source_bytes,
        rules,
        symbol_name,
        kind_filter,
        &mut found,
    );

    Ok(found)
}

fn walk_for_symbol(
    node: Node<'_>,
    source: &[u8],
    rules: &[Rule],
    symbol_name: &str,
    kind_filter: Option<&str>,
    found: &mut Option<SymbolRange>,
) {
    // Stop once we've found a match — we want the first/most prominent one.
    if found.is_some() {
        return;
    }

    let node_type = node.kind();

    for rule in rules {
        if node_type == rule.node_type {
            // Apply kind filter if present.
            if let Some(filter) = kind_filter {
                if rule.kind != filter {
                    break;
                }
            }

            // Extract the symbol name from this node.
            let name = extract_name(&node, source, rule);
            if name == symbol_name {
                // Lines are 0-indexed in tree-sitter; convert to 1-indexed.
                let start_line = node.start_position().row + 1;
                let end_line = node.end_position().row + 1;
                *found = Some((start_line, end_line, rule.kind.to_string()));
                return;
            }
            // Name didn't match — still recurse into children (e.g. nested fns).
            break;
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_for_symbol(child, source, rules, symbol_name, kind_filter, found);
        if found.is_some() {
            return;
        }
    }
}

fn extract_name(node: &Node<'_>, source: &[u8], rule: &Rule) -> String {
    if let Some(name_node) = node.child_by_field_name(rule.name_field)
        && let Ok(text) = name_node.utf8_text(source)
    {
        let clean = text.split('<').next().unwrap_or(text).trim();
        return clean.to_string();
    }

    // Fallback: first child that looks like an identifier.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let kind = child.kind();
        if (kind == "identifier"
            || kind == "type_identifier"
            || kind == "field_identifier"
            || kind == "property_identifier")
            && let Ok(text) = child.utf8_text(source)
        {
            return text.to_string();
        }
    }

    "(anonymous)".to_string()
}

// ─── Grep fallback ────────────────────────────────────────────────────────────

/// Simple text-search fallback when tree-sitter can't locate the symbol.
/// Returns a window of ±5 lines around the first matching line.
fn grep_fallback(source: &str, path: &Path, symbol_name: &str) -> Option<ToolResult> {
    let lines: Vec<&str> = source.lines().collect();
    let idx = lines
        .iter()
        .position(|line| line.contains(symbol_name))?;

    let window_start = idx.saturating_sub(2);
    let window_end = (idx + 20).min(lines.len());

    let numbered: String = lines[window_start..window_end]
        .iter()
        .enumerate()
        .map(|(i, line)| format!("{:4} | {}\n", window_start + i + 1, line))
        .collect();

    Some(ToolResult {
        content: format!(
            "=== `{}` in {} (grep fallback, lines {}..{}) ===\n{}",
            symbol_name,
            path.display(),
            window_start + 1,
            window_end,
            numbered
        ),
    })
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn find(source: &str, lang: Language, rules: &[Rule], name: &str, kind: Option<&str>) -> Option<SymbolRange> {
        find_symbol_range(source, lang, rules, name, kind)
            .expect("tree-sitter failed")
    }

    // ── Rust ──────────────────────────────────────────────────────────────────

    #[test]
    fn rust_finds_function() {
        let src = r#"
fn alpha() {}

fn beta(x: i32) -> i32 {
    x + 1
}

struct Foo;
"#;
        let r = find(src, rust_language(), RUST_RULES, "beta", None).unwrap();
        // start_line should be line 4 (where `fn beta` is)
        assert!(r.0 >= 4, "start_line={}", r.0);
        assert!(r.1 >= r.0, "end_line < start_line");
        assert_eq!(r.2, "function");
    }

    #[test]
    fn rust_finds_struct() {
        let src = "pub struct UserService { field: i32 }";
        let r = find(src, rust_language(), RUST_RULES, "UserService", None).unwrap();
        assert_eq!(r.2, "struct");
    }

    #[test]
    fn rust_finds_impl() {
        let src = r#"
impl UserService {
    pub fn new() -> Self { Self { field: 0 } }
}
"#;
        let r = find(src, rust_language(), RUST_RULES, "UserService", Some("impl")).unwrap();
        assert_eq!(r.2, "impl");
    }

    #[test]
    fn rust_not_found_returns_none() {
        let src = "fn alpha() {}";
        let r = find(src, rust_language(), RUST_RULES, "nonexistent", None);
        assert!(r.is_none());
    }

    #[test]
    fn rust_kind_filter_excludes_wrong_kind() {
        let src = r#"
fn alpha() {}
struct Alpha {}
"#;
        // Search for "Alpha" but filter for function only — should not match struct.
        let r = find(src, rust_language(), RUST_RULES, "Alpha", Some("function"));
        assert!(r.is_none());
    }

    // ── Python ────────────────────────────────────────────────────────────────

    #[test]
    fn python_finds_class() {
        let src = r#"
class UserService:
    def __init__(self):
        pass

    def get_user(self):
        return None
"#;
        let r = find(src, python_language(), PYTHON_RULES, "UserService", None).unwrap();
        assert_eq!(r.2, "class");
        assert!(r.1 > r.0, "class should span multiple lines");
    }

    #[test]
    fn python_finds_function() {
        let src = r#"
def handle_request(req):
    return req
"#;
        let r = find(src, python_language(), PYTHON_RULES, "handle_request", None).unwrap();
        assert_eq!(r.2, "function");
    }

    // ── Tool integration ──────────────────────────────────────────────────────

    #[test]
    fn tool_returns_source_with_line_numbers() {
        use tempfile::NamedTempFile;
        use std::io::Write;

        let mut f = NamedTempFile::with_suffix(".rs").unwrap();
        writeln!(f, "fn foo() {{}}").unwrap();
        writeln!(f, "fn bar() {{ let x = 1; }}").unwrap();

        let tool = ReadSymbol;
        let args = serde_json::json!({ "path": f.path().to_str().unwrap(), "symbol": "bar" });
        let result = tool.run(&args, std::path::Path::new("/")).unwrap();
        assert!(result.content.contains("bar"), "content: {}", result.content);
        assert!(result.content.contains("|"), "should have line numbers");
    }

    #[test]
    fn tool_returns_not_found_for_missing_symbol() {
        use tempfile::NamedTempFile;
        use std::io::Write;

        let mut f = NamedTempFile::with_suffix(".rs").unwrap();
        writeln!(f, "fn foo() {{}}").unwrap();

        let tool = ReadSymbol;
        let args = serde_json::json!({ "path": f.path().to_str().unwrap(), "symbol": "nonexistent" });
        let result = tool.run(&args, std::path::Path::new("/")).unwrap();
        assert!(result.content.contains("not found") || result.content.contains("nonexistent"));
    }

    #[test]
    fn tool_fails_for_missing_file() {
        let tool = ReadSymbol;
        let args = serde_json::json!({ "path": "/nonexistent/file.rs", "symbol": "foo" });
        assert!(tool.run(&args, std::path::Path::new("/")).is_err());
    }

    #[test]
    fn tool_fails_for_unsupported_extension() {
        use tempfile::NamedTempFile;
        use std::io::Write;

        let mut f = NamedTempFile::with_suffix(".cpp").unwrap();
        writeln!(f, "void foo() {{}}").unwrap();

        let tool = ReadSymbol;
        let args = serde_json::json!({ "path": f.path().to_str().unwrap(), "symbol": "foo" });
        assert!(tool.run(&args, std::path::Path::new("/")).is_err());
    }
}

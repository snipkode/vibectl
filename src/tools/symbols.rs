use super::read_file::resolve_path;
use super::{Tool, ToolDef, ToolResult};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::Path;
use tree_sitter::{Language, Node, Parser};

pub struct ListSymbols;

impl Tool for ListSymbols {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "list_symbols",
            // The supported-language sentence is generated from the registry.
            // Hand-maintaining it meant the description could promise a language
            // the dispatch below rejects.
            &format!(
                "List top-level symbols (functions, structs, classes, methods, constants, etc.)\n\
                 in a source file using AST parsing. Supported languages: {}.\n\
                 Detected automatically from the file extension.\n\n\
                 Returns each symbol as:  <kind> <name>  (line <n>)\n\n\
                 Use this before editing a file to understand its structure without reading every \
                 line.\nThe `kind` filter is optional \u{2014} omit it to list all symbols.",
                crate::langs::ast_lang_list()
            ),
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the source file to inspect"
                    },
                    "kind": {
                        "type": "string",
                        "description": "Optional filter: one of 'function', 'struct', 'class', 'method', 'const', 'trait', 'impl', 'type'",
                        "enum": ["function", "struct", "class", "method", "const", "trait", "impl", "type"]
                    }
                },
                "required": ["path"]
            }),
        )
    }

    fn run(&self, args: &Value, cwd: &Path) -> Result<ToolResult> {
        let path_str = args
            .get("path")
            .context("missing 'path'")?
            .as_str()
            .context("'path' must be a string")?;
        let kind_filter = args
            .get("kind")
            .and_then(|v| v.as_str())
            .map(|s| s.to_lowercase());

        let target = resolve_path(cwd, path_str);
        if !target.exists() {
            bail!("list_symbols: {} does not exist", target.display());
        }

        let ext = target
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        let source = std::fs::read_to_string(&target)
            .with_context(|| format!("failed to read {}", target.display()))?;

        let symbols = match ext.as_str() {
            "rs" => extract_symbols(&source, rust_language(), RUST_RULES, kind_filter.as_deref()),
            // .pyi is a type-stub file; the Python grammar parses it fine.
            "py" | "pyi" => extract_symbols(
                &source,
                python_language(),
                PYTHON_RULES,
                kind_filter.as_deref(),
            ),
            "js" | "jsx" | "mjs" | "cjs" => {
                extract_symbols(&source, js_language(), JS_RULES, kind_filter.as_deref())
            }
            // .mts/.cts are the ESM/CommonJS TypeScript variants.
            "ts" | "tsx" | "mts" | "cts" => {
                extract_symbols(&source, ts_language(), TS_RULES, kind_filter.as_deref())
            }
            "go" => extract_symbols(&source, go_language(), GO_RULES, kind_filter.as_deref()),
            other => {
                // The supported list is generated from the registry, so it
                // cannot advertise a language the match above does not handle.
                bail!(
                    "list_symbols: unsupported file extension '.{other}'. \
                     Supported: {}",
                    crate::langs::ast_ext_list()
                )
            }
        }?;

        if symbols.is_empty() {
            return Ok(ToolResult {
                content: format!(
                    "No symbols found in {}{}",
                    target.display(),
                    kind_filter
                        .map(|k| format!(" (kind={k})"))
                        .unwrap_or_default()
                ),
            });
        }

        let header = format!(
            "Symbols in {}{}:\n",
            target.display(),
            kind_filter
                .map(|k| format!(" (kind={k})"))
                .unwrap_or_default()
        );
        let body: String = symbols
            .iter()
            .map(|s| format!("  {:10} {}  (line {})\n", s.kind, s.name, s.line))
            .collect();

        Ok(ToolResult {
            content: format!("{header}{body}"),
        })
    }
}

// ─── Symbol record ────────────────────────────────────────────────────────────

#[derive(Debug)]
struct Symbol {
    kind: String,
    name: String,
    line: usize, // 1-indexed
}

// ─── Extraction rules ─────────────────────────────────────────────────────────

/// Describes how to extract one symbol kind from a tree-sitter node.
struct Rule {
    /// The tree-sitter node type to match (e.g. "function_item").
    node_type: &'static str,
    /// The canonical kind label to emit (e.g. "function").
    kind: &'static str,
    /// Child node field name that holds the identifier (e.g. "name").
    name_field: &'static str,
}

const RUST_RULES: &[Rule] = &[
    Rule {
        node_type: "function_item",
        kind: "function",
        name_field: "name",
    },
    Rule {
        node_type: "struct_item",
        kind: "struct",
        name_field: "name",
    },
    Rule {
        node_type: "enum_item",
        kind: "enum",
        name_field: "name",
    },
    Rule {
        node_type: "trait_item",
        kind: "trait",
        name_field: "name",
    },
    Rule {
        node_type: "impl_item",
        kind: "impl",
        name_field: "type",
    },
    Rule {
        node_type: "type_item",
        kind: "type",
        name_field: "name",
    },
    Rule {
        node_type: "const_item",
        kind: "const",
        name_field: "name",
    },
    Rule {
        node_type: "static_item",
        kind: "static",
        name_field: "name",
    },
    Rule {
        node_type: "mod_item",
        kind: "mod",
        name_field: "name",
    },
    Rule {
        node_type: "macro_definition",
        kind: "macro",
        name_field: "name",
    },
];

const PYTHON_RULES: &[Rule] = &[
    Rule {
        node_type: "function_definition",
        kind: "function",
        name_field: "name",
    },
    Rule {
        node_type: "async_function_definition",
        kind: "function",
        name_field: "name",
    },
    Rule {
        node_type: "class_definition",
        kind: "class",
        name_field: "name",
    },
    Rule {
        node_type: "decorated_definition",
        kind: "decorated",
        name_field: "definition",
    },
];

const JS_RULES: &[Rule] = &[
    Rule {
        node_type: "function_declaration",
        kind: "function",
        name_field: "name",
    },
    Rule {
        node_type: "generator_function_declaration",
        kind: "function",
        name_field: "name",
    },
    Rule {
        node_type: "class_declaration",
        kind: "class",
        name_field: "name",
    },
    Rule {
        node_type: "method_definition",
        kind: "method",
        name_field: "name",
    },
    Rule {
        node_type: "lexical_declaration",
        kind: "const",
        name_field: "name",
    },
    Rule {
        node_type: "variable_declaration",
        kind: "var",
        name_field: "name",
    },
];

const TS_RULES: &[Rule] = &[
    Rule {
        node_type: "function_declaration",
        kind: "function",
        name_field: "name",
    },
    Rule {
        node_type: "generator_function_declaration",
        kind: "function",
        name_field: "name",
    },
    Rule {
        node_type: "class_declaration",
        kind: "class",
        name_field: "name",
    },
    Rule {
        node_type: "method_definition",
        kind: "method",
        name_field: "name",
    },
    Rule {
        node_type: "interface_declaration",
        kind: "interface",
        name_field: "name",
    },
    Rule {
        node_type: "type_alias_declaration",
        kind: "type",
        name_field: "name",
    },
    Rule {
        node_type: "enum_declaration",
        kind: "enum",
        name_field: "name",
    },
    Rule {
        node_type: "lexical_declaration",
        kind: "const",
        name_field: "name",
    },
    Rule {
        node_type: "abstract_class_declaration",
        kind: "class",
        name_field: "name",
    },
];

const GO_RULES: &[Rule] = &[
    Rule {
        node_type: "function_declaration",
        kind: "function",
        name_field: "name",
    },
    Rule {
        node_type: "method_declaration",
        kind: "method",
        name_field: "name",
    },
    Rule {
        node_type: "type_declaration",
        kind: "type",
        name_field: "name",
    },
    Rule {
        node_type: "const_declaration",
        kind: "const",
        name_field: "name",
    },
    Rule {
        node_type: "var_declaration",
        kind: "var",
        name_field: "name",
    },
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

// ─── Core extractor ───────────────────────────────────────────────────────────

fn extract_symbols(
    source: &str,
    language: Language,
    rules: &[Rule],
    kind_filter: Option<&str>,
) -> Result<Vec<Symbol>> {
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .context("failed to set tree-sitter language")?;

    let tree = parser
        .parse(source, None)
        .context("tree-sitter failed to parse source")?;

    let source_bytes = source.as_bytes();
    let mut symbols = Vec::new();

    // Walk the tree, collecting matching nodes.
    walk_tree(
        tree.root_node(),
        source_bytes,
        rules,
        kind_filter,
        &mut symbols,
    );

    // Sort by line number.
    symbols.sort_by_key(|s| s.line);
    Ok(symbols)
}

fn walk_tree(
    node: Node<'_>,
    source: &[u8],
    rules: &[Rule],
    kind_filter: Option<&str>,
    out: &mut Vec<Symbol>,
) {
    let node_type = node.kind();

    for rule in rules {
        if node_type == rule.node_type {
            if let Some(filter) = kind_filter {
                // Normalise: "function" matches both "function" and "method" if filter is "method"
                if rule.kind != filter {
                    // also allow "function" to match async_function_definition etc.
                    // handled by the rule's kind label matching filter exactly.
                    break;
                }
            }

            let name = extract_name(&node, source, rule);
            let line = node.start_position().row + 1; // 1-indexed
            out.push(Symbol {
                kind: rule.kind.to_string(),
                name,
                line,
            });
            // Don't recurse into matched node's children for top-level symbols.
            return;
        }
    }

    // Recurse into children.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_tree(child, source, rules, kind_filter, out);
    }
}

/// Extract the name text from a node using the rule's name_field.
/// Falls back to the node text itself or "(anonymous)" if nothing found.
fn extract_name(node: &Node<'_>, source: &[u8], rule: &Rule) -> String {
    // Try named child by field name.
    if let Some(name_node) = node.child_by_field_name(rule.name_field)
        && let Ok(text) = name_node.utf8_text(source)
    {
        // For impl blocks the "type" field may be a complex path — truncate at '<'.
        let clean = text.split('<').next().unwrap_or(text).trim();
        return clean.to_string();
    }

    // Fallback: first named child that looks like an identifier.
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

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn syms(source: &str, lang: Language, rules: &[Rule], filter: Option<&str>) -> Vec<Symbol> {
        extract_symbols(source, lang, rules, filter).expect("extract failed")
    }

    // ── Rust ──────────────────────────────────────────────────────────────────

    #[test]
    fn rust_functions() {
        let src = r#"
fn alpha() {}
fn beta(x: i32) -> i32 { x }
struct Foo;
"#;
        let s = syms(src, rust_language(), RUST_RULES, Some("function"));
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].name, "alpha");
        assert_eq!(s[1].name, "beta");
    }

    #[test]
    fn rust_structs_and_traits() {
        let src = r#"
pub struct MyStruct { x: i32 }
pub trait MyTrait { fn foo(&self); }
"#;
        let s = syms(src, rust_language(), RUST_RULES, None);
        let kinds: Vec<&str> = s.iter().map(|x| x.kind.as_str()).collect();
        assert!(kinds.contains(&"struct"));
        assert!(kinds.contains(&"trait"));
    }

    #[test]
    fn rust_impl_block() {
        let src = "impl MyStruct { fn new() -> Self { Self {} } }";
        let s = syms(src, rust_language(), RUST_RULES, Some("impl"));
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].name, "MyStruct");
    }

    // ── Python ────────────────────────────────────────────────────────────────

    #[test]
    fn python_functions_and_classes() {
        let src = r#"
def foo():
    pass

class Bar:
    def method(self):
        pass

async def baz():
    pass
"#;
        let all = syms(src, python_language(), PYTHON_RULES, None);
        let names: Vec<&str> = all.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"Bar"));
        assert!(names.contains(&"baz"));
    }

    // ── JavaScript ────────────────────────────────────────────────────────────

    #[test]
    fn js_functions_and_classes() {
        let src = r#"
function greet(name) { return name; }
class Animal { constructor() {} }
"#;
        let all = syms(src, js_language(), JS_RULES, None);
        let names: Vec<&str> = all.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"greet"));
        assert!(names.contains(&"Animal"));
    }

    // ── Go ────────────────────────────────────────────────────────────────────

    #[test]
    fn go_functions() {
        let src = r#"
package main

func Hello() string { return "hi" }
func Add(a, b int) int { return a + b }
"#;
        let s = syms(src, go_language(), GO_RULES, Some("function"));
        assert_eq!(s.len(), 2);
        let names: Vec<&str> = s.iter().map(|x| x.name.as_str()).collect();
        assert!(names.contains(&"Hello"));
        assert!(names.contains(&"Add"));
    }

    // ── Line numbers ──────────────────────────────────────────────────────────

    #[test]
    fn line_numbers_are_correct() {
        let src = "fn a() {}\n\nfn b() {}\n";
        let s = syms(src, rust_language(), RUST_RULES, Some("function"));
        assert_eq!(s[0].line, 1);
        assert_eq!(s[1].line, 3);
    }

    // ── Kind filter ───────────────────────────────────────────────────────────

    #[test]
    fn kind_filter_excludes_others() {
        let src = "fn f() {}\nstruct S;\n";
        let s = syms(src, rust_language(), RUST_RULES, Some("struct"));
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].kind, "struct");
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use serde_json::json;

    /// A syntactically valid, near-empty snippet per language. Only needs to
    /// parse; the test cares about which grammar was selected, not the output.
    fn sample(lang_name: &str) -> &'static str {
        match lang_name {
            "Rust" => "fn main() {}",
            "Python" => "def main():\n    pass\n",
            "JavaScript" => "function main() {}\n",
            "TypeScript" => "function main(): void {}\n",
            "Go" => "package main\n\nfunc main() {}\n",
            other => panic!("no sample for {other}; add one when adding a grammar"),
        }
    }

    /// Every extension the registry claims `ast: true` must actually be handled
    /// by the dispatch above.
    ///
    /// This is the test that stops the two from drifting: the registry is a
    /// table, the dispatch is a `match`, and they are written in different
    /// places. Asserting on behaviour (does a real call get past the
    /// "unsupported extension" guard?) rather than on a second hand-written
    /// list means adding a grammar in one place and forgetting the other fails
    /// here instead of silently in production.
    #[test]
    fn every_extension_the_registry_marks_as_ast_really_dispatches() {
        let tool = ListSymbols;
        let mut checked = 0;

        for lang in crate::langs::LANGS {
            if !lang.ast {
                continue;
            }
            for ext in lang.exts {
                let dir = tempfile::tempdir().expect("tempdir");
                let file = dir.path().join(format!("probe.{ext}"));
                std::fs::write(&file, sample(lang.name)).expect("write probe");

                let result = tool.run(&json!({ "path": file.to_str().unwrap() }), dir.path());
                checked += 1;

                if let Err(e) = result {
                    let msg = e.to_string();
                    assert!(
                        !msg.contains("unsupported file extension"),
                        "registry marks .{ext} as ast, but the dispatch rejects it: {msg}"
                    );
                }
            }
        }

        assert!(
            checked >= 5,
            "expected the known grammars, checked {checked}"
        );
    }

    /// The inverse: an extension nobody claims must be refused, and the error
    /// must list what is actually supported.
    #[test]
    fn an_unclaimed_extension_is_refused_with_a_generated_list() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("probe.png");
        std::fs::write(&file, "not source").expect("write probe");

        let err = match ListSymbols.run(&json!({ "path": file.to_str().unwrap() }), dir.path()) {
            Ok(_) => panic!("a PNG must not produce symbols"),
            Err(e) => e.to_string(),
        };

        assert!(
            err.contains("unsupported file extension '.png'"),
            "got: {err}"
        );
        assert!(
            err.contains(&crate::langs::ast_ext_list()),
            "the supported list should come from the registry: {err}"
        );
    }

    #[test]
    fn a_real_rust_file_yields_its_symbols() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("lib.rs");
        std::fs::write(
            &file,
            "pub struct Config;\n\npub fn load() -> Config { Config }\n\ntrait T {}\n",
        )
        .expect("write");

        let out = ListSymbols
            .run(&json!({ "path": file.to_str().unwrap() }), dir.path())
            .expect("parse")
            .content;

        assert!(out.contains("Config"), "got: {out}");
        assert!(out.contains("load"), "got: {out}");
        assert!(out.contains("struct"), "got: {out}");
    }

    #[test]
    fn the_kind_filter_narrows_the_result() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("lib.rs");
        std::fs::write(&file, "pub struct OnlyAStruct;\n").expect("write");

        let out = ListSymbols
            .run(
                &json!({ "path": file.to_str().unwrap(), "kind": "function" }),
                dir.path(),
            )
            .expect("parse")
            .content;
        assert!(
            !out.contains("OnlyAStruct"),
            "a struct must not match kind=function: {out}"
        );
    }
}

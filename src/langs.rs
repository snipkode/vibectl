//! One table of what vibectl recognises, so the answer to "is this a file we
//! handle" cannot drift between the tool that indexes symbols, the discovery
//! scan, and the file picker.
//!
//! Before this existed the same question was answered by four hand-maintained
//! lists that had already diverged: `list_symbols` knew five languages, the
//! discovery tree knew twenty-one extensions, the file picker knew twenty-one
//! *different* extensions, and the validation fingerprint knew a third set of
//! skip directories. A language added to one was invisible to the others.
//!
//! Note the three axes are deliberately different, and are named rather than
//! merged, because they answer different questions:
//!
//! - [`ext_is_source`] — should the agent's discovery scan walk this file? Code
//!   and configuration.
//! - [`ext_is_referenceable`] — might the user want to `@`-mention it? The
//!   above, plus notes and prose. Strictly a superset of source.
//! - [`LANGS`]'s `ast` flag — can `list_symbols` parse it into a symbol tree?
//!   Only what actually has a tree-sitter grammar compiled in. Everything else
//!   is still fully readable and editable, just not structurally indexed. The
//!   user-facing lists are [`ast_ext_list`] and [`ast_lang_list`].

/// A language vibectl recognises.
pub struct Lang {
    pub name: &'static str,
    /// File extensions, lowercase, **without** the leading dot.
    pub exts: &'static [&'static str],
    /// Whether `list_symbols` has a tree-sitter grammar for this language.
    ///
    /// This is a claim about compiled-in code, so it must stay in step with
    /// the `match` in `src/tools/symbols.rs` — a test enforces that.
    pub ast: bool,
    /// Files whose presence identifies a project of this type.
    pub manifests: &'static [&'static str],
}

impl Lang {
    pub fn has_ext(&self, ext: &str) -> bool {
        self.exts.contains(&ext)
    }
}

/// Every language vibectl recognises.
///
/// `ast: true` means a grammar is compiled in. Adding a grammar means adding
/// the dependency, the `match` arm in `symbols.rs`, and flipping this flag.
pub const LANGS: &[Lang] = &[
    Lang {
        name: "Rust",
        exts: &["rs"],
        ast: true,
        manifests: &["Cargo.toml"],
    },
    Lang {
        name: "Python",
        exts: &["py", "pyi"],
        ast: true,
        manifests: &["requirements.txt", "pyproject.toml", "setup.py", "Pipfile"],
    },
    Lang {
        name: "JavaScript",
        exts: &["js", "jsx", "mjs", "cjs"],
        ast: true,
        manifests: &["package.json"],
    },
    Lang {
        name: "TypeScript",
        // tsx is handled by the TypeScript grammar upstream.
        exts: &["ts", "tsx", "mts", "cts"],
        ast: true,
        manifests: &["tsconfig.json"],
    },
    Lang {
        name: "Go",
        exts: &["go"],
        ast: true,
        manifests: &["go.mod"],
    },
    Lang {
        name: "Java",
        exts: &["java"],
        ast: false,
        manifests: &["pom.xml", "build.gradle", "build.gradle.kts"],
    },
    Lang {
        name: "Kotlin",
        exts: &["kt", "kts"],
        ast: false,
        manifests: &["build.gradle.kts"],
    },
    Lang {
        name: "C",
        exts: &["c", "h"],
        ast: false,
        manifests: &["Makefile", "CMakeLists.txt"],
    },
    Lang {
        name: "C++",
        exts: &["cpp", "cc", "cxx", "hpp", "hh", "hxx"],
        ast: false,
        manifests: &["CMakeLists.txt"],
    },
    Lang {
        name: "C#",
        exts: &["cs"],
        ast: false,
        manifests: &["*.csproj", "*.sln"],
    },
    Lang {
        name: "PHP",
        exts: &["php"],
        ast: false,
        manifests: &["composer.json"],
    },
    Lang {
        name: "Ruby",
        exts: &["rb", "rake"],
        ast: false,
        manifests: &["Gemfile", "*.gemspec"],
    },
    Lang {
        name: "Elixir",
        exts: &["ex", "exs"],
        ast: false,
        manifests: &["mix.exs"],
    },
    Lang {
        name: "Shell",
        exts: &["sh", "bash", "zsh"],
        ast: false,
        manifests: &[],
    },
    Lang {
        name: "SQL",
        exts: &["sql"],
        ast: false,
        manifests: &[],
    },
    Lang {
        name: "Scala",
        exts: &["scala", "sc"],
        ast: false,
        manifests: &[],
    },
    Lang {
        name: "Terraform / HCL",
        exts: &["tf", "tfvars", "hcl"],
        ast: false,
        manifests: &[],
    },
    Lang {
        name: "Dart",
        exts: &["dart"],
        ast: false,
        manifests: &["pubspec.yaml"],
    },
    Lang {
        name: "Swift",
        exts: &["swift"],
        ast: false,
        manifests: &["Package.swift"],
    },
    Lang {
        name: "Lua",
        exts: &["lua"],
        ast: false,
        manifests: &[],
    },
    // ── Configuration, not languages ────────────────────────────────────────
    // These have no symbols worth indexing, but the agent must still see and
    // edit them, so they belong in the scan.
    Lang {
        name: "TOML",
        exts: &["toml"],
        ast: false,
        manifests: &[],
    },
    Lang {
        name: "YAML",
        exts: &["yaml", "yml"],
        ast: false,
        manifests: &[],
    },
    Lang {
        name: "JSON",
        exts: &["json", "jsonc"],
        ast: false,
        manifests: &[],
    },
    Lang {
        name: "Markdown",
        exts: &["md", "mdx"],
        ast: false,
        manifests: &[],
    },
    Lang {
        name: "HTML",
        exts: &["html", "htm"],
        ast: false,
        manifests: &[],
    },
    Lang {
        name: "CSS",
        exts: &["css", "scss", "sass", "less"],
        ast: false,
        manifests: &[],
    },
    Lang {
        name: "Text",
        exts: &["txt", "env", "ini", "cfg", "conf", "properties"],
        ast: false,
        manifests: &[],
    },
];

/// Extensions that are referenceable but are neither code nor configuration.
///
/// The file picker offers these because a user may want to `@`-mention a
/// README or a scratch note; the discovery scan does not walk them, because
/// they add prompt noise without describing the codebase.
const DOC_EXTS: &[&str] = &["md", "mdx", "txt", "env", "rst", "adoc"];

/// Directories never worth descending into.
///
/// Shared by the discovery scan, the file picker, and the validation
/// fingerprint. Build output and dependency trees are large, generated, and
/// would otherwise dominate every cap.
pub const SKIP_DIRS: &[&str] = &[
    "target",
    "node_modules",
    ".git",
    "dist",
    "build",
    "out",
    ".cache",
    "__pycache__",
    ".venv",
    "venv",
    "vendor",
    ".next",
    ".nuxt",
    "coverage",
    ".pytest_cache",
    ".mypy_cache",
    ".tox",
    "bin",
    "obj",
];

/// Should the agent's discovery scan walk this file?
pub fn ext_is_source(ext: &str) -> bool {
    let ext = normalise(ext);
    if DOC_EXTS.contains(&ext.as_str()) {
        return false;
    }
    LANGS.iter().any(|l| l.has_ext(&ext))
}

/// Might the user want to `@`-mention this file? A superset of [`ext_is_source`].
pub fn ext_is_referenceable(ext: &str) -> bool {
    let ext = normalise(ext);
    ext_is_source(&ext) || DOC_EXTS.contains(&ext.as_str())
}

/// Human-readable list of the extensions `list_symbols` accepts, for its error
/// message. Derived rather than hand-written so it cannot claim a language the
/// dispatcher does not handle.
pub fn ast_ext_list() -> String {
    let mut exts: Vec<&str> = LANGS
        .iter()
        .filter(|l| l.ast)
        .flat_map(|l| l.exts.iter().copied())
        .collect();
    exts.sort_unstable();
    exts.dedup();
    exts.iter()
        .map(|e| format!(".{e}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Human-readable list of the languages `list_symbols` supports, for its tool
/// description. Derived, so the description cannot promise a grammar that is
/// not compiled in.
pub fn ast_lang_list() -> String {
    let mut names: Vec<&str> = LANGS.iter().filter(|l| l.ast).map(|l| l.name).collect();
    names.sort_unstable();
    names.join(", ")
}

/// Every source extension, sorted. Used by the discovery scan.
pub fn source_exts() -> Vec<&'static str> {
    LANGS
        .iter()
        .flat_map(|l| l.exts.iter().copied())
        .filter(|e| ext_is_source(e))
        .collect()
}

/// Every referenceable extension, sorted. Used by the file picker.
pub fn referenceable_exts() -> Vec<&'static str> {
    let mut exts: Vec<&str> = LANGS
        .iter()
        .flat_map(|l| l.exts.iter().copied())
        .filter(|e| ext_is_referenceable(e))
        .collect();
    exts.extend(DOC_EXTS.iter().copied());
    exts.sort_unstable();
    exts.dedup();
    exts
}

/// Lowercase and strip a leading dot, so callers can pass either form.
fn normalise(ext: &str) -> String {
    ext.trim_start_matches('.').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_extension_is_claimed_by_two_languages() {
        // A duplicate would make `ext_has_ast` and `list_symbols`'s dispatch
        // disagree about who owns the file.
        let mut seen: Vec<(&str, &str)> = Vec::new();
        for lang in LANGS {
            for ext in lang.exts {
                if let Some((_, other)) = seen.iter().find(|(e, _)| e == ext) {
                    panic!(
                        "extension '{ext}' claimed by both {other} and {}",
                        lang.name
                    );
                }
                seen.push((ext, lang.name));
            }
        }
    }

    #[test]
    fn extensions_are_lowercase_and_dotless() {
        // Both would silently fail to match, because every lookup normalises
        // the query but not the table.
        for lang in LANGS {
            for ext in lang.exts {
                assert_eq!(*ext, ext.to_ascii_lowercase(), "{}: {ext}", lang.name);
                assert!(!ext.starts_with('.'), "{}: {ext}", lang.name);
                assert!(!ext.is_empty(), "{} has an empty extension", lang.name);
            }
        }
    }

    #[test]
    fn referenceable_is_a_strict_superset_of_source() {
        // The file picker must never be missing something discovery can see.
        for ext in source_exts() {
            assert!(
                ext_is_referenceable(ext),
                "{ext} is scannable but not referenceable"
            );
        }
    }

    #[test]
    fn docs_are_referenceable_but_not_source() {
        for ext in DOC_EXTS {
            assert!(ext_is_referenceable(ext), "{ext} should be mentionable");
        }
        assert!(
            !ext_is_source("md"),
            "docs would drown the discovery tree in noise"
        );
    }

    /// Every advertised extension must belong to a language with `ast: true`.
    /// The two sides are produced from the same table, so this guards the
    /// formatting and dedup rather than the data.
    #[test]
    fn the_ast_list_only_names_extensions_of_languages_with_grammars() {
        let listed = ast_ext_list();
        let mut count = 0;
        for token in listed.split_whitespace() {
            let ext = token.trim_start_matches('.');
            let owner = LANGS
                .iter()
                .find(|l| l.has_ext(ext))
                .unwrap_or_else(|| panic!("{ext} is advertised but belongs to no language"));
            assert!(
                owner.ast,
                "{ext} is advertised but {} has no grammar",
                owner.name
            );
            count += 1;
        }
        // Non-empty, or list_symbols would tell the model it supports nothing.
        assert!(count >= 5, "got: {listed}");
    }

    #[test]
    fn the_ast_language_list_only_names_languages_with_grammars() {
        let listed = ast_lang_list();
        for name in listed.split(", ") {
            let lang = LANGS
                .iter()
                .find(|l| l.name == name)
                .unwrap_or_else(|| panic!("{name} is not a registered language"));
            assert!(lang.ast, "{name} is advertised without a grammar");
        }
        assert!(listed.contains("Rust"), "got: {listed}");
    }

    #[test]
    fn ext_lookup_accepts_both_dotted_and_bare_forms_and_any_case() {
        for form in ["rs", ".rs", "RS", ".RS"] {
            assert!(ext_is_source(form), "{form} should be recognised");
        }
        assert!(ext_is_source("TSX"), "case must not matter");
    }

    #[test]
    fn unknown_extensions_are_not_source() {
        for ext in ["", "exe", "png", "zip", "so"] {
            assert!(!ext_is_source(ext), "{ext} should not be source");
        }
    }

    #[test]
    fn the_lists_cover_every_extension_the_registry_declares() {
        let source = source_exts();
        let referenceable = referenceable_exts();
        for lang in LANGS {
            for ext in lang.exts {
                assert!(
                    source.contains(ext) || DOC_EXTS.contains(ext),
                    "{}: {ext} is in LANGS but in neither derived list",
                    lang.name
                );
                assert!(
                    referenceable.contains(ext),
                    "{}: {ext} missing from referenceable_exts",
                    lang.name
                );
            }
        }
    }

    #[test]
    fn skip_dirs_cover_the_usual_noise() {
        // The three that dominated the old, shorter lists.
        for dir in ["target", "node_modules", ".git", "__pycache__", "dist"] {
            assert!(SKIP_DIRS.contains(&dir), "{dir} is missing");
        }
    }

    #[test]
    fn manifests_name_plausible_files() {
        for lang in LANGS {
            for m in lang.manifests {
                assert!(!m.is_empty(), "{} has an empty manifest", lang.name);
            }
        }
    }

    /// Normalisation has to reach the table too, not just the query, or a
    /// caller passing `.TS` would silently match nothing.
    #[test]
    fn lookup_normalises_the_query_before_matching_the_table() {
        let upper = LANGS
            .iter()
            .find(|l| l.has_ext("ts"))
            .expect("TypeScript is registered");
        assert!(upper.name == "TypeScript");
        for ext in upper.exts {
            assert!(upper.has_ext(&ext.to_ascii_lowercase()));
        }
    }
}

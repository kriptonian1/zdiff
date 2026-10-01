//! The supported languages and how their tree-sitter captures map to GitHub's syntax roles.

use std::sync::OnceLock;

use tree_sitter_highlight::HighlightConfiguration;
use tree_sitter_language::LanguageFn;

use crate::Class as C;

pub struct Grammar {
    pub extensions: &'static [&'static str],
    language: LanguageFn,
    /// Highlight queries, joined in order.
    queries: &'static [&'static str],
    /// Captures GitHub colors differently in this language; checked before [`CAPTURES`].
    overrides: &'static [(&'static str, C)],
    /// The query lists winning patterns first; reversed before use.
    first_wins: bool,
}

/// A grammar's compiled highlighter plus the role for each recognized capture name.
pub struct Config {
    pub inner: HighlightConfiguration,
    pub classes: Box<[C]>,
}

/// Capture name to GitHub role, shared by every language.
///
/// The most specific match wins (`function.method` uses `function`); captures not
/// listed here (`variable`, `property`, `punctuation`) keep the default color.
const CAPTURES: &[(&str, C)] = &[
    ("keyword", C::Keyword),
    ("operator", C::Keyword),
    ("string", C::String),
    ("string.escape", C::Tag),
    ("escape", C::Tag),
    ("number", C::Constant),
    ("float", C::Constant),
    ("boolean", C::Constant),
    ("constant", C::Constant),
    ("type.builtin", C::Constant),
    ("variable.builtin", C::Constant),
    ("function.builtin", C::Constant),
    ("function", C::Entity),
    ("constructor", C::Entity),
    ("type", C::Entity),
    ("attribute", C::Entity),
    ("tag", C::Tag),
    ("comment", C::Comment),
];

pub const GRAMMARS: &[Grammar] = &[
    #[cfg(feature = "rust")]
    Grammar {
        extensions: &["rs"],
        language: tree_sitter_rust::LANGUAGE,
        queries: &[tree_sitter_rust::HIGHLIGHTS_QUERY],
        // GitHub shows primitive types (`u32`, `str`) and lifetimes as keywords.
        overrides: &[("type.builtin", C::Keyword), ("label", C::Keyword)],
        first_wins: false,
    },
    #[cfg(feature = "typescript")]
    Grammar {
        extensions: &["ts", "mts", "cts"],
        language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        // The TypeScript query only adds TS-specific rules on top of JavaScript's.
        queries: &[
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
        ],
        overrides: &[],
        first_wins: false,
    },
    #[cfg(feature = "typescript")]
    Grammar {
        extensions: &["tsx"],
        language: tree_sitter_typescript::LANGUAGE_TSX,
        queries: &[
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
        ],
        overrides: &[],
        first_wins: false,
    },
    #[cfg(feature = "javascript")]
    Grammar {
        extensions: &["js", "mjs", "cjs"],
        language: tree_sitter_javascript::LANGUAGE,
        queries: &[tree_sitter_javascript::HIGHLIGHT_QUERY],
        overrides: &[],
        first_wins: false,
    },
    #[cfg(feature = "javascript")]
    Grammar {
        extensions: &["jsx"],
        // The JavaScript grammar parses JSX; only the extra query differs.
        language: tree_sitter_javascript::LANGUAGE,
        queries: &[
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
        ],
        overrides: &[],
        first_wins: false,
    },
    #[cfg(feature = "json")]
    Grammar {
        extensions: &["json"],
        language: tree_sitter_json::LANGUAGE,
        queries: &[tree_sitter_json::HIGHLIGHTS_QUERY],
        // GitHub colors object keys green.
        overrides: &[("string.special.key", C::Tag)],
        // Lists the key rule before the generic string rule.
        first_wins: true,
    },
    #[cfg(feature = "toml")]
    Grammar {
        extensions: &["toml"],
        language: tree_sitter_toml_ng::LANGUAGE,
        queries: &[tree_sitter_toml_ng::HIGHLIGHTS_QUERY],
        // Captures bare keys as `@type`; GitHub colors keys green.
        overrides: &[("type", C::Tag)],
        first_wins: false,
    },
    #[cfg(feature = "yaml")]
    Grammar {
        extensions: &["yml", "yaml"],
        language: tree_sitter_yaml::LANGUAGE,
        queries: &[tree_sitter_yaml::HIGHLIGHTS_QUERY],
        // GitHub colors mapping keys green.
        overrides: &[("property", C::Tag)],
        first_wins: false,
    },
    #[cfg(feature = "python")]
    Grammar {
        extensions: &["py", "pyi"],
        language: tree_sitter_python::LANGUAGE,
        queries: &[tree_sitter_python::HIGHLIGHTS_QUERY],
        overrides: &[],
        first_wins: false,
    },
    #[cfg(feature = "go")]
    Grammar {
        extensions: &["go"],
        language: tree_sitter_go::LANGUAGE,
        queries: &[tree_sitter_go::HIGHLIGHTS_QUERY],
        overrides: &[],
        first_wins: false,
    },
    #[cfg(feature = "bash")]
    Grammar {
        extensions: &["sh", "bash"],
        language: tree_sitter_bash::LANGUAGE,
        queries: &[tree_sitter_bash::HIGHLIGHT_QUERY],
        overrides: &[],
        first_wins: false,
    },
    #[cfg(feature = "xml")]
    Grammar {
        extensions: &["svg", "xml"],
        language: tree_sitter_xml::LANGUAGE_XML,
        queries: &[tree_sitter_xml::XML_HIGHLIGHT_QUERY],
        // GitHub colors attribute names like HTML's, as entities.
        overrides: &[("property", C::Entity)],
        first_wins: false,
    },
];

/// Compiled lazily on first use of each language; a failed build is cached as `None`.
static CONFIGS: [OnceLock<Option<Config>>; GRAMMARS.len()] =
    [const { OnceLock::new() }; GRAMMARS.len()];

/// The compiled highlighter for `GRAMMARS[index]`, or `None` if its query fails to build.
pub fn config(index: usize) -> Option<&'static Config> {
    CONFIGS[index]
        .get_or_init(|| build(&GRAMMARS[index]))
        .as_ref()
}

fn build(grammar: &Grammar) -> Option<Config> {
    let (names, classes): (Vec<&str>, Vec<C>) =
        grammar.overrides.iter().chain(CAPTURES).copied().unzip();
    let language = grammar.language.into();
    let query = styled_patterns(
        &language,
        &grammar.queries.concat(),
        &names,
        grammar.first_wins,
    )?;
    let mut inner = HighlightConfiguration::new(language, "", &query, "", "").ok()?;
    inner.configure(&names);
    Some(Config {
        inner,
        classes: classes.into_boxed_slice(),
    })
}

/// Rewrites a highlight query so the most specific colored capture wins.
///
/// `tree-sitter-highlight` lets the last pattern matching a node win, even one whose
/// capture has no color (such as a trailing catch-all `(identifier) @variable`), which
/// would erase real highlights. Patterns with no colored capture are dropped, and
/// first-wins queries are reversed.
fn styled_patterns(
    language: &tree_sitter::Language,
    source: &str,
    names: &[&str],
    first_wins: bool,
) -> Option<String> {
    let query = tree_sitter::Query::new(language, source).ok()?;
    let colored = |pattern: usize| {
        query
            .capture_quantifiers(pattern)
            .iter()
            .zip(query.capture_names())
            .any(|(quantifier, capture)| {
                *quantifier != tree_sitter::CaptureQuantifier::Zero
                    && names.iter().any(|name| recognizes(name, capture))
            })
    };
    let mut patterns: Vec<&str> = (0..query.pattern_count())
        .filter(|&i| colored(i))
        .map(|i| &source[query.start_byte_for_pattern(i)..query.end_byte_for_pattern(i)])
        .collect();
    if first_wins {
        patterns.reverse();
    }
    Some(patterns.join("\n"))
}

/// Whether `configure` maps `capture` to `name`: every dotted part of `name` is in `capture`.
fn recognizes(name: &str, capture: &str) -> bool {
    name.split('.')
        .all(|part| capture.split('.').any(|c| c == part))
}

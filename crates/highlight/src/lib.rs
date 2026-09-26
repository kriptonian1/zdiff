//! Syntax highlighting for zdiff: source bytes in, GitHub-style syntax roles out.
//!
//! Find a file's [`Language`] with [`language`], then get its [`Token`]s with [`highlight`].
//! Roles follow GitHub's syntax palette (Primer "prettylights"); the caller picks the colors.

mod grammars;

use std::path::Path;

use tree_sitter_highlight::{HighlightEvent, Highlighter};

/// Files larger than this stay plain: bounds parse time and token memory for
/// minified or generated files, and keeps every offset within `u32`.
const MAX_HIGHLIGHT_BYTES: usize = 2 * 1024 * 1024;

/// A syntax role from GitHub's highlight palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Keywords, operators, and in some languages primitive types.
    Keyword,
    String,
    /// Numbers, booleans, constants, and built-ins.
    Constant,
    /// Functions, types, constructors, and attributes.
    Entity,
    /// Markup tags, data-file keys, and string escapes.
    Tag,
    Comment,
}

/// A highlighted byte range of the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub start: u32,
    pub end: u32,
    pub class: Class,
}

/// A supported language, picked from a file's extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Language(usize);

/// The language for `path`, by extension; `None` if unsupported.
#[must_use]
pub fn language(path: &Path) -> Option<Language> {
    let extension = path.extension()?.to_str()?;
    grammars::GRAMMARS
        .iter()
        .position(|g| g.extensions.contains(&extension))
        .map(Language)
}

/// Highlights `source` as `language`, returning sorted, non-overlapping tokens.
///
/// Best effort: files over the size limit or that fail to parse yield no tokens.
#[must_use]
pub fn highlight(language: Language, source: &[u8]) -> Box<[Token]> {
    if source.len() > MAX_HIGHLIGHT_BYTES {
        return Box::default();
    }
    let Some(config) = grammars::config(language.0) else {
        return Box::default();
    };
    let mut highlighter = Highlighter::new();
    let Ok(events) = highlighter.highlight(&config.inner, source, None, None, |_| None) else {
        return Box::default();
    };
    let mut tokens = Vec::new();
    let mut active = Vec::new();
    for event in events {
        match event {
            Ok(HighlightEvent::HighlightStart(h)) => active.push(config.classes[h.0]),
            Ok(HighlightEvent::HighlightEnd) => {
                active.pop();
            }
            Ok(HighlightEvent::Source { start, end }) => {
                if let Some(&class) = active.last() {
                    push(&mut tokens, start, end, class);
                }
            }
            Err(_) => break,
        }
    }
    tokens.into_boxed_slice()
}

/// Appends a token, merging it into the previous one when they touch and share a class.
fn push(tokens: &mut Vec<Token>, start: usize, end: usize, class: Class) {
    let offset = |i: usize| u32::try_from(i).expect("source is at most MAX_HIGHLIGHT_BYTES");
    let (start, end) = (offset(start), offset(end));
    if start == end {
        return;
    }
    match tokens.last_mut() {
        Some(last) if last.class == class && last.end == start => last.end = end,
        _ => tokens.push(Token { start, end, class }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The role of the first occurrence of `word` in `source`, or `None` if it is plain.
    fn role(file: &str, source: &str, word: &str) -> Option<Class> {
        let language = language(Path::new(file)).expect("supported extension");
        let tokens = highlight(language, source.as_bytes());
        let at = u32::try_from(source.find(word).expect("word in source")).unwrap();
        tokens
            .iter()
            .find(|t| t.start <= at && at < t.end)
            .map(|t| t.class)
    }

    fn assert_well_formed(file: &str, source: &str) {
        let tokens = highlight(language(Path::new(file)).unwrap(), source.as_bytes());
        assert!(!tokens.is_empty(), "{file}: no tokens");
        assert!(
            tokens
                .windows(2)
                .all(|w| w[0].end <= w[1].start && w[0].start < w[0].end),
            "{file}: tokens overlap or are unsorted: {tokens:?}"
        );
    }

    #[test]
    fn every_language_yields_sorted_tokens() {
        let samples = [
            ("a.rs", "fn main() { let x = 1; }"),
            ("a.ts", "let n: number = 1;"),
            ("a.tsx", "const a = <B x={1} />;"),
            ("a.js", "function f() { return 'x'; }"),
            ("a.jsx", "const a = <B label=\"x\" />;"),
            ("a.json", "{\"k\": [1, true]}"),
            ("a.toml", "key = \"v\""),
            ("a.yaml", "key: 1"),
            ("a.py", "def f(): return 1"),
            ("a.go", "package main\nfunc f() int { return 1 }"),
            ("a.sh", "echo \"hi\" # c"),
        ];
        for (file, source) in samples {
            assert_well_formed(file, source);
        }
    }

    #[test]
    fn rust_follows_github_roles() {
        let src = "fn f(x: u32) -> String { \"a\\n\" }";
        assert_eq!(role("a.rs", src, "fn"), Some(Class::Keyword));
        assert_eq!(role("a.rs", src, "u32"), Some(Class::Keyword), "override");
        assert_eq!(role("a.rs", src, "String"), Some(Class::Entity));
        assert_eq!(role("a.rs", src, "f("), Some(Class::Entity));
        assert_eq!(role("a.rs", src, "\"a"), Some(Class::String));
        assert_eq!(role("a.rs", src, "\\n"), Some(Class::Tag));
        assert_eq!(role("a.rs", src, "x:"), None, "parameters stay default");
    }

    #[test]
    fn typescript_and_jsx_follow_github_roles() {
        let ts = "let n: number = 1;";
        assert_eq!(role("a.ts", ts, "let"), Some(Class::Keyword));
        assert_eq!(role("a.ts", ts, "number"), Some(Class::Constant));
        assert_eq!(role("a.ts", ts, "1"), Some(Class::Constant));

        // The grammar tags lowercase (HTML) elements; capitalized components are constructors.
        let jsx = "const a = <Button label=\"x\" />; <div />";
        assert_eq!(role("a.jsx", jsx, "div"), Some(Class::Tag));
        assert_eq!(role("a.jsx", jsx, "Button"), Some(Class::Entity));
        assert_eq!(role("a.jsx", jsx, "label"), Some(Class::Entity));

        let tsx = "const a: Props = <span />;";
        assert_eq!(role("a.tsx", tsx, "Props"), Some(Class::Entity));
        assert_eq!(role("a.tsx", tsx, "span"), Some(Class::Tag));
    }

    #[test]
    fn data_file_keys_are_tags() {
        let json = "{\"k\": \"v\", \"n\": 1}";
        assert_eq!(role("a.json", json, "\"k\""), Some(Class::Tag));
        assert_eq!(role("a.json", json, "\"v\""), Some(Class::String));
        assert_eq!(role("a.json", json, "1"), Some(Class::Constant));
        assert_eq!(role("a.yaml", "key: 1", "key"), Some(Class::Tag));
        assert_eq!(role("a.toml", "key = \"v\"", "key"), Some(Class::Tag));
    }

    #[test]
    fn python_and_go_builtins_are_constants() {
        let py = "def f():\n    return print(\"x\")";
        assert_eq!(role("a.py", py, "def"), Some(Class::Keyword));
        assert_eq!(role("a.py", py, "f("), Some(Class::Entity));
        assert_eq!(role("a.py", py, "print"), Some(Class::Constant));
        let go = "package main\nfunc f() { len(x) }";
        assert_eq!(role("a.go", go, "func"), Some(Class::Keyword));
        assert_eq!(
            role("a.go", go, "f("),
            Some(Class::Entity),
            "catch-all must not win"
        );
        assert_eq!(role("a.go", go, "len"), Some(Class::Constant));
        assert_eq!(role("a.go", go, "x)"), None);
    }

    #[test]
    fn block_comment_is_one_token() {
        let src = "/* one\ntwo\nthree */ fn f() {}";
        let tokens = highlight(language(Path::new("a.rs")).unwrap(), src.as_bytes());
        assert_eq!(
            tokens[0],
            Token {
                start: 0,
                end: 19,
                class: Class::Comment
            }
        );
    }

    #[test]
    fn unsupported_or_huge_files_stay_plain() {
        assert_eq!(language(Path::new("README.md")), None);
        assert_eq!(language(Path::new("Makefile")), None);
        let huge = vec![b'x'; MAX_HIGHLIGHT_BYTES + 1];
        assert!(highlight(language(Path::new("a.rs")).unwrap(), &huge).is_empty());
    }
}

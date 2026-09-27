//! Text search over both sides of a diff, for zdiff's find bar.

use std::ops::Range;
use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::bytes::{Regex, RegexBuilder};
use zdiff_core::{FileDiff, Side};

/// Hits kept per search; bounds memory for queries like `e`.
pub const MAX_HITS: usize = 10_000;

/// What the user typed plus the toggles.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    /// Match case exactly; otherwise case is smart (exact only when `text` has capitals).
    pub case: bool,
    /// Treat `text` as a regex instead of plain text.
    pub regex: bool,
    /// Search only added and removed lines, skipping unchanged context.
    pub changed_only: bool,
}

/// A compiled [`Query`].
#[derive(Debug)]
pub struct Finder {
    /// `None` for an empty query, which finds nothing.
    re: Option<Regex>,
    changed_only: bool,
}

/// One match: zero-based `line` on `side`, `range` in that line's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub side: Side,
    pub line: u32,
    pub range: Range<u32>,
    /// The line is added or removed, not unchanged context.
    pub changed: bool,
}

/// Include/exclude path filter from comma-separated globs.
///
/// A pattern without glob characters, like `crates/tui`, also matches everything under it.
#[derive(Debug)]
pub struct Filter {
    /// `None` allows every path.
    include: Option<GlobSet>,
    exclude: GlobSet,
}

/// Which filter field held an invalid glob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Include,
    Exclude,
}

impl Filter {
    /// # Errors
    /// Returns the field with the invalid glob and the glob error.
    pub fn new(include: &str, exclude: &str) -> Result<Self, (Field, globset::Error)> {
        let include = if include.trim().is_empty() {
            None
        } else {
            Some(glob_set(include).map_err(|e| (Field::Include, e))?)
        };
        let exclude = glob_set(exclude).map_err(|e| (Field::Exclude, e))?;
        Ok(Self { include, exclude })
    }

    /// Whether `path` passes: not excluded, and included when an include list is set.
    #[must_use]
    pub fn allows(&self, path: &Path) -> bool {
        !self.exclude.is_match(path) && self.include.as_ref().is_none_or(|set| set.is_match(path))
    }
}

fn glob_set(list: &str) -> Result<GlobSet, globset::Error> {
    let mut set = GlobSetBuilder::new();
    for pattern in list.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        set.add(Glob::new(pattern)?);
        if !pattern.contains(['*', '?', '[', '{']) {
            set.add(Glob::new(&format!("{}/**", pattern.trim_end_matches('/')))?);
        }
    }
    set.build()
}

impl Query {
    /// # Errors
    /// Returns the regex error when `regex` is on and `text` is not a valid pattern.
    pub fn compile(&self) -> Result<Finder, regex::Error> {
        let re = if self.text.is_empty() {
            None
        } else {
            let pattern = if self.regex {
                self.text.clone()
            } else {
                regex::escape(&self.text)
            };
            let smart = !self.text.chars().any(char::is_uppercase);
            Some(
                RegexBuilder::new(&pattern)
                    .case_insensitive(!self.case && smart)
                    .build()?,
            )
        };
        Ok(Finder {
            re,
            changed_only: self.changed_only,
        })
    }
}

impl Finder {
    /// Hits on the old side then the new side, each in line order; at most [`MAX_HITS`].
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "core treats text over u32::MAX bytes as binary, so offsets fit"
    )]
    pub fn find(&self, file: &FileDiff) -> Vec<Hit> {
        let mut hits = Vec::new();
        let Some(re) = &self.re else {
            return hits;
        };
        for side in [Side::Old, Side::New] {
            let text = side.pick(&file.old, &file.new);
            for line in 0..text.len() {
                let changed = changed(file, side, line);
                if self.changed_only && !changed {
                    continue;
                }
                for m in re.find_iter(text.line(line)).filter(|m| !m.is_empty()) {
                    if hits.len() == MAX_HITS {
                        return hits;
                    }
                    hits.push(Hit {
                        side,
                        line,
                        range: m.start() as u32..m.end() as u32,
                        changed,
                    });
                }
            }
        }
        hits
    }
}

/// Whether `line` on `side` is inside one of `file`'s hunks.
fn changed(file: &FileDiff, side: Side, line: u32) -> bool {
    let span = |h: &zdiff_core::Hunk| side.pick(&h.old, &h.new).clone();
    let next = file.hunks.partition_point(|h| span(h).end <= line);
    file.hunks
        .get(next)
        .is_some_and(|h| span(h).contains(&line))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(text: &str) -> Query {
        Query {
            text: text.into(),
            ..Query::default()
        }
    }

    fn hits(query: &Query, old: &str, new: &str) -> Vec<(Side, u32, Range<u32>)> {
        let file = FileDiff::new(old.into(), new.into());
        let finder = query.compile().expect("valid query");
        (finder.find(&file).into_iter())
            .map(|h| (h.side, h.line, h.range))
            .collect()
    }

    #[test]
    fn plain_text_is_literal_and_regex_is_not() {
        let file = "a.b\naxb\n";
        assert_eq!(hits(&query("a.b"), file, file).len(), 2, "one per side");
        let re = Query {
            regex: true,
            ..query("a.b")
        };
        assert_eq!(hits(&re, file, file).len(), 4);
        assert!(query("(").compile().is_ok(), "plain text escapes");
        assert!(
            Query {
                regex: true,
                ..query("(")
            }
            .compile()
            .is_err()
        );
    }

    #[test]
    fn case_is_smart_unless_forced() {
        let file = "Tree\n";
        assert_eq!(hits(&query("tree"), file, "").len(), 1);
        assert!(
            hits(&query("TREE"), file, "").is_empty(),
            "capitals match exactly"
        );
        let exact = Query {
            case: true,
            ..query("tree")
        };
        assert!(hits(&exact, file, "").is_empty());
    }

    #[test]
    fn hits_cover_both_sides_and_changed_only_skips_context() {
        let (old, new) = ("keep x\nold x\n", "keep x\nnew x\n");
        assert_eq!(
            hits(&query("x"), old, new),
            [
                (Side::Old, 0, 5..6),
                (Side::Old, 1, 4..5),
                (Side::New, 0, 5..6),
                (Side::New, 1, 4..5),
            ]
        );
        let changed = Query {
            changed_only: true,
            ..query("x")
        };
        assert_eq!(
            hits(&changed, old, new),
            [(Side::Old, 1, 4..5), (Side::New, 1, 4..5)]
        );
    }

    #[test]
    fn empty_zero_width_binary_and_capped() {
        assert!(hits(&query(""), "a\n", "a\n").is_empty());
        let star = Query {
            regex: true,
            ..query("z*")
        };
        assert!(
            hits(&star, "abc\n", "").is_empty(),
            "zero-width matches skipped"
        );
        let binary = FileDiff::new(b"a\0a".to_vec(), b"a\0".to_vec());
        assert!(query("a").compile().unwrap().find(&binary).is_empty());
        let many = "e".repeat(MAX_HITS + 5) + "\n";
        assert_eq!(hits(&query("e"), &many, "").len(), MAX_HITS);
    }

    #[test]
    fn hits_know_whether_their_line_changed() {
        let file = FileDiff::new(b"keep x\nold x\n".to_vec(), b"keep x\nnew x\n".to_vec());
        let changed: Vec<bool> = query("x")
            .compile()
            .unwrap()
            .find(&file)
            .iter()
            .map(|h| h.changed)
            .collect();
        assert_eq!(changed, [false, true, false, true]);
    }

    #[test]
    fn filter_includes_folders_and_globs_and_exclude_wins() {
        let allows = |include: &str, exclude: &str, path: &str| {
            Filter::new(include, exclude)
                .unwrap()
                .allows(Path::new(path))
        };
        assert!(allows("", "", "any/where.rs"), "empty include allows all");
        assert!(
            allows("crates/tui", "", "crates/tui/src/a.rs"),
            "plain folder covers its files"
        );
        assert!(
            allows(" *.md , crates/tui/ ", "", "crates/tui/a.rs"),
            "trimmed, trailing slash"
        );
        assert!(!allows("crates/tui", "", "crates/core/a.rs"));
        assert!(allows("*.rs", "", "a.rs"));
        assert!(
            !allows("crates", "**/tests/**", "crates/core/tests/repo.rs"),
            "exclude wins"
        );
        assert_eq!(Filter::new("[", "").unwrap_err().0, Field::Include);
        assert_eq!(Filter::new("", "a{").unwrap_err().0, Field::Exclude);
    }
}

//! Word-level changes inside paired change lines, for emphasis within a changed row.

use std::ops::Range;
use std::str;

use gix::diff::blob::sources::words;
use gix::diff::blob::{Algorithm, Diff, InternedInput};

use crate::diff::{FileDiff, Text};

/// Longer lines (minified, generated) get no emphasis; bounds per-pair cost.
const MAX_WORD_DIFF_LINE: usize = 1024;
/// A pair sharing under 40% of its bytes is unrelated; emphasizing it all is noise.
const MIN_SHARED_PERCENT: usize = 40;

/// Changed words within paired change lines, as sorted file byte ranges per side.
#[derive(Debug, Default)]
pub struct WordChanges {
    pub old: Box<[Range<u32>]>,
    pub new: Box<[Range<u32>]>,
}

impl FileDiff {
    /// The words that differ between lines paired by position within a hunk, like
    /// [`FileDiff::rows`]; unpaired, non-UTF-8, very long, or mostly rewritten lines get none.
    #[must_use]
    pub fn word_changes(&self) -> WordChanges {
        let (mut old, mut new) = (Vec::new(), Vec::new());
        let mut input = InternedInput::default();
        let mut diff = Diff::default();
        for hunk in &self.hunks {
            for (o, n) in hunk.old.clone().zip(hunk.new.clone()) {
                let (Some(before), Some(after)) =
                    (word_line(&self.old, o), word_line(&self.new, n))
                else {
                    continue;
                };
                input.clear();
                input.update_before(words(before));
                input.update_after(words(after));
                let tokens = input.interner.num_tokens();
                diff.compute_with(Algorithm::Myers, &input.before, &input.after, tokens);
                diff.postprocess_no_heuristic(&input);

                let (old_mark, new_mark) = (old.len(), new.len());
                let shared = mark(
                    self.old.line_start(o),
                    before,
                    |i| diff.is_removed(i),
                    &mut old,
                ) + mark(
                    self.new.line_start(n),
                    after,
                    |i| diff.is_added(i),
                    &mut new,
                );
                if shared * 100 < (before.len() + after.len()) * MIN_SHARED_PERCENT {
                    old.truncate(old_mark);
                    new.truncate(new_mark);
                }
            }
        }
        WordChanges {
            old: old.into(),
            new: new.into(),
        }
    }
}

/// Line `i` as text, if short enough and valid UTF-8 to word-diff.
fn word_line(text: &Text, i: u32) -> Option<&str> {
    let line = text.line(i);
    str::from_utf8(line)
        .ok()
        .filter(|_| line.len() <= MAX_WORD_DIFF_LINE)
}

/// Pushes the changed words of `line` to `out` as file ranges; returns the unchanged byte count.
fn mark(
    line_start: u32,
    line: &str,
    changed: impl Fn(u32) -> bool,
    out: &mut Vec<Range<u32>>,
) -> usize {
    let mut at = line_start;
    let mut shared = 0;
    for (i, word) in (0..).zip(words(line)) {
        let end = at + u32::try_from(word.len()).expect("line is at most MAX_WORD_DIFF_LINE");
        if !changed(i) {
            shared += word.len();
        } else if let Some(last) = out.last_mut().filter(|last| last.end == at) {
            last.end = end;
        } else {
            out.push(at..end);
        }
        at = end;
    }
    shared
}

#[cfg(test)]
#[expect(
    clippy::single_range_in_vec_init,
    reason = "expected results are lists of ranges"
)]
mod tests {
    use super::*;

    fn check(old: &str, new: &str, expected: (&[Range<u32>], &[Range<u32>])) {
        let changes = FileDiff::new(old.into(), new.into()).word_changes();
        assert_eq!(
            (&*changes.old, &*changes.new),
            expected,
            "{old:?} -> {new:?}"
        );
    }

    #[test]
    fn changed_word_only() {
        check("let a = 1;\n", "let b = 1;\n", (&[4..5], &[4..5]));
    }

    #[test]
    fn ranges_are_file_offsets() {
        check("x\nlet a = 1;\n", "x\nlet bb = 1;\n", (&[6..7], &[6..8]));
    }

    #[test]
    fn indent_change_marks_only_whitespace() {
        check("  let x = 1;\n", "    let x = 1;\n", (&[0..2], &[0..4]));
    }

    #[test]
    fn unrelated_lines_pure_changes_and_long_lines_get_nothing() {
        check("alpha beta\n", "gamma delta\n", (&[], &[]));
        check("a\n", "a\nb\n", (&[], &[]));
        let long = "x ".repeat(MAX_WORD_DIFF_LINE);
        check(&format!("{long}a\n"), &format!("{long}b\n"), (&[], &[]));
    }
}

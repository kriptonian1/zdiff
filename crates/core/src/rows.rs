//! Display rows for a split diff: paired changes, context lines, and folded unchanged runs.

use std::ops::Range;

use crate::diff::{FileDiff, Hunk};

/// One line of a split diff view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// `len` unchanged lines starting at line `old` / `new`, collapsed into one row.
    Fold { old: u32, new: u32, len: u32 },
    /// Start of a block of changes: its line ranges and the nearest heading line (old side).
    Header {
        old: Range<u32>,
        new: Range<u32>,
        heading: Option<u32>,
    },
    /// A display line; a `None` side has no line here and renders as filler.
    Line {
        old: Option<u32>,
        new: Option<u32>,
        kind: Kind,
    },
}

/// Whether a [`Row::Line`] is unchanged context or part of a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Context,
    Change,
}

impl FileDiff {
    /// Builds split-view rows with `context` unchanged lines around each change.
    ///
    /// Removed and added lines are paired by position; changes whose context
    /// overlaps share one block, like `git diff`. Empty for binary or unchanged files.
    #[must_use]
    pub fn rows(&self, context: u32) -> Vec<Row> {
        let capacity = self
            .hunks
            .iter()
            .map(|h| hunk_height(h) + 2 * context + 2)
            .sum::<u32>()
            + 1;
        let mut rows = Vec::with_capacity(capacity as usize);
        let old_len = self.old.len();
        // First line on each side not yet covered by a row.
        let (mut old, mut new) = (0, 0);
        for block in self
            .hunks
            .chunk_by(|a, b| b.old.start - a.old.end <= 2 * context)
        {
            let (first, last) = (&block[0], &block[block.len() - 1]);
            let lead = context.min(first.old.start - old);
            let trail = context.min(old_len - last.old.end);
            let (start_old, start_new) = (first.old.start - lead, first.new.start - lead);
            let (end_old, end_new) = (last.old.end + trail, last.new.end + trail);

            push_fold(&mut rows, old, new, start_old - old);
            rows.push(Row::Header {
                old: start_old..end_old,
                new: start_new..end_new,
                heading: self.heading(start_old),
            });
            let (mut o, mut n) = (start_old, start_new);
            for hunk in block {
                rows.extend(context_lines(o, n, hunk.old.start - o));
                push_change(&mut rows, hunk);
                (o, n) = (hunk.old.end, hunk.new.end);
            }
            rows.extend(context_lines(o, n, trail));
            (old, new) = (end_old, end_new);
        }
        if !self.hunks.is_empty() {
            push_fold(&mut rows, old, new, old_len - old);
        }
        rows
    }

    /// Nearest line before `before` starting with a letter, `_`, or `$` (git's default).
    // ponytail: scans back to line 0 per block; cache per block if headers get slow.
    fn heading(&self, before: u32) -> Option<u32> {
        (0..before).rev().find(|&i| {
            self.old
                .line(i)
                .first()
                .is_some_and(|b| b.is_ascii_alphabetic() || matches!(b, b'_' | b'$'))
        })
    }
}

/// Replaces the [`Row::Fold`] at `index` with its unchanged lines.
///
/// Returns `false`, leaving `rows` untouched, if `index` is not a fold.
pub fn expand(rows: &mut Vec<Row>, index: usize) -> bool {
    let Some(&Row::Fold { old, new, len }) = rows.get(index) else {
        return false;
    };
    rows.splice(index..=index, context_lines(old, new, len));
    true
}

fn hunk_height(hunk: &Hunk) -> u32 {
    (hunk.old.end - hunk.old.start).max(hunk.new.end - hunk.new.start)
}

fn context_lines(old: u32, new: u32, len: u32) -> impl Iterator<Item = Row> {
    (0..len).map(move |k| Row::Line {
        old: Some(old + k),
        new: Some(new + k),
        kind: Kind::Context,
    })
}

fn push_fold(rows: &mut Vec<Row>, old: u32, new: u32, len: u32) {
    if len > 0 {
        rows.push(Row::Fold { old, new, len });
    }
}

fn push_change(rows: &mut Vec<Row>, hunk: &Hunk) {
    let side = |range: &Range<u32>, k: u32| Some(range.start + k).filter(|&i| i < range.end);
    rows.extend((0..hunk_height(hunk)).map(|k| Row::Line {
        old: side(&hunk.old, k),
        new: side(&hunk.new, k),
        kind: Kind::Change,
    }));
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;

    use super::*;

    fn numbered(lines: std::ops::RangeInclusive<u32>) -> String {
        lines.fold(String::new(), |mut out, i| {
            writeln!(out, "line {i}").expect("writing to a String cannot fail");
            out
        })
    }

    fn diff(old: &str, new: &str) -> FileDiff {
        FileDiff::new(old.into(), new.into())
    }

    fn line(old: Option<u32>, new: Option<u32>, kind: Kind) -> Row {
        Row::Line { old, new, kind }
    }

    #[test]
    fn pairs_removed_with_added_and_fills_the_shorter_side() {
        let rows = diff("a\nx\nb\n", "a\n1\n2\n3\nb\n").rows(0);
        let changes: Vec<_> = rows
            .into_iter()
            .filter(|r| matches!(r, Row::Line { .. }))
            .collect();
        assert_eq!(
            changes,
            [
                line(Some(1), Some(1), Kind::Change),
                line(None, Some(2), Kind::Change),
                line(None, Some(3), Kind::Change),
            ]
        );
    }

    #[test]
    fn keeps_context_and_folds_the_rest() {
        let old = numbered(1..=20);
        let new = old.replace("line 10\n", "changed\n");
        let rows = diff(&old, &new).rows(3);
        assert_eq!(
            rows[0],
            Row::Fold {
                old: 0,
                new: 0,
                len: 6
            }
        );
        assert!(matches!(rows[1], Row::Header { ref old, .. } if *old == (6..13)));
        assert_eq!(rows.len(), 1 + 1 + 3 + 1 + 3 + 1);
        assert_eq!(
            rows[rows.len() - 1],
            Row::Fold {
                old: 13,
                new: 13,
                len: 7
            }
        );
    }

    #[test]
    fn close_changes_share_one_block() {
        let old = numbered(1..=20);
        let new = old.replace("line 5\n", "x\n").replace("line 9\n", "y\n");
        let headers = diff(&old, &new)
            .rows(3)
            .iter()
            .filter(|r| matches!(r, Row::Header { .. }))
            .count();
        assert_eq!(headers, 1);
    }

    #[test]
    fn added_file_has_only_new_lines() {
        let rows = diff("", "a\nb\n").rows(3);
        assert!(matches!(rows[0], Row::Header { .. }));
        assert_eq!(
            rows[1..],
            [
                line(None, Some(0), Kind::Change),
                line(None, Some(1), Kind::Change)
            ]
        );
    }

    #[test]
    fn heading_is_nearest_unindented_line() {
        let old = "fn a() {\n    1\n}\nfn b() {\n    2\n    3\n    4\n    5\n}\n";
        let new = old.replace("    5\n", "    five\n");
        let rows = diff(old, &new).rows(1);
        let heading = rows.iter().find_map(|r| match r {
            Row::Header { heading, .. } => *heading,
            _ => None,
        });
        assert_eq!(heading, Some(3));
    }

    #[test]
    fn every_line_is_covered_exactly_once() {
        let old = numbered(1..=40);
        let new = old
            .replace("line 3\n", "")
            .replace("line 20\n", "a\nb\n")
            .replace("line 39\n", "z\n");
        let d = diff(&old, &new);
        let (mut seen_old, mut seen_new) = (Vec::new(), Vec::new());
        for row in d.rows(3) {
            match row {
                Row::Fold { old, new, len } => {
                    seen_old.extend(old..old + len);
                    seen_new.extend(new..new + len);
                }
                Row::Line { old, new, .. } => {
                    seen_old.extend(old);
                    seen_new.extend(new);
                }
                Row::Header { .. } => {}
            }
        }
        assert_eq!(seen_old, (0..d.old.len()).collect::<Vec<_>>());
        assert_eq!(seen_new, (0..d.new.len()).collect::<Vec<_>>());
    }

    #[test]
    fn expand_replaces_fold_with_context_lines() {
        let mut rows = vec![Row::Fold {
            old: 2,
            new: 4,
            len: 2,
        }];
        assert!(expand(&mut rows, 0));
        assert_eq!(
            rows,
            [
                line(Some(2), Some(4), Kind::Context),
                line(Some(3), Some(5), Kind::Context)
            ]
        );
        assert!(!expand(&mut rows, 0), "not a fold");
    }
}

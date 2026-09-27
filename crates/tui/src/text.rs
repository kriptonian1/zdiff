//! How file text shows in the terminal: tabs, control characters, invalid UTF-8, wide characters.

use std::iter::{self, Peekable};
use std::ops::Range;
use std::slice;

use unicode_width::UnicodeWidthChar;
use zdiff_highlight::{Class, Token};

/// Columns a tab expands to; a raw tab would break column alignment.
const TAB_WIDTH: usize = 4;

/// How a run of text is drawn: syntax class, changed-word emphasis, and search match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Look {
    pub class: Option<Class>,
    pub emph: bool,
    pub found: Found,
}

/// Whether text is a search match, and whether it is the current one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Found {
    #[default]
    No,
    Match,
    Current,
}

/// Sorted file byte ranges that style a line; see [`segments`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Marks<'a> {
    pub tokens: &'a [Token],
    pub emph: &'a [Range<u32>],
    pub found: &'a [Range<u32>],
    pub current: Option<&'a Range<u32>>,
}

/// A run of visible text that shares one [`Look`].
pub type Segment = (String, Look);

/// Terminal-safe slice of a line: skips `skip` columns, keeps at most `take`.
///
/// Control characters become `�`, so file content cannot inject escape sequences.
pub fn visible(bytes: &[u8], skip: usize, take: usize) -> String {
    segments(bytes, 0, Marks::default(), skip, take)
        .into_iter()
        .map(|(text, _)| text)
        .collect()
}

/// Like [`visible`], split wherever the [`Look`] from `marks` changes.
///
/// `line_start` is the line's byte offset in the file that `marks` index into.
pub fn segments(
    bytes: &[u8],
    line_start: u32,
    marks: Marks<'_>,
    skip: usize,
    take: usize,
) -> Vec<Segment> {
    let token_span = |t: &Token| (t.start, t.end);
    let range_span = |r: &Range<u32>| (r.start, r.end);
    let mut tokens = from(marks.tokens, line_start, token_span);
    let mut emph = from(marks.emph, line_start, range_span);
    let mut found = from(marks.found, line_start, range_span);
    let mut out: Vec<Segment> = Vec::new();
    let mut column = 0;
    for (offset, c, width) in glyphs(bytes) {
        let end = column + width;
        if end > skip + take {
            break;
        }
        if end > skip {
            let at = line_start as usize + offset;
            let look = Look {
                class: covering(&mut tokens, at, token_span).map(|t| t.class),
                emph: covering(&mut emph, at, range_span).is_some(),
                found: if marks
                    .current
                    .is_some_and(|r| (r.start as usize..r.end as usize).contains(&at))
                {
                    Found::Current
                } else if covering(&mut found, at, range_span).is_some() {
                    Found::Match
                } else {
                    Found::No
                },
            };
            if out.last().is_none_or(|(_, last)| *last != look) {
                out.push((String::new(), look));
            }
            let (text, _) = out.last_mut().expect("a segment was pushed above");
            if column < skip {
                // Wide character cut by the left edge: pad its visible part.
                text.extend(iter::repeat_n(' ', end - skip));
            } else {
                text.push(c);
            }
        }
        column = end;
    }
    out
}

/// Cursor over sorted, non-overlapping `items`, starting at the first that ends after `start`.
fn from<T>(items: &[T], start: u32, span: fn(&T) -> (u32, u32)) -> Peekable<slice::Iter<'_, T>> {
    items[items.partition_point(|i| span(i).1 <= start)..]
        .iter()
        .peekable()
}

/// The item covering byte `at`, advancing `cursor` past items that end before it.
fn covering<'a, T>(
    cursor: &mut Peekable<slice::Iter<'a, T>>,
    at: usize,
    span: fn(&T) -> (u32, u32),
) -> Option<&'a T> {
    while cursor.next_if(|i| span(i).1 as usize <= at).is_some() {}
    cursor.peek().copied().filter(|i| span(i).0 as usize <= at)
}

/// Width of the whole line in terminal columns.
pub fn columns(bytes: &[u8]) -> usize {
    glyphs(bytes).map(|(_, _, width)| width).sum()
}

/// Each displayed character with its source byte offset and width in columns.
fn glyphs(bytes: &[u8]) -> impl Iterator<Item = (usize, char, usize)> + '_ {
    let mut chunk_start = 0;
    bytes
        .utf8_chunks()
        .flat_map(move |chunk| {
            let (start, valid) = (chunk_start, chunk.valid());
            chunk_start += valid.len() + chunk.invalid().len();
            let invalid = (!chunk.invalid().is_empty())
                .then_some((start + valid.len(), char::REPLACEMENT_CHARACTER));
            valid
                .char_indices()
                .map(move |(i, c)| (start + i, c))
                .chain(invalid)
        })
        .flat_map(|(offset, c)| {
            let (shown, count) = match c {
                '\t' => (' ', TAB_WIDTH),
                c if c.is_control() => (char::REPLACEMENT_CHARACTER, 1),
                c => (c, 1),
            };
            iter::repeat_n((offset, shown, shown.width().unwrap_or(0)), count)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_count_tabs_wide_and_replaced_characters() {
        assert_eq!(columns(b"abc"), 3);
        assert_eq!(columns(b"\tx"), TAB_WIDTH + 1);
        assert_eq!(columns("日本".as_bytes()), 4);
        assert_eq!(columns(b"\x1b"), 1);
        assert_eq!(columns(b"a\xffb"), 3);
    }

    #[test]
    fn visible_skips_and_takes_columns() {
        assert_eq!(visible(b"abcdefgh", 2, 3), "cde");
        assert_eq!(visible(b"abc", 5, 3), "");
        assert_eq!(visible(b"abc", 0, 80), "abc");
    }

    #[test]
    fn visible_pads_characters_cut_at_the_edges() {
        assert_eq!(
            visible("日本語".as_bytes(), 1, 4),
            " 本",
            "left half of 日 cut off"
        );
        assert_eq!(
            visible("日本語".as_bytes(), 0, 3),
            "日",
            "本 would cross the right edge"
        );
        assert_eq!(visible(b"\tx", 2, 10), "  x", "rest of the tab stays");
    }

    fn token(start: u32, end: u32, class: Class) -> Token {
        Token { start, end, class }
    }

    fn look(class: Option<Class>, emph: bool) -> Look {
        Look {
            class,
            emph,
            found: Found::No,
        }
    }

    fn marks<'a>(tokens: &'a [Token], emph: &'a [Range<u32>]) -> Marks<'a> {
        Marks {
            tokens,
            emph,
            ..Marks::default()
        }
    }

    #[test]
    fn segments_split_where_the_class_changes() {
        // File "xx" + line "fn main", tokens in file offsets.
        let tokens = [token(2, 4, Class::Keyword), token(5, 9, Class::Entity)];
        assert_eq!(
            segments(b"fn main", 2, marks(&tokens, &[]), 0, 80),
            [
                ("fn".into(), look(Some(Class::Keyword), false)),
                (" ".into(), Look::default()),
                ("main".into(), look(Some(Class::Entity), false)),
            ]
        );
    }

    #[test]
    #[expect(clippy::single_range_in_vec_init, reason = "one emphasized range")]
    fn segments_split_where_emphasis_changes_independent_of_class() {
        let tokens = [token(0, 6, Class::String)];
        assert_eq!(
            segments(b"\"abcd\" x", 0, marks(&tokens, &[3..7]), 0, 80),
            [
                ("\"ab".into(), look(Some(Class::String), false)),
                ("cd\"".into(), look(Some(Class::String), true)),
                (" ".into(), look(None, true)),
                ("x".into(), Look::default()),
            ]
        );
    }

    #[test]
    fn segments_keep_the_class_when_skipping_into_a_token() {
        let tokens = [token(0, 6, Class::String)];
        assert_eq!(
            segments(b"\"abcd\" x", 0, marks(&tokens, &[]), 2, 3),
            [("bcd".into(), look(Some(Class::String), false))]
        );
        let tabbed = [token(1, 2, Class::Keyword)];
        assert_eq!(
            segments(b"\tk", 0, marks(&tabbed, &[]), 2, 10),
            [
                ("  ".into(), Look::default()),
                ("k".into(), look(Some(Class::Keyword), false))
            ]
        );
    }

    #[test]
    fn visible_blocks_escape_sequences() {
        assert_eq!(visible(b"\x1b[31mred", 0, 80), "\u{FFFD}[31mred");
    }

    #[test]
    fn segments_mark_search_matches_and_the_current_one() {
        let current = 6..7;
        let found = [1..2, 6..7];
        let marks = Marks {
            found: &found,
            current: Some(&current),
            ..Marks::default()
        };
        let found_as = |found| Look {
            found,
            ..Look::default()
        };
        assert_eq!(
            segments(b"axxxxxa", 0, marks, 0, 80),
            [
                ("a".into(), Look::default()),
                ("x".into(), found_as(Found::Match)),
                ("xxxx".into(), Look::default()),
                ("a".into(), found_as(Found::Current)),
            ]
        );
    }
}

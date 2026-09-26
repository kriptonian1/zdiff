//! How file text shows in the terminal: tabs, control characters, invalid UTF-8, wide characters.

use std::iter;

use unicode_width::UnicodeWidthChar;
use zdiff_highlight::{Class, Token};

/// Columns a tab expands to; a raw tab would break column alignment.
const TAB_WIDTH: usize = 4;

/// A run of visible text that shares one syntax class.
pub type Segment = (String, Option<Class>);

/// Terminal-safe slice of a line: skips `skip` columns, keeps at most `take`.
///
/// Control characters become `�`, so file content cannot inject escape sequences.
pub fn visible(bytes: &[u8], skip: usize, take: usize) -> String {
    segments(bytes, 0, &[], skip, take)
        .into_iter()
        .map(|(text, _)| text)
        .collect()
}

/// Like [`visible`], split wherever the syntax class from `tokens` changes.
///
/// `line_start` is the line's byte offset in the file that `tokens` index into.
pub fn segments(
    bytes: &[u8],
    line_start: u32,
    tokens: &[Token],
    skip: usize,
    take: usize,
) -> Vec<Segment> {
    let first = tokens.partition_point(|t| t.end <= line_start);
    let mut tokens = tokens[first..].iter().peekable();
    let mut out: Vec<Segment> = Vec::new();
    let mut column = 0;
    for (offset, c, width) in glyphs(bytes) {
        let end = column + width;
        if end > skip + take {
            break;
        }
        if end > skip {
            let at = line_start as usize + offset;
            while tokens.next_if(|t| t.end as usize <= at).is_some() {}
            let class = tokens
                .peek()
                .filter(|t| t.start as usize <= at)
                .map(|t| t.class);
            if out.last().is_none_or(|(_, last)| *last != class) {
                out.push((String::new(), class));
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

    #[test]
    fn segments_split_where_the_class_changes() {
        // File "xx" + line "fn main", tokens in file offsets.
        let tokens = [token(2, 4, Class::Keyword), token(5, 9, Class::Entity)];
        assert_eq!(
            segments(b"fn main", 2, &tokens, 0, 80),
            [
                ("fn".into(), Some(Class::Keyword)),
                (" ".into(), None),
                ("main".into(), Some(Class::Entity)),
            ]
        );
    }

    #[test]
    fn segments_keep_the_class_when_skipping_into_a_token() {
        let tokens = [token(0, 6, Class::String)];
        assert_eq!(
            segments(b"\"abcd\" x", 0, &tokens, 2, 3),
            [("bcd".into(), Some(Class::String))]
        );
        let tabbed = [token(1, 2, Class::Keyword)];
        assert_eq!(
            segments(b"\tk", 0, &tabbed, 2, 10),
            [("  ".into(), None), ("k".into(), Some(Class::Keyword))]
        );
    }

    #[test]
    fn visible_blocks_escape_sequences() {
        assert_eq!(visible(b"\x1b[31mred", 0, 80), "\u{FFFD}[31mred");
    }
}

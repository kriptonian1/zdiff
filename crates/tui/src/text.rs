//! How file text shows in the terminal: tabs, control characters, invalid UTF-8, wide characters.

use std::iter;

use unicode_width::UnicodeWidthChar;

/// Columns a tab expands to; a raw tab would break column alignment.
const TAB_WIDTH: usize = 4;

/// Terminal-safe slice of a line: skips `skip` columns, keeps at most `take`.
///
/// Control characters become `�`, so file content cannot inject escape sequences.
pub fn visible(bytes: &[u8], skip: usize, take: usize) -> String {
    let mut out = String::with_capacity(take);
    let mut column = 0;
    for (c, width) in glyphs(bytes) {
        let end = column + width;
        if end > skip + take {
            break;
        }
        if end > skip {
            if column < skip {
                // Wide character cut by the left edge: pad its visible part.
                out.extend(iter::repeat_n(' ', end - skip));
            } else {
                out.push(c);
            }
        }
        column = end;
    }
    out
}

/// Width of the whole line in terminal columns.
pub fn columns(bytes: &[u8]) -> usize {
    glyphs(bytes).map(|(_, width)| width).sum()
}

/// Each displayed character with its width in columns.
fn glyphs(bytes: &[u8]) -> impl Iterator<Item = (char, usize)> + '_ {
    bytes
        .utf8_chunks()
        .flat_map(|chunk| {
            let invalid = (!chunk.invalid().is_empty()).then_some(char::REPLACEMENT_CHARACTER);
            chunk.valid().chars().chain(invalid)
        })
        .flat_map(|c| {
            let (shown, count) = match c {
                '\t' => (' ', TAB_WIDTH),
                c if c.is_control() => (char::REPLACEMENT_CHARACTER, 1),
                c => (c, 1),
            };
            iter::repeat_n((shown, shown.width().unwrap_or(0)), count)
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

    #[test]
    fn visible_blocks_escape_sequences() {
        assert_eq!(visible(b"\x1b[31mred", 0, 80), "\u{FFFD}[31mred");
    }
}

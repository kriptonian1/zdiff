//! Line diff of two byte buffers, independent of any repository.

use std::ops::Range;

use gix::diff::blob::{Algorithm, Diff, InternedInput};

/// Bytes scanned for NUL in binary detection; same limit as git.
const BINARY_SNIFF_LEN: usize = 8000;
/// Larger files are treated as binary so line offsets fit in `u32`.
const MAX_TEXT_LEN: usize = u32::MAX as usize;

/// Both versions of one file plus the hunks that turn `old` into `new`.
#[derive(Debug)]
pub struct FileDiff {
    pub old: Text,
    pub new: Text,
    /// Empty when the file is binary or both sides are equal.
    pub hunks: Box<[Hunk]>,
    pub binary: bool,
}

/// One changed region, as zero-based line index ranges into each side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old: Range<u32>,
    pub new: Range<u32>,
}

/// One side of a diff: the file bytes plus where each line starts.
#[derive(Debug, Default)]
pub struct Text {
    bytes: Box<[u8]>,
    starts: Box<[u32]>,
}

impl Text {
    fn indexed(bytes: Vec<u8>) -> Self {
        let offset = |i: usize| u32::try_from(i).expect("text over 4 GiB is treated as binary");
        let mut starts = Vec::with_capacity(bytes.len() / 32 + 1);
        if !bytes.is_empty() {
            starts.push(0);
        }
        starts.extend(
            memchr::memchr_iter(b'\n', &bytes)
                .map(|i| i + 1)
                .filter(|&i| i < bytes.len())
                .map(offset),
        );
        Self {
            bytes: bytes.into_boxed_slice(),
            starts: starts.into_boxed_slice(),
        }
    }

    fn unindexed(bytes: Vec<u8>) -> Self {
        Self {
            bytes: bytes.into_boxed_slice(),
            starts: Box::default(),
        }
    }

    /// The raw file content.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Number of lines; zero for binary content.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "line count <= byte length <= MAX_TEXT_LEN"
    )]
    pub fn len(&self) -> u32 {
        self.starts.len() as u32
    }

    /// Whether there are no lines.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.starts.is_empty()
    }

    /// Line `i` (zero-based) without its `\n` or `\r\n` ending.
    ///
    /// # Panics
    /// Panics if `i >= self.len()`.
    #[must_use]
    pub fn line(&self, i: u32) -> &[u8] {
        let i = i as usize;
        let start = self.starts[i] as usize;
        let end = self
            .starts
            .get(i + 1)
            .map_or(self.bytes.len(), |&e| e as usize);
        let line = &self.bytes[start..end];
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        line.strip_suffix(b"\r").unwrap_or(line)
    }
}

impl FileDiff {
    /// Diffs `old` against `new` line by line.
    #[must_use]
    pub fn new(old: Vec<u8>, new: Vec<u8>) -> Self {
        let binary = is_binary(&old) || is_binary(&new);
        let hunks = if binary {
            Box::default()
        } else {
            let input = InternedInput::new(old.as_slice(), new.as_slice());
            let mut diff = Diff::compute(Algorithm::Histogram, &input);
            diff.postprocess_lines(&input);
            diff.hunks()
                .map(|h| Hunk {
                    old: h.before,
                    new: h.after,
                })
                .collect()
        };
        let text = if binary {
            Text::unindexed
        } else {
            Text::indexed
        };
        Self {
            old: text(old),
            new: text(new),
            hunks,
            binary,
        }
    }

    /// Lines added and removed, as `(added, removed)`.
    #[must_use]
    pub fn stats(&self) -> (u32, u32) {
        self.hunks.iter().fold((0, 0), |(added, removed), h| {
            (
                added + h.new.end - h.new.start,
                removed + h.old.end - h.old.start,
            )
        })
    }
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.len() > MAX_TEXT_LEN || bytes[..bytes.len().min(BINARY_SNIFF_LEN)].contains(&0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunks(old: &str, new: &str) -> Vec<Hunk> {
        FileDiff::new(old.into(), new.into()).hunks.into_vec()
    }

    fn hunk(old: Range<u32>, new: Range<u32>) -> Hunk {
        Hunk { old, new }
    }

    fn lines(text: &str) -> Vec<Vec<u8>> {
        let t = Text::indexed(text.into());
        (0..t.len()).map(|i| t.line(i).to_vec()).collect()
    }

    #[test]
    fn lines_strip_endings_and_keep_last_unterminated_line() {
        assert_eq!(lines("a\nb\n"), [b"a".to_vec(), b"b".to_vec()]);
        assert_eq!(lines("a\r\nb"), [b"a".to_vec(), b"b".to_vec()]);
        assert_eq!(lines("\n\n"), [Vec::new(), Vec::new()]);
        assert!(lines("").is_empty());
    }

    #[test]
    fn line_count_matches_the_diff_tokenizer() {
        let d = FileDiff::new(b"a\nb".to_vec(), b"a\nb\nc\n".to_vec());
        assert_eq!((d.old.len(), d.new.len()), (2, 3));
        assert!(d.hunks.iter().all(|h| h.old.end <= 2 && h.new.end <= 3));
    }

    #[test]
    fn stats_count_added_and_removed_lines() {
        let d = FileDiff::new(b"a\nb\nc\n".to_vec(), b"a\nX\nY\nc\nd\n".to_vec());
        assert_eq!(d.stats(), (3, 1));
    }

    #[test]
    fn identical_inputs_have_no_hunks() {
        assert!(hunks("a\nb\n", "a\nb\n").is_empty());
    }

    #[test]
    fn insert_delete_and_replace() {
        assert_eq!(hunks("a\nc\n", "a\nb\nc\n"), [hunk(1..1, 1..2)]);
        assert_eq!(hunks("a\nb\nc\n", "a\nc\n"), [hunk(1..2, 1..1)]);
        assert_eq!(hunks("a\nb\nc\n", "a\nX\nc\n"), [hunk(1..2, 1..2)]);
        assert_eq!(hunks("", "a\n"), [hunk(0..0, 0..1)]);
    }

    #[test]
    fn missing_trailing_newline_changes_last_line() {
        assert_eq!(hunks("a\nb\n", "a\nb"), [hunk(1..2, 1..2)]);
    }

    #[test]
    fn binary_has_no_hunks() {
        let d = FileDiff::new(b"a\0b".to_vec(), b"text\n".to_vec());
        assert!(d.binary);
        assert!(d.hunks.is_empty());
        assert!(!FileDiff::new(b"a\n".to_vec(), b"b\n".to_vec()).binary);
    }

    #[test]
    fn hunks_stay_within_line_counts() {
        let old = "1\n2\n3\n4\n5\n6\n";
        let new = "0\n1\n3\nX\n5\n6\n7\n";
        let lines = |s: &str| u32::try_from(s.lines().count()).unwrap();
        for h in hunks(old, new) {
            assert!(h.old.end <= lines(old) && h.new.end <= lines(new), "{h:?}");
        }
    }
}

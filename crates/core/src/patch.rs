//! Reading a patch file: the one place zdiff parses unified-diff text, for `--patch`.
//!
//! A patch holds hunks, not whole files, so [`Patch::diff`] gives a partial [`FileDiff`]: real
//! line numbers, with the lines between hunks left as empty placeholders.

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;

use crate::Error;
use crate::diff::{FileDiff, Hunk};
use crate::repo::{Repo, Status};

/// A parsed patch: every file it touches, in order.
#[derive(Debug)]
pub struct Patch {
    bytes: Box<[u8]>,
    files: Vec<PatchFile>,
    subject: Option<String>,
}

/// One file in a patch.
#[derive(Debug)]
pub struct PatchFile {
    /// The new path; the old one for a deleted file.
    pub path: PathBuf,
    pub status: Status,
    binary: bool,
    hunks: Vec<PatchHunk>,
    /// Where the file was before the change; differs from `path` for a rename.
    old_path: PathBuf,
    /// Blob id prefixes from `index a..b`, for finding the real files in a repository.
    blobs: Option<(String, String)>,
    /// Whether the old and the new file end without a newline.
    no_eol: (bool, bool),
}

/// One `@@` hunk: where it starts on each side and its lines, as byte ranges into the patch.
#[derive(Debug)]
struct PatchHunk {
    old: (u32, u32),
    new: (u32, u32),
    lines: Vec<(u8, Range<usize>)>,
}

/// A file before its headers are read: modified unless they say added or deleted.
impl Default for PatchFile {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            status: Status::Modified,
            binary: false,
            hunks: Vec::new(),
            old_path: PathBuf::new(),
            blobs: None,
            no_eol: (false, false),
        }
    }
}

impl PatchFile {
    /// Lines added and removed, as `(added, removed)`.
    #[must_use]
    pub fn stats(&self) -> (u32, u32) {
        let count = |marker| {
            let lines = self.hunks.iter().flat_map(|h| &h.lines);
            u32::try_from(lines.filter(|(m, _)| *m == marker).count()).unwrap_or(u32::MAX)
        };
        (count(b'+'), count(b'-'))
    }
}

impl Patch {
    /// Reads a `git diff`, `git format-patch`, or `diff -u` patch.
    ///
    /// # Errors
    /// Returns [`Error::Patch`] for merge diffs, hunks whose lines don't add up to their
    /// header's counts, and text with no file headers at all.
    pub fn parse(bytes: Vec<u8>) -> Result<Self, Error> {
        let mut parser = Parser {
            bytes: &bytes,
            lines: line_ranges(&bytes),
            at: 0,
            files: Vec::new(),
            subject: None,
        };
        parser.run()?;
        let (mut files, subject) = (parser.files, parser.subject);
        if files.is_empty() {
            return Err(Error::Patch {
                line: 1,
                reason: "not a patch",
            });
        }
        number_repeats(&mut files);
        Ok(Self {
            bytes: bytes.into(),
            files,
            subject,
        })
    }

    #[must_use]
    pub fn files(&self) -> &[PatchFile] {
        &self.files
    }

    /// The email subject of a `git format-patch`, without its `[PATCH]` tag.
    #[must_use]
    pub fn subject(&self) -> Option<&str> {
        self.subject.as_deref()
    }

    /// File `i`'s hunks as a diff with real line numbers; unknown lines are empty placeholders.
    ///
    /// # Panics
    /// Panics if `i` is not below `self.files().len()`.
    #[must_use]
    pub fn diff(&self, i: usize) -> FileDiff {
        let file = &self.files[i];
        if file.binary {
            return FileDiff::binary();
        }
        let (mut old, mut new) = (Side::default(), Side::default());
        let (mut hunks, mut known) = (Vec::new(), Vec::new());
        // ponytail: each placeholder line costs 4 bytes of line index; phase 2 loads real files.
        for hunk in &file.hunks {
            old.pad_to(first_line(hunk.old));
            new.pad_to(first_line(hunk.new));
            let (known_old, known_new) = (old.lines, new.lines);
            let mut run: Option<(u32, u32)> = None;
            for (marker, range) in &hunk.lines {
                let line = &self.bytes[range.clone()];
                if *marker == b' ' {
                    end_run(&mut run, &mut hunks, (&old, &new));
                    old.push(line);
                    new.push(line);
                    continue;
                }
                run.get_or_insert((old.lines, new.lines));
                if *marker == b'-' {
                    old.push(line);
                } else {
                    new.push(line);
                }
            }
            end_run(&mut run, &mut hunks, (&old, &new));
            known.push(Hunk {
                old: known_old..old.lines,
                new: known_new..new.lines,
            });
        }
        FileDiff::partial(old.bytes, new.bytes, hunks, known)
    }
}

impl Patch {
    /// File `i` with full context when its original can be found in `repo`: both blobs, the
    /// old blob plus the hunks, or the worktree file plus the hunks. Otherwise, and for
    /// binary files, the hunks only, as [`Patch::diff`] gives.
    ///
    /// # Panics
    /// Panics if `i` is not below `self.files().len()`.
    #[must_use]
    pub fn full_diff(&self, i: usize, repo: Option<&Repo>) -> FileDiff {
        let file = &self.files[i];
        let Some(repo) = repo.filter(|_| !file.binary) else {
            return self.diff(i);
        };
        // An empty id is a side with no file, like the old side of an added file.
        let blob = |hex: &str| {
            if hex.is_empty() {
                Some(Vec::new())
            } else {
                repo.blob(hex)
            }
        };
        let (old_id, new_id) = file.blobs.clone().unwrap_or_default();
        if file.blobs.is_some()
            && let (Some(old), Some(new)) = (blob(&old_id), blob(&new_id))
        {
            return FileDiff::new(old, new);
        }
        let base = (file.blobs.is_some())
            .then(|| blob(&old_id))
            .flatten()
            .or_else(|| match file.status {
                Status::Added => Some(Vec::new()),
                _ => repo.worktree_file(&file.old_path),
            });
        match base.and_then(|old| Some((self.apply(&old, file)?, old))) {
            Some((new, old)) => FileDiff::new(old, new),
            None => self.diff(i),
        }
    }

    /// `old` with `file`'s hunks applied at their exact line numbers; `None` when a context
    /// or removed line doesn't match, so a patch for other content never shows wrong text.
    // ponytail: exact positions only; search nearby lines like `git apply` if drifted bases
    // turn up.
    fn apply(&self, old: &[u8], file: &PatchFile) -> Option<Vec<u8>> {
        let mut lines: Vec<&[u8]> = old.split(|&b| b == b'\n').collect();
        let old_eol = old.is_empty() || old.ends_with(b"\n");
        if old_eol {
            lines.pop();
        }
        let mut out = Vec::with_capacity(old.len() + 64);
        let mut at = 0;
        let push = |out: &mut Vec<u8>, line: &[u8]| {
            out.extend_from_slice(line);
            out.push(b'\n');
        };
        for hunk in &file.hunks {
            let start = first_line(hunk.old) as usize;
            if start < at || start > lines.len() {
                return None;
            }
            lines[at..start]
                .iter()
                .for_each(|line| push(&mut out, line));
            at = start;
            for (marker, range) in &hunk.lines {
                let line = &self.bytes[range.clone()];
                if *marker != b'+' {
                    if lines.get(at) != Some(&line) {
                        return None;
                    }
                    at += 1;
                }
                if *marker != b'-' {
                    push(&mut out, line);
                }
            }
        }
        let touched_end = at == lines.len();
        lines[at..].iter().for_each(|line| push(&mut out, line));
        // The last line's newline is the patch's to say when a hunk reached the end.
        let new_eol = if touched_end { !file.no_eol.1 } else { old_eol };
        if !new_eol {
            out.pop();
        }
        Some(out)
    }
}

/// One side of a partial diff while it's built.
#[derive(Default)]
struct Side {
    bytes: Vec<u8>,
    lines: u32,
}

impl Side {
    fn push(&mut self, line: &[u8]) {
        self.bytes.extend_from_slice(line);
        self.bytes.push(b'\n');
        self.lines += 1;
    }

    /// Empty placeholder lines up to zero-based line `line`.
    fn pad_to(&mut self, line: u32) {
        while self.lines < line {
            self.push(b"");
        }
    }
}

/// Closes a run of changed lines into a hunk.
fn end_run(run: &mut Option<(u32, u32)>, hunks: &mut Vec<Hunk>, (old, new): (&Side, &Side)) {
    if let Some((o, n)) = run.take() {
        hunks.push(Hunk {
            old: o..old.lines,
            new: n..new.lines,
        });
    }
}

/// Zero-based first line of a hunk side given as `(start, count)`; an empty side's start is
/// the line it follows, as in `@@ -0,0` or `@@ -5,0`.
fn first_line((start, count): (u32, u32)) -> u32 {
    if count == 0 {
        start
    } else {
        start.saturating_sub(1)
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    lines: Vec<Range<usize>>,
    at: usize,
    files: Vec<PatchFile>,
    subject: Option<String>,
}

impl Parser<'_> {
    fn line(&self, i: usize) -> &[u8] {
        let line = &self.bytes[self.lines[i].clone()];
        line.strip_suffix(b"\r").unwrap_or(line)
    }

    fn error(&self, reason: &'static str) -> Error {
        Error::Patch {
            line: self.at + 1,
            reason,
        }
    }

    fn run(&mut self) -> Result<(), Error> {
        while self.at < self.lines.len() {
            let line = self.line(self.at);
            if line.starts_with(b"diff --cc ") || line.starts_with(b"diff --combined ") {
                return Err(self.error("merge diffs aren't supported"));
            }
            if let Some(rest) = line.strip_prefix(b"diff --git ") {
                let (old_path, path) = git_header_paths(rest);
                self.files.push(PatchFile {
                    path,
                    old_path,
                    ..PatchFile::default()
                });
            } else if self.plain_start() {
                self.files.push(PatchFile::default());
                continue;
            } else if self.files.is_empty() {
                if let Some(subject) = line.strip_prefix(b"Subject: ") {
                    self.subject = Some(clean_subject(subject));
                }
            } else {
                self.file_line()?;
                continue;
            }
            self.at += 1;
        }
        Ok(())
    }

    /// A `--- `/`+++ ` pair that starts a file in a plain `diff -u`, outside a git header.
    fn plain_start(&self) -> bool {
        let starts =
            |i: usize, prefix: &[u8]| i < self.lines.len() && self.line(i).starts_with(prefix);
        let fresh = self.files.last().is_none_or(|f| !f.hunks.is_empty());
        fresh && starts(self.at, b"--- ") && starts(self.at + 1, b"+++ ")
    }

    /// One line inside the current file: a header line, a hunk, or something to skip.
    fn file_line(&mut self) -> Result<(), Error> {
        let line = self.line(self.at).to_vec();
        let file = self.files.last_mut().expect("called inside a file");
        if line.starts_with(b"@@@") {
            return Err(self.error("merge diffs aren't supported"));
        }
        if line.starts_with(b"@@ ") {
            return self.hunk(&line);
        }
        if line.starts_with(b"new file mode") {
            file.status = Status::Added;
        } else if line.starts_with(b"deleted file mode") {
            file.status = Status::Deleted;
        } else if let Some(path) = line.strip_prefix(b"rename to ") {
            file.path = path_of(path, "");
        } else if let Some(path) = line.strip_prefix(b"rename from ") {
            file.old_path = path_of(path, "");
        } else if let Some(ids) = line.strip_prefix(b"index ") {
            file.blobs = blob_ids(ids);
        } else if let Some(path) = line.strip_prefix(b"--- ") {
            if path == b"/dev/null" {
                file.status = Status::Added;
            } else {
                file.old_path = path_of(path, "a/");
                if file.path.as_os_str().is_empty() {
                    file.path.clone_from(&file.old_path);
                }
            }
        } else if let Some(path) = line.strip_prefix(b"+++ ") {
            if path == b"/dev/null" {
                file.status = Status::Deleted;
            } else {
                file.path = path_of(path, "b/");
            }
        } else if line.starts_with(b"Binary files ") {
            file.binary = true;
        } else if line.starts_with(b"GIT binary patch") {
            file.binary = true;
            // The base85 payload runs until the next file.
            while self.at + 1 < self.lines.len()
                && !self.line(self.at + 1).starts_with(b"diff --git ")
            {
                self.at += 1;
            }
        }
        self.at += 1;
        Ok(())
    }

    /// Reads one hunk, whose header is at the current line.
    fn hunk(&mut self, header: &[u8]) -> Result<(), Error> {
        let (old, new) = hunk_header(header).ok_or_else(|| self.error("bad hunk header"))?;
        let (mut old_left, mut new_left) = (old.1, new.1);
        let mut lines = Vec::new();
        let mut no_eols = (false, false);
        self.at += 1;
        while old_left > 0 || new_left > 0 {
            let Some(range) = self.lines.get(self.at).cloned() else {
                return Err(self.error("hunk line counts don't match"));
            };
            let raw = &self.bytes[range.clone()];
            // Some editors strip the space that marks an empty context line.
            let marker = raw.first().copied().unwrap_or(b' ');
            let content = (range.start + usize::from(!raw.is_empty()))..range.end;
            let (old_take, new_take) = match marker {
                b' ' => (1, 1),
                b'-' => (1, 0),
                b'+' => (0, 1),
                b'\\' => (0, 0),
                _ => return Err(self.error("hunk line counts don't match")),
            };
            if old_left < old_take || new_left < new_take {
                return Err(self.error("hunk line counts don't match"));
            }
            (old_left, new_left) = (old_left - old_take, new_left - new_take);
            if marker == b'\\' {
                no_eol(&mut no_eols, lines.last());
            } else {
                lines.push((marker, content));
            }
            self.at += 1;
        }
        if self.at < self.lines.len() && self.line(self.at).starts_with(b"\\") {
            no_eol(&mut no_eols, lines.last());
            self.at += 1;
        }
        let file = self.files.last_mut().expect("called inside a file");
        file.no_eol.0 |= no_eols.0;
        file.no_eol.1 |= no_eols.1;
        file.hunks.push(PatchHunk { old, new, lines });
        Ok(())
    }
}

/// Byte ranges of each line, without the `\n`.
fn line_ranges(bytes: &[u8]) -> Vec<Range<usize>> {
    let mut ranges = Vec::with_capacity(bytes.len() / 40 + 1);
    let mut start = 0;
    for end in memchr::memchr_iter(b'\n', bytes) {
        ranges.push(start..end);
        start = end + 1;
    }
    if start < bytes.len() {
        ranges.push(start..bytes.len());
    }
    ranges
}

/// `-a,b +c,d` from `@@ -a,b +c,d @@ …`, a missing count meaning 1.
fn hunk_header(line: &[u8]) -> Option<((u32, u32), (u32, u32))> {
    let text = std::str::from_utf8(line).ok()?;
    let mut parts = text.strip_prefix("@@ ")?.split(' ');
    let side = |part: Option<&str>, sign: char| -> Option<(u32, u32)> {
        let part = part?.strip_prefix(sign)?;
        let (start, count) = part.split_once(',').unwrap_or((part, "1"));
        Some((start.parse().ok()?, count.parse().ok()?))
    };
    Some((side(parts.next(), '-')?, side(parts.next(), '+')?))
}

/// The old and new paths from `diff --git a/x b/y`, used until `---`, `+++`, or a rename
/// line say better.
fn git_header_paths(rest: &[u8]) -> (PathBuf, PathBuf) {
    if rest.first() == Some(&b'"') {
        let end = closing_quote(rest).map_or(rest.len(), |i| i + 1);
        let new = rest[end..].strip_prefix(b" ").unwrap_or(&rest[end..]);
        return (path_of(&rest[..end], "a/"), path_of(new, "b/"));
    }
    let text = String::from_utf8_lossy(rest);
    match text.rfind(" b/") {
        Some(i) => (
            path_of(text[..i].as_bytes(), "a/"),
            path_of(text[i + 1..].as_bytes(), "b/"),
        ),
        None => (path_of(rest, "a/"), path_of(rest, "a/")),
    }
}

/// `abc1234..def5678 100644` as its two ids; an all-zero id (no file on that side) is empty.
fn blob_ids(ids: &[u8]) -> Option<(String, String)> {
    let text = std::str::from_utf8(ids).ok()?;
    let range = text.split(' ').next()?;
    let (old, new) = range.split_once("..")?;
    let id = |hex: &str| {
        let real = hex.bytes().any(|b| b != b'0');
        if real { hex.to_owned() } else { String::new() }
    };
    Some((id(old), id(new)))
}

/// Notes a `\ No newline at end of file` for the side(s) of the line `after`.
fn no_eol(flags: &mut (bool, bool), after: Option<&(u8, Range<usize>)>) {
    match after.map(|(marker, _)| *marker) {
        Some(b'-') => flags.0 = true,
        Some(b'+') => flags.1 = true,
        Some(_) => *flags = (true, true),
        None => {}
    }
}

/// A path from a header: unquoted, without `prefix`, and without a trailing tab and timestamp.
fn path_of(raw: &[u8], prefix: &str) -> PathBuf {
    let bytes = if raw.first() == Some(&b'"') {
        unquote(raw)
    } else {
        raw.split(|&b| b == b'\t').next().unwrap_or(raw).to_vec()
    };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    PathBuf::from(text.strip_prefix(prefix).unwrap_or(&text))
}

/// Index of the quote closing the C-style quoted string that `quoted` starts with.
fn closing_quote(quoted: &[u8]) -> Option<usize> {
    let mut escaped = false;
    for (i, &b) in quoted.iter().enumerate().skip(1) {
        match (escaped, b) {
            (false, b'\\') => escaped = true,
            (false, b'"') => return Some(i),
            _ => escaped = false,
        }
    }
    None
}

/// Git's C-style quoting: `"caf\303\251.txt"` is `café.txt`.
fn unquote(quoted: &[u8]) -> Vec<u8> {
    let end = closing_quote(quoted).unwrap_or(quoted.len());
    let inner = &quoted[1..end.max(1)];
    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        if inner[i] != b'\\' || i + 1 == inner.len() {
            out.push(inner[i]);
            i += 1;
            continue;
        }
        let next = inner[i + 1];
        let octal = inner
            .get(i + 1..i + 4)
            .filter(|d| d.iter().all(|b| (b'0'..=b'7').contains(b)));
        if let Some(digits) = octal {
            let value = digits.iter().fold(0u32, |n, d| n * 8 + u32::from(d - b'0'));
            out.push(u8::try_from(value).unwrap_or(b'?'));
            i += 4;
            continue;
        }
        out.push(match next {
            b'n' => b'\n',
            b't' => b'\t',
            other => other,
        });
        i += 2;
    }
    out
}

/// `[PATCH 2/3] Fix it` as `Fix it`.
fn clean_subject(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let text = text.trim();
    match text
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("] "))
    {
        Some((_, subject)) => subject.to_owned(),
        None => text.to_owned(),
    }
}

/// Gives a path seen again in a patch series a ` (2)`, ` (3)`, … suffix, so each stays apart.
fn number_repeats(files: &mut [PatchFile]) {
    let mut seen: HashMap<PathBuf, usize> = HashMap::new();
    for file in files {
        let count = seen.entry(file.path.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            let mut name = file.path.clone().into_os_string();
            name.push(format!(" ({count})"));
            file.path = name.into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rows::Row;

    const SAMPLE: &[u8] = include_bytes!("../../../example/sample.patch");

    fn parse(text: &str) -> Result<Patch, Error> {
        Patch::parse(text.as_bytes().to_vec())
    }

    #[test]
    fn the_sample_lists_every_file_with_its_status_and_counts() {
        let patch = Patch::parse(SAMPLE.to_vec()).expect("the sample parses");
        let files: Vec<(String, Status)> = (patch.files().iter())
            .map(|f| (f.path.display().to_string(), f.status))
            .collect();
        assert_eq!(
            files,
            [
                ("VERSION".into(), Status::Modified),
                ("assets/logo.png".into(), Status::Modified),
                ("docs/café.txt".into(), Status::Modified),
                ("docs/guide.md".into(), Status::Modified),
                ("src/config.rs".into(), Status::Modified),
                ("src/greet.rs".into(), Status::Modified),
                ("src/legacy.rs".into(), Status::Deleted),
                ("src/main.rs".into(), Status::Added),
            ]
        );
        assert_eq!(
            patch.subject(),
            Some("Make the timeout configurable in milliseconds")
        );
        let (added, removed) = (patch.files().iter().map(PatchFile::stats))
            .fold((0, 0), |(a, r), (fa, fr)| (a + fa, r + fr));
        assert_eq!(
            (added, removed),
            (13, 8),
            "the diffstat's 13 insertions, 8 deletions"
        );
        assert!(patch.diff(1).binary, "the logo is a binary patch");
    }

    #[test]
    fn hunks_keep_their_real_line_numbers_with_gaps_between() {
        let patch = Patch::parse(SAMPLE.to_vec()).expect("the sample parses");
        let config = patch
            .files()
            .iter()
            .position(|f| f.path.ends_with("config.rs"));
        let diff = patch.diff(config.expect("config.rs is in the patch"));
        let known = diff.known.as_deref().expect("a partial diff");
        assert_eq!(known.len(), 2);
        assert_eq!(
            (known[0].old.start, known[1].old.start),
            (4, 40),
            "@@ -5 and @@ -41"
        );
        assert_eq!(diff.new.line(4), b"pub struct Config {");
        assert_eq!(diff.new.line(7), b"    pub timeout_ms: u64,");
        assert_eq!(diff.new.line(8), b"    pub retries: u8,");
        let gaps: Vec<u32> = (diff.rows(3).iter())
            .filter_map(|row| match *row {
                Row::Gap { len, .. } => Some(len),
                _ => None,
            })
            .collect();
        assert_eq!(gaps, [4, 29], "lines 1-4, then old lines 12-40");
        assert_eq!(
            diff.rows(u32::MAX),
            diff.rows(3),
            "context can't reveal unknown lines"
        );
        let mut rows = diff.rows(3);
        let gap = rows
            .iter()
            .position(|r| matches!(r, Row::Gap { .. }))
            .expect("a gap");
        assert!(
            !crate::rows::expand(&mut rows, gap),
            "a gap can't be opened"
        );
        let unknown = crate::rows::locate(&rows, crate::Side::Old, 20);
        assert_eq!(unknown, None, "old line 21 isn't in the patch");
    }

    #[test]
    fn plain_diff_u_output_and_the_short_binary_form() {
        let patch = parse(
            "--- a/x.txt\t2026-01-01 10:00:00\n+++ b/x.txt\t2026-01-02 10:00:00\n\
             @@ -1,2 +1,2 @@\n keep\n-old\n+new\n\
             --- a/y.txt\n+++ b/y.txt\n@@ -1 +1 @@\n-a\n+b\n",
        )
        .expect("diff -u parses");
        let paths: Vec<_> = patch.files().iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, [PathBuf::from("x.txt"), PathBuf::from("y.txt")]);
        assert_eq!(patch.files()[0].stats(), (1, 1));

        let patch = parse(
            "diff --git a/i.png b/i.png\nindex 1..2 100644\n\
             Binary files a/i.png and b/i.png differ\n",
        )
        .expect("a binary-only patch parses");
        assert!(patch.diff(0).binary);
    }

    #[test]
    fn a_new_file_and_a_pure_insertion_line_up() {
        let patch = parse(
            "diff --git a/n.rs b/n.rs\nnew file mode 100644\n--- /dev/null\n+++ b/n.rs\n\
             @@ -0,0 +1,2 @@\n+one\n+two\n\
             diff --git a/m.rs b/m.rs\n--- a/m.rs\n+++ b/m.rs\n@@ -5,0 +6,1 @@\n+inserted\n",
        )
        .expect("parses");
        assert_eq!(patch.files()[0].status, Status::Added);
        let diff = patch.diff(0);
        assert_eq!((diff.old.len(), diff.new.len()), (0, 2));
        let diff = patch.diff(1);
        assert_eq!(diff.new.line(5), b"inserted", "after line 5, so new line 6");
        assert_eq!(diff.hunks[0].old, 5..5);
    }

    #[test]
    fn errors_say_where_the_patch_broke() {
        let line = |result: Result<Patch, Error>| match result {
            Err(Error::Patch { line, reason }) => (line, reason),
            other => panic!("expected a patch error, got {other:?}"),
        };
        assert_eq!(line(parse("just some text\n")), (1, "not a patch"));
        assert_eq!(
            line(parse("diff --cc x\n")),
            (1, "merge diffs aren't supported")
        );
        let short = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,3 +1,3 @@\n a\n-b\n";
        assert_eq!(line(parse(short)), (7, "hunk line counts don't match"));
    }

    #[test]
    fn a_series_that_repeats_a_path_numbers_the_repeat() {
        let one = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n";
        let patch = parse(&format!("{one}{one}")).expect("parses");
        let paths: Vec<_> = patch.files().iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, [PathBuf::from("x"), PathBuf::from("x (2)")]);
    }

    #[test]
    fn quoted_paths_unescape_octal_bytes() {
        assert_eq!(
            unquote(br#""caf\303\251 \"x\".txt""#),
            "café \"x\".txt".as_bytes()
        );
        assert_eq!(
            git_header_paths(br#""a/caf\303\251.txt" "b/caf\303\251.txt""#),
            (PathBuf::from("café.txt"), PathBuf::from("café.txt"))
        );
    }

    fn applied(patch: &str, old: &str) -> Option<String> {
        let patch = parse(patch).expect("parses");
        let new = patch.apply(old.as_bytes(), &patch.files()[0])?;
        Some(String::from_utf8(new).expect("utf-8"))
    }

    #[test]
    fn apply_rebuilds_the_new_file_or_refuses_other_content() {
        let edit = "--- a/x\n+++ b/x\n@@ -2,2 +2,3 @@\n b\n-c\n+C\n+D\n";
        assert_eq!(
            applied(edit, "a\nb\nc\nd\n").as_deref(),
            Some("a\nb\nC\nD\nd\n")
        );
        assert_eq!(
            applied(edit, "a\nB\nc\nd\n"),
            None,
            "a context line differs"
        );
        assert_eq!(applied(edit, "a\n"), None, "the file is too short");

        let insert = "--- a/x\n+++ b/x\n@@ -1,0 +2,1 @@\n+new\n";
        assert_eq!(applied(insert, "a\nb\n").as_deref(), Some("a\nnew\nb\n"));
        let delete_all = "--- a/x\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-a\n-b\n";
        assert_eq!(applied(delete_all, "a\nb\n").as_deref(), Some(""));
        let crlf = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\r\n+b\r\n";
        assert_eq!(
            applied(crlf, "a\r\n").as_deref(),
            Some("b\r\n"),
            "CRLF kept"
        );
    }

    #[test]
    fn apply_honors_missing_final_newlines() {
        let lose = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n\\ No newline at end of file\n";
        assert_eq!(applied(lose, "a\n").as_deref(), Some("b"));
        let gain = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n\\ No newline at end of file\n+b\n";
        assert_eq!(applied(gain, "a").as_deref(), Some("b\n"));
        let tail = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n";
        assert_eq!(
            applied(tail, "a\nend").as_deref(),
            Some("b\nend"),
            "untouched end keeps its form"
        );
    }

    #[test]
    fn index_lines_give_blob_ids_and_zeros_mean_no_file() {
        assert_eq!(
            blob_ids(b"3b18e51..8c2f4a7 100644"),
            Some(("3b18e51".into(), "8c2f4a7".into()))
        );
        assert_eq!(
            blob_ids(b"0000000..8c2f4a7"),
            Some((String::new(), "8c2f4a7".into()))
        );
    }

    #[test]
    fn without_a_repo_a_full_diff_is_the_hunks_only() {
        let patch = Patch::parse(SAMPLE.to_vec()).expect("the sample parses");
        assert!(patch.full_diff(4, None).known.is_some());
    }
}

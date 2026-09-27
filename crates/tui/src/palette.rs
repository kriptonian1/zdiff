//! The palette popup's state: go to line, or go to file with fuzzy matching.

use std::cmp::Reverse;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use zdiff_core::Side;
use zdiff_search::{Field, Filter, Finder, Query};

use crate::find::wrap;
use crate::search::Found;
use crate::tree::Tree;

/// The open palette: what was typed and what it searches.
#[derive(Debug)]
pub struct Palette {
    pub input: String,
    pub mode: Mode,
}

#[derive(Debug)]
pub enum Mode {
    Line,
    File(Files),
    Search(Box<Search>),
}

/// Global search over every changed file; results stream in from the worker.
#[derive(Debug, Default)]
pub struct Search {
    /// Its `text` is the SEARCH field.
    pub query: Query,
    pub include: String,
    pub exclude: String,
    /// The field being edited; Tab cycles it.
    pub input: Input,
    /// Bumped per search so batches for an older query are dropped.
    pub generation: u64,
    pub groups: Vec<Group>,
    /// Flat list rows: `(group, None)` is a file row, `(group, Some(hit))` a hit.
    pub rows: Vec<(usize, Option<usize>)>,
    pub list: ListState,
    pub hits: usize,
    pub filtered_out: usize,
    pub done: bool,
    pub error: Option<&'static str>,
    /// Result rows, toggles, and fields (in [`Input::ALL`] order) from the last draw, for clicks.
    pub list_area: Rect,
    pub toggle_areas: [Rect; 3],
    pub field_areas: [Rect; 3],
}

/// Which global-search field has the cursor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Input {
    #[default]
    Query,
    Include,
    Exclude,
}

impl Input {
    pub const ALL: [Self; 3] = [Self::Query, Self::Include, Self::Exclude];
}

/// The hits in one file, under its tree node.
#[derive(Debug)]
pub struct Group {
    pub node: usize,
    pub hits: Vec<Found>,
}

/// Changed files ranked against the input; lives only while the palette is open.
#[derive(Debug)]
pub struct Files {
    matcher: Matcher,
    /// `Utf32Str` scratch, reused across keystrokes.
    buf: Vec<char>,
    pub hits: Vec<Hit>,
    pub list: ListState,
    /// List rows from the last draw, for mouse clicks.
    pub area: Rect,
}

/// A matching file and the char positions in its path that matched, ascending.
#[derive(Debug)]
pub struct Hit {
    pub node: usize,
    pub chars: Vec<u32>,
}

impl Palette {
    pub fn line() -> Self {
        Self {
            input: String::new(),
            mode: Mode::Line,
        }
    }

    pub fn files(tree: &Tree) -> Self {
        let mut files = Files {
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
            buf: Vec::new(),
            hits: Vec::new(),
            list: ListState::default(),
            area: Rect::default(),
        };
        files.update("", tree);
        Self {
            input: String::new(),
            mode: Mode::File(files),
        }
    }

    /// Global search, seeded with the local query and a folder to search in.
    pub fn search(query: Query, include: String) -> Self {
        Self {
            input: String::new(),
            mode: Mode::Search(Box::new(Search {
                query,
                include,
                ..Search::default()
            })),
        }
    }
}

impl Search {
    pub fn field_mut(&mut self) -> &mut String {
        match self.input {
            Input::Query => &mut self.query.text,
            Input::Include => &mut self.include,
            Input::Exclude => &mut self.exclude,
        }
    }

    /// Moves the cursor to the next field, or the previous one, wrapping.
    pub fn cycle(&mut self, forward: bool) {
        let i = Input::ALL
            .iter()
            .position(|&x| x == self.input)
            .unwrap_or(0);
        self.input = Input::ALL[wrap(i, Input::ALL.len(), forward)];
    }

    /// Clears the results and returns the next job, `(generation, finder, change indices)`.
    ///
    /// `None` for an empty query, or a bad regex or glob, which sets [`Search::error`].
    pub fn job(&mut self, tree: &Tree) -> Option<(u64, Finder, Vec<usize>)> {
        self.generation += 1;
        self.groups.clear();
        self.rows.clear();
        self.list.select(None);
        (self.hits, self.filtered_out, self.done) = (0, 0, true);
        self.error = None;
        let finder = self.query.compile().map_err(|_| "bad regex");
        let filter = Filter::new(&self.include, &self.exclude).map_err(|(field, _)| match field {
            Field::Include => "bad include glob",
            Field::Exclude => "bad exclude glob",
        });
        let (finder, filter) = match (finder, filter) {
            (Ok(finder), Ok(filter)) => (finder, filter),
            (Err(e), _) | (_, Err(e)) => {
                self.error = Some(e);
                return None;
            }
        };
        if self.query.text.is_empty() {
            return None;
        }
        let (kept, dropped): (Vec<_>, Vec<_>) = tree
            .files()
            .partition(|(_, entry)| filter.allows(&entry.path));
        self.filtered_out = dropped.len();
        self.done = false;
        Some((
            self.generation,
            finder,
            kept.into_iter().map(|(_, e)| e.change).collect(),
        ))
    }

    /// Adds one file's hits; the first hit ever added gets selected.
    pub fn push(&mut self, node: usize, hits: Vec<Found>) {
        let group = self.groups.len();
        self.rows.push((group, None));
        self.rows
            .extend((0..hits.len()).map(|hit| (group, Some(hit))));
        self.hits += hits.len();
        self.groups.push(Group { node, hits });
        if self.list.selected().is_none() {
            self.list
                .select(self.rows.iter().position(|(_, hit)| hit.is_some()));
        }
    }

    /// Moves to the next or previous hit row, skipping file rows and wrapping.
    pub fn step(&mut self, down: bool) -> bool {
        let (Some(start), len) = (self.list.selected(), self.rows.len()) else {
            return false;
        };
        let mut row = start;
        loop {
            row = wrap(row, len, down);
            if self.rows[row].1.is_some() {
                break;
            }
        }
        self.list.select(Some(row));
        row != start
    }

    /// Where the hit at list `row` opens: tree node, side, and line; `None` for file rows.
    pub fn hit(&self, row: usize) -> Option<(usize, Side, u32)> {
        let (group, hit) = *self.rows.get(row)?;
        let group = &self.groups[group];
        let hit = group.hits.get(hit?)?;
        Some((group.node, hit.side, hit.line))
    }

    pub fn selected(&self) -> Option<(usize, Side, u32)> {
        self.hit(self.list.selected()?)
    }
}

impl Files {
    /// Re-ranks every changed file against `input`, best first; ties keep sidebar order.
    // ponytail: matches on the UI thread; use a worker once repos reach 10k+ changed files.
    pub fn update(&mut self, input: &str, tree: &Tree) {
        let pattern = Pattern::parse(input, CaseMatching::Smart, Normalization::Smart);
        let mut scored = Vec::new();
        for (node, entry) in tree.files() {
            let path = entry.path.to_string_lossy();
            let mut chars = Vec::new();
            let haystack = Utf32Str::new(&path, &mut self.buf);
            if let Some(score) = pattern.indices(haystack, &mut self.matcher, &mut chars) {
                chars.sort_unstable();
                chars.dedup();
                scored.push((score, Hit { node, chars }));
            }
        }
        scored.sort_by_key(|(score, _)| Reverse(*score));
        self.hits = scored.into_iter().map(|(_, hit)| hit).collect();
        self.list.select((!self.hits.is_empty()).then_some(0));
    }

    /// Tree node of the selected hit.
    pub fn selected(&self) -> Option<usize> {
        self.hits.get(self.list.selected()?).map(|hit| hit.node)
    }

    /// Moves the selection by one, wrapping at either end.
    pub fn step(&mut self, down: bool) -> bool {
        let (Some(i), len) = (self.list.selected(), self.hits.len()) else {
            return false;
        };
        self.list.select(Some(wrap(i, len, down)));
        true
    }
}

/// Go-to-line input: `42` or `+42` is new-file line 42, `-42` is old-file line 42.
pub fn parse(input: &str) -> Option<(Side, u32)> {
    let (side, number) = match input.strip_prefix('-') {
        Some(rest) => (Side::Old, rest),
        None => (Side::New, input.strip_prefix('+').unwrap_or(input)),
    };
    Some((side, number.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use zdiff_core::Status;

    use super::*;
    use crate::app::FileEntry;

    fn tree(paths: &[&str]) -> Tree {
        let files = paths.iter().map(|&path| FileEntry {
            path: path.into(),
            status: Status::Modified,
            added: 1,
            removed: 0,
            change: 0,
        });
        Tree::new(files.collect(), HashSet::new())
    }

    fn ranked(input: &str, tree: &Tree) -> Vec<String> {
        let Mode::File(mut files) = Palette::files(tree).mode else {
            unreachable!()
        };
        files.update(input, tree);
        files
            .hits
            .iter()
            .map(|hit| tree.file(hit.node).unwrap().path.display().to_string())
            .collect()
    }

    #[test]
    fn parse_reads_side_and_number() {
        assert_eq!(parse("42"), Some((Side::New, 42)));
        assert_eq!(parse("+3"), Some((Side::New, 3)));
        assert_eq!(parse("-7"), Some((Side::Old, 7)));
        for bad in ["", "-", "+", "4-2", "99999999999"] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn empty_input_lists_every_file_in_sidebar_order() {
        let t = tree(&["b/x.rs", "top.rs", "a/y.rs"]);
        assert_eq!(ranked("", &t), ["a/y.rs", "b/x.rs", "top.rs"]);
    }

    #[test]
    fn fuzzy_input_filters_and_ranks() {
        let t = tree(&["src/app.rs", "src/rows.rs", "src/r/o/w/s.txt", "README.md"]);
        let hits = ranked("rows", &t);
        assert_eq!(hits.first().map(String::as_str), Some("src/rows.rs"));
        assert!(!hits.contains(&"src/app.rs".to_owned()));
        assert_eq!(ranked(".rs$", &t), ["src/app.rs", "src/rows.rs"]);
        // `!` excludes exact substrings, so scattered r..s letters stay.
        assert_eq!(ranked("!rs", &t), ["src/r/o/w/s.txt", "README.md"]);
        assert!(ranked("zzz", &t).is_empty());
    }

    #[test]
    fn hits_carry_sorted_match_positions_and_selection_wraps() {
        let t = tree(&["a/rows.rs", "b/rows.rs"]);
        let Mode::File(mut files) = Palette::files(&t).mode else {
            unreachable!()
        };
        files.update("rows", &t);
        assert_eq!(
            files.hits[0].chars,
            [2, 3, 4, 5],
            "\"rows\" in \"a/rows.rs\""
        );
        assert!(files.step(false));
        assert_eq!(files.list.selected(), Some(1), "up from the top wraps");
        assert!(files.step(true));
        assert_eq!(files.list.selected(), Some(0));
    }

    fn found(line: u32) -> Found {
        Found {
            side: Side::New,
            line,
            changed: true,
            snippet: b"x".to_vec().into(),
            range: 0..1,
            tokens: Box::default(),
        }
    }

    #[test]
    fn search_job_filters_paths_and_reports_errors() {
        let t = tree(&["crates/tui/a.rs", "crates/core/b.rs", "doc.md"]);
        let mut search = Search {
            query: Query {
                text: "x".into(),
                ..Query::default()
            },
            include: "crates".into(),
            exclude: "crates/core".into(),
            ..Search::default()
        };
        let (generation, _, changes) = search.job(&t).expect("a job");
        assert_eq!((generation, changes.len(), search.filtered_out), (1, 1, 2));
        assert!(!search.done);

        search.include = "[".into();
        assert!(search.job(&t).is_none());
        assert_eq!(search.error, Some("bad include glob"));
        search.include.clear();
        search.query.regex = true;
        search.query.text = "(".into();
        assert!(search.job(&t).is_none());
        assert_eq!(search.error, Some("bad regex"));
        search.query.text.clear();
        assert!(search.job(&t).is_none(), "nothing to search");
        assert_eq!(
            (search.error, search.done, search.generation),
            (None, true, 4)
        );
    }

    #[test]
    fn results_group_by_file_and_steps_skip_file_rows() {
        let mut search = Search::default();
        search.push(0, vec![found(1), found(2)]);
        search.push(3, vec![found(5)]);
        assert_eq!(
            search.rows,
            [
                (0, None),
                (0, Some(0)),
                (0, Some(1)),
                (1, None),
                (1, Some(0))
            ]
        );
        assert_eq!(search.list.selected(), Some(1), "first hit selected");
        search.step(true);
        search.step(true);
        assert_eq!(search.list.selected(), Some(4), "file row skipped");
        search.step(true);
        assert_eq!(search.list.selected(), Some(1), "wraps");
        search.step(false);
        assert_eq!(search.selected(), Some((3, Side::New, 5)));
        assert_eq!(search.hits, 3);
        search.cycle(true);
        assert_eq!(search.input, Input::Include);
        search.cycle(false);
        search.cycle(false);
        assert_eq!(search.input, Input::Exclude);
    }
}

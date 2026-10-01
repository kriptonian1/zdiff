//! The palette popup's state: go to line, or go to file with fuzzy matching.

use std::cmp::Reverse;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use zdiff_core::Side;
use zdiff_search::{Field as Glob, Filter, Finder, Query};

use crate::find::wrap;
use crate::input::Field;
use crate::keymap::{Chord, Keymap};
use crate::menu::Action;
use crate::search::Found;
use crate::tree::Tree;
use crate::ui::Theme;

/// The open palette: what was typed and what it searches.
#[derive(Debug)]
pub struct Palette {
    /// What was typed: a line number, or a filter.
    pub field: Field,
    pub mode: Mode,
}

#[derive(Debug)]
pub enum Mode {
    Line,
    File(Files),
    Search(Box<Search>),
    /// The read-only list of keyboard shortcuts.
    Keys(Keys),
    Themes(Themes),
}

/// The theme picker: the built-in themes whose name matches the typed filter.
#[derive(Debug, Default)]
pub struct Themes {
    pub shown: Vec<Theme>,
    pub list: ListState,
    /// List rows from the last draw, for mouse clicks.
    pub area: Rect,
}

impl Themes {
    pub fn selected(&self) -> Option<Theme> {
        self.shown.get(self.list.selected()?).copied()
    }

    /// Keeps the themes whose name contains `input`, ignoring case.
    pub fn filter(&mut self, input: &str) {
        let input = input.to_lowercase();
        self.shown = (Theme::ALL.into_iter())
            .filter(|theme| theme.label().to_lowercase().contains(&input))
            .collect();
        self.list.select((!self.shown.is_empty()).then_some(0));
    }

    /// Moves the selection by one, wrapping at either end.
    pub fn step(&mut self, down: bool) -> bool {
        let (Some(i), len) = (self.list.selected(), self.shown.len()) else {
            return false;
        };
        self.list.select(Some(wrap(i, len, down)));
        true
    }
}

/// One row of the shortcuts screen.
#[derive(Debug)]
pub struct KeyRow {
    pub label: &'static str,
    /// Every key for the action, like `j, ↓`; `—` for menu-only actions.
    pub keys: String,
    /// Where the keys work: `everywhere`, `sidebar`, `diff`, `menu`, or a popup.
    pub place: &'static str,
    /// The action to rebind; `None` for fixed keys inside text inputs and popups.
    pub action: Option<Action>,
    /// Whether the keys differ from the built-in ones.
    pub edited: bool,
}

/// A rebinding in progress on the shortcuts screen.
#[derive(Debug, Clone, Copy)]
pub struct Capture {
    pub action: Action,
    /// Add the key to the action's keys instead of replacing them.
    pub add: bool,
    /// A key already used by another action, waiting for Enter to take it over.
    pub taken: Option<(Chord, Action)>,
}

/// Fixed keys shown on the shortcuts screen: they belong to text inputs and popups.
const LOCKED: [(&str, &str, &str); 8] = [
    ("Next match", "Enter", "find bar"),
    ("Previous match", "Shift+Enter", "find bar"),
    (
        "Match case / regex / changed lines",
        "Alt+C, Alt+R, Alt+D",
        "find bar",
    ),
    ("Next field", "Tab, Shift+Tab", "search"),
    ("Open selected", "Enter", "popups"),
    ("Close", "Esc", "popups"),
    ("Menu: move, open, close", "↑↓←→, Enter, Esc", "menu"),
    ("Quit, always", "Ctrl+C", "everywhere"),
];

/// The shortcuts screen: every row, and the ones matching the typed filter.
#[derive(Debug)]
pub struct Keys {
    pub rows: Vec<KeyRow>,
    /// Indices into `rows` that match the input, in order.
    pub shown: Vec<usize>,
    pub list: ListState,
    /// List rows from the last draw, for mouse clicks.
    pub area: Rect,
    pub capture: Option<Capture>,
}

impl Keys {
    /// One row per bound or menu action, keymap order first, then the locked keys.
    fn new(keymap: &Keymap) -> Self {
        let bound = Action::known().into_iter().map(|action| {
            let mut keys = keymap.keys(action).peekable();
            let place = keys.peek().map_or("menu", |&(context, _)| context.place());
            let keys: Vec<String> = keys.map(|(_, chord)| chord.to_string()).collect();
            KeyRow {
                label: action.label(),
                keys: if keys.is_empty() {
                    "—".into()
                } else {
                    keys.join(", ")
                },
                place,
                action: Some(action),
                edited: keymap.is_edited(action),
            }
        });
        let locked = LOCKED.iter().map(|&(label, keys, place)| KeyRow {
            label,
            keys: keys.into(),
            place,
            action: None,
            edited: false,
        });
        let rows: Vec<KeyRow> = bound.chain(locked).collect();
        let mut keys = Self {
            shown: (0..rows.len()).collect(),
            rows,
            list: ListState::default(),
            area: Rect::default(),
            capture: None,
        };
        keys.list.select(Some(0));
        keys
    }

    /// Rows again after a key change, keeping the filter and the selected row.
    pub fn rebuild(&mut self, keymap: &Keymap, input: &str) {
        let selected = self.list.selected();
        *self = Self::new(keymap);
        self.filter(input);
        if selected.is_some_and(|i| i < self.shown.len()) {
            self.list.select(selected);
        }
    }

    /// The action on the selected row, if it can be rebound.
    pub fn selected_action(&self) -> Option<Action> {
        let row = *self.shown.get(self.list.selected()?)?;
        self.rows[row].action
    }

    /// Keeps the rows whose label, keys, or place contains `input`, ignoring case.
    pub fn filter(&mut self, input: &str) {
        let input = input.to_lowercase();
        let matches = |row: &KeyRow| {
            [row.label, row.keys.as_str(), row.place]
                .iter()
                .any(|text| text.to_lowercase().contains(&input))
        };
        self.shown = (0..self.rows.len())
            .filter(|&i| matches(&self.rows[i]))
            .collect();
        self.list.select((!self.shown.is_empty()).then_some(0));
    }

    /// Moves the selection by one, wrapping at either end.
    pub fn step(&mut self, down: bool) -> bool {
        let (Some(i), len) = (self.list.selected(), self.shown.len()) else {
            return false;
        };
        self.list.select(Some(wrap(i, len, down)));
        true
    }
}

/// Global search over every changed file; results stream in from the worker.
#[derive(Debug, Default)]
pub struct Search {
    /// The search options; its `text` mirrors the SEARCH field, see [`Search::sync`].
    pub query: Query,
    /// SEARCH, include, and exclude, in [`Input::ALL`] order.
    pub fields: [Field; 3],
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
            field: Field::default(),
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
            field: Field::default(),
            mode: Mode::File(files),
        }
    }

    pub fn keys(keymap: &Keymap) -> Self {
        Self {
            field: Field::default(),
            mode: Mode::Keys(Keys::new(keymap)),
        }
    }

    /// The theme picker, starting on `current`.
    pub fn themes(current: Theme) -> Self {
        let mut themes = Themes {
            shown: Theme::ALL.to_vec(),
            ..Themes::default()
        };
        themes
            .list
            .select(Theme::ALL.iter().position(|&t| t == current));
        Self {
            field: Field::default(),
            mode: Mode::Themes(themes),
        }
    }

    /// Global search, seeded with the local query and a folder to search in.
    pub fn search(query: Query, include: &str) -> Self {
        Self {
            field: Field::default(),
            mode: Mode::Search(Box::new(Search {
                fields: [
                    Field::single(&query.text),
                    Field::single(include),
                    Field::default(),
                ],
                query,
                ..Search::default()
            })),
        }
    }
}

impl Search {
    pub fn field_mut(&mut self) -> &mut Field {
        let i = Input::ALL
            .iter()
            .position(|&x| x == self.input)
            .unwrap_or(0);
        &mut self.fields[i]
    }

    /// Copies the SEARCH field into `query.text`, which the search crate reads.
    pub fn sync(&mut self) {
        self.query.text = self.fields[0].text();
    }

    /// Replaces a field's text.
    #[cfg(test)]
    pub fn set(&mut self, input: Input, text: &str) {
        let i = Input::ALL.iter().position(|&x| x == input).unwrap_or(0);
        self.fields[i] = Field::single(text);
        self.sync();
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
        let (include, exclude) = (self.fields[1].text(), self.fields[2].text());
        let filter = Filter::new(&include, &exclude).map_err(|(field, _)| match field {
            Glob::Include => "bad include glob",
            Glob::Exclude => "bad exclude glob",
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
            staged: zdiff_core::Staged::No,
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
        let mut search = Search::default();
        search.set(Input::Query, "x");
        search.set(Input::Include, "crates");
        search.set(Input::Exclude, "crates/core");
        let (generation, _, changes) = search.job(&t).expect("a job");
        assert_eq!((generation, changes.len(), search.filtered_out), (1, 1, 2));
        assert!(!search.done);

        search.set(Input::Include, "[");
        assert!(search.job(&t).is_none());
        assert_eq!(search.error, Some("bad include glob"));
        search.set(Input::Include, "");
        search.query.regex = true;
        search.set(Input::Query, "(");
        assert!(search.job(&t).is_none());
        assert_eq!(search.error, Some("bad regex"));
        search.set(Input::Query, "");
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

//! The find bar's state: a query run over the shown diff, and which hit is current.

use std::ops::Range;

use ratatui::layout::{Position, Rect};
use zdiff_core::{Side, Text, locate};
use zdiff_search::{Hit, MAX_HITS, Query};

use crate::app::DiffView;

/// The open find bar: query, hits in the shown diff, and which one is current.
#[derive(Debug, Default)]
pub struct Find {
    pub query: Query,
    /// Sorted in screen order, so next and previous follow the rows.
    pub hits: Vec<Hit>,
    pub current: Option<usize>,
    /// The query is an invalid regex.
    pub error: bool,
    /// Hit ranges as file byte offsets, `[old, new]`, each sorted; for highlighting.
    pub found: [Vec<Range<u32>>; 2],
    /// The bar and its toggles (in [`Toggle::ALL`] order) from the last draw, for clicks.
    pub area: Rect,
    pub toggle_areas: [Rect; 3],
}

/// A search option, shown as a clickable toggle in the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    Case,
    Regex,
    ChangedOnly,
}

impl Toggle {
    pub const ALL: [Self; 3] = [Self::Case, Self::Regex, Self::ChangedOnly];

    pub fn label(self) -> &'static str {
        match self {
            Self::Case => "[Aa]",
            Self::Regex => "[.*]",
            Self::ChangedOnly => "[±]",
        }
    }

    /// The toggle an Alt+`key` shortcut flips, as in VS Code.
    pub fn for_alt(key: char) -> Option<Self> {
        match key {
            'c' => Some(Self::Case),
            'r' => Some(Self::Regex),
            'd' => Some(Self::ChangedOnly),
            _ => None,
        }
    }

    pub fn is_on(self, query: &Query) -> bool {
        match self {
            Self::Case => query.case,
            Self::Regex => query.regex,
            Self::ChangedOnly => query.changed_only,
        }
    }

    /// Flips this option in `query`; re-run the search afterwards.
    pub fn flip(self, query: &mut Query) {
        let flag = match self {
            Self::Case => &mut query.case,
            Self::Regex => &mut query.regex,
            Self::ChangedOnly => &mut query.changed_only,
        };
        *flag = !*flag;
    }
}

/// The item drawn at `position`, given `items` and the `areas` they were drawn in.
pub fn at<T: Copy>(items: [T; 3], areas: &[Rect; 3], position: Position) -> Option<T> {
    (items.into_iter().zip(areas)).find_map(|(item, area)| area.contains(position).then_some(item))
}

/// The index one step from `i` in `0..len`, wrapping at either end.
pub fn wrap(i: usize, len: usize, forward: bool) -> usize {
    if forward {
        (i + 1) % len
    } else {
        (i + len - 1) % len
    }
}

/// The file byte range of `hit` in `text`.
fn byte_range(hit: &Hit, text: &Text) -> Range<u32> {
    let start = text.line_start(hit.line);
    start + hit.range.start..start + hit.range.end
}

impl Find {
    /// Re-runs the query on `view`; the current hit becomes the first one at or below the scroll.
    pub fn update(&mut self, view: &DiffView) {
        let finder = self.query.compile();
        self.error = finder.is_err();
        self.hits = finder.map(|f| f.find(&view.file)).unwrap_or_default();
        self.hits
            .sort_by_cached_key(|h| locate(&view.rows, h.side, h.line));
        for (side, found) in [Side::Old, Side::New].into_iter().zip(&mut self.found) {
            let text = side.pick(&view.file.old, &view.file.new);
            *found = (self.hits.iter().filter(|h| h.side == side))
                .map(|h| byte_range(h, text))
                .collect();
            found.sort_unstable_by_key(|r| r.start);
        }
        let below =
            |h: &Hit| locate(&view.rows, h.side, h.line).is_some_and(|(row, _)| row >= view.scroll);
        self.current = self
            .hits
            .iter()
            .position(below)
            .or((!self.hits.is_empty()).then_some(0));
    }

    /// Moves `current` by one, wrapping; returns the hit to jump to.
    pub fn step(&mut self, forward: bool) -> Option<&Hit> {
        let next = wrap(self.current?, self.hits.len(), forward);
        self.current = Some(next);
        self.hits.get(next)
    }

    /// The current hit's side and file byte range, for the stronger highlight.
    pub fn current_range(&self, view: &DiffView) -> Option<(Side, Range<u32>)> {
        let hit = self.hits.get(self.current?)?;
        let text = hit.side.pick(&view.file.old, &view.file.new);
        Some((hit.side, byte_range(hit, text)))
    }

    /// `3/17`, `10000+`, `0/0`, or `bad regex`; `true` when it should read as an error.
    pub fn counter(&self) -> (String, bool) {
        if self.error {
            return ("bad regex".into(), true);
        }
        let total = if self.hits.len() == MAX_HITS {
            format!("{MAX_HITS}+")
        } else {
            self.hits.len().to_string()
        };
        let at = self.current.map_or(0, |i| i + 1);
        let empty = self.hits.is_empty() && !self.query.text.is_empty();
        (format!("{at}/{total}"), empty)
    }
}

#[cfg(test)]
mod tests {
    use zdiff_core::{FileDiff, Status};

    use super::*;
    use crate::app::{App, DiffPane, FileEntry};

    fn find_in(old: &str, new: &str, text: &str) -> (App, Find) {
        let mut app = App::new(vec![FileEntry {
            path: "a.rs".into(),
            status: Status::Modified,
            added: 1,
            removed: 1,
            change: 0,
        }]);
        app.show(Some(Ok(FileDiff::new(old.into(), new.into()))));
        let mut find = Find::default();
        find.query.text = text.into();
        if let DiffPane::Loaded(view) = &app.diff {
            find.update(view);
        }
        (app, find)
    }

    #[test]
    fn hits_follow_screen_order_and_found_holds_file_offsets() {
        let (_, find) = find_in("a x\nold x\nz x\n", "a x\nnew x\nz x\n", "x");
        let order: Vec<_> = find.hits.iter().map(|h| (h.side, h.line)).collect();
        assert_eq!(
            order,
            [
                (Side::Old, 0),
                (Side::New, 0),
                (Side::Old, 1),
                (Side::New, 1),
                (Side::Old, 2),
                (Side::New, 2),
            ]
        );
        assert_eq!(find.current, Some(0));
        assert_eq!(
            find.found[0],
            [2..3, 8..9, 12..13],
            "old side, file offsets"
        );
    }

    #[test]
    fn step_wraps_and_counter_reports() {
        let (app, mut find) = find_in("x\ny\nx\n", "x\nY\nx\n", "x");
        assert_eq!(find.counter(), ("1/4".into(), false));
        assert_eq!(
            find.step(false).map(|h| h.line),
            Some(2),
            "back from the first wraps"
        );
        assert_eq!(find.counter(), ("4/4".into(), false));
        find.step(true);
        assert_eq!(find.current, Some(0));
        let DiffPane::Loaded(view) = &app.diff else {
            unreachable!()
        };
        assert_eq!(find.current_range(view), Some((Side::Old, 0..1)));

        find.query.text = "zzz".into();
        find.update(view);
        assert_eq!(find.counter(), ("0/0".into(), true));
        find.query = Query {
            text: "(".into(),
            regex: true,
            ..Query::default()
        };
        find.update(view);
        assert_eq!(find.counter(), ("bad regex".into(), true));
    }
}

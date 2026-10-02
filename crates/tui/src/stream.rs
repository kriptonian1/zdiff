//! The All files view: every changed file's diff stacked in one scroll.
//!
//! Only sections on or near the screen hold a loaded [`DiffView`], so memory follows the screen
//! height, not the number of files. The rest keep their height so scrolling never jumps.

use std::ops::Range;

use zdiff_core::Row;

use crate::app::{DiffView, FileEntry, Shape, View, clamp_add, is_header};
use crate::tree::Tree;

/// Changed lines above which a file starts collapsed, like GitHub's large-diff notice.
pub(crate) const LARGE_DIFF: u32 = 400;
/// Sections kept loaded on each side of the screen.
const PREFETCH: usize = 2;
/// Rows above each section's body: a separator line, then the file's header.
pub const HEADER_ROWS: usize = 2;
/// Rows an unloaded file is guessed to add beyond its changed lines: block headers and context.
const ESTIMATE_EXTRA: usize = 8;

/// What a section shows under its header.
#[derive(Debug)]
pub enum Body {
    /// Not loaded yet, or dropped after scrolling far away.
    Unloaded,
    /// A large diff waiting for Enter or a click.
    Collapsed,
    /// Boxed so unloaded sections, the common case, stay small.
    Loaded(Box<DiffView>),
    Failed(Box<str>),
}

/// One changed file: [`HEADER_ROWS`] rows, then `height` body rows.
#[derive(Debug)]
pub struct Section {
    pub node: usize,
    pub change: usize,
    /// `(added, removed)`, for estimating an unloaded section's height.
    lines: (u32, u32),
    /// Body rows: exact once loaded, estimated before that.
    pub height: usize,
    pub body: Body,
}

/// Every changed file, stacked in sidebar order.
#[derive(Debug)]
pub struct Stream {
    pub sections: Vec<Section>,
    /// First global row of each section: prefix sums of `HEADER_ROWS + height`.
    starts: Vec<usize>,
    /// Global row at the top of the pane.
    pub scroll: usize,
    /// Columns of line text scrolled off the left, shared by every section.
    pub hscroll: usize,
    /// What loaded sections' rows are built for.
    pub shape: Shape,
}

/// Rows an unloaded file with these counts is guessed to take in `view`.
fn estimate((added, removed): (u32, u32), view: View) -> usize {
    let changed = match view {
        View::Split => added.max(removed),
        View::Unified => added + removed,
    };
    changed as usize + ESTIMATE_EXTRA
}

impl Section {
    fn new(node: usize, entry: &FileEntry, view: View) -> Self {
        let lines = (entry.added, entry.removed);
        let large = entry.added + entry.removed > LARGE_DIFF;
        let (height, body) = if large {
            (1, Body::Collapsed)
        } else {
            (estimate(lines, view), Body::Unloaded)
        };
        Self {
            node,
            change: entry.change,
            lines,
            height,
            body,
        }
    }

    /// The body height once `body` is set: rows when loaded, else one message row.
    fn fitted_height(&self) -> usize {
        match &self.body {
            Body::Loaded(view) => view.rows.len().max(1),
            Body::Unloaded => self.height,
            Body::Collapsed | Body::Failed(_) => 1,
        }
    }
}

impl Stream {
    pub fn new(tree: &Tree, shape: Shape) -> Self {
        let sections = (tree.files())
            .map(|(node, entry)| Section::new(node, entry, shape.view))
            .collect();
        let mut stream = Self {
            sections,
            starts: Vec::new(),
            scroll: 0,
            hscroll: 0,
            shape,
        };
        stream.restart();
        stream
    }

    /// Recomputes `starts` after a height changed.
    fn restart(&mut self) {
        self.starts.clear();
        let mut row = 0;
        for section in &self.sections {
            self.starts.push(row);
            row += HEADER_ROWS + section.height;
        }
    }

    /// Rows in the whole stream.
    pub fn total(&self) -> usize {
        (self.starts.last())
            .zip(self.sections.last())
            .map_or(0, |(start, last)| start + HEADER_ROWS + last.height)
    }

    pub fn max_scroll(&self, height: usize) -> usize {
        self.total().saturating_sub(height)
    }

    /// The section showing global `row`, and the row within it; below [`HEADER_ROWS`] is its header.
    pub fn at(&self, row: usize) -> Option<(usize, usize)> {
        let i = self
            .starts
            .partition_point(|&start| start <= row)
            .checked_sub(1)?;
        let local = row - self.starts[i];
        (local < HEADER_ROWS + self.sections[i].height).then_some((i, local))
    }

    /// First global row of `section`.
    #[cfg(test)]
    pub fn start(&self, section: usize) -> usize {
        self.starts[section]
    }

    /// Runs `change`, then keeps the row at the top of the pane in place even if heights above
    /// it changed.
    fn keep_top(&mut self, change: impl FnOnce(&mut Self)) {
        let top = self.at(self.scroll);
        change(self);
        self.restart();
        if let Some((i, local)) = top {
            let last = HEADER_ROWS + self.sections[i].height - 1;
            self.scroll = self.starts[i] + local.min(last);
        }
    }

    pub fn scroll_to(&mut self, row: usize, height: usize) -> bool {
        let scroll = row.min(self.max_scroll(height));
        std::mem::replace(&mut self.scroll, scroll) != scroll
    }

    pub fn scroll_by(&mut self, delta: isize, height: usize) -> bool {
        self.scroll_to(clamp_add(self.scroll, delta, usize::MAX), height)
    }

    /// Sections on screen in a `height`-row pane, plus [`PREFETCH`] on each side.
    fn window(&self, height: usize) -> Range<usize> {
        let first = self.at(self.scroll).map_or(0, |(i, _)| i);
        let bottom = (self.scroll + height).min(self.total()).saturating_sub(1);
        let last = self.at(bottom).map_or(first, |(i, _)| i);
        first.saturating_sub(PREFETCH)..(last + 1 + PREFETCH).min(self.sections.len())
    }

    /// `(section, change)` for every unloaded section near the screen; drops loaded views
    /// outside that window, keeping their height so nothing moves.
    pub fn wanted(&mut self, height: usize) -> Vec<(usize, usize)> {
        let window = self.window(height);
        let mut wanted = Vec::new();
        for (i, section) in self.sections.iter_mut().enumerate() {
            match section.body {
                Body::Unloaded if window.contains(&i) => wanted.push((i, section.change)),
                Body::Loaded(_) if !window.contains(&i) => section.body = Body::Unloaded,
                _ => {}
            }
        }
        wanted
    }

    /// Stores a section's loaded body and its exact height.
    pub fn loaded(&mut self, section: usize, body: Body) {
        self.keep_top(|stream| {
            let section = &mut stream.sections[section];
            section.body = body;
            section.height = section.fitted_height();
        });
    }

    /// Rebuilds every loaded section for `shape` and re-estimates the rest.
    pub fn reshape(&mut self, shape: Shape) {
        self.shape = shape;
        self.keep_top(|stream| {
            for section in &mut stream.sections {
                if let Body::Loaded(view) = &mut section.body {
                    view.set_shape(shape);
                } else if matches!(section.body, Body::Unloaded) {
                    section.height = estimate(section.lines, shape.view);
                }
                section.height = section.fitted_height();
            }
        });
    }

    /// Section starts and change-block headers of `section`, as global rows.
    fn stops(&self, section: usize) -> impl Iterator<Item = usize> + '_ {
        let start = self.starts[section];
        let rows = match &self.sections[section].body {
            Body::Loaded(view) => view.rows.as_slice(),
            _ => &[],
        };
        let headers = (rows.iter().enumerate())
            .filter(|(_, row)| is_header(row))
            .map(move |(k, _)| start + HEADER_ROWS + k);
        std::iter::once(start).chain(headers)
    }

    /// Scrolls to the next change block or file below the top of the pane.
    pub fn next_header(&mut self, height: usize) -> bool {
        let top = self.at(self.scroll).map_or(0, |(i, _)| i);
        let next = (top..self.sections.len())
            .flat_map(|i| self.stops(i))
            .find(|&row| row > self.scroll);
        next.is_some_and(|row| self.scroll_to(row, height))
    }

    /// Scrolls to the previous change block or file above the top of the pane.
    pub fn prev_header(&mut self, height: usize) -> bool {
        let top = self.at(self.scroll).map_or(0, |(i, _)| i);
        let prev = (0..=top.min(self.sections.len().saturating_sub(1)))
            .flat_map(|i| self.stops(i))
            .filter(|&row| row < self.scroll)
            .last();
        prev.is_some_and(|row| self.scroll_to(row, height))
    }

    /// Opens what global `row` shows: a collapsed large diff, or an unchanged-lines fold.
    pub fn expand_at(&mut self, row: usize) -> bool {
        let Some((i, local)) = self.at(row).filter(|&(_, local)| local >= HEADER_ROWS) else {
            return false;
        };
        let expandable = match &self.sections[i].body {
            Body::Collapsed => true,
            Body::Loaded(view) => {
                matches!(view.rows.get(local - HEADER_ROWS), Some(Row::Fold { .. }))
            }
            Body::Unloaded | Body::Failed(_) => false,
        };
        if !expandable {
            return false;
        }
        let view = self.shape.view;
        self.keep_top(|stream| {
            let section = &mut stream.sections[i];
            match &mut section.body {
                Body::Collapsed => {
                    section.body = Body::Unloaded;
                    section.height = estimate(section.lines, view);
                }
                Body::Loaded(diff) => {
                    zdiff_core::expand(&mut diff.rows, local - HEADER_ROWS);
                    section.height = section.fitted_height();
                }
                Body::Unloaded | Body::Failed(_) => {}
            }
        });
        true
    }

    /// Opens the first collapsed diff or fold on screen.
    pub fn expand(&mut self, height: usize) -> bool {
        (self.scroll..(self.scroll + height).min(self.total())).any(|row| self.expand_at(row))
    }

    /// Scrolls so `node`'s section starts at the top.
    pub fn jump(&mut self, node: usize, height: usize) -> bool {
        match self.sections.iter().position(|s| s.node == node) {
            Some(i) => self.scroll_to(self.starts[i], height),
            None => false,
        }
    }

    /// The file at the top of the pane.
    pub fn top_node(&self) -> Option<usize> {
        self.at(self.scroll).map(|(i, _)| self.sections[i].node)
    }

    /// Moves every section's text sideways, as far as the widest loaded line allows.
    pub fn hscroll_by(&mut self, delta: isize) -> bool {
        let max = (self.sections.iter())
            .filter_map(|s| match &s.body {
                Body::Loaded(view) => Some(view.max_hscroll(view.rows.len())),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let hscroll = clamp_add(self.hscroll, delta, max);
        std::mem::replace(&mut self.hscroll, hscroll) != hscroll
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use zdiff_core::{FileDiff, Status};

    use super::*;

    fn shape(view: View) -> Shape {
        Shape { view, context: 3 }
    }

    fn tree(files: &[(&str, u32, u32)]) -> Tree {
        let entries = (files.iter().enumerate())
            .map(|(change, &(path, added, removed))| FileEntry {
                path: path.into(),
                status: Status::Modified,
                added,
                removed,
                change,
                staged: zdiff_core::Staged::No,
            })
            .collect();
        Tree::new(entries, HashSet::new())
    }

    /// A loaded view for a 40-line file with one changed line in the middle.
    fn loaded(shape: Shape) -> Body {
        let old = (1..=40).fold(String::new(), |text, i| text + &format!("line {i}\n"));
        let new = old.replace("line 20\n", "twenty\n");
        let file = FileDiff::new(old.into_bytes(), new.into_bytes());
        Body::Loaded(Box::new(DiffView::new(file, None, shape)))
    }

    #[test]
    fn heights_are_estimated_until_loaded_and_large_diffs_collapse() {
        let t = tree(&[("a.rs", 5, 2), ("b.rs", 500, 0)]);
        let split = Stream::new(&t, shape(View::Split));
        assert_eq!(split.sections[0].height, 5 + ESTIMATE_EXTRA);
        assert!(matches!(split.sections[1].body, Body::Collapsed));
        assert_eq!(split.sections[1].height, 1);
        assert_eq!(split.total(), HEADER_ROWS + 13 + HEADER_ROWS + 1);
        let unified = Stream::new(&t, shape(View::Unified));
        assert_eq!(unified.sections[0].height, 7 + ESTIMATE_EXTRA);
    }

    #[test]
    fn at_maps_global_rows_to_sections() {
        let t = tree(&[("a.rs", 1, 0), ("b.rs", 1, 0)]);
        let stream = Stream::new(&t, shape(View::Split));
        let first = HEADER_ROWS + stream.sections[0].height;
        assert_eq!(stream.at(0), Some((0, 0)), "first separator");
        assert_eq!(stream.at(first - 1), Some((0, first - 1)));
        assert_eq!(stream.at(first), Some((1, 0)), "second separator");
        assert_eq!(stream.at(stream.total()), None);
    }

    #[test]
    fn loading_above_the_top_keeps_the_reading_position() {
        let t = tree(&[("a.rs", 1, 1), ("b.rs", 1, 1)]);
        let mut stream = Stream::new(&t, shape(View::Split));
        stream.scroll = stream.start(1) + 2;
        stream.loaded(0, loaded(stream.shape));
        assert_eq!(stream.at(stream.scroll), Some((1, 2)), "same row of b.rs");
    }

    #[test]
    fn wanted_loads_the_window_and_drops_far_sections() {
        let files: Vec<(String, u32, u32)> = (0..10).map(|i| (format!("{i}.rs"), 1, 0)).collect();
        let refs: Vec<(&str, u32, u32)> =
            files.iter().map(|(p, a, r)| (p.as_str(), *a, *r)).collect();
        let mut stream = Stream::new(&tree(&refs), shape(View::Split));
        let wanted = stream.wanted(5);
        assert_eq!(
            wanted.iter().map(|&(i, _)| i).collect::<Vec<_>>(),
            [0, 1, 2]
        );
        stream.loaded(0, loaded(stream.shape));
        stream.scroll = stream.start(8);
        stream.wanted(5);
        assert!(
            matches!(stream.sections[0].body, Body::Unloaded),
            "dropped when far"
        );
    }

    #[test]
    fn headers_cross_files_and_collapsed_diffs_expand() {
        let t = tree(&[("a.rs", 1, 1), ("big.rs", 900, 0)]);
        let mut stream = Stream::new(&t, shape(View::Split));
        stream.loaded(0, loaded(stream.shape));
        assert!(stream.next_header(1), "to a.rs's change block");
        assert_eq!(stream.at(stream.scroll).map(|(i, _)| i), Some(0));
        assert!(stream.next_header(1));
        assert_eq!(stream.at(stream.scroll), Some((1, 0)), "into big.rs");
        assert!(stream.prev_header(1));
        assert_eq!(stream.at(stream.scroll).map(|(i, _)| i), Some(0));

        let collapsed = stream.start(1) + HEADER_ROWS;
        assert!(stream.expand_at(collapsed), "the collapsed row");
        assert!(
            matches!(stream.sections[1].body, Body::Unloaded),
            "now loads"
        );
        assert!(stream.jump(t.files().nth(1).map(|(n, _)| n).unwrap(), 1));
        assert_eq!(stream.scroll, stream.start(1));
    }
}

//! The branches popup's state: branches and tags, the selected one's graph against HEAD, and
//! what to ask the event loop for.

use ratatui::layout::Rect;
use zdiff_core::{Branch, Commit, GraphRow, RefKind, layout};

use crate::history::{MAX_LANES, Want, contains};
use crate::input::Field;

/// Commits the graph shows for the selected branch.
pub const GRAPH: usize = 200;

/// One line of the list: a group's header, or the branch at this index of `list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    Header(RefKind),
    Branch(usize),
}

/// A question the hint line asks before a branch action.
#[derive(Debug)]
pub enum Ask {
    /// The name for a new branch at the selected one's tip.
    New { name: Box<Field> },
    /// Delete this local branch? `force` once git said it isn't merged.
    Delete { name: String, force: bool },
}

/// The open branches popup.
#[derive(Debug)]
pub struct Branches {
    pub list: Vec<Branch>,
    /// Indices of the branches the search and the remotes toggle keep.
    pub shown: Vec<usize>,
    pub selected: usize,
    pub scroll: usize,
    /// Whether remote branches are listed; `r` flips it.
    pub remotes: bool,
    /// The search box, while there is a search; `typing` while keys go into it.
    pub search: Option<Field>,
    pub typing: bool,
    /// The graph of the selected branch, once loaded, and the tip it's for or loading.
    pub graph: Vec<Commit>,
    pub rows: Vec<GraphRow>,
    pub graph_of: Option<String>,
    pub graph_scroll: usize,
    pub loaded: bool,
    /// The popup, its list, graph and search row from the last draw, for the mouse.
    pub area: Rect,
    pub list_area: Rect,
    pub graph_area: Rect,
    pub search_area: Rect,
    pub clear_area: Rect,
    /// The question the hint line asks, and its confirm and cancel buttons from the last draw.
    pub ask: Option<Ask>,
    pub ask_buttons: [Rect; 2],
}

impl Branches {
    /// A popup waiting for its list.
    pub fn new() -> (Self, Want) {
        let branches = Self {
            list: Vec::new(),
            shown: Vec::new(),
            selected: 0,
            scroll: 0,
            remotes: true,
            search: None,
            typing: false,
            graph: Vec::new(),
            rows: Vec::new(),
            graph_of: None,
            graph_scroll: 0,
            loaded: false,
            area: Rect::default(),
            list_area: Rect::default(),
            graph_area: Rect::default(),
            search_area: Rect::default(),
            clear_area: Rect::default(),
            ask: None,
            ask_buttons: [Rect::default(); 2],
        };
        (branches, Want::Branches)
    }

    /// Takes the list; selects HEAD's branch and asks for its graph.
    pub fn loaded(&mut self, list: Vec<Branch>) -> Vec<Want> {
        self.selected = list.iter().position(|b| b.head).unwrap_or(0);
        self.list = list;
        self.loaded = true;
        self.filter()
    }

    pub fn selected_branch(&self) -> Option<&Branch> {
        self.shown
            .contains(&self.selected)
            .then(|| self.list.get(self.selected))
            .flatten()
    }

    /// The search text; empty without a search.
    pub fn query(&self) -> String {
        self.search.as_ref().map(Field::text).unwrap_or_default()
    }

    /// Recomputes `shown` for the search and the remotes toggle, moving the selection onto a
    /// kept branch.
    pub fn filter(&mut self) -> Vec<Want> {
        let query = self.query();
        self.shown = (self.list.iter().enumerate())
            .filter(|(_, b)| self.remotes || b.kind != RefKind::Remote)
            .filter(|(_, b)| query.is_empty() || contains(&b.name, &query))
            .map(|(i, _)| i)
            .collect();
        if !self.shown.contains(&self.selected)
            && let Some(&first) = self.shown.first()
        {
            self.selected = first;
        }
        self.wants_graph().into_iter().collect()
    }

    /// Shows or hides the remote branches.
    pub fn toggle_remotes(&mut self) -> Vec<Want> {
        self.remotes = !self.remotes;
        self.filter()
    }

    /// The list as drawn: each group of shown branches under its header.
    pub fn lines(&self) -> Vec<Line> {
        let mut lines = Vec::with_capacity(self.shown.len() + 3);
        let mut group = None;
        for &i in &self.shown {
            let kind = self.list[i].kind;
            if group != Some(kind) {
                group = Some(kind);
                lines.push(Line::Header(kind));
            }
            lines.push(Line::Branch(i));
        }
        lines
    }

    /// Moves the selection by `delta` branches, stopping at either end; returns whether it
    /// moved and what to load for the new one.
    pub fn step(&mut self, delta: isize) -> (bool, Vec<Want>) {
        let Some(at) = self.shown.iter().position(|&i| i == self.selected) else {
            return (false, Vec::new());
        };
        let to = at
            .saturating_add_signed(delta)
            .min(self.shown.len().saturating_sub(1));
        if to == at {
            return (false, Vec::new());
        }
        self.selected = self.shown[to];
        self.moved();
        (true, self.wants_graph().into_iter().collect())
    }

    /// Selects the branch at `index` of `list`.
    pub fn select(&mut self, index: usize) -> Vec<Want> {
        self.selected = index;
        self.moved();
        self.wants_graph().into_iter().collect()
    }

    /// Takes the graph for `tip`, unless the selection has moved on.
    pub fn graph_loaded(&mut self, tip: &str, commits: Vec<Commit>) -> bool {
        if self.graph_of.as_deref() != Some(tip) {
            return false;
        }
        self.rows = layout(&commits, MAX_LANES);
        self.graph = commits;
        true
    }

    /// Whether the selected branch's graph is still on its way.
    pub fn graph_loading(&self) -> bool {
        // Asking sets `graph_of`, so an empty graph for a selected branch is still coming.
        self.selected_branch().is_some() && self.graph.is_empty()
    }

    /// Scrolls the graph by `delta` rows.
    pub fn scroll_graph(&mut self, delta: isize) -> bool {
        let height = usize::from(self.graph_area.height);
        let last = self.graph.len().saturating_sub(height);
        let to = self.graph_scroll.saturating_add_signed(delta).min(last);
        to != std::mem::replace(&mut self.graph_scroll, to)
    }

    /// A delete is asked about one branch, so moving cancels it; a new branch starts from
    /// wherever the selection is when it's confirmed.
    fn moved(&mut self) {
        if matches!(self.ask, Some(Ask::Delete { .. })) {
            self.ask = None;
        }
    }

    /// The new branch's name field, while asking for one.
    pub fn ask_field(&mut self) -> Option<&mut Field> {
        match &mut self.ask {
            Some(Ask::New { name }) => Some(name),
            _ => None,
        }
    }

    /// What confirming the open question asks for, closing it; a blank name stays open.
    pub fn answer(&mut self) -> Option<Want> {
        match self.ask.take()? {
            Ask::Delete { name, force } => Some(Want::DeleteBranch { name, force }),
            Ask::New { name } => {
                let text = name.text().trim().to_owned();
                let from = self.selected_branch().map(|b| b.tip.clone());
                if text.is_empty() || from.is_none() {
                    self.ask = Some(Ask::New { name });
                    return None;
                }
                Some(Want::NewBranch {
                    name: text,
                    from: from.unwrap_or_default(),
                })
            }
        }
    }

    /// Asks for the selected branch's graph unless it's shown or loading.
    fn wants_graph(&mut self) -> Option<Want> {
        let tip = self.selected_branch()?.tip.clone();
        if self.graph_of.as_deref() == Some(tip.as_str()) {
            return None;
        }
        (self.graph, self.rows, self.graph_scroll) = (Vec::new(), Vec::new(), 0);
        self.graph_of = Some(tip.clone());
        Some(Want::Graph(tip))
    }
}

#[cfg(test)]
pub(crate) fn branch(name: &str, kind: RefKind, head: bool) -> Branch {
    Branch {
        name: name.into(),
        kind,
        tip: format!("{name}-tip"),
        summary: format!("tip of {name}"),
        author: "Sawan".into(),
        time: 0,
        head,
        upstream: None,
    }
}

#[cfg(test)]
pub(crate) fn sample() -> Vec<Branch> {
    vec![
        branch("feat", RefKind::Local, false),
        branch("master", RefKind::Local, true),
        branch("origin/master", RefKind::Remote, false),
        branch("v0.1", RefKind::Tag, false),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_get_headers_and_moving_skips_them_and_asks_for_each_graph_once() {
        let (mut branches, first) = Branches::new();
        assert_eq!(first, Want::Branches);
        let wants = branches.loaded(sample());
        assert_eq!(
            wants,
            [Want::Graph("master-tip".into())],
            "HEAD's branch first"
        );
        assert_eq!(
            branches.lines(),
            [
                Line::Header(RefKind::Local),
                Line::Branch(0),
                Line::Branch(1),
                Line::Header(RefKind::Remote),
                Line::Branch(2),
                Line::Header(RefKind::Tag),
                Line::Branch(3),
            ]
        );
        let (moved, wants) = branches.step(1);
        assert!(moved);
        assert_eq!(wants, [Want::Graph("origin/master-tip".into())]);
        assert!(
            !branches.graph_loaded("master-tip", Vec::new()),
            "late answer dropped"
        );
        assert!(branches.graph_loaded("origin/master-tip", Vec::new()));
        let (_, wants) = branches.step(-1);
        branches.step(1);
        assert_eq!(wants.len(), 1);
        assert!(branches.step(9).1.len() == 1 && branches.selected == 3);
    }

    #[test]
    fn the_search_and_remotes_toggle_keep_only_what_matches() {
        let (mut branches, _) = Branches::new();
        branches.loaded(sample());
        branches.step(1);
        assert_eq!(branches.selected, 2, "on the remote");
        branches.toggle_remotes();
        assert_eq!(branches.shown, [0, 1, 3]);
        assert_eq!(branches.selected, 0, "moved off the hidden remote");
        branches.toggle_remotes();
        branches.search = Some(Field::single("mas"));
        branches.filter();
        assert_eq!(branches.shown, [1, 2]);
        assert_eq!(
            branches.lines(),
            [
                Line::Header(RefKind::Local),
                Line::Branch(1),
                Line::Header(RefKind::Remote),
                Line::Branch(2),
            ],
            "no header for the empty tags group"
        );
    }

    #[test]
    fn moving_cancels_a_delete_but_not_a_new_branch() {
        let (mut branches, _) = Branches::new();
        branches.loaded(sample());
        branches.ask = Some(Ask::Delete {
            name: "master".into(),
            force: false,
        });
        branches.step(1);
        assert!(branches.ask.is_none());
        branches.ask = Some(Ask::New {
            name: Box::new(Field::single("")),
        });
        assert!(branches.answer().is_none(), "blank stays open");
        branches.step(-1);
        branches.ask_field().expect("a name").paste("try-it");
        assert_eq!(
            branches.answer(),
            Some(Want::NewBranch {
                name: "try-it".into(),
                from: "master-tip".into()
            })
        );
    }
}

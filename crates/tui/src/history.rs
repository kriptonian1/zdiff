//! The history popup's state: recent commits, the selected one's files, and what to ask the
//! event loop for, since only it holds the repository.

use std::path::PathBuf;

use ratatui::layout::Rect;
use zdiff_core::{Branch, Commit, GraphRow, StashOp, StashPush, layout};

use crate::app::{DiffView, FileEntry};
use crate::input::Field;
use crate::stream::LARGE_DIFF;

/// Commits loaded at a time.
pub const PAGE: usize = 200;
/// Graph lanes drawn side by side; more show as `┊`.
pub const MAX_LANES: usize = 4;
/// How close to the last loaded commit the selection gets before the next page loads.
const PREFETCH: usize = 20;
/// Commits a search loads, page by page, while it has too few matches to fill the list.
// ponytail: matches only what's loaded; walk further on demand if 2000 is too few.
pub const SEARCH_LIMIT: usize = 2000;

/// What the popup lists: the commit history or the stashes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Log,
    Stash,
}

/// Which list in the popup takes the keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pane {
    #[default]
    Commits,
    Files,
    /// The diff of the selected file; keys scroll it.
    Preview,
}

/// Work for the event loop, which answers through the `App::history_*` and `viewing_*` calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Want {
    /// The newest `limit` commits.
    Commits(usize),
    /// The files `commit` changed.
    Files(Commit),
    /// The diff of `path` in `commit`, for the preview.
    Preview { commit: Commit, path: PathBuf },
    /// Show `commit` in the main view on `file`, against its parent or, with `worktree`,
    /// against the working tree; `back` is the file to return to.
    Open {
        commit: Commit,
        file: Option<PathBuf>,
        back: Option<PathBuf>,
        worktree: bool,
    },
    /// Show the working tree again.
    Back,
    /// The branches and tags, for the branches popup.
    Branches,
    /// The graph of the branch at this tip against HEAD.
    Graph(String),
    /// Switch to this branch or tag.
    Checkout(Branch),
    /// A new branch `name` at the commit `from`, checked out.
    NewBranch { name: String, from: String },
    /// Delete the local branch `name`, even unmerged with `force`.
    DeleteBranch { name: String, force: bool },
    /// The stashes, as commits.
    Stashes,
    /// Apply, pop or drop the stash `commit`.
    Stash { op: StashOp, commit: Commit },
    /// Stash the current changes.
    StashPush(StashPush),
    /// A new branch `name` from the stash `commit`.
    StashBranch { commit: Commit, name: String },
}

/// A question the hint row asks before a stash action.
#[derive(Debug)]
pub enum Ask {
    /// Drop the stash with this id?
    Drop(String),
    /// The stash form; `focus` 0 is the message, 1 to 3 the toggles.
    Push {
        message: Field,
        flags: StashPush,
        focus: u8,
    },
    /// The name for a branch from the stash with this id.
    Branch { id: String, name: Field },
}

/// The push form's toggles, as `Button::Toggle` numbers them.
const TOGGLES: [(u8, &str); 3] = [(1, "untracked"), (2, "keep staged"), (3, "staged only")];

/// What a hint-row button or its key does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Search,
    Enter,
    Open,
    Worktree,
    Copy,
    Back,
    Apply,
    Pop,
    Drop,
    Stash,
    Branch,
    /// Answers the open [`Ask`].
    Confirm,
    Cancel,
    /// Flips one of the push form's toggles, numbered as in [`TOGGLES`].
    Toggle(u8),
}

/// What pressing a button (or its key) asks of the app.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reply {
    pub redraw: bool,
    pub wants: Vec<Want>,
    /// Text for the clipboard.
    pub copy: Option<String>,
    pub close: bool,
}

/// The open history popup.
#[derive(Debug, Default)]
pub struct History {
    pub kind: Kind,
    /// The branch name and tip the list walks from, opened from the branches popup; `None`
    /// walks from every local branch.
    pub from: Option<(String, String)>,
    pub commits: Vec<Commit>,
    pub rows: Vec<GraphRow>,
    /// How many commits were asked for; fewer back means the history ended.
    limit: usize,
    pub done: bool,
    pub selected: usize,
    pub scroll: usize,
    pub pane: Pane,
    /// The selected commit's files, once loaded.
    pub files: Vec<FileEntry>,
    pub file: usize,
    pub file_scroll: usize,
    /// The commit `files` belong to.
    files_of: Option<String>,
    /// The selected file's diff, unified, once loaded.
    pub preview: Option<DiffView>,
    /// The commit and file `preview` shows, or is loading.
    preview_of: Option<(String, PathBuf)>,
    /// The selected file is too large to load without asking.
    pub large: bool,
    /// The search box, while there is a search; `typing` while keys go into it.
    pub search: Option<Field>,
    pub typing: bool,
    /// The question the hint row asks, for a stash action.
    pub ask: Option<Ask>,
    /// The commit HEAD points to, to tell whether a stash's base moved.
    pub head: Option<String>,
    /// Indices of the commits the search matches, all of them without one.
    pub shown: Vec<usize>,
    /// The popup, its lists, and the preview's rows from the last draw, for the mouse.
    pub area: Rect,
    pub list_area: Rect,
    pub files_area: Rect,
    pub preview_area: Rect,
    /// The hint row's buttons, the hash and `⧉` that copy it, and the search row and its `✕`.
    pub buttons: Vec<(Rect, Button)>,
    pub copy_areas: [Rect; 2],
    pub search_area: Rect,
    pub clear_area: Rect,
}

impl History {
    /// A popup waiting for its first page.
    pub fn new(kind: Kind) -> (Self, Want) {
        let history = Self {
            kind,
            limit: PAGE,
            ..Self::default()
        };
        let want = match kind {
            Kind::Log => Want::Commits(PAGE),
            Kind::Stash => Want::Stashes,
        };
        (history, want)
    }

    /// The hint row's buttons, in the order they're drawn.
    pub fn buttons(&self) -> &'static [Button] {
        use Button::{
            Apply, Back, Branch, Cancel, Confirm, Copy, Drop, Enter, Open, Pop, Search, Stash,
            Toggle, Worktree,
        };
        match (&self.ask, self.kind, self.pane) {
            (Some(Ask::Push { .. }), ..) => &[Confirm, Cancel, Toggle(1), Toggle(2), Toggle(3)],
            (Some(_), ..) => &[Confirm, Cancel],
            (None, Kind::Log, Pane::Commits) => &[Search, Enter, Open, Worktree, Copy, Back],
            (None, Kind::Log, Pane::Files) => &[Enter, Open, Worktree, Copy, Back],
            (None, Kind::Log, Pane::Preview) => &[Open, Worktree, Copy, Back],
            (None, Kind::Stash, Pane::Commits) => &[
                Search, Enter, Open, Apply, Pop, Drop, Stash, Branch, Copy, Back,
            ],
            (None, Kind::Stash, Pane::Files) => &[Enter, Open, Apply, Pop, Stash, Copy, Back],
            (None, Kind::Stash, Pane::Preview) => &[Open, Apply, Pop, Stash, Copy, Back],
        }
    }

    /// `[y copy hash]`, as the hint row draws `button`.
    pub fn label(&self, button: Button) -> String {
        let label = match (button, self.pane, &self.ask) {
            (Button::Search, ..) => "[/ search]",
            (Button::Enter, Pane::Commits, _) => "[↵ files]",
            (Button::Enter, ..) => "[↵ preview]",
            (Button::Open, ..) => "[o open]",
            (Button::Worktree, ..) => "[w vs worktree]",
            (Button::Copy, ..) => "[y copy hash]",
            (Button::Back, Pane::Commits, _) => "[esc close]",
            (Button::Back, ..) => "[esc back]",
            (Button::Apply, ..) => "[a apply]",
            (Button::Pop, ..) => "[p pop]",
            (Button::Drop, ..) => "[d drop]",
            (Button::Stash, ..) => "[s stash]",
            (Button::Branch, ..) => "[b branch]",
            (Button::Confirm, _, Some(Ask::Push { .. })) => "[↵ stash]",
            (Button::Confirm, _, Some(Ask::Branch { .. })) => "[↵ create]",
            (Button::Confirm, ..) => "[y drop]",
            (Button::Cancel, ..) => "[esc cancel]",
            (Button::Toggle(n), _, Some(Ask::Push { flags, .. })) => {
                let name = TOGGLES.iter().find(|t| t.0 == n).map_or("", |t| t.1);
                let mark = if checked(flags, n) { "✓" } else { " " };
                return format!("[{mark} {name}]");
            }
            (Button::Toggle(_), ..) => "",
        };
        label.to_owned()
    }

    /// The text field of an open push form or branch name.
    pub fn ask_field(&mut self) -> Option<&mut Field> {
        match &mut self.ask {
            Some(Ask::Push {
                message, focus: 0, ..
            }) => Some(message),
            Some(Ask::Branch { name, .. }) => Some(name),
            _ => None,
        }
    }

    /// Moves the push form's focus between the message and its toggles.
    pub fn ask_focus(&mut self, forward: bool) -> bool {
        let Some(Ask::Push { focus, .. }) = &mut self.ask else {
            return false;
        };
        let count = u8::try_from(TOGGLES.len()).unwrap_or(u8::MAX) + 1;
        *focus = if forward {
            (*focus + 1) % count
        } else {
            (*focus + count - 1) % count
        };
        true
    }

    /// The toggle the push form's focus is on.
    pub fn focused_toggle(&self) -> Option<u8> {
        match &self.ask {
            Some(Ask::Push { focus, .. }) if *focus > 0 => Some(*focus),
            _ => None,
        }
    }

    /// Takes a newly loaded list; asks for the selected commit's files if they aren't loaded,
    /// and for more commits if a search has too few matches.
    pub fn loaded(&mut self, commits: Vec<Commit>) -> Vec<Want> {
        // Stashes come all at once, so there's never a next page.
        self.done = self.kind == Kind::Stash || commits.len() < self.limit;
        if self.kind == Kind::Log {
            self.rows = layout(&commits, MAX_LANES);
        }
        self.commits = commits;
        self.selected = self.selected.min(self.commits.len().saturating_sub(1));
        self.filter()
    }

    /// The search text; empty without a search.
    pub fn query(&self) -> String {
        self.search.as_ref().map(Field::text).unwrap_or_default()
    }

    /// Whether `commit` matches `query`: summary or author containing it, or an id starting
    /// with it. Lowercase queries ignore case.
    fn matches(commit: &Commit, query: &str) -> bool {
        let has = |text: &str| contains(text, query);
        commit.id.starts_with(&query.to_lowercase()) || has(&commit.summary) || has(&commit.author)
    }

    /// Recomputes `shown` for the query, moving the selection onto a match.
    pub fn filter(&mut self) -> Vec<Want> {
        let query = self.query();
        self.shown = (0..self.commits.len())
            .filter(|&i| query.is_empty() || Self::matches(&self.commits[i], &query))
            .collect();
        let mut wants = Vec::new();
        if let Some(&first) = self.shown.first()
            && !self.shown.contains(&self.selected)
        {
            self.select(first);
        }
        wants.extend(self.wants_files());
        let height = usize::from(self.list_area.height).max(1);
        let short = !query.is_empty() && self.shown.len() < height;
        if short && !self.done && self.commits.len() < SEARCH_LIMIT {
            wants.push(self.next_page());
        }
        wants
    }

    /// Starts typing a search, keeping any text already there.
    pub fn start_search(&mut self) -> bool {
        self.search.get_or_insert_with(Field::default);
        self.pane = Pane::Commits;
        !std::mem::replace(&mut self.typing, true)
    }

    /// Clears the search and shows every commit again.
    pub fn clear_search(&mut self) -> Vec<Want> {
        (self.search, self.typing) = (None, false);
        self.filter()
    }

    fn select(&mut self, commit: usize) {
        self.selected = commit;
        (self.preview, self.preview_of, self.large) = (None, None, false);
        // The push form doesn't depend on the selection; the other questions do.
        if !matches!(self.ask, Some(Ask::Push { .. })) {
            self.ask = None;
        }
    }

    fn next_page(&mut self) -> Want {
        // Marked done until the page answers, so it's asked for once.
        self.done = true;
        self.limit = self.commits.len() + PAGE;
        Want::Commits(self.limit)
    }

    /// Where the selection is among the shown commits.
    pub fn position(&self) -> Option<usize> {
        self.shown.iter().position(|&i| i == self.selected)
    }

    /// Takes the files of commit `id`, unless the selection has moved on; asks for the first
    /// file's diff.
    pub fn files_loaded(&mut self, id: &str, files: Vec<FileEntry>) -> Option<Want> {
        if self.selected_commit().is_none_or(|c| c.id != id) {
            return None;
        }
        self.files = files;
        self.files_of = Some(id.to_owned());
        (self.file, self.file_scroll) = (0, 0);
        self.wants_preview(false)
    }

    /// Takes the diff of `path` in commit `id`, unless the selection has moved on.
    pub fn preview_loaded(&mut self, id: &str, path: &std::path::Path, view: DiffView) -> bool {
        let current = self
            .preview_of
            .as_ref()
            .is_some_and(|(of, at)| of == id && at == path);
        if current {
            self.preview = Some(view);
        }
        current
    }

    /// The selected file, for the preview's header.
    pub fn selected_file(&self) -> Option<&FileEntry> {
        self.files_current()
            .then(|| self.files.get(self.file))
            .flatten()
    }

    /// Asks for the selected file's diff unless it's shown or loading; a large one waits for
    /// `force`, as in the All files view.
    pub fn wants_preview(&mut self, force: bool) -> Option<Want> {
        let commit = self.selected_commit()?.clone();
        let file = self.selected_file()?;
        let path = file.path.clone();
        let large = file.added + file.removed > LARGE_DIFF;
        let of = Some((commit.id.clone(), path.clone()));
        if self.preview_of == of && (self.preview.is_some() || !self.large) {
            return None;
        }
        self.preview = None;
        self.large = large && !force;
        if self.large {
            self.preview_of = None;
            return None;
        }
        self.preview_of = of;
        Some(Want::Preview { commit, path })
    }

    pub fn selected_commit(&self) -> Option<&Commit> {
        self.commits.get(self.selected)
    }

    /// Moves the selection in the current pane by `delta`, stopping at either end; returns
    /// whether it moved and what to load for the new spot.
    pub fn step(&mut self, delta: isize) -> (bool, Vec<Want>) {
        match self.pane {
            Pane::Files => {
                let file = clamp_step(self.file, delta, self.files.len());
                if std::mem::replace(&mut self.file, file) == file {
                    return (false, Vec::new());
                }
                (true, self.wants_preview(false).into_iter().collect())
            }
            Pane::Preview => {
                let Some(view) = &mut self.preview else {
                    return (false, Vec::new());
                };
                let height = usize::from(self.preview_area.height);
                (view.scroll_by(delta, height), Vec::new())
            }
            Pane::Commits => {
                let Some(at) = self.position() else {
                    return (false, Vec::new());
                };
                let selected = self.shown[clamp_step(at, delta, self.shown.len())];
                if selected == self.selected {
                    return (false, Vec::new());
                }
                self.select(selected);
                let mut wants: Vec<Want> = self.wants_files().into_iter().collect();
                let near_end = selected + PREFETCH >= self.commits.len();
                if !self.done && near_end && self.query().is_empty() {
                    wants.push(self.next_page());
                }
                (true, wants)
            }
        }
    }

    /// Switches between the commit list and its files; files only once there are some.
    pub fn switch_pane(&mut self) -> bool {
        let pane = match self.pane {
            Pane::Commits if self.selected_file().is_some() => Pane::Files,
            _ => Pane::Commits,
        };
        pane != std::mem::replace(&mut self.pane, pane)
    }

    /// Enter: from the commits into their files, from a file into its preview, loading a
    /// large one first.
    pub fn enter(&mut self) -> (bool, Option<Want>) {
        match self.pane {
            Pane::Commits => (self.switch_pane(), None),
            Pane::Files if self.large => (true, self.wants_preview(true)),
            Pane::Files if self.preview.is_some() => {
                self.pane = Pane::Preview;
                (true, None)
            }
            Pane::Files | Pane::Preview => (false, None),
        }
    }

    /// Esc: back one pane; `false` from the commits, where Esc closes the popup.
    pub fn back(&mut self) -> bool {
        self.pane = match self.pane {
            Pane::Commits => return false,
            Pane::Files => Pane::Commits,
            Pane::Preview => Pane::Files,
        };
        true
    }

    /// Moves the preview to the next or previous change.
    pub fn preview_change(&mut self, forward: bool) -> bool {
        let height = usize::from(self.preview_area.height);
        self.preview.as_mut().is_some_and(|view| {
            if forward {
                view.next_header(height)
            } else {
                view.prev_header(height)
            }
        })
    }

    /// Scrolls the preview by `delta` rows from any pane.
    pub fn scroll_preview(&mut self, delta: isize) -> bool {
        let height = usize::from(self.preview_area.height);
        (self.preview.as_mut()).is_some_and(|view| view.scroll_by(delta, height))
    }

    /// What `button` does; `back` is the file the main view returns to after opening.
    pub fn press(&mut self, button: Button, back: Option<PathBuf>) -> Reply {
        let mut reply = Reply::default();
        match button {
            Button::Search => reply.redraw = self.start_search() || self.typing,
            Button::Enter => {
                let (moved, want) = self.enter();
                reply.redraw = moved;
                reply.wants.extend(want);
            }
            Button::Open | Button::Worktree => {
                reply
                    .wants
                    .extend(self.open(back, button == Button::Worktree));
                reply.redraw = !reply.wants.is_empty();
            }
            Button::Copy => {
                reply.copy = self.selected_commit().map(|c| c.id.clone());
                reply.redraw = reply.copy.is_some();
            }
            Button::Apply | Button::Pop => {
                let op = if button == Button::Pop {
                    StashOp::Pop
                } else {
                    StashOp::Apply
                };
                reply.wants.extend(self.stash(op));
                reply.redraw = !reply.wants.is_empty();
            }
            Button::Drop | Button::Branch => {
                let stash = self.selected_commit().filter(|_| self.kind == Kind::Stash);
                let id = stash.map(|c| c.id.clone());
                self.ask = id.map(|id| match button {
                    Button::Drop => Ask::Drop(id),
                    _ => Ask::Branch {
                        id,
                        name: Field::single(""),
                    },
                });
                reply.redraw = self.ask.is_some();
            }
            Button::Stash => {
                if self.kind == Kind::Stash {
                    let flags = StashPush {
                        untracked: true,
                        ..StashPush::default()
                    };
                    self.ask = Some(Ask::Push {
                        message: Field::single(""),
                        flags,
                        focus: 0,
                    });
                    self.typing = false;
                    reply.redraw = true;
                }
            }
            Button::Toggle(n) => {
                if let Some(Ask::Push { flags, .. }) = &mut self.ask {
                    let on = !checked(flags, n);
                    match n {
                        1 => flags.untracked = on,
                        2 => flags.keep_index = on,
                        _ => flags.staged = on,
                    }
                    // git refuses `--staged` with the other two.
                    if on && n == 3 {
                        (flags.untracked, flags.keep_index) = (false, false);
                    } else if on {
                        flags.staged = false;
                    }
                    reply.redraw = true;
                }
            }
            Button::Confirm => {
                reply.wants.extend(self.answer());
                reply.redraw = true;
            }
            Button::Cancel => {
                self.ask = None;
                reply.redraw = true;
            }
            Button::Back if self.ask.is_some() => {
                self.ask = None;
                reply.redraw = true;
            }
            Button::Back => {
                reply.redraw = true;
                if self.typing || self.search.is_some() {
                    reply.wants = self.clear_search();
                } else if !self.back() {
                    reply.close = true;
                }
            }
        }
        reply
    }

    /// What answering the open question asks for, closing it; a blank branch name stays open.
    fn answer(&mut self) -> Option<Want> {
        match self.ask.take()? {
            Ask::Drop(id) => Some(Want::Stash {
                op: StashOp::Drop,
                commit: self.commits.iter().find(|c| c.id == id)?.clone(),
            }),
            Ask::Push { message, flags, .. } => Some(Want::StashPush(StashPush {
                message: message.text(),
                ..flags
            })),
            Ask::Branch { id, name } => {
                let text = name.text().trim().to_owned();
                if text.is_empty() {
                    self.ask = Some(Ask::Branch { id, name });
                    return None;
                }
                let commit = self.commits.iter().find(|c| c.id == id)?.clone();
                Some(Want::StashBranch { commit, name: text })
            }
        }
    }

    /// `op` on the selected stash; nothing in the commit history.
    fn stash(&self, op: StashOp) -> Option<Want> {
        let commit = self
            .selected_commit()
            .filter(|_| self.kind == Kind::Stash)?;
        Some(Want::Stash {
            op,
            commit: commit.clone(),
        })
    }

    /// Opening the selection: the commit on the chosen file, or its first file.
    pub fn open(&self, back: Option<PathBuf>, worktree: bool) -> Option<Want> {
        let commit = self.selected_commit()?.clone();
        let index = if self.pane == Pane::Commits {
            0
        } else {
            self.file
        };
        let file = (self.files_current())
            .then(|| self.files.get(index).map(|f| f.path.clone()))
            .flatten();
        Some(Want::Open {
            commit,
            file,
            back,
            worktree,
        })
    }

    /// Whether the selected commit's files are still on their way; never without a commit.
    pub fn files_loading(&self) -> bool {
        self.selected_commit().is_some() && !self.files_current()
    }

    /// Whether `files` are the selected commit's.
    fn files_current(&self) -> bool {
        self.files_of.as_deref() == self.selected_commit().map(|c| c.id.as_str())
    }

    fn wants_files(&mut self) -> Option<Want> {
        if self.files_current() {
            return None;
        }
        self.pane = Pane::Commits;
        Some(Want::Files(self.selected_commit()?.clone()))
    }
}

/// Whether the push flag `Button::Toggle(n)` flips is on.
fn checked(flags: &StashPush, n: u8) -> bool {
    match n {
        1 => flags.untracked,
        2 => flags.keep_index,
        _ => flags.staged,
    }
}

/// Whether `text` contains `query`, ignoring case when the query is all lowercase.
pub fn contains(text: &str, query: &str) -> bool {
    if query.chars().any(char::is_uppercase) {
        text.contains(query)
    } else {
        text.to_lowercase().contains(query)
    }
}

/// `at + delta` kept inside `0..len`.
fn clamp_step(at: usize, delta: isize, len: usize) -> usize {
    at.saturating_add_signed(delta).min(len.saturating_sub(1))
}

/// `a1b2c3d^ → a1b2c3d`, `∅ → a1b2c3d` for a root commit, or `a1b2c3d → worktree`, for
/// the footer.
pub fn label(commit: &Commit, worktree: bool) -> String {
    let id = stash_name(commit).unwrap_or_else(|| short(&commit.id));
    if worktree {
        format!("{id} → worktree")
    } else if stash_name(commit).is_some() {
        id.to_owned()
    } else if commit.parents.is_empty() {
        format!("∅ → {id}")
    } else {
        format!("{id}^ → {id}")
    }
}

/// `stash@{n}` for a stash listed by [`zdiff_core::Repo::stashes`].
pub fn stash_name(commit: &Commit) -> Option<&str> {
    (commit.refs.first())
        .map(String::as_str)
        .filter(|_| commit.is_stash())
}

/// The 7-character id git shows.
pub fn short(id: &str) -> &str {
    id.get(..7).unwrap_or(id)
}

#[cfg(test)]
pub(crate) fn commit(id: &str, parents: &[&str]) -> Commit {
    Commit {
        id: id.into(),
        summary: format!("commit {id}"),
        message: format!("commit {id}"),
        author: "Sawan".into(),
        time: 0,
        parents: parents.iter().map(|p| (*p).to_owned()).collect(),
        refs: Box::default(),
        head: false,
    }
}

/// Two stashes on `base`, `stash@{0}` first.
#[cfg(test)]
pub(crate) fn stashes() -> Vec<Commit> {
    (0..2)
        .map(|n| {
            let mut stash = commit(&format!("s{n}"), &["base"]);
            stash.refs = vec![format!("stash@{{{n}}}")].into();
            stash
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(n: usize) -> Vec<Commit> {
        let ids: Vec<String> = (0..n).map(|i| format!("{i:07}")).collect();
        (0..n)
            .map(|i| match ids.get(i + 1) {
                Some(parent) => commit(&ids[i], &[parent]),
                None => commit(&ids[i], &[]),
            })
            .collect()
    }

    #[test]
    fn moving_asks_for_files_and_the_next_page_near_the_end() {
        let (mut history, first) = History::new(Kind::Log);
        assert_eq!(first, Want::Commits(PAGE));
        let want = history.loaded(chain(PAGE));
        assert!(matches!(&want[..], [Want::Files(c)] if c.id == "0000000"));
        assert!(!history.done);
        let (moved, wants) = history.step(1);
        assert!(moved);
        assert!(matches!(&wants[..], [Want::Files(c)] if c.id == "0000001"));
        let (_, wants) = history.step(500);
        assert_eq!(history.selected, PAGE - 1, "stops at the last loaded");
        assert!(wants.contains(&Want::Commits(2 * PAGE)));
        let (_, wants) = history.step(-1);
        assert!(
            !wants.iter().any(|w| matches!(w, Want::Commits(_))),
            "asked once"
        );

        history.loaded(chain(PAGE + 3));
        assert!(history.done, "fewer than asked: the end");
    }

    #[test]
    fn files_are_kept_only_for_the_selected_commit() {
        let (mut history, _) = History::new(Kind::Log);
        history.loaded(chain(3));
        assert!(
            history.files_loaded("0000001", Vec::new()).is_none(),
            "not selected"
        );
        assert!(!history.switch_pane(), "no files yet");
        let entry = crate::app::FileEntry {
            path: "a.rs".into(),
            status: zdiff_core::Status::Modified,
            from: None,
            added: 1,
            removed: 0,
            change: 0,
            staged: zdiff_core::Staged::No,
        };
        assert!(
            history.files_loaded("0000000", vec![entry]).is_some(),
            "asks for its diff"
        );
        assert!(history.switch_pane());
        let Some(Want::Open { commit, file, .. }) = history.open(None, false) else {
            panic!("opens")
        };
        assert_eq!((commit.id.as_str(), file), ("0000000", Some("a.rs".into())));
        history.pane = Pane::Commits;
        history.step(1);
        assert_eq!(history.pane, Pane::Commits);
        let Some(Want::Open { file, .. }) = history.open(None, false) else {
            panic!("opens")
        };
        assert_eq!(file, None, "files not loaded for this one yet");
    }

    #[test]
    fn labels_name_the_parent_or_the_empty_tree() {
        assert_eq!(
            label(&commit("abcdef1234", &["0"]), false),
            "abcdef1^ → abcdef1"
        );
        assert_eq!(label(&commit("abcdef1234", &[]), false), "∅ → abcdef1");
    }

    /// Three commits: `fix theme` by Riya, `Add Search` by Sawan, `docs` by Sawan.
    fn named() -> History {
        let mut commits = chain(3);
        for (commit, (summary, author)) in commits.iter_mut().zip([
            ("fix theme", "Riya"),
            ("Add Search", "Sawan"),
            ("docs", "Sawan"),
        ]) {
            (commit.summary, commit.author) = (summary.into(), author.into());
        }
        commits[2].id = "abc1234ff".into();
        let (mut history, _) = History::new(Kind::Log);
        history.loaded(commits);
        history
    }

    fn search(history: &mut History, text: &str) -> Vec<Want> {
        history.start_search();
        history.search = Some(Field::single(text));
        history.filter()
    }

    #[test]
    fn a_search_matches_summary_author_or_hash_ignoring_case_when_lowercase() {
        let mut history = named();
        let shown = |h: &mut History, text: &str| {
            search(h, text);
            h.shown.clone()
        };
        assert_eq!(shown(&mut history, "search"), [1], "lowercase ignores case");
        assert!(shown(&mut history, "Search").contains(&1));
        assert!(
            shown(&mut history, "SEARCH").is_empty(),
            "uppercase is exact"
        );
        assert_eq!(shown(&mut history, "sawan"), [1, 2], "author");
        assert_eq!(shown(&mut history, "abc12"), [2], "hash prefix");
        assert_eq!(history.selected, 2, "the selection moves onto a match");
        history.clear_search();
        assert_eq!(history.shown, [0, 1, 2]);
    }

    #[test]
    fn moving_skips_commits_the_search_hides() {
        let mut history = named();
        search(&mut history, "sawan");
        assert_eq!(history.selected, 1);
        history.step(1);
        assert_eq!(history.selected, 2);
        history.step(1);
        assert_eq!(history.selected, 2, "the last match");
        history.step(-5);
        assert_eq!(history.selected, 1, "commit 0 is hidden");
    }

    #[test]
    fn too_few_matches_load_more_commits_up_to_the_limit() {
        let (mut history, _) = History::new(Kind::Log);
        history.loaded(chain(PAGE));
        history.list_area = Rect::new(0, 0, 40, 10);
        let wants = search(&mut history, "0000001");
        assert!(wants.contains(&Want::Commits(2 * PAGE)), "{wants:?}");
        let wants = search(&mut history, "0000001");
        assert!(
            !wants.iter().any(|w| matches!(w, Want::Commits(_))),
            "asked once"
        );

        history.loaded(chain(SEARCH_LIMIT));
        history.list_area = Rect::new(0, 0, 40, 10);
        let wants = history.filter();
        assert!(
            !wants.iter().any(|w| matches!(w, Want::Commits(_))),
            "stops at the limit"
        );
    }

    #[test]
    fn buttons_copy_open_against_the_worktree_and_step_back() {
        let mut history = named();
        let reply = history.press(Button::Copy, None);
        assert_eq!(reply.copy.as_deref(), Some("0000000"), "the full id");
        let reply = history.press(Button::Worktree, Some("a.rs".into()));
        assert!(matches!(
            &reply.wants[..],
            [Want::Open {
                worktree: true,
                back: Some(_),
                ..
            }]
        ));

        search(&mut history, "docs");
        let reply = history.press(Button::Back, None);
        assert!(
            history.search.is_none() && !reply.close,
            "Back clears the search first"
        );
        assert!(history.press(Button::Back, None).close, "then closes");
        assert!(history.press(Button::Search, None).redraw);
        assert!(history.typing && history.search.is_some());
    }

    #[test]
    fn labels_say_when_the_other_side_is_the_working_tree() {
        assert_eq!(
            label(&commit("abcdef1234", &["0"]), true),
            "abcdef1 → worktree"
        );
    }

    #[test]
    fn a_stash_list_loads_once_and_drops_only_after_confirming() {
        let (mut history, first) = History::new(Kind::Stash);
        assert_eq!(first, Want::Stashes);
        history.loaded(stashes());
        assert!(
            history.done && history.rows.is_empty(),
            "no pages, no graph"
        );
        assert_eq!(label(&history.commits[0], false), "stash@{0}");

        let reply = history.press(Button::Drop, None);
        assert!(reply.wants.is_empty() && matches!(history.ask, Some(Ask::Drop(_))));
        assert_eq!(history.buttons(), [Button::Confirm, Button::Cancel]);
        history.press(Button::Cancel, None);
        assert!(history.ask.is_none());

        history.press(Button::Drop, None);
        history.step(1);
        assert!(history.ask.is_none(), "moving cancels");
        history.press(Button::Drop, None);
        let reply = history.press(Button::Confirm, None);
        assert!(matches!(
            &reply.wants[..],
            [Want::Stash { op: StashOp::Drop, commit }] if commit.id == "s1"
        ));
        let reply = history.press(Button::Pop, None);
        assert!(matches!(
            &reply.wants[..],
            [Want::Stash {
                op: StashOp::Pop,
                ..
            }]
        ));
    }

    #[test]
    fn the_commit_history_has_no_stash_actions() {
        let (mut history, _) = History::new(Kind::Log);
        history.loaded(chain(3));
        assert!(history.press(Button::Pop, None).wants.is_empty());
        assert!(!history.buttons().contains(&Button::Drop));
    }

    fn flags(history: &History) -> (bool, bool, bool) {
        match &history.ask {
            Some(Ask::Push { flags, .. }) => (flags.untracked, flags.keep_index, flags.staged),
            _ => panic!("no push form"),
        }
    }

    #[test]
    fn the_push_form_keeps_staged_only_apart_from_the_other_toggles() {
        let (mut history, _) = History::new(Kind::Stash);
        history.loaded(Vec::new());
        history.press(Button::Stash, None);
        assert_eq!(
            flags(&history),
            (true, false, false),
            "untracked by default"
        );
        history.press(Button::Toggle(2), None);
        history.press(Button::Toggle(3), None);
        assert_eq!(flags(&history), (false, false, true));
        history.press(Button::Toggle(1), None);
        assert_eq!(flags(&history), (true, false, false));

        history.ask_field().expect("message").paste("wip");
        let reply = history.press(Button::Confirm, None);
        assert!(matches!(
            &reply.wants[..],
            [Want::StashPush(push)] if push.message == "wip" && push.untracked
        ));
        assert!(history.ask.is_none());
    }

    #[test]
    fn a_branch_needs_a_name_and_moving_cancels_it_but_not_the_push_form() {
        let (mut history, _) = History::new(Kind::Stash);
        history.loaded(stashes());
        history.press(Button::Branch, None);
        assert!(
            history.press(Button::Confirm, None).wants.is_empty(),
            "blank"
        );
        assert!(
            matches!(history.ask, Some(Ask::Branch { .. })),
            "still asking"
        );
        history.ask_field().expect("name").paste("try-it");
        let reply = history.press(Button::Confirm, None);
        assert!(matches!(
            &reply.wants[..],
            [Want::StashBranch { commit, name }] if commit.id == "s0" && name == "try-it"
        ));

        history.press(Button::Branch, None);
        history.step(1);
        assert!(history.ask.is_none(), "moving cancels a branch");
        history.press(Button::Stash, None);
        history.step(-1);
        assert!(
            matches!(history.ask, Some(Ask::Push { .. })),
            "but not the form"
        );
    }
}

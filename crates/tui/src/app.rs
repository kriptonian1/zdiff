use std::mem;
use std::path::PathBuf;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::ListState;
use zdiff_core::{Error, FileDiff, Row, Side, Status, Text, WordChanges};
use zdiff_highlight::{Language, Token};
use zdiff_search::{Finder, Query};

use crate::find::{self, Find, Toggle};
use crate::palette::{self, Mode, Palette};
use crate::search::Found;
use crate::text;
use crate::tree::Node;
use crate::tree::Tree;

/// Rows moved per scroll-wheel tick; the usual step in terminal apps.
const WHEEL_STEP: isize = 3;
/// Unchanged lines shown around each change; git's default.
const CONTEXT_LINES: u32 = 3;
/// Columns moved per horizontal key press or scroll tick.
const H_STEP: isize = 4;
/// Columns you can scroll past the end of the widest line, so its last character isn't flush with the edge.
const H_OVERSCROLL: usize = 4;

#[derive(Debug)]
pub struct FileEntry {
    pub path: PathBuf,
    pub status: Status,
    pub added: u32,
    pub removed: u32,
    /// Index of this file's `Change` in the list the app was started with.
    pub change: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Diff,
}

/// The loaded diff of the selected file and its scroll position.
#[derive(Debug)]
pub struct DiffView {
    pub file: FileDiff,
    pub rows: Vec<Row>,
    pub scroll: usize,
    /// Columns of line text scrolled off the left, shared by both panes.
    pub hscroll: usize,
    /// Width of the widest line number, in digits.
    pub gutter: usize,
    /// Columns of line text the narrower pane shows; set on every draw.
    pub text_width: usize,
    pub old_tokens: Box<[Token]>,
    pub new_tokens: Box<[Token]>,
    pub words: WordChanges,
    /// Row reached with go to line; cleared by the next scroll.
    pub mark: Option<usize>,
}

#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "the app holds a single pane, so boxing saves nothing"
)]
pub enum DiffPane {
    Empty,
    Failed(Box<str>),
    Loaded(DiffView),
}

#[derive(Debug)]
pub struct App {
    pub tree: Tree,
    /// Footer text: repo folder, current branch, and what the diff compares.
    pub repo_name: String,
    pub branch: String,
    pub compare: String,
    /// Selected sidebar row.
    cursor: usize,
    /// Node index of the file the diff pane shows; stays put while a folder is selected.
    file: Option<usize>,
    pub list: ListState,
    pub focus: Focus,
    pub diff: DiffPane,
    /// Sidebar area from the last draw, for mouse hit-testing.
    pub sidebar: Rect,
    /// Diff body area from the last draw, for mouse hit-testing and paging.
    pub diff_area: Rect,
    /// Width the user dragged the sidebar to; `None` keeps the automatic width.
    pub sidebar_width: Option<u16>,
    /// Whether Ctrl+B or the footer label hid the sidebar; its width is kept for showing it again.
    pub sidebar_hidden: bool,
    /// The footer's show/hide label from the last draw, for clicks.
    pub sidebar_toggle: Rect,
    /// Whether the sidebar divider is being dragged.
    pub resizing: bool,
    /// The find bar while it is open; its query outlives file switches.
    pub find: Option<Find>,
    /// A global search for the event loop to hand to the worker: `(generation, finder, change indices)`.
    pub pending_search: Option<(u64, Finder, Vec<usize>)>,
    /// Where to jump, one-based, once the diff opened from a search hit loads.
    pending_jump: Option<(Side, u32)>,
    /// The go-to popup while it is open.
    pub palette: Option<Palette>,
    /// Palette area from the last draw, for closing it on an outside click.
    pub palette_area: Rect,
    pub quit: bool,
}

impl App {
    pub fn new(files: Vec<FileEntry>) -> Self {
        let mut app = Self {
            tree: Tree::default(),
            repo_name: String::new(),
            branch: String::new(),
            compare: String::new(),
            cursor: 0,
            file: None,
            list: ListState::default(),
            focus: Focus::Sidebar,
            diff: DiffPane::Empty,
            sidebar: Rect::default(),
            diff_area: Rect::default(),
            sidebar_width: None,
            sidebar_hidden: false,
            sidebar_toggle: Rect::default(),
            resizing: false,
            find: None,
            pending_search: None,
            pending_jump: None,
            palette: None,
            palette_area: Rect::default(),
            quit: false,
        };
        app.set_files(files);
        let first_file = (0..app.tree.len()).find(|&row| app.tree.file_at(row).is_some());
        app.select(first_file.unwrap_or(0));
        app
    }

    /// Swaps in a new file list, keeping the selection, shown file, and closed folders by path.
    pub fn refresh(&mut self, files: Vec<FileEntry>) {
        let cursor_path = self.tree.node(self.cursor).map(|n| n.path().to_owned());
        let file_path = self.selected_file().map(|f| f.path.clone());
        let picked = match &self.palette {
            Some(Palette {
                mode: Mode::File(files),
                ..
            }) => files.selected().and_then(|node| self.tree.file(node)),
            _ => None,
        }
        .map(|f| f.path.clone());
        self.set_files(files);
        if let Some(Palette {
            input,
            mode: Mode::File(files),
        }) = &mut self.palette
        {
            files.update(input, &self.tree);
            let tree = &self.tree;
            let same = picked.and_then(|path| {
                (files.hits.iter())
                    .position(|hit| tree.file(hit.node).is_some_and(|f| f.path == path))
            });
            if same.is_some() {
                files.list.select(same);
            }
        }
        self.file = file_path.and_then(|path| self.tree.find_file(&path));
        let same = cursor_path.and_then(|path| self.tree.find(&path));
        let last = self.tree.len().saturating_sub(1);
        self.select(same.unwrap_or(self.cursor.min(last)));
        // Tree nodes and change indices changed, so run an open global search again.
        self.search();
    }

    fn set_files(&mut self, files: Vec<FileEntry>) {
        self.tree = Tree::new(files, mem::take(&mut self.tree).into_collapsed());
    }

    /// The file shown in the diff pane: the last file the sidebar cursor was on.
    pub fn selected_file(&self) -> Option<&FileEntry> {
        self.tree.file(self.file?)
    }

    /// Replaces the diff pane with the selected file's diff, scrolled to the top.
    pub fn show(&mut self, diff: Option<Result<FileDiff, Error>>) {
        self.diff = match diff {
            None => DiffPane::Empty,
            Some(Ok(file)) => {
                let language = self
                    .selected_file()
                    .and_then(|f| zdiff_highlight::language(&f.path));
                DiffPane::Loaded(DiffView::new(file, language))
            }
            Some(Err(e)) => DiffPane::Failed(e.to_string().into()),
        };
        self.sync_view();
    }

    /// Re-runs the find bar on the shown diff and applies a pending search-hit jump.
    fn sync_view(&mut self) {
        let height = usize::from(self.diff_area.height);
        let DiffPane::Loaded(view) = &mut self.diff else {
            return;
        };
        if let Some(find) = &mut self.find {
            find.update(view);
        }
        let Some((side, line)) = self.pending_jump.take() else {
            return;
        };
        view.jump(side, line, height);
        if let Some(find) = &mut self.find {
            let same = |h: &zdiff_search::Hit| h.side == side && h.line + 1 == line;
            find.current = find.hits.iter().position(same).or(find.current);
        }
    }

    /// Like [`App::show`] for the file already shown: keeps the scroll position.
    pub fn reload(&mut self, diff: Option<Result<FileDiff, Error>>) {
        let position = match &self.diff {
            DiffPane::Loaded(view) => (view.scroll, view.hscroll),
            _ => (0, 0),
        };
        self.show(diff);
        if let DiffPane::Loaded(view) = &mut self.diff {
            (view.scroll, view.hscroll) = position;
        }
    }

    /// Applies `event`; returns whether anything visible changed and a redraw is needed.
    pub fn handle(&mut self, event: &Event) -> bool {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => self.on_key(*key),
            Event::Mouse(mouse) => self.on_mouse(*mouse),
            Event::Resize(..) => true,
            _ => false,
        }
    }

    fn on_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Raw mode turns Ctrl+C into a key press instead of SIGINT.
        if ctrl && key.code == KeyCode::Char('c') {
            return self.stop();
        }
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('p') if ctrl => return self.open_files(),
            KeyCode::Char('g') if ctrl => return self.open_goto(),
            KeyCode::Char('b') if ctrl => return self.toggle_sidebar(),
            KeyCode::Char('f' | 'F') if ctrl && shift => {
                return self.open_search(Query::default(), String::new());
            }
            KeyCode::Char('f') if ctrl => return self.ctrl_f(),
            _ if self.palette.is_some() => return self.palette_key(key),
            _ if self.find.is_some() => return self.find_key(key),
            _ => {}
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.stop(),
            KeyCode::Tab if !self.sidebar_hidden => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Diff,
                    Focus::Diff => Focus::Sidebar,
                };
                true
            }
            _ => match self.focus {
                Focus::Sidebar => self.sidebar_key(key.code),
                Focus::Diff => self.diff_key(key.code, ctrl),
            },
        }
    }

    /// Hides or shows the sidebar; hiding moves focus to the diff so keys never go to a hidden pane.
    fn toggle_sidebar(&mut self) -> bool {
        self.sidebar_hidden = !self.sidebar_hidden;
        self.resizing = false;
        if self.sidebar_hidden {
            self.focus = Focus::Diff;
        }
        true
    }

    fn open_goto(&mut self) -> bool {
        if !matches!(self.diff, DiffPane::Loaded(_)) {
            return false;
        }
        self.palette = Some(Palette::line());
        self.focus = Focus::Diff;
        true
    }

    /// Ctrl+F: find in the file, widen an open find bar to all files, or search a sidebar folder.
    fn ctrl_f(&mut self) -> bool {
        if let Some(find) = self.find.take() {
            return self.open_search(find.query, String::new());
        }
        if self.focus == Focus::Sidebar
            && let Some(Node::Dir { path, .. }) = self.tree.node(self.cursor)
        {
            let include = path.display().to_string();
            return self.open_search(Query::default(), include);
        }
        self.open_find()
    }

    fn open_find(&mut self) -> bool {
        if !matches!(self.diff, DiffPane::Loaded(_)) {
            return false;
        }
        self.palette = None;
        self.find.get_or_insert_default();
        self.focus = Focus::Diff;
        true
    }

    /// Edits the find query and moves between hits; Esc closes the bar.
    fn find_key(&mut self, key: KeyEvent) -> bool {
        let (Some(find), DiffPane::Loaded(view)) = (&mut self.find, &mut self.diff) else {
            return false;
        };
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let height = usize::from(self.diff_area.height);
        match key.code {
            KeyCode::Esc => {
                self.find = None;
                return true;
            }
            KeyCode::Enter | KeyCode::Down | KeyCode::Up => {
                let forward = key.code == KeyCode::Down || (key.code == KeyCode::Enter && !shift);
                if let Some(hit) = find.step(forward) {
                    view.jump(hit.side, hit.line + 1, height);
                }
                return true;
            }
            KeyCode::Char(c) if alt => match Toggle::for_alt(c) {
                Some(toggle) => toggle.flip(&mut find.query),
                None => return false,
            },
            KeyCode::Char(c) => find.query.text.push(c),
            KeyCode::Backspace => {
                find.query.text.pop();
            }
            _ => return false,
        }
        refind(find, view, height);
        true
    }

    fn open_files(&mut self) -> bool {
        if self.tree.is_empty() {
            return false;
        }
        self.palette = Some(Palette::files(&self.tree));
        true
    }

    fn open_search(&mut self, query: Query, include: String) -> bool {
        if self.tree.is_empty() {
            return false;
        }
        self.find = None;
        self.palette = Some(Palette::search(query, include));
        self.search();
        true
    }

    /// Restarts the global search from its current fields; the event loop sends the job.
    fn search(&mut self) {
        if let Some(Palette {
            mode: Mode::Search(search),
            ..
        }) = &mut self.palette
        {
            self.pending_search = search.job(&self.tree);
        }
    }

    /// Adds one file's hits from the worker, unless they belong to an older search.
    pub fn search_found(&mut self, generation: u64, change: usize, hits: Vec<Found>) -> bool {
        let node = (self.tree.files())
            .find(|(_, entry)| entry.change == change)
            .map(|(node, _)| node);
        let (Some(node), Some(search)) = (node, self.current_search(generation)) else {
            return false;
        };
        search.push(node, hits);
        true
    }

    pub fn search_done(&mut self, generation: u64) -> bool {
        let Some(search) = self.current_search(generation) else {
            return false;
        };
        search.done = true;
        true
    }

    fn current_search(&mut self, generation: u64) -> Option<&mut palette::Search> {
        match &mut self.palette {
            Some(Palette {
                mode: Mode::Search(search),
                ..
            }) if search.generation == generation => Some(search),
            _ => None,
        }
    }

    /// Opens the file of a search hit at its line, with the find bar on the same query.
    fn open_hit(&mut self, target: Option<(usize, Side, u32)>) -> bool {
        let Some((node, side, line)) = target else {
            return false;
        };
        let query = match &self.palette {
            Some(Palette {
                mode: Mode::Search(search),
                ..
            }) => search.query.clone(),
            _ => Query::default(),
        };
        self.find = Some(Find {
            query,
            ..Find::default()
        });
        self.pending_jump = Some((side, line + 1));
        let shown = self.file == Some(node);
        self.open_file(node);
        // The event loop reloads only a different file, so apply the jump here.
        if shown {
            self.sync_view();
        }
        true
    }

    /// Keys for the global search palette.
    fn search_key(&mut self, key: KeyEvent) -> bool {
        let Some(Palette {
            mode: Mode::Search(search),
            ..
        }) = &mut self.palette
        else {
            return false;
        };
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => self.palette = None,
            KeyCode::Enter => {
                let target = search.selected();
                return self.open_hit(target);
            }
            KeyCode::Up => return search.step(false),
            KeyCode::Down => return search.step(true),
            KeyCode::Tab => search.cycle(true),
            KeyCode::BackTab => search.cycle(false),
            KeyCode::Char(c) if alt => match Toggle::for_alt(c) {
                Some(toggle) => {
                    toggle.flip(&mut search.query);
                    self.search();
                }
                None => return false,
            },
            KeyCode::Char(c) => {
                search.field_mut().push(c);
                self.search();
            }
            KeyCode::Backspace => {
                search.field_mut().pop();
                self.search();
            }
            _ => return false,
        }
        true
    }

    /// Edits the palette input and acts on it; Esc cancels.
    fn palette_key(&mut self, key: KeyEvent) -> bool {
        let Some(Palette { input, mode }) = &mut self.palette else {
            return false;
        };
        if matches!(mode, Mode::Search(_)) {
            return self.search_key(key);
        }
        match (key.code, mode) {
            (KeyCode::Esc, _) => self.palette = None,
            (KeyCode::Enter, Mode::Line) => {
                let target = palette::parse(input);
                self.palette = None;
                let height = usize::from(self.diff_area.height);
                if let (Some((side, line)), DiffPane::Loaded(view)) = (target, &mut self.diff) {
                    view.jump(side, line, height);
                }
            }
            (KeyCode::Enter, Mode::File(files)) => match files.selected() {
                Some(node) => return self.open_file(node),
                None => self.palette = None,
            },
            (KeyCode::Up, Mode::File(files)) => return files.step(false),
            (KeyCode::Down, Mode::File(files)) => return files.step(true),
            (KeyCode::Char(c @ '0'..='9'), Mode::Line) => input.push(c),
            (KeyCode::Char(c @ ('+' | '-')), Mode::Line) if input.is_empty() => input.push(c),
            (KeyCode::Backspace, Mode::Line) => {
                input.pop();
            }
            (KeyCode::Char(c), Mode::File(files)) => {
                input.push(c);
                files.update(input, &self.tree);
            }
            (KeyCode::Backspace, Mode::File(files)) => {
                input.pop();
                files.update(input, &self.tree);
            }
            _ => return false,
        }
        true
    }

    /// Selects the file at tree `node` in the sidebar, opening its folders, and closes the palette.
    fn open_file(&mut self, node: usize) -> bool {
        self.palette = None;
        let path = self.tree.file(node).map(|f| f.path.clone());
        if let Some(row) = path.and_then(|path| self.tree.reveal(&path)) {
            self.select(row);
            self.focus = Focus::Diff;
        }
        true
    }

    fn sidebar_key(&mut self, code: KeyCode) -> bool {
        let page = page(self.sidebar);
        match code {
            KeyCode::Char('j') | KeyCode::Down => self.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_by(-1),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::Char('g') | KeyCode::Home => self.move_by(isize::MIN),
            KeyCode::Char('G') | KeyCode::End => self.move_by(isize::MAX),
            KeyCode::Enter | KeyCode::Char(' ') => self.toggle(),
            KeyCode::Char('h') | KeyCode::Left => match self.tree.is_open(self.cursor) {
                Some(true) => self.tree.set_open(self.cursor, false),
                _ => self
                    .tree
                    .parent(self.cursor)
                    .map(|row| self.select(row))
                    .is_some(),
            },
            KeyCode::Char('l') | KeyCode::Right => match self.tree.is_open(self.cursor) {
                Some(false) => self.tree.set_open(self.cursor, true),
                Some(true) => self.move_by(1),
                None => false,
            },
            _ => false,
        }
    }

    /// Opens or closes the folder under the cursor; `false` on a file.
    fn toggle(&mut self) -> bool {
        self.tree
            .is_open(self.cursor)
            .is_some_and(|open| self.tree.set_open(self.cursor, !open))
    }

    fn diff_key(&mut self, code: KeyCode, ctrl: bool) -> bool {
        let page = page(self.diff_area);
        let height = usize::from(self.diff_area.height);
        let DiffPane::Loaded(view) = &mut self.diff else {
            return false;
        };
        match code {
            KeyCode::Char('j') | KeyCode::Down => view.scroll_by(1, height),
            KeyCode::Char('k') | KeyCode::Up => view.scroll_by(-1, height),
            KeyCode::Char('d') if ctrl => view.scroll_by(page / 2, height),
            KeyCode::Char('u') if ctrl => view.scroll_by(-page / 2, height),
            KeyCode::PageDown => view.scroll_by(page, height),
            KeyCode::PageUp => view.scroll_by(-page, height),
            KeyCode::Char('g') | KeyCode::Home => view.scroll_by(isize::MIN, height),
            KeyCode::Char('G') | KeyCode::End => view.scroll_by(isize::MAX, height),
            KeyCode::Char(']') => view.next_header(height),
            KeyCode::Char('[') => view.prev_header(height),
            KeyCode::Enter => view.expand_visible_fold(height),
            KeyCode::Char('h') | KeyCode::Left => view.hscroll_by(-H_STEP, height),
            KeyCode::Char('l') | KeyCode::Right => view.hscroll_by(H_STEP, height),
            _ => false,
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent) -> bool {
        let position = Position::new(mouse.column, mouse.row);
        let height = usize::from(self.diff_area.height);
        if let (Some(find), DiffPane::Loaded(view)) = (&mut self.find, &mut self.diff)
            && self.palette.is_none()
            && find.area.contains(position)
        {
            // The bar floats over the diff; clicks on it never reach the rows below.
            let click = mouse.kind == MouseEventKind::Down(MouseButton::Left);
            let Some(toggle) =
                find::at(Toggle::ALL, &find.toggle_areas, position).filter(|_| click)
            else {
                return false;
            };
            toggle.flip(&mut find.query);
            refind(find, view, height);
            return true;
        }
        if let Some(palette) = &self.palette {
            // The palette owns the mouse: clicks pick a file or close it, nothing reaches the panes.
            if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
                return false;
            }
            if !self.palette_area.contains(position) {
                self.palette = None;
                return true;
            }
            let row_at = |area: Rect, offset: usize| {
                area.contains(position)
                    .then(|| offset + usize::from(mouse.row - area.y))
            };
            return match &palette.mode {
                Mode::Line => false,
                Mode::File(files) => {
                    let node = row_at(files.area, files.list.offset())
                        .and_then(|row| files.hits.get(row))
                        .map(|hit| hit.node);
                    node.is_some_and(|node| self.open_file(node))
                }
                Mode::Search(search) => {
                    if let Some(toggle) = find::at(Toggle::ALL, &search.toggle_areas, position) {
                        if let Some(search) = self.current_search(search.generation) {
                            toggle.flip(&mut search.query);
                        }
                        self.search();
                        return true;
                    }
                    let field = find::at(palette::Input::ALL, &search.field_areas, position);
                    if let Some(input) = field {
                        if let Some(search) = self.current_search(search.generation) {
                            search.input = input;
                        }
                        return true;
                    }
                    let target = row_at(search.list_area, search.list.offset())
                        .and_then(|row| search.hit(row));
                    self.open_hit(target)
                }
            };
        }
        if mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && self.sidebar_toggle.contains(position)
        {
            return self.toggle_sidebar();
        }
        // Checked before hit-testing panes so a drag keeps going outside the sidebar.
        let on_divider =
            self.sidebar.contains(position) && mouse.column + 1 == self.sidebar.right();
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if on_divider => {
                self.resizing = true;
                return true;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.resizing => {
                let width = Some(mouse.column.saturating_sub(self.sidebar.x) + 1);
                return mem::replace(&mut self.sidebar_width, width) != width;
            }
            MouseEventKind::Up(MouseButton::Left) if self.resizing => {
                self.resizing = false;
                return true;
            }
            _ => {}
        }
        if self.sidebar.contains(position) {
            self.sidebar_mouse(mouse)
        } else if self.diff_area.contains(position) {
            self.diff_mouse(mouse)
        } else {
            false
        }
    }

    fn sidebar_mouse(&mut self, mouse: MouseEvent) -> bool {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.focus = Focus::Sidebar;
                let row = self.list.offset() + usize::from(mouse.row - self.sidebar.y);
                if row >= self.tree.len() {
                    return false;
                }
                let moved = row != self.cursor;
                self.select(row);
                self.toggle() || moved
            }
            MouseEventKind::ScrollDown => self.move_by(WHEEL_STEP),
            MouseEventKind::ScrollUp => self.move_by(-WHEEL_STEP),
            _ => false,
        }
    }

    fn diff_mouse(&mut self, mouse: MouseEvent) -> bool {
        let height = usize::from(self.diff_area.height);
        let row = usize::from(mouse.row - self.diff_area.y);
        // Terminals that report no horizontal wheel send Shift+wheel for it.
        let shift = mouse.modifiers.contains(KeyModifiers::SHIFT);
        let DiffPane::Loaded(view) = &mut self.diff else {
            return false;
        };
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.focus = Focus::Diff;
                zdiff_core::expand(&mut view.rows, view.scroll + row)
            }
            MouseEventKind::ScrollRight => view.hscroll_by(H_STEP, height),
            MouseEventKind::ScrollLeft => view.hscroll_by(-H_STEP, height),
            MouseEventKind::ScrollDown if shift => view.hscroll_by(H_STEP, height),
            MouseEventKind::ScrollUp if shift => view.hscroll_by(-H_STEP, height),
            MouseEventKind::ScrollDown => view.scroll_by(WHEEL_STEP, height),
            MouseEventKind::ScrollUp => view.scroll_by(-WHEEL_STEP, height),
            _ => false,
        }
    }

    /// Moves the selection by `delta` rows, clamped to the first and last row.
    fn move_by(&mut self, delta: isize) -> bool {
        let Some(last) = self.tree.len().checked_sub(1) else {
            return false;
        };
        let cursor = clamp_add(self.cursor, delta, last);
        let moved = cursor != self.cursor;
        self.select(cursor);
        moved
    }

    fn select(&mut self, row: usize) {
        self.cursor = row;
        self.list.select((row < self.tree.len()).then_some(row));
        if let Some(file) = self.tree.file_at(row) {
            self.file = Some(file);
        }
    }

    fn stop(&mut self) -> bool {
        self.quit = true;
        true
    }
}

impl DiffView {
    fn new(file: FileDiff, language: Option<Language>) -> Self {
        let rows = file.rows(CONTEXT_LINES);
        let gutter = file.old.len().max(file.new.len()).to_string().len();
        // ponytail: highlight + word diff run on the UI thread; use a worker if big files stall.
        let tokens = |text: &Text| {
            language.map_or_else(Box::default, |l| {
                zdiff_highlight::highlight(l, text.bytes())
            })
        };
        Self {
            old_tokens: tokens(&file.old),
            words: file.word_changes(),
            new_tokens: tokens(&file.new),
            file,
            rows,
            scroll: 0,
            hscroll: 0,
            gutter,
            text_width: 0,
            mark: None,
        }
    }

    /// Furthest horizontal scroll: the widest visible line on either side, plus a little slack.
    fn max_hscroll(&self, height: usize) -> usize {
        let width =
            |text: &Text, line: Option<u32>| line.map_or(0, |i| text::columns(text.line(i)));
        let widest = self
            .rows
            .iter()
            .skip(self.scroll)
            .take(height)
            .map(|row| match row {
                Row::Line { old, new, .. } => {
                    width(&self.file.old, *old).max(width(&self.file.new, *new))
                }
                _ => 0,
            })
            .max()
            .unwrap_or(0);
        if widest <= self.text_width {
            0
        } else {
            widest - self.text_width + H_OVERSCROLL
        }
    }

    fn hscroll_by(&mut self, delta: isize, height: usize) -> bool {
        let hscroll = clamp_add(self.hscroll, delta, self.max_hscroll(height));
        let moved = hscroll != self.hscroll;
        self.hscroll = hscroll;
        moved
    }

    /// Largest scroll that still fills a body `height` rows tall.
    pub fn max_scroll(&self, height: usize) -> usize {
        self.rows.len().saturating_sub(height)
    }

    fn scroll_to(&mut self, row: usize, height: usize) -> bool {
        self.mark = None;
        let scroll = row.min(self.max_scroll(height));
        let moved = scroll != self.scroll;
        self.scroll = scroll;
        moved
    }

    fn scroll_by(&mut self, delta: isize, height: usize) -> bool {
        self.scroll_to(clamp_add(self.scroll, delta, usize::MAX), height)
    }

    /// Scrolls one-based `line` on `side` a third of the way down and marks it.
    ///
    /// Numbers past the end go to the last line; `0` goes to the first.
    fn jump(&mut self, side: Side, line: u32, height: usize) {
        let len = match side {
            Side::Old => self.file.old.len(),
            Side::New => self.file.new.len(),
        };
        let Some(last) = len.checked_sub(1) else {
            return;
        };
        let target = line.saturating_sub(1).min(last);
        if let Some(row) = zdiff_core::reveal(&mut self.rows, side, target) {
            self.scroll_to(row.saturating_sub(height / 3), height);
            self.mark = Some(row);
        }
    }

    fn next_header(&mut self, height: usize) -> bool {
        let start = self.scroll + 1;
        match self.rows.iter().skip(start).position(is_header) {
            Some(offset) => self.scroll_to(start + offset, height),
            None => false,
        }
    }

    fn prev_header(&mut self, height: usize) -> bool {
        match self.rows[..self.scroll].iter().rposition(is_header) {
            Some(row) => self.scroll_to(row, height),
            None => false,
        }
    }

    fn expand_visible_fold(&mut self, height: usize) -> bool {
        let visible = self.scroll..(self.scroll + height).min(self.rows.len());
        match self.rows[visible.clone()]
            .iter()
            .position(|r| matches!(r, Row::Fold { .. }))
        {
            Some(offset) => zdiff_core::expand(&mut self.rows, visible.start + offset),
            None => false,
        }
    }
}

fn is_header(row: &Row) -> bool {
    matches!(row, Row::Header { .. })
}

/// Re-runs `find` on `view` and jumps to its current hit, as editors do while you type.
fn refind(find: &mut Find, view: &mut DiffView, height: usize) {
    find.update(view);
    if let Some(hit) = find.current.and_then(|i| find.hits.get(i)) {
        view.jump(hit.side, hit.line + 1, height);
    }
}

fn clamp_add(value: usize, delta: isize, max: usize) -> usize {
    value.saturating_add_signed(delta).min(max)
}

fn page(area: Rect) -> isize {
    isize::try_from(area.height).unwrap_or(1).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::Node;

    fn entry(path: &str) -> FileEntry {
        FileEntry {
            path: path.into(),
            status: Status::Modified,
            added: 1,
            removed: 0,
            change: 0,
        }
    }

    fn app(paths: &[&str]) -> App {
        App::new(paths.iter().copied().map(entry).collect())
    }

    /// App with a loaded diff of 30 numbered lines where line 15 changed.
    fn diff_app() -> App {
        let old: String = (1..=30).map(|i| i.to_string() + "\n").collect();
        let new = old.replace("\n15\n", "\nfifteen\n");
        let mut app = app(&["a.rs"]);
        app.show(Some(Ok(FileDiff::new(old.into(), new.into()))));
        app.diff_area = Rect::new(40, 1, 60, 5);
        app.focus = Focus::Diff;
        app
    }

    fn labels(app: &App) -> Vec<String> {
        app.tree
            .visible()
            .map(|node| match node {
                Node::Dir { label, .. } => format!("{label}/"),
                Node::File { entry, .. } => entry.path.display().to_string(),
            })
            .collect()
    }

    fn press(app: &mut App, code: KeyCode) -> bool {
        app.handle(&Event::Key(code.into()))
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    fn scroll(app: &App) -> usize {
        match &app.diff {
            DiffPane::Loaded(view) => view.scroll,
            _ => panic!("no diff loaded"),
        }
    }

    fn rows(app: &App) -> &[Row] {
        match &app.diff {
            DiffPane::Loaded(view) => &view.rows,
            _ => panic!("no diff loaded"),
        }
    }

    #[test]
    fn lists_files_in_a_folder_tree_and_starts_on_the_first_file() {
        let app = app(&["b/x.rs", "top.rs", "a/z.rs", "a/y.rs"]);
        assert_eq!(
            labels(&app),
            ["a/", "a/y.rs", "a/z.rs", "b/", "b/x.rs", "top.rs"]
        );
        assert_eq!(selected(&app).as_deref(), Some("a/y.rs"));
    }

    #[test]
    fn sidebar_keys_move_over_every_row_and_stop_at_ends() {
        let mut app = app(&["b/x.rs", "top.rs", "a/y.rs", "a/z.rs"]);
        let selected = |app: &mut App, code| {
            press(app, code);
            app.list.selected()
        };
        let down: Vec<_> = (0..4)
            .map(|_| selected(&mut app, KeyCode::Char('j')))
            .collect();
        assert_eq!(down, [Some(2), Some(3), Some(4), Some(5)]);
        assert_eq!(selected(&mut app, KeyCode::Char('k')), Some(4));
        assert_eq!(selected(&mut app, KeyCode::Char('g')), Some(0));
        assert_eq!(selected(&mut app, KeyCode::Char('G')), Some(5));
        press(&mut app, KeyCode::Char('q'));
        assert!(app.quit);
    }

    #[test]
    fn dragging_the_divider_resizes_the_sidebar() {
        let mut app = app(&["a.rs", "b.rs"]);
        app.sidebar = Rect::new(0, 0, 30, 10);
        let left = MouseButton::Left;

        assert!(app.handle(&mouse(MouseEventKind::Down(left), 29, 1)));
        assert!(app.resizing);
        assert!(app.handle(&mouse(MouseEventKind::Drag(left), 44, 3)));
        assert_eq!(app.sidebar_width, Some(45));
        assert!(
            !app.handle(&mouse(MouseEventKind::Drag(left), 44, 4)),
            "same width"
        );
        assert!(app.handle(&mouse(MouseEventKind::Up(left), 44, 4)));
        assert!(!app.resizing);
        assert!(!app.handle(&mouse(MouseEventKind::Drag(left), 60, 4)));
        assert_eq!(app.sidebar_width, Some(45), "drag without grabbing");

        app.handle(&mouse(MouseEventKind::Down(left), 5, 1));
        assert!(!app.resizing, "clicking a row doesn't resize");
        assert_eq!(app.list.selected(), Some(1));
    }

    #[test]
    fn ctrl_b_and_the_footer_label_hide_and_show_the_sidebar() {
        let mut app = app(&["a.rs", "b.rs"]);
        app.sidebar_width = Some(45);
        assert!(ctrl(&mut app, 'b'));
        assert!(app.sidebar_hidden);
        assert_eq!(app.focus, Focus::Diff);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Diff, "Tab skips the hidden sidebar");
        assert!(ctrl(&mut app, 'b'));
        assert!(!app.sidebar_hidden);
        assert_eq!(app.sidebar_width, Some(45), "width kept");

        let click = |column, row| mouse(MouseEventKind::Down(MouseButton::Left), column, row);
        app.sidebar_toggle = Rect::new(0, 9, 9, 1);
        assert!(app.handle(&click(4, 9)));
        assert!(app.sidebar_hidden);
        // A hidden draw leaves the sidebar zero-wide, so its old spot picks nothing.
        app.sidebar = Rect::new(0, 0, 0, 9);
        let before = app.list.selected();
        assert!(!app.handle(&click(5, 2)));
        assert_eq!(app.list.selected(), before);
    }

    #[test]
    fn click_selects_files_and_toggles_folders() {
        let mut app = app(&["a/y.rs", "a/z.rs"]);
        app.sidebar = Rect::new(0, 2, 30, 10);
        let click = |row| mouse(MouseEventKind::Down(MouseButton::Left), 5, row);

        assert!(app.handle(&click(4)));
        assert_eq!(selected(&app).as_deref(), Some("a/z.rs"));
        assert!(app.handle(&click(2)), "folder row closes");
        assert_eq!(labels(&app), ["a/"]);
        assert_eq!(
            selected(&app).as_deref(),
            Some("a/z.rs"),
            "diff keeps its file"
        );
        assert!(!app.handle(&click(3)), "empty space below the rows");
        assert!(!app.handle(&click(40)), "outside the sidebar");
        assert!(app.handle(&click(2)), "folder row opens again");
        assert_eq!(labels(&app).len(), 3);
    }

    #[test]
    fn arrows_close_open_and_walk_the_tree() {
        let mut app = app(&["a/b/x.rs", "a/c/y.rs"]);
        let at = |app: &App| app.list.selected();
        assert_eq!(at(&app), Some(2), "starts on x.rs");
        assert!(press(&mut app, KeyCode::Left));
        assert_eq!(at(&app), Some(1), "file goes to its folder");
        assert!(press(&mut app, KeyCode::Left));
        assert_eq!(app.tree.is_open(1), Some(false), "open folder closes");
        assert!(press(&mut app, KeyCode::Left));
        assert_eq!(at(&app), Some(0), "closed folder goes to its parent");
        assert!(press(&mut app, KeyCode::Left));
        assert_eq!(app.tree.is_open(0), Some(false));
        assert!(press(&mut app, KeyCode::Right));
        assert_eq!(app.tree.is_open(0), Some(true));
        assert!(press(&mut app, KeyCode::Right));
        assert_eq!(at(&app), Some(1), "open folder steps into its first child");
        assert!(press(&mut app, KeyCode::Enter));
        assert_eq!(app.tree.is_open(1), Some(true), "Enter toggles");
        assert_eq!(selected(&app).as_deref(), Some("a/b/x.rs"));
    }

    #[test]
    fn refresh_keeps_closed_folders_closed() {
        let mut app = app(&["a/x.rs", "top.rs"]);
        press(&mut app, KeyCode::Char('g'));
        press(&mut app, KeyCode::Enter);
        app.refresh(
            ["a/x.rs", "a/y.rs", "top.rs"]
                .into_iter()
                .map(entry)
                .collect(),
        );
        assert_eq!(labels(&app), ["a/", "top.rs"]);
        assert_eq!(app.list.selected(), Some(0));
    }

    #[test]
    fn mouse_motion_needs_no_redraw() {
        let mut app = app(&["a.rs"]);
        app.sidebar = Rect::new(0, 0, 30, 10);
        assert!(!app.handle(&mouse(MouseEventKind::Moved, 1, 1)));
    }

    #[test]
    fn empty_list_ignores_navigation() {
        let mut app = app(&[]);
        assert!(!press(&mut app, KeyCode::Char('j')));
        assert!(app.selected_file().is_none());
    }

    fn selected(app: &App) -> Option<String> {
        app.selected_file().map(|f| f.path.display().to_string())
    }

    #[test]
    fn refresh_keeps_selection_by_path() {
        let mut app = app(&["b.rs", "c.rs"]);
        press(&mut app, KeyCode::Char('j'));
        app.focus = Focus::Diff;
        app.refresh(["a.rs", "b.rs", "c.rs"].into_iter().map(entry).collect());
        assert_eq!(selected(&app).as_deref(), Some("c.rs"));
        assert_eq!(app.focus, Focus::Diff);
    }

    #[test]
    fn refresh_selects_neighbour_when_selected_file_is_gone() {
        let mut app = app(&["a.rs", "b.rs", "c.rs"]);
        press(&mut app, KeyCode::Char('G'));
        app.refresh(["a.rs", "b.rs"].into_iter().map(entry).collect());
        assert_eq!(selected(&app).as_deref(), Some("b.rs"));
        app.refresh(Vec::new());
        assert_eq!(selected(&app), None);
    }

    fn ctrl(app: &mut App, c: char) -> bool {
        app.handle(&Event::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::CONTROL,
        )))
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    fn view(app: &App) -> &DiffView {
        match &app.diff {
            DiffPane::Loaded(view) => view,
            _ => panic!("no diff loaded"),
        }
    }

    #[test]
    fn goto_opens_folds_marks_and_clamps() {
        let mut app = diff_app();
        app.focus = Focus::Sidebar;
        assert!(ctrl(&mut app, 'g'));
        assert_eq!(app.focus, Focus::Diff);
        type_text(&mut app, "25");
        assert!(press(&mut app, KeyCode::Enter));
        assert!(app.palette.is_none());
        let (v, row) = (view(&app), view(&app).mark.expect("marked"));
        assert!(
            matches!(v.rows[row], Row::Line { new: Some(24), .. }),
            "inside the last fold"
        );
        assert!((v.scroll..v.scroll + 5).contains(&row), "on screen");

        ctrl(&mut app, 'g');
        type_text(&mut app, "-99999");
        press(&mut app, KeyCode::Enter);
        let row = view(&app).mark.expect("marked");
        assert!(
            matches!(view(&app).rows[row], Row::Line { old: Some(29), .. }),
            "last old line"
        );

        press(&mut app, KeyCode::Char('k'));
        assert_eq!(view(&app).mark, None, "scrolling clears the mark");
    }

    #[test]
    fn goto_modal_owns_input_until_closed() {
        let mut app = diff_app();
        app.palette_area = Rect::new(50, 1, 20, 5);
        let selected = app.list.selected();
        ctrl(&mut app, 'g');
        type_text(&mut app, "4j");
        assert_eq!(
            app.palette.as_ref().map(|p| p.input.as_str()),
            Some("4"),
            "letters are ignored"
        );
        assert_eq!(app.list.selected(), selected);
        assert!(press(&mut app, KeyCode::Esc));
        assert!(
            app.palette.is_none() && !app.quit,
            "Esc closes the modal, not the app"
        );

        ctrl(&mut app, 'g');
        let before = scroll(&app);
        assert!(!app.handle(&mouse(MouseEventKind::ScrollDown, 45, 3)));
        assert_eq!(scroll(&app), before, "wheel doesn't reach the diff");
        let left = MouseEventKind::Down(MouseButton::Left);
        assert!(
            !app.handle(&mouse(left, 55, 2)),
            "click inside keeps it open"
        );
        assert!(app.handle(&mouse(left, 5, 2)));
        assert!(app.palette.is_none(), "click outside closes it");

        let mut empty = App::new(vec![entry("a.rs")]);
        assert!(!ctrl(&mut empty, 'g'), "no diff loaded");
        assert!(empty.palette.is_none());
    }

    fn picked(app: &App) -> Option<String> {
        match &app.palette {
            Some(Palette {
                mode: Mode::File(files),
                ..
            }) => files
                .selected()
                .map(|n| app.tree.file(n).unwrap().path.display().to_string()),
            _ => None,
        }
    }

    #[test]
    fn ctrl_p_finds_and_opens_a_file() {
        let mut app = app(&["src/app.rs", "src/rows.rs", "src/repo.rs", "top.rs"]);
        app.tree.set_open(0, false);
        assert!(ctrl(&mut app, 'p'));
        type_text(&mut app, "r");
        assert!(press(&mut app, KeyCode::Backspace));
        type_text(&mut app, "ro");
        assert_eq!(picked(&app).as_deref(), Some("src/rows.rs"));
        assert!(press(&mut app, KeyCode::Down));
        let second = picked(&app);
        assert!(press(&mut app, KeyCode::Up));
        assert!(press(&mut app, KeyCode::Up), "wraps to the last hit");
        assert_ne!(picked(&app).as_deref(), Some("src/rows.rs"));
        assert!(press(&mut app, KeyCode::Down));
        assert!(press(&mut app, KeyCode::Down));
        assert_eq!(picked(&app), second);

        assert!(press(&mut app, KeyCode::Enter));
        assert!(app.palette.is_none());
        assert_eq!(selected(&app), second, "sidebar follows");
        assert_eq!(app.tree.is_open(0), Some(true), "its folder opened");
        assert_eq!(app.focus, Focus::Diff);
    }

    #[test]
    fn file_palette_closes_switches_and_survives_refresh() {
        let mut app = app(&["a.rs", "b.rs"]);
        app.palette_area = Rect::new(10, 2, 70, 16);
        ctrl(&mut app, 'p');
        type_text(&mut app, "q");
        assert!(press(&mut app, KeyCode::Esc));
        assert!(
            app.palette.is_none() && !app.quit,
            "Esc closes the palette only"
        );

        ctrl(&mut app, 'p');
        press(&mut app, KeyCode::Down);
        app.refresh(["0.rs", "a.rs", "b.rs"].into_iter().map(entry).collect());
        assert_eq!(
            picked(&app).as_deref(),
            Some("b.rs"),
            "selection kept by path"
        );

        let left = MouseEventKind::Down(MouseButton::Left);
        if let Some(Palette {
            mode: Mode::File(files),
            ..
        }) = &mut app.palette
        {
            files.area = Rect::new(11, 5, 68, 10);
        }
        assert!(app.handle(&mouse(left, 20, 6)), "second hit row");
        assert_eq!(selected(&app).as_deref(), Some("a.rs"));
        assert!(app.palette.is_none());

        ctrl(&mut app, 'p');
        assert!(app.handle(&mouse(left, 1, 1)));
        assert!(app.palette.is_none(), "outside click closes it");
    }

    #[test]
    fn ctrl_g_and_ctrl_p_switch_palette_modes() {
        let mut app = diff_app();
        let mode = |app: &App| app.palette.as_ref().map(|p| matches!(p.mode, Mode::Line));
        ctrl(&mut app, 'p');
        assert_eq!(mode(&app), Some(false));
        ctrl(&mut app, 'g');
        assert_eq!(mode(&app), Some(true));
        ctrl(&mut app, 'p');
        assert_eq!(mode(&app), Some(false));
    }

    fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> bool {
        app.handle(&Event::Key(KeyEvent::new(code, modifiers)))
    }

    fn jumped_line(app: &App) -> Option<(Option<u32>, Option<u32>)> {
        let v = view(app);
        match v.rows.get(v.mark?)? {
            Row::Line { old, new, .. } => Some((*old, *new)),
            _ => None,
        }
    }

    #[test]
    fn ctrl_f_finds_as_you_type_and_steps_through_hits() {
        let mut app = diff_app();
        assert!(ctrl(&mut app, 'f'));
        type_text(&mut app, "fif");
        assert_eq!(
            jumped_line(&app),
            Some((Some(14), Some(14))),
            "fifteen, new side"
        );
        assert!(
            press(&mut app, KeyCode::Enter),
            "a single hit wraps to itself"
        );

        // Starting from the top, `3` is first found inside the leading fold.
        let mut app = diff_app();
        ctrl(&mut app, 'f');
        type_text(&mut app, "3");
        assert_eq!(
            jumped_line(&app),
            Some((Some(2), Some(2))),
            "`3` inside the first fold"
        );
        assert!(
            view(&app)
                .rows
                .iter()
                .take(3)
                .all(|r| !matches!(r, Row::Fold { .. }))
        );
        key(&mut app, KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(
            jumped_line(&app),
            Some((Some(29), Some(29))),
            "back wraps to `30`"
        );
        press(&mut app, KeyCode::Down);
        assert_eq!(jumped_line(&app), Some((Some(2), Some(2))));
        press(&mut app, KeyCode::Up);
        assert_eq!(jumped_line(&app), Some((Some(29), Some(29))));
    }

    #[test]
    fn find_toggles_errors_closes_and_survives_reloads() {
        let mut app = diff_app();
        ctrl(&mut app, 'f');
        type_text(&mut app, "(");
        assert!(key(&mut app, KeyCode::Char('r'), KeyModifiers::ALT));
        assert!(app.find.as_ref().unwrap().error, "`(` is a bad regex");
        key(&mut app, KeyCode::Char('r'), KeyModifiers::ALT);
        press(&mut app, KeyCode::Backspace);
        type_text(&mut app, "1");

        let before = scroll(&app);
        assert!(app.handle(&mouse(MouseEventKind::ScrollDown, 45, 3)));
        assert_ne!(scroll(&app), before, "wheel still scrolls the diff");

        let same = FileDiff::new(
            view(&app).file.old.bytes().into(),
            view(&app).file.new.bytes().into(),
        );
        let scrolled = scroll(&app);
        app.reload(Some(Ok(same)));
        assert_eq!(scroll(&app), scrolled, "reload keeps scroll");
        assert!(!app.find.as_ref().unwrap().hits.is_empty(), "query re-ran");
        app.show(Some(Ok(FileDiff::new(b"q\n".to_vec(), b"q1\n".to_vec()))));
        assert_eq!(
            app.find.as_ref().unwrap().hits.len(),
            1,
            "query runs on the new file"
        );

        assert!(press(&mut app, KeyCode::Esc));
        assert!(app.find.is_none() && !app.quit, "Esc closes the bar only");
        let mut empty = App::new(vec![entry("a.rs")]);
        assert!(!ctrl(&mut empty, 'f'), "no diff loaded");
    }

    #[test]
    fn clicking_a_find_toggle_flips_it_and_the_bar_swallows_other_clicks() {
        let mut app = diff_app();
        ctrl(&mut app, 'f');
        type_text(&mut app, "(");
        let find = app.find.as_mut().unwrap();
        find.area = Rect::new(52, 1, 48, 3);
        find.toggle_areas = [
            Rect::new(80, 2, 4, 1),
            Rect::new(85, 2, 4, 1),
            Rect::new(90, 2, 3, 1),
        ];
        let left = MouseEventKind::Down(MouseButton::Left);
        assert!(app.handle(&mouse(left, 86, 2)), "[.*] clicked");
        let find = app.find.as_ref().unwrap();
        assert!(find.query.regex && find.error, "regex on, `(` is invalid");
        assert!(app.handle(&mouse(left, 86, 2)));
        assert!(
            !app.find.as_ref().unwrap().query.regex,
            "clicked again: off"
        );

        let before = scroll(&app);
        assert!(
            !app.handle(&mouse(left, 60, 2)),
            "bar background does nothing"
        );
        assert!(!app.handle(&mouse(MouseEventKind::ScrollDown, 60, 2)));
        assert_eq!(
            scroll(&app),
            before,
            "clicks on the bar never reach the diff"
        );
    }

    fn app_of(paths: &[&str]) -> App {
        let files = paths.iter().enumerate().map(|(change, path)| FileEntry {
            change,
            ..entry(path)
        });
        App::new(files.collect())
    }

    fn ctrl_shift_f(app: &mut App) -> bool {
        key(
            app,
            KeyCode::Char('F'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        )
    }

    fn global(app: &mut App) -> &mut palette::Search {
        match &mut app.palette {
            Some(Palette {
                mode: Mode::Search(search),
                ..
            }) => search,
            _ => panic!("global search not open"),
        }
    }

    fn found(line: u32) -> Found {
        Found {
            side: Side::New,
            line,
            changed: true,
            snippet: b"fifteen".to_vec().into(),
            range: 0..3,
            tokens: Box::default(),
        }
    }

    #[test]
    fn global_search_sends_jobs_and_drops_stale_results() {
        let mut app = app_of(&["a/x.rs", "b/y.rs"]);
        assert!(ctrl_shift_f(&mut app));
        assert!(app.pending_search.is_none(), "empty query sends nothing");
        type_text(&mut app, "q");
        let (first, _, changes) = app.pending_search.take().expect("job");
        assert_eq!(changes, [0, 1]);
        type_text(&mut app, "r");
        let (second, ..) = app.pending_search.take().expect("job");
        assert!(second > first);
        assert!(
            !app.search_found(first, 0, vec![found(1)]),
            "stale batch dropped"
        );
        assert!(app.search_found(second, 1, vec![found(1)]));
        assert!(app.search_done(second));
        assert_eq!(global(&mut app).hits, 1);

        press(&mut app, KeyCode::Tab);
        type_text(&mut app, "a");
        let (_, _, changes) = app.pending_search.take().expect("job");
        assert_eq!(changes, [0], "include narrows the files");
        app.refresh(["a/x.rs", "b/y.rs"].map(entry).into());
        assert!(app.pending_search.is_some(), "refresh reruns the search");
    }

    #[test]
    fn enter_on_a_hit_opens_it_at_the_line_with_the_query_highlighted() {
        let mut app = diff_app();
        ctrl_shift_f(&mut app);
        type_text(&mut app, "fif");
        let generation = global(&mut app).generation;
        let node = app.tree.files().next().map(|(n, _)| n).unwrap();
        app.search_found(generation, 0, vec![found(14)]);
        assert!(press(&mut app, KeyCode::Enter));
        assert!(app.palette.is_none());
        assert_eq!(
            app.find.as_ref().map(|f| f.query.text.as_str()),
            Some("fif")
        );
        assert_eq!(
            jumped_line(&app),
            Some((Some(14), Some(14))),
            "same file: jumped now"
        );
        assert_eq!(app.file, Some(node));

        // Another file: the jump waits for its diff to load.
        let mut app = app_of(&["a.rs", "b.rs"]);
        ctrl_shift_f(&mut app);
        type_text(&mut app, "fif");
        let generation = global(&mut app).generation;
        app.search_found(generation, 1, vec![found(14)]);
        press(&mut app, KeyCode::Enter);
        assert_eq!(selected(&app).as_deref(), Some("b.rs"));
        let old: String = (1..=30).map(|i| i.to_string() + "\n").collect();
        let new = old.replace("\n15\n", "\nfifteen\n");
        app.diff_area = Rect::new(40, 1, 60, 5);
        app.show(Some(Ok(FileDiff::new(old.into(), new.into()))));
        assert_eq!(jumped_line(&app), Some((Some(14), Some(14))));
    }

    #[test]
    fn ctrl_f_widens_find_and_searches_sidebar_folders() {
        let mut app = diff_app();
        ctrl(&mut app, 'f');
        type_text(&mut app, "ab");
        assert!(ctrl(&mut app, 'f'), "second Ctrl+F widens");
        assert!(app.find.is_none());
        assert_eq!(global(&mut app).query.text, "ab");

        let mut app = app_of(&["src/a.rs", "top.rs"]);
        press(&mut app, KeyCode::Char('g'));
        assert!(ctrl(&mut app, 'f'), "on the src folder row");
        assert_eq!(global(&mut app).include, "src");
    }

    #[test]
    fn global_search_clicks_toggle_open_and_close() {
        let mut app = app_of(&["a.rs", "b.rs"]);
        app.palette_area = Rect::new(5, 2, 90, 26);
        ctrl_shift_f(&mut app);
        type_text(&mut app, "q");
        let generation = global(&mut app).generation;
        app.search_found(generation, 1, vec![found(3)]);
        let search = global(&mut app);
        search.toggle_areas = [
            Rect::new(70, 3, 4, 1),
            Rect::new(75, 3, 4, 1),
            Rect::new(80, 3, 3, 1),
        ];
        search.list_area = Rect::new(6, 7, 88, 15);
        search.field_areas = [
            Rect::new(6, 3, 60, 1),
            Rect::new(6, 4, 88, 1),
            Rect::new(6, 5, 88, 1),
        ];
        let left = MouseEventKind::Down(MouseButton::Left);
        app.pending_search = None;
        assert!(app.handle(&mouse(left, 30, 5)), "exclude field");
        assert_eq!(global(&mut app).input, palette::Input::Exclude);
        assert!(app.pending_search.is_none(), "focus alone does not search");
        assert!(app.handle(&mouse(left, 76, 3)), "[.*]");
        assert!(global(&mut app).query.regex);
        assert!(
            app.pending_search.take().is_some(),
            "toggling searches again"
        );
        let generation = global(&mut app).generation;
        app.search_found(generation, 1, vec![found(3)]);
        assert!(
            app.handle(&mouse(left, 20, 8)),
            "the hit row under the file row"
        );
        assert_eq!(selected(&app).as_deref(), Some("b.rs"));

        ctrl_shift_f(&mut app);
        assert!(app.handle(&mouse(left, 1, 1)));
        assert!(app.palette.is_none(), "outside click closes");
    }

    #[test]
    fn reload_keeps_scroll() {
        let mut app = diff_app();
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('j'));
        let DiffPane::Loaded(view) = &app.diff else {
            panic!("no diff loaded")
        };
        let same = FileDiff::new(view.file.old.bytes().into(), view.file.new.bytes().into());
        app.reload(Some(Ok(same)));
        assert_eq!(scroll(&app), 2);
    }

    /// App showing one changed line: 30 columns on the old side, 50 on the new side.
    fn wide_app() -> App {
        let mut app = app(&["a.rs"]);
        let (old, new) = ("o".repeat(30) + "\n", "n".repeat(50) + "\n");
        app.show(Some(Ok(FileDiff::new(old.into(), new.into()))));
        app.diff_area = Rect::new(40, 1, 60, 5);
        app.focus = Focus::Diff;
        if let DiffPane::Loaded(view) = &mut app.diff {
            view.text_width = 20;
        }
        app
    }

    fn hscroll(app: &App) -> usize {
        match &app.diff {
            DiffPane::Loaded(view) => view.hscroll,
            _ => panic!("no diff loaded"),
        }
    }

    #[test]
    fn horizontal_scroll_is_capped_by_the_wider_side() {
        let mut app = wide_app();
        for _ in 0..20 {
            press(&mut app, KeyCode::Char('l'));
        }
        assert_eq!(
            hscroll(&app),
            50 - 20 + H_OVERSCROLL,
            "new side is the widest"
        );
        press(&mut app, KeyCode::Left);
        assert_eq!(hscroll(&app), 50 - 20 + H_OVERSCROLL - 4);
        for _ in 0..20 {
            press(&mut app, KeyCode::Char('h'));
        }
        assert_eq!(hscroll(&app), 0);
    }

    #[test]
    fn trackpad_and_shift_wheel_scroll_horizontally() {
        let mut app = wide_app();
        assert!(app.handle(&mouse(MouseEventKind::ScrollRight, 50, 2)));
        assert_eq!(hscroll(&app), 4);
        let shift_wheel = Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 50,
            row: 2,
            modifiers: KeyModifiers::SHIFT,
        });
        assert!(app.handle(&shift_wheel));
        assert_eq!((hscroll(&app), scroll(&app)), (8, 0));
        assert!(app.handle(&mouse(MouseEventKind::ScrollLeft, 50, 2)));
        assert_eq!(hscroll(&app), 4);
    }

    #[test]
    fn lines_that_fit_do_not_scroll_sideways() {
        let mut app = diff_app();
        if let DiffPane::Loaded(view) = &mut app.diff {
            view.text_width = 20;
        }
        assert!(!press(&mut app, KeyCode::Char('l')));
    }

    #[test]
    fn new_file_resets_and_reload_keeps_horizontal_scroll() {
        let mut app = wide_app();
        press(&mut app, KeyCode::Char('l'));
        let DiffPane::Loaded(view) = &app.diff else {
            panic!("no diff loaded")
        };
        let same = FileDiff::new(view.file.old.bytes().into(), view.file.new.bytes().into());
        app.reload(Some(Ok(same)));
        assert_eq!(hscroll(&app), 4);
        app.show(Some(Ok(FileDiff::new(b"a\n".to_vec(), b"b\n".to_vec()))));
        assert_eq!(hscroll(&app), 0);
    }

    #[test]
    fn tab_switches_which_pane_keys_drive() {
        let mut app = diff_app();
        app.focus = Focus::Sidebar;
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Diff);
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(scroll(&app), 1);
    }

    #[test]
    fn diff_scroll_clamps_to_last_screen() {
        let mut app = diff_app();
        // Fold, header, 3 context, change, 3 context, fold.
        assert_eq!(rows(&app).len(), 10);
        assert!(press(&mut app, KeyCode::Char('G')));
        assert_eq!(scroll(&app), 10 - 5);
        assert!(
            !press(&mut app, KeyCode::Char('j')),
            "already at the bottom"
        );
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(scroll(&app), 0);
    }

    #[test]
    fn brackets_jump_between_headers() {
        let mut app = diff_app();
        assert!(press(&mut app, KeyCode::Char(']')));
        assert_eq!(scroll(&app), 1);
        assert!(!press(&mut app, KeyCode::Char(']')), "no later header");
        assert!(!press(&mut app, KeyCode::Char('[')), "no earlier header");
    }

    #[test]
    fn click_on_fold_expands_it() {
        let mut app = diff_app();
        let click = mouse(MouseEventKind::Down(MouseButton::Left), 50, 1);
        assert!(app.handle(&click));
        assert_eq!(rows(&app).len(), 10 - 1 + 11);
        assert!(!app.handle(&click), "row 0 is now a plain line");
    }

    #[test]
    fn wheel_scrolls_the_pane_under_the_pointer() {
        let mut app = diff_app();
        app.sidebar = Rect::new(0, 0, 30, 10);
        app.focus = Focus::Sidebar;
        assert!(app.handle(&mouse(MouseEventKind::ScrollDown, 50, 2)));
        assert_eq!(scroll(&app), 3);
    }
}

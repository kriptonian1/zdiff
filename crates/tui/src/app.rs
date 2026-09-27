use std::mem;
use std::path::PathBuf;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::ListState;
use zdiff_core::{Error, FileDiff, Row, Status, Text, WordChanges};
use zdiff_highlight::{Language, Token};

use crate::text;
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
}

#[derive(Debug)]
pub enum DiffPane {
    Empty,
    Failed(Box<str>),
    Loaded(DiffView),
}

#[derive(Debug)]
pub struct App {
    pub tree: Tree,
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
    /// Whether the sidebar divider is being dragged.
    pub resizing: bool,
    pub quit: bool,
}

impl App {
    pub fn new(files: Vec<FileEntry>) -> Self {
        let mut app = Self {
            tree: Tree::default(),
            cursor: 0,
            file: None,
            list: ListState::default(),
            focus: Focus::Sidebar,
            diff: DiffPane::Empty,
            sidebar: Rect::default(),
            diff_area: Rect::default(),
            sidebar_width: None,
            resizing: false,
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
        self.set_files(files);
        self.file = file_path.and_then(|path| self.tree.find_file(&path));
        let same = cursor_path.and_then(|path| self.tree.find(&path));
        let last = self.tree.len().saturating_sub(1);
        self.select(same.unwrap_or(self.cursor.min(last)));
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
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.stop(),
            // Raw mode turns Ctrl+C into a key press instead of SIGINT.
            KeyCode::Char('c') if ctrl => self.stop(),
            KeyCode::Tab => {
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
        let scroll = row.min(self.max_scroll(height));
        let moved = scroll != self.scroll;
        self.scroll = scroll;
        moved
    }

    fn scroll_by(&mut self, delta: isize, height: usize) -> bool {
        self.scroll_to(clamp_add(self.scroll, delta, usize::MAX), height)
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

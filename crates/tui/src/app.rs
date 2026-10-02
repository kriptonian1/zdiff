use std::collections::HashMap;
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::ListState;
use ratatui_image::picker::Picker;
use resvg::usvg::fontdb::Database;
use serde::{Deserialize, Serialize};
use zdiff_core::{Commit, Error, FileDiff, Row, Side, Staged, Status, Text, WordChanges};
use zdiff_highlight::{Language, Token};
use zdiff_search::{Finder, Query};

use crate::find::{self, Find, Toggle};
use crate::history::{self, Button, History, Pane, Want};
use crate::input::{Edit, Field};
use crate::keymap::{Chord, Context, Keymap};
use crate::menu::{self, Action, MENUS, Menu};
use crate::palette::{self, Capture, Mode, Palette};
use crate::preview::{self, Compare, Preview, SvgView};
use crate::search::Found;
use crate::settings::{General, Layout, Settings};
use crate::stream::{Body, Stream};
use crate::text;
use crate::tree::Node;
use crate::tree::Tree;
use crate::ui::Theme;

/// Rows moved per scroll-wheel tick; the usual step in terminal apps.
const WHEEL_STEP: isize = 3;
/// Unchanged lines shown around each change; git's default.
pub(crate) const CONTEXT_LINES: u32 = 3;
/// Percent the swipe divider or onion opacity moves per key press.
const MIX_STEP: u8 = 10;
/// Columns moved per horizontal key press or scroll tick.
const H_STEP: isize = 4;
/// Columns you can scroll past the end of the widest line, so its last character isn't flush with the edge.
const H_OVERSCROLL: usize = 4;
/// Narrowest diff pane that still shows split view when no view was picked.
const SPLIT_MIN_WIDTH: u16 = 90;

/// Characters of copied text the toast quotes.
const SNIPPET_LEN: usize = 24;

/// `"first words…"`, or `"…" (3 lines)` for several: what a copy toast quotes.
fn snippet(text: &str) -> String {
    let lines = text.lines().count();
    let first = text.lines().next().unwrap_or_default();
    let quoted: String = first.chars().take(SNIPPET_LEN).collect();
    let cut = if quoted.len() < first.len() || lines > 1 {
        "…"
    } else {
        ""
    };
    if lines > 1 {
        format!("\"{quoted}{cut}\" ({lines} lines)")
    } else {
        format!("\"{quoted}{cut}\"")
    }
}

/// How long a toast stays up.
const TOAST_FOR: Duration = Duration::from_secs(2);

/// A short confirmation drawn over everything, gone after [`TOAST_FOR`].
#[derive(Debug)]
pub struct Toast {
    pub text: String,
    pub until: Instant,
}

impl Toast {
    pub fn new(text: String) -> Self {
        Self {
            text,
            until: Instant::now() + TOAST_FOR,
        }
    }
}

/// A commit shown in the main view instead of the working tree.
#[derive(Debug)]
pub struct Viewing {
    /// `a1b2c3d^ → a1b2c3d`, for the footer.
    pub label: String,
    /// The file shown before, to select again when going back.
    pub back: Option<PathBuf>,
}

/// How the diff pane lays out a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum View {
    #[default]
    Split,
    Unified,
}

/// Whether the diff pane shows one file or every changed file stacked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    File,
    All,
}

/// The view to show in a `width`-column pane: the picked one, else split when it fits.
/// Column of a sidebar row's checkbox, after the selection bar, a space, and the indent.
pub(crate) fn checkbox_column(depth: usize) -> usize {
    2 + 2 * depth
}

/// An empty commit message box, with its hint.
fn commit_field() -> Field {
    let mut field = Field::multi("");
    field.set_placeholder("i to write · Ctrl+Enter commits");
    field
}

/// Whether the index already holds what a checkbox set to `want` asks for.
fn matches_index(staged: Staged, want: bool) -> bool {
    staged == if want { Staged::Fully } else { Staged::No }
}

fn view_for(width: u16, choice: Option<View>) -> View {
    choice.unwrap_or(if width < SPLIT_MIN_WIDTH {
        View::Unified
    } else {
        View::Split
    })
}

/// The sidebar's drag handle column (its inner edge, next to the diff) and the width a drag
/// to `column` gives, for a sidebar at `area` on the left or the right.
fn handle(area: Rect, right: bool, column: u16) -> (u16, u16) {
    if right {
        (area.x, area.right().saturating_sub(column))
    } else {
        (
            area.right().saturating_sub(1),
            column.saturating_sub(area.x) + 1,
        )
    }
}

/// Context lines added or removed by one More/Less context step.
const CONTEXT_STEP: u32 = 3;

/// What a file's rows are built for; they're rebuilt when it changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    pub view: View,
    /// Unchanged lines around each change; `u32::MAX` shows every line.
    pub context: u32,
}

/// Context lines after one More/Less step: 3 at a time, down to 0; Less from "all" is the default.
fn next_context(context: u32, more: bool) -> u32 {
    match (context, more) {
        (u32::MAX, false) => CONTEXT_LINES,
        (context, true) => context.saturating_add(CONTEXT_STEP),
        (context, false) => context.saturating_sub(CONTEXT_STEP),
    }
}

/// Display rows of `file` for `shape`.
fn build_rows(file: &FileDiff, shape: Shape) -> Vec<Row> {
    let rows = file.rows(shape.context);
    match shape.view {
        View::Split => rows,
        View::Unified => zdiff_core::unified(&rows),
    }
}

#[derive(Debug)]
pub struct FileEntry {
    pub path: PathBuf,
    pub status: Status,
    pub added: u32,
    pub removed: u32,
    /// Index of this file's `Change` in the list the app was started with.
    pub change: usize,
    /// How much of the change is in the index.
    pub staged: Staged,
}

/// A one-line footer message, cleared by the next key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Error(String),
    /// Something finished, like a commit.
    Done(String),
}

/// Git work the event loop runs: index changes, then an optional commit.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Apply {
    pub stage: Vec<PathBuf>,
    pub unstage: Vec<PathBuf>,
    pub commit: Option<String>,
}

/// A sidebar checkbox: whether the file will be in the index after Stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick {
    Off,
    /// Partly staged, or a folder with some files on.
    Part,
    On,
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
    /// Sizes, and pictures once [`App::show`] decodes them, when the file is an image.
    pub preview: Option<Preview>,
    /// What `rows` were built for.
    shape: Shape,
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
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent UI flags, not one state machine"
)]
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
    /// Whether the sidebar sits right of the diff instead of left.
    pub sidebar_right: bool,
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
    /// What `--focus` shows, as `src/app.rs` or `3 paths`; `None` without it.
    pub only: Option<String>,
    /// The history popup while it is open.
    pub history: Option<Box<History>>,
    /// History work for the event loop, which holds the repository.
    pub pending_history: Vec<Want>,
    /// The commit the main view shows instead of the working tree.
    pub viewing: Option<Viewing>,
    /// The footer's back-to-worktree button from the last draw, for clicks.
    pub back_button: Rect,
    /// Viewing a patch file, which has no history.
    pub patch_mode: bool,
    /// A confirmation such as `Copied a1b2c3d`; the event loop clears it when it expires.
    pub toast: Option<Toast>,
    /// How the terminal draws images; `None` when it can't, so previews are text only.
    pub images: Option<Picker>,
    /// Whether changed images are drawn as pictures; a setting.
    pub image_previews: bool,
    /// How a changed image's sides are compared, and the swipe or onion percent.
    pub image_compare: Compare,
    pub mix: u8,
    /// The compare tabs (in [`Compare::ALL`] order) and the drawn picture, for clicks.
    pub compare_tabs: [Rect; 3],
    pub picture_area: Rect,
    /// Whether SVGs show as code or pictures, for the whole session.
    pub svg_view: SvgView,
    /// The header's Code and Preview tabs (in [`SvgView::ALL`] order) from the last draw.
    pub svg_tabs: [Rect; 2],
    /// Whether SVGs open as Preview; a setting, which `svg_view` starts from.
    pub svg_preview_default: bool,
    /// System fonts for SVG `<text>`, loaded the first time one is drawn.
    pub fonts: Option<Arc<Database>>,
    pub menu: Menu,
    /// The view the shown rows are built for.
    pub view: View,
    /// The view picked with `v` or the View menu; `None` follows the pane width.
    pub view_choice: Option<View>,
    /// Unchanged lines shown around each change; `u32::MAX` expands every fold.
    pub context: u32,
    /// Whether changed words get the stronger highlight.
    pub word_highlights: bool,
    /// Set by Reload; the event loop takes it and reloads the file list.
    pub pending_reload: bool,
    /// Every changed file stacked, while the All files view is on.
    pub stream: Option<Stream>,
    pub keymap: Keymap,
    /// Whether diffs get syntax colors.
    pub syntax: bool,
    /// The saved Staging preference; see [`App::can_stage`].
    pub staging: bool,
    /// From `--read-only`: staging is off for this run, whatever the preference says.
    pub read_only: bool,
    /// Whether the next start watches for changes without `--watch`.
    pub watch_default: bool,
    pub theme: Theme,
    /// The layout zdiff starts with, from Save layout as default.
    pub saved_layout: Option<Layout>,
    /// Set by a settings change; the event loop takes it and writes the settings file.
    pub pending_save: bool,
    /// A one-line message for the footer, such as a settings warning; cleared by the next key.
    pub notice: Option<Notice>,
    /// Checkboxes flipped away from the index: `true` stages the file, `false` unstages it.
    pub marks: HashMap<PathBuf, bool>,
    /// Set by Stage and Commit; the event loop runs it and reports back through `finish`.
    pub pending_apply: Option<Apply>,
    /// The file list inside the sidebar, above the commit panel, from the last draw.
    pub sidebar_rows: Rect,
    /// The Stage button from the last draw, for clicks.
    pub stage_button: Rect,
    /// Set by a cut or copy; the event loop puts it on the system clipboard.
    pub pending_copy: Option<String>,
    pub commit_msg: Field,
    /// Typed keys go to the commit message.
    pub writing: bool,
    /// The commit message box and the Commit button from the last draw, for clicks.
    pub commit_box: Rect,
    pub commit_button: Rect,
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
            sidebar_right: false,
            sidebar_toggle: Rect::default(),
            resizing: false,
            find: None,
            pending_search: None,
            pending_jump: None,
            palette: None,
            palette_area: Rect::default(),
            quit: false,
            only: None,
            history: None,
            pending_history: Vec::new(),
            viewing: None,
            back_button: Rect::default(),
            patch_mode: false,
            toast: None,
            images: None,
            image_previews: true,
            image_compare: Compare::default(),
            mix: 50,
            compare_tabs: [Rect::default(); 3],
            picture_area: Rect::default(),
            svg_view: SvgView::default(),
            svg_tabs: [Rect::default(); 2],
            svg_preview_default: false,
            fonts: None,
            menu: Menu::default(),
            view: View::default(),
            view_choice: None,
            context: CONTEXT_LINES,
            word_highlights: true,
            pending_reload: false,
            stream: None,
            keymap: Keymap::defaults(),
            syntax: true,
            staging: true,
            read_only: false,
            watch_default: false,
            theme: Theme::default(),
            saved_layout: None,
            pending_save: false,
            notice: None,
            marks: HashMap::new(),
            pending_apply: None,
            sidebar_rows: Rect::default(),
            stage_button: Rect::default(),
            pending_copy: None,
            commit_msg: commit_field(),
            writing: false,
            commit_box: Rect::default(),
            commit_button: Rect::default(),
        };
        app.set_files(files);
        let first_file = (0..app.tree.len()).find(|&row| app.tree.file_at(row).is_some());
        app.select(first_file.unwrap_or(0));
        app
    }

    /// Swaps in a new file list, keeping the selection, shown file, and closed folders by path.
    pub fn refresh(&mut self, files: Vec<FileEntry>) {
        let stream_top = (self.stream.as_ref())
            .and_then(Stream::top_node)
            .and_then(|node| self.tree.file(node))
            .map(|f| f.path.clone());
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
            field,
            mode: Mode::File(files),
        }) = &mut self.palette
        {
            files.update(&field.text(), &self.tree);
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
        // ponytail: drops every loaded section; keep unchanged ones if reloads get slow.
        if self.stream.is_some() {
            let height = usize::from(self.diff_area.height);
            let mut stream = Stream::new(&self.tree, self.shape());
            if let Some(node) = stream_top.and_then(|path| self.tree.find_file(&path)) {
                stream.jump(node, height);
            }
            self.stream = Some(stream);
        }
        // Tree nodes and change indices changed, so run an open global search again.
        self.search();
        // A mark stays only while its file is listed and the index still differs from it.
        let tree = &self.tree;
        self.marks.retain(|path, want| {
            (tree.files()).any(|(_, f)| &f.path == path && !matches_index(f.staged, *want))
        });
    }

    /// The checkbox for `entry`: its mark, else what the index holds.
    pub fn tick(&self, entry: &FileEntry) -> Tick {
        match (self.marks.get(&entry.path), entry.staged) {
            (Some(true), _) | (None, Staged::Fully) => Tick::On,
            (Some(false), _) | (None, Staged::No) => Tick::Off,
            (None, Staged::Partly) => Tick::Part,
        }
    }

    /// The checkbox at sidebar `row`: a file's own, or a folder's from every file inside.
    pub fn row_tick(&self, row: usize) -> Tick {
        let mut ticks = self.tree.files_in(row).map(|f| self.tick(f));
        let Some(first) = ticks.next() else {
            return Tick::Off;
        };
        if ticks.all(|t| t == first) {
            first
        } else {
            Tick::Part
        }
    }

    /// Whether any file at sidebar `row` has an unapplied checkbox change.
    pub fn row_pending(&self, row: usize) -> bool {
        (self.tree.files_in(row)).any(|f| self.marks.contains_key(&f.path))
    }

    /// Whether staging controls show and work: the preference, unless running read-only.
    pub fn can_stage(&self) -> bool {
        self.staging && !self.read_only && self.viewing.is_none()
    }

    /// Ticks every file at `row`, or unticks them when all are already on; with staging off,
    /// opens or closes the folder instead, as Space did before staging.
    fn toggle_stage(&mut self, row: usize) -> bool {
        if !self.can_stage() {
            return self.toggle();
        }
        let want = self.row_tick(row) != Tick::On;
        let files: Vec<_> = (self.tree.files_in(row))
            .map(|f| (f.path.clone(), f.staged))
            .collect();
        for (path, staged) in &files {
            if matches_index(*staged, want) {
                self.marks.remove(path);
            } else {
                self.marks.insert(path.clone(), want);
            }
        }
        !files.is_empty()
    }

    /// The marked files to stage and to unstage, in sidebar order; no commit.
    pub fn pending(&self) -> Apply {
        let mut work = Apply::default();
        for (_, file) in self.tree.files() {
            match self.marks.get(&file.path) {
                Some(true) => work.stage.push(file.path.clone()),
                Some(false) => work.unstage.push(file.path.clone()),
                None => {}
            }
        }
        work
    }

    /// Hands the pending ticks and the message to the event loop; a blank message opens the box.
    fn commit(&mut self) -> bool {
        if !self.can_stage() {
            return false;
        }
        if self.commit_msg.is_blank() {
            self.notice = Some(Notice::Error("write a commit message first".into()));
            self.writing = true;
            return true;
        }
        self.pending_apply = Some(Apply {
            commit: Some(self.commit_msg.text()),
            ..self.pending()
        });
        true
    }

    /// What the event loop's git work did: the commit's short id, if it committed.
    pub fn finish(&mut self, result: Result<Option<String>, String>) {
        match result {
            Ok(committed) => {
                self.marks.clear();
                self.pending_reload = true;
                if let Some(id) = committed {
                    self.commit_msg = commit_field();
                    self.writing = false;
                    self.notice = Some(Notice::Done(format!("committed {id}")));
                }
            }
            Err(e) => self.notice = Some(Notice::Error(e)),
        }
    }

    /// Edits the commit message like a text field; Esc stops writing and keeps it.
    fn commit_key(&mut self, key: KeyEvent) -> bool {
        key.code == KeyCode::Esc && mem::take(&mut self.writing)
    }

    /// Offers `key` to the focused text field before any binding sees it; `None` when the
    /// field leaves it to the caller, like Enter, Up, or Ctrl+K.
    fn field_key(&mut self, key: KeyEvent) -> Option<bool> {
        let plain = (key.modifiers - KeyModifiers::SHIFT).is_empty();
        if let Some(Palette { field, mode }) = &self.palette {
            match (mode, key.code) {
                // Delete resets the selected shortcut there.
                (Mode::Keys(_), KeyCode::Delete) => return None,
                // A line number: digits, and one sign at the very start.
                (Mode::Line, KeyCode::Char(c))
                    if plain
                        && !(c.is_ascii_digit()
                            || matches!(c, '+' | '-')
                                && field.column() == 0
                                && !field.text().starts_with(['+', '-'])) =>
                {
                    return Some(false);
                }
                _ => {}
            }
        }
        self.with_field(|field| field.key(key))
    }

    /// Pastes into the focused text field; a line number keeps only its digits.
    fn paste(&mut self, text: &str) -> bool {
        if self.history.as_ref().is_some_and(|h| h.typing) {
            return self.history_search_edit(|field| field.paste(text));
        }
        let digits: String;
        let text = if matches!(
            &self.palette,
            Some(Palette {
                mode: Mode::Line,
                ..
            })
        ) {
            digits = text.chars().filter(char::is_ascii_digit).collect();
            &digits
        } else {
            text
        };
        self.with_field(|field| field.paste(text)).unwrap_or(false)
    }

    /// Runs `act` on the focused field (a palette's, then the find bar's, then the commit
    /// message), then reacts like typing does: filters, searches, and the clipboard.
    fn with_field(&mut self, act: impl FnOnce(&mut Field) -> Edit) -> Option<bool> {
        let mut research = false;
        let (edit, copied) = if let Some(Palette { field, mode }) = &mut self.palette {
            let field = match mode {
                Mode::Search(search) => search.field_mut(),
                _ => field,
            };
            let edit = act(field);
            let copied = field.take_copied();
            if edit == Edit::Changed {
                let text = field.text();
                match mode {
                    Mode::File(files) => files.update(&text, &self.tree),
                    Mode::Keys(keys) => keys.filter(&text),
                    Mode::Themes(themes) => themes.filter(&text),
                    Mode::Search(search) => {
                        search.sync();
                        research = true;
                    }
                    Mode::Line => {}
                }
            }
            (edit, copied)
        } else if let Some(find) = &mut self.find {
            let edit = act(&mut find.field);
            if edit == Edit::Changed {
                find.sync();
                if let DiffPane::Loaded(view) = &mut self.diff {
                    refind(find, view, usize::from(self.diff_area.height));
                }
            }
            (edit, find.field.take_copied())
        } else if self.writing {
            (act(&mut self.commit_msg), self.commit_msg.take_copied())
        } else {
            return None;
        };
        if research {
            self.search();
        }
        if let Some(text) = copied {
            let label = snippet(&text);
            self.copy(text, &label);
        }
        (edit != Edit::Ignored).then_some(true)
    }

    /// Switches an SVG between code and pictures, or cycles how an image's sides compare.
    fn picture_action(&mut self, action: Action) -> bool {
        match action {
            Action::SvgView => self.set_svg_view(match self.svg_view {
                SvgView::Code => SvgView::Preview,
                SvgView::Preview => SvgView::Code,
            }),
            Action::ImageCompare if self.comparable() => {
                self.image_compare = self.image_compare.next();
                true
            }
            _ => false,
        }
    }

    /// Settings menu items that change what the settings file holds.
    fn settings_action(&mut self, action: Action) -> bool {
        match action {
            Action::SaveLayout => self.saved_layout = Some(self.layout()),
            Action::ResetLayout => {
                self.saved_layout = None;
                self.set_layout(&Layout::default());
            }
            Action::Syntax => self.syntax = !self.syntax,
            Action::WatchAtStart => self.watch_default = !self.watch_default,
            Action::ImagePreviews => {
                self.image_previews = !self.image_previews;
                self.reload_preview();
            }
            Action::SvgPreviewDefault => {
                self.svg_preview_default = !self.svg_preview_default;
                let view = svg_view_for(self.svg_preview_default);
                if !self.set_svg_view(view) {
                    self.svg_view = view;
                }
            }
            Action::Staging if self.read_only => {
                let why = "read-only: restart without --read-only to stage";
                self.notice = Some(Notice::Error(why.into()));
                return true;
            }
            Action::Staging => {
                self.staging = !self.staging;
                if !self.staging {
                    // Hidden ticks would stage by surprise later; the message is kept.
                    self.marks.clear();
                    self.writing = false;
                }
            }
            _ => return false,
        }
        self.save_settings()
    }

    /// Hands the pending checkbox changes to the event loop; `false` when there are none.
    fn stage(&mut self) -> bool {
        if !self.can_stage() || self.marks.is_empty() {
            return false;
        }
        self.pending_apply = Some(self.pending());
        true
    }

    fn set_files(&mut self, files: Vec<FileEntry>) {
        self.tree = Tree::new(files, mem::take(&mut self.tree).into_collapsed());
    }

    /// The file shown in the diff pane: the last file the sidebar cursor was on.
    pub fn selected_file(&self) -> Option<&FileEntry> {
        self.tree.file(self.file?)
    }

    /// The file the single-file view should load; `None` in the All files view.
    pub fn single_file(&self) -> Option<&FileEntry> {
        self.stream
            .is_none()
            .then(|| self.selected_file())
            .flatten()
    }

    pub fn scope(&self) -> Scope {
        if self.stream.is_some() {
            Scope::All
        } else {
            Scope::File
        }
    }

    /// Switches between one file and every file stacked; each keeps the file you're on.
    fn toggle_scope(&mut self) -> bool {
        if let Some(stream) = self.stream.take() {
            self.file = stream.top_node().or(self.file);
        } else {
            let mut stream = Stream::new(&self.tree, self.shape());
            if let Some(node) = self.file {
                stream.jump(node, usize::from(self.diff_area.height));
            }
            self.stream = Some(stream);
            self.find = None;
            self.diff = DiffPane::Empty;
        }
        true
    }

    /// `(section, change)` pairs the All files view needs loaded before the next draw.
    pub fn stream_wants(&mut self) -> Vec<(usize, usize)> {
        let height = usize::from(self.diff_area.height);
        self.stream
            .as_mut()
            .map_or_else(Vec::new, |stream| stream.wanted(height))
    }

    /// Stores a loaded diff in its All files section.
    pub fn stream_loaded(&mut self, section: usize, diff: Result<FileDiff, Error>) {
        let shape = self.shape();
        let Some(node) = (self.stream.as_ref()).map(|stream| stream.sections[section].node) else {
            return;
        };
        let language = self.language(node);
        let Some(stream) = &mut self.stream else {
            return;
        };
        let body = match diff {
            Ok(file) => Body::Loaded(Box::new(DiffView::new(file, language, shape))),
            Err(e) => Body::Failed(e.to_string().into()),
        };
        stream.loaded(section, body);
    }

    /// Points the sidebar at the file on top of the All files view, without jumping back.
    pub fn sync_top(&mut self) {
        let Some(node) = self.stream.as_ref().and_then(Stream::top_node) else {
            return;
        };
        if self.file == Some(node) {
            return;
        }
        self.file = Some(node);
        let row = (self.tree.file(node)).and_then(|f| self.tree.find(&f.path));
        if let Some(row) = row {
            self.cursor = row;
            self.list.select(Some(row));
        }
    }

    /// Replaces the diff pane with the selected file's diff, scrolled to the top.
    pub fn show(&mut self, diff: Option<Result<FileDiff, Error>>) {
        self.diff = match diff {
            None => DiffPane::Empty,
            Some(Ok(file)) => {
                let language = self.file.and_then(|node| self.language(node));
                DiffPane::Loaded(DiffView::new(file, language, self.shape()))
            }
            Some(Err(e)) => DiffPane::Failed(e.to_string().into()),
        };
        self.reload_preview();
        self.sync_view();
    }

    /// Rebuilds the shown image's preview: pictures when on and drawable, else sizes only.
    fn reload_preview(&mut self) {
        let picker = (self.images.as_ref()).filter(|_| self.image_previews);
        let max_px = picker.map_or((0, 0), |picker| self.max_px(picker));
        let svg = self.is_svg();
        let svg_view = self.svg_view;
        let DiffPane::Loaded(view) = &mut self.diff else {
            return;
        };
        if !svg {
            view.preview = Preview::load(&view.file, picker, max_px);
            return;
        }
        // Code draws no pictures, so nothing is rendered until Preview is picked; an `.svgz`
        // has no code to show, so it always gets a preview, sizes only without a picker.
        let render = picker.filter(|_| svg_view == SvgView::Preview || view.file.binary);
        if render.is_none() && !view.file.binary {
            view.preview = None;
            return;
        }
        if render.is_some() && self.fonts.is_none() && preview::needs_fonts(&view.file) {
            // ponytail: about 50–200 ms on the UI thread, once per run; load on a thread if
            // anyone notices the pause.
            let mut fonts = Database::new();
            fonts.load_system_fonts();
            self.fonts = Some(Arc::new(fonts));
        }
        view.preview = Preview::svg(&view.file, render, max_px, self.fonts.as_ref());
    }

    /// Makes the shown pictures again when the pane has grown enough to draw them sharper.
    pub fn fit_preview(&mut self) {
        let Some(picker) = &self.images else {
            return;
        };
        let max_px = self.max_px(picker);
        let blurry = matches!(&self.diff, DiffPane::Loaded(view)
            if view.preview.as_ref().is_some_and(|p| p.blurry_at(max_px)));
        if blurry && self.picture_shown() {
            self.reload_preview();
        }
    }

    /// Whether the shown file is an SVG or `.svgz`, drawn by resvg.
    pub fn is_svg(&self) -> bool {
        (self.selected_file()).is_some_and(|f| {
            (f.path.extension()).is_some_and(|ext| {
                ext.eq_ignore_ascii_case("svg") || ext.eq_ignore_ascii_case("svgz")
            })
        })
    }

    /// Whether the shown SVG has code to switch to: an `.svgz` is binary.
    pub fn svg_has_code(&self) -> bool {
        self.is_svg() && matches!(&self.diff, DiffPane::Loaded(view) if !view.file.binary)
    }

    /// Shows the SVG as `view`; a notice instead when pictures can't be drawn.
    fn set_svg_view(&mut self, view: SvgView) -> bool {
        if !self.svg_has_code() || view == self.svg_view {
            return false;
        }
        let why = if view == SvgView::Code {
            self.svg_view = view;
            self.reload_preview();
            return true;
        } else if self.images.is_none() {
            "this terminal can't draw images"
        } else if !self.image_previews {
            "image previews are off: turn them on in the View menu"
        } else {
            self.svg_view = view;
            self.reload_preview();
            return true;
        };
        self.notice = Some(Notice::Error(why.into()));
        true
    }

    /// Opens the history popup and asks for its first page.
    fn open_history(&mut self) -> bool {
        if self.patch_mode {
            self.notice = Some(Notice::Error("a patch has no history".into()));
            return true;
        }
        let (history, want) = History::new();
        self.history = Some(Box::new(history));
        self.palette = None;
        self.pending_history.push(want);
        true
    }

    /// Keys while the history popup is open; it takes them all, like the open menu.
    fn history_key(&mut self, key: KeyEvent) -> bool {
        let Some(history) = &mut self.history else {
            return false;
        };
        if history.typing {
            return self.history_search_key(key);
        }
        let half = page(history.preview_area) / 2;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = page(match history.pane {
            Pane::Preview => history.preview_area,
            _ => history.list_area,
        });
        let button = match key.code {
            KeyCode::Char('d') if ctrl => return history.scroll_preview(half),
            KeyCode::Char('u') if ctrl => return history.scroll_preview(-half),
            KeyCode::Tab | KeyCode::BackTab => return history.switch_pane(),
            KeyCode::Char(']') => return history.preview_change(true),
            KeyCode::Char('[') => return history.preview_change(false),
            KeyCode::Esc => Button::Back,
            KeyCode::Enter => Button::Enter,
            KeyCode::Char('/') => Button::Search,
            KeyCode::Char('o') => Button::Open,
            KeyCode::Char('w') => Button::Worktree,
            KeyCode::Char('y') => Button::Copy,
            code => {
                let delta = match code {
                    KeyCode::Down | KeyCode::Char('j') => 1,
                    KeyCode::Up | KeyCode::Char('k') => -1,
                    KeyCode::PageDown => page,
                    KeyCode::PageUp => -page,
                    KeyCode::Home | KeyCode::Char('g') => isize::MIN / 2,
                    KeyCode::End | KeyCode::Char('G') => isize::MAX / 2,
                    _ => return false,
                };
                let (moved, wants) = history.step(delta);
                self.pending_history.extend(wants);
                return moved;
            }
        };
        self.history_press(button)
    }

    /// Keys while typing a history search: text goes to the box, arrows still move.
    fn history_search_key(&mut self, key: KeyEvent) -> bool {
        let Some(history) = &mut self.history else {
            return false;
        };
        match key.code {
            KeyCode::Esc => {
                let wants = history.clear_search();
                self.pending_history.extend(wants);
                true
            }
            KeyCode::Enter => {
                history.typing = false;
                true
            }
            KeyCode::Up | KeyCode::Down => {
                let (moved, wants) = history.step(if key.code == KeyCode::Up { -1 } else { 1 });
                self.pending_history.extend(wants);
                moved
            }
            _ => self.history_search_edit(|field| field.key(key)),
        }
    }

    /// Edits the history search with `act`, filtering again when the text changed.
    fn history_search_edit(&mut self, act: impl FnOnce(&mut Field) -> Edit) -> bool {
        let Some(history) = &mut self.history else {
            return false;
        };
        let Some(field) = &mut history.search else {
            return false;
        };
        let edit = act(field);
        let copied = field.take_copied();
        if edit == Edit::Changed {
            let wants = history.filter();
            self.pending_history.extend(wants);
        }
        if let Some(text) = copied {
            let label = snippet(&text);
            self.copy(text, &label);
        }
        edit != Edit::Ignored
    }

    /// Sends `text` to the clipboard and confirms it with a `Copied {label}` toast.
    fn copy(&mut self, text: String, label: &str) {
        self.toast = Some(Toast::new(format!("Copied {label}")));
        self.pending_copy = Some(text);
    }

    /// Does what a hint-row button or its key does.
    fn history_press(&mut self, button: Button) -> bool {
        let back = match &self.viewing {
            Some(viewing) => viewing.back.clone(),
            None => self.selected_file().map(|f| f.path.clone()),
        };
        let Some(history) = &mut self.history else {
            return false;
        };
        let reply = history.press(button, back);
        self.pending_history.extend(reply.wants);
        if let Some(id) = reply.copy {
            let label = history::short(&id).to_owned();
            self.copy(id, &label);
        }
        if reply.close {
            self.history = None;
        }
        reply.redraw
    }

    /// Mouse over the history popup: buttons press, the hash copies, rows select and a second
    /// click goes one step in; a click outside closes.
    fn history_mouse(&mut self, kind: MouseEventKind, position: Position) -> bool {
        let Some(history) = &mut self.history else {
            return false;
        };
        let click = kind == MouseEventKind::Down(MouseButton::Left);
        let wheel = match kind {
            MouseEventKind::ScrollDown => WHEEL_STEP,
            MouseEventKind::ScrollUp => -WHEEL_STEP,
            _ if !click => return false,
            _ if !history.area.contains(position) => {
                self.history = None;
                return true;
            }
            _ => 0,
        };
        if click {
            let pressed = (history.buttons.iter())
                .find(|(area, _)| area.contains(position))
                .map(|&(_, button)| button);
            let button = pressed
                .or_else(|| {
                    (history.copy_areas.iter().any(|a| a.contains(position)))
                        .then_some(Button::Copy)
                })
                .or_else(|| {
                    history
                        .search_area
                        .contains(position)
                        .then_some(Button::Search)
                });
            if history.clear_area.contains(position) {
                let wants = history.clear_search();
                self.pending_history.extend(wants);
                return true;
            }
            if let Some(button) = button {
                return self.history_press(button);
            }
            // A click elsewhere ends typing and keeps the search.
            history.typing = false;
        }
        if history.preview_area.contains(position) {
            if wheel != 0 {
                return history.scroll_preview(wheel);
            }
            let open = history.preview.is_some();
            if open {
                history.pane = Pane::Preview;
            }
            return open;
        }
        let (pane, area, top, at) = if history.files_area.contains(position) {
            let at = history.file;
            (
                Pane::Files,
                history.files_area,
                history.file_scroll,
                Some(at),
            )
        } else if history.list_area.contains(position) {
            let at = history.position();
            (Pane::Commits, history.list_area, history.scroll, at)
        } else {
            return click;
        };
        history.pane = pane;
        if wheel != 0 {
            let (_, wants) = history.step(wheel);
            self.pending_history.extend(wants);
            return true;
        }
        let Some(at) = at else {
            return false;
        };
        let row = top + usize::from(position.y - area.y);
        if row == at {
            let (moved, want) = history.enter();
            self.pending_history.extend(want);
            return moved;
        }
        let delta = isize::try_from(row).unwrap_or(isize::MAX) - isize::try_from(at).unwrap_or(0);
        let (moved, wants) = history.step(delta);
        self.pending_history.extend(wants);
        moved
    }

    /// The history popup's commits arrived.
    pub fn history_commits(&mut self, commits: Vec<Commit>) {
        if let Some(history) = &mut self.history {
            self.pending_history.extend(history.loaded(commits));
        }
    }

    /// The files of commit `id` arrived, for the popup.
    pub fn history_files(&mut self, id: &str, files: Vec<FileEntry>) {
        if let Some(history) = &mut self.history {
            self.pending_history.extend(history.files_loaded(id, files));
        }
    }

    /// The diff of `path` in commit `id` arrived, for the popup's unified preview.
    pub fn history_preview(&mut self, id: &str, path: &Path, diff: Result<FileDiff, Error>) {
        let language = zdiff_highlight::language(path).filter(|_| self.syntax);
        let shape = Shape {
            view: View::Unified,
            context: self.context,
        };
        let Some(history) = &mut self.history else {
            return;
        };
        // A file that can't be read previews as nothing changed, rather than an error box.
        let file = diff.unwrap_or_else(|_| FileDiff::new(Vec::new(), Vec::new()));
        history.preview_loaded(id, path, DiffView::new(file, language, shape));
    }

    /// The main view now shows `commit`; `refresh` already listed its files.
    pub fn viewing_opened(
        &mut self,
        (commit, worktree): (&Commit, bool),
        file: Option<&Path>,
        back: Option<PathBuf>,
    ) {
        self.history = None;
        self.viewing = Some(Viewing {
            label: history::label(commit, worktree),
            back,
        });
        self.select_path(file);
    }

    /// The main view shows the working tree again; `refresh` already listed its files.
    pub fn viewing_closed(&mut self) {
        let back = self.viewing.take().and_then(|v| v.back);
        self.select_path(back.as_deref());
    }

    /// Selects the file at `path` when it's listed, else the first file.
    fn select_path(&mut self, path: Option<&Path>) {
        let row = path.and_then(|path| self.tree.find(path));
        let row = row.or_else(|| (0..self.tree.len()).find(|&r| self.tree.file_at(r).is_some()));
        if let Some(row) = row {
            self.select(row);
        }
    }

    /// Whether the diff pane draws the shown image as pictures: no popup covers it.
    /// A text file has pictures only as an SVG in Preview, so popups show its code.
    pub fn picture_shown(&self) -> bool {
        let popup = self.palette.is_some()
            || self.menu.open.is_some()
            || self.find.is_some()
            || self.history.is_some();
        !popup
            && self.stream.is_none()
            && matches!(&self.diff, DiffPane::Loaded(view)
                if view.preview.as_ref().is_some_and(Preview::drawable))
    }

    /// Whether the shown image has both sides as pictures, so swipe and onion skin apply.
    fn comparable(&self) -> bool {
        self.picture_shown()
            && matches!(&self.diff, DiffPane::Loaded(view)
                if view.preview.as_ref().is_some_and(Preview::comparable))
    }

    /// Moves the swipe divider or onion opacity to `mix`, clamped to 0..=100.
    fn set_mix(&mut self, mix: u8) -> bool {
        let mix = mix.min(100);
        mix != mem::replace(&mut self.mix, mix)
    }

    /// The diff pane's size in pixels, which pictures are shrunk to; a guess before the first draw.
    fn max_px(&self, picker: &Picker) -> (u32, u32) {
        let font = picker.font_size();
        match self.diff_area {
            Rect { width: 0, .. } | Rect { height: 0, .. } => (1024, 1024),
            area => (
                u32::from(area.width) * u32::from(font.width),
                u32::from(area.height) * u32::from(font.height),
            ),
        }
    }

    /// Switches to the view that fits a `width`-column diff pane; a no-op when it already matches.
    pub fn fit_view(&mut self, width: u16) {
        self.view = view_for(width, self.view_choice);
        let shape = self.shape();
        if let Some(stream) = &mut self.stream
            && stream.shape != shape
        {
            stream.reshape(shape);
        }
        // ponytail: rebuilding rows closes folds opened by hand; carry them over if anyone minds.
        if let DiffPane::Loaded(diff) = &mut self.diff
            && diff.shape != shape
        {
            diff.set_shape(shape);
            if let Some(find) = &mut self.find {
                find.update(diff);
            }
        }
    }

    /// The rows the shown file should have: current view and context.
    fn shape(&self) -> Shape {
        Shape {
            view: self.view,
            context: self.context,
        }
    }

    /// Flips between split and unified; sticks until picked again.
    fn toggle_view(&mut self) -> bool {
        self.view_choice = Some(match self.view {
            View::Split => View::Unified,
            View::Unified => View::Split,
        });
        true
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
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                let cleared = self.notice.take().is_some();
                self.on_key(*key) || cleared
            }
            Event::Mouse(mouse) => self.on_mouse(*mouse),
            Event::Paste(text) => self.paste(text),
            Event::Resize(..) => true,
            _ => false,
        }
    }

    fn on_key(&mut self, key: KeyEvent) -> bool {
        // Raw mode turns Ctrl+C into a key press instead of SIGINT; it always quits.
        // Ctrl+Shift+C is copy, so only plain Ctrl+C quits.
        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('c') {
            return self.stop();
        }
        let chord = Chord::from_event(key);
        if self.capture().is_some() {
            return self.capture_key(chord);
        }
        let global = self.keymap.lookup(Context::Global, chord);
        if self.menu.open.is_some() && global != Some(Action::OpenMenu) {
            return self.menu_key(key.code);
        }
        if self.history.is_some() {
            return self.history_key(key);
        }
        let typing = self.palette.is_some() || self.find.is_some() || self.writing;
        if let Some(redraw) = self.field_key(key) {
            return redraw;
        }
        // Esc quits, except while a commit is shown: then it goes back to the working tree.
        if self.viewing.is_some()
            && !typing
            && chord == Chord::new(KeyCode::Esc, KeyModifiers::NONE)
        {
            self.pending_history.push(Want::Back);
            return true;
        }
        // Over a text input only Ctrl, Alt, and F-keys reach bindings, so letters stay text.
        if let Some(action) = global.filter(|_| !typing || chord.is_command()) {
            return self.run(action);
        }
        if self.palette.is_some() {
            return self.palette_key(key);
        }
        if self.find.is_some() {
            return self.find_key(key);
        }
        if self.writing {
            return self.commit_key(key);
        }
        let context = match self.focus {
            Focus::Sidebar => Context::Sidebar,
            Focus::Diff => Context::Diff,
        };
        (self.keymap.lookup(context, chord)).is_some_and(|action| self.run(action))
    }

    /// Keys for the open menu; it takes every key so none reach the panes.
    fn menu_key(&mut self, code: KeyCode) -> bool {
        let Some((menu, item)) = self.menu.open else {
            return false;
        };
        let items = MENUS[menu].1;
        match code {
            KeyCode::Up | KeyCode::Down => {
                let item = menu::step(items, item, code == KeyCode::Down);
                self.menu.open = Some((menu, item));
            }
            KeyCode::Left | KeyCode::Right => {
                let menu = find::wrap(menu, MENUS.len(), code == KeyCode::Right);
                self.menu.open = Some((menu, 0));
            }
            KeyCode::Enter => return self.run(items[item]),
            KeyCode::Esc => self.menu.open = None,
            _ => return false,
        }
        true
    }

    /// Closes the menu and runs `item`.
    /// Carries out `action`, from a key or a menu; returns whether anything visible changed.
    fn run(&mut self, action: Action) -> bool {
        let menu_was_open = self.menu.open.take().is_some();
        match action {
            Action::GoToFile => {
                self.open_files();
                true
            }
            Action::Reload => self.request_reload(),
            Action::Quit => self.stop(),
            Action::Show(choice) => {
                self.view_choice = choice;
                true
            }
            Action::Sidebar => self.toggle_sidebar(),
            Action::GoToLine => self.open_goto(),
            Action::Find => self.ctrl_f(),
            Action::SearchAll => self.open_search(Query::default(), ""),
            Action::Top
            | Action::Bottom
            | Action::ScrollDown
            | Action::ScrollUp
            | Action::HalfPageDown
            | Action::HalfPageUp
            | Action::PageDown
            | Action::PageUp
            | Action::ScrollLeft
            | Action::ScrollRight
            | Action::NextChange
            | Action::PrevChange
            | Action::ExpandFold => self.diff_action(action),
            Action::SelectDown
            | Action::SelectUp
            | Action::SelectPageDown
            | Action::SelectPageUp
            | Action::SelectFirst
            | Action::SelectLast
            | Action::ToggleFolder
            | Action::CloseFolder
            | Action::OpenFolder => self.sidebar_action(action),
            Action::ToggleStage => self.toggle_stage(self.cursor),
            Action::Stage => self.stage(),
            Action::Commit => self.commit(),
            Action::FocusCommit => self.can_stage() && !mem::replace(&mut self.writing, true),
            Action::ToggleView => self.toggle_view(),
            Action::ToggleScope => self.toggle_scope(),
            Action::ToggleFocus if !self.sidebar_hidden => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Diff,
                    Focus::Diff => Focus::Sidebar,
                };
                true
            }
            Action::OpenMenu => {
                self.menu.open = (!menu_was_open).then_some((0, 0));
                self.palette = None;
                true
            }
            Action::Shortcuts => self.open_shortcuts(),
            Action::SaveLayout
            | Action::ResetLayout
            | Action::Syntax
            | Action::WatchAtStart
            | Action::ImagePreviews
            | Action::SvgPreviewDefault
            | Action::Staging => self.settings_action(action),
            Action::SvgView | Action::ImageCompare => self.picture_action(action),
            Action::History => self.open_history(),
            Action::Theme => {
                self.palette = Some(Palette::themes(self.theme));
                true
            }
            Action::NextFile => self.step_file(true),
            Action::PrevFile => self.step_file(false),
            Action::Scope(scope) => scope != self.scope() && self.toggle_scope(),
            Action::SidebarRight => {
                self.sidebar_right = !self.sidebar_right;
                true
            }
            // The next draw's `fit_view` rebuilds the rows for the new context.
            Action::ExpandAll => {
                self.context = u32::MAX;
                true
            }
            Action::CollapseAll => {
                self.context = CONTEXT_LINES;
                true
            }
            Action::MoreContext | Action::LessContext => {
                self.context = next_context(self.context, action == Action::MoreContext);
                true
            }
            Action::WordHighlights => {
                self.word_highlights = !self.word_highlights;
                true
            }
            // Tab with the sidebar hidden; separators are never selected or clicked.
            Action::ToggleFocus | Action::Separator => false,
        }
    }

    /// Asks the event loop to reload the file list, like a `--watch` refresh.
    fn request_reload(&mut self) -> bool {
        self.pending_reload = true;
        true
    }

    /// Hides or shows the sidebar; hiding moves focus to the diff so keys never go to a hidden pane.
    /// The syntax of the file at `node`, or `None` while syntax colors are off.
    fn language(&self, node: usize) -> Option<Language> {
        let file = self.tree.file(node).filter(|_| self.syntax)?;
        zdiff_highlight::language(&file.path)
    }

    /// The live window state, as Save layout as default stores it.
    fn layout(&self) -> Layout {
        Layout {
            view: self.view_choice,
            all_files: self.stream.is_some(),
            sidebar_right: self.sidebar_right,
            sidebar_hidden: self.sidebar_hidden,
            sidebar_width: self.sidebar_width,
            context: if self.context == u32::MAX {
                CONTEXT_LINES
            } else {
                self.context
            },
            all_lines: self.context == u32::MAX,
            word_highlights: self.word_highlights,
        }
    }

    /// Puts the window in `layout`.
    fn set_layout(&mut self, layout: &Layout) {
        self.view_choice = layout.view;
        self.sidebar_right = layout.sidebar_right;
        self.sidebar_hidden = layout.sidebar_hidden;
        self.sidebar_width = layout.sidebar_width;
        self.context = if layout.all_lines {
            u32::MAX
        } else {
            layout.context
        };
        self.word_highlights = layout.word_highlights;
        if layout.all_files != self.stream.is_some() {
            self.toggle_scope();
        }
        if self.sidebar_hidden {
            self.focus = Focus::Diff;
        }
    }

    /// Applies loaded settings; returns warnings about keys it couldn't use.
    pub fn apply(&mut self, settings: Settings) -> Vec<String> {
        self.syntax = settings.general.syntax;
        self.image_previews = settings.general.images;
        self.svg_preview_default = settings.general.svg_preview;
        self.svg_view = svg_view_for(self.svg_preview_default);
        self.staging = settings.general.staging;
        self.watch_default = settings.general.watch;
        self.theme = settings.general.theme;
        let (keymap, warnings) = Keymap::with_overrides(&settings.keys);
        self.keymap = keymap;
        if let Some(layout) = &settings.layout {
            self.set_layout(layout);
        }
        self.saved_layout = settings.layout;
        warnings
    }

    /// What the settings file should hold now.
    pub fn settings(&self) -> Settings {
        Settings {
            general: General {
                syntax: self.syntax,
                staging: self.staging,
                images: self.image_previews,
                svg_preview: self.svg_preview_default,
                watch: self.watch_default,
                theme: self.theme,
            },
            layout: self.saved_layout.clone(),
            keys: self.keymap.overrides(),
        }
    }

    /// Marks the settings file for saving and returns that a redraw is needed.
    fn save_settings(&mut self) -> bool {
        self.pending_save = true;
        true
    }

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
            return self.open_search(find.query, "");
        }
        if self.focus == Focus::Sidebar
            && let Some(Node::Dir { path, .. }) = self.tree.node(self.cursor)
        {
            let include = path.display().to_string();
            return self.open_search(Query::default(), &include);
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
            _ => return false,
        }
        refind(find, view, height);
        true
    }

    /// The rebinding in progress on the shortcuts screen.
    fn capture(&self) -> Option<Capture> {
        match &self.palette {
            Some(Palette {
                mode: Mode::Keys(keys),
                ..
            }) => keys.capture,
            _ => None,
        }
    }

    /// Takes the key pressed while rebinding: Esc cancels, a taken key asks first.
    fn capture_key(&mut self, chord: Chord) -> bool {
        let Some(Palette {
            field,
            mode: Mode::Keys(keys),
        }) = &mut self.palette
        else {
            return false;
        };
        let Some(mut capture) = keys.capture else {
            return false;
        };
        let esc = Chord::new(KeyCode::Esc, KeyModifiers::NONE);
        let enter = Chord::new(KeyCode::Enter, KeyModifiers::NONE);
        let bind = match capture.taken {
            _ if chord == esc => None,
            Some((key, holder)) if chord == enter => {
                self.keymap.remove(holder, key);
                Some(key)
            }
            Some(_) => None,
            None if chord.is_locked() => return false,
            None => match self.keymap.holder(capture.action, chord) {
                Some(holder) => {
                    capture.taken = Some((chord, holder));
                    keys.capture = Some(capture);
                    return true;
                }
                None => Some(chord),
            },
        };
        keys.capture = None;
        let Some(key) = bind else {
            return true;
        };
        if capture.add {
            self.keymap.add(capture.action, key);
        } else {
            self.keymap.set(capture.action, &[key]);
        }
        keys.rebuild(&self.keymap, &field.text());
        self.save_settings()
    }

    /// Starts rebinding the selected shortcut, replacing its keys or adding one.
    fn start_capture(&mut self, add: bool) -> bool {
        let Some(Palette {
            mode: Mode::Keys(keys),
            ..
        }) = &mut self.palette
        else {
            return false;
        };
        let Some(action) = keys.selected_action() else {
            return false;
        };
        keys.capture = Some(Capture {
            action,
            add,
            taken: None,
        });
        true
    }

    /// Puts the selected shortcut back on its built-in keys.
    fn reset_shortcut(&mut self) -> bool {
        let Some(Palette {
            field,
            mode: Mode::Keys(keys),
        }) = &mut self.palette
        else {
            return false;
        };
        let Some(action) = keys.selected_action() else {
            return false;
        };
        self.keymap.reset(action);
        keys.rebuild(&self.keymap, &field.text());
        self.save_settings()
    }

    /// The theme to draw with: the picker's selection while it is open, as a live preview.
    pub fn shown_theme(&self) -> Theme {
        match &self.palette {
            Some(Palette {
                mode: Mode::Themes(themes),
                ..
            }) => themes.selected().unwrap_or(self.theme),
            _ => self.theme,
        }
    }

    /// Switches to `theme`, closes the picker, and saves; `None` only closes it.
    fn pick_theme(&mut self, theme: Option<Theme>) -> bool {
        self.palette = None;
        if let Some(theme) = theme {
            self.theme = theme;
            self.save_settings();
        }
        true
    }

    fn open_shortcuts(&mut self) -> bool {
        self.palette = Some(Palette::keys(&self.keymap));
        true
    }

    fn open_files(&mut self) -> bool {
        if self.tree.is_empty() {
            return false;
        }
        self.palette = Some(Palette::files(&self.tree));
        true
    }

    fn open_search(&mut self, query: Query, include: &str) -> bool {
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
        // A hit opens in the single-file view, where find and the jump work.
        self.stream = None;
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
            _ => return false,
        }
        true
    }

    /// Edits the palette input and acts on it; Esc cancels.
    fn palette_key(&mut self, key: KeyEvent) -> bool {
        let Some(Palette { field, mode }) = &mut self.palette else {
            return false;
        };
        if matches!(mode, Mode::Search(_)) {
            return self.search_key(key);
        }
        match (key.code, mode) {
            (KeyCode::Esc, _) => self.palette = None,
            (KeyCode::Enter, Mode::Line) => {
                let target = palette::parse(&field.text());
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
            (KeyCode::Enter, Mode::Keys(_)) => {
                return self.start_capture(key.modifiers.contains(KeyModifiers::SHIFT));
            }
            (KeyCode::Delete, Mode::Keys(_)) => return self.reset_shortcut(),
            (KeyCode::Enter, Mode::Themes(themes)) => {
                let theme = themes.selected();
                return self.pick_theme(theme);
            }
            (KeyCode::Up, Mode::Themes(themes)) => return themes.step(false),
            (KeyCode::Down, Mode::Themes(themes)) => return themes.step(true),
            (KeyCode::Up, Mode::Keys(keys)) => return keys.step(false),
            (KeyCode::Down, Mode::Keys(keys)) => return keys.step(true),
            _ => return false,
        }
        true
    }

    /// Selects the file at tree `node` in the sidebar, opening its folders, and closes the palette.
    /// Opens the next or previous changed file in sidebar order, even inside a closed folder;
    /// stops at either end. In All files, `select` jumps the stream there.
    fn step_file(&mut self, forward: bool) -> bool {
        let next = {
            let mut nodes = self.tree.files().map(|(node, _)| node);
            match (self.file, forward) {
                (None, _) => nodes.next(),
                (Some(file), true) => nodes.skip_while(|&n| n != file).nth(1),
                (Some(file), false) => nodes.take_while(|&n| n != file).last(),
            }
        };
        next.is_some_and(|node| self.open_file(node))
    }

    fn open_file(&mut self, node: usize) -> bool {
        self.palette = None;
        let path = self.tree.file(node).map(|f| f.path.clone());
        if let Some(row) = path.and_then(|path| self.tree.reveal(&path)) {
            self.select(row);
            self.focus = Focus::Diff;
        }
        true
    }

    /// Sidebar movement and folders.
    fn sidebar_action(&mut self, action: Action) -> bool {
        let page = page(self.sidebar_rows);
        match action {
            Action::SelectDown => self.move_by(1),
            Action::SelectUp => self.move_by(-1),
            Action::SelectPageDown => self.move_by(page),
            Action::SelectPageUp => self.move_by(-page),
            Action::SelectFirst => self.move_by(isize::MIN),
            Action::SelectLast => self.move_by(isize::MAX),
            Action::ToggleFolder => self.toggle(),
            Action::CloseFolder => match self.tree.is_open(self.cursor) {
                Some(true) => self.tree.set_open(self.cursor, false),
                _ => self
                    .tree
                    .parent(self.cursor)
                    .map(|row| self.select(row))
                    .is_some(),
            },
            Action::OpenFolder => match self.tree.is_open(self.cursor) {
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

    /// Diff pane movement, for the single file or the All files stream.
    fn diff_action(&mut self, action: Action) -> bool {
        let page = page(self.diff_area);
        let height = usize::from(self.diff_area.height);
        let rows = scroll_rows(action, page);
        let columns = scroll_columns(action);
        if let Some(delta) = columns
            && self.image_compare != Compare::TwoUp
            && self.comparable()
        {
            let mix = if delta < 0 {
                self.mix.saturating_sub(MIX_STEP)
            } else {
                self.mix + MIX_STEP
            };
            return self.set_mix(mix);
        }
        if let Some(stream) = &mut self.stream {
            let moved = match action {
                _ if rows.is_some() => rows.is_some_and(|delta| stream.scroll_by(delta, height)),
                _ if columns.is_some() => columns.is_some_and(|delta| stream.hscroll_by(delta)),
                Action::NextChange => stream.next_header(height),
                Action::PrevChange => stream.prev_header(height),
                Action::ExpandFold => stream.expand(height),
                _ => false,
            };
            self.sync_top();
            return moved;
        }
        let DiffPane::Loaded(view) = &mut self.diff else {
            return false;
        };
        match action {
            _ if rows.is_some() => rows.is_some_and(|delta| view.scroll_by(delta, height)),
            _ if columns.is_some() => columns.is_some_and(|delta| view.hscroll_by(delta, height)),
            Action::NextChange => view.next_header(height),
            Action::PrevChange => view.prev_header(height),
            Action::ExpandFold => view.expand_visible_fold(height),
            _ => false,
        }
    }

    /// A click on a compare tab picks it; a click or drag on a swiped picture moves the
    /// divider there. `None` when the mouse isn't on either.
    fn compare_mouse(&mut self, kind: MouseEventKind, position: Position) -> Option<bool> {
        let left = matches!(
            kind,
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left)
        );
        if !left || !self.comparable() {
            return None;
        }
        if let Some(mode) = find::at(Compare::ALL, &self.compare_tabs, position) {
            return Some(mode != mem::replace(&mut self.image_compare, mode));
        }
        let area = self.picture_area;
        if self.image_compare != Compare::Swipe || !area.contains(position) {
            return None;
        }
        let percent = u32::from(position.x - area.x) * 100 / u32::from(area.width);
        let step = u32::from(MIX_STEP);
        let mix = (percent + step / 2) / step * step;
        Some(self.set_mix(u8::try_from(mix).expect("a percent fits a byte")))
    }

    /// A click on the commit box starts writing; any other click stops it, then acts as usual.
    fn on_mouse(&mut self, mouse: MouseEvent) -> bool {
        let position = Position::new(mouse.column, mouse.row);
        let click = mouse.kind == MouseEventKind::Down(MouseButton::Left);
        let over = self.menu.open.is_some() || self.palette.is_some() || self.history.is_some();
        if click && !over && self.commit_box.contains(position) {
            return !mem::replace(&mut self.writing, true);
        }
        let stopped = click && mem::take(&mut self.writing);
        self.pointer(mouse) || stopped
    }

    fn pointer(&mut self, mouse: MouseEvent) -> bool {
        let position = Position::new(mouse.column, mouse.row);
        let click = mouse.kind == MouseEventKind::Down(MouseButton::Left);
        if self.menu.open.is_some() {
            // The open menu owns the mouse: a click runs an item or closes it.
            if !click {
                return false;
            }
            if let Some(item) = self.menu.item_at(position) {
                return self.run(item);
            }
            self.menu.open = None;
            return true;
        }
        let height = usize::from(self.diff_area.height);
        if let (Some(find), DiffPane::Loaded(view)) = (&mut self.find, &mut self.diff)
            && self.palette.is_none()
            && find.area.contains(position)
        {
            // The bar floats over the diff; clicks on it never reach the rows below.
            let Some(toggle) =
                find::at(Toggle::ALL, &find.toggle_areas, position).filter(|_| click)
            else {
                return false;
            };
            toggle.flip(&mut find.query);
            refind(find, view, height);
            return true;
        }
        if self.palette.is_some() {
            return self.palette_mouse(mouse, position);
        }
        if self.history.is_some() {
            return self.history_mouse(mouse.kind, position);
        }
        if click && self.viewing.is_some() && self.back_button.contains(position) {
            self.pending_history.push(Want::Back);
            return true;
        }
        if let Some(moved) = self.compare_mouse(mouse.kind, position) {
            return moved;
        }
        if let Some(view) = find::at(SvgView::ALL, &self.svg_tabs, position)
            .filter(|_| click && self.is_svg() && self.stream.is_none())
        {
            return self.set_svg_view(view);
        }
        if let Some(menu) = self.menu.title_at(position).filter(|_| click) {
            self.menu.open = Some((menu, 0));
            return true;
        }
        if click && self.sidebar_toggle.contains(position) {
            return self.toggle_sidebar();
        }
        if click && self.commit_button.contains(position) {
            return self.commit();
        }
        // Checked before hit-testing panes so a drag keeps going outside the sidebar.
        let (edge, width) = handle(self.sidebar, self.sidebar_right, mouse.column);
        let on_divider = self.sidebar.contains(position) && mouse.column == edge;
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if on_divider => {
                self.resizing = true;
                return true;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.resizing => {
                let width = Some(width);
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

    /// The palette owns the mouse: clicks pick a row or close it, nothing reaches the panes.
    fn palette_mouse(&mut self, mouse: MouseEvent, position: Position) -> bool {
        let click = mouse.kind == MouseEventKind::Down(MouseButton::Left);
        let wheel = match mouse.kind {
            MouseEventKind::ScrollDown => Some(true),
            MouseEventKind::ScrollUp => Some(false),
            _ => None,
        };
        if let Some(down) = wheel {
            return self.palette_step(down);
        }
        if !click {
            return false;
        }
        let Some(palette) = &self.palette else {
            return false;
        };
        if !self.palette_area.contains(position) {
            self.palette = None;
            return true;
        }
        let row_at = |area: Rect, offset: usize| {
            area.contains(position)
                .then(|| offset + usize::from(mouse.row - area.y))
        };
        match &palette.mode {
            Mode::Line => false,
            Mode::Themes(themes) => {
                let theme = row_at(themes.area, themes.list.offset())
                    .and_then(|row| themes.shown.get(row).copied());
                theme.is_some() && self.pick_theme(theme)
            }
            Mode::Keys(keys) => {
                let row =
                    row_at(keys.area, keys.list.offset()).filter(|&row| row < keys.shown.len());
                if let Some(Palette {
                    mode: Mode::Keys(keys),
                    ..
                }) = &mut self.palette
                    && row.is_some()
                {
                    keys.list.select(row);
                }
                row.is_some()
            }
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
                let target =
                    row_at(search.list_area, search.list.offset()).and_then(|row| search.hit(row));
                self.open_hit(target)
            }
        }
    }

    /// Moves a palette's selection by one wheel step.
    fn palette_step(&mut self, down: bool) -> bool {
        let Some(palette) = &mut self.palette else {
            return false;
        };
        let mut step = || match &mut palette.mode {
            Mode::File(files) => files.step(down),
            Mode::Search(search) => search.step(down),
            Mode::Keys(keys) => keys.step(down),
            Mode::Themes(themes) => themes.step(down),
            Mode::Line => false,
        };
        (0..WHEEL_STEP.unsigned_abs()).fold(false, |moved, _| step() | moved)
    }

    fn sidebar_mouse(&mut self, mouse: MouseEvent) -> bool {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let position = Position::new(mouse.column, mouse.row);
                if self.stage_button.contains(position) {
                    return self.stage();
                }
                // The stage bar's row holds no file.
                if !self.stage_button.is_empty() && mouse.row == self.stage_button.y {
                    return false;
                }
                self.focus = Focus::Sidebar;
                let row = self.list.offset() + usize::from(mouse.row - self.sidebar.y);
                if row >= self.tree.len() {
                    return false;
                }
                // The drag handle takes the first column when the sidebar is on the right.
                let rows_x = self.sidebar.x + u16::from(self.sidebar_right);
                let column = usize::from(mouse.column.saturating_sub(rows_x));
                let depth = self.tree.node(row).map_or(0, Node::depth);
                if self.can_stage()
                    && (checkbox_column(depth)..checkbox_column(depth) + 2).contains(&column)
                {
                    return self.toggle_stage(row);
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
        if let Some(stream) = &mut self.stream {
            let moved = match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    self.focus = Focus::Diff;
                    stream.expand_at(stream.scroll + row)
                }
                MouseEventKind::ScrollRight => stream.hscroll_by(H_STEP),
                MouseEventKind::ScrollLeft => stream.hscroll_by(-H_STEP),
                MouseEventKind::ScrollDown if shift => stream.hscroll_by(H_STEP),
                MouseEventKind::ScrollUp if shift => stream.hscroll_by(-H_STEP),
                MouseEventKind::ScrollDown => stream.scroll_by(WHEEL_STEP, height),
                MouseEventKind::ScrollUp => stream.scroll_by(-WHEEL_STEP, height),
                _ => false,
            };
            self.sync_top();
            return moved;
        }
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
            if let Some(stream) = &mut self.stream {
                stream.jump(file, usize::from(self.diff_area.height));
            }
        }
    }

    fn stop(&mut self) -> bool {
        self.quit = true;
        true
    }
}

impl DiffView {
    pub(crate) fn new(file: FileDiff, language: Option<Language>, shape: Shape) -> Self {
        let rows = build_rows(&file, shape);
        let gutter = file.old.len().max(file.new.len()).to_string().len();
        // ponytail: highlight + word diff run on the UI thread; use a worker if big files stall.
        let tokens = |text: &Text| {
            language.map_or_else(Box::default, |l| {
                zdiff_highlight::highlight(l, text.bytes())
            })
        };
        let preview = Preview::load(&file, None, (0, 0));
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
            preview,
            shape,
        }
    }

    /// Furthest horizontal scroll: the widest visible line on either side, plus a little slack.
    pub(crate) fn max_hscroll(&self, height: usize) -> usize {
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

    /// Rebuilds the rows for `shape`, keeping the top line in view.
    pub(crate) fn set_shape(&mut self, shape: Shape) {
        let top = self.rows[self.scroll.min(self.rows.len())..]
            .iter()
            .find_map(|row| match *row {
                Row::Line { old, new, .. } => {
                    new.map(|n| (Side::New, n)).or(old.map(|o| (Side::Old, o)))
                }
                Row::Fold { new, .. } => Some((Side::New, new)),
                Row::Header { .. } | Row::Gap { .. } => None,
            });
        self.rows = build_rows(&self.file, shape);
        self.shape = shape;
        self.mark = None;
        self.scroll = top
            .and_then(|(side, line)| zdiff_core::locate(&self.rows, side, line))
            .map_or(0, |(row, _)| row);
    }

    fn scroll_to(&mut self, row: usize, height: usize) -> bool {
        self.mark = None;
        let scroll = row.min(self.max_scroll(height));
        let moved = scroll != self.scroll;
        self.scroll = scroll;
        moved
    }

    pub(crate) fn scroll_by(&mut self, delta: isize, height: usize) -> bool {
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

    pub(crate) fn next_header(&mut self, height: usize) -> bool {
        let start = self.scroll + 1;
        match self.rows.iter().skip(start).position(is_header) {
            Some(offset) => self.scroll_to(start + offset, height),
            None => false,
        }
    }

    pub(crate) fn prev_header(&mut self, height: usize) -> bool {
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

/// How SVGs open: as pictures when `preview`, else as code.
fn svg_view_for(preview: bool) -> SvgView {
    if preview {
        SvgView::Preview
    } else {
        SvgView::Code
    }
}

pub(crate) fn is_header(row: &Row) -> bool {
    matches!(row, Row::Header { .. })
}

/// Re-runs `find` on `view` and jumps to its current hit, as editors do while you type.
fn refind(find: &mut Find, view: &mut DiffView, height: usize) {
    find.update(view);
    if let Some(hit) = find.current.and_then(|i| find.hits.get(i)) {
        view.jump(hit.side, hit.line + 1, height);
    }
}

/// Rows a scrolling action moves in a pane showing `page` rows; `None` for other actions.
fn scroll_rows(action: Action, page: isize) -> Option<isize> {
    match action {
        Action::ScrollDown => Some(1),
        Action::ScrollUp => Some(-1),
        Action::HalfPageDown => Some(page / 2),
        Action::HalfPageUp => Some(-page / 2),
        Action::PageDown => Some(page),
        Action::PageUp => Some(-page),
        Action::Top => Some(isize::MIN),
        Action::Bottom => Some(isize::MAX),
        _ => None,
    }
}

/// Columns a sideways scrolling action moves; `None` for other actions.
fn scroll_columns(action: Action) -> Option<isize> {
    match action {
        Action::ScrollLeft => Some(-H_STEP),
        Action::ScrollRight => Some(H_STEP),
        _ => None,
    }
}

pub(crate) fn clamp_add(value: usize, delta: isize, max: usize) -> usize {
    value.saturating_add_signed(delta).min(max)
}

fn page(area: Rect) -> isize {
    isize::try_from(area.height).unwrap_or(1).max(1)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::tree::Node;

    fn entry(path: &str) -> FileEntry {
        FileEntry {
            path: path.into(),
            status: Status::Modified,
            added: 1,
            removed: 0,
            change: 0,
            staged: zdiff_core::Staged::No,
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

    /// App showing a changed `w`×`h` PNG as pictures, in a test terminal's stand-in protocol.
    fn image_app((w, h): (u32, u32)) -> App {
        use crate::preview::png;
        let mut app = app(&["logo.png"]);
        app.images = Some(Picker::halfblocks());
        app.show(Some(Ok(FileDiff::new(png(w, h), png(w, h)))));
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
    fn file_menu_runs_items_from_keys_and_clicks() {
        let mut app = app(&["a.rs", "b.rs"]);
        assert!(ctrl(&mut app, 'r'));
        assert!(mem::take(&mut app.pending_reload), "Ctrl+R");

        assert!(press(&mut app, KeyCode::F(10)));
        assert_eq!(app.menu.open, Some((0, 0)));
        press(&mut app, KeyCode::Enter);
        assert!(
            mem::take(&mut app.pending_reload),
            "Reload, the first File item"
        );
        assert_eq!(app.menu.open, None);
        assert!(!app.quit, "keys went to the menu, not the panes");

        let click = |column, row| mouse(MouseEventKind::Down(MouseButton::Left), column, row);
        app.menu.titles = [
            Rect::new(0, 0, 6, 1),
            Rect::new(6, 0, 6, 1),
            Rect::new(12, 0, 10, 1),
            Rect::new(22, 0, 5, 1),
            Rect::new(27, 0, 10, 1),
        ];
        app.menu.item_areas[..3].copy_from_slice(&[
            Rect::new(1, 2, 26, 1),
            Rect::new(1, 3, 26, 1),
            Rect::new(1, 4, 26, 1),
        ]);
        assert!(app.handle(&click(14, 0)));
        assert_eq!(app.menu.open, Some((2, 0)), "the Navigate title opens it");
        assert!(
            app.handle(&click(5, 2)),
            "Go to file, the first Navigate item"
        );
        assert!(matches!(
            app.palette,
            Some(Palette {
                mode: Mode::File(_),
                ..
            })
        ));
        assert_eq!(app.menu.open, None);

        press(&mut app, KeyCode::F(10));
        assert!(app.palette.is_none(), "the menu replaces the palette");
        assert!(app.handle(&click(60, 8)));
        assert_eq!(app.menu.open, None, "outside click closes");
        press(&mut app, KeyCode::F(10));
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Enter);
        assert!(app.quit, "Up wraps to Quit");
    }

    #[test]
    fn all_files_scrolls_through_every_file_and_moves_the_sidebar() {
        let mut app = app(&["a.rs", "b.rs"]);
        app.diff_area = Rect::new(30, 2, 70, 2);
        assert!(press(&mut app, KeyCode::Char('a')));
        assert_eq!(app.scope(), Scope::All);
        assert!(
            app.single_file().is_none(),
            "the event loop stops loading one file"
        );
        for (section, _) in app.stream_wants() {
            app.stream_loaded(section, Ok(FileDiff::new(b"x\n".to_vec(), b"y\n".to_vec())));
        }
        assert!(
            app.stream_wants().is_empty(),
            "everything near the screen is loaded"
        );

        app.focus = Focus::Diff;
        let path = |app: &App| app.selected_file().map(|f| f.path.display().to_string());
        for _ in 0..10 {
            if path(&app).as_deref() == Some("b.rs") {
                break;
            }
            assert!(press(&mut app, KeyCode::Char('j')), "scrolls on");
        }
        assert_eq!(
            path(&app).as_deref(),
            Some("b.rs"),
            "sidebar follows the top file"
        );
        assert_eq!(app.tree.file_at(app.cursor), app.file);
        assert!(!ctrl(&mut app, 'f'), "find is off in All files");

        app.focus = Focus::Sidebar;
        press(&mut app, KeyCode::Char('k'));
        let top = app.stream.as_ref().and_then(Stream::top_node);
        assert_eq!(
            top,
            app.tree.find_file(Path::new("a.rs")),
            "sidebar jumps the stream"
        );

        assert!(press(&mut app, KeyCode::Char('a')));
        assert_eq!(app.scope(), Scope::File);
        assert_eq!(path(&app).as_deref(), Some("a.rs"), "back on the top file");
        assert!(app.single_file().is_some());
    }

    #[test]
    fn n_and_p_step_through_files_even_in_closed_folders() {
        let mut app = app_of(&["a/x.rs", "a/y.rs", "b.rs"]);
        let path = |app: &App| app.selected_file().map(|f| f.path.display().to_string());
        assert_eq!(path(&app).as_deref(), Some("a/x.rs"));
        app.tree.set_open(0, false);
        assert!(press(&mut app, KeyCode::Char('n')));
        assert_eq!(path(&app).as_deref(), Some("a/y.rs"));
        assert_eq!(app.tree.is_open(0), Some(true), "its folder opens");
        assert!(press(&mut app, KeyCode::Char('n')));
        assert_eq!(path(&app).as_deref(), Some("b.rs"));
        assert!(
            !press(&mut app, KeyCode::Char('n')),
            "stops at the last file"
        );
        assert!(press(&mut app, KeyCode::Char('p')));
        assert_eq!(path(&app).as_deref(), Some("a/y.rs"));

        app.diff_area = Rect::new(30, 2, 70, 2);
        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Char('n'));
        let top = app.stream.as_ref().and_then(Stream::top_node);
        assert_eq!(
            top,
            app.tree.find_file(Path::new("b.rs")),
            "jumps the All files view"
        );
    }

    #[test]
    fn navigate_items_run_their_shortcuts() {
        let navigate = |app: &mut App, item: Action| {
            let index = MENUS[2]
                .1
                .iter()
                .position(|&i| i == item)
                .expect("a Navigate item");
            app.menu.open = Some((2, index));
            press(app, KeyCode::Enter)
        };
        let mut app = diff_app();
        app.focus = Focus::Sidebar;
        assert!(
            navigate(&mut app, Action::NextChange),
            "diff meaning, even from the sidebar"
        );
        assert!(matches!(rows(&app)[scroll(&app)], Row::Header { .. }));
        assert!(navigate(&mut app, Action::Find));
        assert!(
            app.find.is_some() && app.palette.is_none(),
            "find bar, not a search"
        );
        assert!(navigate(&mut app, Action::SearchAll));
        assert!(matches!(
            app.palette,
            Some(Palette {
                mode: Mode::Search(_),
                ..
            })
        ));
        assert!(navigate(&mut app, Action::GoToFile));
        assert!(matches!(
            app.palette,
            Some(Palette {
                mode: Mode::File(_),
                ..
            })
        ));
    }

    #[test]
    fn ctrl_k_lists_shortcuts_filters_them_and_typing_stays_text() {
        let mut app = app(&["a.rs", "b.rs"]);
        assert!(ctrl(&mut app, 'k'));
        let keys = |app: &App| match &app.palette {
            Some(Palette {
                mode: Mode::Keys(keys),
                ..
            }) => keys
                .shown
                .iter()
                .map(|&i| keys.rows[i].place)
                .collect::<Vec<_>>(),
            _ => panic!("the shortcuts screen is open"),
        };
        let all = keys(&app).len();
        assert!(all > 40, "every action and the fixed keys: {all}");
        type_text(&mut app, "sidebar");
        let places = keys(&app);
        assert!(!places.is_empty() && places.len() < all);
        assert!(places.contains(&"sidebar"));
        press(&mut app, KeyCode::Esc);
        assert!(app.palette.is_none());

        assert!(ctrl(&mut app, 'p'));
        let file = app.file;
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(
            app.file, file,
            "`n` is typed into the palette, not Next file"
        );
        assert!(ctrl(&mut app, 'b'), "Ctrl+B still works over the palette");
        assert!(app.sidebar_hidden);
    }

    /// The shortcuts screen's state, for tests.
    fn shortcuts(app: &App) -> &palette::Keys {
        match &app.palette {
            Some(Palette {
                mode: Mode::Keys(keys),
                ..
            }) => keys,
            _ => panic!("the shortcuts screen is open"),
        }
    }

    #[test]
    fn shortcuts_screen_scrolls_with_arrows_and_the_wheel() {
        let mut app = app(&["a.rs"]);
        ctrl(&mut app, 'k');
        let selected = |app: &App| shortcuts(app).list.selected();
        for _ in 0..30 {
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(selected(&app), Some(30), "arrows move the selection");

        app.palette_area = Rect::new(10, 2, 80, 20);
        if let Some(Palette {
            mode: Mode::Keys(keys),
            ..
        }) = &mut app.palette
        {
            keys.area = Rect::new(11, 5, 78, 15);
        }
        assert!(
            app.handle(&mouse(MouseEventKind::ScrollDown, 20, 8)),
            "the wheel scrolls"
        );
        assert_eq!(selected(&app), Some(33));
        assert!(app.handle(&mouse(MouseEventKind::ScrollUp, 20, 8)));
        assert_eq!(selected(&app), Some(30));
    }

    #[test]
    fn rebinding_replaces_adds_takes_over_cancels_and_resets() {
        let mut app = app_of(&["a.rs", "b.rs"]);
        let path = |app: &App| app.selected_file().map(|f| f.path.display().to_string());
        ctrl(&mut app, 'k');
        type_text(&mut app, "next file");
        assert_eq!(shortcuts(&app).selected_action(), Some(Action::NextFile));

        press(&mut app, KeyCode::Enter);
        assert!(shortcuts(&app).capture.is_some(), "waiting for a key");
        press(&mut app, KeyCode::Char('m'));
        assert!(mem::take(&mut app.pending_save));
        assert!(shortcuts(&app).capture.is_none());
        let row = &shortcuts(&app).rows[shortcuts(&app).shown[0]];
        assert_eq!((row.keys.as_str(), row.edited), ("m", true));

        key(&mut app, KeyCode::Enter, KeyModifiers::SHIFT);
        press(&mut app, KeyCode::Char('x'));
        let keys: Vec<_> = (app.keymap.keys(Action::NextFile))
            .map(|(_, c)| c.to_string())
            .collect();
        assert_eq!(keys, ["m", "x"], "Shift+Enter adds");

        press(&mut app, KeyCode::Enter);
        ctrl(&mut app, 'b');
        let taken = shortcuts(&app).capture.and_then(|c| c.taken);
        assert_eq!(taken.map(|(_, holder)| holder), Some(Action::Sidebar));
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.keymap.keys(Action::Sidebar).count(),
            0,
            "Ctrl+B moved over"
        );
        assert!(!app.sidebar_hidden, "the key was bound, not run");

        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);
        assert!(
            shortcuts(&app).capture.is_none(),
            "Esc cancels, the screen stays"
        );
        press(&mut app, KeyCode::Delete);
        let keys: Vec<_> = (app.keymap.keys(Action::NextFile))
            .map(|(_, c)| c.to_string())
            .collect();
        assert_eq!(keys, ["n"], "Del resets");

        press(&mut app, KeyCode::Esc);
        app.keymap.set(Action::NextFile, &["m".parse().unwrap()]);
        press(&mut app, KeyCode::Char('m'));
        assert_eq!(path(&app).as_deref(), Some("b.rs"), "the new key works");
    }

    fn staged_app(files: &[(&str, Staged)]) -> App {
        App::new(
            (files.iter().enumerate())
                .map(|(change, &(path, staged))| FileEntry {
                    change,
                    staged,
                    ..entry(path)
                })
                .collect(),
        )
    }

    fn ticks(app: &App) -> Vec<Tick> {
        (0..app.tree.len()).map(|row| app.row_tick(row)).collect()
    }

    fn strings(paths: &[PathBuf]) -> Vec<String> {
        paths.iter().map(|p| p.display().to_string()).collect()
    }

    #[test]
    fn space_cycles_the_checkbox_and_counts_pending() {
        use Tick::{Off, On, Part};
        let mut app = staged_app(&[
            ("f.rs", Staged::Fully),
            ("n.rs", Staged::No),
            ("p.rs", Staged::Partly),
        ]);
        assert_eq!(ticks(&app), [On, Off, Part]);
        for _ in 0..3 {
            assert!(press(&mut app, KeyCode::Char(' ')));
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(ticks(&app), [Off, On, On]);
        let Apply { stage, unstage, .. } = app.pending();
        assert_eq!(
            (strings(&stage), strings(&unstage)),
            (
                vec!["n.rs".to_owned(), "p.rs".to_owned()],
                vec!["f.rs".to_owned()]
            )
        );

        // The last Down stayed on p.rs, so one Up is n.rs.
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(ticks(&app), [Off, Off, On]);
        assert!(
            !app.marks.contains_key(Path::new("n.rs")),
            "back to the index: not pending"
        );
    }

    #[test]
    fn a_folder_checkbox_ticks_every_file_inside() {
        use Tick::{Off, On, Part};
        let mut app = staged_app(&[("a/x.rs", Staged::No), ("a/y.rs", Staged::Fully)]);
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(ticks(&app), [Part, Off, On]);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(ticks(&app), [On, On, On]);
        assert_eq!(app.marks.len(), 1, "y.rs is already staged");
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(ticks(&app), [Off, Off, Off]);
        assert_eq!(app.marks.get(Path::new("a/y.rs")), Some(&false));
    }

    #[test]
    fn stage_sends_the_pending_lists_and_nothing_when_clean() {
        let mut app = staged_app(&[("n.rs", Staged::No)]);
        assert!(!press(&mut app, KeyCode::Char('s')), "nothing to stage");
        assert!(app.pending_apply.is_none());
        press(&mut app, KeyCode::Char(' '));
        assert!(press(&mut app, KeyCode::Char('s')));
        let Apply {
            stage,
            unstage,
            commit,
        } = app.pending_apply.take().expect("staged");
        assert_eq!(commit, None);
        assert_eq!(
            (strings(&stage), unstage.len()),
            (vec!["n.rs".to_owned()], 0)
        );
    }

    #[test]
    fn refresh_drops_marks_that_now_match_the_index() {
        let mut app = staged_app(&[
            ("gone.rs", Staged::No),
            ("n.rs", Staged::No),
            ("m.rs", Staged::No),
        ]);
        for _ in 0..3 {
            press(&mut app, KeyCode::Char(' '));
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(app.marks.len(), 3);
        let next = |path: &str, staged| FileEntry {
            staged,
            ..entry(path)
        };
        app.refresh(vec![next("m.rs", Staged::No), next("n.rs", Staged::Fully)]);
        assert_eq!(
            app.marks.keys().collect::<Vec<_>>(),
            [Path::new("m.rs")],
            "staged elsewhere, or gone"
        );
    }

    #[test]
    fn clicking_the_checkbox_toggles_without_opening_the_file() {
        let mut app = staged_app(&[("a.rs", Staged::No), ("b.rs", Staged::No)]);
        app.sidebar = Rect::new(0, 2, 30, 10);
        app.stage_button = Rect::new(18, 11, 11, 1);
        let click = |column, row| mouse(MouseEventKind::Down(MouseButton::Left), column, row);
        assert!(app.handle(&click(3, 3)), "b.rs's checkbox");
        assert_eq!(
            selected(&app).as_deref(),
            Some("a.rs"),
            "the diff stays on a.rs"
        );
        assert_eq!(ticks(&app), [Tick::Off, Tick::On]);
        assert!(
            !app.handle(&click(5, 11)),
            "the stage bar row holds no file"
        );
        assert!(app.handle(&click(20, 11)), "the Stage button");
        assert!(app.pending_apply.is_some());
    }

    fn ctrl_enter(app: &mut App) -> bool {
        app.handle(&Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::CONTROL,
        )))
    }

    #[test]
    fn writing_takes_letters_as_text_and_esc_keeps_the_message() {
        let mut app = staged_app(&[("n.rs", Staged::No)]);
        assert!(press(&mut app, KeyCode::Char('i')));
        assert!(app.writing);
        for c in "sqi".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        assert_eq!(app.commit_msg.text().as_str(), "sqi");
        assert!(
            !app.quit && app.marks.is_empty(),
            "letters are text, not keys"
        );
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.commit_msg.text().as_str(), "sqi\n");
        press(&mut app, KeyCode::Esc);
        assert!(!app.writing);
        assert_eq!(
            app.commit_msg.text().as_str(),
            "sqi\n",
            "Esc keeps the message"
        );
    }

    #[test]
    fn the_commit_box_edits_anywhere_like_a_text_field() {
        let mut app = staged_app(&[("n.rs", Staged::No)]);
        press(&mut app, KeyCode::Char('i'));
        for c in "fix bug".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Home);
        assert!(press(&mut app, KeyCode::Right));
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Char(':'));
        assert_eq!(app.commit_msg.text().as_str(), "fix: bug");
        press(&mut app, KeyCode::End);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('b'));
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Up);
        assert_eq!(
            app.commit_msg.cursor(),
            (0, 1),
            "Up keeps the column past the empty line"
        );
        press(&mut app, KeyCode::End);
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Delete);
        assert_eq!(app.commit_msg.text().as_str(), "fix: bu\n\nb");
        assert!(app.writing, "arrows stay in the box");
    }

    #[test]
    fn commit_sends_ticks_and_the_message_and_refuses_blank() {
        let mut app = staged_app(&[("n.rs", Staged::No)]);
        assert!(ctrl_enter(&mut app));
        assert!(
            matches!(app.notice, Some(Notice::Error(_))),
            "blank message"
        );
        assert!(app.writing, "the box opens for the message");
        assert!(app.pending_apply.is_none());

        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Char('i'));
        for c in "fix".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        assert!(ctrl_enter(&mut app), "Ctrl+Enter commits while writing");
        assert_eq!(
            app.pending_apply,
            Some(Apply {
                stage: vec!["n.rs".into()],
                unstage: Vec::new(),
                commit: Some("fix".into()),
            })
        );
    }

    #[test]
    fn finish_clears_on_success_and_keeps_everything_on_error() {
        let mut app = staged_app(&[("n.rs", Staged::No)]);
        press(&mut app, KeyCode::Char(' '));
        (app.commit_msg, app.writing) = (Field::multi("fix"), true);
        app.finish(Err("commit signing is on".into()));
        assert_eq!(
            app.notice,
            Some(Notice::Error("commit signing is on".into()))
        );
        assert_eq!(
            (app.commit_msg.text().as_str(), app.marks.len()),
            ("fix", 1)
        );

        app.finish(Ok(Some("abc1234".into())));
        assert_eq!(app.notice, Some(Notice::Done("committed abc1234".into())));
        assert!(app.commit_msg.text().is_empty() && !app.writing && app.marks.is_empty());
        assert!(app.pending_reload);
    }

    #[test]
    fn clicking_the_box_writes_and_clicking_away_stops() {
        let mut app = staged_app(&[("n.rs", Staged::No)]);
        app.commit_box = Rect::new(0, 10, 30, 4);
        app.commit_button = Rect::new(20, 15, 8, 1);
        let click = |column, row| mouse(MouseEventKind::Down(MouseButton::Left), column, row);
        assert!(app.handle(&click(5, 11)));
        assert!(app.writing);
        assert!(
            app.handle(&click(50, 40)),
            "a click elsewhere stops writing"
        );
        assert!(!app.writing);
        assert!(app.handle(&click(22, 15)), "the Commit button");
        assert!(
            matches!(app.notice, Some(Notice::Error(_))),
            "still no message"
        );
    }

    #[test]
    fn page_down_in_the_sidebar_moves_one_visible_page_of_files() {
        let names: Vec<String> = (0..40).map(|i| format!("f{i:02}.rs")).collect();
        let mut app = app(&names.iter().map(String::as_str).collect::<Vec<_>>());
        crate::ui::render(&mut app, 120, 24);
        let visible = usize::from(app.sidebar_rows.height);
        assert!(
            visible < usize::from(app.sidebar.height),
            "the commit panel takes rows"
        );
        press(&mut app, KeyCode::PageDown);
        assert_eq!(
            app.cursor, visible,
            "a page is the file list, not the whole sidebar"
        );
    }

    #[test]
    fn staging_off_hides_ticks_and_space_opens_folders() {
        let mut app = staged_app(&[("a/x.rs", Staged::No), ("b.rs", Staged::No)]);
        press(&mut app, KeyCode::Char(' '));
        app.commit_msg = Field::multi("wip");
        assert!(app.run(Action::Staging));
        assert!(!app.can_stage() && mem::take(&mut app.pending_save));
        assert!(app.marks.is_empty(), "hidden ticks are dropped");
        assert_eq!(app.commit_msg.text().as_str(), "wip", "the message is kept");

        press(&mut app, KeyCode::Char('g'));
        assert!(
            press(&mut app, KeyCode::Char(' ')),
            "Space closes the folder"
        );
        assert_eq!(labels(&app), ["a/", "b.rs"]);
        for key in [KeyCode::Char('s'), KeyCode::Char('i')] {
            assert!(!press(&mut app, key));
        }
        assert!(!ctrl_enter(&mut app));
        assert!(!app.writing && app.pending_apply.is_none());

        app.sidebar = Rect::new(0, 2, 30, 10);
        let click = mouse(MouseEventKind::Down(MouseButton::Left), 3, 3);
        assert!(app.handle(&click), "the checkbox column selects the row");
        assert!(app.marks.is_empty());
        assert_eq!(selected(&app).as_deref(), Some("b.rs"));
    }

    #[test]
    fn read_only_blocks_staging_and_leaves_the_setting_alone() {
        let mut app = staged_app(&[("n.rs", Staged::No)]);
        app.read_only = true;
        assert!(!app.can_stage());
        assert!(app.run(Action::Staging));
        assert!(matches!(app.notice, Some(Notice::Error(_))));
        assert!(
            app.staging && !app.pending_save,
            "the preference is untouched"
        );

        app.commit_msg = Field::multi("fix");
        app.marks.insert("n.rs".into(), true);
        assert!(!app.run(Action::Stage) && !app.run(Action::Commit));
        assert!(app.pending_apply.is_none(), "no git writes");
        assert!(
            app.settings().general.staging,
            "saving never writes read-only"
        );
    }

    #[test]
    fn theme_picker_switches_saves_and_round_trips() {
        let mut app = app(&["a.rs"]);
        assert!(app.run(Action::Theme));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.theme, Theme::GithubLight);
        assert!(app.palette.is_none(), "Enter closes the picker");
        assert!(mem::take(&mut app.pending_save));

        app.run(Action::Theme);
        assert!(press(&mut app, KeyCode::Up));
        assert_eq!(app.shown_theme(), Theme::GithubDark, "moving previews");
        assert_eq!(app.theme, Theme::GithubLight, "but doesn't pick");
        app.handle(&mouse(MouseEventKind::ScrollDown, 0, 0));
        assert_eq!(app.shown_theme(), Theme::ALL[3], "the wheel previews too");
        press(&mut app, KeyCode::Esc);
        assert_eq!(
            app.shown_theme(),
            Theme::GithubLight,
            "Esc drops the preview"
        );
        assert!(!app.pending_save);

        app.run(Action::Theme);
        app.palette_area = Rect::new(9, 3, 32, 6);
        if let Some(Palette {
            mode: Mode::Themes(themes),
            ..
        }) = &mut app.palette
        {
            themes.area = Rect::new(10, 5, 30, 2);
        }
        app.handle(&mouse(MouseEventKind::Down(MouseButton::Left), 12, 5));
        assert_eq!(app.theme, Theme::GithubDark, "a click picks the row");

        app.run(Action::Theme);
        for c in "LATTE".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        assert_eq!(app.shown_theme(), Theme::CatppuccinLatte, "typing filters");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.theme, Theme::CatppuccinLatte);

        let mut fresh = self::app(&["a.rs"]);
        assert!(fresh.apply(app.settings()).is_empty());
        assert_eq!(fresh.theme, Theme::CatppuccinLatte);
    }

    #[test]
    fn saved_layout_round_trips_through_settings() {
        let mut app = app(&["a.rs"]);
        app.view_choice = Some(View::Unified);
        app.sidebar_right = true;
        app.context = u32::MAX;
        app.word_highlights = false;
        assert!(app.run(Action::SaveLayout));
        assert!(mem::take(&mut app.pending_save));
        app.run(Action::Syntax);

        let mut fresh = self::app(&["a.rs"]);
        assert!(fresh.apply(app.settings()).is_empty());
        assert_eq!(fresh.view_choice, Some(View::Unified));
        assert!(fresh.sidebar_right && !fresh.word_highlights && !fresh.syntax);
        assert_eq!(fresh.context, u32::MAX);

        fresh.run(Action::ResetLayout);
        assert_eq!(
            (fresh.saved_layout.as_ref(), fresh.view_choice),
            (None, None)
        );
        assert!(!fresh.sidebar_right && fresh.word_highlights);
        assert_eq!(fresh.context, CONTEXT_LINES);
    }

    /// Opens the View menu on `item` and presses Enter.
    fn pick(app: &mut App, item: Action) {
        let index = MENUS[1]
            .1
            .iter()
            .position(|&i| i == item)
            .expect("a View item");
        app.menu.open = Some((1, index));
        press(app, KeyCode::Enter);
    }

    #[test]
    fn view_menu_and_v_pick_the_view() {
        let mut app = app(&["a.rs"]);
        press(&mut app, KeyCode::F(10));
        press(&mut app, KeyCode::Left);
        let last = MENUS.len() - 1;
        assert_eq!(
            app.menu.open,
            Some((last, 0)),
            "Left wraps to the last menu"
        );
        press(&mut app, KeyCode::Right);
        assert_eq!(app.menu.open, Some((0, 0)), "Right wraps to File");
        press(&mut app, KeyCode::Right);
        assert_eq!(app.menu.open, Some((1, 0)), "then View");
        press(&mut app, KeyCode::Up);
        let last = MENUS[1].1.len() - 1;
        assert_eq!(app.menu.open, Some((1, last)), "Up wraps to the last item");
        press(&mut app, KeyCode::Esc);

        pick(&mut app, Action::Sidebar);
        assert!(app.sidebar_hidden);
        assert_eq!(app.focus, Focus::Diff);
        pick(&mut app, Action::Sidebar);
        assert!(!app.sidebar_hidden, "shown again");
        pick(&mut app, Action::SidebarRight);
        assert!(app.sidebar_right);

        pick(&mut app, Action::Show(Some(View::Unified)));
        assert_eq!(app.view_choice, Some(View::Unified));
        app.fit_view(200);
        assert_eq!(app.view, View::Unified, "the pick wins over the width");
        press(&mut app, KeyCode::Char('v'));
        assert_eq!(app.view_choice, Some(View::Split));

        let auto = MENUS[1]
            .1
            .iter()
            .position(|&i| i == Action::Show(None))
            .unwrap();
        app.menu.open = Some((1, auto));
        press(&mut app, KeyCode::Down);
        assert_eq!(
            MENUS[1].1[app.menu.open.unwrap().1],
            Action::ExpandAll,
            "Down from Auto skips the separator"
        );
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.context, u32::MAX);
        pick(&mut app, Action::Show(None));
        assert_eq!(app.view_choice, None, "Auto");
        app.fit_view(80);
        assert_eq!(app.view, View::Unified, "Auto follows the width again");
    }

    #[test]
    fn handle_sits_on_the_inner_edge_on_either_side() {
        assert_eq!(handle(Rect::new(0, 0, 30, 10), false, 44), (29, 45));
        assert_eq!(handle(Rect::new(70, 0, 30, 10), true, 55), (70, 45));
    }

    #[test]
    fn dragging_a_right_sidebar_grows_it_leftward() {
        let mut app = app(&["a.rs"]);
        app.sidebar_right = true;
        app.sidebar = Rect::new(70, 0, 30, 10);
        let left = MouseButton::Left;
        assert!(app.handle(&mouse(MouseEventKind::Down(left), 70, 1)));
        assert!(app.resizing);
        assert!(app.handle(&mouse(MouseEventKind::Drag(left), 55, 1)));
        assert_eq!(app.sidebar_width, Some(45));
    }

    #[test]
    fn next_context_steps_by_three_and_resets_from_all() {
        assert_eq!(next_context(3, true), 6);
        assert_eq!(next_context(3, false), 0);
        assert_eq!(next_context(0, false), 0);
        assert_eq!(next_context(u32::MAX, false), CONTEXT_LINES);
        assert_eq!(next_context(u32::MAX, true), u32::MAX);
    }

    #[test]
    fn fold_context_and_word_keys_reshape_the_rows() {
        let old = (1..=40).fold(String::new(), |text, i| text + &format!("line {i}\n"));
        let new = old.replace("line 20\n", "twenty\n");
        let mut app = app(&["a.rs"]);
        app.show(Some(Ok(FileDiff::new(old.into_bytes(), new.into_bytes()))));
        let folds = |app: &App| {
            (view(app).rows.iter())
                .filter(|r| matches!(r, Row::Fold { .. }))
                .count()
        };
        app.fit_view(140);
        assert_eq!(folds(&app), 2);
        let rows = view(&app).rows.len();

        press(&mut app, KeyCode::Char('+'));
        app.fit_view(140);
        assert_eq!(app.context, 6);
        assert_eq!(
            view(&app).rows.len(),
            rows + 6,
            "three more lines on each side"
        );

        press(&mut app, KeyCode::Char('e'));
        app.fit_view(140);
        assert_eq!(folds(&app), 0, "expand all");
        press(&mut app, KeyCode::Char('c'));
        app.fit_view(140);
        assert_eq!(
            (app.context, folds(&app)),
            (CONTEXT_LINES, 2),
            "collapse all"
        );

        press(&mut app, KeyCode::Char('w'));
        assert!(!app.word_highlights);
    }

    #[test]
    fn view_follows_the_pane_width_until_picked() {
        assert_eq!(view_for(89, None), View::Unified);
        assert_eq!(view_for(90, None), View::Split);
        assert_eq!(view_for(40, Some(View::Split)), View::Split);
    }

    #[test]
    fn fitting_a_narrow_pane_rebuilds_unified_rows_at_the_same_line() {
        let old = (1..=40).fold(String::new(), |text, i| text + &format!("line {i}\n"));
        let new = old.replace("line 20\n", "twenty\n");
        let mut app = app(&["a.rs"]);
        app.show(Some(Ok(FileDiff::new(old.into_bytes(), new.into_bytes()))));
        app.fit_view(140);
        let split = view(&app).rows.len();
        let change = view(&app)
            .rows
            .iter()
            .position(|r| {
                matches!(
                    r,
                    Row::Line {
                        kind: zdiff_core::Kind::Change,
                        ..
                    }
                )
            })
            .expect("a change row");
        if let DiffPane::Loaded(v) = &mut app.diff {
            v.scroll = change;
        }
        app.fit_view(80);
        assert_eq!(app.view, View::Unified);
        let v = view(&app);
        assert_eq!(v.rows.len(), split + 1, "the paired change splits in two");
        assert!(matches!(
            v.rows[v.scroll],
            Row::Line { new: Some(19), .. } | Row::Line { old: Some(19), .. }
        ));
    }

    #[test]
    fn click_selects_files_and_toggles_folders() {
        let mut app = app(&["a/y.rs", "a/z.rs"]);
        app.sidebar = Rect::new(0, 2, 30, 10);
        // Column 12 is on the name, past the checkbox.
        let click = |row| mouse(MouseEventKind::Down(MouseButton::Left), 12, row);

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
            app.palette.as_ref().map(|p| p.field.text()).as_deref(),
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

    fn palette_text(app: &App) -> String {
        match &app.palette {
            Some(Palette {
                mode: Mode::Search(search),
                ..
            }) => search.fields.each_ref().map(Field::text).join("|"),
            Some(palette) => palette.field.text(),
            None => String::new(),
        }
    }

    #[test]
    fn text_keys_go_to_the_field_and_commands_still_reach_zdiff() {
        let mut app = diff_app();
        ctrl(&mut app, 'f');
        type_text(&mut app, "fif teen");
        assert!(ctrl(&mut app, 'a'), "Ctrl+A, what Cmd+Left sends");
        type_text(&mut app, "x");
        let alt_b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT);
        app.handle(&Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)));
        assert!(
            app.handle(&Event::Key(alt_b)),
            "Alt+B, what Option+Left sends"
        );
        type_text(&mut app, "y");
        assert_eq!(
            app.find.as_ref().map(|f| f.query.text.as_str()),
            Some("xfif yteen"),
            "the search follows the field"
        );
        assert!(ctrl(&mut app, 'k'), "Ctrl+K still opens the shortcuts");
        assert!(matches!(
            app.palette,
            Some(Palette {
                mode: Mode::Keys(_),
                ..
            })
        ));
    }

    fn key_with(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> bool {
        app.handle(&Event::Key(KeyEvent::new(code, modifiers)))
    }

    #[test]
    fn copy_toasts_quote_the_start_of_the_text() {
        assert_eq!(snippet("fix bug"), "\"fix bug\"");
        assert_eq!(snippet(&"x".repeat(30)), format!("\"{}…\"", "x".repeat(24)));
        assert_eq!(snippet("first\nsecond\nthird"), "\"first…\" (3 lines)");
    }

    #[test]
    fn searching_history_and_copying_the_query_toasts_too() {
        let mut app = history_open();
        press(&mut app, KeyCode::Char('/'));
        type_into(&mut app, "theme");
        key_with(&mut app, KeyCode::Home, KeyModifiers::SHIFT);
        assert!(key_with(
            &mut app,
            KeyCode::Char('c'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        ));
        assert_eq!(app.pending_copy.as_deref(), Some("theme"));
        assert_eq!(
            app.toast.as_ref().map(|t| t.text.as_str()),
            Some("Copied \"theme\"")
        );
    }

    #[test]
    fn cutting_in_the_find_bar_copies_and_finds_again() {
        let mut app = diff_app();
        ctrl(&mut app, 'f');
        type_text(&mut app, "fif teen");
        key_with(
            &mut app,
            KeyCode::Left,
            KeyModifiers::ALT | KeyModifiers::SHIFT,
        );
        assert!(ctrl(&mut app, 'x'));
        assert_eq!(app.pending_copy.as_deref(), Some("teen"));
        let toast = app.toast.as_ref().map(|t| t.text.as_str());
        assert_eq!(
            toast,
            Some("Copied \"teen\""),
            "text fields confirm copies too"
        );
        assert_eq!(
            app.find.as_ref().map(|f| f.query.text.as_str()),
            Some("fif ")
        );

        key_with(&mut app, KeyCode::Home, KeyModifiers::SHIFT);
        assert!(
            press(&mut app, KeyCode::Esc),
            "the first Esc drops the selection"
        );
        assert!(app.find.is_some(), "the bar stays open");
        press(&mut app, KeyCode::Esc);
        assert!(app.find.is_none());
    }

    #[test]
    fn pasting_goes_to_the_focused_field() {
        let paste = |app: &mut App, text: &str| app.handle(&Event::Paste(text.into()));
        let mut app = diff_app();
        assert!(!paste(&mut app, "x"), "nothing focused");

        ctrl(&mut app, 'f');
        type_text(&mut app, "fif");
        assert!(paste(&mut app, "\nteen"));
        assert_eq!(
            app.find.as_ref().map(|f| f.query.text.as_str()),
            Some("fif teen"),
            "one line: the pasted newline wasn't Enter"
        );
        press(&mut app, KeyCode::Esc);

        ctrl(&mut app, 'g');
        paste(&mut app, "line 42!");
        assert_eq!(palette_text(&app), "42", "a line number keeps its digits");
        press(&mut app, KeyCode::Esc);

        app.run(Action::SearchAll);
        app.pending_search = None;
        paste(&mut app, "needle");
        assert!(app.pending_search.is_some(), "pasting searches like typing");
    }

    #[test]
    fn the_find_bar_and_popups_edit_anywhere_like_text_fields() {
        let mut app = diff_app();
        ctrl(&mut app, 'f');
        type_text(&mut app, "fiten");
        press(&mut app, KeyCode::Home);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Right);
        assert!(press(&mut app, KeyCode::Char('f')));
        press(&mut app, KeyCode::End);
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Delete);
        // fi|ten, type f: fif|ten; End, Left, Left: fift|en; Delete takes the e.
        assert_eq!(
            app.find.as_ref().map(|f| f.query.text.as_str()),
            Some("fiftn")
        );
        press(&mut app, KeyCode::Esc);

        ctrl(&mut app, 'g');
        type_text(&mut app, "12");
        press(&mut app, KeyCode::Home);
        type_text(&mut app, "-");
        press(&mut app, KeyCode::Right);
        type_text(&mut app, "-");
        assert_eq!(palette_text(&app), "-12", "a sign only goes first");
        press(&mut app, KeyCode::Esc);

        app.run(Action::SearchAll);
        let typed = palette_text(&app)
            .split('|')
            .next()
            .unwrap_or_default()
            .to_owned();
        type_text(&mut app, "xy");
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Backspace);
        assert_eq!(palette_text(&app), format!("{typed}y||"));
        press(&mut app, KeyCode::Tab);
        let include = palette_text(&app).split('|').nth(1).map(str::to_owned);
        type_text(&mut app, "src");
        press(&mut app, KeyCode::Home);
        press(&mut app, KeyCode::Delete);
        let edited = format!("{}src", include.unwrap_or_default())[1..].to_owned();
        assert_eq!(
            palette_text(&app),
            format!("{typed}y|{edited}|"),
            "each field has its own cursor, starting at its end"
        );
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
            staged: zdiff_core::Staged::No,
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
        assert_eq!(global(&mut app).fields[1].text(), "src");
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

    /// App showing a changed `icon.svg`, with a stand-in image protocol when `picker`.
    fn svg_app(picker: bool) -> App {
        let [old, new] = crate::preview::SVGS;
        let mut app = app(&["icon.svg"]);
        app.images = picker.then(Picker::halfblocks);
        app.show(Some(Ok(FileDiff::new(old.into(), new.into()))));
        app.focus = Focus::Diff;
        app
    }

    #[test]
    fn r_switches_an_svg_between_code_and_pictures() {
        let mut app = svg_app(true);
        let preview = |app: &App| matches!(&app.diff, DiffPane::Loaded(v) if v.preview.is_some());
        assert!(!preview(&app), "code first, nothing rendered");
        assert!(press(&mut app, KeyCode::Char('r')));
        assert_eq!(app.svg_view, SvgView::Preview);
        assert!(app.picture_shown());
        app.find = Some(Find::default());
        assert!(!app.picture_shown(), "the find bar shows the code");
        app.find = None;
        assert!(press(&mut app, KeyCode::Char('r')));
        assert_eq!(app.svg_view, SvgView::Code);
        assert!(!preview(&app), "pixels freed");

        let mut text = diff_app();
        assert!(!press(&mut text, KeyCode::Char('r')), "not an SVG");
    }

    #[test]
    fn fonts_load_only_for_an_svg_with_text_shown_as_pictures() {
        let mut icon = svg_app(true);
        press(&mut icon, KeyCode::Char('r'));
        assert!(icon.picture_shown());
        assert!(icon.fonts.is_none(), "an icon needs no fonts");

        let text =
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2"><text>Hi</text></svg>"#;
        let mut app = app(&["t.svg"]);
        app.images = Some(Picker::halfblocks());
        app.show(Some(Ok(FileDiff::new(Vec::new(), text.into()))));
        assert!(app.fonts.is_none(), "code view draws nothing");
        press(&mut app, KeyCode::Char('r'));
        assert!(app.fonts.is_some());
    }

    #[test]
    fn a_bigger_pane_remakes_shrunk_pictures_and_a_smaller_one_does_not() {
        use crate::preview::png;
        let mut app = app(&["logo.png"]);
        app.images = Some(Picker::halfblocks());
        // Halfblocks cells are 10×20 pixels.
        app.diff_area = Rect::new(0, 0, 10, 5);
        app.show(Some(Ok(FileDiff::new(png(400, 300), png(400, 300)))));
        let fitted = |app: &App| match &app.diff {
            DiffPane::Loaded(view) => view.preview.as_ref().map(|p| p.fitted),
            _ => None,
        };
        assert_eq!(fitted(&app), Some((100, 100)));
        app.diff_area = Rect::new(0, 0, 40, 15);
        app.fit_preview();
        assert_eq!(fitted(&app), Some((400, 300)));
        app.diff_area = Rect::new(0, 0, 10, 5);
        app.fit_preview();
        assert_eq!(fitted(&app), Some((400, 300)), "shrinking remakes nothing");
    }

    #[test]
    fn the_svg_default_setting_opens_svgs_as_pictures() {
        let mut app = svg_app(true);
        assert!(app.run(Action::SvgPreviewDefault));
        assert_eq!(app.svg_view, SvgView::Preview);
        assert!(app.picture_shown(), "the shown SVG switched too");
        assert!(app.pending_save && app.settings().general.svg_preview);

        let mut fresh = App::new(Vec::new());
        let mut settings = Settings::default();
        settings.general.svg_preview = true;
        fresh.apply(settings);
        assert_eq!(fresh.svg_view, SvgView::Preview);
    }

    #[test]
    fn an_svgz_always_previews_and_has_no_code_to_switch_to() {
        let zipped = crate::preview::gzip(crate::preview::SVGS[0].as_bytes());
        let mut app = app(&["icon.svgz"]);
        app.show(Some(Ok(FileDiff::new(zipped.clone(), zipped))));
        let DiffPane::Loaded(view) = &app.diff else {
            panic!("loaded")
        };
        let caption = view.preview.as_ref().map(Preview::caption);
        assert!(caption.is_some_and(|c| c.starts_with("SVG changed · 4×2")));
        assert!(!app.svg_has_code());
        assert!(!press(&mut app, KeyCode::Char('r')));
    }

    #[test]
    fn r_explains_why_an_svg_cant_be_previewed() {
        let notice = |app: &App| match &app.notice {
            Some(Notice::Error(text)) => text.clone(),
            other => panic!("{other:?}"),
        };
        let mut app = svg_app(false);
        assert!(press(&mut app, KeyCode::Char('r')));
        assert_eq!(app.svg_view, SvgView::Code);
        assert!(notice(&app).contains("can't draw images"));

        let mut app = svg_app(true);
        app.image_previews = false;
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(app.svg_view, SvgView::Code);
        assert!(notice(&app).contains("previews are off"));
    }

    #[test]
    fn shift_l_opens_history_and_moving_asks_for_commits_and_files() {
        use crate::history::{PAGE, commit};
        let mut app = app(&["a.rs"]);
        assert!(press(&mut app, KeyCode::Char('L')));
        assert_eq!(mem::take(&mut app.pending_history), [Want::Commits(PAGE)]);
        app.history_commits(vec![commit("c1", &["c2"]), commit("c2", &[])]);
        let wants = mem::take(&mut app.pending_history);
        assert!(
            matches!(&wants[..], [Want::Files(c)] if c.id == "c1"),
            "{wants:?}"
        );
        assert!(
            press(&mut app, KeyCode::Char('j')),
            "the popup takes the keys"
        );
        let wants = mem::take(&mut app.pending_history);
        assert!(
            matches!(&wants[..], [Want::Files(c)] if c.id == "c2"),
            "{wants:?}"
        );
        app.history_files("c2", vec![entry("b.rs")]);
        let wants = mem::take(&mut app.pending_history);
        let [Want::Preview { commit, path }] = &wants[..] else {
            panic!("the first file's diff: {wants:?}")
        };
        assert_eq!(
            (commit.id.as_str(), path.as_path()),
            ("c2", Path::new("b.rs"))
        );
        assert!(press(&mut app, KeyCode::Enter), "into the files");
        assert!(!press(&mut app, KeyCode::Enter), "no preview loaded yet");
        let diff = FileDiff::new(b"a\n".to_vec(), b"b\n".to_vec());
        app.history_preview("c2", Path::new("b.rs"), Ok(diff));
        assert!(press(&mut app, KeyCode::Enter), "into the preview");
        assert_eq!(app.history.as_ref().map(|h| h.pane), Some(Pane::Preview));
        assert!(press(&mut app, KeyCode::Esc), "back to the files");
        assert!(app.history.is_some());
        assert!(press(&mut app, KeyCode::Char('o')), "open in the main view");
        let wants = mem::take(&mut app.pending_history);
        let [
            Want::Open {
                commit,
                file,
                back,
                worktree: false,
            },
        ] = &wants[..]
        else {
            panic!("{wants:?}")
        };
        assert_eq!(commit.id, "c2");
        assert_eq!(
            (file.as_deref(), back.as_deref()),
            (Some(Path::new("b.rs")), Some(Path::new("a.rs")))
        );
        press(&mut app, KeyCode::Esc);
        assert!(
            press(&mut app, KeyCode::Esc),
            "files, then commits, then closed"
        );
        assert!(
            app.history.is_none() && !app.quit,
            "Esc closes the popup only"
        );
    }

    /// App with the history popup open on three commits, `c1` selected, drawn once.
    fn history_open() -> App {
        use crate::history::commit;
        let mut app = app(&["a.rs"]);
        press(&mut app, KeyCode::Char('L'));
        let mut commits = vec![
            commit("c1", &["c2"]),
            commit("c2", &["c3"]),
            commit("c3", &[]),
        ];
        commits[1].summary = "fix the theme".into();
        app.history_commits(commits);
        app.pending_history.clear();
        crate::ui::render(&mut app, 140, 30);
        app
    }

    fn type_into(app: &mut App, text: &str) {
        for ch in text.chars() {
            press(app, KeyCode::Char(ch));
        }
    }

    #[test]
    fn slash_searches_arrows_move_while_typing_and_esc_clears_first() {
        let mut app = history_open();
        assert!(press(&mut app, KeyCode::Char('/')));
        type_into(&mut app, "theme");
        let history = app.history.as_ref().expect("open");
        assert_eq!(
            history.query(),
            "theme",
            "letters go to the box, not to j/k or w"
        );
        assert_eq!((history.shown.as_slice(), history.selected), (&[1][..], 1));
        assert!(press(&mut app, KeyCode::Enter));
        assert!(
            !app.history.as_ref().is_some_and(|h| h.typing),
            "Enter keeps the filter"
        );
        assert!(press(&mut app, KeyCode::Esc));
        assert!(
            app.history.as_ref().is_some_and(|h| h.search.is_none()),
            "Esc clears it"
        );
        assert!(press(&mut app, KeyCode::Esc));
        assert!(app.history.is_none(), "then closes");
    }

    #[test]
    fn y_copies_the_hash_and_w_opens_against_the_worktree() {
        let mut app = history_open();
        assert!(press(&mut app, KeyCode::Char('y')));
        assert_eq!(app.pending_copy.take().as_deref(), Some("c1"));
        assert_eq!(
            app.toast.as_ref().map(|t| t.text.as_str()),
            Some("Copied c1")
        );
        let screen = crate::ui::render(&mut app, 140, 30);
        let toast = screen.iter().position(|row| row.contains("✓ Copied c1 "));
        let toast = toast.expect("drawn over the popup");
        assert!(
            toast >= screen.len() - 4,
            "bottom, above the footer: {screen:#?}"
        );
        assert!(
            screen[toast].trim_end().ends_with("│"),
            "right edge: {screen:#?}"
        );
        assert!(press(&mut app, KeyCode::Char('w')));
        assert!(matches!(
            &app.pending_history[..],
            [Want::Open { worktree: true, .. }]
        ));
    }

    #[test]
    fn the_mouse_presses_buttons_copies_the_hash_and_drives_the_search() {
        let mut app = history_open();
        let down = MouseEventKind::Down(MouseButton::Left);
        let click = |app: &mut App, area: Rect| app.handle(&mouse(down, area.x, area.y));
        let button = |app: &App, which: Button| {
            let history = app.history.as_ref().expect("open");
            (history.buttons.iter())
                .find(|(_, b)| *b == which)
                .expect("drawn")
                .0
        };
        let copy = button(&app, Button::Copy);
        assert!(click(&mut app, copy));
        assert_eq!(app.pending_copy.take().as_deref(), Some("c1"));
        let hash = app.history.as_ref().expect("open").copy_areas[0];
        assert!(click(&mut app, hash), "the hash in the details copies too");
        assert_eq!(app.pending_copy.take().as_deref(), Some("c1"));
        let worktree = button(&app, Button::Worktree);
        assert!(click(&mut app, worktree));
        assert!(matches!(
            &app.pending_history[..],
            [Want::Open { worktree: true, .. }]
        ));

        let search = button(&app, Button::Search);
        assert!(click(&mut app, search));
        type_into(&mut app, "theme");
        crate::ui::render(&mut app, 140, 30);
        let clear = app.history.as_ref().expect("open").clear_area;
        assert!(click(&mut app, clear));
        assert!(
            app.history
                .as_ref()
                .is_some_and(|h| h.search.is_none() && !h.typing)
        );
    }

    #[test]
    fn a_viewed_commit_is_read_only_and_esc_goes_back_to_the_working_tree() {
        let mut app = app(&["a.rs", "b.rs"]);
        assert!(app.can_stage());
        app.refresh(vec![entry("x.rs"), entry("y.rs")]);
        let commit = crate::history::commit("c1", &["c0"]);
        app.viewing_opened(
            (&commit, false),
            Some(Path::new("y.rs")),
            Some("b.rs".into()),
        );
        assert!(!app.can_stage(), "no staging on a commit");
        assert_eq!(
            app.selected_file().map(|f| f.path.clone()),
            Some("y.rs".into())
        );
        let screen = crate::ui::render(&mut app, 100, 8);
        let footer = screen.last().expect("a footer");
        assert!(
            footer.contains("c1^ → c1 · read-only") && footer.contains("✕ back to worktree"),
            "{footer}"
        );

        assert!(press(&mut app, KeyCode::Esc));
        assert_eq!(mem::take(&mut app.pending_history), [Want::Back]);
        assert!(!app.quit, "Esc goes back instead of quitting");
        app.refresh(vec![entry("a.rs"), entry("b.rs")]);
        app.viewing_closed();
        assert!(app.viewing.is_none() && app.can_stage());
        assert_eq!(
            app.selected_file().map(|f| f.path.clone()),
            Some("b.rs".into())
        );
    }

    #[test]
    fn the_footer_button_goes_back_too() {
        let mut app = app(&["a.rs"]);
        app.viewing_opened((&crate::history::commit("c1", &[]), false), None, None);
        crate::ui::render(&mut app, 100, 8);
        let button = app.back_button;
        assert!(!button.is_empty());
        let down = MouseEventKind::Down(MouseButton::Left);
        assert!(app.handle(&mouse(down, button.x, button.y)));
        assert_eq!(app.pending_history, [Want::Back]);
    }

    #[test]
    fn a_patch_has_no_history() {
        let mut app = app(&["a.rs"]);
        app.patch_mode = true;
        assert!(press(&mut app, KeyCode::Char('L')));
        assert!(app.history.is_none() && app.pending_history.is_empty());
        assert!(matches!(&app.notice, Some(Notice::Error(text)) if text.contains("no history")));
    }

    #[test]
    fn o_cycles_image_compare_modes_and_arrows_move_the_mix() {
        let mut app = image_app((3, 2));
        assert!(!press(&mut app, KeyCode::Char('l')), "2-up has no mix");
        assert!(press(&mut app, KeyCode::Char('o')));
        assert_eq!(app.image_compare, Compare::Swipe);
        assert!(press(&mut app, KeyCode::Char('l')));
        assert_eq!(app.mix, 60);
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Char('h'));
        assert_eq!(app.mix, 40);
        app.mix = 100;
        assert!(!press(&mut app, KeyCode::Right), "clamped");
        app.mix = 0;
        assert!(!press(&mut app, KeyCode::Left), "clamped");
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(app.image_compare, Compare::Onion);
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(app.image_compare, Compare::TwoUp);

        let mut text = diff_app();
        assert!(!press(&mut text, KeyCode::Char('o')), "not an image");
    }

    #[test]
    fn turning_image_previews_off_frees_the_pictures_and_on_decodes_them() {
        let mut app = image_app((3, 2));
        let drawable = |app: &App| {
            matches!(&app.diff, DiffPane::Loaded(view)
                if view.preview.as_ref().is_some_and(Preview::drawable))
        };
        assert!(drawable(&app));
        assert!(app.run(Action::ImagePreviews));
        assert!(!drawable(&app));
        assert!(app.pending_save && !app.settings().general.images);
        app.run(Action::ImagePreviews);
        assert!(drawable(&app));
    }

    #[test]
    fn clicking_a_compare_tab_picks_it_and_the_swiped_picture_moves_the_divider() {
        let mut app = image_app((400, 300));
        crate::ui::render(&mut app, 120, 30);
        let swipe = app.compare_tabs[1];
        let down = MouseEventKind::Down(MouseButton::Left);
        assert!(app.handle(&mouse(down, swipe.x, swipe.y)));
        assert_eq!(app.image_compare, Compare::Swipe);

        crate::ui::render(&mut app, 120, 30);
        let area = app.picture_area;
        assert!(area.width > 10, "{area:?}");
        assert!(app.handle(&mouse(down, area.x, area.y)));
        assert_eq!(app.mix, 0);
        let drag = MouseEventKind::Drag(MouseButton::Left);
        assert!(app.handle(&mouse(drag, area.x + area.width / 2, area.y)));
        assert_eq!(app.mix, 50);
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

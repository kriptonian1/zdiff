//! The menu bar's state: which menu and item are picked, and where things were drawn.

use ratatui::layout::{Position, Rect};

use crate::app::{Scope, View};
use crate::find;
use crate::keymap::Keymap;

/// Everything a key or a menu item can do; `App::run` carries each one out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    GoToFile,
    Reload,
    Quit,
    /// Picks the diff view; `None` is Auto, which follows the pane width.
    Show(Option<View>),
    /// Flips between split and unified.
    ToggleView,
    /// One file, or every changed file stacked.
    Scope(Scope),
    /// Flips between one file and every file.
    ToggleScope,
    /// Shows or hides the sidebar.
    Sidebar,
    /// Moves the sidebar to the other side of the diff.
    SidebarRight,
    /// Moves focus between the sidebar and the diff.
    ToggleFocus,
    /// Shows every unchanged line.
    ExpandAll,
    /// Back to the default context, folding the rest.
    CollapseAll,
    MoreContext,
    LessContext,
    /// Turns the changed-word highlight on or off.
    WordHighlights,
    GoToLine,
    /// Find in the file; again widens to every file, and on a sidebar folder searches it.
    Find,
    SearchAll,
    NextChange,
    PrevChange,
    NextFile,
    PrevFile,
    /// Diff pane movement.
    Top,
    Bottom,
    ScrollDown,
    ScrollUp,
    HalfPageDown,
    HalfPageUp,
    PageDown,
    PageUp,
    ScrollLeft,
    ScrollRight,
    /// Opens the first unchanged-lines fold on screen.
    ExpandFold,
    /// Sidebar movement.
    SelectDown,
    SelectUp,
    SelectPageDown,
    SelectPageUp,
    SelectFirst,
    SelectLast,
    ToggleFolder,
    /// Closes the folder, or moves to its parent.
    CloseFolder,
    /// Opens the folder, or moves into it.
    OpenFolder,
    OpenMenu,
    /// Lists every keyboard shortcut.
    Shortcuts,
    /// Stores the current view, sidebar, and context as the startup layout.
    SaveLayout,
    /// Forgets the startup layout and goes back to the built-in one.
    ResetLayout,
    /// Turns syntax colors on or off.
    Syntax,
    /// Whether zdiff watches for changes when started without `--watch`.
    WatchAtStart,
    /// Opens the theme picker.
    Theme,
    /// Checks or unchecks the file or folder under the cursor for staging.
    ToggleStage,
    /// Stages the checked files and unstages the unchecked ones.
    Stage,
    /// Turns the checkboxes, Stage, and Commit on or off.
    Staging,
    /// Turns pictures of changed images on or off.
    ImagePreviews,
    /// Cycles a changed image through 2-up, swipe, and onion skin.
    ImageCompare,
    /// Switches an SVG between its code and rendered pictures.
    SvgView,
    /// Whether SVGs open as pictures; a setting.
    SvgPreviewDefault,
    /// Moves typing into the commit message box.
    FocusCommit,
    /// Applies the checkboxes, then commits the index with the message.
    Commit,
    /// Opens the recent commits.
    History,
    /// Opens the stashes.
    Stash,
    /// A line between menu groups; never bound, selected, or run.
    Separator,
}

impl Action {
    /// Every action a key or a menu can run: keymap defaults first, then menu-only ones.
    pub fn known() -> Vec<Self> {
        let menu = (MENUS.iter()).flat_map(|(_, items)| items.iter().copied());
        let mut known = Vec::new();
        for action in Keymap::defaults().bindings().map(|(_, _, a)| a).chain(menu) {
            if action != Self::Separator && !known.contains(&action) {
                known.push(action);
            }
        }
        known
    }

    /// The stable name the settings file uses for this action.
    pub fn id(self) -> &'static str {
        match self {
            Self::GoToFile => "go_to_file",
            Self::Reload => "reload",
            Self::Quit => "quit",
            Self::Show(Some(View::Split)) => "view_split",
            Self::Show(Some(View::Unified)) => "view_unified",
            Self::Show(None) => "view_auto",
            Self::ToggleView => "toggle_view",
            Self::Scope(Scope::File) => "single_file",
            Self::Scope(Scope::All) => "all_files",
            Self::ToggleScope => "toggle_scope",
            Self::Sidebar => "toggle_sidebar",
            Self::SidebarRight => "sidebar_right",
            Self::ToggleFocus => "toggle_focus",
            Self::ExpandAll => "expand_all",
            Self::CollapseAll => "collapse_all",
            Self::MoreContext => "more_context",
            Self::LessContext => "less_context",
            Self::WordHighlights => "word_highlights",
            Self::GoToLine => "go_to_line",
            Self::Find => "find",
            Self::SearchAll => "search_all",
            Self::NextChange => "next_change",
            Self::PrevChange => "prev_change",
            Self::NextFile => "next_file",
            Self::PrevFile => "prev_file",
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::ScrollDown => "scroll_down",
            Self::ScrollUp => "scroll_up",
            Self::HalfPageDown => "half_page_down",
            Self::HalfPageUp => "half_page_up",
            Self::PageDown => "page_down",
            Self::PageUp => "page_up",
            Self::ScrollLeft => "scroll_left",
            Self::ScrollRight => "scroll_right",
            Self::ExpandFold => "expand_fold",
            Self::SelectDown => "select_down",
            Self::SelectUp => "select_up",
            Self::SelectPageDown => "select_page_down",
            Self::SelectPageUp => "select_page_up",
            Self::SelectFirst => "select_first",
            Self::SelectLast => "select_last",
            Self::ToggleFolder => "toggle_folder",
            Self::CloseFolder => "close_folder",
            Self::OpenFolder => "open_folder",
            Self::OpenMenu => "open_menu",
            Self::Shortcuts => "shortcuts",
            Self::SaveLayout => "save_layout",
            Self::ResetLayout => "reset_layout",
            Self::Syntax => "syntax",
            Self::WatchAtStart => "watch_at_start",
            Self::Theme => "theme",
            Self::ToggleStage => "toggle_stage",
            Self::Stage => "stage",
            Self::Staging => "staging",
            Self::ImagePreviews => "image_previews",
            Self::ImageCompare => "image_compare",
            Self::SvgView => "svg_view",
            Self::SvgPreviewDefault => "svg_preview_default",
            Self::FocusCommit => "focus_commit",
            Self::Commit => "commit",
            Self::History => "history",
            Self::Stash => "stash",
            Self::Separator => "separator",
        }
    }

    /// The name shown in menus and on the shortcuts screen.
    pub fn label(self) -> &'static str {
        match self {
            Self::GoToFile => "Go to file…",
            Self::Reload => "Reload",
            Self::Quit => "Quit",
            Self::Show(Some(View::Split)) => "Split view",
            Self::Show(Some(View::Unified)) => "Unified view",
            Self::Show(None) => "Auto",
            Self::ToggleView => "Switch split / unified",
            Self::Scope(Scope::File) => "Single file",
            Self::Scope(Scope::All) => "All files",
            Self::ToggleScope => "Switch single / all files",
            Self::Sidebar => "Sidebar",
            Self::SidebarRight => "Sidebar on right",
            Self::ToggleFocus => "Switch focus",
            Self::ExpandAll => "Expand all folds",
            Self::CollapseAll => "Collapse all folds",
            Self::MoreContext => "More context",
            Self::LessContext => "Less context",
            Self::WordHighlights => "Word highlights",
            Self::GoToLine => "Go to line…",
            Self::Find => "Find in file…",
            Self::SearchAll => "Search all files…",
            Self::NextChange => "Next change",
            Self::PrevChange => "Previous change",
            Self::NextFile => "Next file",
            Self::PrevFile => "Previous file",
            Self::Top => "Top",
            Self::Bottom => "Bottom",
            Self::ScrollDown => "Scroll down",
            Self::ScrollUp => "Scroll up",
            Self::HalfPageDown => "Half page down",
            Self::HalfPageUp => "Half page up",
            Self::ScrollLeft => "Scroll left",
            Self::ScrollRight => "Scroll right",
            Self::ExpandFold => "Expand fold",
            Self::SelectDown => "Down",
            Self::SelectUp => "Up",
            Self::PageDown | Self::SelectPageDown => "Page down",
            Self::PageUp | Self::SelectPageUp => "Page up",
            Self::SelectFirst => "First file",
            Self::SelectLast => "Last file",
            Self::ToggleFolder => "Open / close folder",
            Self::CloseFolder => "Close folder",
            Self::OpenFolder => "Open folder",
            Self::OpenMenu => "Open menu",
            Self::Shortcuts => "Keyboard shortcuts…",
            Self::SaveLayout => "Save layout as default",
            Self::ResetLayout => "Reset layout",
            Self::Syntax => "Syntax highlighting",
            Self::WatchAtStart => "Watch for changes (next start)",
            Self::Theme => "Theme…",
            Self::ToggleStage => "Check / uncheck file",
            Self::Stage => "Stage checked",
            Self::Staging => "Staging",
            Self::ImagePreviews => "Image previews",
            Self::ImageCompare => "Image compare mode",
            Self::SvgView => "Switch SVG code / preview",
            Self::SvgPreviewDefault => "Open SVGs as preview",
            Self::FocusCommit => "Write commit message",
            Self::Commit => "Commit",
            Self::History => "History…",
            Self::Stash => "Stash…",
            Self::Separator => "",
        }
    }
}

/// Every menu, left to right: its title and items.
pub const MENUS: [(&str, &[Action]); 5] = [
    ("File", &[Action::Reload, Action::Quit]),
    (
        "View",
        &[
            Action::Scope(Scope::File),
            Action::Scope(Scope::All),
            Action::Separator,
            Action::Sidebar,
            Action::SidebarRight,
            Action::Separator,
            Action::Show(Some(View::Split)),
            Action::Show(Some(View::Unified)),
            Action::Show(None),
            Action::Separator,
            Action::ExpandAll,
            Action::CollapseAll,
            Action::MoreContext,
            Action::LessContext,
            Action::Separator,
            Action::WordHighlights,
            Action::Staging,
            Action::ImagePreviews,
            Action::SvgPreviewDefault,
            Action::Separator,
            Action::Theme,
        ],
    ),
    (
        "Navigate",
        &[
            Action::GoToFile,
            Action::GoToLine,
            Action::Separator,
            Action::Find,
            Action::SearchAll,
            Action::Separator,
            Action::NextChange,
            Action::PrevChange,
            Action::NextFile,
            Action::PrevFile,
            Action::Separator,
            Action::Top,
            Action::Bottom,
        ],
    ),
    ("Git", &[Action::History, Action::Stash]),
    (
        "Settings",
        &[
            Action::Shortcuts,
            Action::Separator,
            Action::SaveLayout,
            Action::ResetLayout,
            Action::Separator,
            Action::Syntax,
            Action::WatchAtStart,
        ],
    ),
];

/// Items in the longest menu, which sizes [`Menu::item_areas`].
const MAX_ITEMS: usize = 21;

/// The menu bar: its titles and, while open, one menu's dropdown.
#[derive(Debug, Default)]
pub struct Menu {
    /// `(menu, item)` picked while a dropdown is open; `None` when closed.
    pub open: Option<(usize, usize)>,
    /// Titles (in [`MENUS`] order) and the open menu's items from the last draw, for clicks.
    pub titles: [Rect; MENUS.len()],
    pub item_areas: [Rect; MAX_ITEMS],
}

impl Menu {
    /// The menu whose title is drawn at `position`.
    pub fn title_at(&self, position: Position) -> Option<usize> {
        self.titles
            .iter()
            .position(|title| title.contains(position))
    }

    /// The open menu's item drawn at `position`.
    pub fn item_at(&self, position: Position) -> Option<Action> {
        let (menu, _) = self.open?;
        (MENUS[menu].1.iter().zip(&self.item_areas))
            .find_map(|(&item, area)| area.contains(position).then_some(item))
            .filter(|&item| item != Action::Separator)
    }
}

/// The item after (or before) `i` in `items`, wrapping and skipping separators.
///
/// Ends because every menu has a real item; item 0 is always one.
pub fn step(items: &[Action], mut i: usize, down: bool) -> usize {
    loop {
        i = find::wrap(i, items.len(), down);
        if items[i] != Action::Separator {
            return i;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_areas_fit_the_longest_menu() {
        let longest = MENUS.iter().map(|(_, items)| items.len()).max();
        assert_eq!(longest, Some(MAX_ITEMS));
        assert!(MENUS.iter().all(|(_, items)| items[0] != Action::Separator));
    }

    #[test]
    fn step_skips_separators_and_wraps() {
        let view = MENUS[1].1;
        let at = |item| view.iter().position(|&i| i == item).expect("a View item");
        let (all, sidebar) = (at(Action::Scope(Scope::All)), at(Action::Sidebar));
        assert_eq!(view[all + 1], Action::Separator);
        assert_eq!(step(view, all, true), sidebar, "over the separator");
        assert_eq!(step(view, sidebar, false), all);
        assert_eq!(
            step(view, 0, false),
            view.len() - 1,
            "wraps to the last item"
        );
        let auto = at(Action::Show(None));
        assert_eq!(step(view, auto, true), at(Action::ExpandAll));
    }
}

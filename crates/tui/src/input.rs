//! Text inputs: `ratatui-textarea`'s editor under zdiff's keys and theme.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui_textarea::{CursorMove, TextArea};

use crate::ui::Colors;

/// Undo steps kept by single-line fields, which are short and rarely edited at length.
const SINGLE_HISTORY: usize = 20;
/// Undo steps kept by the commit message.
const MULTI_HISTORY: usize = 50;

/// What a key did to a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// Not an editing key, or nothing to do; the caller handles or ignores it.
    Ignored,
    /// Only the cursor moved.
    Moved,
    /// The text changed.
    Changed,
}

/// One text input: the crate's editor plus whether Enter and Up/Down belong to it.
#[derive(Debug, Clone)]
pub struct Field {
    area: TextArea<'static>,
    multiline: bool,
    /// The column Up and Down aim for; the crate forgets it on shorter lines, editors don't.
    goal: Option<usize>,
    /// Text the last cut or copy took, waiting to go to the system clipboard.
    copied: Option<String>,
}

impl Field {
    /// A one-line field (find, popups, search) holding `text`, cursor at the end.
    pub fn single(text: &str) -> Self {
        Self::new(text, false, SINGLE_HISTORY)
    }

    /// A multi-line field (the commit message) holding `text`, cursor at the end.
    pub fn multi(text: &str) -> Self {
        Self::new(text, true, MULTI_HISTORY)
    }

    fn new(text: &str, multiline: bool, history: usize) -> Self {
        let mut area = TextArea::new(text.split('\n').map(str::to_owned).collect());
        area.move_cursor(CursorMove::Bottom);
        area.move_cursor(CursorMove::End);
        area.set_max_histories(history);
        Self {
            area,
            multiline,
            goal: None,
            copied: None,
        }
    }

    /// Applies one of zdiff's text keys; Super is Cmd on terminals that pass it through.
    pub fn key(&mut self, key: KeyEvent) -> Edit {
        let mods = key.modifiers - KeyModifiers::SHIFT;
        let (ctrl, alt, cmd) = (
            mods == KeyModifiers::CONTROL,
            mods == KeyModifiers::ALT,
            mods == KeyModifiers::SUPER,
        );
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let multi = self.multiline;
        if multi && mods.is_empty() && matches!(key.code, KeyCode::Up | KeyCode::Down) {
            return self.line(key.code == KeyCode::Down, shift);
        }
        // Any other key ends a run of Up/Down moves.
        self.goal = None;
        let edit = |changed: bool| {
            if changed {
                Edit::Changed
            } else {
                Edit::Ignored
            }
        };
        match key.code {
            KeyCode::Esc if self.area.is_selecting() => {
                self.area.cancel_selection();
                Edit::Moved
            }
            KeyCode::Char('a') if cmd => {
                self.area.select_all();
                Edit::Moved
            }
            KeyCode::Char('x') if ctrl => {
                let cut = self.area.cut();
                if cut {
                    self.copied = Some(self.area.yank_text());
                }
                edit(cut)
            }
            KeyCode::Char('c' | 'C') if (cmd || ctrl && shift) && self.area.is_selecting() => {
                self.area.copy();
                self.copied = Some(self.area.yank_text());
                Edit::Moved
            }
            KeyCode::Left if mods.is_empty() => self.travel(CursorMove::Back, shift),
            KeyCode::Right if mods.is_empty() => self.travel(CursorMove::Forward, shift),
            KeyCode::Left if alt || ctrl => self.travel(CursorMove::WordBack, shift),
            KeyCode::Right if alt || ctrl => self.travel(CursorMove::WordForward, shift),
            KeyCode::Char('b') if alt => self.travel(CursorMove::WordBack, shift),
            KeyCode::Char('f') if alt => self.travel(CursorMove::WordForward, shift),
            KeyCode::Home if mods.is_empty() => self.travel(CursorMove::Head, shift),
            KeyCode::End if mods.is_empty() => self.travel(CursorMove::End, shift),
            KeyCode::Char('a') if ctrl => self.travel(CursorMove::Head, shift),
            KeyCode::Char('e') if ctrl => self.travel(CursorMove::End, shift),
            KeyCode::Left if cmd => self.travel(CursorMove::Head, shift),
            KeyCode::Right if cmd => self.travel(CursorMove::End, shift),
            KeyCode::Home if multi && ctrl => self.travel(CursorMove::Top, shift),
            KeyCode::End if multi && ctrl => self.travel(CursorMove::Bottom, shift),
            KeyCode::Up if multi && cmd => self.travel(CursorMove::Top, shift),
            KeyCode::Down if multi && cmd => self.travel(CursorMove::Bottom, shift),
            KeyCode::Backspace if alt => edit(self.area.delete_word()),
            KeyCode::Char('w') if ctrl => edit(self.area.delete_word()),
            KeyCode::Delete if alt || ctrl => edit(self.area.delete_next_word()),
            KeyCode::Char('u') if ctrl => edit(self.area.delete_line_by_head()),
            KeyCode::Backspace if cmd => edit(self.area.delete_line_by_head()),
            KeyCode::Char('z' | 'Z') if (ctrl || cmd) && shift => edit(self.area.redo()),
            KeyCode::Char('z') if ctrl || cmd => edit(self.area.undo()),
            KeyCode::Enter if multi && mods.is_empty() => {
                self.area.insert_newline();
                Edit::Changed
            }
            KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete if mods.is_empty() => {
                edit(self.area.input_without_shortcuts(key))
            }
            _ => Edit::Ignored,
        }
    }

    /// One line up or down, back to the column the moves started from where the line allows.
    fn line(&mut self, down: bool, shift: bool) -> Edit {
        let (row, column) = self.cursor();
        let goal = *self.goal.get_or_insert(column);
        let target = if down { row + 1 } else { row.wrapping_sub(1) };
        let Some(line) = self.area.lines().get(target) else {
            return Edit::Ignored;
        };
        let column = goal.min(line.chars().count());
        let (Ok(row), Ok(column)) = (u16::try_from(target), u16::try_from(column)) else {
            return Edit::Ignored;
        };
        self.select(shift);
        self.area.move_cursor(CursorMove::Jump(row, column));
        Edit::Moved
    }

    /// Starts a selection for a Shift move, or drops it for a plain one; `true` if one was
    /// dropped, which redraws even when the cursor can't move.
    fn select(&mut self, shift: bool) -> bool {
        if shift {
            if !self.area.is_selecting() {
                self.area.start_selection();
            }
            return false;
        }
        let selecting = self.area.is_selecting();
        self.area.cancel_selection();
        selecting
    }

    /// Moves the cursor, selecting while `shift` is held; `Ignored` when nothing changed,
    /// like Left at the start.
    fn travel(&mut self, motion: CursorMove, shift: bool) -> Edit {
        // Like any editor, a plain Left or Right over a selection lands on its edge.
        if let (false, Some((start, end))) = (shift, self.area.selection_range())
            && matches!(motion, CursorMove::Back | CursorMove::Forward)
        {
            let (row, column) = if motion == CursorMove::Back {
                start
            } else {
                end
            };
            self.area.cancel_selection();
            if let (Ok(row), Ok(column)) = (u16::try_from(row), u16::try_from(column)) {
                self.area.move_cursor(CursorMove::Jump(row, column));
            }
            return Edit::Moved;
        }
        let before = self.area.cursor();
        let dropped = self.select(shift);
        self.area.move_cursor(motion);
        if self.area.cursor() == before && !dropped {
            Edit::Ignored
        } else {
            Edit::Moved
        }
    }

    /// Inserts pasted `text` at the cursor, replacing any selection; one-line fields turn its
    /// line breaks into spaces.
    pub fn paste(&mut self, text: &str) -> Edit {
        self.goal = None;
        let inserted = if self.multiline {
            self.area.insert_str(text.replace("\r\n", "\n"))
        } else {
            self.area
                .insert_str(text.replace("\r\n", " ").replace(['\n', '\r'], " "))
        };
        if inserted {
            Edit::Changed
        } else {
            Edit::Ignored
        }
    }

    /// What the last cut or copy took, once; for the system clipboard.
    pub fn take_copied(&mut self) -> Option<String> {
        self.copied.take()
    }

    /// The text, lines joined with `\n`; allocates, so callers read it after a change.
    pub fn text(&self) -> String {
        self.area.lines().join("\n")
    }

    /// Whether there's nothing but whitespace, without building the text.
    pub fn is_blank(&self) -> bool {
        self.area.lines().iter().all(|line| line.trim().is_empty())
    }

    /// The cursor's column on its line, in chars.
    pub fn column(&self) -> usize {
        self.area.cursor().1
    }

    /// The cursor as `(line, column)`, in chars.
    pub fn cursor(&self) -> (usize, usize) {
        let at = self.area.cursor();
        (at.0, at.1)
    }

    pub fn set_placeholder(&mut self, text: &str) {
        self.area.set_placeholder_text(text);
    }

    /// Draws the text in the theme; the cursor shows only while `focused`.
    pub fn draw(&mut self, frame: &mut Frame, area: Rect, c: &Colors, focused: bool) {
        self.area.set_style(Style::new().fg(c.fg));
        self.area.set_cursor_line_style(Style::new());
        self.area.set_placeholder_style(Style::new().fg(c.dim));
        self.area.set_selection_style(Style::new().bg(c.selected));
        self.area.set_cursor_style(if focused {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new()
        });
        frame.render_widget(&self.area, area);
    }
}

impl Default for Field {
    fn default() -> Self {
        Self::single("")
    }
}

impl PartialEq for Field {
    fn eq(&self, other: &Self) -> bool {
        self.multiline == other.multiline && self.area.lines() == other.area.lines()
    }
}

impl Eq for Field {}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(field: &mut Field, code: KeyCode, mods: KeyModifiers) -> Edit {
        field.key(KeyEvent::new(code, mods))
    }

    fn keys(field: &mut Field, codes: &[KeyCode]) {
        for &code in codes {
            press(field, code, KeyModifiers::NONE);
        }
    }

    fn typed(multiline: bool, text: &str) -> Field {
        let mut field = if multiline {
            Field::multi("")
        } else {
            Field::single("")
        };
        for c in text.chars() {
            if c == '\n' {
                press(&mut field, KeyCode::Enter, KeyModifiers::NONE);
            } else {
                press(&mut field, KeyCode::Char(c), KeyModifiers::NONE);
            }
        }
        field
    }

    #[test]
    fn typing_in_the_middle_inserts_at_the_cursor() {
        let mut field = typed(false, "hllo");
        assert_eq!(
            press(&mut field, KeyCode::Home, KeyModifiers::NONE),
            Edit::Moved
        );
        keys(&mut field, &[KeyCode::Right]);
        assert_eq!(
            press(&mut field, KeyCode::Char('e'), KeyModifiers::NONE),
            Edit::Changed
        );
        assert_eq!(field.text(), "hello");
        assert_eq!(field.column(), 2);
        keys(&mut field, &[KeyCode::End]);
        assert_eq!(
            press(&mut field, KeyCode::Right, KeyModifiers::NONE),
            Edit::Ignored
        );
    }

    #[test]
    fn word_moves_and_readline_line_keys() {
        let ctrl = KeyModifiers::CONTROL;
        let alt = KeyModifiers::ALT;
        let mut field = typed(false, "fix the bug");
        press(&mut field, KeyCode::Left, alt);
        assert_eq!(field.column(), 8, "Option+Left: start of `bug`");
        press(&mut field, KeyCode::Char('b'), alt);
        assert_eq!(field.column(), 4, "Alt+B, what Option+Left sends");
        press(&mut field, KeyCode::Char('a'), ctrl);
        assert_eq!(field.column(), 0, "Ctrl+A, what Cmd+Left sends");
        press(&mut field, KeyCode::Char('f'), alt);
        assert!(field.column() > 0, "Alt+F moves forward");
        press(&mut field, KeyCode::Char('e'), ctrl);
        assert_eq!(field.column(), 11, "Ctrl+E, what Cmd+Right sends");
        press(&mut field, KeyCode::Left, KeyModifiers::SUPER);
        assert_eq!(field.column(), 0, "Cmd+Left when the terminal passes Cmd");
    }

    #[test]
    fn word_and_line_deletes_then_undo_and_redo() {
        let ctrl = KeyModifiers::CONTROL;
        let mut field = typed(false, "fix the bug");
        assert_eq!(
            press(&mut field, KeyCode::Backspace, KeyModifiers::ALT),
            Edit::Changed
        );
        assert_eq!(field.text(), "fix the ");
        press(&mut field, KeyCode::Char('w'), ctrl);
        assert_eq!(field.text(), "fix ");
        press(&mut field, KeyCode::Char('u'), ctrl);
        assert_eq!(field.text(), "", "Ctrl+U, what Cmd+Backspace sends");
        press(&mut field, KeyCode::Char('z'), ctrl);
        assert_eq!(field.text(), "fix ");
        press(&mut field, KeyCode::Char('z'), ctrl | KeyModifiers::SHIFT);
        assert_eq!(field.text(), "");
        let mut field = typed(false, "one two");
        keys(&mut field, &[KeyCode::Home]);
        press(&mut field, KeyCode::Delete, KeyModifiers::ALT);
        assert_eq!(field.text(), " two", "delete the word after");
    }

    #[test]
    fn single_line_fields_leave_enter_and_arrows_to_the_caller() {
        let mut field = typed(false, "abc");
        for code in [KeyCode::Enter, KeyCode::Up, KeyCode::Down, KeyCode::Tab] {
            assert_eq!(
                press(&mut field, code, KeyModifiers::NONE),
                Edit::Ignored,
                "{code:?}"
            );
        }
        for c in ['k', 'p', 'f', 'g', 'b', 'r'] {
            let edit = press(&mut field, KeyCode::Char(c), KeyModifiers::CONTROL);
            assert_eq!(edit, Edit::Ignored, "Ctrl+{c} stays a zdiff command");
        }
        assert_eq!(field.text(), "abc");
    }

    #[test]
    fn the_commit_box_keeps_its_column_across_short_lines() {
        let mut field = typed(true, "subject line\n\nbody text");
        keys(&mut field, &[KeyCode::Up, KeyCode::Up]);
        assert_eq!(field.cursor(), (0, 9), "past the empty line");
        press(&mut field, KeyCode::End, KeyModifiers::CONTROL);
        assert_eq!(field.cursor(), (2, 9), "Ctrl+End: the very end");
        assert!(!field.is_blank());
        assert!(Field::multi(" \n ").is_blank());
    }

    #[test]
    fn multibyte_chars_move_and_delete_whole() {
        let mut field = typed(false, "héllo ✓");
        keys(&mut field, &[KeyCode::Left, KeyCode::Backspace]);
        assert_eq!(field.text(), "héllo✓");
        keys(
            &mut field,
            &[KeyCode::Home, KeyCode::Right, KeyCode::Delete],
        );
        assert_eq!(field.text(), "hllo✓");
    }

    #[test]
    fn shift_moves_select_and_typing_replaces_the_selection() {
        let shift = KeyModifiers::SHIFT;
        let mut field = typed(false, "fix the bug");
        press(&mut field, KeyCode::Left, KeyModifiers::ALT | shift);
        for c in "issue".chars() {
            press(&mut field, KeyCode::Char(c), KeyModifiers::NONE);
        }
        assert_eq!(
            field.text(),
            "fix the issue",
            "Shift+Option+Left selected `bug`"
        );
        press(&mut field, KeyCode::Left, shift);
        press(&mut field, KeyCode::Left, shift);
        assert_eq!(
            press(&mut field, KeyCode::Left, KeyModifiers::NONE),
            Edit::Moved
        );
        press(&mut field, KeyCode::Char('!'), KeyModifiers::NONE);
        assert_eq!(
            field.text(),
            "fix the iss!ue",
            "a plain move dropped the selection"
        );
    }

    #[test]
    fn cut_and_copy_hand_the_selection_over() {
        let mut field = typed(false, "keep cut");
        press(
            &mut field,
            KeyCode::Left,
            KeyModifiers::ALT | KeyModifiers::SHIFT,
        );
        assert_eq!(
            press(&mut field, KeyCode::Char('x'), KeyModifiers::CONTROL),
            Edit::Changed
        );
        assert_eq!(field.text(), "keep ");
        assert_eq!(field.take_copied().as_deref(), Some("cut"));
        assert_eq!(field.take_copied(), None, "taken once");

        press(&mut field, KeyCode::Home, KeyModifiers::SHIFT);
        assert_eq!(
            press(&mut field, KeyCode::Char('c'), KeyModifiers::SUPER),
            Edit::Moved
        );
        assert_eq!(field.text(), "keep ", "copy keeps the text");
        assert_eq!(field.take_copied().as_deref(), Some("keep "));
        // The crate drops the selection after a copy, so select again.
        press(&mut field, KeyCode::End, KeyModifiers::NONE);
        press(&mut field, KeyCode::Home, KeyModifiers::SHIFT);
        let ctrl_shift = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        press(&mut field, KeyCode::Char('C'), ctrl_shift);
        assert_eq!(
            field.take_copied().as_deref(),
            Some("keep "),
            "Ctrl+Shift+C"
        );

        press(&mut field, KeyCode::End, KeyModifiers::NONE);
        assert_eq!(
            press(&mut field, KeyCode::Char('c'), KeyModifiers::SUPER),
            Edit::Ignored
        );
        assert_eq!(
            field.take_copied(),
            None,
            "nothing selected, nothing copied"
        );
    }

    #[test]
    fn select_all_then_backspace_clears_and_esc_drops_a_selection_first() {
        let mut field = typed(true, "one\ntwo");
        assert_eq!(
            press(&mut field, KeyCode::Char('a'), KeyModifiers::SUPER),
            Edit::Moved
        );
        press(&mut field, KeyCode::Backspace, KeyModifiers::NONE);
        assert!(field.text().is_empty());

        let mut field = typed(false, "abc");
        press(&mut field, KeyCode::Left, KeyModifiers::SHIFT);
        assert_eq!(
            press(&mut field, KeyCode::Esc, KeyModifiers::NONE),
            Edit::Moved
        );
        assert_eq!(
            press(&mut field, KeyCode::Esc, KeyModifiers::NONE),
            Edit::Ignored
        );
        assert_eq!(field.text(), "abc");
    }

    #[test]
    fn paste_flattens_line_breaks_in_one_line_fields() {
        let mut field = typed(false, "a");
        assert_eq!(field.paste("b\r\nc\nd"), Edit::Changed);
        assert_eq!(field.text(), "ab c d");
        let mut field = typed(true, "x");
        field.paste("1\r\n2");
        assert_eq!(field.text(), "x1\n2", "the commit box keeps lines");
        let mut field = typed(false, "old");
        press(&mut field, KeyCode::Home, KeyModifiers::SHIFT);
        field.paste("new");
        assert_eq!(field.text(), "new", "a paste replaces the selection");
        assert_eq!(field.paste(""), Edit::Ignored);
    }
}

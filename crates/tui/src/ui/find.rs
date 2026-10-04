//! The find bar at the top-right of the diff pane.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::Clear;

use super::{Colors, popup};
use crate::app::App;
use crate::find::Toggle;
use zdiff_search::Query;

/// Bar width; fits the badge, a short query, the counter, and the toggles.
const WIDTH: u16 = 48;
/// The ` FIND ` badge plus the space after it.
const PROMPT_WIDTH: u16 = 7;

/// Width of [`toggle_spans`]: a space before each toggle label, plus a trailing space.
pub(super) const TOGGLES_WIDTH: u16 = 4 + 1 + 4 + 1 + 3 + 1 + 1;

/// ` [Aa] [.*] [±] ` starting at column `x`, lit when on; records each label's area for clicks.
pub(super) fn toggle_spans(
    c: &Colors,
    query: &Query,
    mut x: u16,
    y: u16,
    areas: &mut [Rect; 3],
) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(7);
    for (toggle, area) in Toggle::ALL.into_iter().zip(areas) {
        let color = if toggle.is_on(query) { c.accent } else { c.dim };
        let label = Span::from(toggle.label()).fg(color);
        x += 1;
        *area = Rect::new(x, y, span_width(&label), 1);
        x += span_width(&label);
        spans.extend([" ".into(), label]);
    }
    spans.push(" ".into());
    spans
}

fn span_width(span: &Span) -> u16 {
    u16::try_from(span.width()).unwrap_or(u16::MAX)
}

pub(super) fn draw(frame: &mut Frame, app: &mut App) {
    let (pane, c) = (app.diff_area, app.shown_theme().colors());
    // A palette over the bar takes the keys, and with them the cursor.
    let focused = app.palette.is_none();
    let Some(find) = &mut app.find else {
        return;
    };
    let width = WIDTH.min(pane.width);
    let bar = Rect {
        x: pane.right() - width,
        y: pane.y,
        width,
        height: 3.min(pane.height),
    };
    find.area = bar;
    let block = popup(c);
    let inner = block.inner(bar);
    frame.render_widget(Clear, bar);
    frame.render_widget(block, bar);

    let (counter, bad) = find.counter();
    let counter = Span::from(counter).fg(if bad { c.red } else { c.dim });
    let counter_width = span_width(&counter);
    let right_width = counter_width + 1 + TOGGLES_WIDTH;
    let [left, status] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_width)]).areas(inner);
    let mut spans = vec![counter, " ".into()];
    let x = status.x + counter_width + 1;
    spans.extend(toggle_spans(
        c,
        &find.query,
        x,
        status.y,
        &mut find.toggle_areas,
    ));
    frame.render_widget(Line::from(spans), status);

    let [badge, text] =
        Layout::horizontal([Constraint::Length(PROMPT_WIDTH), Constraint::Fill(1)]).areas(left);
    let prompt = Line::from(vec![" FIND ".fg(c.badge_fg).bg(c.badge).bold(), " ".into()]);
    frame.render_widget(prompt, badge);
    find.field.draw(frame, text, c, focused);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Modifier;
    use zdiff_core::{FileDiff, Status};

    use super::*;
    use crate::app::{DiffPane, FileEntry};
    use crate::find::Find;
    use crate::ui::DARK;

    fn render(query: &str, regex: bool) -> Buffer {
        let mut app = App::new(vec![FileEntry {
            path: "a.rs".into(),
            status: Status::Modified,
            from: None,
            added: 1,
            removed: 1,
            change: 0,
            staged: zdiff_core::Staged::No,
        }]);
        app.show(Some(Ok(FileDiff::new(b"x\n".to_vec(), b"xy\n".to_vec()))));
        let mut find = Find::default();
        find.set_text(query);
        find.query.regex = regex;
        if let DiffPane::Loaded(view) = &app.diff {
            find.update(view);
        }
        app.find = Some(find);
        let mut terminal = Terminal::new(TestBackend::new(200, 8)).expect("test backend");
        terminal
            .draw(|f| crate::ui::draw(f, &mut app))
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    fn row(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    #[test]
    fn bar_shows_query_counter_and_highlights() {
        let buffer = render("x", false);
        // Menu bar row 0, header row 1, bar rows 2..5 at the top-right; its content is row 3.
        assert!(
            row(&buffer, 3).contains(" FIND  x"),
            "{:?}",
            row(&buffer, 3)
        );
        assert!(row(&buffer, 3).contains("1/2  [Aa] [.*] [±]"));
        let y = (0..8)
            .find(|&y| row(&buffer, y).contains("1 - x"))
            .expect("change row");
        let xs: Vec<u16> = (0..200)
            .filter(|&x| buffer[(x, y)].symbol() == "x")
            .collect();
        assert_eq!(
            buffer[(xs[0], y)].bg,
            DARK.find_current,
            "old side hit is current"
        );
        assert_eq!(buffer[(xs[1], y)].bg, DARK.find, "new side hit");
    }

    #[test]
    fn bad_regex_reads_red() {
        let buffer = render("(", true);
        let line = row(&buffer, 3);
        let byte = line.find("bad regex").expect("error shown");
        let x = u16::try_from(line[..byte].chars().count()).unwrap();
        assert_eq!(buffer[(x, 3)].fg, DARK.red);
    }

    #[test]
    fn toggle_hit_areas_cover_their_labels() {
        let mut app = App::new(vec![FileEntry {
            path: "a.rs".into(),
            status: Status::Modified,
            from: None,
            added: 1,
            removed: 1,
            change: 0,
            staged: zdiff_core::Staged::No,
        }]);
        app.show(Some(Ok(FileDiff::new(b"x\n".to_vec(), b"y\n".to_vec()))));
        app.find = Some(Find::default());
        let mut terminal = Terminal::new(TestBackend::new(120, 8)).expect("test backend");
        terminal
            .draw(|f| crate::ui::draw(f, &mut app))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let find = app.find.as_ref().unwrap();
        for (toggle, area) in Toggle::ALL.into_iter().zip(find.toggle_areas) {
            let text: String = (area.x..area.right())
                .map(|x| buffer[(x, area.y)].symbol())
                .collect();
            assert_eq!(text, toggle.label());
        }
        assert!(find.area.contains(find.toggle_areas[0].as_position()));
    }

    #[test]
    fn toggles_width_matches_the_spans() {
        let spans = toggle_spans(DARK, &Query::default(), 0, 0, &mut [Rect::default(); 3]);
        let width: usize = spans.iter().map(Span::width).sum();
        assert_eq!(width, usize::from(TOGGLES_WIDTH));
    }

    #[test]
    fn the_cursor_cell_sits_at_the_caret_not_the_end() {
        use crossterm::event::KeyCode;

        let mut app = App::new(vec![FileEntry {
            path: "a.rs".into(),
            status: Status::Modified,
            from: None,
            added: 1,
            removed: 1,
            change: 0,
            staged: zdiff_core::Staged::No,
        }]);
        app.show(Some(Ok(FileDiff::new(b"x\n".to_vec(), b"xy\n".to_vec()))));
        let mut find = Find::default();
        find.set_text("héllo");
        let key =
            |code| crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE);
        find.field.key(key(KeyCode::Home));
        find.field.key(key(KeyCode::Right));
        app.find = Some(find);
        let mut terminal = Terminal::new(TestBackend::new(200, 8)).expect("test backend");
        terminal
            .draw(|f| crate::ui::draw(f, &mut app))
            .expect("draw");
        let bar = app.find.as_ref().expect("open").area;
        let reversed = |buffer: &Buffer, x, y| {
            let cell = &buffer[(x, y)];
            (
                cell.symbol().to_owned(),
                cell.modifier.contains(Modifier::REVERSED),
            )
        };
        // Border, the ` FIND ` badge and its space, then one char of text: the `é`.
        let buffer = terminal.backend().buffer().clone();
        let x = bar.x + 1 + PROMPT_WIDTH + 1;
        assert_eq!(reversed(&buffer, x, bar.y + 1), ("é".into(), true));
        assert!(!reversed(&buffer, x + 1, bar.y + 1).1, "one cursor cell");

        app.find = None;
        app.palette = Some(crate::palette::Palette::line());
        if let Some(palette) = &mut app.palette {
            palette.field = crate::input::Field::single("123");
            palette.field.key(key(KeyCode::Left));
        }
        terminal
            .draw(|f| crate::ui::draw(f, &mut app))
            .expect("draw");
        // Border, ` LINE `, ` :`, then two of the three digits; the cursor is on the `3`.
        let buffer = terminal.backend().buffer().clone();
        let (x, y) = (app.palette_area.x + 1 + 6 + 2 + 2, app.palette_area.y + 1);
        assert_eq!(reversed(&buffer, x, y), ("3".into(), true));
    }

    #[test]
    fn selected_text_is_highlighted() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = App::new(vec![FileEntry {
            path: "a.rs".into(),
            status: Status::Modified,
            from: None,
            added: 1,
            removed: 1,
            change: 0,
            staged: zdiff_core::Staged::No,
        }]);
        app.show(Some(Ok(FileDiff::new(b"x\n".to_vec(), b"xy\n".to_vec()))));
        let mut find = Find::default();
        find.set_text("abcd");
        let shift_left = KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT);
        find.field.key(shift_left);
        find.field.key(shift_left);
        app.find = Some(find);
        let mut terminal = Terminal::new(TestBackend::new(200, 8)).expect("test backend");
        terminal
            .draw(|f| crate::ui::draw(f, &mut app))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let bar = app.find.as_ref().expect("open").area;
        let x = bar.x + 1 + PROMPT_WIDTH;
        let cell = |dx: u16| &buffer[(x + dx, bar.y + 1)];
        assert_ne!(cell(1).bg, DARK.selected, "b is not selected");
        let cursor = cell(2)
            .modifier
            .contains(ratatui::style::Modifier::REVERSED);
        assert!(cursor, "the cursor sits on c, drawn over the selection");
        assert_eq!(cell(3).bg, DARK.selected, "d is selected");
    }
}

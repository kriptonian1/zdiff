//! The find bar at the top-right of the diff pane.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear};

use super::{ACCENT, BADGE_BG, DIM, MODAL_BG, RED};
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
    query: &Query,
    mut x: u16,
    y: u16,
    areas: &mut [Rect; 3],
) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(7);
    for (toggle, area) in Toggle::ALL.into_iter().zip(areas) {
        let color = if toggle.is_on(query) { ACCENT } else { DIM };
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
    let pane = app.diff_area;
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
    let block = Block::bordered()
        .border_style(Style::new().fg(DIM))
        .bg(MODAL_BG);
    let inner = block.inner(bar);
    frame.render_widget(Clear, bar);
    frame.render_widget(block, bar);

    let (counter, bad) = find.counter();
    let counter = Span::from(counter).fg(if bad { RED } else { DIM });
    let counter_width = span_width(&counter);
    let right_width = counter_width + 1 + TOGGLES_WIDTH;
    let [left, status] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_width)]).areas(inner);
    let mut spans = vec![counter, " ".into()];
    let x = status.x + counter_width + 1;
    spans.extend(toggle_spans(
        &find.query,
        x,
        status.y,
        &mut find.toggle_areas,
    ));
    frame.render_widget(Line::from(spans), status);

    let q = &find.query;
    frame.render_widget(
        Line::from(vec![
            " FIND ".fg(Color::Black).bg(BADGE_BG).bold(),
            " ".into(),
            q.text.as_str().into(),
        ]),
        left,
    );
    // A palette drawn later moves the cursor to itself.
    let typed = u16::try_from(q.text.chars().count()).unwrap_or(u16::MAX);
    let x = left
        .x
        .saturating_add(PROMPT_WIDTH + typed)
        .min(left.right());
    frame.set_cursor_position((x, left.y));
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use zdiff_core::{FileDiff, Status};

    use super::super::{FIND_BG, FIND_CURRENT_BG};
    use super::*;
    use crate::app::{DiffPane, FileEntry};
    use crate::find::Find;

    fn render(query: &str, regex: bool) -> Buffer {
        let mut app = App::new(vec![FileEntry {
            path: "a.rs".into(),
            status: Status::Modified,
            added: 1,
            removed: 1,
            change: 0,
        }]);
        app.show(Some(Ok(FileDiff::new(b"x\n".to_vec(), b"xy\n".to_vec()))));
        let mut find = Find::default();
        find.query.text = query.into();
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
        // Header row 0, bar rows 1..4 at the top-right; its content is row 2.
        assert!(
            row(&buffer, 2).contains(" FIND  x"),
            "{:?}",
            row(&buffer, 2)
        );
        assert!(row(&buffer, 2).contains("1/2  [Aa] [.*] [±]"));
        let y = (0..8)
            .find(|&y| row(&buffer, y).contains("1 - x"))
            .expect("change row");
        let xs: Vec<u16> = (0..200)
            .filter(|&x| buffer[(x, y)].symbol() == "x")
            .collect();
        assert_eq!(
            buffer[(xs[0], y)].bg,
            FIND_CURRENT_BG,
            "old side hit is current"
        );
        assert_eq!(buffer[(xs[1], y)].bg, FIND_BG, "new side hit");
    }

    #[test]
    fn bad_regex_reads_red() {
        let buffer = render("(", true);
        let line = row(&buffer, 2);
        let byte = line.find("bad regex").expect("error shown");
        let x = u16::try_from(line[..byte].chars().count()).unwrap();
        assert_eq!(buffer[(x, 2)].fg, RED);
    }

    #[test]
    fn toggle_hit_areas_cover_their_labels() {
        let mut app = App::new(vec![FileEntry {
            path: "a.rs".into(),
            status: Status::Modified,
            added: 1,
            removed: 1,
            change: 0,
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
        let spans = toggle_spans(&Query::default(), 0, 0, &mut [Rect::default(); 3]);
        let width: usize = spans.iter().map(Span::width).sum();
        assert_eq!(width, usize::from(TOGGLES_WIDTH));
    }
}

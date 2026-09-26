use std::ops::Range;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Widget,
};
use zdiff_core::{Kind, Row, Text};
use zdiff_highlight::Token;

use super::{ADDED_BG, DIM, FILLER_BG, FOLD_BG, GREEN, RED, REMOVED_BG, class_color, counts};
use crate::app::{App, DiffPane, DiffView, FileEntry};
use crate::text;

/// Columns before the line text in a cell: space, marker, space.
const MARKER_WIDTH: usize = 3;

/// How one side of a changed line looks.
struct Side {
    marker: &'static str,
    fg: Color,
    bg: Color,
}

const OLD: Side = Side {
    marker: "-",
    fg: RED,
    bg: REMOVED_BG,
};
const NEW: Side = Side {
    marker: "+",
    fg: GREEN,
    bg: ADDED_BG,
};

pub(super) fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let [header, body] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    app.diff_area = body;
    if let Some(file) = app.selected_file() {
        draw_header(frame.buffer_mut(), file, header);
    }
    match &mut app.diff {
        DiffPane::Empty => {}
        DiffPane::Failed(error) => message(frame.buffer_mut(), body, error),
        DiffPane::Loaded(view) if view.rows.is_empty() => {
            let text = if view.file.binary {
                "Binary file changed"
            } else {
                "No content changes"
            };
            message(frame.buffer_mut(), body, text);
        }
        DiffPane::Loaded(view) => draw_rows(frame, view, body),
    }
}

fn draw_header(buf: &mut Buffer, file: &FileEntry, area: Rect) {
    Line::from(format!(" {}", file.path.display())).render(area, buf);
    Line::from(counts(file.added, file.removed))
        .right_aligned()
        .render(area, buf);
}

fn message(buf: &mut Buffer, area: Rect, text: &str) {
    Paragraph::new(format!(" {text}")).fg(DIM).render(area, buf);
}

/// Renders only the rows that fit on screen, so cost follows screen height, not file size.
fn draw_rows(frame: &mut Frame, view: &mut DiffView, body: Rect) {
    let height = usize::from(body.height);
    view.scroll = view.scroll.min(view.max_scroll(height));
    let [left, divider, right] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(body);
    let text_width =
        |pane: Rect| usize::from(pane.width).saturating_sub(view.gutter + MARKER_WIDTH);
    view.text_width = text_width(left).min(text_width(right));

    let buf = frame.buffer_mut();
    Block::new()
        .borders(Borders::LEFT)
        .fg(DIM)
        .render(divider, buf);
    for (y, row) in (body.y..body.bottom()).zip(&view.rows[view.scroll..]) {
        let at = |area: Rect| Rect {
            y,
            height: 1,
            ..area
        };
        match row {
            Row::Fold { len, .. } => Line::from(format!(" ▾ {len} unchanged lines"))
                .fg(DIM)
                .bg(FOLD_BG)
                .render(at(body), buf),
            Row::Header { old, new, heading } => {
                let heading = heading
                    .map(|i| text::visible(view.file.old.line(i), 0, usize::from(body.width)))
                    .unwrap_or_default();
                let text = format!(
                    " @@ -{},{} +{},{} @@ {heading}",
                    git_start(old),
                    old.len(),
                    git_start(new),
                    new.len()
                );
                Line::from(text).fg(DIM).bg(FOLD_BG).render(at(body), buf);
            }
            Row::Line { old, new, kind } => {
                let columns = |pane| view.hscroll..view.hscroll + text_width(pane);
                let old_side = (&view.file.old, &*view.old_tokens);
                cell(old_side, *old, *kind, &OLD, view.gutter, columns(left)).render(at(left), buf);
                let new_side = (&view.file.new, &*view.new_tokens);
                cell(new_side, *new, *kind, &NEW, view.gutter, columns(right))
                    .render(at(right), buf);
            }
        }
    }

    let mut scrollbar = ScrollbarState::new(view.max_scroll(height)).position(view.scroll);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None),
        body,
        &mut scrollbar,
    );
}

/// `12 - text` for one side of a line; a missing side is filler.
fn cell(
    (text, tokens): (&Text, &[Token]),
    line: Option<u32>,
    kind: Kind,
    side: &Side,
    gutter: usize,
    columns: Range<usize>,
) -> Line<'static> {
    let Some(i) = line else {
        return Line::default().bg(FILLER_BG);
    };
    let (marker, fg, bg) = match kind {
        Kind::Context => (" ", DIM, Color::Reset),
        Kind::Change => (side.marker, side.fg, side.bg),
    };
    let mut spans = vec![
        format!("{:>gutter$} ", i + 1).fg(fg),
        marker.fg(fg),
        " ".into(),
    ];
    let segments = text::segments(
        text.line(i),
        text.line_start(i),
        tokens,
        columns.start,
        columns.len(),
    );
    spans.extend(segments.into_iter().map(|(content, class)| match class {
        Some(class) => content.fg(class_color(class)),
        None => content.into(),
    }));
    Line::from(spans).bg(bg)
}

/// First line number as `git diff` prints it: 1-based, or the insertion point when empty.
fn git_start(range: &Range<u32>) -> u32 {
    if range.is_empty() {
        range.start
    } else {
        range.start + 1
    }
}

#[cfg(test)]
mod tests {
    use zdiff_core::{FileDiff, Status};

    use super::super::render;
    use super::*;
    use crate::app::FileEntry;

    #[test]
    fn split_view_pairs_lines_folds_and_fills() {
        let old: String = (1..=10).map(|i| i.to_string() + "\n").collect();
        let new = old.replace("\n8\n", "\neight\nextra\n");
        let mut app = App::new(vec![FileEntry {
            path: "a.rs".into(),
            status: Status::Modified,
            added: 2,
            removed: 1,
            change: 0,
        }]);
        app.show(Some(Ok(FileDiff::new(old.into(), new.into()))));

        let screen = render(&mut app, 100, 12);
        let split = |row: &str| -> (String, String) {
            let diff: String = row.chars().skip(30).collect();
            let (left, right) = diff.split_once('│').expect("divider on every body row");
            (left.to_owned(), right.to_owned())
        };
        assert!(screen[0].contains("a.rs") && screen[0].trim_end().ends_with("+2 -1"));
        assert!(screen[1].contains("▾ 4 unchanged lines"), "{screen:#?}");
        assert!(screen[2].contains("@@ -5,6 +5,7 @@"), "{screen:#?}");
        let (l, r) = split(&screen[6]);
        assert!(
            l.starts_with(" 8 - 8") && r.starts_with(" 8 + eight"),
            "{screen:#?}"
        );
        let (l, r) = split(&screen[7]);
        assert!(
            l.trim().is_empty() && r.starts_with(" 9 + extra"),
            "{screen:#?}"
        );
    }

    #[test]
    fn changed_rust_line_is_syntax_colored_on_the_diff_background() {
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = App::new(vec![FileEntry {
            path: "a.rs".into(),
            status: Status::Modified,
            added: 1,
            removed: 1,
            change: 0,
        }]);
        app.show(Some(Ok(FileDiff::new(
            b"fn a() {}\n".to_vec(),
            b"fn b() {}\n".to_vec(),
        ))));
        let mut terminal = Terminal::new(TestBackend::new(100, 6)).expect("test backend");
        terminal
            .draw(|f| crate::ui::draw(f, &mut app))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        // Sidebar is 30 wide; old cell = "1 - fn a() {}", so `fn` starts 4 columns in.
        let fn_cell = &buffer[(34, 2)];
        assert_eq!(fn_cell.symbol(), "f");
        assert_eq!(
            fn_cell.fg,
            crate::ui::class_color(zdiff_highlight::Class::Keyword)
        );
        assert_eq!(fn_cell.bg, REMOVED_BG);
    }

    #[test]
    fn horizontal_scroll_moves_text_but_not_line_numbers() {
        let old = "abcdefghijklmnopqrstuvwxyz0123456789\n";
        let mut app = App::new(vec![FileEntry {
            path: "a.rs".into(),
            status: Status::Modified,
            added: 1,
            removed: 1,
            change: 0,
        }]);
        app.show(Some(Ok(FileDiff::new(old.into(), b"x\n".to_vec()))));
        if let DiffPane::Loaded(view) = &mut app.diff {
            view.hscroll = 10;
        }
        let screen = render(&mut app, 100, 6);
        let left: String = screen[2].chars().skip(30).collect();
        assert!(left.starts_with("1 - klmnop"), "{screen:#?}");
    }
}

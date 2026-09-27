use std::ops::Range;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Widget,
};
use zdiff_core::{Kind, Row, Status, Text};
use zdiff_highlight::Token;

use super::{
    ADDED_BG, ADDED_EMPH_BG, DIM, FILLER_BG, FOLD_BG, GREEN, RED, REMOVED_BG, REMOVED_EMPH_BG,
    STRIPE_FG, STRIPE_GAP, class_color, counts,
};
use crate::app::{App, DiffPane, DiffView, FileEntry};
use crate::text;

/// Columns before the line text in a cell: space, marker, space.
const MARKER_WIDTH: usize = 3;

/// How one side of a changed line looks.
struct Side {
    marker: &'static str,
    fg: Color,
    bg: Color,
    emph_bg: Color,
}

const OLD: Side = Side {
    marker: "-",
    fg: RED,
    bg: REMOVED_BG,
    emph_bg: REMOVED_EMPH_BG,
};
const NEW: Side = Side {
    marker: "+",
    fg: GREEN,
    bg: ADDED_BG,
    emph_bg: ADDED_EMPH_BG,
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
    let mut title = Line::from(format!(" {}", file.path.display()));
    if file.status == Status::Untracked {
        title.push_span(" (untracked)".fg(DIM));
    }
    title.render(area, buf);
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
                let sides = [
                    (
                        left,
                        &OLD,
                        (&view.file.old, &*view.old_tokens, &*view.words.old),
                        *old,
                    ),
                    (
                        right,
                        &NEW,
                        (&view.file.new, &*view.new_tokens, &*view.words.new),
                        *new,
                    ),
                ];
                for (pane, side, content, line) in sides {
                    match cell(content, line, *kind, side, view.gutter, columns(pane)) {
                        Some(line) => line.render(at(pane), buf),
                        None => filler(buf, at(pane), view.gutter),
                    }
                }
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

/// `12 - text` for one side of a line; `None` when that side is missing.
fn cell(
    (text, tokens, emph): (&Text, &[Token], &[Range<u32>]),
    line: Option<u32>,
    kind: Kind,
    side: &Side,
    gutter: usize,
    columns: Range<usize>,
) -> Option<Line<'static>> {
    let i = line?;
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
        emph,
        columns.start,
        columns.len(),
    );
    spans.extend(segments.into_iter().map(|(content, look)| {
        let span = match look.class {
            Some(class) => content.fg(class_color(class)),
            None => content.into(),
        };
        if look.emph {
            span.bg(side.emph_bg)
        } else {
            span
        }
    }));
    Some(Line::from(spans).bg(bg))
}

/// Missing side of a line: GitHub-style diagonal stripes past the gutter.
fn filler(buf: &mut Buffer, area: Rect, gutter: usize) {
    buf.set_style(area, Style::new().bg(FILLER_BG));
    let text_x = area
        .x
        .saturating_add(u16::try_from(gutter + MARKER_WIDTH).unwrap_or(u16::MAX));
    for x in text_x..area.right() {
        if (u32::from(x) + u32::from(area.y)).is_multiple_of(STRIPE_GAP) {
            buf[(x, area.y)].set_symbol("╱").set_fg(STRIPE_FG);
        }
    }
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
    use zdiff_core::FileDiff;

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
            l.trim_matches([' ', '╱']).is_empty() && r.starts_with(" 9 + extra"),
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
        assert_eq!(buffer[(37, 2)].symbol(), "a");
        assert_eq!(
            buffer[(37, 2)].bg,
            REMOVED_EMPH_BG,
            "changed word is emphasized"
        );
    }

    #[test]
    fn filler_stripes_text_area_but_not_gutter() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 2));
        filler(&mut buf, Rect::new(0, 1, 12, 1), 1);
        for x in 0..12 {
            let cell = &buf[(x, 1)];
            assert_eq!(cell.bg, FILLER_BG, "x = {x}");
            if x >= 4 && (u32::from(x) + 1).is_multiple_of(STRIPE_GAP) {
                assert_eq!((cell.symbol(), cell.fg), ("╱", STRIPE_FG), "x = {x}");
            } else {
                assert_eq!(cell.symbol(), " ", "x = {x}");
            }
        }
    }

    #[test]
    fn header_marks_untracked_files() {
        let mut app = App::new(vec![FileEntry {
            path: "new.rs".into(),
            status: Status::Untracked,
            added: 1,
            removed: 0,
            change: 0,
        }]);
        let header: String = render(&mut app, 100, 3)[0].chars().skip(30).collect();
        assert!(header.starts_with(" new.rs (untracked)"), "{header:?}");
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

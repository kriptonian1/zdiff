use std::borrow::Cow;
use std::ops::Range;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Widget,
};
use ratatui_image::picker::Picker;
use ratatui_image::{Resize, StatefulImage};
use zdiff_core::{Kind, Row, Side as FileSide, Status, Text};

use super::{Colors, STRIPE_GAP, counts, styled};
use crate::app::{App, DiffPane, DiffView, FileEntry, View};
use crate::find::Find;
use crate::preview::{Compare, Preview, SvgView};
use crate::stream::{Body, HEADER_ROWS};
use crate::text::{self, Marks};

/// Columns before the line text in a cell: space, marker, space.
const MARKER_WIDTH: usize = 3;

/// How one side of a changed line looks.
struct Side {
    marker: &'static str,
    fg: Color,
    bg: Color,
    emph_bg: Color,
}

impl Side {
    fn removed(c: &Colors) -> Self {
        Self {
            marker: "-",
            fg: c.red,
            bg: c.removed,
            emph_bg: c.removed_emph,
        }
    }

    fn added(c: &Colors) -> Self {
        Self {
            marker: "+",
            fg: c.green,
            bg: c.added,
            emph_bg: c.added_emph,
        }
    }
}

pub(super) fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let [header, body] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    app.diff_area = body;
    app.fit_view(area.width);
    app.fit_preview();
    let c = app.shown_theme().colors();
    // In All files `sync_top` keeps this on the file at the top: a sticky header.
    let partial = matches!(&app.diff, DiffPane::Loaded(view) if view.file.known.is_some());
    let svg = (app.svg_has_code() && app.stream.is_none())
        .then(|| (app.svg_view, app.images.is_some() && app.image_previews));
    if let Some(file) = app.selected_file() {
        app.svg_tabs = draw_header(frame.buffer_mut(), c, (file, partial, svg), header);
    }
    if app.stream.is_some() {
        draw_stream(frame, app, body);
        return;
    }
    let paint = Paint::of(app);
    // Sixel and iTerm2 pictures cover any text drawn over them, popups included.
    let shown = app.picture_shown();
    let none_left = (app.only.as_ref()).filter(|_| app.selected_file().is_none());
    let none_left = none_left.map(|only| format!("No changes in {only}"));
    match (&mut app.diff, &app.images) {
        (DiffPane::Empty, _) => {
            if let Some(text) = none_left {
                message(frame.buffer_mut(), c, body, &text);
            }
        }
        (DiffPane::Failed(error), _) => message(frame.buffer_mut(), c, body, error),
        (
            DiffPane::Loaded(DiffView {
                preview: Some(preview),
                ..
            }),
            Some(picker),
        ) if shown => {
            let compare = (picker, app.image_compare, app.mix);
            (app.compare_tabs, app.picture_area) =
                draw_preview(frame, c, preview, (paint.layout, compare), body);
        }
        (DiffPane::Loaded(view), _) if view.rows.is_empty() => {
            message(frame.buffer_mut(), c, body, &empty_reason(view));
        }
        (DiffPane::Loaded(view), _) => {
            draw_rows(frame, view, paint, app.find.as_ref(), body);
        }
    }
}

/// The file's path and counts; `partial` marks a patch file shown without its full context,
/// and `svg` adds Code / Preview tabs: the current view and whether pictures can be drawn.
/// Returns the tabs' areas, empty without `svg`.
fn draw_header(
    buf: &mut Buffer,
    c: &Colors,
    (file, partial, svg): (&FileEntry, bool, Option<(SvgView, bool)>),
    area: Rect,
) -> [Rect; 2] {
    let mut title = Line::from(format!(" {}", file.path.display()));
    if file.status == Status::Untracked {
        title.push_span(" (untracked)".fg(c.dim));
    }
    if partial {
        title.push_span(" (hunks only)".fg(c.dim));
    }
    let mut tabs = [Rect::default(); 2];
    if let Some((view, drawable)) = svg {
        title.push_span("   ");
        let shown = if drawable { view } else { SvgView::Code };
        tabs = push_tabs(
            &mut title,
            c,
            SvgView::ALL.map(|tab| (tab.label(), tab == shown)),
            area,
        );
    }
    title.render(area, buf);
    Line::from(counts(c, file.added, file.removed))
        .right_aligned()
        .render(area, buf);
    tabs
}

fn message(buf: &mut Buffer, c: &Colors, area: Rect, text: &str) {
    Paragraph::new(format!(" {text}"))
        .fg(c.dim)
        .render(area, buf);
}

/// Why a loaded diff has no rows.
fn empty_reason(view: &DiffView) -> Cow<'static, str> {
    match &view.preview {
        Some(preview) => preview.caption().into(),
        None if view.file.binary => "Binary file changed".into(),
        None => "No content changes".into(),
    }
}

/// A changed image: compare tabs when both sides are pictures, then the swiped or faded
/// picture, or 2-up boxes. Returns the tabs' and the picture's areas, for clicks.
fn draw_preview(
    frame: &mut Frame,
    c: &Colors,
    preview: &mut Preview,
    (layout, (picker, mode, mix)): (View, (&Picker, Compare, u8)),
    area: Rect,
) -> ([Rect; 3], Rect) {
    let mut tabs = [Rect::default(); 3];
    let area = if preview.comparable() {
        let [bar, rest] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        tabs = draw_tabs(frame.buffer_mut(), c, (mode, mix), bar);
        rest
    } else {
        area
    };
    let caption = format!(" {} ", preview.caption());
    let Some(image) = preview.blended(picker, mode, mix, rgb(c.accent)) else {
        draw_two_up(frame, c, preview, layout, area);
        return (tabs, Rect::default());
    };
    let title = match mode {
        Compare::Swipe => " old ◂▸ new ",
        _ => " old ▾ new ",
    };
    let block = Block::bordered()
        .border_style(c.accent)
        .title(title)
        .title_bottom(caption);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let size = image.size_for(Resize::Fit(None), inner.as_size());
    frame.render_stateful_widget(StatefulImage::default(), inner, image);
    (tabs, Rect::new(inner.x, inner.y, size.width, size.height))
}

/// ` 2-up │ Swipe │ Onion skin`, the current one lit, and the percent when it applies.
fn draw_tabs(buf: &mut Buffer, c: &Colors, (mode, mix): (Compare, u8), area: Rect) -> [Rect; 3] {
    let mut line = Line::from(" ");
    let tabs = push_tabs(
        &mut line,
        c,
        Compare::ALL.map(|tab| (tab.label(), tab == mode)),
        area,
    );
    line.render(area, buf);
    if mode != Compare::TwoUp {
        Line::from(format!(" ← → {mix}% "))
            .fg(c.dim)
            .right_aligned()
            .render(area, buf);
    }
    tabs.map(|rect| rect.intersection(area))
}

/// Appends `Label │ Label` to `line`, lit where the flag is set; returns each label's area,
/// with `line` drawn from the left of `area`.
fn push_tabs<const N: usize>(
    line: &mut Line<'static>,
    c: &Colors,
    tabs: [(&'static str, bool); N],
    area: Rect,
) -> [Rect; N] {
    let mut areas = [Rect::default(); N];
    for (i, ((label, lit), rect)) in tabs.into_iter().zip(&mut areas).enumerate() {
        if i > 0 {
            line.push_span(" │ ".fg(c.dim));
        }
        let x = area
            .x
            .saturating_add(u16::try_from(line.width()).unwrap_or(u16::MAX));
        let width = u16::try_from(label.len()).unwrap_or(u16::MAX);
        *rect = Rect::new(x, area.y, width, 1).intersection(area);
        line.push_span(if lit {
            label.fg(c.accent).bold()
        } else {
            label.fg(c.dim)
        });
    }
    areas
}

/// A theme color as RGB, for drawing into a picture; white for non-RGB colors.
fn rgb(color: Color) -> [u8; 3] {
    match color {
        Color::Rgb(r, g, b) => [r, g, b],
        _ => [255; 3],
    }
}

/// Old and new pictures in red and green boxes: side by side in split view, stacked in unified.
fn draw_two_up(frame: &mut Frame, c: &Colors, preview: &mut Preview, layout: View, area: Rect) {
    let [old, new] = &mut preview.sides;
    let sides: Vec<_> = [(" old ", c.red, old), (" new ", c.green, new)]
        .into_iter()
        .filter_map(|(title, color, side)| Some((title, color, side.as_mut()?)))
        .collect();
    let boxes = match layout {
        View::Split => Layout::horizontal(vec![Constraint::Fill(1); sides.len()]),
        View::Unified => Layout::vertical(vec![Constraint::Fill(1); sides.len()]),
    }
    .split(area);
    for ((title, color, picture), &area) in sides.into_iter().zip(boxes.iter()) {
        let block = Block::bordered()
            .border_style(color)
            .title(title)
            .title_bottom(format!(" {} ", picture.label()));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        match &mut picture.image {
            Some(image) => frame.render_stateful_widget(StatefulImage::default(), inner, image),
            None => message(frame.buffer_mut(), c, inner, "too large to preview"),
        }
    }
}

/// How rows are painted: the view, which highlights are on, and the colors.
#[derive(Debug, Clone, Copy)]
struct Paint {
    layout: View,
    words: bool,
    syntax: bool,
    colors: &'static Colors,
}

impl Paint {
    fn of(app: &App) -> Self {
        Self {
            layout: app.view,
            words: app.word_highlights,
            syntax: app.syntax,
            colors: app.shown_theme().colors(),
        }
    }
}

/// Renders only the rows that fit on screen, so cost follows screen height, not file size.
fn draw_rows(
    frame: &mut Frame,
    view: &mut DiffView,
    paint: Paint,
    find: Option<&Find>,
    body: Rect,
) {
    let height = usize::from(body.height);
    view.scroll = view.scroll.min(view.max_scroll(height));
    let first = view.scroll;
    draw_body(frame, (&mut *view, paint), find, body, first);
    draw_scrollbar(frame, view.scroll, view.max_scroll(height), body);
}

/// Every changed file stacked: a header row per file, then its rows, collapsed notice, or state.
fn draw_stream(frame: &mut Frame, app: &mut App, body: Rect) {
    let height = usize::from(body.height);
    let paint = Paint::of(app);
    let c = paint.colors;
    let Some(stream) = &mut app.stream else {
        return;
    };
    stream.scroll = stream.scroll.min(stream.max_scroll(height));
    let (mut row, mut y) = (stream.scroll, body.y);
    while y < body.bottom() {
        let Some((i, local)) = stream.at(row) else {
            break;
        };
        let section = &mut stream.sections[i];
        let rows = (HEADER_ROWS + section.height - local).min(usize::from(body.bottom() - y));
        let mut area = Rect {
            y,
            height: u16::try_from(rows).unwrap_or(u16::MAX),
            ..body
        };
        let file = app.tree.file(section.node);
        // The separator and header rows still on screen, top to bottom.
        for head in local..HEADER_ROWS {
            if area.is_empty() {
                break;
            }
            let line = Rect { height: 1, ..area };
            let buf = frame.buffer_mut();
            match (head, file) {
                (0, _) => Line::from("─".repeat(usize::from(line.width)))
                    .fg(c.dim)
                    .render(line, buf),
                (_, Some(file)) => {
                    let partial =
                        matches!(&section.body, Body::Loaded(view) if view.file.known.is_some());
                    draw_header(buf, c, (file, partial, None), line);
                    // The header row stands out so files read apart.
                    buf.set_style(line, Style::new().bg(c.selected));
                }
                (_, None) => {}
            }
            area.y += 1;
            area.height -= 1;
        }
        let first = local.saturating_sub(HEADER_ROWS);
        let buf = frame.buffer_mut();
        match &mut section.body {
            _ if area.is_empty() => {}
            Body::Loaded(view) if view.rows.is_empty() => {
                message(buf, c, area, &empty_reason(view));
            }
            Body::Loaded(view) => {
                view.hscroll = stream.hscroll;
                draw_body(frame, (view, paint), None, area, first);
            }
            Body::Collapsed => {
                let lines = file.map_or(0, |f| f.added + f.removed);
                let text = format!(" ▸ Large diff: {lines} lines changed · Enter to load");
                Line::from(text).fg(c.dim).bg(c.fold).render(area, buf);
            }
            Body::Unloaded => message(buf, c, area, "loading…"),
            Body::Failed(error) => message(buf, c, area, error),
        }
        y += u16::try_from(rows).unwrap_or(u16::MAX);
        row += rows;
    }
    draw_scrollbar(frame, stream.scroll, stream.max_scroll(height), body);
    // Clamping above may have moved the top file, e.g. after a resize.
    app.sync_top();
}

/// Draws `view.rows[first..]` into `body`, as many as fit; no scroll clamp and no scrollbar.
fn draw_body(
    frame: &mut Frame,
    (view, paint): (&mut DiffView, Paint),
    find: Option<&Find>,
    body: Rect,
    first: usize,
) {
    let (unified, c) = (paint.layout == View::Unified, paint.colors);
    let [left, divider, right] = if unified {
        [body, Rect::default(), Rect::default()]
    } else {
        Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(body)
    };
    // Unified shows both line numbers, so its gutter is one number and a space wider.
    let gutter_width = if unified {
        2 * view.gutter + 1
    } else {
        view.gutter
    };
    let text_width =
        |pane: Rect| usize::from(pane.width).saturating_sub(gutter_width + MARKER_WIDTH);
    view.text_width = if unified {
        text_width(body)
    } else {
        text_width(left).min(text_width(right))
    };

    let view = &*view;
    let current = find.and_then(|f| f.current_range(view));
    let marks = |side: FileSide| Marks {
        tokens: if paint.syntax {
            side.pick(&view.old_tokens, &view.new_tokens)
        } else {
            &[]
        },
        emph: if paint.words {
            side.pick(&view.words.old, &view.words.new)
        } else {
            &[]
        },
        found: find.map_or(&[], |f| side.pick(&f.found[0], &f.found[1])),
        current: current.as_ref().filter(|(s, _)| *s == side).map(|(_, r)| r),
    };
    let buf = frame.buffer_mut();
    if !unified {
        Block::new()
            .borders(Borders::LEFT)
            .fg(c.dim)
            .render(divider, buf);
    }
    let rows = view.rows[first.min(view.rows.len())..].iter().enumerate();
    for (y, (k, row)) in (body.y..body.bottom()).zip(rows) {
        let jumped = view.mark == Some(first + k);
        let at = |area: Rect| Rect {
            y,
            height: 1,
            ..area
        };
        match row {
            Row::Fold { .. } | Row::Gap { .. } => Line::from(collapsed(row))
                .fg(c.dim)
                .bg(c.fold)
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
                Line::from(text).fg(c.dim).bg(c.fold).render(at(body), buf);
            }
            Row::Line { old, new, kind } if unified => {
                let columns = view.hscroll..view.hscroll + text_width(body);
                let lines = (*old, *new);
                if let Some(line) = unified_cell(c, view, marks, lines, (*kind, jumped), columns) {
                    line.render(at(body), buf);
                }
            }
            Row::Line { old, new, kind } => {
                let columns = |pane| view.hscroll..view.hscroll + text_width(pane);
                let (old_marks, new_marks) = (marks(FileSide::Old), marks(FileSide::New));
                let sides = [
                    (left, Side::removed(c), (&view.file.old, old_marks), *old),
                    (right, Side::added(c), (&view.file.new, new_marks), *new),
                ];
                for (pane, side, content, line) in sides {
                    let look = (*kind, jumped);
                    let gutter = line
                        .map_or_else(String::new, |i| format!("{:>w$} ", i + 1, w = view.gutter));
                    match cell(c, content, line, gutter, look, &side, columns(pane)) {
                        Some(line) => line.render(at(pane), buf),
                        None => filler(buf, c, at(pane), view.gutter),
                    }
                }
            }
        }
    }
}

/// The text of a row that stands for many lines: a fold to open, or a patch's unknown lines.
fn collapsed(row: &Row) -> String {
    match row {
        Row::Gap { len, .. } => format!(" ┄ {len} lines not in the patch"),
        Row::Fold { len, .. } => format!(" ▾ {len} unchanged lines"),
        Row::Header { .. } | Row::Line { .. } => String::new(),
    }
}

fn draw_scrollbar(frame: &mut Frame, position: usize, max: usize, body: Rect) {
    let mut scrollbar = ScrollbarState::new(max).position(position);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None),
        body,
        &mut scrollbar,
    );
}

/// `11 12 + text` for a unified row: both line numbers, then the one side it shows.
fn unified_cell<'a>(
    c: &Colors,
    view: &'a DiffView,
    marks: impl Fn(FileSide) -> Marks<'a>,
    (old, new): (Option<u32>, Option<u32>),
    look: (Kind, bool),
    columns: Range<usize>,
) -> Option<Line<'static>> {
    // Removed lines show the old text; added and context lines the new.
    // ponytail: old-side find hits on context lines aren't marked; merge marks if missed.
    let (side, text, file_side) = match (old, new) {
        (Some(_), None) => (Side::removed(c), &view.file.old, FileSide::Old),
        _ => (Side::added(c), &view.file.new, FileSide::New),
    };
    let number = |n: Option<u32>| n.map_or_else(String::new, |i| (i + 1).to_string());
    let gutter = format!("{:>w$} {:>w$} ", number(old), number(new), w = view.gutter);
    let line = file_side.pick(old, new);
    cell(
        c,
        (text, marks(file_side)),
        line,
        gutter,
        look,
        &side,
        columns,
    )
}

/// `12 - text` for one side of a line, after the `number` gutter; `None` when that side is missing.
fn cell(
    c: &Colors,
    (text, marks): (&Text, Marks<'_>),
    line: Option<u32>,
    number: String,
    (kind, jumped): (Kind, bool),
    side: &Side,
    columns: Range<usize>,
) -> Option<Line<'static>> {
    let i = line?;
    let (marker, fg, bg) = match kind {
        Kind::Context => (" ", c.dim, c.bg),
        Kind::Change => (side.marker, side.fg, side.bg),
    };
    let mut spans = vec![
        if jumped {
            number.fg(c.accent).bold()
        } else {
            number.fg(fg)
        },
        marker.fg(fg),
        " ".into(),
    ];
    let segments = text::segments(
        text.line(i),
        text.line_start(i),
        marks,
        columns.start,
        columns.len(),
    );
    spans.extend(
        (segments.into_iter()).map(|(content, look)| styled(c, content, look, side.emph_bg)),
    );
    Some(Line::from(spans).bg(bg))
}

/// Missing side of a line: GitHub-style diagonal stripes past the gutter.
fn filler(buf: &mut Buffer, c: &Colors, area: Rect, gutter: usize) {
    buf.set_style(area, Style::new().bg(c.filler));
    let text_x = area
        .x
        .saturating_add(u16::try_from(gutter + MARKER_WIDTH).unwrap_or(u16::MAX));
    for x in text_x..area.right() {
        if (u32::from(x) + u32::from(area.y)).is_multiple_of(STRIPE_GAP) {
            buf[(x, area.y)].set_symbol("╱").set_fg(c.stripe);
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
    use crate::ui::DARK;

    #[test]
    fn svgs_have_code_and_preview_tabs_and_show_code_under_popups() {
        let [old, new] = crate::preview::SVGS;
        let entry = |path: &str| FileEntry {
            path: path.into(),
            status: Status::Modified,
            added: 1,
            removed: 1,
            change: 0,
            staged: zdiff_core::Staged::No,
        };
        let mut app = App::new(vec![entry("icon.svg")]);
        app.view_choice = Some(View::Split);
        app.images = Some(ratatui_image::picker::Picker::halfblocks());
        app.show(Some(Ok(FileDiff::new(old.into(), new.into()))));
        let has = |screen: &[String], text: &str| screen.iter().any(|row| row.contains(text));
        let screen = render(&mut app, 120, 20);
        assert!(has(&screen, "icon.svg   Code │ Preview"), "{screen:#?}");
        assert!(has(&screen, "<svg xmlns"), "code first: {screen:#?}");

        let tab = app.svg_tabs[1];
        let click = crossterm::event::Event::Mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: tab.x,
            row: tab.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
        assert!(app.handle(&click));
        let screen = render(&mut app, 120, 20);
        assert!(
            has(&screen, " old ") && !has(&screen, "<svg xmlns"),
            "{screen:#?}"
        );

        app.find = Some(Find::default());
        let screen = render(&mut app, 120, 20);
        assert!(
            has(&screen, "<svg xmlns") && !has(&screen, " old "),
            "{screen:#?}"
        );

        let mut rs = App::new(vec![entry("a.rs")]);
        rs.show(Some(Ok(FileDiff::new(b"a\n".to_vec(), b"b\n".to_vec()))));
        assert!(!has(&render(&mut rs, 120, 20), "Code │ Preview"));
    }

    #[test]
    fn a_focus_with_no_changes_says_so() {
        let mut app = App::new(Vec::new());
        assert!(
            !render(&mut app, 80, 6)
                .iter()
                .any(|row| row.contains("No changes in"))
        );
        app.only = Some("src/app.rs".into());
        let screen = render(&mut app, 80, 6);
        assert!(
            screen
                .iter()
                .any(|row| row.contains("No changes in src/app.rs")),
            "{screen:#?}"
        );
    }

    #[test]
    fn images_draw_in_boxes_or_as_a_caption() {
        use crate::preview::png;

        let mut app = App::new(vec![FileEntry {
            path: "logo.png".into(),
            status: Status::Modified,
            added: 0,
            removed: 0,
            change: 0,
            staged: zdiff_core::Staged::No,
        }]);
        app.view_choice = Some(View::Split);
        let diff = || Some(Ok(FileDiff::new(png(3, 2), png(4, 4))));
        app.show(diff());
        let has = |screen: &[String], text: &str| screen.iter().any(|row| row.contains(text));
        let screen = render(&mut app, 120, 20);
        assert!(has(&screen, "Image changed · 3×2 → 4×4"), "{screen:#?}");

        // Halfblocks stand in for a real protocol, which a test terminal can't answer.
        app.images = Some(ratatui_image::picker::Picker::halfblocks());
        app.show(diff());
        let screen = render(&mut app, 120, 20);
        assert!(
            has(&screen, " old ") && has(&screen, " new ") && has(&screen, " 4×4 · "),
            "{screen:#?}"
        );
        assert!(!has(&screen, "Image changed"), "{screen:#?}");
        assert!(has(&screen, " 2-up │ Swipe │ Onion skin"), "{screen:#?}");
        assert!(!has(&screen, "50%"), "no mix in 2-up");
        app.image_compare = crate::preview::Compare::Swipe;
        let screen = render(&mut app, 120, 20);
        assert!(
            has(&screen, " old ◂▸ new ") && has(&screen, "← → 50%"),
            "{screen:#?}"
        );

        app.find = Some(Find::default());
        let screen = render(&mut app, 120, 20);
        assert!(
            !has(&screen, " old "),
            "no pictures under a popup: {screen:#?}"
        );
        assert!(has(&screen, "Image changed"), "{screen:#?}");
    }

    #[test]
    fn all_files_stack_headers_collapse_large_diffs_and_show_loading() {
        let entry = |path: &str, added| FileEntry {
            path: path.into(),
            status: Status::Modified,
            added,
            removed: 0,
            change: 0,
            staged: zdiff_core::Staged::No,
        };
        let mut app = App::new(vec![
            entry("a.rs", 1),
            entry("big.rs", 900),
            entry("c.rs", 1),
        ]);
        app.view_choice = Some(View::Split);
        let a = crossterm::event::Event::Key(crossterm::event::KeyCode::Char('a').into());
        assert!(app.handle(&a));
        app.stream_loaded(0, Ok(FileDiff::new(b"x\n".to_vec(), b"y\n".to_vec())));
        let screen = render(&mut app, 120, 14);
        let pane = |row: usize| screen[row].chars().skip(36).collect::<String>();
        // The last column holds the scrollbar.
        let rule = |row: usize| {
            let line = pane(row);
            line.chars()
                .take(line.chars().count() - 1)
                .all(|c| c == '─')
        };
        assert!(pane(0).starts_with(" a.rs"), "sticky header: {:?}", pane(0));
        assert!(rule(1), "a separator above each file: {:?}", pane(1));
        assert!(pane(2).starts_with(" a.rs"), "a.rs's own header");
        assert!(pane(3).contains("@@ -1,1 +1,1 @@"));
        assert!(rule(5));
        assert!(pane(6).starts_with(" big.rs"));
        assert!(
            pane(7).contains("▸ Large diff: 900 lines changed"),
            "{:?}",
            pane(7)
        );
        assert!(rule(8));
        assert!(pane(9).starts_with(" c.rs"));
        assert!(pane(10).contains("loading…"), "c.rs isn't loaded yet");
    }

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
            staged: zdiff_core::Staged::No,
        }]);
        app.view_choice = Some(View::Split);
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
    fn light_theme_paints_its_own_background() {
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = App::new(vec![FileEntry {
            path: "a.rs".into(),
            status: Status::Modified,
            added: 1,
            removed: 1,
            change: 0,
            staged: zdiff_core::Staged::No,
        }]);
        app.theme = crate::ui::Theme::GithubLight;
        app.view_choice = Some(View::Unified);
        app.show(Some(Ok(FileDiff::new(
            b"a\nb\n".to_vec(),
            b"a\nc\n".to_vec(),
        ))));
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).expect("test backend");
        terminal
            .draw(|f| crate::ui::draw(f, &mut app))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let light = app.theme.colors();
        let context = (0..12)
            .find(|&y| {
                (0..80)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .ends_with(" a")
            })
            .expect("the context line is drawn");
        assert_eq!(buffer[(79, context)].bg, light.bg, "context line");
        assert_eq!(buffer[(60, 10)].bg, light.bg, "empty pane below the rows");
        assert_eq!(buffer[(60, 10)].fg, light.fg);
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
            staged: zdiff_core::Staged::No,
        }]);
        app.view_choice = Some(View::Split);
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
        let fn_cell = &buffer[(34, 3)];
        assert_eq!(fn_cell.symbol(), "f");
        assert_eq!(fn_cell.fg, DARK.class(zdiff_highlight::Class::Keyword));
        assert_eq!(fn_cell.bg, DARK.removed);
        assert_eq!(buffer[(37, 3)].symbol(), "a");
        assert_eq!(
            buffer[(37, 3)].bg,
            DARK.removed_emph,
            "changed word is emphasized"
        );

        app.word_highlights = false;
        terminal
            .draw(|f| crate::ui::draw(f, &mut app))
            .expect("draw");
        let cell = &terminal.backend().buffer()[(37, 3)];
        assert_eq!(cell.bg, DARK.removed, "no word emphasis when turned off");

        app.syntax = false;
        terminal
            .draw(|f| crate::ui::draw(f, &mut app))
            .expect("draw");
        let fn_cell = &terminal.backend().buffer()[(34, 3)];
        assert_eq!(fn_cell.symbol(), "f");
        assert_ne!(
            fn_cell.fg,
            DARK.class(zdiff_highlight::Class::Keyword),
            "no syntax colors when turned off"
        );
    }

    #[test]
    fn filler_stripes_text_area_but_not_gutter() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 2));
        filler(&mut buf, DARK, Rect::new(0, 1, 12, 1), 1);
        for x in 0..12 {
            let cell = &buf[(x, 1)];
            assert_eq!(cell.bg, DARK.filler, "x = {x}");
            if x >= 4 && (u32::from(x) + 1).is_multiple_of(STRIPE_GAP) {
                assert_eq!((cell.symbol(), cell.fg), ("╱", DARK.stripe), "x = {x}");
            } else {
                assert_eq!(cell.symbol(), " ", "x = {x}");
            }
        }
    }

    #[test]
    fn go_to_line_target_has_an_accent_line_number() {
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = App::new(vec![FileEntry {
            path: "a.rs".into(),
            status: Status::Modified,
            added: 1,
            removed: 1,
            change: 0,
            staged: zdiff_core::Staged::No,
        }]);
        app.view_choice = Some(View::Split);
        app.show(Some(Ok(FileDiff::new(b"a\n".to_vec(), b"b\n".to_vec()))));
        if let DiffPane::Loaded(view) = &mut app.diff {
            view.mark = view.rows.iter().position(|r| matches!(r, Row::Line { .. }));
        }
        let mut terminal = Terminal::new(TestBackend::new(100, 6)).expect("test backend");
        terminal
            .draw(|f| crate::ui::draw(f, &mut app))
            .expect("draw");
        // Menu bar at row 0, header at row 2, the line at row 3; the old side's number starts the pane.
        let number = &terminal.backend().buffer()[(30, 3)];
        assert_eq!((number.symbol(), number.fg), ("1", DARK.accent));
    }

    #[test]
    fn header_marks_untracked_files() {
        let mut app = App::new(vec![FileEntry {
            path: "new.rs".into(),
            status: Status::Untracked,
            added: 1,
            removed: 0,
            change: 0,
            staged: zdiff_core::Staged::No,
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
            staged: zdiff_core::Staged::No,
        }]);
        app.view_choice = Some(View::Split);
        app.show(Some(Ok(FileDiff::new(old.into(), b"x\n".to_vec()))));
        if let DiffPane::Loaded(view) = &mut app.diff {
            view.hscroll = 10;
        }
        let screen = render(&mut app, 100, 6);
        let left: String = screen[2].chars().skip(30).collect();
        assert!(left.starts_with("1 - klmnop"), "{screen:#?}");
    }

    #[test]
    fn narrow_pane_draws_unified_rows_with_both_line_numbers() {
        let mut app = App::new(vec![FileEntry {
            path: "a.rs".into(),
            status: Status::Modified,
            added: 1,
            removed: 1,
            change: 0,
            staged: zdiff_core::Staged::No,
        }]);
        app.show(Some(Ok(FileDiff::new(
            b"x\ny\n".to_vec(),
            b"x\nz\n".to_vec(),
        ))));
        let screen = render(&mut app, 100, 8);
        assert_eq!(
            app.view,
            View::Unified,
            "a 70-column pane is too narrow for split"
        );
        let body: Vec<&str> = screen[2..5].iter().map(|row| &row[..]).collect();
        let diff = |row: &str| row.chars().skip(30).collect::<String>();
        assert_eq!(
            diff(body[0]).trim_end(),
            "1 1   x",
            "context shows both numbers"
        );
        assert_eq!(diff(body[1]).trim_end(), "2   - y");
        assert_eq!(diff(body[2]).trim_end(), "  2 + z");
        assert!(
            body.iter()
                .all(|row| diff(row).trim_end().chars().filter(|&c| c == '│').count() == 0)
        );
    }
}

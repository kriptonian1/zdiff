//! The palette popup (go to line or go to file), drawn over both panes without dimming them.

use std::mem;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, Paragraph};
use zdiff_core::Side;

use zdiff_search::MAX_HITS;

use super::find::{TOGGLES_WIDTH, toggle_spans};
use super::{
    ACCENT, BADGE_BG, DIM, GREEN, MODAL_BG, RED, SELECTED_BG, counts, right_aligned,
    sidebar::status_color, styled,
};
use crate::app::{App, DiffPane, FileEntry};
use crate::palette::{self, Files, Hit, Input, Mode, Search};
use crate::text::{self, Marks};
use crate::tree::Tree;

/// Go-to-line box: border, input, rule, footer, border.
const LINE_SIZE: (u16, u16) = (60, 5);
/// Go-to-file box: room for about ten results.
const FILE_SIZE: (u16, u16) = (70, 16);
/// Global search box: three fields and about eighteen result rows.
const SEARCH_SIZE: (u16, u16) = (90, 26);
/// Global search field badges, padded to one width so the inputs line up.
const BADGES: [&str; 3] = [" SEARCH  ", " include ", " exclude "];

pub(super) fn draw(frame: &mut Frame, app: &mut App) {
    let Some(palette) = &mut app.palette else {
        return;
    };
    let ((width, height), badge, sep) = match palette.mode {
        Mode::Line => (LINE_SIZE, " LINE ", " :"),
        Mode::File(_) => (FILE_SIZE, " FILE ", " "),
        Mode::Search(_) => (SEARCH_SIZE, "", ""),
    };
    let area = frame.area();
    let width = width.min(area.width.saturating_sub(4));
    let top = area.height / 10;
    let modal = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + top,
        width,
        height: height.min(area.height - top),
    };
    app.palette_area = modal;

    let block = Block::bordered()
        .border_style(Style::new().fg(DIM))
        .bg(MODAL_BG);
    let inner = block.inner(modal);
    frame.render_widget(Clear, modal);
    frame.render_widget(block, modal);
    let prompt_rows = if matches!(palette.mode, Mode::Search(_)) {
        3
    } else {
        1
    };
    let [prompt, rule, list, footer] = Layout::vertical([
        Constraint::Length(prompt_rows),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Line::from("─".repeat(usize::from(rule.width))).fg(DIM),
        rule,
    );
    if let Mode::Search(search) = &mut palette.mode {
        draw_search(frame, search, &app.tree, [prompt, list, footer]);
        return;
    }

    let input = palette.input.as_str();
    frame.render_widget(
        Line::from(vec![
            badge.fg(Color::Black).bg(BADGE_BG).bold(),
            sep.into(),
            input.into(),
        ]),
        prompt,
    );
    // Badge, separator, and input are ASCII, so bytes are columns.
    let typed = u16::try_from(badge.len() + sep.len() + input.len()).unwrap_or(u16::MAX);
    frame.set_cursor_position((prompt.x.saturating_add(typed), prompt.y));

    let status = match &mut palette.mode {
        Mode::Line => {
            let (old, new) = match &app.diff {
                DiffPane::Loaded(view) => (view.file.old.len(), view.file.new.len()),
                _ => (0, 0),
            };
            hint(input, old, new)
        }
        Mode::File(files) => {
            draw_hits(frame, files, &app.tree, list);
            let total = app.tree.files().count();
            format!("{} of {total} files · ↑↓ move · ⏎ open", files.hits.len())
        }
        Mode::Search(_) => unreachable!("drawn by draw_search"),
    };
    frame.render_widget(Line::from(format!(" {status}")).fg(DIM), footer);
}

/// Global search: three fields, the toggles, streamed results grouped by file, and a status footer.
fn draw_search(
    frame: &mut Frame,
    search: &mut Search,
    tree: &Tree,
    [prompt, list, footer]: [Rect; 3],
) {
    let rows: [Rect; 3] = Layout::vertical([Constraint::Length(1); 3]).areas(prompt);
    let count = format!("{} hits", search.hits);
    let right_width = u16::try_from(count.len()).unwrap_or(u16::MAX) + TOGGLES_WIDTH;
    let [first, right] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_width)]).areas(rows[0]);
    let x = right.x + right_width - TOGGLES_WIDTH;
    let mut spans = vec![Span::from(count).fg(DIM)];
    spans.extend(toggle_spans(
        &search.query,
        x,
        right.y,
        &mut search.toggle_areas,
    ));
    frame.render_widget(Line::from(spans), right);

    let areas = [first, rows[1], rows[2]];
    search.field_areas = areas;
    for ((badge, input), area) in BADGES.into_iter().zip(Input::ALL).zip(areas) {
        let text = match input {
            Input::Query => &search.query.text,
            Input::Include => &search.include,
            Input::Exclude => &search.exclude,
        };
        let active = search.input == input;
        let badge_bg = if active { BADGE_BG } else { SELECTED_BG };
        frame.render_widget(
            Line::from(vec![
                badge.fg(Color::Black).bg(badge_bg).bold(),
                " ".into(),
                text.as_str().into(),
            ]),
            area,
        );
        if active {
            let typed = badge.len() + 1 + text.chars().count();
            let x = area
                .x
                .saturating_add(u16::try_from(typed).unwrap_or(u16::MAX));
            frame.set_cursor_position((x.min(area.right()), area.y));
        }
    }

    draw_results(frame, search, tree, list);
    let status = if let Some(error) = search.error {
        Line::from(format!(" {error}")).fg(RED)
    } else {
        let plus = if search.hits == MAX_HITS { "+" } else { "" };
        let state = if search.done { "done" } else { "searching…" };
        let text = format!(
            " {}{plus} hits in {} files · {} filtered out · {state}",
            search.hits,
            search.groups.len(),
            search.filtered_out
        );
        Line::from(text).fg(DIM)
    };
    frame.render_widget(status, footer);
}

/// The visible slice of search results, scrolled to keep the selection in view.
fn draw_results(frame: &mut Frame, search: &mut Search, tree: &Tree, area: Rect) {
    search.list_area = area;
    if search.rows.is_empty() {
        let text = if search.query.text.is_empty() {
            " Type to search every changed file"
        } else if search.done {
            " No matches"
        } else {
            ""
        };
        frame.render_widget(Paragraph::new(text).fg(DIM), area);
        return;
    }
    let height = usize::from(area.height).max(1);
    let selected = search.list.selected();
    let mut offset = search.list.offset();
    if let Some(row) = selected {
        offset = offset.min(row).max((row + 1).saturating_sub(height));
    }
    *search.list.offset_mut() = offset;
    let end = (offset + height).min(search.rows.len());
    for (y, row) in (area.y..).zip(offset..end) {
        let line = result_row(search, tree, row, selected == Some(row), area.width);
        let rect = Rect {
            y,
            height: 1,
            ..area
        };
        frame.render_widget(line, rect);
    }
}

fn result_row(
    search: &Search,
    tree: &Tree,
    row: usize,
    selected: bool,
    width: u16,
) -> Line<'static> {
    let (group, hit) = search.rows[row];
    let group = &search.groups[group];
    let Some(hit) = hit.and_then(|hit| group.hits.get(hit)) else {
        let path = tree
            .file(group.node)
            .map_or_else(String::new, |f| f.path.display().to_string());
        let spans = vec![" ".into(), Span::from(path).bold()];
        return right_aligned(
            spans,
            vec![Span::from(group.hits.len().to_string()).fg(DIM)],
            width,
        );
    };
    let marker = match (hit.changed, hit.side) {
        (false, _) => " ".into(),
        (true, zdiff_core::Side::Old) => "-".fg(RED),
        (true, zdiff_core::Side::New) => "+".fg(GREEN),
    };
    let mut spans = vec![
        if selected {
            "▌".fg(ACCENT)
        } else {
            " ".into()
        },
        "  ".into(),
        marker,
        format!(" {:>5}  ", hit.line + 1).fg(DIM),
    ];
    let used: usize = spans.iter().map(Span::width).sum();
    let found = u32::from(hit.range.start)..u32::from(hit.range.end);
    let marks = Marks {
        tokens: &hit.tokens,
        found: std::slice::from_ref(&found),
        ..Marks::default()
    };
    let take = usize::from(width).saturating_sub(used + 1);
    let segments = text::segments(&hit.snippet, 0, marks, 0, take);
    spans.extend((segments.into_iter()).map(|(text, look)| styled(text, look, Color::Reset)));
    let line = Line::from(spans);
    if selected { line.bg(SELECTED_BG) } else { line }
}

/// Ranked results: status, name, and dim folder with matched chars highlighted, counts on the right.
fn draw_hits(frame: &mut Frame, files: &mut Files, tree: &Tree, area: Rect) {
    files.area = area;
    if files.hits.is_empty() {
        frame.render_widget(Paragraph::new(" No matches").fg(DIM), area);
        return;
    }
    let selected = files.list.selected();
    let rows: Vec<ListItem> = (files.hits.iter().enumerate())
        .filter_map(|(i, hit)| Some((i, hit, tree.file(hit.node)?)))
        .map(|(i, hit, entry)| ListItem::new(hit_row(hit, entry, selected == Some(i), area.width)))
        .collect();
    let list = List::new(rows).highlight_style(Style::new().bg(SELECTED_BG));
    frame.render_stateful_widget(list, area, &mut files.list);
}

fn hit_row(hit: &Hit, entry: &FileEntry, selected: bool, width: u16) -> Line<'static> {
    let path = entry.path.to_string_lossy();
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", &path));
    // Match positions count chars across the whole path; the name starts after `dir/`.
    let name_base = if dir.is_empty() {
        0
    } else {
        u32::try_from(dir.chars().count() + 1).unwrap_or(u32::MAX)
    };
    let mut spans = vec![
        if selected {
            "▌".fg(ACCENT)
        } else {
            " ".into()
        },
        entry.status.as_str().fg(status_color(entry.status)),
        " ".into(),
    ];
    spans.extend(marked(name, name_base, &hit.chars, Style::new()));
    spans.push("  ".into());
    spans.extend(marked(dir, 0, &hit.chars, Style::new().fg(DIM)));
    right_aligned(spans, counts(entry.added, entry.removed), width)
}

/// `text` as spans in `style`, with chars whose position (from `base`) is in `hits` in ACCENT.
fn marked(text: &str, base: u32, hits: &[u32], style: Style) -> Vec<Span<'static>> {
    let lit = style.fg(ACCENT).underlined();
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_lit = false;
    for (i, c) in (base..).zip(text.chars()) {
        let hit = hits.binary_search(&i).is_ok();
        if hit != run_lit && !run.is_empty() {
            spans.push(Span::styled(
                mem::take(&mut run),
                if run_lit { lit } else { style },
            ));
        }
        run_lit = hit;
        run.push(c);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, if run_lit { lit } else { style }));
    }
    spans
}

/// What Enter will do with `input`, given each side's line count.
fn hint(input: &str, old_len: u32, new_len: u32) -> String {
    let Some((side, line)) = palette::parse(input) else {
        return "Enter a line number · -N for the old side".into();
    };
    let (name, len) = match side {
        Side::Old => ("old", old_len),
        Side::New => ("new", new_len),
    };
    if len == 0 {
        format!("The {name} side is empty")
    } else if line > len {
        format!("Past the end: goes to {name} line {len}")
    } else {
        format!("Go to {name} line {} of {len}", line.max(1))
    }
}

#[cfg(test)]
mod tests {
    use zdiff_core::{FileDiff, Status};

    use super::*;
    use crate::app::FileEntry;

    #[test]
    fn hint_says_where_enter_goes() {
        assert_eq!(hint("", 5, 9), "Enter a line number · -N for the old side");
        assert_eq!(hint("-", 5, 9), "Enter a line number · -N for the old side");
        assert_eq!(hint("4", 5, 9), "Go to new line 4 of 9");
        assert_eq!(hint("+0", 5, 9), "Go to new line 1 of 9");
        assert_eq!(hint("-3", 5, 9), "Go to old line 3 of 5");
        assert_eq!(hint("99", 5, 9), "Past the end: goes to new line 9");
        assert_eq!(hint("-1", 0, 9), "The old side is empty");
    }

    #[test]
    fn modal_draws_over_the_panes_without_dimming_them() {
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = App::new(vec![FileEntry {
            path: "a.rs".into(),
            status: Status::Modified,
            added: 1,
            removed: 1,
            change: 0,
        }]);
        app.show(Some(Ok(FileDiff::new(
            b"a\nb\n".to_vec(),
            b"a\nc\n".to_vec(),
        ))));
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).expect("test backend");
        terminal
            .draw(|f| crate::ui::draw(f, &mut app))
            .expect("draw");
        let before = terminal.backend().buffer().clone();

        app.palette = Some(crate::palette::Palette::line());
        app.palette.as_mut().unwrap().input.push('2');
        terminal
            .draw(|f| crate::ui::draw(f, &mut app))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let row = |y: u16| -> String { (0..100).map(|x| buffer[(x, y)].symbol()).collect() };
        let modal = app.palette_area;
        assert!(
            row(modal.y + 1).contains(" LINE  :2"),
            "{:?}",
            row(modal.y + 1)
        );
        assert!(row(modal.y + 3).contains("Go to new line 2 of 2"));
        let outside = (0, modal.bottom());
        assert_eq!(
            buffer[outside], before[outside],
            "rest of the screen untouched"
        );
    }

    fn render_files(paths: &[&str], input: &str) -> (Vec<String>, Rect, ratatui::buffer::Buffer) {
        use ratatui::{Terminal, backend::TestBackend};

        let files = paths.iter().map(|&path| FileEntry {
            path: path.into(),
            status: Status::Modified,
            added: 3,
            removed: 1,
            change: 0,
        });
        let mut app = App::new(files.collect());
        let mut palette = crate::palette::Palette::files(&app.tree);
        palette.input = input.into();
        if let Mode::File(files) = &mut palette.mode {
            files.update(input, &app.tree);
        }
        app.palette = Some(palette);
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).expect("test backend");
        terminal
            .draw(|f| crate::ui::draw(f, &mut app))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        let rows = (0..20)
            .map(|y| (0..100).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        (rows, app.palette_area, buffer)
    }

    #[test]
    fn file_palette_lists_ranked_hits_with_highlighted_matches() {
        let (rows, modal, buffer) = render_files(&["src/rows.rs", "src/app.rs", "doc.md"], "rows");
        let row = |k: u16| rows[usize::from(modal.y + k)].as_str();
        assert!(row(1).contains(" FILE  rows"), "{:?}", row(1));
        assert!(row(3).contains("▌M rows.rs  src"), "{:?}", row(3));
        assert!(row(3).contains("+3 -1 │"), "{:?}", row(3));
        let footer = rows.iter().find(|r| r.contains("files ·")).expect("footer");
        assert!(footer.contains("1 of 3 files"), "{footer:?}");
        // First name char `r` is a match.
        let r = row(3).chars().position(|c| c == 'r').expect("name");
        let x = u16::try_from(r).unwrap();
        assert_eq!(buffer[(x, modal.y + 3)].fg, ACCENT);
    }

    #[test]
    fn file_palette_says_when_nothing_matches() {
        let (rows, modal, _) = render_files(&["a.rs"], "zzz");
        assert!(rows[usize::from(modal.y + 3)].contains(" No matches"));
        assert!(rows.iter().any(|r| r.contains("0 of 1 files")));
    }

    #[test]
    fn global_search_draws_fields_groups_hits_and_footer() {
        use ratatui::{Terminal, backend::TestBackend};

        use crate::palette::Palette;
        use crate::search::Found as Hit;

        let mut app = App::new(vec![FileEntry {
            path: "src/a.rs".into(),
            status: Status::Modified,
            added: 1,
            removed: 0,
            change: 0,
        }]);
        let query = zdiff_search::Query {
            text: "need".into(),
            ..zdiff_search::Query::default()
        };
        app.palette = Some(Palette::search(query, "src".into()));
        let node = app.tree.files().next().map(|(n, _)| n).unwrap();
        if let Some(Palette {
            mode: Mode::Search(search),
            ..
        }) = &mut app.palette
        {
            search.push(
                node,
                vec![Hit {
                    side: zdiff_core::Side::New,
                    line: 0,
                    changed: true,
                    snippet: b"let needle = 1;".to_vec().into(),
                    range: 4..8,
                    tokens: [zdiff_highlight::Token {
                        start: 0,
                        end: 3,
                        class: zdiff_highlight::Class::Keyword,
                    }]
                    .into(),
                }],
            );
        }
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).expect("test backend");
        terminal
            .draw(|f| crate::ui::draw(f, &mut app))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let rows: Vec<String> = (0..30)
            .map(|y| (0..120).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        let has = |text: &str| rows.iter().position(|r| r.contains(text));
        assert!(has(" SEARCH   need").is_some(), "{rows:#?}");
        assert!(has(" include  src").is_some());
        assert!(has(" exclude ").is_some());
        assert!(has("1 hits [Aa] [.*] [±]").is_some());
        assert!(has(" src/a.rs").is_some(), "file row");
        let hit = has("▌  +     1  let needle = 1;").expect("hit row");
        let y = u16::try_from(hit).unwrap();
        let n = (0..120).find(|&x| buffer[(x, y)].symbol() == "n").unwrap();
        assert_eq!(buffer[(n, y)].bg, crate::ui::FIND_BG, "match highlighted");
        let l = (0..120).find(|&x| buffer[(x, y)].symbol() == "l").unwrap();
        let keyword = crate::ui::class_color(zdiff_highlight::Class::Keyword);
        assert_eq!(buffer[(l, y)].fg, keyword, "`let` syntax colored");
        assert!(has("1 hits in 1 files · 0 filtered out · searching…").is_some());
    }
}

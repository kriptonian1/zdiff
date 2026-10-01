//! The palette popup (go to line or go to file), drawn over both panes without dimming them.

use std::mem;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, Paragraph};
use zdiff_core::Side;

use zdiff_search::MAX_HITS;

use super::find::{TOGGLES_WIDTH, toggle_spans};
use super::{Colors, counts, popup, right_aligned, styled};
use crate::app::{App, DiffPane, FileEntry};
use crate::palette::{self, Capture, Files, Hit, Input, Keys, Mode, Search, Themes};
use crate::text::{self, Marks};
use crate::tree::Tree;
use crate::ui::Theme;

/// Go-to-line box: border, input, rule, footer, border.
const LINE_SIZE: (u16, u16) = (60, 5);
/// Go-to-file box: room for about ten results.
const FILE_SIZE: (u16, u16) = (70, 16);
/// Global search box: three fields and about eighteen result rows.
const SEARCH_SIZE: (u16, u16) = (90, 26);
/// Shortcuts box: room for about fifteen rows.
const KEYS_SIZE: (u16, u16) = (84, 20);
/// Theme picker: room for about fifteen themes.
const THEMES_SIZE: (u16, u16) = (50, 20);
/// Columns for an action's label and its keys on the shortcuts screen.
const KEY_LABEL_WIDTH: usize = 34;
const KEY_KEYS_WIDTH: usize = 22;
/// Global search field badges, padded to one width so the inputs line up.
const BADGES: [&str; 3] = [" SEARCH  ", " include ", " exclude "];

pub(super) fn draw(frame: &mut Frame, app: &mut App) {
    let (c, current) = (app.shown_theme().colors(), app.theme);
    let Some(palette) = &mut app.palette else {
        return;
    };
    let ((width, height), badge, sep) = match palette.mode {
        Mode::Line => (LINE_SIZE, " LINE ", " :"),
        Mode::File(_) => (FILE_SIZE, " FILE ", " "),
        Mode::Search(_) => (SEARCH_SIZE, "", ""),
        Mode::Keys(_) => (KEYS_SIZE, " KEYS ", " "),
        Mode::Themes(_) => (THEMES_SIZE, " THEME ", " "),
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

    let block = popup(c);
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
        Line::from("─".repeat(usize::from(rule.width))).fg(c.dim),
        rule,
    );
    if let Mode::Search(search) = &mut palette.mode {
        draw_search(frame, c, search, &app.tree, [prompt, list, footer]);
        return;
    }

    // Badge and separator are ASCII, so bytes are columns.
    let label = u16::try_from(badge.len() + sep.len()).unwrap_or(u16::MAX);
    let [label_area, text] =
        Layout::horizontal([Constraint::Length(label), Constraint::Fill(1)]).areas(prompt);
    let prompt_line = Line::from(vec![badge.fg(c.badge_fg).bg(c.badge).bold(), sep.into()]);
    frame.render_widget(prompt_line, label_area);
    palette.field.draw(frame, text, c, true);
    let input = palette.field.text();
    let input = input.as_str();

    let status = match &mut palette.mode {
        Mode::Line => {
            let (old, new) = match &app.diff {
                DiffPane::Loaded(view) => (view.file.old.len(), view.file.new.len()),
                _ => (0, 0),
            };
            hint(input, old, new)
        }
        Mode::File(files) => {
            draw_hits(frame, c, files, &app.tree, list);
            let total = app.tree.files().count();
            format!("{} of {total} files · ↑↓ move · ⏎ open", files.hits.len())
        }
        Mode::Search(_) => unreachable!("drawn by draw_search"),
        Mode::Keys(keys) => {
            draw_keys(frame, c, keys, list);
            keys_status(keys)
        }
        Mode::Themes(themes) => {
            draw_themes(frame, c, themes, current, list);
            format!(
                "{} themes · ↑↓ preview · ⏎ apply · Esc close",
                themes.shown.len()
            )
        }
    };
    frame.render_widget(Line::from(format!(" {status}")).fg(c.dim), footer);
}

/// Global search: three fields, the toggles, streamed results grouped by file, and a status footer.
fn draw_search(
    frame: &mut Frame,
    c: &Colors,
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
    let mut spans = vec![Span::from(count).fg(c.dim)];
    spans.extend(toggle_spans(
        c,
        &search.query,
        x,
        right.y,
        &mut search.toggle_areas,
    ));
    frame.render_widget(Line::from(spans), right);

    let areas = [first, rows[1], rows[2]];
    search.field_areas = areas;
    let fields = BADGES.into_iter().zip(Input::ALL).zip(&mut search.fields);
    for (((badge, input), field), area) in fields.zip(areas) {
        let active = search.input == input;
        let (fg, bg) = if active {
            (c.badge_fg, c.badge)
        } else {
            (c.dim, c.selected)
        };
        let label = u16::try_from(badge.len() + 1).unwrap_or(u16::MAX);
        let [label_area, text] =
            Layout::horizontal([Constraint::Length(label), Constraint::Fill(1)]).areas(area);
        frame.render_widget(
            Line::from(vec![badge.fg(fg).bg(bg).bold(), " ".into()]),
            label_area,
        );
        field.draw(frame, text, c, active);
    }

    draw_results(frame, c, search, tree, list);
    let status = if let Some(error) = search.error {
        Line::from(format!(" {error}")).fg(c.red)
    } else {
        let plus = if search.hits == MAX_HITS { "+" } else { "" };
        let state = if search.done { "done" } else { "searching…" };
        let text = format!(
            " {}{plus} hits in {} files · {} filtered out · {state}",
            search.hits,
            search.groups.len(),
            search.filtered_out
        );
        Line::from(text).fg(c.dim)
    };
    frame.render_widget(status, footer);
}

/// The visible slice of search results, scrolled to keep the selection in view.
fn draw_results(frame: &mut Frame, c: &Colors, search: &mut Search, tree: &Tree, area: Rect) {
    search.list_area = area;
    if search.rows.is_empty() {
        let text = if search.query.text.is_empty() {
            " Type to search every changed file"
        } else if search.done {
            " No matches"
        } else {
            ""
        };
        frame.render_widget(Paragraph::new(text).fg(c.dim), area);
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
        let line = result_row(c, search, tree, row, selected == Some(row), area.width);
        let rect = Rect {
            y,
            height: 1,
            ..area
        };
        frame.render_widget(line, rect);
    }
}

fn result_row(
    c: &Colors,
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
            vec![Span::from(group.hits.len().to_string()).fg(c.dim)],
            width,
        );
    };
    let marker = match (hit.changed, hit.side) {
        (false, _) => " ".into(),
        (true, zdiff_core::Side::Old) => "-".fg(c.red),
        (true, zdiff_core::Side::New) => "+".fg(c.green),
    };
    let mut spans = vec![
        if selected {
            "▌".fg(c.accent)
        } else {
            " ".into()
        },
        "  ".into(),
        marker,
        format!(" {:>5}  ", hit.line + 1).fg(c.dim),
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
    spans.extend((segments.into_iter()).map(|(text, look)| styled(c, text, look, c.bg)));
    let line = Line::from(spans);
    if selected { line.bg(c.selected) } else { line }
}

/// Ranked results: status, name, and dim folder with matched chars highlighted, counts on the right.
/// The shortcuts footer: the rebinding prompt, or the count and what the keys do.
fn keys_status(keys: &Keys) -> String {
    match keys.capture {
        Some(Capture {
            taken: Some((key, holder)),
            ..
        }) => format!(
            "{key} is \"{}\" · Enter take it · Esc cancel",
            holder.label()
        ),
        Some(capture) => format!(
            "press a key for \"{}\" · Esc cancel",
            capture.action.label()
        ),
        None => format!(
            "{} shortcuts · ⏎ change · ⇧⏎ add a key · Del reset · Esc close",
            keys.shown.len()
        ),
    }
}

/// The shortcuts screen: action, its keys, and where they work; fixed keys get a lock.
fn draw_keys(frame: &mut Frame, c: &Colors, keys: &mut Keys, area: Rect) {
    keys.area = area;
    if keys.shown.is_empty() {
        frame.render_widget(Paragraph::new(" No matches").fg(c.dim), area);
        return;
    }
    let rows: Vec<ListItem> = (keys.shown.iter())
        .map(|&i| {
            let row = &keys.rows[i];
            let mark = match (row.action, row.edited) {
                (None, _) => " 🔒",
                (Some(_), true) => " ✎",
                (Some(_), false) => "",
            };
            ListItem::new(Line::from(vec![
                format!(" {:<KEY_LABEL_WIDTH$} ", row.label).into(),
                format!("{:<KEY_KEYS_WIDTH$} ", row.keys).fg(c.accent),
                row.place.fg(c.dim),
                mark.into(),
            ]))
        })
        .collect();
    let list = List::new(rows).highlight_style(Style::new().bg(c.selected));
    frame.render_stateful_widget(list, area, &mut keys.list);
}

/// One row per theme, with a check on the one in use.
fn draw_themes(frame: &mut Frame, c: &Colors, themes: &mut Themes, current: Theme, area: Rect) {
    themes.area = area;
    if themes.shown.is_empty() {
        frame.render_widget(Paragraph::new(" No matches").fg(c.dim), area);
        return;
    }
    let rows = themes.shown.iter().map(|&theme| {
        let check = if theme == current { "✓" } else { " " };
        ListItem::new(format!(" {check} {}", theme.label()))
    });
    let list = List::new(rows).highlight_style(Style::new().bg(c.selected));
    frame.render_stateful_widget(list, area, &mut themes.list);
}

fn draw_hits(frame: &mut Frame, c: &Colors, files: &mut Files, tree: &Tree, area: Rect) {
    files.area = area;
    if files.hits.is_empty() {
        frame.render_widget(Paragraph::new(" No matches").fg(c.dim), area);
        return;
    }
    let selected = files.list.selected();
    let rows: Vec<ListItem> = (files.hits.iter().enumerate())
        .filter_map(|(i, hit)| Some((i, hit, tree.file(hit.node)?)))
        .map(|(i, hit, entry)| {
            ListItem::new(hit_row(c, hit, entry, selected == Some(i), area.width))
        })
        .collect();
    let list = List::new(rows).highlight_style(Style::new().bg(c.selected));
    frame.render_stateful_widget(list, area, &mut files.list);
}

fn hit_row(c: &Colors, hit: &Hit, entry: &FileEntry, selected: bool, width: u16) -> Line<'static> {
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
            "▌".fg(c.accent)
        } else {
            " ".into()
        },
        entry.status.as_str().fg(c.status(entry.status)),
        " ".into(),
    ];
    let lit = Style::new().fg(c.accent).underlined();
    spans.extend(marked(name, name_base, &hit.chars, (Style::new(), lit)));
    spans.push("  ".into());
    spans.extend(marked(dir, 0, &hit.chars, (Style::new().fg(c.dim), lit)));
    right_aligned(spans, counts(c, entry.added, entry.removed), width)
}

/// `text` as spans in `style`, with chars whose position (from `base`) is in `hits` in `lit`.
fn marked(text: &str, base: u32, hits: &[u32], (style, lit): (Style, Style)) -> Vec<Span<'static>> {
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
    use crate::ui::DARK;

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
            staged: zdiff_core::Staged::No,
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
        app.palette.as_mut().unwrap().field = crate::input::Field::single("2");
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
            staged: zdiff_core::Staged::No,
        });
        let mut app = App::new(files.collect());
        let mut palette = crate::palette::Palette::files(&app.tree);
        palette.field = crate::input::Field::single(input);
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
        assert_eq!(buffer[(x, modal.y + 3)].fg, DARK.accent);
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
            staged: zdiff_core::Staged::No,
        }]);
        let query = zdiff_search::Query {
            text: "need".into(),
            ..zdiff_search::Query::default()
        };
        app.palette = Some(Palette::search(query, "src"));
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
        assert_eq!(buffer[(n, y)].bg, DARK.find, "match highlighted");
        let l = (0..120).find(|&x| buffer[(x, y)].symbol() == "l").unwrap();
        let keyword = DARK.class(zdiff_highlight::Class::Keyword);
        assert_eq!(buffer[(l, y)].fg, keyword, "`let` syntax colored");
        assert!(has("1 hits in 1 files · 0 filtered out · searching…").is_some());
    }

    #[test]
    fn shortcuts_screen_lists_actions_with_their_keys_and_place() {
        let mut app = App::new(Vec::new());
        app.palette = Some(crate::palette::Palette::keys(&app.keymap));
        let screen = crate::ui::render(&mut app, 100, 30);
        let row = (screen.iter())
            .find(|row| row.contains("Sidebar ") && row.contains("Ctrl+B"))
            .expect("a Sidebar row");
        assert!(row.contains("everywhere"), "{row:?}");

        app.keymap
            .set(crate::menu::Action::NextFile, &["m".parse().unwrap()]);
        app.palette = Some(crate::palette::Palette::keys(&app.keymap));
        if let Some(crate::palette::Palette {
            mode: Mode::Keys(keys),
            ..
        }) = &mut app.palette
        {
            let action = crate::menu::Action::NextFile;
            keys.capture = Some(Capture {
                action,
                add: false,
                taken: Some(("Ctrl+B".parse().unwrap(), crate::menu::Action::Sidebar)),
            });
        }
        let rebinding = crate::ui::render(&mut app, 100, 30);
        let edited = rebinding
            .iter()
            .find(|row| row.contains("Next file"))
            .expect("row");
        assert!(edited.contains(" m ") && edited.contains('✎'), "{edited:?}");
        assert!(
            (rebinding.iter()).any(|row| row.contains("Ctrl+B is \"Sidebar\" · Enter take it")),
            "the taken prompt"
        );
        assert!(
            screen
                .iter()
                .any(|row| row.contains("shortcuts · ⏎ change · ⇧⏎ add a key · Del reset"))
        );
    }

    #[test]
    fn inactive_search_badges_stay_readable_in_the_light_theme() {
        use ratatui::{Terminal, backend::TestBackend};

        use crate::palette::Palette;

        let mut app = App::new(Vec::new());
        app.theme = crate::ui::Theme::GithubLight;
        app.palette = Some(Palette::search(zdiff_search::Query::default(), ""));
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("test backend");
        terminal
            .draw(|f| crate::ui::draw(f, &mut app))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let c = app.theme.colors();
        let Some(Palette {
            mode: Mode::Search(search),
            ..
        }) = &app.palette
        else {
            panic!("search is open");
        };
        let [query, include, _] = search.field_areas;
        let cell = |area: Rect| &buffer[(area.x + 1, area.y)];
        assert_eq!((cell(query).fg, cell(query).bg), (c.badge_fg, c.badge));
        assert_eq!(cell(include).symbol(), "i");
        assert_eq!((cell(include).fg, cell(include).bg), (c.dim, c.selected));
    }

    #[test]
    fn theme_picker_lists_every_theme_and_checks_the_current_one() {
        let mut app = App::new(Vec::new());
        app.theme = crate::ui::Theme::GithubLight;
        app.palette = Some(crate::palette::Palette::themes(app.theme));
        let screen = crate::ui::render(&mut app, 100, 30);
        let find = |text: &str| screen.iter().find(|row| row.contains(text)).cloned();
        assert!(find(" THEME ").is_some(), "{screen:#?}");
        assert!(find("  GitHub Dark").is_some(), "{screen:#?}");
        assert!(find("✓ GitHub Light").is_some(), "{screen:#?}");
    }
}

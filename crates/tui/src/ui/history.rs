//! The history and stash popup: the commit graph and list, the selected commit's details
//! and files, and a unified preview of the selected file.

use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use zdiff_core::{Cell, Commit, GraphRow};

use super::diff::{Paint, draw_view};
use super::{Colors, counts, popup, right_aligned};
use crate::app::App;
use crate::history::{Ask, Button, History, Kind, Pane, short, stash_name};

/// Terminals at least this wide get the large popup; narrower ones a centred column.
const WIDE: u16 = 90;
/// Width of the popup on narrow terminals.
const NARROW_WIDTH: u16 = 40;
/// Columns of summary kept before a commit's names are dropped, then before its age is.
const NAMES_KEEP: usize = 24;
const AGE_KEEP: usize = 12;
/// Lines of the commit message shown under the list.
const MESSAGE_LINES: usize = 4;

pub(super) fn draw(frame: &mut Frame, app: &mut App) {
    let c = app.shown_theme().colors();
    let paint = Paint::unified(app);
    let Some(history) = &mut app.history else {
        return;
    };
    let screen = frame.area();
    let wide = screen.width >= WIDE;
    let width = if wide {
        screen.width * 9 / 10
    } else {
        NARROW_WIDTH.min(screen.width)
    };
    let height = screen.height * 9 / 10;
    let area = Rect {
        x: screen.x + (screen.width - width) / 2,
        y: screen.y + (screen.height - height) / 2,
        width,
        height,
    };
    history.area = area;
    let title = match history.kind {
        Kind::Log => " History ",
        Kind::Stash => " Stash ",
    };
    let block = popup(c).title(title);
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    // The push form and branch name need a line for their text above the buttons.
    let form = matches!(history.ask, Some(Ask::Push { .. } | Ask::Branch { .. }));
    let [main, hint_rule, hint] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(if form { 2 } else { 1 }),
    ])
    .areas(inner);
    frame.render_widget(rule(c, "─", inner.width), hint_rule);
    draw_buttons(frame, c, history, hint);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    // Narrow popups have no room beside the lists, so an open preview takes the whole popup.
    if !wide {
        if history.pane == Pane::Preview {
            (history.list_area, history.files_area) = Default::default();
            draw_preview(frame, (c, paint), history, main);
        } else {
            history.preview_area = Rect::default();
            draw_lists(frame, c, history, main, now);
        }
        return;
    }
    let left_width = (main.width * 2 / 5).max(NARROW_WIDTH).min(main.width);
    let [left, line, right] = Layout::horizontal([
        Constraint::Length(left_width),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(main);
    let bar = vec![Line::from("│").fg(c.dim); usize::from(line.height)];
    frame.render_widget(Paragraph::new(bar), line);
    draw_lists(frame, c, history, left, now);
    draw_preview(frame, (c, paint), history, right);
}

/// The commit list, the selected commit's details, and its files.
fn draw_lists(frame: &mut Frame, c: &Colors, history: &mut History, area: Rect, now: i64) {
    let commit = history.selected_commit();
    let message = commit.map_or(0, |c| c.message.lines().count().min(MESSAGE_LINES));
    let base = usize::from(commit.is_some_and(Commit::is_stash));
    let details = u16::try_from(1 + base + message).unwrap_or(1);
    // Details and files get at most 40% of the height; the commit list the rest.
    let below = (area.height * 2 / 5).max(details + 2);
    let files = below.saturating_sub(details + 2).max(1);
    let [list, divider, details_area, line, files_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(details),
        Constraint::Length(1),
        Constraint::Length(files),
    ])
    .areas(area);
    frame.render_widget(rule(c, "═", area.width), divider);
    frame.render_widget(rule(c, "─", area.width), line);
    draw_commits(frame, c, history, list, now);
    history.copy_areas = Default::default();
    if let Some(commit) = history.selected_commit() {
        let (details, copy) = details_paragraph(c, (commit, history.head.as_deref()), now);
        frame.render_widget(details, details_area);
        history.copy_areas = copy.map(|(x, width)| {
            Rect::new(details_area.x + x, details_area.y, width, 1).intersection(details_area)
        });
    }
    draw_files(frame, c, history, files_area);
}

/// `[/ search] [↵ files] …` for the current pane, as many as fit; records each for clicks.
/// A drop waiting for confirmation asks first, in red; the push form and branch name draw
/// their text on the line above.
fn draw_buttons(frame: &mut Frame, c: &Colors, history: &mut History, area: Rect) {
    history.buttons.clear();
    let area = if area.height > 1 {
        let [text, buttons] = Layout::vertical([Constraint::Length(1); 2]).areas(area);
        draw_ask_text(frame, c, history, text);
        buttons
    } else {
        area
    };
    let mut x = area.x + 1;
    let asking = matches!(history.ask, Some(Ask::Drop(_))).then(|| history.selected_commit());
    if let Some(commit) = asking.flatten() {
        let name = stash_name(commit).unwrap_or_else(|| short(&commit.id));
        let ask = format!("drop {name} “{}”? ", commit.summary);
        let room = usize::from(area.width).saturating_sub(24);
        let ask = fit(&ask, room);
        let width = u16::try_from(ask.width()).unwrap_or(u16::MAX);
        frame.render_widget(Line::from(ask).fg(c.red), Rect::new(x, area.y, width, 1));
        x += width;
    }
    let focused = history.focused_toggle();
    for &button in history.buttons() {
        let label = history.label(button);
        let width = u16::try_from(label.width()).unwrap_or(u16::MAX);
        if x + width > area.right() {
            break;
        }
        let at = Rect::new(x, area.y, width, 1);
        let color = if button == Button::Back {
            c.dim
        } else {
            c.accent
        };
        let mut line = Line::from(label).fg(color);
        if focused.is_some_and(|n| button == Button::Toggle(n)) {
            line = line.underlined();
        }
        frame.render_widget(line, at);
        history.buttons.push((at, button));
        x += width + 1;
    }
}

/// ` message: text` for the push form, ` new branch from stash@{0}: name` for a branch.
fn draw_ask_text(frame: &mut Frame, c: &Colors, history: &mut History, area: Rect) {
    let stash = history
        .selected_commit()
        .and_then(stash_name)
        .unwrap_or("stash");
    let prompt = match &history.ask {
        Some(Ask::Branch { .. }) => format!(" new branch from {stash}: "),
        _ => " message: ".to_owned(),
    };
    let width = u16::try_from(prompt.width()).unwrap_or(u16::MAX);
    let [label, field] =
        Layout::horizontal([Constraint::Length(width), Constraint::Fill(1)]).areas(area);
    frame.render_widget(Line::from(prompt).fg(c.accent), label);
    if let Some(input) = history.ask_field() {
        input.draw(frame, field, c, true);
    } else if let Some(Ask::Push { message, .. }) = &mut history.ask {
        // Focus is on a toggle: the message shows without its cursor.
        message.draw(frame, field, c, false);
    }
}

/// ` / text   3 of 412+  ✕`: the search box, its count, and the button that clears it.
fn draw_search(frame: &mut Frame, c: &Colors, history: &mut History, area: Rect) {
    history.search_area = area;
    let total = history.commits.len();
    let more = if history.done { "" } else { "+" };
    let count = format!(" {} of {total}{more} ", history.shown.len());
    let count_width = u16::try_from(count.width()).unwrap_or(u16::MAX);
    let [slash, field, counter, clear] = Layout::horizontal([
        Constraint::Length(3),
        Constraint::Fill(1),
        Constraint::Length(count_width),
        Constraint::Length(3),
    ])
    .areas(area);
    frame.render_widget(Line::from(" / ").fg(c.accent), slash);
    if let Some(input) = &mut history.search {
        input.draw(frame, field, c, history.typing);
    }
    frame.render_widget(Line::from(count).fg(c.dim), counter);
    frame.render_widget(Line::from(" ✕ ").fg(c.dim), clear);
    history.clear_area = clear;
}

/// The selected file's path and counts, then its unified diff.
fn draw_preview(
    frame: &mut Frame,
    (c, paint): (&Colors, Paint),
    history: &mut History,
    area: Rect,
) {
    let [header, line, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    history.preview_area = body;
    frame.render_widget(rule(c, "─", area.width), line);
    let Some(file) = history.selected_file() else {
        let text = if history.files_loading() {
            " loading…"
        } else {
            ""
        };
        frame.render_widget(Line::from(text).fg(c.dim), body);
        return;
    };
    let path = format!(" {}", file.path.display());
    let path = if history.pane == Pane::Preview {
        path.fg(c.accent).bold()
    } else {
        path.into()
    };
    let mut right = vec![format!("{}  ", file.status.as_str()).fg(c.status(file.status))];
    right.extend(counts(c, file.added, file.removed));
    frame.render_widget(right_aligned(vec![path], right, header.width), header);
    let lines = file.added + file.removed;
    match &mut history.preview {
        _ if history.large => {
            let text = format!(" ▸ Large diff: {lines} lines changed · Enter to load");
            frame.render_widget(Line::from(text).fg(c.dim), body);
        }
        Some(view) => draw_view(frame, view, paint, body),
        None => frame.render_widget(Line::from(" loading…").fg(c.dim), body),
    }
}

/// A full-width line of `ch`, dimmed.
fn rule(c: &Colors, ch: &str, width: u16) -> Line<'static> {
    Line::from(ch.repeat(usize::from(width))).fg(c.dim)
}

fn draw_commits(frame: &mut Frame, c: &Colors, history: &mut History, area: Rect, now: i64) {
    let area = if history.search.is_some() {
        let [search, rest] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        draw_search(frame, c, history, search);
        rest
    } else {
        (history.search_area, history.clear_area) = Default::default();
        area
    };
    history.list_area = area;
    let height = usize::from(area.height);
    if history.commits.is_empty() {
        let text = if !history.done {
            " loading…"
        } else if history.kind == Kind::Stash {
            " No stashes"
        } else {
            " No commits yet"
        };
        frame.render_widget(Line::from(text).fg(c.dim), area);
        return;
    }
    let query = history.query();
    if history.shown.is_empty() {
        let text = if history.done {
            " No matching commits"
        } else {
            " searching…"
        };
        frame.render_widget(Line::from(text).fg(c.dim), area);
        return;
    }
    let at = history.position().unwrap_or(0);
    history.scroll = visible_from(history.scroll, at, height);
    let focused = history.pane == Pane::Commits;
    let rows = history.shown.iter().skip(history.scroll).take(height);
    for (y, &i) in (area.y..).zip(rows) {
        let selected = i == history.selected;
        // Lines don't join up across a filtered list, so matches show as plain dots.
        let graph = history.rows.get(i).filter(|_| query.is_empty());
        let row = (&history.commits[i], graph, query.as_str());
        let mut line = commit_line(c, row, now, area.width, selected && focused);
        if selected {
            line = line.bg(c.selected);
        }
        frame.render_widget(
            line,
            Rect {
                y,
                height: 1,
                ..area
            },
        );
    }
    if !history.done && history.scroll + height >= history.shown.len() && height > 0 {
        let last = Rect {
            y: area.bottom() - 1,
            height: 1,
            ..area
        };
        frame.render_widget(Line::from(" ┊ loading more…").fg(c.dim), last);
    }
}

/// One commit: graph, short id, summary, then its age and names on the right.
fn commit_line(
    c: &Colors,
    (commit, graph, query): (&Commit, Option<&GraphRow>, &str),
    now: i64,
    width: u16,
    marked: bool,
) -> Line<'static> {
    let mut spans = vec![if marked {
        "▌".fg(c.accent)
    } else {
        " ".into()
    }];
    match graph {
        Some(graph) => spans.extend(graph_spans(c, graph, commit.head)),
        None => spans.push(if commit.head { "★ " } else { "● " }.fg(c.accent)),
    }
    // A stash leads with its `stash@{n}` instead of the hash, so it isn't repeated as a name.
    let stash = stash_name(commit);
    spans.push(format!("{} ", stash.unwrap_or_else(|| short(&commit.id))).fg(c.dim));
    let refs = if stash.is_some() {
        &[][..]
    } else {
        &commit.refs
    };
    let names = (refs.iter()).fold(String::new(), |mut names, name| {
        let _ = write!(names, " ‹{name}›");
        names
    });
    let age = format!(" {}", age(now, commit.time));
    let used: usize = spans.iter().map(Span::width).sum();
    let room = usize::from(width).saturating_sub(used + 1);
    let summary = commit.summary.width();
    let mut right = Vec::new();
    if room >= summary.min(AGE_KEEP) + age.len() {
        right.push(age.clone().fg(c.dim));
    }
    if !names.is_empty() && room >= summary.min(NAMES_KEEP) + age.len() + names.width() {
        right.push(names.fg(c.accent));
    }
    let right_width: usize = right.iter().map(Span::width).sum();
    let summary = fit(&commit.summary, room.saturating_sub(right_width + 1));
    spans.extend(highlight(c, summary, query));
    right_aligned(spans, right, width)
}

/// `text` with the first place it contains `query` on the search background; lowercase
/// queries ignore case, as the search does.
fn highlight(c: &Colors, text: String, query: &str) -> Vec<Span<'static>> {
    let found = if query.is_empty() {
        None
    } else if query.chars().any(char::is_uppercase) {
        text.find(query).map(|at| at..at + query.len())
    } else {
        // Lowercasing keeps byte offsets for ASCII; other text just isn't highlighted.
        (text
            .is_ascii()
            .then(|| text.to_ascii_lowercase().find(query)))
        .flatten()
        .map(|at| at..at + query.len())
    };
    let Some(range) = found else {
        return vec![text.into()];
    };
    vec![
        text[..range.start].to_owned().into(),
        text[range.clone()].to_owned().bg(c.find),
        text[range.end..].to_owned().into(),
    ]
}

/// The lanes of one row, two columns each, colored by lane.
fn graph_spans(c: &Colors, row: &GraphRow, head: bool) -> Vec<Span<'static>> {
    let lanes = [c.accent, c.green, c.yellow, c.red];
    let color = |lane: usize| lanes[lane % lanes.len()];
    let (low, high) = row.span;
    let mut spans = Vec::with_capacity(row.cells.len() * 2 + 2);
    for (lane, cell) in row.cells.iter().enumerate() {
        let glyph = match cell {
            Cell::Node | Cell::Merge if head => "★",
            Cell::Node => "●",
            Cell::Merge => "◆",
            Cell::Pass => "│",
            Cell::Join => "╯",
            Cell::Fork if lane > row.node => "╮",
            Cell::Fork => "╭",
            Cell::Empty if low < lane && lane < high => "─",
            Cell::Empty => " ",
        };
        // The horizontal line takes the commit's color; lines keep their own.
        let tint = if matches!(cell, Cell::Empty) {
            color(row.node)
        } else {
            color(lane)
        };
        spans.push(glyph.fg(tint));
        let across = (low..high).contains(&lane);
        spans.push(if across { "─" } else { " " }.fg(color(row.node)));
    }
    if row.overflow {
        spans.push("┊ ".fg(c.dim));
    }
    spans
}

/// `a1b2c3d · Sawan · 5h ago`, then the message.
/// The details, and where on their first line the hash and the `⧉` that copy it sit, as
/// `(column, width)`.
/// A stash adds ` on <base>`, warning when HEAD has moved off it.
fn details_paragraph(
    c: &Colors,
    (commit, head): (&Commit, Option<&str>),
    now: i64,
) -> (Paragraph<'static>, [(u16, u16); 2]) {
    let id = short(&commit.id).to_owned();
    let about = format!(" · {} · {} ago ", commit.author, age(now, commit.time));
    let width = |text: &str| u16::try_from(text.width()).unwrap_or(u16::MAX);
    let copy = [(1, width(&id)), (1 + width(&id) + width(&about), 1)];
    let mut lines = vec![Line::from(vec![
        " ".into(),
        id.fg(c.accent),
        about.fg(c.dim),
        "⧉".fg(c.accent),
    ])];
    if commit.is_stash() {
        let base = commit.parents.first().map_or("", String::as_str);
        let mut line = vec![format!(" on {}", short(base)).fg(c.dim)];
        if head.is_some_and(|head| head != base) {
            line.push(" · ⚠ HEAD moved since".fg(c.yellow));
        }
        lines.push(Line::from(line));
    }
    lines.extend(
        (commit.message.lines().take(MESSAGE_LINES)).map(|line| Line::from(format!(" {line}"))),
    );
    (Paragraph::new(lines).wrap(Wrap { trim: false }), copy)
}

fn draw_files(frame: &mut Frame, c: &Colors, history: &mut History, area: Rect) {
    history.files_area = area;
    let height = usize::from(area.height);
    if history.files.is_empty() {
        let text = if history.files_loading() {
            " loading…"
        } else if history.selected_commit().is_some() {
            " No files"
        } else {
            ""
        };
        frame.render_widget(Line::from(text).fg(c.dim), area);
        return;
    }
    history.file_scroll = visible_from(history.file_scroll, history.file, height);
    // The previewed file stands out from every pane; the bar shows where keys go, as in the
    // commit list.
    let focused = history.pane != Pane::Commits;
    let previewed = history.selected_file().is_some().then_some(history.file);
    let rows = history
        .files
        .iter()
        .enumerate()
        .skip(history.file_scroll)
        .take(height);
    for (y, (i, file)) in (area.y..).zip(rows) {
        let selected = previewed == Some(i);
        let left = vec![
            if selected && focused {
                "▌".fg(c.accent)
            } else {
                " ".into()
            },
            format!("{} ", file.status.as_str()).fg(c.status(file.status)),
            file.path.display().to_string().into(),
        ];
        let mut line = right_aligned(left, counts(c, file.added, file.removed), area.width);
        if selected {
            line = line.bg(c.selected);
        }
        frame.render_widget(
            line,
            Rect {
                y,
                height: 1,
                ..area
            },
        );
    }
}

/// The first row to draw so `selected` stays in a `height`-row window that starts at `top`.
fn visible_from(top: usize, selected: usize, height: usize) -> usize {
    if selected < top {
        selected
    } else if height > 0 && selected >= top + height {
        selected + 1 - height
    } else {
        top
    }
}

/// `5m`, `5h`, `3d`, `2w`, `4mo`, `2y`: how long before `now` `time` was.
pub(super) fn age(now: i64, time: i64) -> String {
    let seconds = (now - time).max(0);
    let (minute, hour, day) = (60, 3600, 86_400);
    match seconds {
        s if s < hour => format!("{}m", s / minute),
        s if s < day => format!("{}h", s / hour),
        s if s < 7 * day => format!("{}d", s / day),
        s if s < 30 * day => format!("{}w", s / (7 * day)),
        s if s < 365 * day => format!("{}mo", s / (30 * day)),
        s => format!("{}y", s / (365 * day)),
    }
}

/// `text` cut to `width` columns, ending in `…` when cut.
fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        used += w;
        out.push(ch);
    }
    if width > 0 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::render;
    use super::*;
    use crate::history::commit;

    fn now() -> i64 {
        i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
        .unwrap()
    }

    fn open(app: &mut App) {
        let mut merge = commit("m000000aa", &["a000000aa", "b000000aa"]);
        merge.refs = vec!["main".to_owned()].into();
        merge.head = true;
        merge.summary = "Merge side work".into();
        merge.message = "Merge side work\n\nwith a body".into();
        let mut commits = vec![
            merge,
            commit("b000000aa", &["a000000aa"]),
            commit("a000000aa", &[]),
        ];
        for (i, c) in commits.iter_mut().enumerate() {
            c.time = now() - 3600 * (i64::try_from(i).unwrap() + 1);
        }
        let (mut history, _) = History::new(Kind::Log);
        history.loaded(commits);
        app.history = Some(Box::new(history));
    }

    #[test]
    fn the_popup_draws_the_graph_names_details_and_hint() {
        let mut app = App::new(Vec::new());
        open(&mut app);
        let screen = render(&mut app, 120, 30);
        let has = |text: &str| screen.iter().any(|row| row.contains(text));
        assert!(has("★─╮ m000000 Merge side work"), "{screen:#?}");
        assert!(has("‹main›") && has(" 1h"), "{screen:#?}");
        assert!(has("│ ● b000000"), "{screen:#?}");
        assert!(has("●─╯ a000000"), "{screen:#?}");
        assert!(
            has("m000000 · Sawan · 1h ago") && has("with a body"),
            "{screen:#?}"
        );
        assert!(
            has("esc close") && has("loading…"),
            "files not loaded yet: {screen:#?}"
        );
    }

    #[test]
    fn a_narrow_popup_drops_the_names_before_the_age() {
        let mut app = App::new(Vec::new());
        open(&mut app);
        let screen = render(&mut app, 70, 30);
        let has = |text: &str| screen.iter().any(|row| row.contains(text));
        assert!(!has("‹main›") && has(" 1h"), "{screen:#?}");
    }

    /// Selects the merge's files: a small `src/app.rs` with its diff loaded, and a large lock file.
    fn with_preview(app: &mut App) {
        let history = app.history.as_mut().expect("open");
        let file = |path: &str, added| crate::app::FileEntry {
            path: path.into(),
            status: zdiff_core::Status::Modified,
            added,
            removed: 1,
            change: 0,
            staged: zdiff_core::Staged::No,
        };
        let files = vec![file("src/app.rs", 1), file("Cargo.lock", 900)];
        history
            .files_loaded("m000000aa", files)
            .expect("asks for the diff");
        let shape = crate::app::Shape {
            view: crate::app::View::Unified,
            context: 3,
        };
        let diff =
            zdiff_core::FileDiff::new(b"let old = 1;\n".to_vec(), b"let new = 2;\n".to_vec());
        let view = crate::app::DiffView::new(diff, None, shape);
        assert!(history.preview_loaded("m000000aa", std::path::Path::new("src/app.rs"), view));
    }

    #[test]
    fn a_wide_popup_previews_the_selected_file_unified_on_the_right() {
        let mut app = App::new(Vec::new());
        open(&mut app);
        with_preview(&mut app);
        let screen = render(&mut app, 140, 30);
        let has = |text: &str| screen.iter().any(|row| row.contains(text));
        let row = |text: &str| {
            screen
                .iter()
                .find(|row| row.contains(text))
                .cloned()
                .unwrap_or_default()
        };
        assert!(
            row("Merge side work").contains("│ src/app.rs"),
            "list left, preview right: {screen:#?}"
        );
        assert!(
            has("- let old = 1;") && has("+ let new = 2;"),
            "unified rows: {screen:#?}"
        );
        assert!(has("[o open]") && has("[y copy hash]"), "{screen:#?}");

        let history = app.history.as_mut().expect("open");
        history.pane = Pane::Files;
        let (_, wants) = history.step(1);
        assert!(wants.is_empty(), "a large file waits for Enter");
        let screen = render(&mut app, 140, 30);
        assert!(
            screen
                .iter()
                .any(|r| r.contains("Large diff: 901 lines changed · Enter to load")),
            "{screen:#?}"
        );
        let (moved, want) = app.history.as_mut().expect("open").enter();
        assert!(moved && matches!(want, Some(crate::history::Want::Preview { .. })));
    }

    #[test]
    fn the_previewed_file_is_highlighted_whichever_pane_has_the_keys() {
        let mut app = App::new(Vec::new());
        open(&mut app);
        with_preview(&mut app);
        let file_row = |app: &mut App| {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 30)).unwrap();
            terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
            let buffer = terminal.backend().buffer().clone();
            let area = app.history.as_ref().expect("open").files_area;
            let text: String = (area.x..area.right())
                .map(|x| buffer[(x, area.y)].symbol())
                .collect();
            (text, buffer[(area.x + 2, area.y)].bg)
        };
        for pane in [Pane::Commits, Pane::Files, Pane::Preview] {
            app.history.as_mut().expect("open").pane = pane;
            let (text, bg) = file_row(&mut app);
            assert!(text.contains("src/app.rs"), "{text}");
            assert_eq!(
                bg,
                crate::ui::DARK.selected,
                "{pane:?}: the previewed file stands out"
            );
            let marker = text.starts_with('▌');
            assert_eq!(
                marker,
                pane != Pane::Commits,
                "{pane:?}: the bar shows where keys go"
            );
        }
    }

    #[test]
    fn a_narrow_popup_shows_the_preview_on_its_own() {
        let mut app = App::new(Vec::new());
        open(&mut app);
        with_preview(&mut app);
        let has = |screen: &[String], text: &str| screen.iter().any(|row| row.contains(text));
        let screen = render(&mut app, 70, 30);
        assert!(!has(&screen, "let new"), "the lists only: {screen:#?}");
        app.history.as_mut().expect("open").pane = Pane::Preview;
        let screen = render(&mut app, 70, 30);
        assert!(
            has(&screen, "+ let new = 2;") && !has(&screen, "Merge side work"),
            "{screen:#?}"
        );
    }

    #[test]
    fn a_search_shows_its_box_count_plain_dots_and_the_match_highlighted() {
        let mut app = App::new(Vec::new());
        open(&mut app);
        let history = app.history.as_mut().expect("open");
        history.start_search();
        history.search = Some(crate::input::Field::single("side"));
        history.filter();
        let screen = render(&mut app, 140, 30);
        let has = |text: &str| screen.iter().any(|row| row.contains(text));
        assert!(
            has(" / side") && has(" 1 of 3 ") && has(" ✕ "),
            "{screen:#?}"
        );
        assert!(
            has("▌★ m000000 Merge side work"),
            "no graph lines: {screen:#?}"
        );
        assert!(!has("●─╯"), "{screen:#?}");

        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 30))
            .expect("test backend");
        terminal
            .draw(|f| crate::ui::draw(f, &mut app))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let row = (0..30).find(|&y| {
            (0..140)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .contains("Merge side")
        });
        let y = row.expect("the match row");
        let line: String = (0..140).map(|x| buffer[(x, y)].symbol()).collect();
        let x = u16::try_from(line.find("side work").expect("on the row")).unwrap();
        let x = u16::try_from(line[..usize::from(x)].chars().count()).unwrap();
        assert_eq!(
            buffer[(x, y)].bg,
            crate::ui::DARK.find,
            "the match is highlighted"
        );
    }

    #[test]
    fn buttons_fit_the_popup_and_the_copy_areas_are_recorded() {
        let mut app = App::new(Vec::new());
        open(&mut app);
        render(&mut app, 140, 30);
        let history = app.history.as_ref().expect("open");
        assert_eq!(history.buttons.len(), history.buttons().len());
        assert!(history.copy_areas.iter().all(|area| !area.is_empty()));
        render(&mut app, 50, 30);
        let history = app.history.as_ref().expect("open");
        let fits = history.buttons.len();
        assert!(fits < history.buttons().len(), "dropped from the right");
        assert!(
            history
                .buttons
                .iter()
                .all(|(area, _)| area.right() <= history.area.right())
        );
    }

    #[test]
    fn ages_and_fitting() {
        assert_eq!(age(1000, 1000 - 300), "5m");
        assert_eq!(age(1_000_000, 1_000_000 - 5 * 3600), "5h");
        assert_eq!(age(10_000_000, 10_000_000 - 3 * 86_400), "3d");
        assert_eq!(age(10_000_000, 10_000_000 - 14 * 86_400), "2w");
        assert_eq!(age(100_000_000, 100_000_000 - 120 * 86_400), "4mo");
        assert_eq!(age(100_000_000, 100_000_000 - 800 * 86_400), "2y");
        assert_eq!(fit("short", 10), "short");
        assert_eq!(fit("a longer summary", 8), "a longe…");
    }

    #[test]
    fn stashes_lead_with_their_name_and_ask_before_dropping() {
        let mut app = App::new(Vec::new());
        let (mut history, _) = History::new(Kind::Stash);
        history.loaded(crate::history::stashes());
        app.history = Some(Box::new(history));
        let screen = render(&mut app, 120, 30);
        let has = |screen: &[String], text: &str| screen.iter().any(|row| row.contains(text));
        assert!(
            has(&screen, " Stash ") && has(&screen, "● stash@{0} commit s0"),
            "{screen:#?}"
        );
        assert!(!has(&screen, "‹stash@"), "not repeated as a name");
        assert!(has(&screen, "[a apply] [p pop] [d drop]"), "{screen:#?}");

        if let Some(history) = &mut app.history {
            history.ask = Some(Ask::Drop("s0".into()));
        }
        let screen = render(&mut app, 120, 30);
        assert!(
            has(&screen, "drop stash@{0} “commit s0”? [y drop] [esc cancel]"),
            "{screen:#?}"
        );

        let (mut empty, _) = History::new(Kind::Stash);
        empty.loaded(Vec::new());
        app.history = Some(Box::new(empty));
        assert!(has(&render(&mut app, 120, 30), "No stashes"));
    }

    #[test]
    fn the_push_form_and_a_moved_base_draw() {
        let mut app = App::new(Vec::new());
        let (mut history, _) = History::new(Kind::Stash);
        history.loaded(crate::history::stashes());
        history.head = Some("base".into());
        app.history = Some(Box::new(history));
        let has = |screen: &[String], text: &str| screen.iter().any(|row| row.contains(text));
        let screen = render(&mut app, 120, 30);
        assert!(
            has(&screen, " on base") && !has(&screen, "HEAD moved"),
            "{screen:#?}"
        );

        if let Some(history) = &mut app.history {
            history.head = Some("elsewhere".into());
            history.press(Button::Stash, None);
        }
        let screen = render(&mut app, 120, 30);
        assert!(has(&screen, "on base · ⚠ HEAD moved since"), "{screen:#?}");
        assert!(has(&screen, " message: "), "{screen:#?}");
        assert!(
            has(
                &screen,
                "[↵ stash] [esc cancel] [✓ untracked] [  keep staged]"
            ),
            "{screen:#?}"
        );
    }

    #[test]
    fn an_empty_list_or_a_commit_without_files_isnt_loading() {
        let mut app = App::new(Vec::new());
        let (mut empty, _) = History::new(Kind::Stash);
        empty.loaded(Vec::new());
        app.history = Some(Box::new(empty));
        let screen = render(&mut app, 120, 30);
        assert!(
            !screen.iter().any(|row| row.contains("loading")),
            "{screen:#?}"
        );

        let (mut history, _) = History::new(Kind::Log);
        history.loaded(vec![commit("e000000", &[])]);
        history.files_loaded("e000000", Vec::new());
        app.history = Some(Box::new(history));
        let screen = render(&mut app, 120, 30);
        assert!(
            !screen.iter().any(|row| row.contains("loading")),
            "{screen:#?}"
        );
        assert!(
            screen.iter().any(|row| row.contains("No files")),
            "{screen:#?}"
        );
    }
}

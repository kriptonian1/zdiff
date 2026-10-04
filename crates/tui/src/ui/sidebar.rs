use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, Paragraph, Widget};

use super::{Colors, counts, popup, right_aligned};
use crate::app::{App, Tick};
use crate::tree::Node;

pub(super) fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    app.sidebar = area;
    let c = app.shown_theme().colors();
    if area.is_empty() {
        return;
    }
    // The drag handle is the column next to the diff pane.
    let [rows, handle] = if app.sidebar_right {
        let [handle, rows] =
            Layout::horizontal([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        [rows, handle]
    } else {
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(1)]).areas(area)
    };
    // Painted over the whole area so the handle column shares the background.
    frame.render_widget(Block::new().bg(c.sidebar), area);
    let block = Block::new();
    (app.stage_button, app.commit_button, app.commit_box) = Default::default();
    if app.tree.is_empty() {
        frame.render_widget(Paragraph::new(" No changes").fg(c.dim).block(block), rows);
    } else {
        let staging = app.can_stage();
        // Short sidebars keep their rows for files and drop the message box.
        let message = if staging && rows.height >= COMMIT_MIN_HEIGHT {
            COMMIT_BOX_HEIGHT
        } else {
            0
        };
        let [rows, message, bar] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(message),
            Constraint::Length(u16::from(staging)),
        ])
        .areas(rows);
        draw_rows(frame, app, block, rows);
        if !message.is_empty() {
            commit_box(frame, app, message);
        }
        if staging {
            git_bar(frame, app, bar);
        }
    }
    divider(frame.buffer_mut(), c, handle, app.resizing);
}

fn draw_rows(frame: &mut Frame, app: &mut App, block: Block, area: Rect) {
    app.sidebar_rows = area;
    let (selected, c) = (app.list.selected(), app.shown_theme().colors());
    let staging = app.can_stage();
    // ponytail: builds every visible row per draw; render only the on-screen slice at 10k+ files.
    let rows: Vec<ListItem> = app
        .tree
        .visible()
        .enumerate()
        .map(|(i, node)| {
            let open = app.tree.is_open(i) != Some(false);
            let check = staging.then(|| (app.row_tick(i), app.row_pending(i)));
            // `node_row` leaves its last column empty; that column is now the handle.
            ListItem::new(node_row(
                c,
                node,
                (open, check),
                selected == Some(i),
                area.width + 1,
            ))
        })
        .collect();
    let list = List::new(rows)
        .block(block)
        .highlight_style(Style::new().bg(c.selected));
    frame.render_stateful_widget(list, area, &mut app.list);
}

/// ` 3 staged   [ Stage 2 ]`, short enough for the narrowest sidebar; records the button.
/// The commit message box: its last lines, a placeholder when empty, the cursor while writing.
fn commit_box(frame: &mut Frame, app: &mut App, area: Rect) {
    let c = app.shown_theme().colors();
    app.commit_box = area;
    let mut block = popup(c).title(" Commit message ");
    if app.writing {
        block = block.border_style(Style::new().fg(c.accent));
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    // A space keeps the text off the border, like the other fields' badges.
    let [_, text] = Layout::horizontal([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
    let focused = app.writing && app.palette.is_none() && app.find.is_none();
    app.commit_msg.draw(frame, text, c, focused);
}

/// ` 3 staged  [Stage 2] [Commit]`: buttons from the right, the count only when it fits.
fn git_bar(frame: &mut Frame, app: &mut App, area: Rect) {
    let c = app.shown_theme().colors();
    let staged = (app.tree.files())
        .filter(|(_, f)| f.staged != zdiff_core::Staged::No)
        .count();
    let pending = app.marks.len();
    let lit = |on: bool, text: String| {
        let span = Span::from(text);
        if on {
            span.fg(c.accent).bold()
        } else {
            span.fg(c.dim)
        }
    };
    let can_commit = !app.commit_msg.is_blank() && staged + pending > 0;
    let stage = lit(pending > 0, format!("[Stage {pending}]"));
    let commit = lit(can_commit, "[Commit]".into());
    let width = |span: &Span| u16::try_from(span.width()).unwrap_or(u16::MAX);
    // The last column stays free, like every other right-aligned row.
    let commit_x = area.right().saturating_sub(width(&commit) + 1);
    let stage_x = commit_x.saturating_sub(width(&stage) + 1);
    let at = |x, span: &Span| {
        Rect {
            x,
            width: width(span),
            ..area
        }
        .intersection(area)
    };
    (app.stage_button, app.commit_button) = (at(stage_x, &stage), at(commit_x, &commit));
    let status = format!(" {staged} staged");
    let buf = frame.buffer_mut();
    if area.x + u16::try_from(status.len()).unwrap_or(u16::MAX) < stage_x {
        Line::from(status.fg(c.dim)).render(area, buf);
    }
    Line::from(stage).render(app.stage_button, buf);
    Line::from(commit).render(app.commit_button, buf);
}

/// Sidebar rows below which the commit message box is hidden.
const COMMIT_MIN_HEIGHT: u16 = 16;
/// Border, two text rows, border.
const COMMIT_BOX_HEIGHT: u16 = 4;

/// The drag handle: a `│` down the 1-column `area`, lit while dragging.
fn divider(buf: &mut Buffer, c: &Colors, area: Rect, active: bool) {
    let color = if active { c.accent } else { c.dim };
    for y in area.top()..area.bottom() {
        buf[(area.x, y)].set_symbol("│").set_fg(color);
    }
}

/// `▌   ☑ M name.rs   +31 -7`: selection bar, indent, checkbox, body, counts right-aligned
/// within `width`; the checkbox is lit while it differs from the index.
fn node_row<'a>(
    c: &Colors,
    node: &'a Node,
    (open, check): (bool, Option<(Tick, bool)>),
    selected: bool,
    width: u16,
) -> Line<'a> {
    let mut spans = vec![
        if selected {
            "▌".fg(c.accent)
        } else {
            " ".into()
        },
        " ".into(),
        "  ".repeat(node.depth()).into(),
    ];
    if let Some((tick, pending)) = check {
        let (mark, color) = match tick {
            Tick::On => ("☑ ", c.green),
            Tick::Part => ("◐ ", c.yellow),
            Tick::Off => ("☐ ", c.dim),
        };
        spans.push(mark.fg(if pending { c.accent } else { color }));
    }
    let counts = match node {
        Node::Dir {
            label,
            added,
            removed,
            ..
        } => {
            spans.push(format!("{} {label}", if open { '▾' } else { '▸' }).fg(c.dim));
            // Open folders show their files' counts instead.
            if open {
                Vec::new()
            } else {
                counts(c, *added, *removed)
            }
        }
        Node::File { entry, .. } => {
            let name = entry
                .path
                .file_name()
                .map_or_else(|| entry.path.to_string_lossy(), |n| n.to_string_lossy());
            spans.push(entry.status.as_str().fg(c.status(entry.status)));
            spans.push(" ".into());
            spans.push(Span::raw(name));
            if let Some(from) = &entry.from {
                spans.push(format!(" ← {}", from.display()).fg(c.dim));
            }
            counts(c, entry.added, entry.removed)
        }
    };
    right_aligned(spans, counts, width)
}

#[cfg(test)]
mod tests {
    use super::super::{SIDEBAR_MIN, render};
    use zdiff_core::Status;

    use super::*;
    use crate::app::FileEntry;
    use crate::ui::DARK;

    #[test]
    fn sidebar_shows_header_status_and_counts() {
        let mut app = App::new(vec![
            FileEntry {
                path: "src/repo.rs".into(),
                status: Status::Modified,
                from: None,
                added: 31,
                removed: 7,
                change: 0,
                staged: zdiff_core::Staged::No,
            },
            FileEntry {
                path: "src/new.rs".into(),
                status: Status::Untracked,
                from: None,
                added: 2,
                removed: 0,
                change: 1,
                staged: zdiff_core::Staged::No,
            },
        ]);
        // Leave out the divider column.
        let text = usize::from(SIDEBAR_MIN - 1);
        let screen = render(&mut app, 80, 6);
        let (_footer, panes) = screen.split_last().expect("rows");
        assert!(panes.iter().all(|row| row.chars().nth(text) == Some('│')));
        let sidebar: Vec<String> = screen
            .iter()
            .map(|row| row.chars().take(text).collect())
            .collect();
        assert_eq!(sidebar[0].trim_end(), "  ☐ ▾ src");
        assert!(sidebar[1].starts_with("▌   ☐ ? new.rs"), "{sidebar:?}");
        assert!(sidebar[2].starts_with("    ☐ M repo.rs"), "{sidebar:?}");
        assert!(sidebar[2].trim_end().ends_with("+31 -7"), "{sidebar:?}");

        app.tree.set_open(0, false);
        let closed = render(&mut app, 80, 6);
        let folder: String = closed[0].chars().take(text).collect();
        assert!(folder.starts_with("  ☐ ▸ src"), "{folder:?}");
        assert!(
            folder.trim_end().ends_with("+33 -7"),
            "closed folder totals"
        );
    }

    #[test]
    fn divider_lights_up_while_dragging() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 5, 2));
        let handle = Rect::new(4, 0, 1, 2);
        divider(&mut buf, DARK, handle, false);
        assert_eq!((buf[(4, 1)].symbol(), buf[(4, 1)].fg), ("│", DARK.dim));
        divider(&mut buf, DARK, handle, true);
        assert_eq!(buf[(4, 0)].fg, DARK.accent);
    }

    #[test]
    fn right_sidebar_puts_the_handle_on_its_left_edge() {
        let mut app = App::new(vec![FileEntry {
            path: "a.rs".into(),
            status: Status::Modified,
            from: None,
            added: 1,
            removed: 0,
            change: 0,
            staged: zdiff_core::Staged::No,
        }]);
        app.sidebar_right = true;
        let screen = render(&mut app, 80, 5);
        let x = usize::from(app.sidebar.x);
        assert_eq!(
            x + usize::from(app.sidebar.width),
            80,
            "flush with the right edge"
        );
        let at = |row: &str, i: usize| row.chars().nth(i);
        assert_eq!(at(&screen[0], x), Some('│'));
        assert_eq!(
            at(&screen[0], x + 1),
            Some('▌'),
            "selection mark after the handle"
        );
        assert_eq!(app.diff_area.x, 0, "the diff moves to the left");
    }

    #[test]
    fn rows_show_checkboxes_and_the_stage_bar() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let entry = |path: &str, staged, change| FileEntry {
            path: path.into(),
            status: Status::Modified,
            from: None,
            added: 1,
            removed: 0,
            change,
            staged,
        };
        let mut app = App::new(vec![
            entry("f.rs", zdiff_core::Staged::Fully, 0),
            entry("n.rs", zdiff_core::Staged::No, 1),
            entry("p.rs", zdiff_core::Staged::Partly, 2),
        ]);
        // Wide enough that the bar keeps its count; the narrowest sidebar drops it.
        let mut terminal = Terminal::new(TestBackend::new(120, 8)).expect("test backend");
        let mut draw = |app: &mut App| {
            terminal.draw(|f| crate::ui::draw(f, app)).expect("draw");
            terminal.backend().buffer().clone()
        };
        let buffer = draw(&mut app);
        let row = |y: u16| -> String { (0..30).map(|x| buffer[(x, y)].symbol()).collect() };
        assert!(row(1).starts_with("▌ ☑ M f.rs"), "{:?}", row(1));
        assert!(row(2).starts_with("  ☐ M n.rs"), "{:?}", row(2));
        assert!(row(3).starts_with("  ◐ M p.rs"), "{:?}", row(3));
        let bar = row(app.stage_button.y);
        assert!(
            bar.starts_with(" 2 staged") && bar.contains("[Stage 0]"),
            "{bar:?}"
        );
        assert_eq!(
            buffer[(app.stage_button.x, app.stage_button.y)].fg,
            DARK.dim,
            "nothing pending"
        );

        app.marks.insert("n.rs".into(), true);
        let buffer = draw(&mut app);
        assert_eq!(
            (buffer[(2, 2)].symbol(), buffer[(2, 2)].fg),
            ("☑", DARK.accent),
            "pending"
        );
        assert_eq!(
            buffer[(app.stage_button.x, app.stage_button.y)].fg,
            DARK.accent
        );
    }

    #[test]
    fn commit_panel_shows_placeholder_text_and_buttons() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = App::new(vec![FileEntry {
            path: "f.rs".into(),
            status: Status::Modified,
            from: None,
            added: 1,
            removed: 0,
            change: 0,
            staged: zdiff_core::Staged::Fully,
        }]);
        let draw = |app: &mut App, height| {
            let mut terminal = Terminal::new(TestBackend::new(120, height)).expect("test backend");
            terminal.draw(|f| crate::ui::draw(f, app)).expect("draw");
            terminal.backend().buffer().clone()
        };
        let text = |buffer: &ratatui::buffer::Buffer, y| -> String {
            (0..36).map(|x| buffer[(x, y)].symbol()).collect()
        };
        let buffer = draw(&mut app, 24);
        let top = app.commit_box.y;
        assert!(
            text(&buffer, top).contains("Commit message"),
            "{:?}",
            text(&buffer, top)
        );
        assert!(text(&buffer, top + 1).contains("i to write"), "placeholder");
        let button = app.commit_button;
        assert_eq!(buffer[(button.x, button.y)].fg, DARK.dim, "no message yet");

        app.commit_msg =
            crate::input::Field::multi("first\nsecond line that is long enough to scroll xyz");
        app.writing = true;
        let buffer = draw(&mut app, 24);
        let tail = text(&buffer, top + 2);
        assert!(
            tail.contains("scroll xyz") && !tail.contains("second"),
            "the tail: {tail:?}"
        );
        assert_eq!(buffer[(button.x, button.y)].fg, DARK.accent);

        app.commit_msg = crate::input::Field::multi("one\ntwo\nthree");
        let buffer = draw(&mut app, 24);
        assert!(
            text(&buffer, top + 1).contains("two"),
            "the last lines, cursor at the end"
        );
        let up = crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Up);
        app.commit_msg.key(up);
        app.commit_msg.key(up);
        let buffer = draw(&mut app, 24);
        assert!(
            text(&buffer, top + 1).contains("one"),
            "the box follows the cursor up"
        );

        draw(&mut app, 12);
        assert!(app.commit_box.is_empty(), "short sidebars drop the box");
        assert!(!app.commit_button.is_empty(), "but keep the buttons");
    }

    #[test]
    fn staging_off_draws_plain_rows_and_no_panel() {
        let mut app = App::new(vec![FileEntry {
            path: "f.rs".into(),
            status: Status::Modified,
            from: None,
            added: 1,
            removed: 0,
            change: 0,
            staged: zdiff_core::Staged::No,
        }]);
        app.staging = false;
        let screen = render(&mut app, 120, 24);
        assert!(screen[0].starts_with("▌ M f.rs"), "{:?}", screen[0]);
        assert!(
            screen
                .iter()
                .all(|row| !row.contains('☐') && !row.contains("Commit message"))
        );
        assert!(app.stage_button.is_empty() && app.commit_button.is_empty());
        assert_eq!(
            app.sidebar_rows.height, app.sidebar.height,
            "the list gets every row"
        );
    }
}

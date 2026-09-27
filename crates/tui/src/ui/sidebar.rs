use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, Paragraph};
use zdiff_core::Status;

use super::{ACCENT, DIM, GREEN, RED, SELECTED_BG, SIDEBAR_BG, YELLOW, counts};
use crate::app::App;
use crate::tree::Node;

pub(super) fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    app.sidebar = area;
    let block = Block::new().bg(SIDEBAR_BG);
    if app.tree.is_empty() {
        frame.render_widget(Paragraph::new(" No changes").fg(DIM).block(block), area);
    } else {
        draw_rows(frame, app, block, area);
    }
    divider(frame.buffer_mut(), area, app.resizing);
}

fn draw_rows(frame: &mut Frame, app: &mut App, block: Block, area: Rect) {
    let selected = app.list.selected();
    // ponytail: builds every visible row per draw; render only the on-screen slice at 10k+ files.
    let rows: Vec<ListItem> = app
        .tree
        .visible()
        .enumerate()
        .map(|(i, node)| {
            let open = app.tree.is_open(i) != Some(false);
            ListItem::new(node_row(node, open, selected == Some(i), area.width))
        })
        .collect();
    let list = List::new(rows)
        .block(block)
        .highlight_style(Style::new().bg(SELECTED_BG));
    frame.render_stateful_widget(list, area, &mut app.list);
}

/// The drag handle in the sidebar's last column, which rows leave empty.
fn divider(buf: &mut Buffer, area: Rect, active: bool) {
    if area.is_empty() {
        return;
    }
    let color = if active { ACCENT } else { DIM };
    for y in area.top()..area.bottom() {
        buf[(area.right() - 1, y)].set_symbol("│").set_fg(color);
    }
}

/// `▌   M name.rs      +31 -7`: selection bar, indent, body, counts right-aligned within `width`.
fn node_row(node: &Node, open: bool, selected: bool, width: u16) -> Line<'_> {
    let mut spans = vec![
        if selected {
            "▌".fg(ACCENT)
        } else {
            " ".into()
        },
        " ".into(),
        "  ".repeat(node.depth()).into(),
    ];
    let counts = match node {
        Node::Dir {
            label,
            added,
            removed,
            ..
        } => {
            spans.push(format!("{} {label}", if open { '▾' } else { '▸' }).fg(DIM));
            // Open folders show their files' counts instead.
            if open {
                Vec::new()
            } else {
                counts(*added, *removed)
            }
        }
        Node::File { entry, .. } => {
            let name = entry
                .path
                .file_name()
                .map_or_else(|| entry.path.to_string_lossy(), |n| n.to_string_lossy());
            spans.push(entry.status.as_str().fg(status_color(entry.status)));
            spans.push(" ".into());
            spans.push(Span::raw(name));
            counts(entry.added, entry.removed)
        }
    };
    let used: usize = spans.iter().chain(&counts).map(Span::width).sum();
    let pad = usize::from(width).saturating_sub(used + 1).max(1);
    spans.push(" ".repeat(pad).into());
    spans.extend(counts);
    Line::from(spans)
}

fn status_color(status: Status) -> Color {
    match status {
        Status::Added | Status::Untracked => GREEN,
        Status::Modified => YELLOW,
        Status::Deleted => RED,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{SIDEBAR_MIN, render};
    use super::*;
    use crate::app::FileEntry;

    #[test]
    fn sidebar_shows_header_status_and_counts() {
        let mut app = App::new(vec![
            FileEntry {
                path: "src/repo.rs".into(),
                status: Status::Modified,
                added: 31,
                removed: 7,
                change: 0,
            },
            FileEntry {
                path: "src/new.rs".into(),
                status: Status::Untracked,
                added: 2,
                removed: 0,
                change: 1,
            },
        ]);
        // Leave out the divider column.
        let text = usize::from(SIDEBAR_MIN - 1);
        let screen = render(&mut app, 80, 5);
        assert!(screen.iter().all(|row| row.chars().nth(text) == Some('│')));
        let sidebar: Vec<String> = screen
            .iter()
            .map(|row| row.chars().take(text).collect())
            .collect();
        assert_eq!(sidebar[0].trim_end(), "  ▾ src");
        assert!(sidebar[1].starts_with("▌   ? new.rs"), "{sidebar:?}");
        assert!(sidebar[2].starts_with("    M repo.rs"), "{sidebar:?}");
        assert!(sidebar[2].trim_end().ends_with("+31 -7"), "{sidebar:?}");

        app.tree.set_open(0, false);
        let closed = render(&mut app, 80, 5);
        let folder: String = closed[0].chars().take(text).collect();
        assert!(folder.starts_with("  ▸ src"), "{folder:?}");
        assert!(
            folder.trim_end().ends_with("+33 -7"),
            "closed folder totals"
        );
    }

    #[test]
    fn divider_lights_up_while_dragging() {
        let area = Rect::new(0, 0, 5, 2);
        let mut buf = Buffer::empty(area);
        divider(&mut buf, area, false);
        assert_eq!((buf[(4, 1)].symbol(), buf[(4, 1)].fg), ("│", DIM));
        divider(&mut buf, area, true);
        assert_eq!(buf[(4, 0)].fg, ACCENT);
    }
}

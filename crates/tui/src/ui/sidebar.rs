use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, Paragraph};
use zdiff_core::Status;

use super::{ACCENT, DIM, GREEN, RED, SELECTED_BG, SIDEBAR_BG, YELLOW, counts};
use crate::app::{App, FileEntry, Item};

pub(super) fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    app.sidebar = area;
    let block = Block::new().bg(SIDEBAR_BG);
    if app.items.is_empty() {
        frame.render_widget(Paragraph::new(" No changes").fg(DIM).block(block), area);
        return;
    }
    let selected = app.list.selected();
    // ponytail: builds every row per draw; render only the visible slice at 10k+ files.
    let rows: Vec<ListItem> = app
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| match item {
            Item::Dir(dir) => ListItem::new(Line::from(format!("  {dir}")).fg(DIM)),
            Item::File(file) => ListItem::new(file_row(file, selected == Some(i), area.width)),
        })
        .collect();
    let list = List::new(rows)
        .block(block)
        .highlight_style(Style::new().bg(SELECTED_BG));
    frame.render_stateful_widget(list, area, &mut app.list);
}

/// `▌ M name.rs      +31 -7`, counts right-aligned within `width`.
fn file_row(file: &FileEntry, selected: bool, width: u16) -> Line<'_> {
    let name = file
        .path
        .file_name()
        .map_or_else(|| file.path.to_string_lossy(), |n| n.to_string_lossy());
    let mut spans = vec![
        if selected {
            "▌".fg(ACCENT)
        } else {
            " ".into()
        },
        " ".into(),
        file.status.as_str().fg(status_color(file.status)),
        " ".into(),
        Span::raw(name),
    ];
    let counts = counts(file.added, file.removed);
    let used: usize = spans.iter().chain(&counts).map(Span::width).sum();
    let pad = usize::from(width).saturating_sub(used + 1).max(1);
    spans.push(" ".repeat(pad).into());
    spans.extend(counts);
    Line::from(spans)
}

fn status_color(status: Status) -> Color {
    match status {
        Status::Added => GREEN,
        Status::Modified => YELLOW,
        Status::Deleted => RED,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{SIDEBAR_MIN, render};
    use super::*;

    #[test]
    fn sidebar_shows_header_status_and_counts() {
        let mut app = App::new(vec![FileEntry {
            path: "src/repo.rs".into(),
            status: Status::Modified,
            added: 31,
            removed: 7,
            change: 0,
        }]);
        let sidebar: Vec<String> = render(&mut app, 80, 4)
            .iter()
            .map(|row| row.chars().take(usize::from(SIDEBAR_MIN)).collect())
            .collect();
        assert_eq!(sidebar[0].trim_end(), "  src");
        assert!(sidebar[1].starts_with("▌ M repo.rs"), "{sidebar:?}");
        assert!(sidebar[1].trim_end().ends_with("+31 -7"), "{sidebar:?}");
    }
}

mod diff;
mod sidebar;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Stylize};
use ratatui::text::Span;

use crate::app::App;

const SIDEBAR_BG: Color = Color::Rgb(22, 24, 29);
const SELECTED_BG: Color = Color::Rgb(42, 46, 56);
const FOLD_BG: Color = Color::Rgb(32, 35, 42);
const FILLER_BG: Color = Color::Rgb(16, 17, 20);
const REMOVED_BG: Color = Color::Rgb(60, 25, 28);
const ADDED_BG: Color = Color::Rgb(25, 50, 32);
const DIM: Color = Color::Rgb(140, 146, 158);
const ACCENT: Color = Color::Rgb(229, 192, 123);
const GREEN: Color = Color::Rgb(152, 195, 121);
const RED: Color = Color::Rgb(224, 108, 117);
const YELLOW: Color = Color::Rgb(229, 192, 123);

/// Sidebar width bounds in columns; narrower makes file names unreadable.
const SIDEBAR_MIN: u16 = 24;
const SIDEBAR_MAX: u16 = 40;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let width = (frame.area().width * 3 / 10).clamp(SIDEBAR_MIN, SIDEBAR_MAX);
    let [sidebar, diff] =
        Layout::horizontal([Constraint::Length(width), Constraint::Fill(1)]).areas(frame.area());
    sidebar::draw(frame, app, sidebar);
    diff::draw(frame, app, diff);
}

/// `+N -M` in green and red; zero counts are left out.
fn counts(added: u32, removed: u32) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(2);
    if added > 0 {
        spans.push(format!("+{added}").fg(GREEN));
    }
    if removed > 0 {
        spans.push(format!(" -{removed}").fg(RED));
    }
    spans
}

/// Renders `app` into a `width` x `height` test terminal, one string per screen row.
#[cfg(test)]
fn render(app: &mut App, width: u16, height: u16) -> Vec<String> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Cell;

    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test backend");
    terminal.draw(|f| draw(f, app)).expect("draw");
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(usize::from(width))
        .map(|row| row.iter().map(Cell::symbol).collect())
        .collect()
}

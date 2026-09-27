mod diff;
mod sidebar;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Stylize};
use ratatui::text::Span;
use zdiff_highlight::Class;

use crate::app::App;

const SIDEBAR_BG: Color = Color::Rgb(22, 24, 29);
const SELECTED_BG: Color = Color::Rgb(42, 46, 56);
const FOLD_BG: Color = Color::Rgb(32, 35, 42);
/// Missing side of a line; GitHub's `canvas.subtle`.
const FILLER_BG: Color = Color::Rgb(0x16, 0x1b, 0x22);
/// Diagonal stripes, barely above [`FILLER_BG`] so they hint rather than distract.
const STRIPE_FG: Color = Color::Rgb(0x21, 0x26, 0x2d);
/// Columns between filler stripes; keep >= 3 or rows checker instead of slanting.
const STRIPE_GAP: u32 = 3;
/// GitHub's dark diff line colors (`rgba(248,81,73,.15)` / `rgba(46,160,67,.15)`), blended over `#0d1117`.
const REMOVED_BG: Color = Color::Rgb(0x30, 0x1b, 0x1e);
const ADDED_BG: Color = Color::Rgb(0x12, 0x26, 0x1e);
/// GitHub's changed-word colors (same hues at 40%), blended over `#0d1117`.
const REMOVED_EMPH_BG: Color = Color::Rgb(0x6b, 0x2b, 0x2b);
const ADDED_EMPH_BG: Color = Color::Rgb(0x1a, 0x4a, 0x29);
const DIM: Color = Color::Rgb(140, 146, 158);
const ACCENT: Color = Color::Rgb(229, 192, 123);
const GREEN: Color = Color::Rgb(152, 195, 121);
const RED: Color = Color::Rgb(224, 108, 117);
const YELLOW: Color = Color::Rgb(229, 192, 123);

/// Narrowest sidebar, also when dragged; narrower makes file names unreadable.
const SIDEBAR_MIN: u16 = 24;
/// Widest automatic sidebar; dragging can go up to half the terminal.
const SIDEBAR_MAX: u16 = 40;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let width = sidebar_width(frame.area().width, app.sidebar_width);
    let [sidebar, diff] =
        Layout::horizontal([Constraint::Length(width), Constraint::Fill(1)]).areas(frame.area());
    sidebar::draw(frame, app, sidebar);
    diff::draw(frame, app, diff);
}

/// Sidebar columns for a `total`-wide terminal: the dragged width, else 30%.
fn sidebar_width(total: u16, dragged: Option<u16>) -> u16 {
    let auto = (total * 3 / 10).clamp(SIDEBAR_MIN, SIDEBAR_MAX);
    dragged
        .unwrap_or(auto)
        .clamp(SIDEBAR_MIN, (total / 2).max(SIDEBAR_MIN))
}

/// GitHub's dark syntax palette (Primer `prettylights.syntax.*`).
fn class_color(class: Class) -> Color {
    match class {
        Class::Keyword => Color::Rgb(0xff, 0x7b, 0x72),
        Class::String => Color::Rgb(0xa5, 0xd6, 0xff),
        Class::Constant => Color::Rgb(0x79, 0xc0, 0xff),
        Class::Entity => Color::Rgb(0xd2, 0xa8, 0xff),
        Class::Tag => Color::Rgb(0x7e, 0xe7, 0x87),
        Class::Comment => Color::Rgb(0x91, 0x98, 0xa1),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_width_is_auto_then_clamped() {
        assert_eq!(sidebar_width(100, None), 30);
        assert_eq!(sidebar_width(80, None), SIDEBAR_MIN);
        assert_eq!(sidebar_width(200, None), SIDEBAR_MAX);
        assert_eq!(sidebar_width(100, Some(5)), SIDEBAR_MIN);
        assert_eq!(sidebar_width(100, Some(70)), 50, "half the terminal");
        assert_eq!(sidebar_width(100, Some(45)), 45);
    }
}

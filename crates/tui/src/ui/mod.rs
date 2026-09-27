mod diff;
mod find;
mod palette;
mod sidebar;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Stylize};
use ratatui::text::{Line, Span};
use zdiff_highlight::Class;

use crate::app::App;
use crate::text::{Found, Look};

const SIDEBAR_BG: Color = Color::Rgb(22, 24, 29);
const SELECTED_BG: Color = Color::Rgb(42, 46, 56);
const FOLD_BG: Color = Color::Rgb(32, 35, 42);
/// Search matches; GitHub's attention amber, blended dark.
const FIND_BG: Color = Color::Rgb(0x5c, 0x4a, 0x0f);
/// The current search match; GitHub's `attention.emphasis`.
const FIND_CURRENT_BG: Color = Color::Rgb(0x9e, 0x6a, 0x03);
/// Modal background; GitHub's overlay color.
const MODAL_BG: Color = Color::Rgb(0x16, 0x1b, 0x22);
/// Modal badges such as ` LINE `; GitHub's accent blue.
const BADGE_BG: Color = Color::Rgb(0x1f, 0x6f, 0xeb);
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
    let [panes, bottom] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
    let width = if app.sidebar_hidden {
        0
    } else {
        sidebar_width(panes.width, app.sidebar_width)
    };
    let [sidebar, diff] =
        Layout::horizontal([Constraint::Length(width), Constraint::Fill(1)]).areas(panes);
    footer(frame, app, bottom);
    sidebar::draw(frame, app, sidebar);
    diff::draw(frame, app, diff);
    find::draw(frame, app);
    palette::draw(frame, app);
}

/// ` ◧ files   ZDIFF  master · HEAD → worktree` on the left, the change totals on the right.
fn footer(frame: &mut Frame, app: &mut App, area: Rect) {
    let (icon, color) = if app.sidebar_hidden {
        ("◫", DIM)
    } else {
        ("◧", ACCENT)
    };
    let toggle = Span::from(format!(" {icon} files ")).fg(color);
    app.sidebar_toggle = Rect {
        width: u16::try_from(toggle.width()).unwrap_or(u16::MAX),
        height: 1,
        ..area
    };
    let left = vec![
        toggle,
        " ".into(),
        format!(" {} ", app.repo_name.to_uppercase())
            .fg(Color::Black)
            .bg(BADGE_BG)
            .bold(),
        format!(" {} · {}", app.branch, app.compare).fg(DIM),
    ];
    // ponytail: totals summed per draw, O(files); cache them in `refresh` at 10k+ changed files.
    let (files, added, removed) = (app.tree.files()).fold((0, 0, 0), |(n, a, r), (_, e)| {
        (n + 1, a + e.added, r + e.removed)
    });
    let mut right = vec![format!("{files} files ").fg(DIM)];
    right.extend(counts(added, removed));
    frame.render_widget(right_aligned(left, right, area.width), area);
}

/// Sidebar columns for a `total`-wide terminal: the dragged width, else 30%.
fn sidebar_width(total: u16, dragged: Option<u16>) -> u16 {
    let auto = (total * 3 / 10).clamp(SIDEBAR_MIN, SIDEBAR_MAX);
    dragged
        .unwrap_or(auto)
        .clamp(SIDEBAR_MIN, (total / 2).max(SIDEBAR_MIN))
}

/// `text` in its look: syntax color, then the search or changed-word background.
fn styled(text: String, look: Look, emph_bg: Color) -> Span<'static> {
    let span = match look.class {
        Some(class) => text.fg(class_color(class)),
        None => text.into(),
    };
    match (look.found, look.emph) {
        (Found::Current, _) => span.bg(FIND_CURRENT_BG),
        (Found::Match, _) => span.bg(FIND_BG),
        (Found::No, true) => span.bg(emph_bg),
        (Found::No, false) => span,
    }
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

/// `spans` with `counts` pushed to the right edge of `width`, leaving the last column free.
fn right_aligned<'a>(mut spans: Vec<Span<'a>>, counts: Vec<Span<'a>>, width: u16) -> Line<'a> {
    let used: usize = spans.iter().chain(&counts).map(Span::width).sum();
    let pad = usize::from(width).saturating_sub(used + 1).max(1);
    spans.push(" ".repeat(pad).into());
    spans.extend(counts);
    Line::from(spans)
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
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use zdiff_core::Status;

    use super::*;
    use crate::app::FileEntry;

    #[test]
    fn footer_shows_repo_branch_compare_and_totals() {
        let entry = |path: &str, added, removed| FileEntry {
            path: path.into(),
            status: Status::Modified,
            added,
            removed,
            change: 0,
        };
        let mut app = App::new(vec![entry("a.rs", 2, 1), entry("b/c.rs", 1, 0)]);
        app.repo_name = "zdiff".into();
        app.branch = "master".into();
        app.compare = "HEAD → worktree".into();
        let mut terminal = Terminal::new(TestBackend::new(80, 6)).expect("test backend");
        terminal.draw(|f| draw(f, &mut app)).expect("draw");
        let buffer = terminal.backend().buffer();
        let row = |y: u16| -> String { (0..80).map(|x| buffer[(x, y)].symbol()).collect() };
        let footer = row(5);
        assert!(
            footer.starts_with(" ◧ files   ZDIFF  master · HEAD → worktree"),
            "{footer:?}"
        );
        assert!(footer.trim_end().ends_with("2 files +3 -1"), "{footer:?}");
        assert_eq!(buffer[(11, 5)].bg, BADGE_BG);
        assert_eq!(app.sidebar_toggle, Rect::new(0, 5, 9, 1));
        assert!(row(0).contains('│'), "divider shown");

        app.sidebar_hidden = true;
        terminal.draw(|f| draw(f, &mut app)).expect("draw");
        let buffer = terminal.backend().buffer();
        let row = |y: u16| -> String { (0..80).map(|x| buffer[(x, y)].symbol()).collect() };
        assert!(row(5).starts_with(" ◫ files "), "{:?}", row(5));
        assert!(!row(0).contains('│'), "no divider: {:?}", row(0));
        assert_eq!(app.sidebar.width, 0);
    }

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

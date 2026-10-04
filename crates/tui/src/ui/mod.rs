mod branches;
mod diff;
mod find;
mod history;
mod menu;
mod palette;
mod sidebar;
mod theme;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Block;

use crate::app::{App, Notice};
use crate::text::{Found, Look};
pub(crate) use theme::Colors;
pub use theme::Theme;

/// The default theme's colors, for tests that check what was drawn.
#[cfg(test)]
const DARK: &Colors = Theme::GithubDark.colors();

/// Columns between filler stripes; keep >= 3 or rows checker instead of slanting.
const STRIPE_GAP: u32 = 3;
/// Narrowest sidebar, also when dragged; narrower makes file names unreadable.
const SIDEBAR_MIN: u16 = 24;
/// Widest automatic sidebar; dragging can go up to half the terminal.
const SIDEBAR_MAX: u16 = 40;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [bar, panes, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let c = app.shown_theme().colors();
    frame.render_widget(Block::new().fg(c.fg).bg(c.bg), frame.area());
    let width = if app.sidebar_hidden {
        0
    } else {
        sidebar_width(panes.width, app.sidebar_width)
    };
    let [sidebar, diff] = if app.sidebar_right {
        let [diff, sidebar] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(width)]).areas(panes);
        [sidebar, diff]
    } else {
        Layout::horizontal([Constraint::Length(width), Constraint::Fill(1)]).areas(panes)
    };
    menu::draw_bar(frame, app, bar);
    footer(frame, app, bottom);
    sidebar::draw(frame, app, sidebar);
    diff::draw(frame, app, diff);
    find::draw(frame, app);
    palette::draw(frame, app);
    branches::draw(frame, app);
    history::draw(frame, app);
    menu::draw_dropdown(frame, app);
    toast(frame, app);
}

/// The toast, bottom right above the footer, over everything else.
fn toast(frame: &mut Frame, app: &App) {
    let Some(toast) = &app.toast else {
        return;
    };
    let c = app.shown_theme().colors();
    let text = format!(" ✓ {} ", toast.text);
    let screen = frame.area();
    let width =
        (u16::try_from(Span::from(&*text).width()).unwrap_or(u16::MAX) + 2).min(screen.width);
    let area = Rect {
        x: screen.right().saturating_sub(width + 1),
        y: screen.bottom().saturating_sub(4),
        width,
        height: 3.min(screen.height),
    };
    let block = popup(c).border_style(Style::new().fg(c.green));
    let inner = block.inner(area);
    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(Line::from(text).fg(c.green).bold(), inner);
}

/// ` ◧ files   ZDIFF  master · HEAD → worktree` on the left, the change totals on the right.
fn footer(frame: &mut Frame, app: &mut App, area: Rect) {
    let c = app.shown_theme().colors();
    let (icon, color) = match (app.sidebar_hidden, app.sidebar_right) {
        (true, _) => ("◫", c.dim),
        (false, true) => ("◨", c.accent),
        (false, false) => ("◧", c.accent),
    };
    let toggle = Span::from(format!(" {icon} files ")).fg(color);
    app.sidebar_toggle = Rect {
        width: u16::try_from(toggle.width()).unwrap_or(u16::MAX),
        height: 1,
        ..area
    };
    let compare = app.viewing.as_ref().map_or(&app.compare, |v| &v.label);
    let read_only = app.read_only || app.viewing.is_some();
    let mut left = vec![
        toggle,
        " ".into(),
        format!(" {} ", app.repo_name.to_uppercase())
            .fg(c.badge_fg)
            .bg(c.badge)
            .bold(),
        format!(" {} · {compare}", app.branch).fg(c.dim),
        if read_only { " · read-only" } else { "" }.fg(c.dim),
    ];
    app.back_button = Rect::default();
    if app.viewing.is_some() {
        let used: usize = left.iter().map(Span::width).sum();
        let button = Span::from(" ✕ back to worktree ").fg(c.accent).bold();
        app.back_button = Rect {
            x: area.x + u16::try_from(used + 2).unwrap_or(u16::MAX),
            width: u16::try_from(button.width()).unwrap_or(u16::MAX),
            height: 1,
            ..area
        }
        .intersection(area);
        left.extend(["  ".into(), button]);
    }
    // ponytail: totals summed per draw, O(files); cache them in `refresh` at 10k+ changed files.
    let (files, added, removed) = (app.tree.files()).fold((0, 0, 0), |(n, a, r), (_, e)| {
        (n + 1, a + e.added, r + e.removed)
    });
    let mut right = if let Some(notice) = &app.notice {
        let (text, color) = match notice {
            Notice::Error(text) => (text, c.red),
            Notice::Done(text) => (text, c.green),
        };
        vec![format!("{text} ").fg(color)]
    } else {
        let mut right = vec![format!("{files} files ").fg(c.dim)];
        right.extend(counts(c, added, removed));
        right
    };
    // Our memory and CPU go last, and are the first thing left out when the footer is full.
    if let Some(usage) = app.usage.filter(|_| app.show_usage) {
        let color = match usage.cpu_tenths {
            901.. => c.red,
            500..=900 => c.yellow,
            _ => c.dim,
        };
        let badge = Span::from(usage.label()).fg(color);
        let used: usize = left.iter().chain(&right).map(Span::width).sum();
        if used + badge.width() + 2 <= usize::from(area.width) {
            right.push(badge);
        }
    }
    frame.render_widget(right_aligned(left, right, area.width), area);
}

/// Sidebar columns for a `total`-wide terminal: the dragged width, else 30%.
fn sidebar_width(total: u16, dragged: Option<u16>) -> u16 {
    let auto = (total * 3 / 10).clamp(SIDEBAR_MIN, SIDEBAR_MAX);
    dragged
        .unwrap_or(auto)
        .clamp(SIDEBAR_MIN, (total / 2).max(SIDEBAR_MIN))
}

/// A popup's bordered box; sets text color too, since `Clear` under it wipes the base one.
fn popup(c: &Colors) -> Block<'static> {
    Block::bordered()
        .border_style(Style::new().fg(c.dim))
        .fg(c.fg)
        .bg(c.modal)
}

/// `text` in its look: syntax color, then the search or changed-word background.
fn styled(c: &Colors, text: String, look: Look, emph_bg: Color) -> Span<'static> {
    let span = match look.class {
        Some(class) => text.fg(c.class(class)),
        None => text.into(),
    };
    match (look.found, look.emph) {
        (Found::Current, _) => span.bg(c.find_current),
        (Found::Match, _) => span.bg(c.find),
        (Found::No, true) => span.bg(emph_bg),
        (Found::No, false) => span,
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
fn counts(c: &Colors, added: u32, removed: u32) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(2);
    if added > 0 {
        spans.push(format!("+{added}").fg(c.green));
    }
    if removed > 0 {
        spans.push(format!(" -{removed}").fg(c.red));
    }
    spans
}

/// Renders `app` into a `width` x `height` test terminal, one string per screen row.
#[cfg(test)]
pub(crate) fn render(app: &mut App, width: u16, height: u16) -> Vec<String> {
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
        // The menu bar row is covered by its own test; callers see the panes and footer.
        .skip(1)
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
    fn footer_ends_with_our_usage_coloured_by_cpu_and_drops_it_first() {
        use crate::usage::Usage;
        let entry = FileEntry {
            path: "a.rs".into(),
            status: Status::Modified,
            from: None,
            added: 2,
            removed: 1,
            change: 0,
            staged: zdiff_core::Staged::No,
        };
        let mut app = App::new(vec![entry]);
        (app.repo_name, app.branch) = ("zdiff".into(), "master".into());
        let footer = |app: &mut App, width: u16| {
            let mut terminal = Terminal::new(TestBackend::new(width, 6)).expect("test backend");
            terminal.draw(|f| draw(f, app)).expect("draw");
            let buffer = terminal.backend().buffer().clone();
            let text: String = (0..width).map(|x| buffer[(x, 5)].symbol()).collect();
            let at = text.find('▮').map(|i| text[..i].chars().count());
            let color = at.map(|x| buffer[(u16::try_from(x).unwrap_or(0), 5)].fg);
            (text, color)
        };
        assert!(!footer(&mut app, 100).0.contains('▮'), "no sample yet");

        app.usage = Some(Usage {
            mb: 24,
            cpu_tenths: 4,
        });
        let (text, color) = footer(&mut app, 100);
        assert!(
            text.trim_end().ends_with("+2 -1 ▮ 24 MB · 0.4% CPU"),
            "{text:?}"
        );
        assert!(text.contains("1 files"), "counts stay: {text:?}");
        assert_eq!(color, Some(DARK.dim));
        app.usage = Some(Usage {
            mb: 24,
            cpu_tenths: 600,
        });
        assert_eq!(footer(&mut app, 100).1, Some(DARK.yellow));
        app.usage = Some(Usage {
            mb: 24,
            cpu_tenths: 950,
        });
        assert_eq!(footer(&mut app, 100).1, Some(DARK.red));

        let (narrow, _) = footer(&mut app, 50);
        assert!(
            !narrow.contains('▮') && narrow.contains("1 files"),
            "{narrow:?}"
        );
        app.show_usage = false;
        assert!(!footer(&mut app, 100).0.contains('▮'), "turned off");
    }

    #[test]
    fn footer_shows_repo_branch_compare_and_totals() {
        let entry = |path: &str, added, removed| FileEntry {
            path: path.into(),
            status: Status::Modified,
            from: None,
            added,
            removed,
            change: 0,
            staged: zdiff_core::Staged::No,
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
        assert!(!footer.contains("read-only"));
        assert_eq!(buffer[(11, 5)].bg, DARK.badge);
        assert_eq!(app.sidebar_toggle, Rect::new(0, 5, 9, 1));
        assert!(row(1).contains('│'), "divider shown");

        app.sidebar_hidden = true;
        terminal.draw(|f| draw(f, &mut app)).expect("draw");
        let buffer = terminal.backend().buffer();
        let row = |y: u16| -> String { (0..80).map(|x| buffer[(x, y)].symbol()).collect() };
        assert!(row(5).starts_with(" ◫ files "), "{:?}", row(5));
        assert!(!row(1).contains('│'), "no divider: {:?}", row(1));
        assert_eq!(app.sidebar.width, 0);

        (app.sidebar_hidden, app.sidebar_right) = (false, true);
        terminal.draw(|f| draw(f, &mut app)).expect("draw");
        let buffer = terminal.backend().buffer();
        let footer: String = (0..80).map(|x| buffer[(x, 5)].symbol()).collect();
        assert!(footer.starts_with(" ◨ files "), "{footer:?}");

        app.notice = Some(Notice::Error("settings: unknown action \"nope\"".into()));
        terminal.draw(|f| draw(f, &mut app)).expect("draw");
        let buffer = terminal.backend().buffer();
        let footer: String = (0..80).map(|x| buffer[(x, 5)].symbol()).collect();
        let x = footer
            .find("settings:")
            .expect("the notice replaces the totals");
        let x = u16::try_from(footer[..x].chars().count()).unwrap();
        assert_eq!(buffer[(x, 5)].fg, DARK.red);

        app.notice = Some(Notice::Done("committed abc1234".into()));
        terminal.draw(|f| draw(f, &mut app)).expect("draw");
        let buffer = terminal.backend().buffer();
        let footer: String = (0..80).map(|x| buffer[(x, 5)].symbol()).collect();
        let x = footer
            .find("committed")
            .expect("the notice replaces the totals");
        let x = u16::try_from(footer[..x].chars().count()).unwrap();
        assert_eq!(buffer[(x, 5)].fg, DARK.green, "a success is green");

        (app.notice, app.read_only) = (None, true);
        terminal.draw(|f| draw(f, &mut app)).expect("draw");
        let buffer = terminal.backend().buffer();
        let footer: String = (0..80).map(|x| buffer[(x, 5)].symbol()).collect();
        assert!(footer.contains("HEAD → worktree · read-only"), "{footer:?}");
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

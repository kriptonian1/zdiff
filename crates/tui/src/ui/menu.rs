//! The menu bar on the top row and the open menu's dropdown under it.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::Clear;

use super::{popup, right_aligned};
use crate::app::App;
use crate::keymap::Keymap;
use crate::menu::{Action, MENUS};

/// Columns around an item's label and shortcut: border, check mark, and gap.
const DROPDOWN_PADDING: usize = 8;

/// The key a menu shows for `action`: its first binding, or nothing.
fn shortcut(keymap: &Keymap, action: Action) -> String {
    (keymap.keys(action).next()).map_or_else(String::new, |(_, chord)| chord.to_string())
}

/// `item`'s name in a menu, saying why when it can't take effect: in a read-only run, or a
/// terminal that can't draw `images`.
fn label(item: Action, (read_only, images): (bool, bool)) -> &'static str {
    match item {
        Action::Staging if read_only => "Staging (read-only)",
        Action::ImagePreviews if !images => "Image previews (not supported)",
        Action::SvgPreviewDefault if !images => "Open SVGs as preview (not supported)",
        _ => item.label(),
    }
}

/// Dropdown width for `items`: the widest label and shortcut, plus [`DROPDOWN_PADDING`].
fn dropdown_width(items: &[Action], app: &App) -> u16 {
    let widest = (items.iter())
        .map(|&item| {
            label(item, (app.read_only, app.images.is_some()))
                .chars()
                .count()
                + shortcut(&app.keymap, item).chars().count()
        })
        .max()
        .unwrap_or(0);
    u16::try_from(widest + DROPDOWN_PADDING).unwrap_or(u16::MAX)
}

pub(super) fn draw_bar(frame: &mut Frame, app: &mut App, area: Rect) {
    let (open, c) = (
        app.menu.open.map(|(menu, _)| menu),
        app.shown_theme().colors(),
    );
    let mut x = area.x;
    let mut spans = Vec::with_capacity(MENUS.len());
    for (i, ((name, _), slot)) in MENUS.iter().zip(&mut app.menu.titles).enumerate() {
        let title = Span::from(format!(" {name} "));
        let title = if open == Some(i) {
            title.fg(c.accent).bg(c.selected)
        } else {
            title.fg(c.dim)
        };
        let width = u16::try_from(title.width()).unwrap_or(u16::MAX);
        *slot = Rect { x, width, ..area }.intersection(area);
        x = x.saturating_add(width);
        spans.push(title);
    }
    frame.render_widget(Line::from(spans).bg(c.bar), area);
}

pub(super) fn draw_dropdown(frame: &mut Frame, app: &mut App) {
    let Some((menu, selected)) = app.menu.open else {
        return;
    };
    let (items, c) = (MENUS[menu].1, app.shown_theme().colors());
    let title = app.menu.titles[menu];
    let height = u16::try_from(items.len() + 2).unwrap_or(u16::MAX);
    let area = Rect::new(title.x, title.bottom(), dropdown_width(items, app), height)
        .intersection(frame.area());
    let block = popup(c);
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    let scope = app.scope();
    // Read before the loop, which borrows the menu's areas mutably.
    let can_stage = app.can_stage();
    for (i, (&item, slot)) in items.iter().zip(&mut app.menu.item_areas).enumerate() {
        *slot = Rect {
            y: inner.y + u16::try_from(i).unwrap_or(u16::MAX),
            height: 1,
            ..inner
        }
        .intersection(inner);
        if item == Action::Separator {
            // Drawn over the side borders so the line joins the box.
            let rule = format!("├{}┤", "─".repeat(usize::from(inner.width)));
            let row = Rect {
                y: slot.y,
                height: 1,
                ..area
            };
            frame.render_widget(Line::from(rule).fg(c.dim), row);
            continue;
        }
        let label = label(item, (app.read_only, app.images.is_some()));
        let key = shortcut(&app.keymap, item);
        let checked = match item {
            Action::Show(choice) => choice == app.view_choice,
            Action::Scope(item_scope) => item_scope == scope,
            Action::Sidebar => !app.sidebar_hidden,
            Action::SidebarRight => app.sidebar_right,
            Action::WordHighlights => app.word_highlights,
            Action::Syntax => app.syntax,
            Action::WatchAtStart => app.watch_default,
            Action::Staging => can_stage,
            Action::ImagePreviews => app.image_previews,
            Action::SvgPreviewDefault => app.svg_preview_default,
            _ => false,
        };
        let check = if checked { "✓" } else { " " };
        let text = Span::from(format!(" {check} {label}"));
        // Read-only runs can't turn staging on, nor plain terminals draw images: greyed.
        let off = (item == Action::Staging && app.read_only)
            || (matches!(item, Action::ImagePreviews | Action::SvgPreviewDefault)
                && app.images.is_none());
        let line = right_aligned(
            vec![if off { text.fg(c.dim) } else { text }],
            vec![Span::from(key).fg(c.dim)],
            inner.width,
        );
        let line = if i == selected {
            line.bg(c.selected)
        } else {
            line
        };
        frame.render_widget(line, *slot);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use super::*;
    use crate::app::{Scope, View};
    use crate::ui::DARK;
    use crate::ui::Theme;

    fn render(app: &mut App) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
        terminal.draw(|f| crate::ui::draw(f, app)).expect("draw");
        terminal.backend().buffer().clone()
    }

    fn row(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    #[test]
    fn bar_shows_both_menus_and_each_dropdown_lists_its_items() {
        let mut app = App::new(Vec::new());
        let buffer = render(&mut app);
        let bar = row(&buffer, 0);
        assert!(
            bar.starts_with(" File  View  Navigate  Settings "),
            "{bar:?}"
        );
        assert_eq!(
            app.menu.titles,
            [
                Rect::new(0, 0, 6, 1),
                Rect::new(6, 0, 6, 1),
                Rect::new(12, 0, 10, 1),
                Rect::new(22, 0, 10, 1)
            ]
        );
        assert_eq!(buffer[(40, 0)].bg, DARK.bar, "the whole row");

        for (menu, (_, items)) in MENUS.iter().enumerate() {
            app.menu.open = Some((menu, 0));
            let buffer = render(&mut app);
            let real = items.iter().zip(app.menu.item_areas);
            for (item, area) in real.filter(|(item, _)| **item != Action::Separator) {
                let (label, key) = (item.label(), shortcut(&app.keymap, *item));
                let text = row(&buffer, area.y);
                assert!(text.contains(label) && text.contains(&key), "{text:?}");
            }
            let first = app.menu.item_areas[0];
            assert_eq!(buffer[(first.x + 1, first.y)].bg, DARK.selected);
        }

        app.menu.open = Some((2, 0));
        let search = MENUS[2]
            .1
            .iter()
            .position(|&i| i == Action::SearchAll)
            .unwrap();
        let buffer = render(&mut app);
        let text = row(&buffer, app.menu.item_areas[search].y);
        assert!(
            text.contains("Search all files…  Ctrl+Shift+F"),
            "fits: {text:?}"
        );

        app.menu.open = Some((1, 0));
        let line = |app: &mut App, item: Action| {
            let i = MENUS[1]
                .1
                .iter()
                .position(|&it| it == item)
                .expect("a View item");
            let buffer = render(app);
            row(&buffer, app.menu.item_areas[i].y)
        };
        assert!(app.menu.item_areas[0].x >= 6, "under the View title");
        assert!(line(&mut app, Action::Scope(Scope::File)).contains("✓ Single file"));
        assert!(line(&mut app, Action::Scope(Scope::All)).contains("  All files"));
        assert!(line(&mut app, Action::Sidebar).contains("✓ Sidebar"));
        assert!(line(&mut app, Action::SidebarRight).contains("  Sidebar on right"));
        assert!(line(&mut app, Action::Show(Some(View::Split))).contains("  Split view"));
        assert!(
            line(&mut app, Action::Show(None)).contains("✓ Auto"),
            "Auto is the default"
        );
        let separators = (MENUS[1].1.iter().zip(app.menu.item_areas))
            .filter(|(item, _)| **item == Action::Separator)
            .map(|(_, area)| row(&render(&mut app), area.y));
        for rule in separators.collect::<Vec<_>>() {
            let rule = rule.trim();
            assert!(rule.starts_with('├') && rule.ends_with('┤'), "{rule:?}");
        }
        assert!(line(&mut app, Action::WordHighlights).contains("✓ Word highlights"));
        app.word_highlights = false;
        assert!(line(&mut app, Action::WordHighlights).contains("  Word highlights"));
        app.sidebar_hidden = true;
        assert!(line(&mut app, Action::Sidebar).contains("  Sidebar"));

        assert!(line(&mut app, Action::Theme).contains("  Theme…"));
    }
    #[test]
    fn light_theme_popups_use_its_text_color() {
        let mut app = App::new(Vec::new());
        app.theme = Theme::GithubLight;
        let fg = app.theme.colors().fg;
        app.menu.open = Some((2, 0));
        let buffer = render(&mut app);
        let item = app.menu.item_areas[0];
        assert_eq!(buffer[(item.x + 3, item.y)].fg, fg, "dropdown label");

        app.menu.open = None;
        app.palette = Some(crate::palette::Palette::line());
        app.palette.as_mut().expect("just opened").field = crate::input::Field::single("12");
        let buffer = render(&mut app);
        let inner = app.palette_area.inner(ratatui::layout::Margin::new(1, 1));
        // After the ` LINE ` badge and ` :`.
        assert_eq!(buffer[(inner.x + 8, inner.y)].symbol(), "1");
        assert_eq!(buffer[(inner.x + 8, inner.y)].fg, fg, "palette input");
    }

    #[test]
    fn view_menu_checks_staging_and_greys_it_out_when_read_only() {
        let mut app = App::new(Vec::new());
        let view = MENUS
            .iter()
            .position(|(name, _)| *name == "View")
            .expect("the View menu");
        let line = |app: &mut App, item: Action| {
            app.menu.open = Some((view, 0));
            let i = (MENUS[view].1.iter().position(|&it| it == item)).expect("a View item");
            let buffer = render(app);
            let area = app.menu.item_areas[i];
            (row(&buffer, area.y), buffer[(area.x + 3, area.y)].fg)
        };
        let (text, fg) = line(&mut app, Action::Staging);
        assert!(text.contains("✓ Staging"), "{text:?}");
        assert_ne!(fg, DARK.dim);

        app.read_only = true;
        let (text, fg) = line(&mut app, Action::Staging);
        assert!(text.contains("  Staging (read-only)"), "{text:?}");
        assert_eq!(fg, DARK.dim);
    }

    #[test]
    fn image_previews_is_greyed_where_the_terminal_cant_draw_images() {
        let mut app = App::new(Vec::new());
        let view = (MENUS.iter().position(|(name, _)| *name == "View")).expect("the View menu");
        let i = (MENUS[view]
            .1
            .iter()
            .position(|&it| it == Action::ImagePreviews))
        .expect("a View item");
        let line = |app: &mut App| {
            app.menu.open = Some((view, 0));
            let buffer = render(app);
            let area = app.menu.item_areas[i];
            (row(&buffer, area.y), buffer[(area.x + 3, area.y)].fg)
        };
        let (text, fg) = line(&mut app);
        assert!(text.contains("Image previews (not supported)"), "{text:?}");
        assert_eq!(fg, DARK.dim);

        app.images = Some(ratatui_image::picker::Picker::halfblocks());
        let (text, fg) = line(&mut app);
        assert!(
            text.contains("✓ Image previews") && !text.contains("not"),
            "{text:?}"
        );
        assert_ne!(fg, DARK.dim);
    }
}

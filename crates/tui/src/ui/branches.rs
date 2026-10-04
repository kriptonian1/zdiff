//! The branches popup: branches and tags on the left, the selected one's graph against HEAD on
//! the right.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;
use zdiff_core::{Branch, COUNT_LIMIT, RefKind};

use super::history::{
    NARROW_WIDTH, age, commit_line, fit, now, popup_area, rule, search_row, visible_from,
};
use super::{Colors, popup, right_aligned};
use crate::app::App;
use crate::branches::{Ask, Branches, Line as Row};

/// Columns of name kept before a branch's age is dropped, then its counts.
const NAME_KEEP: usize = 12;

pub(super) fn draw(frame: &mut Frame, app: &mut App) {
    let c = app.shown_theme().colors();
    let Some(branches) = &mut app.branches else {
        return;
    };
    let (area, wide) = popup_area(frame.area());
    branches.area = area;
    let block = popup(c).title(" Branches ");
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    let [main, about_rule, about, hint_rule, hint] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(rule(c, "─", inner.width), about_rule);
    frame.render_widget(rule(c, "─", inner.width), hint_rule);
    let now = now();
    let summary = branches.selected_branch().map(|b| about_line(c, b, now));
    frame.render_widget(summary.unwrap_or_default(), about);
    if branches.ask.is_some() {
        draw_ask(frame, c, branches, hint);
    } else {
        let remotes = if branches.remotes { "on" } else { "off" };
        let keys = format!(
            " ↵ history  c checkout  n new  d delete  / filter  r remotes ({remotes})  y copy  esc close"
        );
        frame.render_widget(
            Line::from(fit(&keys, usize::from(hint.width))).fg(c.dim),
            hint,
        );
    }

    let (list, graph) = if wide {
        let left = (main.width * 2 / 5).max(NARROW_WIDTH).min(main.width);
        let [list, line, graph] = Layout::horizontal([
            Constraint::Length(left),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(main);
        let bar = vec![Line::from("│").fg(c.dim); usize::from(line.height)];
        frame.render_widget(Paragraph::new(bar), line);
        (list, graph)
    } else {
        let [list, line, graph] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(main.height * 2 / 5),
        ])
        .areas(main);
        frame.render_widget(rule(c, "═", main.width), line);
        (list, graph)
    };
    draw_list(frame, c, branches, list, now);
    draw_graph(frame, c, branches, graph, now);
}

/// The question in place of the hint line, with its confirm and cancel buttons, which stay
/// whole on a narrow popup while the question is cut.
fn draw_ask(frame: &mut Frame, c: &Colors, branches: &mut Branches, area: Rect) {
    let from = branches
        .selected_branch()
        .map(|b| b.name.clone())
        .unwrap_or_default();
    let (prompt, color, confirm) = match &branches.ask {
        Some(Ask::New { .. }) => (format!(" new branch from {from}: "), c.accent, "[↵ create]"),
        Some(Ask::Delete { name, force: false }) => {
            (format!(" delete {name}? "), c.red, "[y delete]")
        }
        Some(Ask::Delete { name, force: true }) => (
            format!(" {name} isn't merged; delete anyway? "),
            c.red,
            "[y delete]",
        ),
        None => return,
    };
    let cancel = "[esc cancel]";
    let width = |text: &str| u16::try_from(text.width()).unwrap_or(u16::MAX);
    let buttons = width(confirm) + 1 + width(cancel) + 1;
    let field = if branches.ask_field().is_some() {
        area.width / 3
    } else {
        0
    };
    let room = area.width.saturating_sub(buttons + field);
    let prompt = fit(&prompt, usize::from(room));
    let [label, input, ok, _, no] = Layout::horizontal([
        Constraint::Length(width(&prompt)),
        Constraint::Length(field),
        Constraint::Length(width(confirm)),
        Constraint::Length(1),
        Constraint::Length(width(cancel)),
    ])
    .areas(area);
    frame.render_widget(Line::from(prompt).fg(color), label);
    if let Some(name) = branches.ask_field() {
        name.draw(frame, input, c, true);
    }
    frame.render_widget(Line::from(confirm).fg(c.accent), ok);
    frame.render_widget(Line::from(cancel).fg(c.dim), no);
    branches.ask_buttons = [ok, no];
}

fn draw_list(frame: &mut Frame, c: &Colors, branches: &mut Branches, area: Rect, now: i64) {
    let area = if let Some(input) = &mut branches.search {
        let [search, rest] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        let count = format!(" {} of {} ", branches.shown.len(), branches.list.len());
        branches.clear_area = search_row(frame, c, (input, branches.typing), &count, search);
        branches.search_area = search;
        rest
    } else {
        (branches.search_area, branches.clear_area) = Default::default();
        area
    };
    branches.list_area = area;
    let empty = if !branches.loaded {
        Some(" loading…")
    } else if branches.list.is_empty() {
        Some(" No branches yet")
    } else if branches.shown.is_empty() {
        Some(" No matching branches")
    } else {
        None
    };
    if let Some(text) = empty {
        frame.render_widget(Line::from(text).fg(c.dim), area);
        return;
    }
    let lines = branches.lines();
    let at = (lines.iter())
        .position(|line| *line == Row::Branch(branches.selected))
        .unwrap_or(0);
    let height = usize::from(area.height);
    branches.scroll = visible_from(branches.scroll, at, height);
    for (y, line) in (area.y..).zip(lines.iter().skip(branches.scroll).take(height)) {
        let row = Rect {
            y,
            height: 1,
            ..area
        };
        let line = match *line {
            Row::Header(kind) => Line::from(format!(" {}", header(kind))).fg(c.dim).bold(),
            Row::Branch(i) => {
                let selected = i == branches.selected;
                let line = branch_line(c, &branches.list[i], now, area.width, selected);
                if selected { line.bg(c.selected) } else { line }
            }
        };
        frame.render_widget(line, row);
    }
}

fn header(kind: RefKind) -> &'static str {
    match kind {
        RefKind::Local => "LOCAL",
        RefKind::Remote => "REMOTE",
        RefKind::Tag => "TAGS",
    }
}

/// `▌★ master   ↑2 ↓1  1d`: the mark, the name, then how far it is from its upstream and its
/// age; the age goes first when the row is narrow, then the counts.
fn branch_line(c: &Colors, branch: &Branch, now: i64, width: u16, selected: bool) -> Line<'static> {
    let gone = branch.upstream.as_ref().is_some_and(|u| u.gone);
    let mark = if branch.head {
        "★ ".fg(c.accent)
    } else if gone {
        "○ ".fg(c.dim)
    } else if branch.kind == RefKind::Tag {
        "◆ ".fg(c.yellow)
    } else if branch.kind == RefKind::Remote {
        "● ".fg(c.dim)
    } else {
        "● ".fg(c.accent)
    };
    let bar = if selected {
        "▌".fg(c.accent)
    } else {
        " ".into()
    };
    let mut counts: Vec<Span<'static>> = Vec::new();
    match &branch.upstream {
        Some(up) if up.gone => counts.push(" gone".fg(c.dim)),
        Some(up) => {
            if up.ahead > 0 {
                counts.push(format!(" ↑{}", count(up.ahead)).fg(c.green));
            }
            if up.behind > 0 {
                counts.push(format!(" ↓{}", count(up.behind)).fg(c.red));
            }
        }
        None => {}
    }
    let age = format!("  {}", age(now, branch.time));
    let room = usize::from(width).saturating_sub(3 + 1);
    let name = branch.name.width();
    let counts_width: usize = counts.iter().map(Span::width).sum();
    let mut right = Vec::new();
    if room >= name.min(NAME_KEEP) + counts_width {
        right.extend(counts);
    }
    let right_width: usize = right.iter().map(Span::width).sum();
    if room >= name.min(NAME_KEEP) + right_width + age.len() {
        right.push(age.fg(c.dim));
    }
    let right_width: usize = right.iter().map(Span::width).sum();
    let name = fit(&branch.name, room.saturating_sub(right_width + 1));
    right_aligned(vec![bar, mark, name.into()], right, width)
}

/// `n`, or `1000+` once counting stopped.
fn count(n: usize) -> String {
    if n >= COUNT_LIMIT {
        format!("{COUNT_LIMIT}+")
    } else {
        n.to_string()
    }
}

/// `master → origin/master · 2 ahead · 1 behind · 1d ago by Sawan`, for the selection.
fn about_line(c: &Colors, branch: &Branch, now: i64) -> Line<'static> {
    let when = format!(" · {} ago by {}", age(now, branch.time), branch.author);
    let mut spans = vec![format!(" {}", branch.name).fg(c.accent)];
    match (&branch.upstream, branch.kind) {
        (Some(up), _) if up.gone => spans.push(format!(" · {} is gone", up.name).fg(c.dim)),
        (Some(up), _) => {
            spans.push(format!(" → {}", up.name).into());
            if up.ahead == 0 && up.behind == 0 {
                spans.push(" · up to date".fg(c.dim));
            }
            if up.ahead > 0 {
                spans.push(format!(" · {} ahead", count(up.ahead)).fg(c.green));
            }
            if up.behind > 0 {
                spans.push(format!(" · {} behind", count(up.behind)).fg(c.red));
            }
        }
        (None, RefKind::Local) => spans.push(" · no upstream".fg(c.dim)),
        (None, _) => spans.push(format!(" · {}", branch.summary).into()),
    }
    spans.push(when.fg(c.dim));
    Line::from(spans)
}

/// The selected branch's graph against HEAD, from `graph_scroll`.
fn draw_graph(frame: &mut Frame, c: &Colors, branches: &mut Branches, area: Rect, now: i64) {
    branches.graph_area = area;
    if branches.graph_loading() {
        frame.render_widget(Line::from(" loading…").fg(c.dim), area);
        return;
    }
    let rows = (branches.graph.iter().zip(&branches.rows)).skip(branches.graph_scroll);
    for (y, (commit, graph)) in (area.y..area.bottom()).zip(rows) {
        let line = commit_line(c, (commit, Some(graph), ""), now, area.width, false);
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

#[cfg(test)]
mod tests {
    use super::super::render;
    use crate::app::App;
    use crate::branches::{Branches, sample};
    use crate::history::commit;
    use zdiff_core::Upstream;

    fn open(app: &mut App) {
        let mut list = sample();
        list[1].upstream = Some(Upstream {
            name: "origin/master".into(),
            ahead: 2,
            behind: 1,
            gone: false,
        });
        list[0].upstream = Some(Upstream {
            name: "origin/feat".into(),
            ahead: 0,
            behind: 0,
            gone: true,
        });
        let (mut branches, _) = Branches::new();
        branches.loaded(list);
        branches.graph_loaded("master-tip", vec![commit("m000000", &[])]);
        app.branches = Some(Box::new(branches));
    }

    #[test]
    fn the_list_shows_groups_counts_and_the_selected_graph() {
        let mut app = App::new(Vec::new());
        open(&mut app);
        let screen = render(&mut app, 120, 30);
        let has = |text: &str| screen.iter().any(|row| row.contains(text));
        assert!(has(" Branches ") && has("LOCAL") && has("REMOTE") && has("TAGS"));
        assert!(has("▌★ master") && has("↑2 ↓1"), "{screen:#?}");
        assert!(has("○ feat") && has(" gone"), "{screen:#?}");
        assert!(has("◆ v0.1"), "{screen:#?}");
        assert!(has("m000000 commit m000000"), "the graph: {screen:#?}");
        assert!(
            has("master → origin/master · 2 ahead · 1 behind"),
            "{screen:#?}"
        );
        assert!(!has("loading"), "{screen:#?}");

        let narrow = render(&mut app, 60, 30);
        let list = narrow.iter().position(|row| row.contains("★ master"));
        let graph = narrow.iter().position(|row| row.contains("m000000"));
        assert!(list < graph, "the list sits over the graph: {narrow:#?}");
    }

    #[test]
    fn a_repo_without_branches_says_so() {
        let mut app = App::new(Vec::new());
        let (mut branches, _) = Branches::new();
        branches.loaded(Vec::new());
        app.branches = Some(Box::new(branches));
        let screen = render(&mut app, 120, 30);
        assert!(screen.iter().any(|row| row.contains("No branches yet")));
        assert!(
            !screen.iter().any(|row| row.contains("loading")),
            "{screen:#?}"
        );
    }

    #[test]
    fn the_prompts_draw_in_place_of_the_hint_line() {
        use crate::branches::Ask;
        use crate::input::Field;
        let mut app = App::new(Vec::new());
        open(&mut app);
        let has = |screen: &[String], text: &str| screen.iter().any(|row| row.contains(text));
        let screen = render(&mut app, 120, 30);
        assert!(has(&screen, "c checkout  n new  d delete"), "{screen:#?}");

        if let Some(branches) = &mut app.branches {
            branches.ask = Some(Ask::New {
                name: Box::new(Field::single("")),
            });
        }
        let screen = render(&mut app, 120, 30);
        assert!(has(&screen, "new branch from master:"), "{screen:#?}");
        assert!(has(&screen, "[↵ create] [esc cancel]"), "{screen:#?}");

        if let Some(branches) = &mut app.branches {
            branches.ask = Some(Ask::Delete {
                name: "feat".into(),
                force: true,
            });
        }
        let screen = render(&mut app, 120, 30);
        assert!(
            has(&screen, "feat isn't merged; delete anyway? [y delete]"),
            "{screen:#?}"
        );
        let narrow = render(&mut app, 50, 30);
        assert!(
            has(&narrow, "[y delete] [esc cancel]"),
            "buttons stay: {narrow:#?}"
        );
    }
}

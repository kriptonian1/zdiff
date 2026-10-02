//! Lanes for drawing commit history as a graph, one row per commit, like `git log --graph`.

use crate::history::Commit;

/// What one lane holds on a commit's row. A lane's index is also its color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    Empty,
    /// Another line of history passes by.
    Pass,
    /// This row's commit. A `node` past the last drawn lane is shown as the overflow mark.
    Node,
    /// This row's commit, with more than one parent.
    Merge,
    /// A line that ends here, joining the commit's lane.
    Join,
    /// A line that starts here, for a merge's other parent.
    Fork,
}

/// One commit's row: its lanes, which lane holds the commit, and how far a horizontal line
/// reaches from it to the joins and forks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    pub cells: Box<[Cell]>,
    pub node: usize,
    /// Lowest and highest lane the horizontal line touches; `(node, node)` for none.
    pub span: (usize, usize),
    /// More lanes were open than `max_lanes`; they aren't in `cells`.
    pub overflow: bool,
}

/// The graph rows for `commits`, newest first as [`crate::Repo::log`] gives them, showing at
/// most `max_lanes` lanes.
#[must_use]
pub fn layout(commits: &[Commit], max_lanes: usize) -> Vec<GraphRow> {
    // Each lane waits for the commit its line leads to.
    let mut lanes: Vec<Option<&str>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    for commit in commits {
        let id = commit.id.as_str();
        let waiting = |lanes: &[Option<&str>]| lanes.iter().position(|l| *l == Some(id));
        let node = waiting(&lanes).unwrap_or_else(|| free(&mut lanes));
        let mut cells: Vec<Cell> = (lanes.iter())
            .map(|lane| {
                if lane.is_some() {
                    Cell::Pass
                } else {
                    Cell::Empty
                }
            })
            .collect();
        cells[node] = if commit.parents.len() > 1 {
            Cell::Merge
        } else {
            Cell::Node
        };
        let mut span = (node, node);
        for (lane, waits) in lanes.iter_mut().enumerate() {
            if lane != node && *waits == Some(id) {
                cells[lane] = Cell::Join;
                *waits = None;
                span = (span.0.min(lane), span.1.max(lane));
            }
        }
        lanes[node] = commit.parents.first().map(String::as_str);
        for parent in commit.parents.iter().skip(1) {
            let parent = Some(parent.as_str());
            let lane = (lanes.iter().position(|l| *l == parent)).unwrap_or_else(|| {
                let lane = free(&mut lanes);
                lanes[lane] = parent;
                lane
            });
            if lane >= cells.len() {
                cells.resize(lane + 1, Cell::Empty);
            }
            cells[lane] = Cell::Fork;
            span = (span.0.min(lane), span.1.max(lane));
        }
        while lanes.last() == Some(&None) {
            lanes.pop();
        }
        let overflow = cells.len() > max_lanes;
        let last = max_lanes.saturating_sub(1);
        cells.truncate(max_lanes);
        rows.push(GraphRow {
            cells: cells.into(),
            node,
            span: (span.0.min(last), span.1.min(last)),
            overflow,
        });
    }
    rows
}

/// The first lane waiting for nothing, opening a new one if all are busy.
fn free(lanes: &mut Vec<Option<&str>>) -> usize {
    lanes.iter().position(Option::is_none).unwrap_or_else(|| {
        lanes.push(None);
        lanes.len() - 1
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use Cell::{Fork, Join, Merge, Node, Pass};

    fn commit(id: &str, parents: &[&str]) -> Commit {
        Commit {
            id: id.into(),
            summary: String::new(),
            message: String::new(),
            author: String::new(),
            time: 0,
            parents: parents.iter().map(|p| (*p).to_owned()).collect(),
            refs: Box::default(),
            head: false,
        }
    }

    fn cells(rows: &[GraphRow]) -> Vec<Vec<Cell>> {
        rows.iter().map(|row| row.cells.to_vec()).collect()
    }

    #[test]
    fn a_straight_line_stays_in_one_lane() {
        let rows = layout(
            &[commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])],
            4,
        );
        assert_eq!(cells(&rows), [vec![Node], vec![Node], vec![Node]]);
        assert!(rows.iter().all(|row| row.span == (0, 0) && !row.overflow));
    }

    #[test]
    fn a_merged_branch_forks_off_then_joins_back() {
        let rows = layout(
            &[
                commit("m", &["a", "b"]),
                commit("b", &["a"]),
                commit("a", &["r"]),
                commit("r", &[]),
            ],
            4,
        );
        assert_eq!(
            cells(&rows),
            [
                vec![Merge, Fork],
                vec![Pass, Node],
                vec![Node, Join],
                vec![Node]
            ]
        );
        assert_eq!(rows[0].span, (0, 1), "merge to fork");
        assert_eq!(rows[1].node, 1);
        assert_eq!(rows[2].span, (0, 1), "join back");
    }

    #[test]
    fn two_branch_tips_share_a_parent() {
        let rows = layout(
            &[commit("x", &["a"]), commit("y", &["a"]), commit("a", &[])],
            4,
        );
        assert_eq!(
            cells(&rows),
            [vec![Node], vec![Pass, Node], vec![Node, Join]]
        );
    }

    #[test]
    fn lanes_past_the_limit_are_cut_and_flagged() {
        let tips: Vec<_> = (0..6).map(|i| commit(&format!("t{i}"), &["a"])).collect();
        let rows = layout(&tips, 4);
        assert_eq!(rows[5].cells.len(), 4);
        assert!(rows[5].overflow && !rows[0].overflow);
        assert_eq!(rows[3].cells.to_vec(), [Pass, Pass, Pass, Node]);
        assert_eq!(rows[4].cells.to_vec(), [Pass, Pass, Pass, Pass]);
        assert_eq!(rows[0].cells.to_vec(), [Node]);
    }
}

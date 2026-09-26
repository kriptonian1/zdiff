//! One loaded view of the repository: the changed files and their line counts.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use zdiff_core::{Change, Error, Repo, Spec};

use crate::app::FileEntry;

/// Repo-relative paths a batch of file events touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Touched {
    /// Anything may have changed (e.g. a commit moved HEAD).
    All,
    Paths(HashSet<PathBuf>),
}

impl Touched {
    pub fn contains(&self, path: &Path) -> bool {
        match self {
            Self::All => true,
            Self::Paths(paths) => paths.contains(path),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub changes: Vec<Change>,
    /// `(added, removed)` per entry of `changes`, same order.
    stats: Box<[(u32, u32)]>,
}

impl Snapshot {
    /// Lists the changes for `spec` with line counts, diffing only files `touched` since `previous`.
    pub fn load(
        repo: &Repo,
        spec: &Spec,
        previous: Option<&Self>,
        touched: &Touched,
    ) -> Result<Self, Error> {
        let changes = repo.changes(spec)?;
        let known: HashMap<&Path, (u32, u32)> = previous
            .map(|p| {
                p.changes
                    .iter()
                    .map(|c| c.path.as_path())
                    .zip(p.stats.iter().copied())
                    .collect()
            })
            .unwrap_or_default();
        let stats = changes
            .iter()
            .map(|change| {
                cached(&known, &change.path, touched).unwrap_or_else(|| {
                    // A file that vanished mid-read shows no counts instead of failing the load.
                    repo.diff(change).map(|d| d.stats()).unwrap_or_default()
                })
            })
            .collect();
        Ok(Self { changes, stats })
    }

    pub fn entries(&self) -> Vec<FileEntry> {
        self.changes
            .iter()
            .zip(&self.stats)
            .enumerate()
            .map(|(index, (change, &(added, removed)))| FileEntry {
                path: change.path.clone(),
                status: change.status(),
                added,
                removed,
                change: index,
            })
            .collect()
    }
}

/// Counts from the previous load, if `path` was seen then and has not been touched since.
fn cached(
    known: &HashMap<&Path, (u32, u32)>,
    path: &Path,
    touched: &Touched,
) -> Option<(u32, u32)> {
    if touched.contains(path) {
        None
    } else {
        known.get(path).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reuses_counts_only_for_untouched_known_paths() {
        let known = HashMap::from([(Path::new("a.rs"), (3, 1)), (Path::new("b.rs"), (5, 0))]);
        let touched = Touched::Paths(HashSet::from([PathBuf::from("b.rs")]));

        assert_eq!(cached(&known, Path::new("a.rs"), &touched), Some((3, 1)));
        assert_eq!(cached(&known, Path::new("b.rs"), &touched), None, "touched");
        assert_eq!(
            cached(&known, Path::new("new.rs"), &touched),
            None,
            "unknown"
        );
        assert_eq!(cached(&known, Path::new("a.rs"), &Touched::All), None);
    }
}

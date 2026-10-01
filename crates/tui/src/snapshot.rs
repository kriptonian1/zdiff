//! One loaded view of the repository: the changed files and their line counts.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
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

/// Repo-relative paths to show, from `--focus`; empty keeps everything. A folder keeps
/// everything under it, and a glob such as `src/*.rs` keeps what it matches.
#[derive(Debug, Clone, Default)]
pub struct Only {
    paths: Box<[PathBuf]>,
    /// `*` stays within one folder and `**` crosses folders, as in a shell.
    globs: GlobSet,
    /// Everything asked for, as typed, for the label.
    asked: Box<[String]>,
}

impl Only {
    pub fn new(paths: Vec<PathBuf>) -> Result<Self, globset::Error> {
        // `--focus .` from the root is the whole repository.
        if paths.iter().any(|p| p.as_os_str().is_empty()) {
            return Ok(Self::default());
        }
        let mut globs = GlobSetBuilder::new();
        for path in &paths {
            let text = path.to_string_lossy();
            if is_glob(&text) {
                globs.add(GlobBuilder::new(&text).literal_separator(true).build()?);
            }
        }
        Ok(Self {
            asked: paths.iter().map(|p| p.display().to_string()).collect(),
            // Globs stay plain paths too: Next.js names files like `pages/[id].tsx`.
            paths: paths.into(),
            globs: globs.build()?,
        })
    }

    /// Whole components only, so `src/a` keeps `src/a/b.rs` but not `src/ab.rs`.
    pub fn keeps(&self, path: &Path) -> bool {
        self.asked.is_empty()
            || self.paths.iter().any(|only| path.starts_with(only))
            || self.globs.is_match(path)
    }

    /// Whether a batch of file events can change what's shown.
    pub fn touches(&self, touched: &Touched) -> bool {
        match touched {
            Touched::All => true,
            Touched::Paths(paths) => paths.iter().any(|path| self.keeps(path)),
        }
    }

    /// `src/app.rs`, or `3 paths`; `None` when everything is shown.
    pub fn label(&self) -> Option<String> {
        match &*self.asked {
            [] => None,
            [path] => Some(path.clone()),
            paths => Some(format!("{} paths", paths.len())),
        }
    }
}

/// Whether `text` has glob characters, so it may match more than one path.
pub fn is_glob(text: &str) -> bool {
    text.contains(['*', '?', '[', '{'])
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub changes: Vec<Change>,
    /// `(added, removed)` per entry of `changes`, same order.
    stats: Box<[(u32, u32)]>,
}

impl Snapshot {
    /// Lists the changes for `spec` that `only` keeps, with line counts, diffing only files
    /// `touched` since `previous`.
    pub fn load(
        repo: &Repo,
        spec: &Spec,
        previous: Option<&Self>,
        touched: &Touched,
        only: &Only,
    ) -> Result<Self, Error> {
        let mut changes = repo.changes(spec)?;
        // ponytail: status still scans the whole repo; pass `only` to gix as a pathspec if
        // huge repos get slow.
        changes.retain(|change| only.keeps(&change.path));
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
                staged: change.staged(),
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

    #[test]
    fn only_keeps_files_and_folders_by_whole_components() {
        let only = Only::new(vec!["src/a".into(), "README.md".into()]).expect("valid");
        assert!(only.keeps(Path::new("README.md")));
        assert!(
            only.keeps(Path::new("src/a/b.rs")),
            "a folder keeps what's inside"
        );
        assert!(
            !only.keeps(Path::new("src/ab.rs")),
            "not a shared name prefix"
        );
        assert!(!only.keeps(Path::new("docs/README.md")));
        assert!(Only::default().keeps(Path::new("any.rs")));
        assert!(
            Only::new(vec!["a.rs".into(), PathBuf::new()])
                .expect("valid")
                .keeps(Path::new("any.rs")),
            "`.` is everything"
        );
    }

    #[test]
    fn a_focused_load_lists_and_counts_only_kept_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .current_dir(dir.path())
                .status()
                .expect("git runs");
            assert!(ok.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(dir.path().join(name), "one\n").unwrap();
        }
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "one"]);
        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "two\n").unwrap();
        let repo = Repo::discover(dir.path()).expect("a repo");
        let load = |only: Vec<PathBuf>| {
            let snapshot = Snapshot::load(
                &repo,
                &Spec::default(),
                None,
                &Touched::All,
                &Only::new(only).expect("valid"),
            );
            snapshot.expect("loads").entries()
        };
        let entries = load(vec!["a.txt".into()]);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            (entries[0].path.as_path(), entries[0].added),
            (Path::new("a.txt"), 1)
        );
        assert!(load(vec!["c.txt".into()]).is_empty(), "unchanged");
        assert_eq!(load(Vec::new()).len(), 2);
    }

    #[test]
    fn globs_match_like_a_shell_and_bracketed_names_still_match_as_typed() {
        let only = Only::new(vec!["src/*.rs".into(), "pages/[id].tsx".into()]).expect("valid");
        assert!(only.keeps(Path::new("src/a.rs")));
        assert!(
            !only.keeps(Path::new("src/ui/b.rs")),
            "`*` stays in one folder"
        );
        assert!(!only.keeps(Path::new("src/a.toml")));
        assert!(only.keeps(Path::new("pages/[id].tsx")), "the file itself");
        assert!(
            only.keeps(Path::new("pages/i.tsx")),
            "and what the glob matches"
        );
        assert_eq!(only.label().as_deref(), Some("2 paths"));
        assert!(is_glob("src/*.rs") && is_glob("a/{b,c}") && !is_glob("src/app.rs"));
    }

    #[test]
    fn only_skips_batches_outside_it_and_labels_itself() {
        let only = Only::new(vec!["a.rs".into()]).expect("valid");
        let paths = |p: &[&str]| Touched::Paths(p.iter().map(PathBuf::from).collect());
        assert!(only.touches(&Touched::All));
        assert!(only.touches(&paths(&["b.rs", "a.rs"])));
        assert!(!only.touches(&paths(&["b.rs"])));

        assert_eq!(only.label().as_deref(), Some("a.rs"));
        let two = Only::new(vec!["a.rs".into(), "b".into()]).expect("valid");
        assert_eq!(two.label().as_deref(), Some("2 paths"));
        assert_eq!(Only::default().label(), None);
    }
}

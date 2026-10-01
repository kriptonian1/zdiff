//! Where diffs come from: the repository, or a patch file given with `--patch`.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use zdiff_core::{Error, FileDiff, Patch, Repo, Spec, Staged};

use crate::app::FileEntry;
use crate::search::Items;
use crate::snapshot::{Only, Snapshot};

pub enum Source {
    Repo {
        repo: Repo,
        spec: Spec,
        snapshot: Snapshot,
        /// Paths shown, from `--focus`; `snapshot` holds only these.
        only: Only,
    },
    Patch {
        /// The patch file; `None` when it came from stdin.
        file: Option<PathBuf>,
        patch: Arc<Patch>,
        /// The repository around the current directory, if any, for full-context diffs.
        repo: Option<Repo>,
        /// Paths shown, from `--focus`.
        only: Only,
    },
}

impl Source {
    /// Reads and parses the patch at `path`, or stdin for `-`, showing the files `only` keeps.
    pub fn patch(path: &Path, only: Only) -> anyhow::Result<Self> {
        let file = (path != Path::new("-")).then(|| path.to_owned());
        let parsed = read_patch(file.as_deref())
            .with_context(|| format!("reading patch {}", path.display()))?;
        Ok(Self::Patch {
            file,
            patch: Arc::new(parsed),
            repo: Repo::discover(".").ok(),
            only,
        })
    }

    /// The paths `--focus` shows.
    pub fn only(&self) -> &Only {
        match self {
            Self::Repo { only, .. } | Self::Patch { only, .. } => only,
        }
    }

    /// The sidebar's files.
    pub fn entries(&self) -> Vec<FileEntry> {
        match self {
            Self::Repo { snapshot, .. } => snapshot.entries(),
            // Filtered after numbering, so `change` stays the patch file's index.
            Self::Patch { patch, only, .. } => (patch.files().iter().enumerate())
                .filter(|(_, file)| only.keeps(&file.path))
                .map(|(change, file)| {
                    let (added, removed) = file.stats();
                    FileEntry {
                        path: file.path.clone(),
                        status: file.status,
                        added,
                        removed,
                        change,
                        staged: Staged::No,
                    }
                })
                .collect(),
        }
    }

    /// The diff of file `change`, an index into [`Source::entries`].
    pub fn diff(&self, change: usize) -> Result<FileDiff, Error> {
        match self {
            Self::Repo { repo, snapshot, .. } => repo.diff(&snapshot.changes[change]),
            Self::Patch { patch, repo, .. } => Ok(patch.full_diff(change, repo.as_ref())),
        }
    }

    /// The files at `indices`, for a global search job.
    pub fn search_items(&self, indices: Vec<usize>) -> Items {
        match self {
            Self::Repo { snapshot, .. } => Items::Changes(
                (indices.into_iter())
                    .filter_map(|i| Some((i, snapshot.changes.get(i)?.clone())))
                    .collect(),
            ),
            Self::Patch { patch, .. } => Items::Patch(Arc::clone(patch), indices),
        }
    }

    /// The repository's root, which the search worker opens its own repo from.
    pub fn workdir(&self) -> Option<&Path> {
        match self {
            Self::Repo { repo, .. } => Some(repo.workdir()),
            Self::Patch { repo, .. } => repo.as_ref().map(Repo::workdir),
        }
    }

    /// Parses the patch file again; `Ok(false)` for stdin, which can't be read twice.
    pub fn reload_patch(&mut self) -> anyhow::Result<bool> {
        let Self::Patch {
            file: Some(file),
            patch,
            ..
        } = self
        else {
            return Ok(false);
        };
        *patch = Arc::new(read_patch(Some(file))?);
        Ok(true)
    }
}

fn read_patch(file: Option<&Path>) -> anyhow::Result<Patch> {
    let mut bytes = Vec::new();
    match file {
        Some(file) => bytes = std::fs::read(file)?,
        None => {
            io::stdin().read_to_end(&mut bytes)?;
        }
    }
    Ok(Patch::parse(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_lists_its_files_and_reloads_from_disk() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("x.patch");
        let one = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-a\n+b\n";
        std::fs::write(&path, one).unwrap();
        let mut source = Source::patch(&path, Only::default()).expect("parses");
        let entries = source.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!((entries[0].added, entries[0].removed), (1, 1));
        assert!(
            matches!(source, Source::Patch { .. }),
            "no repository needed"
        );
        assert_eq!(source.diff(0).expect("a diff").new.line(0), b"b");

        let two = one.replace("a.rs", "c.rs");
        std::fs::write(&path, format!("{one}{two}")).unwrap();
        assert!(source.reload_patch().expect("parses again"));
        assert_eq!(source.entries().len(), 2);
    }

    #[test]
    fn a_bad_patch_says_which_file_and_line() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "hello\n").unwrap();
        let error = Source::patch(&path, Only::default())
            .err()
            .expect("not a patch");
        let text = format!("{error:#}");
        assert!(
            text.contains("notes.txt") && text.contains("line 1: not a patch"),
            "{text}"
        );
    }

    #[test]
    fn the_sample_patch_shows_real_line_numbers_and_gaps() {
        let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example/sample.patch");
        let source = Source::patch(&sample, Only::default()).expect("the sample parses");
        let entries = source.entries();
        let config = (entries.iter().position(|e| e.path.ends_with("config.rs")))
            .expect("config.rs is in the patch");
        let mut app = crate::app::App::new(entries);
        app.view_choice = Some(crate::app::View::Unified);
        app.show(Some(source.diff(config)));
        let screen = crate::ui::render(&mut app, 120, 30);
        let has = |text: &str| screen.iter().any(|row| row.contains(text));
        assert!(has("4 lines not in the patch"), "{screen:#?}");
        assert!(has("29 lines not in the patch"), "{screen:#?}");
        assert!(has("@@ -41,7 +42,8 @@"), "real line numbers: {screen:#?}");
        assert!(has("pub retries: u8"), "{screen:#?}");
        assert!(has("(hunks only)"), "no base for it here: {screen:#?}");
    }

    #[test]
    fn focus_keeps_patch_files_and_their_indices() {
        let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example/sample.patch");
        let only = Only::new(vec!["src/config.rs".into()]).expect("valid");
        let source = Source::patch(&sample, only).expect("the sample parses");
        let entries = source.entries();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].path.ends_with("src/config.rs"));
        assert!(entries[0].change > 0, "still the patch's index");
        let diff = source.diff(entries[0].change).expect("a diff");
        assert!(diff.known.is_some(), "config.rs's hunks");
    }

    #[test]
    fn a_patch_inside_its_repo_shows_whole_files() {
        use std::process::Command;

        let dir = tempfile::tempdir().expect("temp dir");
        let git = |args: &[&str]| {
            let out = Command::new("git")
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
                .output()
                .expect("git runs");
            assert!(out.status.success(), "git {args:?}");
            out.stdout
        };
        let lines = |edit: &str| -> String {
            (1..=30)
                .map(|i| {
                    if i == 15 {
                        format!("{edit}\n")
                    } else {
                        format!("line {i}\n")
                    }
                })
                .collect()
        };
        git(&["init", "-q"]);
        std::fs::write(dir.path().join("a.txt"), lines("line 15")).unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "one"]);
        std::fs::write(dir.path().join("a.txt"), lines("fifteen")).unwrap();
        git(&["commit", "-q", "-am", "two"]);
        let patch = Patch::parse(git(&["diff", "HEAD~1", "HEAD"])).expect("parses");
        let source = Source::Patch {
            file: None,
            patch: Arc::new(patch),
            repo: Repo::discover(dir.path()).ok(),
            only: Only::default(),
        };

        let mut app = crate::app::App::new(source.entries());
        app.show(Some(source.diff(0)));
        let screen = crate::ui::render(&mut app, 120, 30);
        let has = |text: &str| screen.iter().any(|row| row.contains(text));
        assert!(has("unchanged lines"), "folds that open: {screen:#?}");
        assert!(
            !has("not in the patch") && !has("(hunks only)"),
            "{screen:#?}"
        );
    }
}

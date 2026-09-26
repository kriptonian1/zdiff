//! Repository access: which files changed for a [`Spec`], and loading both sides of one.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use gix::ObjectId;
use gix::bstr::{BStr, BString, ByteSlice};
use gix::status::{UntrackedFiles, index_worktree, tree_index::TrackRenames};

use crate::Error;
use crate::diff::FileDiff;

/// Which two sides to compare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spec {
    /// A revision against the files on disk, like `git diff <rev>`.
    Worktree(String),
    /// A revision against the index, like `git diff --cached <rev>`.
    Staged(String),
    /// The index against the files on disk, like `git diff`.
    Unstaged,
    /// Two revisions, like `git diff <a> <b>`.
    Revs(String, String),
}

impl Default for Spec {
    fn default() -> Self {
        Self::Worktree("HEAD".into())
    }
}

/// How a file differs between the two sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Added,
    Modified,
    Deleted,
    /// On disk but not in the index, like `??` in `git status`.
    Untracked,
}

impl Status {
    /// One-letter code as shown by `git status --short`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Untracked => "?",
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One changed file, as listed by [`Repo::changes`].
#[derive(Debug, Clone)]
pub struct Change {
    /// Path relative to the worktree root.
    pub path: PathBuf,
    old: Source,
    new: Source,
    untracked: bool,
}

impl Change {
    /// Whether the file was added, modified, deleted, or is untracked.
    #[must_use]
    pub fn status(&self) -> Status {
        if self.untracked {
            return Status::Untracked;
        }
        match (self.old, self.new) {
            (Source::Missing, _) => Status::Added,
            (_, Source::Missing) => Status::Deleted,
            _ => Status::Modified,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Blob(ObjectId),
    File,
    Missing,
}

enum Side<'repo> {
    Tree(gix::Tree<'repo>),
    Index(gix::worktree::Index),
    Worktree,
}

/// An opened git repository with a worktree.
#[derive(Debug)]
pub struct Repo {
    inner: gix::Repository,
    workdir: PathBuf,
}

impl Repo {
    /// Opens the repository containing `dir`, searching parent directories like git does.
    ///
    /// # Errors
    /// Returns [`Error::Discover`] if `dir` is not inside a repository, or the repository is bare.
    pub fn discover(dir: impl AsRef<Path>) -> Result<Self, Error> {
        let dir = dir.as_ref();
        let err = |source| Error::Discover {
            path: dir.to_owned(),
            source,
        };
        let inner = gix::discover(dir).map_err(|e| err(Some(e)))?;
        let workdir = inner.workdir().ok_or_else(|| err(None))?.to_owned();
        Ok(Self { inner, workdir })
    }

    /// Root directory of the worktree.
    #[must_use]
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// Lists the files that differ between the two sides of `spec`, sorted by path.
    ///
    /// Only paths and statuses are computed; use [`Repo::diff`] for contents.
    ///
    /// # Errors
    /// Returns [`Error::Git`] if a revision doesn't resolve or git status fails, and
    /// [`Error::Read`] if a worktree path can't be inspected.
    pub fn changes(&self, spec: &Spec) -> Result<Vec<Change>, Error> {
        let (old, new) = match spec {
            Spec::Worktree(rev) => (Side::Tree(self.tree(rev)?), Side::Worktree),
            Spec::Staged(rev) => (
                Side::Tree(self.tree(rev)?),
                Side::Index(self.inner.index_or_empty()?),
            ),
            Spec::Unstaged => (Side::Index(self.inner.index_or_empty()?), Side::Worktree),
            Spec::Revs(a, b) => (Side::Tree(self.tree(a)?), Side::Tree(self.tree(b)?)),
        };
        let mut paths = match (&old, &new) {
            (Side::Tree(a), Side::Tree(b)) => tree_paths(a, b)?,
            (Side::Tree(tree), new) => {
                self.status_paths(Some(tree.id), matches!(new, Side::Worktree))?
            }
            _ => self.status_paths(None, true)?,
        };
        // Parallel status is unordered; both streams may report one path.
        paths.sort_unstable();
        paths.dedup_by(|a, b| a.0 == b.0);

        let mut changes = Vec::with_capacity(paths.len());
        for (path, untracked) in paths {
            let old = self.source(&old, path.as_bstr())?;
            let new = self.source(&new, path.as_bstr())?;
            // Same blob (staged then reverted) or absent on both sides.
            // ponytail: File never equals a blob; hash worktree file to drop reverts.
            if old != new {
                changes.push(Change {
                    path: gix::path::from_bstring(path),
                    old,
                    new,
                    untracked,
                });
            }
        }
        Ok(changes)
    }

    /// Loads both sides of `change` and diffs them line by line.
    ///
    /// # Errors
    /// Returns [`Error::Read`] if the worktree file vanished or can't be read, and
    /// [`Error::Git`] if a blob can't be loaded.
    pub fn diff(&self, change: &Change) -> Result<FileDiff, Error> {
        let old = self.read(change.old, &change.path)?;
        let new = self.read(change.new, &change.path)?;
        Ok(FileDiff::new(old, new))
    }

    fn tree(&self, rev: &str) -> Result<gix::Tree<'_>, Error> {
        if rev == "HEAD" && self.inner.head()?.is_unborn() {
            return Ok(self.inner.empty_tree());
        }
        Ok(self.inner.rev_parse_single(rev)?.object()?.peel_to_tree()?)
    }

    fn status_paths(
        &self,
        head: Option<ObjectId>,
        worktree: bool,
    ) -> Result<Vec<(BString, bool)>, Error> {
        // ponytail: renames show as delete + add; add Status::Renamed when needed.
        let status = self
            .inner
            .status(gix::progress::Discard)?
            .untracked_files(if worktree {
                UntrackedFiles::Files
            } else {
                UntrackedFiles::None
            })
            .index_worktree_rewrites(None)
            .tree_index_track_renames(TrackRenames::Disabled);

        let mut paths = Vec::new();
        let Some(head) = head else {
            for item in status.into_index_worktree_iter(Vec::new())? {
                paths.extend(worktree_path(&item?));
            }
            return Ok(paths);
        };
        // ponytail: Staged still scans index-vs-worktree; gix has no switch.
        for item in status.head_tree(head).into_iter(Vec::new())? {
            match item? {
                gix::status::Item::TreeIndex(change) => {
                    paths.push((change.location().to_owned(), false));
                }
                gix::status::Item::IndexWorktree(item) if worktree => {
                    paths.extend(worktree_path(&item));
                }
                gix::status::Item::IndexWorktree(_) => {}
            }
        }
        Ok(paths)
    }

    fn source(&self, side: &Side<'_>, path: &BStr) -> Result<Source, Error> {
        Ok(match side {
            Side::Tree(tree) => match tree.lookup_entry_by_path(gix::path::from_bstr(path))? {
                Some(entry) if entry.mode().is_blob_or_symlink() => Source::Blob(entry.object_id()),
                _ => Source::Missing,
            },
            Side::Index(index) => match index.entry_by_path(path) {
                Some(entry) if entry.mode != gix::index::entry::Mode::COMMIT => {
                    Source::Blob(entry.id)
                }
                _ => Source::Missing,
            },
            Side::Worktree => {
                let full = self.workdir.join(gix::path::from_bstr(path));
                match fs::symlink_metadata(&full) {
                    // Submodule or plain directory.
                    Ok(meta) if meta.is_dir() => Source::Missing,
                    Ok(_) => Source::File,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => Source::Missing,
                    Err(source) => return Err(Error::Read { path: full, source }),
                }
            }
        })
    }

    fn read(&self, source: Source, path: &Path) -> Result<Vec<u8>, Error> {
        match source {
            Source::Missing => Ok(Vec::new()),
            Source::Blob(id) => Ok(self.inner.find_blob(id)?.take_data()),
            Source::File => {
                let full = self.workdir.join(path);
                read_worktree(&full).map_err(|source| Error::Read { path: full, source })
            }
        }
    }
}

fn read_worktree(path: &Path) -> io::Result<Vec<u8>> {
    // Git stores a symlink's target path, not the pointed-to content.
    if fs::symlink_metadata(path)?.is_symlink() {
        Ok(gix::path::into_bstr(fs::read_link(path)?)
            .into_owned()
            .into())
    } else {
        fs::read(path)
    }
}

fn tree_paths(old: &gix::Tree<'_>, new: &gix::Tree<'_>) -> Result<Vec<(BString, bool)>, Error> {
    let mut paths = Vec::new();
    old.changes()?
        .options(|o| {
            o.track_path().track_rewrites(None);
        })
        .for_each_to_obtain_tree(new, |change| {
            if !change.entry_mode().is_tree() {
                paths.push((change.location().to_owned(), false));
            }
            Ok(gix::object::tree::diff::Action::Continue(()))
        })?;
    Ok(paths)
}

/// The item's path and whether it is untracked; `None` for ignored or pruned entries.
fn worktree_path(item: &index_worktree::Item) -> Option<(BString, bool)> {
    let untracked = match item {
        index_worktree::Item::DirectoryContents { entry, .. } => {
            if entry.status != gix::dir::entry::Status::Untracked {
                return None;
            }
            true
        }
        _ => false,
    };
    Some((item.rela_path().to_owned(), untracked))
}

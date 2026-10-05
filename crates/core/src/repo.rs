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
    /// What the stash commit with this id holds, untracked files included.
    Stash(String),
}

impl Default for Spec {
    fn default() -> Self {
        Self::Worktree("HEAD".into())
    }
}

/// The two sides as `old → new`, e.g. `HEAD → worktree`.
impl fmt::Display for Spec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Worktree(rev) => write!(f, "{rev} → worktree"),
            Self::Staged(rev) => write!(f, "{rev} → index"),
            Self::Unstaged => f.write_str("index → worktree"),
            Self::Revs(old, new) => write!(f, "{old} → {new}"),
            Self::Stash(id) => write!(f, "stash {}", id.get(..7).unwrap_or(id)),
        }
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
    /// Moved from another path, with or without edits.
    Renamed,
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
            Self::Renamed => "R",
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether a file's change is in the index; only [`Spec::Worktree`] reports more than `No`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Staged {
    No,
    /// Some of the change is staged and the worktree differs again.
    Partly,
    Fully,
}

/// One changed file, as listed by [`Repo::changes`].
#[derive(Debug, Clone)]
pub struct Change {
    /// Path relative to the worktree root.
    pub path: PathBuf,
    /// Where a renamed file was before, relative to the worktree root.
    pub from: Option<PathBuf>,
    old: Source,
    new: Source,
    pub(crate) untracked: bool,
    staged: Staged,
}

impl Change {
    /// Whether the file was added, modified, deleted, or is untracked.
    #[must_use]
    pub fn status(&self) -> Status {
        if self.untracked {
            return Status::Untracked;
        }
        if self.from.is_some() {
            return Status::Renamed;
        }
        match (self.old, self.new) {
            (Source::Missing, _) => Status::Added,
            (_, Source::Missing) => Status::Deleted,
            _ => Status::Modified,
        }
    }

    #[must_use]
    pub fn staged(&self) -> Staged {
        self.staged
    }
}

/// Which status streams reported a path.
#[derive(Debug, Clone, Copy, Default)]
struct Flags {
    untracked: bool,
    /// HEAD and the index differ.
    index: bool,
    /// The index and the worktree differ.
    worktree: bool,
}

impl Flags {
    fn staged(self) -> Staged {
        match (self.index, self.worktree) {
            (true, false) => Staged::Fully,
            (true, true) => Staged::Partly,
            (false, _) => Staged::No,
        }
    }
}

/// A path one of the status streams reported, with where it was renamed from.
#[derive(Debug)]
struct Found {
    path: BString,
    from: Option<BString>,
    flags: Flags,
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
    pub(crate) inner: gix::Repository,
    pub(crate) workdir: PathBuf,
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
        let workdir = inner.workdir().ok_or_else(|| err(None))?;
        // gix keeps the workdir relative to `dir` (e.g. `../..`), which has no folder name.
        let workdir = fs::canonicalize(workdir).unwrap_or_else(|_| workdir.to_owned());
        Ok(Self { inner, workdir })
    }

    /// Root directory of the worktree.
    #[must_use]
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// The checked-out branch, the short commit id when detached, or `HEAD` if unreadable.
    #[must_use]
    pub fn head_name(&self) -> String {
        match self.inner.head_name() {
            Ok(Some(name)) => name.shorten().to_string(),
            Ok(None) => (self.inner.head_id())
                .map_or_else(|_| "HEAD".into(), |id| id.to_hex_with_len(7).to_string()),
            Err(_) => "HEAD".into(),
        }
    }

    /// The commit `a` and `b` last shared, as a full hex id: where a branch split off.
    ///
    /// # Errors
    /// Returns [`Error::Git`] for a name that isn't a commit, and [`Error::Refused`] when the
    /// two share no history.
    pub fn merge_base(&self, a: &str, b: &str) -> Result<String, Error> {
        let a = self
            .inner
            .rev_parse_single(a)?
            .object()?
            .peel_to_commit()?
            .id;
        let b = self
            .inner
            .rev_parse_single(b)?
            .object()?
            .peel_to_commit()?
            .id;
        let base =
            (self.inner.merge_base(a, b)).map_err(|_| Error::Refused("no shared history"))?;
        Ok(base.to_string())
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
            Spec::Stash(id) => return self.stash_changes(id),
            Spec::Worktree(rev) => (Side::Tree(self.tree(rev)?), Side::Worktree),
            Spec::Staged(rev) => (
                Side::Tree(self.tree(rev)?),
                Side::Index(self.inner.index_or_empty()?),
            ),
            Spec::Unstaged => (Side::Index(self.inner.index_or_empty()?), Side::Worktree),
            Spec::Revs(a, b) => (Side::Tree(self.tree(a)?), Side::Tree(self.tree(b)?)),
        };
        let mut found = match (&old, &new) {
            (Side::Tree(a), Side::Tree(b)) => tree_paths(a, b)?,
            (Side::Tree(tree), Side::Index(index)) => self.staged_paths(tree.id, index)?,
            (Side::Tree(tree), _) => self.status_paths(Some(tree.id))?,
            _ => self.status_paths(None)?,
        };
        // Parallel status is unordered; both streams may report one path, so merge their flags.
        found.sort_unstable_by(|a, b| a.path.cmp(&b.path));
        found.dedup_by(|later, kept| {
            let same = later.path == kept.path;
            if same {
                kept.from = kept.from.take().or(later.from.take());
                kept.flags.untracked |= later.flags.untracked;
                kept.flags.index |= later.flags.index;
                kept.flags.worktree |= later.flags.worktree;
            }
            same
        });
        // Staging is only offered against HEAD, where both streams ran.
        let staging = matches!(spec, Spec::Worktree(_));

        let mut changes = Vec::with_capacity(found.len());
        for Found { path, from, flags } in found {
            let old = self.source(&old, from.as_ref().unwrap_or(&path).as_bstr())?;
            let new = self.source(&new, path.as_bstr())?;
            let path = gix::path::from_bstring(path);
            // Staged, then edited back to the old side: the worktree matches, so no change.
            let reverted = match (old, new) {
                (Source::Blob(id), Source::File) if flags.index && flags.worktree => {
                    self.worktree_is(&path, id)?
                }
                (old, new) => old == new && from.is_none(),
            };
            if !reverted {
                changes.push(Change {
                    path,
                    from: from.map(gix::path::from_bstring),
                    old,
                    new,
                    untracked: flags.untracked,
                    staged: if staging { flags.staged() } else { Staged::No },
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

    /// The blob whose id starts with `hex`, if this repository has exactly one.
    #[must_use]
    pub fn blob(&self, hex: &str) -> Option<Vec<u8>> {
        let id = self.inner.rev_parse_single(hex).ok()?;
        Some(self.inner.find_blob(id).ok()?.take_data())
    }

    /// The worktree file at `path` (relative to the root), or a symlink's target.
    #[must_use]
    pub fn worktree_file(&self, path: &Path) -> Option<Vec<u8>> {
        read_worktree(&self.workdir.join(path)).ok()
    }

    pub(crate) fn tree(&self, rev: &str) -> Result<gix::Tree<'_>, Error> {
        if rev == crate::history::EMPTY_TREE || (rev == "HEAD" && self.inner.head()?.is_unborn()) {
            return Ok(self.inner.empty_tree());
        }
        Ok(self.inner.rev_parse_single(rev)?.object()?.peel_to_tree()?)
    }

    /// HEAD's tree (or none, for the index) against the worktree, both streams merged.
    fn status_paths(&self, head: Option<ObjectId>) -> Result<Vec<Found>, Error> {
        let status = (self.inner.status(gix::progress::Discard)?)
            .untracked_files(UntrackedFiles::Files)
            // Like `git status`, renames show once staged; until then they're `D` and `??`.
            .index_worktree_rewrites(None)
            .tree_index_track_renames(TrackRenames::AsConfigured);
        let mut found = Vec::new();
        let Some(head) = head else {
            for item in status.into_index_worktree_iter(Vec::new())? {
                found.extend(worktree_path(&item?));
            }
            return Ok(found);
        };
        for item in status.head_tree(head).into_iter(Vec::new())? {
            match item? {
                gix::status::Item::TreeIndex(change) => found.push(index_path(&change)),
                gix::status::Item::IndexWorktree(item) => found.extend(worktree_path(&item)),
            }
        }
        Ok(found)
    }

    /// The tree `head` against the index, without looking at the worktree.
    fn staged_paths(&self, head: ObjectId, index: &gix::index::State) -> Result<Vec<Found>, Error> {
        let mut found = Vec::new();
        self.inner.tree_index_status(
            &head,
            index,
            None,
            TrackRenames::AsConfigured,
            |change, _, _| {
                found.push(index_path(&change));
                Ok(std::ops::ControlFlow::Continue(()))
            },
        )?;
        Ok(found)
    }

    /// Whether the worktree file at `path` is stored as the blob `id`, after git's filters.
    fn worktree_is(&self, path: &Path, id: ObjectId) -> Result<bool, Error> {
        let index = self.inner.index_or_empty()?;
        let (mut pipeline, _) = self.inner.filter_pipeline(None)?;
        let bytes = self.to_git(&mut pipeline, &index, path)?;
        let hash = gix::objs::compute_hash(self.inner.object_hash(), gix::objs::Kind::Blob, &bytes);
        Ok(hash.is_ok_and(|hash| hash == id))
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

pub(crate) fn read_worktree(path: &Path) -> io::Result<Vec<u8>> {
    // Git stores a symlink's target path, not the pointed-to content.
    if fs::symlink_metadata(path)?.is_symlink() {
        Ok(gix::path::into_bstr(fs::read_link(path)?)
            .into_owned()
            .into())
    } else {
        fs::read(path)
    }
}

fn tree_paths(old: &gix::Tree<'_>, new: &gix::Tree<'_>) -> Result<Vec<Found>, Error> {
    use gix::object::tree::diff::Change;
    let mut found = Vec::new();
    old.changes()?
        .options(|o| {
            o.track_path()
                .track_rewrites(Some(gix::diff::Rewrites::default()));
        })
        .for_each_to_obtain_tree(new, |change| {
            if !change.entry_mode().is_tree() {
                let from = match change {
                    Change::Rewrite {
                        source_location,
                        copy: false,
                        ..
                    } => Some(source_location.to_owned()),
                    _ => None,
                };
                found.push(Found {
                    path: change.location().to_owned(),
                    from,
                    flags: Flags::default(),
                });
            }
            Ok(gix::object::tree::diff::Action::Continue(()))
        })?;
    Ok(found)
}

/// A HEAD-against-index change, renames included; copies count as additions.
fn index_path(change: &gix::diff::index::ChangeRef<'_, '_>) -> Found {
    let from = match change {
        gix::diff::index::ChangeRef::Rewrite {
            source_location,
            copy: false,
            ..
        } => Some(source_location.clone().into_owned()),
        _ => None,
    };
    Found {
        path: change.location().to_owned(),
        from,
        flags: Flags {
            index: true,
            ..Flags::default()
        },
    }
}

/// The item's path and whether it is untracked; `None` for ignored or pruned entries.
fn worktree_path(item: &index_worktree::Item) -> Option<Found> {
    let untracked = match item {
        index_worktree::Item::DirectoryContents { entry, .. } => {
            if entry.status != gix::dir::entry::Status::Untracked {
                return None;
            }
            true
        }
        _ => false,
    };
    let flags = Flags {
        untracked,
        worktree: true,
        index: false,
    };
    Some(Found {
        path: item.rela_path().to_owned(),
        from: None,
        flags,
    })
}

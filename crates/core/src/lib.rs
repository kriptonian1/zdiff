//! Git change listing and line diffs for zdiff.
//!
//! Open a repository with [`Repo::discover`], list what changed for a [`Spec`]
//! with [`Repo::changes`], then load one file's hunks with [`Repo::diff`].

mod diff;
mod repo;
mod rows;
mod words;

use std::path::PathBuf;

pub use diff::{FileDiff, Hunk, Text};
pub use repo::{Change, Repo, Spec, Status};
pub use rows::{Kind, Row, Side, expand, locate, reveal};
pub use words::WordChanges;

/// Errors returned by zdiff-core.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// `path` is not inside a git worktree (no repository, or a bare one).
    #[error("not a git worktree: {}", path.display())]
    Discover {
        path: PathBuf,
        source: Option<gix::Error>,
    },
    /// A worktree file could not be read.
    #[error("failed to read {}", path.display())]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Any other git failure (bad revision, corrupt object, status failure).
    #[error(transparent)]
    Git(#[from] gix::Error),
}

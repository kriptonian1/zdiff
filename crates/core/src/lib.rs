//! Git change listing and line diffs for zdiff.
//!
//! Open a repository with [`Repo::discover`], list what changed for a [`Spec`]
//! with [`Repo::changes`], then load one file's hunks with [`Repo::diff`].

mod branches;
mod diff;
mod graph;
mod history;
mod patch;
mod repo;
mod rows;
mod stash;
mod words;
mod write;

use std::path::PathBuf;

pub use branches::{Branch, COUNT_LIMIT, DETACHED, RefKind, Upstream};
pub use diff::{FileDiff, Hunk, Text};
pub use graph::{Cell, GraphRow, layout};
pub use history::Commit;
pub use patch::{Patch, PatchFile};
pub use repo::{Change, Repo, Spec, Staged, Status};
pub use rows::{Kind, Row, Side, expand, locate, reveal, unified};
pub use stash::{StashOp, StashPush};
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
    /// A patch file couldn't be read; `line` is 1-based.
    #[error("line {line}: {reason}")]
    Patch { line: usize, reason: &'static str },
    /// Staging or committing was refused; the message says why.
    #[error("{0}")]
    Refused(&'static str),
    /// A `git` command couldn't run or failed; `stderr` is its first line.
    #[error("git {cmd}: {stderr}")]
    Command { cmd: &'static str, stderr: String },
    /// Any other git failure (bad revision, corrupt object, status failure).
    #[error(transparent)]
    Git(#[from] gix::Error),
}

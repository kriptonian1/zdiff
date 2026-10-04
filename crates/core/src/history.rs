//! Recent commits, newest first, for the history popup.

use std::collections::HashMap;

use gix::ObjectId;
use gix::bstr::ByteSlice;
use gix::revision::walk::Sorting;
use gix::traverse::commit::simple::CommitTimeOrder;

use crate::Error;
use crate::repo::{Repo, Spec};

/// Git's empty tree, the old side of a root commit; [`Repo::changes`] knows it without a lookup.
pub(crate) const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// A commit's whole message and its author's name.
pub(crate) fn describe(commit: &gix::Commit<'_>) -> (String, String) {
    let message = (commit.message_raw_sloppy().to_str_lossy())
        .trim_end()
        .to_owned();
    // A malformed signature still lists the commit, without an author.
    let author = (commit.author())
        .map(|a| a.name.to_str_lossy().trim().to_owned())
        .unwrap_or_default();
    (message, author)
}

/// Branch and tag names by the commit they point to.
type Names = HashMap<ObjectId, Vec<String>>;

/// One commit, with what the history list shows for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// Full hex id; show the first 7 characters.
    pub id: String,
    pub summary: String,
    /// The whole message, summary included.
    pub message: String,
    pub author: String,
    /// Commit time in seconds since the Unix epoch.
    pub time: i64,
    /// Full hex ids, first parent first; empty for a root commit.
    pub parents: Box<[String]>,
    /// Short names of the branches (remote ones too) and tags pointing here.
    pub refs: Box<[String]>,
    /// Whether HEAD points here.
    pub head: bool,
}

impl Commit {
    /// Whether this is a stash from [`Repo::stashes`], whose first name is `stash@{n}`.
    #[must_use]
    pub fn is_stash(&self) -> bool {
        self.refs.first().is_some_and(|r| r.starts_with("stash@"))
    }
}

impl Repo {
    /// The commit HEAD points to, as full hex; `None` before the first commit.
    #[must_use]
    pub fn head_id(&self) -> Option<String> {
        self.inner.head_id().ok().map(|id| id.to_string())
    }

    /// Up to `limit` commits reachable from HEAD and the local branches, newest first.
    ///
    /// # Errors
    /// Returns [`Error::Git`] if a reference or commit can't be read.
    pub fn log(&self, limit: usize) -> Result<Vec<Commit>, Error> {
        let Ok(head) = self.inner.head_id().map(gix::Id::detach) else {
            // Unborn HEAD: nothing committed yet.
            return Ok(Vec::new());
        };
        let (_, branches) = self.ref_names()?;
        let mut tips = vec![head];
        tips.extend(branches.into_iter().filter(|id| *id != head));
        self.walk(tips, limit)
    }

    /// Up to `limit` commits reachable from `tips` (revisions such as ids or `HEAD`), newest
    /// first.
    ///
    /// # Errors
    /// Returns [`Error::Git`] if a tip doesn't resolve or a commit can't be read.
    pub fn log_from(&self, tips: &[String], limit: usize) -> Result<Vec<Commit>, Error> {
        let tips = (tips.iter())
            .map(|tip| Ok(self.inner.rev_parse_single(tip.as_str())?.detach()))
            .collect::<Result<Vec<_>, Error>>()?;
        self.walk(tips, limit)
    }

    fn walk(&self, tips: Vec<ObjectId>, limit: usize) -> Result<Vec<Commit>, Error> {
        let head = self.inner.head_id().ok().map(gix::Id::detach);
        let (refs, _) = self.ref_names()?;
        // ponytail: each page walks again from the tips, O(total) per page; keep the walk
        // between pages if 50k-commit repos feel slow.
        let walk = (self.inner.rev_walk(tips))
            .sorting(Sorting::ByCommitTime(CommitTimeOrder::NewestFirst))
            .all()?;
        let mut commits = Vec::with_capacity(limit.min(1024));
        for info in walk.take(limit) {
            let info = info?;
            let (message, author) = describe(&info.object()?);
            commits.push(Commit {
                id: info.id.to_string(),
                summary: message.lines().next().unwrap_or_default().to_owned(),
                message,
                author,
                time: info.commit_time.unwrap_or_default(),
                parents: info.parent_ids.iter().map(ToString::to_string).collect(),
                refs: refs.get(&info.id).cloned().unwrap_or_default().into(),
                head: Some(info.id) == head,
            });
        }
        Ok(commits)
    }

    /// The two sides that show `commit`: its first parent against it, or the empty tree for a
    /// root. Merges show what they brought in over their first parent, like `--first-parent`;
    /// a stash shows what it saved.
    #[must_use]
    pub fn commit_spec(&self, commit: &Commit) -> Spec {
        if commit.is_stash() {
            return Spec::Stash(commit.id.clone());
        }
        let parent = commit.parents.first().map_or(EMPTY_TREE, String::as_str);
        Spec::Revs(parent.to_owned(), commit.id.clone())
    }

    /// Local branch and tag names by the commit they point to, and the branch tips to walk.
    fn ref_names(&self) -> Result<(Names, Vec<ObjectId>), Error> {
        let mut names = Names::new();
        let mut branches = Vec::new();
        let platform = self.inner.references()?;
        // Remote branches are only names to show, never walked from.
        let refs = (platform.local_branches()?.map(|r| (r, true)))
            .chain(platform.remote_branches()?.map(|r| (r, false)))
            .chain(platform.tags()?.map(|r| (r, false)));
        for (reference, branch) in refs {
            // A broken ref shouldn't hide the history; it's left out.
            let Ok(mut reference) = reference else {
                continue;
            };
            let name = reference.name().shorten().to_string();
            if name.ends_with("/HEAD") {
                continue;
            }
            if let Ok(id) = reference.peel_to_id() {
                let id = id.detach();
                names.entry(id).or_default().push(name);
                if branch {
                    branches.push(id);
                }
            }
        }
        Ok((names, branches))
    }
}

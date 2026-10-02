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
    /// Short names of the local branches and tags pointing here.
    pub refs: Box<[String]>,
    /// Whether HEAD points here.
    pub head: bool,
}

impl Repo {
    /// Up to `limit` commits reachable from HEAD and the local branches, newest first.
    ///
    /// # Errors
    /// Returns [`Error::Git`] if a reference or commit can't be read.
    pub fn log(&self, limit: usize) -> Result<Vec<Commit>, Error> {
        let Ok(head) = self.inner.head_id().map(gix::Id::detach) else {
            // Unborn HEAD: nothing committed yet.
            return Ok(Vec::new());
        };
        let (refs, branches) = self.ref_names()?;
        let mut tips = vec![head];
        tips.extend(branches.into_iter().filter(|id| *id != head));
        // ponytail: each page walks again from the tips, O(total) per page; keep the walk
        // between pages if 50k-commit repos feel slow.
        let walk = (self.inner.rev_walk(tips))
            .sorting(Sorting::ByCommitTime(CommitTimeOrder::NewestFirst))
            .all()?;
        let mut commits = Vec::with_capacity(limit.min(1024));
        for info in walk.take(limit) {
            let info = info?;
            let commit = info.object()?;
            let message = commit
                .message_raw_sloppy()
                .to_str_lossy()
                .trim_end()
                .to_owned();
            // A malformed signature still lists the commit, without an author.
            let author = (commit.author())
                .map(|a| a.name.to_str_lossy().trim().to_owned())
                .unwrap_or_default();
            commits.push(Commit {
                id: info.id.to_string(),
                summary: message.lines().next().unwrap_or_default().to_owned(),
                message,
                author,
                time: info.commit_time.unwrap_or_default(),
                parents: info.parent_ids.iter().map(ToString::to_string).collect(),
                refs: refs.get(&info.id).cloned().unwrap_or_default().into(),
                head: info.id == head,
            });
        }
        Ok(commits)
    }

    /// The two sides that show `commit`: its first parent against it, or the empty tree for a
    /// root. Merges show what they brought in over their first parent, like `--first-parent`.
    #[must_use]
    pub fn commit_spec(&self, commit: &Commit) -> Spec {
        let parent = commit.parents.first().map_or(EMPTY_TREE, String::as_str);
        Spec::Revs(parent.to_owned(), commit.id.clone())
    }

    /// Local branch and tag names by the commit they point to, and the branch tips to walk.
    fn ref_names(&self) -> Result<(Names, Vec<ObjectId>), Error> {
        let mut names = Names::new();
        let mut branches = Vec::new();
        let platform = self.inner.references()?;
        let refs = (platform.local_branches()?.map(|r| (r, true)))
            .chain(platform.tags()?.map(|r| (r, false)));
        for (reference, branch) in refs {
            // A broken ref shouldn't hide the history; it's left out.
            let Ok(mut reference) = reference else {
                continue;
            };
            let name = reference.name().shorten().to_string();
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

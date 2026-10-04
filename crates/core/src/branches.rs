//! Branches and tags, with how far each local branch is from its upstream.

use gix::ObjectId;
use gix::remote::Direction;

use crate::Error;
use crate::history::describe;
use crate::repo::Repo;
use crate::stash::check_name;

/// The name of the row a detached HEAD gets.
pub const DETACHED: &str = "(detached)";

/// Ahead and behind counts stop here.
pub const COUNT_LIMIT: usize = 1000;

/// Where a ref lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Local,
    Remote,
    Tag,
}

/// A branch or tag and the commit it points to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// Short name: `master`, `origin/master`, `v0.1`, or `(detached)` for a detached HEAD.
    pub name: String,
    pub kind: RefKind,
    /// Full hex id of the commit; tags are peeled to theirs.
    pub tip: String,
    pub summary: String,
    pub author: String,
    /// Commit time in seconds since the Unix epoch.
    pub time: i64,
    /// HEAD is on this branch.
    pub head: bool,
    /// Only for local branches with one configured.
    pub upstream: Option<Upstream>,
}

/// The remote branch a local branch tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    /// Short name, such as `origin/master`.
    pub name: String,
    /// Commits only the local branch has, then only the upstream; both stop at
    /// [`COUNT_LIMIT`].
    pub ahead: usize,
    pub behind: usize,
    /// Configured, but its ref is gone, as after the remote branch was deleted and pruned.
    pub gone: bool,
}

impl Repo {
    /// Local branches, then remote ones (without `*/HEAD`), then tags, each sorted by name; a
    /// detached HEAD comes first as `(detached)`.
    ///
    /// # Errors
    /// Returns [`Error::Git`] if the refs or a commit can't be read.
    pub fn branches(&self) -> Result<Vec<Branch>, Error> {
        let head = self.inner.head()?;
        let head_id = self.inner.head_id().ok().map(gix::Id::detach);
        // gix keeps the config from when the repo opened; a checkout with `--track` since then
        // wrote the upstream there, so read it again.
        let mut config = self.inner.clone();
        config.reload()?;
        let on = head.referent_name().map(|name| name.as_bstr().to_string());
        let mut branches = Vec::new();
        if on.is_none()
            && let Some(id) = head_id
        {
            branches.push(self.branch(DETACHED.into(), RefKind::Local, id, true)?);
        }
        let platform = self.inner.references()?;
        let groups = [
            (RefKind::Local, platform.local_branches()?),
            (RefKind::Remote, platform.remote_branches()?),
            (RefKind::Tag, platform.tags()?),
        ];
        for (kind, refs) in groups {
            let mut group = Vec::new();
            for reference in refs {
                // A broken ref shouldn't hide the others; it's left out.
                let Ok(mut reference) = reference else {
                    continue;
                };
                let name = reference.name().shorten().to_string();
                let full = reference.name().as_bstr().to_string();
                let Ok(id) = reference.peel_to_id() else {
                    continue;
                };
                if name.ends_with("/HEAD") {
                    continue;
                }
                let head = on.as_deref() == Some(full.as_str());
                let mut branch = self.branch(name, kind, id.detach(), head)?;
                if kind == RefKind::Local {
                    branch.upstream = self.upstream(&config, reference.name(), id.detach());
                }
                group.push(branch);
            }
            group.sort_by(|a, b| a.name.cmp(&b.name));
            branches.extend(group);
        }
        Ok(branches)
    }

    /// Switches to `branch`: a local one as is, a remote one as a new tracking branch, and a
    /// tag or a detached HEAD by its commit.
    ///
    /// # Errors
    /// Returns [`Error::Command`] if git refuses, such as for local changes in the way or a
    /// local branch that already has the remote one's name.
    pub fn checkout(&self, branch: &Branch) -> Result<(), Error> {
        let args = match branch.kind {
            RefKind::Local if branch.name != DETACHED => vec!["switch", branch.name.as_str()],
            RefKind::Remote => vec!["switch", "--track", branch.name.as_str()],
            RefKind::Local | RefKind::Tag => vec!["switch", "--detach", branch.tip.as_str()],
        };
        let output = self.git("switch", &args)?;
        self.finish("switch", &output, false).map(drop)
    }

    /// A new branch `name` at the commit `from`, checked out.
    ///
    /// # Errors
    /// Returns [`Error::Refused`] for a blank name or one starting with `-`, and
    /// [`Error::Command`] if git refuses, such as for a name already taken.
    pub fn create_branch(&self, name: &str, from: &str) -> Result<(), Error> {
        let name = check_name(name)?;
        let output = self.git("switch -c", &["switch", "-c", name, from])?;
        self.finish("switch -c", &output, false).map(drop)
    }

    /// Deletes the local branch `name`, even unmerged with `force`; `false` when git kept it
    /// because it isn't merged, so the caller can ask before forcing.
    ///
    /// # Errors
    /// Returns [`Error::Refused`] for a name starting with `-`, and [`Error::Command`] if git
    /// refuses for another reason.
    pub fn delete_branch(&self, name: &str, force: bool) -> Result<bool, Error> {
        let name = check_name(name)?;
        let (cmd, flag) = if force {
            ("branch -D", "-D")
        } else {
            ("branch -d", "-d")
        };
        if !force && !self.merged(name)? {
            return Ok(false);
        }
        let output = self.git(cmd, &["branch", flag, name])?;
        self.finish(cmd, &output, false).map(|_| true)
    }

    /// Whether the local branch `name` is merged where `git branch -d` looks: into its
    /// upstream when that's set and exists, else into HEAD. A missing branch counts as merged,
    /// so git reports it.
    fn merged(&self, name: &str) -> Result<bool, Error> {
        let full = format!("refs/heads/{name}");
        let Some(mut branch) = self.inner.try_find_reference(full.as_str())? else {
            return Ok(true);
        };
        let tip = branch.peel_to_id()?.detach();
        let mut config = self.inner.clone();
        config.reload()?;
        let base = match self.tracking(&config, branch.name()) {
            Some((_, Some(id))) => id,
            _ => match self.inner.head_id() {
                Ok(id) => id.detach(),
                Err(_) => return Ok(false),
            },
        };
        // No common base means unrelated histories: not merged.
        Ok(self.inner.merge_base(tip, base).is_ok_and(|b| b == tip))
    }

    fn branch(
        &self,
        name: String,
        kind: RefKind,
        id: ObjectId,
        head: bool,
    ) -> Result<Branch, Error> {
        let commit = self.inner.find_commit(id)?;
        let (message, author) = describe(&commit);
        Ok(Branch {
            name,
            kind,
            tip: id.to_string(),
            summary: message.lines().next().unwrap_or_default().to_owned(),
            author,
            time: commit.time().map(|t| t.seconds).unwrap_or_default(),
            head,
            upstream: None,
        })
    }

    /// The upstream of the local branch `name` at `tip`, as `config` sets it; `None` when none
    /// is configured.
    fn upstream(
        &self,
        config: &gix::Repository,
        name: &gix::refs::FullNameRef,
        tip: ObjectId,
    ) -> Option<Upstream> {
        let (short, found) = self.tracking(config, name)?;
        let Some(id) = found else {
            return Some(Upstream {
                name: short,
                ahead: 0,
                behind: 0,
                gone: true,
            });
        };
        let only = |from: ObjectId, hide: ObjectId| {
            (self.inner.rev_walk([from]).with_hidden([hide]).all())
                .map_or(0, |walk| walk.take(COUNT_LIMIT).count())
        };
        Some(Upstream {
            name: short,
            ahead: only(tip, id),
            behind: only(id, tip),
            gone: false,
        })
    }

    /// The short name of the upstream `config` sets for the local branch `name`, and its tip;
    /// `None` when none is configured, and no tip when its ref is gone.
    fn tracking(
        &self,
        config: &gix::Repository,
        name: &gix::refs::FullNameRef,
    ) -> Option<(String, Option<ObjectId>)> {
        let tracking = (config.branch_remote_tracking_ref_name(name, Direction::Fetch))?.ok()?;
        let tip = (self
            .inner
            .try_find_reference(tracking.as_ref())
            .ok()
            .flatten())
        .and_then(|mut r| r.peel_to_id().ok())
        .map(gix::Id::detach);
        Some((tracking.as_ref().shorten().to_string(), tip))
    }
}

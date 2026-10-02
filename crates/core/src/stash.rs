//! The stash: listed with gix, changed by running `git stash`, since gix can't apply one.

use std::io;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use gix::ObjectId;
use gix::bstr::ByteSlice;
use gix::index::entry::Stage;

use crate::Error;
use crate::history::{Commit, EMPTY_TREE, describe};
use crate::repo::{Change, Repo, Spec};

/// What to do with one stash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StashOp {
    Apply,
    Pop,
    Drop,
}

/// What `git stash push` saves; the flags match its options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StashPush {
    /// Blank for git's `WIP on <branch>: …`.
    pub message: String,
    pub untracked: bool,
    pub keep_index: bool,
    /// Only what's staged; git refuses it with the other two.
    pub staged: bool,
}

impl StashOp {
    /// The `git stash` subcommand.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apply => "apply",
            Self::Pop => "pop",
            Self::Drop => "drop",
        }
    }
}

impl Repo {
    /// The stashes, newest first, as commits: the base is the only parent, so
    /// [`Repo::commit_spec`] shows what was stashed, and `refs` holds `stash@{n}`.
    ///
    /// # Errors
    /// Returns [`Error::Git`] if the stash reflog or one of its commits can't be read.
    pub fn stashes(&self) -> Result<Vec<Commit>, Error> {
        let Some(reference) = self.inner.try_find_reference("refs/stash")? else {
            return Ok(Vec::new());
        };
        let mut log = reference.log_iter();
        let read = |source| Error::Read {
            path: self.inner.git_dir().join("logs/refs/stash"),
            source,
        };
        let Some(lines) = log.all().map_err(read)? else {
            return Ok(Vec::new());
        };
        let ids = (lines.map(|line| Ok(ObjectId::from_hex(line?.new_oid)?)))
            .collect::<Result<Vec<_>, gix::Error>>()?;
        (ids.iter().rev().enumerate())
            .map(|(n, id)| {
                let commit = self.inner.find_commit(*id)?;
                let (message, author) = describe(&commit);
                let base = commit.parent_ids().next().map(|p| p.to_string());
                Ok(Commit {
                    id: id.to_string(),
                    summary: message.lines().next().unwrap_or_default().to_owned(),
                    message,
                    author,
                    time: commit.time().map(|t| t.seconds).unwrap_or_default(),
                    parents: base.into_iter().collect(),
                    refs: Box::new([format!("stash@{{{n}}}")]),
                    head: false,
                })
            })
            .collect()
    }

    /// Runs `git stash <op>` on the stash whose commit is `id`, wherever it is in the list now.
    /// Returns the paths left conflicted, empty when it went cleanly; a pop that conflicts
    /// keeps the stash, as git does.
    ///
    /// # Errors
    /// Returns [`Error::Refused`] if that stash is gone, and [`Error::Command`] if git can't
    /// run or fails for another reason, such as local changes in the way.
    pub fn stash(&self, op: StashOp, id: &str) -> Result<Vec<PathBuf>, Error> {
        let name = self.stash_ref(id)?;
        let cmd = op.as_str();
        let output = self.git_stash(cmd, &[&name])?;
        self.finish(cmd, &output, op != StashOp::Drop)
    }

    /// Stashes the changes as `push` says; `false` when there was nothing to stash.
    ///
    /// # Errors
    /// Returns [`Error::Command`] if git can't run or refuses, and [`Error::Git`] if the stash
    /// list can't be read.
    pub fn stash_push(&self, push: &StashPush) -> Result<bool, Error> {
        let top = |repo: &Self| repo.stashes().map(|s| s.first().map(|c| c.id.clone()));
        let before = top(self)?;
        let flags = [
            (push.untracked, "--include-untracked"),
            (push.keep_index, "--keep-index"),
            (push.staged, "--staged"),
        ];
        let mut args: Vec<&str> = (flags.iter()).filter(|f| f.0).map(|f| f.1).collect();
        let message = push.message.trim();
        if !message.is_empty() {
            args.extend(["-m", message]);
        }
        let output = self.git_stash("push", &args)?;
        self.finish("push", &output, false)?;
        // git exits 0 with "No local changes to save", so the list says whether it stashed.
        Ok(top(self)? != before)
    }

    /// `git stash branch <name>` on the stash `id`: a new branch at its base with the stash
    /// applied, then dropped. Returns the paths left conflicted, empty when clean.
    ///
    /// # Errors
    /// Returns [`Error::Refused`] for a blank name or one starting with `-`, or if the stash is
    /// gone, and [`Error::Command`] if git refuses, such as for a name already taken.
    pub fn stash_branch(&self, id: &str, name: &str) -> Result<Vec<PathBuf>, Error> {
        let name = name.trim();
        if name.is_empty() || name.starts_with('-') {
            return Err(Error::Refused("branch name can't be blank or start with -"));
        }
        let stash = self.stash_ref(id)?;
        let output = self.git_stash("branch", &[name, &stash])?;
        self.finish("branch", &output, true)
    }

    /// What a stash holds: its base against it, plus the untracked files it saved, which git
    /// keeps in a third parent.
    pub(crate) fn stash_changes(&self, id: &str) -> Result<Vec<Change>, Error> {
        let commit = self
            .inner
            .rev_parse_single(id)?
            .object()?
            .peel_to_commit()?;
        let parents: Vec<String> = commit.parent_ids().map(|p| p.to_string()).collect();
        let base = parents.first().map_or(EMPTY_TREE, String::as_str);
        let mut changes = self.changes(&Spec::Revs(base.to_owned(), id.to_owned()))?;
        if let Some(untracked) = parents.get(2) {
            let saved = self.changes(&Spec::Revs(EMPTY_TREE.to_owned(), untracked.clone()))?;
            changes.extend(saved.into_iter().map(|mut c| {
                c.untracked = true;
                c
            }));
            changes.sort_by(|a, b| a.path.cmp(&b.path));
        }
        Ok(changes)
    }

    /// `stash@{n}` for the stash `id` where it is in the list now.
    fn stash_ref(&self, id: &str) -> Result<String, Error> {
        // ponytail: another git can change the list between this lookup and the command; the
        // gap is milliseconds.
        let n = (self.stashes()?.iter())
            .position(|s| s.id == id)
            .ok_or(Error::Refused("that stash is gone; reopen the list"))?;
        Ok(format!("stash@{{{n}}}"))
    }

    /// Runs `git stash <cmd> <args>` in the worktree.
    fn git_stash(&self, cmd: &'static str, args: &[&str]) -> Result<Output, Error> {
        Command::new("git")
            .arg("-C")
            .arg(&self.workdir)
            .args(["stash", cmd])
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| Error::Command {
                cmd,
                stderr: if e.kind() == io::ErrorKind::NotFound {
                    "git not found on PATH".into()
                } else {
                    e.to_string()
                },
            })
    }

    /// The conflicted paths a failed command left when `merges` (it applies a stash), else
    /// its first line of stderr as the error.
    fn finish(
        &self,
        cmd: &'static str,
        output: &Output,
        merges: bool,
    ) -> Result<Vec<PathBuf>, Error> {
        if output.status.success() {
            return Ok(Vec::new());
        }
        let conflicts = if merges {
            self.conflicts()?
        } else {
            Vec::new()
        };
        if conflicts.is_empty() {
            let stderr = output.stderr.to_str_lossy();
            let line = stderr.lines().find(|l| !l.trim().is_empty());
            return Err(Error::Command {
                cmd,
                stderr: line.unwrap_or("failed").trim().to_owned(),
            });
        }
        Ok(conflicts)
    }

    /// The paths with merge conflicts in the index, each once.
    fn conflicts(&self) -> Result<Vec<PathBuf>, Error> {
        let index = self.index()?;
        let mut paths: Vec<PathBuf> = (index.entries().iter())
            .filter(|e| e.stage() != Stage::Unconflicted)
            .map(|e| gix::path::from_bstr(e.path(&index)).into_owned())
            .collect();
        paths.dedup();
        Ok(paths)
    }
}

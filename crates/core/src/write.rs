//! Changing the index and committing it: `git add`, `git restore --staged`, and `git commit`.

use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;

use gix::bstr::BStr;
use gix::index::entry::{Flags, Mode, Stage, Stat};

use crate::Error;
use crate::repo::{Repo, read_worktree};

impl Repo {
    /// Stages `stage` and unstages `unstage`, both worktree-relative, in one index write.
    ///
    /// # Errors
    /// Returns [`Error::Git`] if the index is locked by another git or can't be written,
    /// [`Error::Read`] if a file can't be read, and [`Error::Refused`] for a sparse index.
    pub fn apply(&self, stage: &[PathBuf], unstage: &[PathBuf]) -> Result<(), Error> {
        let mut index = self.index()?;
        let head = self.tree("HEAD")?;
        let (mut pipeline, _) = self.inner.filter_pipeline(None)?;
        for path in stage {
            let key = gix::path::into_bstr(path.as_path());
            let full = self.workdir.join(path);
            let meta = match gix::index::fs::Metadata::from_path_no_follow(&full) {
                Ok(meta) => meta,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    remove(&mut index, &key);
                    continue;
                }
                Err(source) => return Err(Error::Read { path: full, source }),
            };
            let read_err = |source| Error::Read {
                path: full.clone(),
                source,
            };
            let bytes = if meta.is_symlink() {
                read_worktree(&full).map_err(read_err)?
            } else {
                let file = fs::File::open(&full).map_err(read_err)?;
                let mut bytes = Vec::new();
                pipeline
                    .convert_to_git(file, path, &index)?
                    .read_to_end(&mut bytes)
                    .map_err(read_err)?;
                bytes
            };
            let id = self.inner.write_blob(bytes)?.detach();
            let mode = if meta.is_symlink() {
                Mode::SYMLINK
            } else if meta.is_executable() {
                Mode::FILE_EXECUTABLE
            } else {
                Mode::FILE
            };
            // A stat that can't be read only costs a re-hash on the next status.
            let stat = Stat::from_fs(&meta).unwrap_or_default();
            upsert(&mut index, &key, id, mode, stat);
        }
        for path in unstage {
            let key = gix::path::into_bstr(path.as_path());
            match head.lookup_entry_by_path(path)? {
                // A zero stat makes the next status re-hash the file instead of trusting it.
                Some(entry) => {
                    let mode = Mode::from(entry.mode());
                    upsert(&mut index, &key, entry.object_id(), mode, Stat::default());
                }
                None => remove(&mut index, &key),
            }
        }
        // ponytail: dropping the tree cache makes git's next commit rebuild every tree;
        // keep it updated instead once gix issue #2421 lands.
        index.remove_tree();
        (index.write(gix::index::write::Options::default())).map_err(|e| Error::Git(e.into()))?;
        Ok(())
    }

    /// Commits the index on HEAD and returns the new commit's short id.
    ///
    /// Hooks don't run. Refused when signing is on, rather than writing an unsigned commit.
    ///
    /// # Errors
    /// Returns [`Error::Refused`] when signing is on, the index has conflicts, or nothing is
    /// staged, and [`Error::Git`] if the author isn't configured or a write fails.
    pub fn commit(&self, message: &str) -> Result<String, Error> {
        let message = clean(message);
        if message.is_empty() {
            return Err(Error::Refused("the commit message is empty"));
        }
        if self.inner.config_snapshot().boolean("commit.gpgsign") == Some(true) {
            return Err(Error::Refused(
                "commit signing is on (commit.gpgsign); zdiff can't sign commits",
            ));
        }
        let index = self.index()?;
        let head = self.tree("HEAD")?;
        // ponytail: rebuilds the tree from every index entry, O(files); edit HEAD's tree with
        // only the staged changes once repos pass ~100k files.
        let mut editor = self.inner.empty_tree().edit()?;
        for entry in index.entries() {
            if entry.stage() != Stage::Unconflicted {
                return Err(Error::Refused("resolve conflicts first"));
            }
            let Some(mode) = entry.mode.to_tree_entry_mode() else {
                continue;
            };
            editor.upsert(entry.path(&index), mode.kind(), entry.id)?;
        }
        let tree = editor.write()?.detach();
        if tree == head.id {
            return Err(Error::Refused("nothing to commit"));
        }
        let parents = if self.inner.head()?.is_unborn() {
            None
        } else {
            Some(self.inner.head_id()?.detach())
        };
        let id = self.inner.commit("HEAD", message, tree, parents)?;
        Ok(id.to_hex_with_len(7).to_string())
    }

    /// The index fresh from disk, so a write never undoes another git's changes; empty in a
    /// new repository, which has no index file until something is staged.
    pub(crate) fn index(&self) -> Result<gix::index::File, Error> {
        let path = self.inner.index_path();
        let index = if path.exists() {
            self.inner.open_index()?
        } else {
            let state = gix::index::State::new(self.inner.object_hash());
            gix::index::File::from_state(state, path)
        };
        if index.is_sparse() {
            return Err(Error::Refused("sparse indexes aren't supported"));
        }
        Ok(index)
    }
}

/// Points the unconflicted entry for `path` at `id`, adding it when missing.
fn upsert(index: &mut gix::index::File, path: &BStr, id: gix::ObjectId, mode: Mode, stat: Stat) {
    if let Some(entry) = index.entry_mut_by_path_and_stage(path, Stage::Unconflicted) {
        (entry.id, entry.mode, entry.stat) = (id, mode, stat);
    } else {
        index.dangerously_push_entry(stat, id, Flags::empty(), mode, path);
        // Lookups binary-search, so the next path must see sorted entries.
        index.sort_entries();
    }
}

/// `message` the way `git commit -m` stores it: trailing spaces and blank lines at either end
/// dropped, runs of blank lines collapsed to one, ending in a newline; empty if nothing is left.
fn clean(message: &str) -> String {
    let mut out = String::with_capacity(message.len() + 1);
    let mut blank = false;
    for line in message.lines().map(str::trim_end) {
        if line.is_empty() {
            blank = !out.is_empty();
            continue;
        }
        if blank {
            out.push('\n');
            blank = false;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn remove(index: &mut gix::index::File, path: &BStr) {
    index.remove_entries(|_, entry_path, _| entry_path == path);
}

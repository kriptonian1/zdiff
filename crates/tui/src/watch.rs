//! Background refresh for `--watch`: file events in, finished snapshots out.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use notify::{Event, RecursiveMode, Watcher};
use zdiff_core::{Repo, Spec};

use crate::Msg;
use crate::snapshot::{Snapshot, Touched};

/// Quiet time that ends a batch; editors write a file several times per save.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(50);
/// Longest a batch waits, so a steady stream of writes (a build) still refreshes.
const WATCH_MAX_DELAY: Duration = Duration::from_millis(500);

/// What one changed path means for the next refresh.
#[derive(Debug, PartialEq, Eq)]
enum Touch {
    All,
    Path(PathBuf),
}

/// Starts watching `workdir`; each batch of edits sends a fresh [`Snapshot`] to `tx`.
///
/// Setup happens before returning so watcher errors surface before the TUI starts.
pub fn spawn(workdir: &Path, spec: Spec, first: Snapshot, tx: Sender<Msg>) -> anyhow::Result<()> {
    // Event paths are canonical (e.g. /private/var on macOS); match them against a canonical root.
    let root = workdir.canonicalize()?;
    let (events_tx, events) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(events_tx)?;
    watcher.watch(&root, RecursiveMode::Recursive)?;
    let repo = Repo::discover(&root)?;
    // ponytail: gitignored dirs (target/) still trigger refreshes; filter with gix excludes if noisy.
    thread::spawn(move || {
        // Events stop once the watcher is dropped, so it lives as long as this thread.
        let _watcher = watcher;
        let mut previous = first;
        while let Some(touched) = collect(&events, &root) {
            // Index locked or mid-rebase: skip this batch, the next event retries.
            let Ok(snapshot) = Snapshot::load(&repo, &spec, Some(&previous), &touched) else {
                continue;
            };
            // ponytail: snapshot cloned for the UI (paths + counts); share via Arc if repos get huge.
            let message = Msg::Refreshed {
                snapshot: snapshot.clone(),
                touched,
            };
            if tx.send(message).is_err() {
                return;
            }
            previous = snapshot;
        }
    });
    Ok(())
}

/// Blocks until a batch of relevant events is ready; `None` once the watcher is gone.
fn collect(events: &Receiver<notify::Result<Event>>, root: &Path) -> Option<Touched> {
    loop {
        let mut touched = Touched::Paths(HashSet::new());
        let mut next = Some(events.recv().ok()?);
        let deadline = Instant::now() + WATCH_MAX_DELAY;
        while let Some(event) = next {
            add(&mut touched, event, root);
            let wait = WATCH_DEBOUNCE.min(deadline.saturating_duration_since(Instant::now()));
            next = events.recv_timeout(wait).ok();
        }
        if !matches!(&touched, Touched::Paths(paths) if paths.is_empty()) {
            return Some(touched);
        }
    }
}

fn add(touched: &mut Touched, event: notify::Result<Event>, root: &Path) {
    let event = match event {
        // Our own reads while refreshing would otherwise trigger another refresh.
        Ok(event) if event.kind.is_access() => return,
        Ok(event) => event,
        // Missed or overflowed events: rescan everything.
        Err(_) => {
            *touched = Touched::All;
            return;
        }
    };
    for path in event.paths {
        match (classify(root, &path), &mut *touched) {
            (Some(Touch::All), _) => *touched = Touched::All,
            (Some(Touch::Path(path)), Touched::Paths(paths)) => {
                paths.insert(path);
            }
            _ => {}
        }
    }
}

/// Maps an event path to what it invalidates; `None` for git internals that don't matter.
fn classify(root: &Path, path: &Path) -> Option<Touch> {
    let Ok(relative) = path.strip_prefix(root) else {
        return Some(Touch::All);
    };
    let mut parts = relative.components();
    if parts.next() != Some(Component::Normal(".git".as_ref())) {
        return Some(Touch::Path(relative.to_owned()));
    }
    // Staging, committing, or switching branches rewrites one of these.
    match parts.next()?.as_os_str().to_str()? {
        "index" | "HEAD" | "refs" | "packed-refs" => Some(Touch::All),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use notify::EventKind;
    use notify::event::{AccessKind, ModifyKind};

    use super::*;

    const ROOT: &str = "/repo";

    fn modified(path: &str) -> Event {
        Event::new(EventKind::Modify(ModifyKind::Any)).add_path(path.into())
    }

    #[test]
    fn classify_worktree_files_and_git_internals() {
        let root = Path::new(ROOT);
        let touch = |p: &str| classify(root, Path::new(p));
        assert_eq!(
            touch("/repo/src/a.rs"),
            Some(Touch::Path("src/a.rs".into()))
        );
        assert_eq!(touch("/repo/.git/index"), Some(Touch::All));
        assert_eq!(touch("/repo/.git/refs/heads/main"), Some(Touch::All));
        assert_eq!(touch("/repo/.git/objects/ab/cdef"), None);
        assert_eq!(touch("/elsewhere/a.rs"), Some(Touch::All));
    }

    #[test]
    fn a_burst_of_events_becomes_one_batch() {
        let (tx, rx) = mpsc::channel();
        for path in ["/repo/a.rs", "/repo/b.rs", "/repo/a.rs"] {
            tx.send(Ok(modified(path))).unwrap();
        }
        let expected = HashSet::from([PathBuf::from("a.rs"), PathBuf::from("b.rs")]);
        assert_eq!(
            collect(&rx, Path::new(ROOT)),
            Some(Touched::Paths(expected))
        );
        drop(tx);
        assert_eq!(collect(&rx, Path::new(ROOT)), None, "watcher gone");
    }

    #[test]
    fn reads_and_git_objects_are_ignored() {
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(
            Event::new(EventKind::Access(AccessKind::Any)).add_path("/repo/a.rs".into())
        ))
        .unwrap();
        tx.send(Ok(modified("/repo/.git/objects/x"))).unwrap();
        tx.send(Ok(modified("/repo/b.rs"))).unwrap();
        let expected = HashSet::from([PathBuf::from("b.rs")]);
        assert_eq!(
            collect(&rx, Path::new(ROOT)),
            Some(Touched::Paths(expected))
        );
    }

    #[test]
    fn editing_a_real_repo_sends_a_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(init.success());
        std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
        let repo = Repo::discover(dir.path()).unwrap();
        let first = Snapshot::load(&repo, &Spec::default(), None, &Touched::All).unwrap();
        let (tx, rx) = mpsc::channel();
        spawn(repo.workdir(), Spec::default(), first, tx).unwrap();

        std::fs::write(dir.path().join("b.txt"), "b\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        // Fails via `expect` unless a refresh listing both files arrives in time.
        loop {
            let wait = deadline.saturating_duration_since(Instant::now());
            let msg = rx.recv_timeout(wait).expect("a refresh within 5s");
            if matches!(msg, Msg::Refreshed { snapshot, .. } if snapshot.changes.len() == 2) {
                break;
            }
        }
    }

    #[test]
    fn a_steady_stream_still_returns_by_the_deadline() {
        let (tx, rx) = mpsc::channel();
        let writer = thread::spawn(move || {
            while tx.send(Ok(modified("/repo/target/out"))).is_ok() {
                thread::sleep(Duration::from_millis(10));
            }
        });
        let started = Instant::now();
        assert!(collect(&rx, Path::new(ROOT)).is_some());
        assert!(started.elapsed() < WATCH_MAX_DELAY + WATCH_DEBOUNCE * 2);
        drop(rx);
        writer.join().unwrap();
    }
}

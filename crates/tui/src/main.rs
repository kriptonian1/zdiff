mod app;
mod clipboard;
mod find;
mod history;
mod input;
mod keymap;
mod menu;
mod palette;
mod preview;
mod search;
mod settings;
mod snapshot;
mod source;
mod stream;
mod text;
mod tree;
mod ui;
mod watch;

use std::fmt::Write as _;
use std::io::{self, IsTerminal, Write};
use std::mem;
use std::panic;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Instant;

use clap::Parser;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use ratatui::DefaultTerminal;
use ratatui_image::picker::{Picker, ProtocolType};
use zdiff_core::{Repo, Spec, StashOp};

use anyhow::anyhow;
use app::{App, Apply, Focus, Notice, Toast};
use settings::Settings;
use snapshot::{Only, Snapshot, Touched};
use source::Source;

/// Terminal viewer for your current git changes.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Keep running and refresh the diff as files change.
    #[arg(short, long)]
    watch: bool,
    /// Show diffs only: no staging or committing, whatever the settings say.
    #[arg(long)]
    read_only: bool,
    /// View a patch file instead of the repository; `-` reads stdin.
    #[arg(long, value_name = "FILE", conflicts_with = "watch")]
    patch: Option<PathBuf>,
    /// Show only these files, folders, or globs (`'src/**/*.rs'`), relative to the current
    /// directory.
    #[arg(short = 'f', long, value_name = "PATH", num_args = 1..)]
    focus: Vec<PathBuf>,
}

/// Everything the UI thread reacts to, from the input and watch threads.
pub enum Msg {
    Term(io::Result<Event>),
    Refreshed {
        snapshot: Snapshot,
        touched: Touched,
    },
    /// Global search hits in one file, for search `generation`.
    Found {
        generation: u64,
        change: usize,
        hits: Vec<search::Found>,
    },
    SearchDone {
        generation: u64,
    },
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let cwd = std::env::current_dir()?;
    let mut source = if let Some(path) = &args.patch {
        // A patch's paths are as written in it, not on disk.
        let only = Only::new(args.focus.iter().map(|p| clean(p)).collect());
        Source::patch(path, only.map_err(|e| anyhow!("--focus: {e}"))?)?
    } else {
        let spec = Spec::default();
        let repo = Repo::discover(".")?;
        let only = resolve(&args.focus, &cwd, repo.workdir())?;
        // ponytail: counts computed serially at startup; use a worker thread at 1000+ files.
        let snapshot = Snapshot::load(&repo, &spec, None, &Touched::All, &only)?;
        Source::Repo {
            repo,
            spec,
            snapshot,
            only,
        }
    };
    let mut app = App::new(source.entries());
    let name = |path: &Path| (path.file_name()).map(|name| name.to_string_lossy().into_owned());
    match &source {
        Source::Repo { repo, spec, .. } => {
            app.repo_name = name(repo.workdir()).unwrap_or_default();
            app.branch = repo.head_name();
            app.compare = spec.to_string();
        }
        Source::Patch { file, patch, .. } => {
            app.repo_name = file
                .as_deref()
                .and_then(name)
                .unwrap_or_else(|| "stdin".into());
            patch
                .subject()
                .unwrap_or("patch")
                .clone_into(&mut app.branch);
            app.compare = "patch".into();
        }
    }
    let config = settings::path();
    let (loaded, mut warnings) = config.as_deref().map(Settings::load).unwrap_or_default();
    warnings.extend(app.apply(loaded));
    // A patch is only viewed, never applied.
    app.read_only = args.read_only || args.patch.is_some();
    app.patch_mode = args.patch.is_some();
    if !warnings.is_empty() {
        app.notice = Some(Notice::Error(format!("settings: {}", warnings.join("; "))));
    }
    app.only = source.only().label();
    if let Some(label) = &app.only {
        let _ = write!(app.compare, " · only {label}");
    }
    // One file needs no file list; a folder or glob keeps it.
    if let [path] = args.focus.as_slice()
        && !cwd.join(path).is_dir()
        && !snapshot::is_glob(&path.to_string_lossy())
    {
        app.sidebar_hidden = true;
        app.focus = Focus::Diff;
    }

    let (tx, rx) = mpsc::channel();
    if let Source::Repo {
        repo,
        spec,
        snapshot,
        only,
    } = &source
        && (args.watch || app.watch_default)
    {
        let (spec, first) = (spec.clone(), snapshot.clone());
        watch::spawn(repo.workdir(), spec, first, only.clone(), tx.clone())?;
    }

    // Runs after ratatui's own restore hook, so a crash also releases the mouse and keyboard.
    let hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            DisableBracketedPaste,
            PopKeyboardEnhancementFlags
        );
        hook(info);
    }));
    ratatui::run(|terminal| {
        // Bracketed paste: a paste arrives as one event, not as typed keys (newlines as Enter).
        execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
        // Kitty keyboard protocol: without it terminals send Shift+Enter as Enter.
        // Queried before the input thread starts, since the query reads stdin.
        let enhanced = crossterm::terminal::supports_keyboard_enhancement()?;
        if enhanced {
            let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
            execute!(io::stdout(), PushKeyboardEnhancementFlags(flags))?;
        }
        // Image protocol for previews; this query reads stdin too, so it also runs first.
        // A piped stdin (`--patch -`) can't answer, and halfblocks count as no images.
        app.images = (io::stdin().is_terminal())
            .then(Picker::from_query_stdio)
            .and_then(Result::ok)
            .filter(|picker| picker.protocol_type() != ProtocolType::Halfblocks);
        spawn_input(tx.clone());
        let result = event_loop(terminal, app, (&mut source, config.as_deref()), (&tx, &rx));
        if enhanced {
            execute!(io::stdout(), PopKeyboardEnhancementFlags)?;
        }
        execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste)?;
        result
    })?;
    Ok(())
}

/// Forwards terminal input to the UI thread; started after raw mode is on.
fn spawn_input(tx: Sender<Msg>) {
    thread::spawn(move || {
        loop {
            let event = event::read();
            let failed = event.is_err();
            if tx.send(Msg::Term(event)).is_err() || failed {
                return;
            }
        }
    });
}

/// Redraws only after a visible change and keeps the diff pane on the selected file.
fn event_loop(
    terminal: &mut DefaultTerminal,
    mut app: App,
    (source, config): (&mut Source, Option<&Path>),
    (tx, rx): (&Sender<Msg>, &Receiver<Msg>),
) -> io::Result<()> {
    // Started on the first global search, so sessions that never search pay nothing.
    let mut searcher: Option<Sender<search::Job>> = None;
    let mut dirty = true;
    let mut shown: Option<PathBuf> = None;
    // The history popup's selected commit and its changes, kept for the preview's diffs.
    let mut history_files: Option<(String, Snapshot)> = None;
    // Whether the shown file changed on disk and its diff must reload in place.
    let mut stale = false;
    while !app.quit {
        let selected = app.single_file().map(|f| (f.path.clone(), f.change));
        let moved = selected.as_ref().map(|(path, _)| path) != shown.as_ref();
        if moved || stale {
            // ponytail: diff loads on the UI thread; use a worker if big files stall input.
            let diff = selected.as_ref().map(|&(_, change)| source.diff(change));
            if moved {
                app.show(diff);
            } else {
                app.reload(diff);
            }
            shown = selected.map(|(path, _)| path);
            stale = false;
            dirty = true;
        }
        // ponytail: All files sections load on the UI thread; use a worker like
        // `search::spawn` if fast scrolling stutters.
        for (section, change) in app.stream_wants() {
            app.stream_loaded(section, source.diff(change));
            dirty = true;
        }
        if dirty {
            terminal.draw(|frame| ui::draw(frame, &mut app))?;
        }
        let Some(message) = wait(&mut app, rx) else {
            // A toast expired: draw without it.
            dirty = true;
            continue;
        };
        dirty = match message {
            Ok(Msg::Term(event)) => app.handle(&event?),
            // The watcher compares against the working tree, not the commit being viewed.
            Ok(Msg::Refreshed { .. }) if app.viewing.is_some() => false,
            Ok(Msg::Refreshed {
                snapshot: next,
                touched,
            }) => {
                stale = shown.as_deref().is_some_and(|path| touched.contains(path));
                app.refresh(next.entries());
                if let Source::Repo { repo, snapshot, .. } = source {
                    app.branch = repo.head_name();
                    *snapshot = next;
                }
                true
            }
            Ok(Msg::Found {
                generation,
                change,
                hits,
            }) => app.search_found(generation, change, hits),
            Ok(Msg::SearchDone { generation }) => app.search_done(generation),
            Err(_) => break,
        };
        if let Some((generation, finder, indices)) = app.pending_search.take() {
            if searcher.is_none() {
                // ponytail: a worker that fails to start leaves search silent; surface it if seen.
                searcher = search::spawn(source.workdir(), tx.clone()).ok();
            }
            if let Some(jobs) = &searcher {
                let _ = jobs.send(search::Job {
                    generation,
                    finder,
                    items: source.search_items(indices),
                });
            }
        }
        // ponytail: staging runs on the UI thread, milliseconds for normal files; use a
        // worker if LFS or other process filters stall input.
        if let (Some(work), Source::Repo { repo, .. }) = (app.pending_apply.take(), &*source) {
            app.finish(run_git(repo, work).map_err(|e| e.to_string()));
            dirty = true;
        }
        // Answers can ask for more (files, then the first file's diff), so loop until done.
        while !app.pending_history.is_empty() {
            for want in mem::take(&mut app.pending_history) {
                if history_want(&mut app, source, want, &mut history_files) {
                    // Same path, other commit: the diff must load again.
                    shown = None;
                }
                dirty = true;
            }
        }
        if mem::take(&mut app.pending_reload) {
            stale |= reload(&mut app, source, tx);
            dirty = true;
        }
        if let Some(text) = app.pending_copy.take() {
            // An invisible control sequence, so it doesn't disturb the screen.
            let mut out = io::stdout();
            out.write_all(clipboard::osc52(&text).as_bytes())?;
            out.flush()?;
        }
        if mem::take(&mut app.pending_save)
            && let Some(path) = config
            && let Err(e) = app.settings().save(path)
        {
            app.notice = Some(Notice::Error(format!("couldn't save settings: {e}")));
        }
    }
    Ok(())
}

/// The next message, or `None` once a shown toast expires and has been cleared, so it isn't
/// left on screen; with no toast this simply blocks.
fn wait(app: &mut App, rx: &Receiver<Msg>) -> Option<Result<Msg, mpsc::RecvError>> {
    let Some(toast) = &app.toast else {
        return Some(rx.recv());
    };
    match rx.recv_timeout(toast.until.saturating_duration_since(Instant::now())) {
        Err(RecvTimeoutError::Timeout) => {
            app.toast = None;
            None
        }
        message => Some(message.map_err(|_| mpsc::RecvError)),
    }
}

/// Reloads the file list; `true` when the shown file must reload in place now.
fn reload(app: &mut App, source: &mut Source, tx: &Sender<Msg>) -> bool {
    match source {
        // Sent through the watcher's path, so a manual reload refreshes the same way.
        // A failed load (index locked) keeps the current view, like the watcher does.
        Source::Repo {
            repo,
            spec,
            snapshot,
            only,
        } => {
            let Ok(next) = Snapshot::load(repo, spec, Some(snapshot), &Touched::All, only) else {
                return false;
            };
            // Refreshes are dropped while a commit is shown, so apply this one here: against
            // the working tree (`w`) the files can change.
            if app.viewing.is_some() {
                *snapshot = next;
                app.refresh(snapshot.entries());
                return true;
            }
            let _ = tx.send(Msg::Refreshed {
                snapshot: next,
                touched: Touched::All,
            });
            false
        }
        Source::Patch { .. } => match source.reload_patch() {
            Ok(reloaded) => {
                if reloaded {
                    app.refresh(source.entries());
                }
                reloaded
            }
            Err(e) => {
                app.notice = Some(Notice::Error(format!("{e:#}")));
                false
            }
        },
    }
}

/// Does history work for the popup and the commit view; `true` when the main view now shows
/// another commit or the working tree again.
fn history_want(
    app: &mut App,
    source: &mut Source,
    want: history::Want,
    files: &mut Option<(String, Snapshot)>,
) -> bool {
    let Source::Repo {
        repo,
        spec,
        snapshot,
        only,
    } = source
    else {
        return false;
    };
    let load = |spec: &Spec| Snapshot::load(repo, spec, None, &Touched::All, only);
    // ponytail: log, file lists and counts run on the UI thread; use a worker if big
    // commits or long histories stall input.
    let result = match want {
        history::Want::Commits(limit) => repo.log(limit).map(|commits| {
            app.history_commits(commits);
            false
        }),
        history::Want::Files(commit) => load(&repo.commit_spec(&commit)).map(|snapshot| {
            app.history_files(&commit.id, snapshot.entries());
            *files = Some((commit.id.clone(), snapshot));
            false
        }),
        history::Want::Preview { commit, path } => {
            // The commit's changes from its file list, unless the selection moved since.
            if files.as_ref().is_none_or(|(id, _)| *id != commit.id) {
                match load(&repo.commit_spec(&commit)) {
                    Ok(snapshot) => *files = Some((commit.id.clone(), snapshot)),
                    Err(e) => {
                        app.notice = Some(Notice::Error(format!("history: {e}")));
                        return false;
                    }
                }
            }
            let changes = files.as_ref().map(|(_, snapshot)| &snapshot.changes);
            let change = changes.and_then(|changes| changes.iter().find(|c| c.path == path));
            let diff = change.map_or_else(
                || Ok(zdiff_core::FileDiff::new(Vec::new(), Vec::new())),
                |change| repo.diff(change),
            );
            app.history_preview(&commit.id, &path, diff);
            Ok(false)
        }
        history::Want::Open {
            commit,
            file,
            back,
            worktree,
        } => {
            let next = if worktree {
                Spec::Worktree(commit.id.clone())
            } else {
                repo.commit_spec(&commit)
            };
            load(&next).map(|files| {
                (*spec, *snapshot) = (next, files);
                app.refresh(snapshot.entries());
                app.viewing_opened((&commit, worktree), file.as_deref(), back);
                true
            })
        }
        history::Want::Stashes => repo.stashes().map(|stashes| {
            if let Some(history) = &mut app.history {
                history.head = repo.head_id();
            }
            app.history_commits(stashes);
            false
        }),
        want @ (history::Want::Stash { .. }
        | history::Want::StashPush(_)
        | history::Want::StashBranch { .. }) => {
            stash_want(app, repo, want);
            return false;
        }
        history::Want::Back => load(&Spec::default()).map(|files| {
            (*spec, *snapshot) = (Spec::default(), files);
            app.refresh(snapshot.entries());
            app.viewing_closed();
            true
        }),
    };
    result.unwrap_or_else(|e| {
        app.notice = Some(Notice::Error(format!("history: {e}")));
        false
    })
}

/// Runs a stash command for the popup and reports how it went.
// ponytail: git runs on the UI thread; a worker and a `Msg` if big repos freeze.
fn stash_want(app: &mut App, repo: &Repo, want: history::Want) {
    match want {
        history::Want::Stash { op, commit } => {
            let name = history::stash_name(&commit).unwrap_or_default();
            let done = match op {
                StashOp::Apply => "applied",
                StashOp::Pop => "popped",
                StashOp::Drop => "dropped",
            };
            let kept = if op == StashOp::Pop {
                "; stash kept"
            } else {
                ""
            };
            let result = repo.stash(op, &commit.id);
            stash_done(app, (op.as_str(), kept), format!("✓ {done} {name}"), result);
        }
        history::Want::StashPush(push) => match repo.stash_push(&push) {
            Ok(false) => app.toast = Some(Toast::new("nothing to stash".into())),
            result => stash_done(
                app,
                ("push", ""),
                "✓ stashed".into(),
                result.map(|_| Vec::new()),
            ),
        },
        history::Want::StashBranch { commit, name } => {
            let from = history::stash_name(&commit).unwrap_or_default();
            let result = repo.stash_branch(&commit.id, &name);
            let done = format!("✓ branch {name} from {from}");
            stash_done(app, ("branch", "; stash kept"), done, result);
        }
        _ => {}
    }
}

/// Shows `done` or the conflicts a stash command left, then reloads the list and the
/// worktree, which may both have changed even when it failed; `kept` follows the conflicts
/// when git kept the stash.
fn stash_done(
    app: &mut App,
    (cmd, kept): (&str, &str),
    done: String,
    result: Result<Vec<PathBuf>, zdiff_core::Error>,
) {
    match result {
        Ok(conflicts) if conflicts.is_empty() => app.toast = Some(Toast::new(done)),
        Ok(conflicts) => {
            let paths: Vec<_> = conflicts.iter().map(|p| p.display().to_string()).collect();
            let why = format!(
                "{cmd}: {} conflicts ({}){kept}",
                paths.len(),
                paths.join(", ")
            );
            app.notice = Some(Notice::Error(why));
        }
        Err(e) => app.notice = Some(Notice::Error(e.to_string())),
    }
    app.pending_history.push(history::Want::Stashes);
    app.pending_reload = true;
}

/// `path` without `.` and `..` parts, worked out from the text alone so a deleted file still
/// resolves; a `..` past the start is kept.
fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir if out.file_name().is_some() => {
                out.pop();
            }
            part => out.push(part),
        }
    }
    out
}

/// `--focus` paths given relative to `cwd`, as paths relative to the repository root.
fn resolve(paths: &[PathBuf], cwd: &Path, workdir: &Path) -> anyhow::Result<Only> {
    // Both exist, so canonical forms compare reliably (macOS: /var is /private/var).
    let (cwd, root) = (cwd.canonicalize()?, workdir.canonicalize()?);
    let only = (paths.iter())
        .map(|path| {
            let full = clean(&cwd.join(path));
            (full.strip_prefix(&root).map(Path::to_owned))
                .map_err(|_| anyhow!("--focus {}: not inside the repository", path.display()))
        })
        .collect::<anyhow::Result<_>>()?;
    Only::new(only).map_err(|e| anyhow!("--focus: {e}"))
}

/// Stages and unstages `work`'s files, then commits when asked; the commit's short id.
fn run_git(repo: &Repo, work: Apply) -> Result<Option<String>, zdiff_core::Error> {
    if !(work.stage.is_empty() && work.unstage.is_empty()) {
        repo.apply(&work.stage, &work.unstage)?;
    }
    work.commit.map(|message| repo.commit(&message)).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_can_be_viewed_but_not_watched() {
        let args = Args::try_parse_from(["zdiff", "--patch", "x.patch"]).expect("parses");
        assert_eq!(args.patch.as_deref(), Some(Path::new("x.patch")));
        assert!(Args::try_parse_from(["zdiff", "--patch", "x.patch", "--watch"]).is_err());
    }

    #[test]
    fn focus_takes_several_paths_and_goes_with_other_flags() {
        let focus = |argv: &[&str]| {
            let args = Args::try_parse_from(["zdiff"].iter().chain(argv)).expect("parses");
            (args.focus.iter())
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            focus(&["-f", "a.rs", "src", "docs", "--patch", "x"]),
            ["a.rs", "src", "docs"]
        );
        assert_eq!(focus(&["-f", "a.rs", "-f", "b.rs"]), ["a.rs", "b.rs"]);
        assert_eq!(focus(&["--watch", "--focus", "src/*.rs"]), ["src/*.rs"]);
    }

    #[test]
    fn clean_drops_dots_without_touching_the_disk() {
        assert_eq!(clean(Path::new("/r/./src/../a.rs")), Path::new("/r/a.rs"));
        assert_eq!(clean(Path::new("./gone.rs")), Path::new("gone.rs"));
        assert_eq!(
            clean(Path::new("../../x")),
            Path::new("../../x"),
            "kept past the start"
        );
        assert_eq!(clean(Path::new(".")), Path::new(""));
    }

    #[test]
    fn focus_paths_resolve_from_the_current_dir_to_the_repo_root() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/ui")).unwrap();
        let only = |cwd: &Path, paths: &[&str]| {
            let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
            resolve(&paths, cwd, root)
        };
        let ui = root.join("src/ui");
        let kept = only(&ui, &["diff.rs", "../app.rs", "deleted.rs"]).expect("inside");
        assert!(kept.keeps(Path::new("src/ui/diff.rs")));
        assert!(kept.keeps(Path::new("src/app.rs")));
        assert!(kept.keeps(Path::new("src/ui/deleted.rs")), "no file needed");
        assert!(!kept.keeps(Path::new("src/main.rs")));

        let error = only(root, &["../x"]).expect_err("outside");
        assert!(
            error.to_string().contains("--focus ../x: not inside"),
            "{error}"
        );
        assert_eq!(
            only(&ui, &["."]).expect("inside").label().as_deref(),
            Some("src/ui")
        );
        assert_eq!(
            only(root, &["."]).expect("inside").label(),
            None,
            "the whole repo"
        );

        let globs = only(&ui, &["*.rs", "../**/mod.rs"]).expect("inside");
        assert!(globs.keeps(Path::new("src/ui/diff.rs")));
        assert!(
            !globs.keeps(Path::new("src/ui/x/deep.rs")),
            "`*` stays in its folder"
        );
        assert!(
            globs.keeps(Path::new("src/a/b/mod.rs")),
            "`**` crosses folders"
        );
        let error = only(root, &["src/[x"]).expect_err("a bad glob");
        assert!(error.to_string().starts_with("--focus: "), "{error}");
    }

    /// A repo with one commit adding `old.txt`, plus an uncommitted `wip.txt`, opened as zdiff
    /// would open it.
    fn one_commit_repo() -> (tempfile::TempDir, App, Source) {
        let dir = tempfile::tempdir().expect("temp dir");
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(["-c", "commit.gpgsign=false"])
                .args(args)
                .current_dir(dir.path())
                .status()
                .expect("git runs");
            assert!(ok.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        std::fs::write(dir.path().join("old.txt"), "1\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "add old"]);
        std::fs::write(dir.path().join("wip.txt"), "uncommitted\n").unwrap();

        let repo = Repo::discover(dir.path()).expect("a repo");
        let (spec, only) = (Spec::default(), Only::default());
        let snapshot = Snapshot::load(&repo, &spec, None, &Touched::All, &only).expect("loads");
        let app = App::new(snapshot.entries());
        let source = Source::Repo {
            repo,
            spec,
            snapshot,
            only,
        };
        (dir, app, source)
    }

    #[test]
    fn opening_a_commit_shows_its_files_and_back_restores_the_worktree() {
        let (dir, mut app, mut source) = one_commit_repo();
        let paths = |app: &App| {
            (app.tree.files())
                .map(|(_, f)| f.path.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(paths(&app), [PathBuf::from("wip.txt")]);

        assert!(
            !history_want(&mut app, &mut source, history::Want::Commits(10), &mut None),
            "popup closed: nothing to fill"
        );
        let Source::Repo { repo, .. } = &source else {
            unreachable!()
        };
        let commit = repo.log(1).expect("a log").remove(0);
        let open = history::Want::Open {
            commit: commit.clone(),
            file: None,
            back: Some("wip.txt".into()),
            worktree: false,
        };
        assert!(history_want(&mut app, &mut source, open, &mut None));
        assert_eq!(
            paths(&app),
            [PathBuf::from("old.txt")],
            "the commit's files"
        );
        assert!(app.viewing.is_some() && !app.can_stage());

        assert!(history_want(
            &mut app,
            &mut source,
            history::Want::Back,
            &mut None
        ));
        assert_eq!(
            paths(&app),
            [PathBuf::from("wip.txt")],
            "the working tree again"
        );
        assert!(app.viewing.is_none());
        assert!(matches!(&source, Source::Repo { spec, .. } if *spec == Spec::default()));

        let against_worktree = history::Want::Open {
            commit,
            file: None,
            back: None,
            worktree: true,
        };
        assert!(history_want(
            &mut app,
            &mut source,
            against_worktree,
            &mut None
        ));
        assert_eq!(
            paths(&app),
            [PathBuf::from("wip.txt")],
            "what changed since the commit"
        );
        let label = app
            .viewing
            .as_ref()
            .map(|v| v.label.clone())
            .unwrap_or_default();
        assert!(label.ends_with("→ worktree"), "{label}");

        // Watch refreshes are dropped while viewing, so Ctrl+R applies its own.
        std::fs::write(dir.path().join("new.txt"), "2\n").unwrap();
        let (tx, _rx) = mpsc::channel();
        assert!(reload(&mut app, &mut source, &tx));
        assert_eq!(
            paths(&app),
            [PathBuf::from("new.txt"), PathBuf::from("wip.txt")]
        );
    }

    #[test]
    fn popping_a_stash_restores_it_and_reloads() {
        let (dir, mut app, mut source) = one_commit_repo();
        let out = std::process::Command::new("git")
            .args(["stash", "push", "-q", "-u"])
            .current_dir(dir.path())
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!dir.path().join("wip.txt").exists(), "stashed away");
        app.history = Some(Box::new(history::History::new(history::Kind::Stash).0));

        history_want(&mut app, &mut source, history::Want::Stashes, &mut None);
        let stash = app.history.as_ref().expect("open").commits[0].clone();
        let pop = history::Want::Stash {
            op: StashOp::Pop,
            commit: stash,
        };
        history_want(&mut app, &mut source, pop, &mut None);
        assert!(
            app.toast
                .as_ref()
                .is_some_and(|t| t.text == "✓ popped stash@{0}")
        );
        assert!(dir.path().join("wip.txt").exists(), "back on disk");
        assert!(app.pending_reload);
        let follow_up = std::mem::take(&mut app.pending_history);
        assert_eq!(follow_up.last(), Some(&history::Want::Stashes));
        history_want(&mut app, &mut source, history::Want::Stashes, &mut None);
        assert!(app.history.as_ref().expect("open").commits.is_empty());
    }

    #[test]
    fn stashing_untracked_files_lists_them_in_the_stash() {
        let (dir, mut app, mut source) = one_commit_repo();
        app.history = Some(Box::new(history::History::new(history::Kind::Stash).0));
        let push = zdiff_core::StashPush {
            untracked: true,
            ..zdiff_core::StashPush::default()
        };
        history_want(
            &mut app,
            &mut source,
            history::Want::StashPush(push),
            &mut None,
        );
        assert!(app.toast.as_ref().is_some_and(|t| t.text == "✓ stashed"));
        assert!(!dir.path().join("wip.txt").exists(), "stashed away");
        // The follow-up lists the stash, which asks for its files.
        while !app.pending_history.is_empty() {
            for want in std::mem::take(&mut app.pending_history) {
                history_want(&mut app, &mut source, want, &mut None);
            }
        }
        let history = app.history.as_ref().expect("open");
        let files: Vec<_> = (history.files.iter())
            .map(|f| (f.path.clone(), f.status))
            .collect();
        assert_eq!(
            files,
            [(PathBuf::from("wip.txt"), zdiff_core::Status::Untracked)]
        );
    }

    #[test]
    fn an_expired_toast_wakes_the_loop_and_is_cleared() {
        let (tx, rx) = mpsc::channel();
        let mut app = App::new(Vec::new());
        app.toast = Some(app::Toast {
            text: "Copied a1b2c3d".into(),
            until: Instant::now(),
        });
        assert!(
            wait(&mut app, &rx).is_none(),
            "no message, but the toast is due"
        );
        assert!(app.toast.is_none());
        tx.send(Msg::SearchDone { generation: 0 }).unwrap();
        assert!(matches!(
            wait(&mut app, &rx),
            Some(Ok(Msg::SearchDone { .. }))
        ));
    }
}

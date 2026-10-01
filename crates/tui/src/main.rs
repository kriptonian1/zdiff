mod app;
mod clipboard;
mod find;
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
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use clap::Parser;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use ratatui::DefaultTerminal;
use ratatui_image::picker::{Picker, ProtocolType};
use zdiff_core::{Repo, Spec};

use anyhow::anyhow;
use app::{App, Apply, Focus, Notice};
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
        dirty = match rx.recv() {
            Ok(Msg::Term(event)) => app.handle(&event?),
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
            if let Ok(next) = Snapshot::load(repo, spec, Some(snapshot), &Touched::All, only) {
                let _ = tx.send(Msg::Refreshed {
                    snapshot: next,
                    touched: Touched::All,
                });
            }
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
}

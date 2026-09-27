mod app;
mod snapshot;
mod text;
mod tree;
mod ui;
mod watch;

use std::io;
use std::panic;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use clap::Parser;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use crossterm::execute;
use ratatui::DefaultTerminal;
use zdiff_core::{Repo, Spec};

use app::App;
use snapshot::{Snapshot, Touched};

/// Terminal viewer for your current git changes.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Keep running and refresh the diff as files change.
    #[arg(short, long)]
    watch: bool,
}

/// Everything the UI thread reacts to, from the input and watch threads.
pub enum Msg {
    Term(io::Result<Event>),
    Refreshed {
        snapshot: Snapshot,
        touched: Touched,
    },
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let spec = Spec::default();
    let repo = Repo::discover(".")?;
    // ponytail: counts computed serially at startup; use a worker thread at 1000+ files.
    let snapshot = Snapshot::load(&repo, &spec, None, &Touched::All)?;
    let app = App::new(snapshot.entries());

    let (tx, rx) = mpsc::channel();
    if args.watch {
        watch::spawn(repo.workdir(), spec, snapshot.clone(), tx.clone())?;
    }

    // Runs after ratatui's own restore hook, so a crash also releases the mouse.
    let hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), DisableMouseCapture);
        hook(info);
    }));
    ratatui::run(|terminal| {
        execute!(io::stdout(), EnableMouseCapture)?;
        spawn_input(tx);
        let result = event_loop(terminal, app, &repo, snapshot, &rx);
        execute!(io::stdout(), DisableMouseCapture)?;
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
    repo: &Repo,
    mut snapshot: Snapshot,
    rx: &Receiver<Msg>,
) -> io::Result<()> {
    let mut dirty = true;
    let mut shown: Option<PathBuf> = None;
    // Whether the shown file changed on disk and its diff must reload in place.
    let mut stale = false;
    while !app.quit {
        let selected = app.selected_file().map(|f| (f.path.clone(), f.change));
        let moved = selected.as_ref().map(|(path, _)| path) != shown.as_ref();
        if moved || stale {
            // ponytail: diff loads on the UI thread; use a worker if big files stall input.
            let diff = selected
                .as_ref()
                .map(|&(_, change)| repo.diff(&snapshot.changes[change]));
            if moved {
                app.show(diff);
            } else {
                app.reload(diff);
            }
            shown = selected.map(|(path, _)| path);
            stale = false;
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
                snapshot = next;
                true
            }
            Err(_) => break,
        };
    }
    Ok(())
}

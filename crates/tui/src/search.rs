//! Global search worker: runs a query over changed files off the UI thread and streams hits.

use std::ops::Range;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use zdiff_core::{Change, Repo, Side};
use zdiff_highlight::Token;
use zdiff_search::{Finder, MAX_HITS};

use crate::Msg;

/// Bytes of the line kept before a match in a snippet.
const LEAD: usize = 40;
/// Longest snippet in bytes; the result row shows as much as fits.
const SNIPPET: usize = 160;

/// One search over `changes`, as `(index into the snapshot's changes, change)`.
pub struct Job {
    pub generation: u64,
    pub finder: Finder,
    pub changes: Vec<(usize, Change)>,
}

/// A hit plus a trimmed piece of its line; whole files never leave the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub side: Side,
    pub line: u32,
    pub changed: bool,
    pub snippet: Box<[u8]>,
    /// The match inside `snippet`.
    pub range: Range<u16>,
    /// Syntax tokens of `snippet`, offset from its start; empty for unsupported languages.
    pub tokens: Box<[Token]>,
}

/// Starts the worker with its own repo; send it [`Job`]s, a newer job replaces a running one.
pub fn spawn(workdir: &Path, tx: Sender<Msg>) -> anyhow::Result<Sender<Job>> {
    let repo = Repo::discover(workdir)?;
    let (jobs_tx, jobs) = mpsc::channel();
    thread::spawn(move || {
        let mut next = None;
        while let Some(job) = next.take().or_else(|| jobs.recv().ok()) {
            next = run(&repo, &job, &jobs, &tx);
        }
    });
    Ok(jobs_tx)
}

/// Runs `job`, returning a newer job that arrived meanwhile; stops early if the UI is gone.
fn run(repo: &Repo, job: &Job, jobs: &Receiver<Job>, tx: &Sender<Msg>) -> Option<Job> {
    let mut total = 0;
    for (index, change) in &job.changes {
        if let Some(newer) = jobs.try_iter().last() {
            return Some(newer);
        }
        // A file that vanished or can't be read is skipped, like git grep.
        let Ok(diff) = repo.diff(change) else {
            continue;
        };
        let language = zdiff_highlight::language(&change.path);
        // Each side is highlighted once, on its first hit, with whole-file context.
        // ponytail: whole file per side even for one hit; highlight a window if huge files lag.
        let mut sides: [Option<Box<[Token]>>; 2] = [None, None];
        let hits: Vec<Found> = (job.finder.find(&diff).into_iter())
            .take(MAX_HITS - total)
            .map(|hit| {
                let text = hit.side.pick(&diff.old, &diff.new);
                let line = text.line(hit.line);
                let (kept, range) = snippet(line, hit.range);
                let tokens = language.map_or_else(Box::default, |language| {
                    let side = &mut sides[hit.side.pick(0, 1)];
                    let all = side
                        .get_or_insert_with(|| zdiff_highlight::highlight(language, text.bytes()));
                    let start = text.line_start(hit.line);
                    clip(all, start + kept.start, start + kept.end)
                });
                Found {
                    side: hit.side,
                    line: hit.line,
                    changed: hit.changed,
                    snippet: line[kept.start as usize..kept.end as usize].into(),
                    range,
                    tokens,
                }
            })
            .collect();
        if hits.is_empty() {
            continue;
        }
        total += hits.len();
        let found = Msg::Found {
            generation: job.generation,
            change: *index,
            hits,
        };
        if tx.send(found).is_err() {
            return None;
        }
        if total == MAX_HITS {
            break;
        }
    }
    let _ = tx.send(Msg::SearchDone {
        generation: job.generation,
    });
    None
}

/// The part of `line` to show around `range`: up to [`SNIPPET`] bytes, starting at a char and
/// without leading blanks; returns it as a byte range of `line` plus the match inside it.
#[expect(
    clippy::cast_possible_truncation,
    reason = "offsets stay within `range`, which is already u32"
)]
fn snippet(line: &[u8], range: Range<u32>) -> (Range<u32>, Range<u16>) {
    let (start, end) = (range.start as usize, range.end as usize);
    let is_continuation = |b: u8| b & 0b1100_0000 == 0b1000_0000;
    let mut from = start.saturating_sub(LEAD);
    while from < start && (is_continuation(line[from]) || line[from].is_ascii_whitespace()) {
        from += 1;
    }
    let mut to = line.len().min(from + SNIPPET);
    while to > end && to < line.len() && is_continuation(line[to]) {
        to -= 1;
    }
    let offset = |i: usize| u16::try_from(i - from).unwrap_or(u16::MAX);
    (from as u32..to as u32, offset(start)..offset(end.min(to)))
}

/// The tokens overlapping file bytes `from..to`, clamped to it and offset from `from`.
fn clip(tokens: &[Token], from: u32, to: u32) -> Box<[Token]> {
    let first = tokens.partition_point(|t| t.end <= from);
    (tokens[first..].iter())
        .take_while(|t| t.start < to)
        .map(|t| Token {
            start: t.start.max(from) - from,
            end: t.end.min(to) - from,
            class: t.class,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;
    use std::time::Duration;

    use zdiff_core::Spec;
    use zdiff_search::Query;

    use super::*;

    /// `snippet` applied: the kept bytes and the match inside them.
    fn cut(line: &[u8], range: Range<u32>) -> (&[u8], Range<u16>) {
        let (kept, found) = snippet(line, range);
        (&line[kept.start as usize..kept.end as usize], found)
    }

    #[test]
    fn clip_keeps_overlapping_tokens_relative_to_the_snippet() {
        use zdiff_highlight::Class;
        let token = |start, end| Token {
            start,
            end,
            class: Class::String,
        };
        let tokens = [token(0, 4), token(5, 12), token(14, 20), token(30, 40)];
        assert_eq!(
            *clip(&tokens, 8, 16),
            [token(0, 4), token(6, 8)],
            "clamped and shifted"
        );
        assert!(clip(&tokens, 21, 29).is_empty(), "nothing overlaps");
        assert!(clip(&[], 0, 10).is_empty());
    }

    #[test]
    fn snippet_trims_and_keeps_the_match() {
        let line = format!("    {}needle tail", "x".repeat(60));
        let (text, range) = cut(line.as_bytes(), 64..70);
        assert!(text.len() <= SNIPPET);
        assert_eq!(
            &text[usize::from(range.start)..usize::from(range.end)],
            b"needle"
        );
        assert_eq!(range.start, 40, "LEAD bytes of context");
        let (text, range) = cut(b"   needle", 3..9);
        assert_eq!(
            (text, range),
            (&b"needle"[..], 0..6),
            "leading blanks trimmed"
        );
        let (text, _) = cut("ééééééééééééééééééééééé needle".as_bytes(), 47..53);
        assert!(
            std::str::from_utf8(text).is_ok(),
            "starts on a char boundary"
        );
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .status()
            .expect("git runs")
            .success();
        assert!(ok, "git {args:?}");
    }

    fn job(repo: &Repo, generation: u64, text: &str) -> Job {
        let query = Query {
            text: text.into(),
            ..Query::default()
        };
        let changes = repo.changes(&Spec::default()).expect("changes");
        Job {
            generation,
            finder: query.compile().expect("valid"),
            changes: changes.into_iter().enumerate().collect(),
        }
    }

    #[test]
    fn a_job_streams_hits_per_file_then_done() {
        let dir = tempfile::tempdir().expect("temp dir");
        git(dir.path(), &["init", "-q"]);
        fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
        fs::write(dir.path().join("b.rs"), "fn b() {}\n").unwrap();
        fs::write(dir.path().join("c.txt"), "fn c\n").unwrap();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "c"]);
        fs::write(dir.path().join("a.rs"), "fn a() { needle }\n").unwrap();
        fs::write(dir.path().join("b.rs"), "fn b() { hay }\n").unwrap();
        fs::write(dir.path().join("c.txt"), "fn c needle\n").unwrap();

        let repo = Repo::discover(dir.path()).expect("repo");
        let (tx, rx) = mpsc::channel();
        let jobs = spawn(dir.path(), tx).expect("worker");
        jobs.send(job(&repo, 7, "needle")).unwrap();
        let recv = || rx.recv_timeout(Duration::from_secs(5)).expect("message");
        let Msg::Found {
            generation,
            change,
            hits,
        } = recv()
        else {
            panic!("expected hits in a.rs");
        };
        assert_eq!(
            (generation, change, hits.len()),
            (7, 0, 1),
            "b.rs has no match"
        );
        assert!(hits[0].changed && hits[0].side == Side::New);
        let fn_keyword = hits[0].tokens.first().map(|t| (t.start, t.end, t.class));
        assert_eq!(
            fn_keyword,
            Some((0, 2, zdiff_highlight::Class::Keyword)),
            "whole-file highlight, clipped"
        );
        let Msg::Found { change, hits, .. } = recv() else {
            panic!("expected hits in c.txt");
        };
        assert_eq!(change, 2);
        assert!(hits[0].tokens.is_empty(), "plain text has no tokens");
        assert!(matches!(recv(), Msg::SearchDone { generation: 7 }));

        // A newer job waiting in the queue replaces this one before any file is read.
        let (tx, rx) = mpsc::channel();
        let (queue, pending) = mpsc::channel();
        queue.send(job(&repo, 9, "hay")).unwrap();
        let newer = run(&repo, &job(&repo, 8, "needle"), &pending, &tx);
        assert_eq!(newer.map(|j| j.generation), Some(9));
        assert!(rx.try_recv().is_err(), "the replaced job sent nothing");
    }
}

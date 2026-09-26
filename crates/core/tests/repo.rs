use std::fs;
use std::path::Path;
use std::process::Command;

use zdiff_core::{Change, Error, Hunk, Repo, Spec, Status};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
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
        .output()
        .expect("git is installed");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .expect("git prints utf-8")
        .trim()
        .to_owned()
}

fn init() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    git(dir.path(), &["init", "-q"]);
    dir
}

fn commit(dir: &Path) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "c"]);
    git(dir, &["rev-parse", "HEAD"])
}

fn summary(changes: &[Change]) -> Vec<(String, Status)> {
    changes
        .iter()
        .map(|c| (c.path.display().to_string(), c.status()))
        .collect()
}

fn list(repo: &Repo, spec: &Spec) -> Vec<(String, Status)> {
    summary(&repo.changes(spec).expect("changes"))
}

#[test]
fn unborn_head_lists_untracked_as_added() {
    let dir = init();
    fs::write(dir.path().join("a.txt"), "hi\n").unwrap();
    fs::write(dir.path().join(".gitignore"), "ignored\n").unwrap();
    fs::write(dir.path().join("ignored"), "x").unwrap();

    let repo = Repo::discover(dir.path()).unwrap();
    let changes = repo.changes(&Spec::default()).unwrap();
    assert_eq!(
        summary(&changes),
        [
            (".gitignore".into(), Status::Added),
            ("a.txt".into(), Status::Added)
        ]
    );
    let diff = repo.diff(&changes[1]).unwrap();
    assert_eq!(diff.new.bytes(), b"hi\n");
    assert_eq!(diff.hunks.len(), 1);
}

#[test]
fn modified_and_deleted_against_head() {
    let dir = init();
    fs::write(dir.path().join("keep.txt"), "1\n2\n3\n").unwrap();
    fs::write(dir.path().join("gone.txt"), "x\n").unwrap();
    commit(dir.path());
    fs::write(dir.path().join("keep.txt"), "1\nTWO\n3\n").unwrap();
    fs::remove_file(dir.path().join("gone.txt")).unwrap();

    let repo = Repo::discover(dir.path()).unwrap();
    let changes = repo.changes(&Spec::default()).unwrap();
    assert_eq!(
        summary(&changes),
        [
            ("gone.txt".into(), Status::Deleted),
            ("keep.txt".into(), Status::Modified)
        ]
    );
    let hunks = repo.diff(&changes[1]).unwrap().hunks;
    assert_eq!(
        *hunks,
        [Hunk {
            old: 1..2,
            new: 1..2
        }]
    );
}

#[test]
fn staged_then_reverted_is_not_a_staged_change() {
    let dir = init();
    fs::write(dir.path().join("f.txt"), "a\n").unwrap();
    commit(dir.path());
    fs::write(dir.path().join("f.txt"), "b\n").unwrap();
    git(dir.path(), &["add", "f.txt"]);
    let repo = Repo::discover(dir.path()).unwrap();
    assert_eq!(
        list(&repo, &Spec::Staged("HEAD".into())),
        [("f.txt".into(), Status::Modified)]
    );

    fs::write(dir.path().join("f.txt"), "a\n").unwrap();
    git(dir.path(), &["add", "f.txt"]);
    let repo = Repo::discover(dir.path()).unwrap();
    assert!(list(&repo, &Spec::Staged("HEAD".into())).is_empty());
    assert!(list(&repo, &Spec::Unstaged).is_empty());
}

#[test]
fn between_two_revisions() {
    let dir = init();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/a.rs"), "a\n").unwrap();
    let c1 = commit(dir.path());
    fs::write(dir.path().join("src/a.rs"), "b\n").unwrap();
    fs::write(dir.path().join("new.rs"), "n\n").unwrap();
    let c2 = commit(dir.path());

    let repo = Repo::discover(dir.path()).unwrap();
    assert_eq!(
        list(&repo, &Spec::Revs(c1, c2)),
        [
            ("new.rs".into(), Status::Added),
            ("src/a.rs".into(), Status::Modified)
        ]
    );
}

#[test]
fn open_repo_sees_later_index_changes() {
    let dir = init();
    fs::write(dir.path().join("f.txt"), "a\n").unwrap();
    commit(dir.path());
    let repo = Repo::discover(dir.path()).unwrap();
    let staged = Spec::Staged("HEAD".into());
    assert!(list(&repo, &staged).is_empty());

    fs::write(dir.path().join("f.txt"), "b\n").unwrap();
    git(dir.path(), &["add", "f.txt"]);
    assert_eq!(list(&repo, &staged), [("f.txt".into(), Status::Modified)]);

    commit(dir.path());
    assert!(list(&repo, &staged).is_empty(), "new HEAD is picked up");
}

#[test]
fn non_repo_is_a_discover_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        Repo::discover(dir.path()),
        Err(Error::Discover { .. })
    ));
}

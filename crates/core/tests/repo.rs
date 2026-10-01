use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use zdiff_core::{Change, Error, Hunk, Patch, Repo, Spec, Staged, Status};

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
fn unborn_head_lists_untracked_files() {
    let dir = init();
    fs::write(dir.path().join("a.txt"), "hi\n").unwrap();
    fs::write(dir.path().join(".gitignore"), "ignored\n").unwrap();
    fs::write(dir.path().join("ignored"), "x").unwrap();

    let repo = Repo::discover(dir.path()).unwrap();
    let changes = repo.changes(&Spec::default()).unwrap();
    assert_eq!(
        summary(&changes),
        [
            (".gitignore".into(), Status::Untracked),
            ("a.txt".into(), Status::Untracked)
        ]
    );
    let diff = repo.diff(&changes[1]).unwrap();
    assert_eq!(diff.new.bytes(), b"hi\n");
    assert_eq!(diff.hunks.len(), 1);
}

#[test]
fn staging_an_untracked_file_makes_it_added() {
    let dir = init();
    fs::write(dir.path().join("a.txt"), "a\n").unwrap();
    commit(dir.path());
    fs::write(dir.path().join("new.rs"), "n\n").unwrap();

    let repo = Repo::discover(dir.path()).unwrap();
    let untracked = [("new.rs".to_owned(), Status::Untracked)];
    assert_eq!(list(&repo, &Spec::default()), untracked);
    assert_eq!(list(&repo, &Spec::Unstaged), untracked);

    git(dir.path(), &["add", "new.rs"]);
    let repo = Repo::discover(dir.path()).unwrap();
    assert_eq!(
        list(&repo, &Spec::default()),
        [("new.rs".to_owned(), Status::Added)]
    );
    assert_eq!(list(&repo, &Spec::Unstaged), []);
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
fn head_name_is_the_branch_or_the_short_id_when_detached() {
    let dir = init();
    let repo = Repo::discover(dir.path()).unwrap();
    let branch = git(dir.path(), &["symbolic-ref", "--short", "HEAD"]);
    assert_eq!(repo.head_name(), branch, "unborn branch");
    fs::write(dir.path().join("a"), "a\n").unwrap();
    let id = commit(dir.path());
    git(dir.path(), &["checkout", "-q", "--detach"]);
    assert_eq!(repo.head_name(), id[..7]);
}

#[test]
fn spec_reads_as_old_then_new() {
    let cases = [
        (Spec::default(), "HEAD → worktree"),
        (Spec::Staged("HEAD".into()), "HEAD → index"),
        (Spec::Unstaged, "index → worktree"),
        (Spec::Revs("main".into(), "dev".into()), "main → dev"),
    ];
    for (spec, text) in cases {
        assert_eq!(spec.to_string(), text);
    }
}

#[test]
fn workdir_is_absolute_when_discovered_from_a_relative_path() {
    // Tests run in the crate dir, two levels below the repo root.
    let repo = Repo::discover(".").unwrap();
    assert!(repo.workdir().is_absolute(), "{:?}", repo.workdir());
    assert!(repo.workdir().file_name().is_some(), "names the folder");
}

#[test]
fn non_repo_is_a_discover_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        Repo::discover(dir.path()),
        Err(Error::Discover { .. })
    ));
}

/// `git status --porcelain`, untrimmed: its first column is often a space.
fn status(dir: &Path) -> String {
    let out = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(dir)
        .output()
        .expect("git is installed");
    String::from_utf8(out.stdout)
        .expect("git prints utf-8")
        .trim_end()
        .to_owned()
}

/// Saves an identity in the repo's own config: gix reads it there, not from `-c` flags.
fn configure(dir: &Path) {
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["config", "user.email", "t@t"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

fn staged(repo: &Repo) -> Vec<(String, Staged)> {
    (repo.changes(&Spec::default()).expect("changes").iter())
        .map(|c| (c.path.display().to_string(), c.staged()))
        .collect()
}

fn paths(names: &[&str]) -> Vec<PathBuf> {
    names.iter().map(PathBuf::from).collect()
}

#[test]
fn staged_state_is_no_partly_or_fully() {
    let dir = init();
    for name in ["full", "part", "edit"] {
        fs::write(dir.path().join(name), "1\n").unwrap();
    }
    commit(dir.path());
    fs::write(dir.path().join("full"), "2\n").unwrap();
    fs::write(dir.path().join("part"), "2\n").unwrap();
    git(dir.path(), &["add", "full", "part"]);
    fs::write(dir.path().join("part"), "3\n").unwrap();
    fs::write(dir.path().join("edit"), "2\n").unwrap();
    fs::write(dir.path().join("new"), "1\n").unwrap();

    let repo = Repo::discover(dir.path()).unwrap();
    assert_eq!(
        staged(&repo),
        [
            ("edit".into(), Staged::No),
            ("full".into(), Staged::Fully),
            ("new".into(), Staged::No),
            ("part".into(), Staged::Partly),
        ]
    );
    let unstaged = repo.changes(&Spec::Unstaged).unwrap();
    assert!(
        unstaged.iter().all(|c| c.staged() == Staged::No),
        "only against HEAD"
    );
}

#[test]
fn apply_stages_new_modified_deleted_executable_and_symlink() {
    let dir = init();
    fs::write(dir.path().join("mod"), "1\n").unwrap();
    fs::write(dir.path().join("gone"), "1\n").unwrap();
    commit(dir.path());
    fs::write(dir.path().join("mod"), "2\n").unwrap();
    fs::remove_file(dir.path().join("gone")).unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join("sub/new"), "new\n").unwrap();
    let run = dir.path().join("run.sh");
    fs::write(&run, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&run, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("mod", dir.path().join("link")).unwrap();

    let repo = Repo::discover(dir.path()).unwrap();
    let all = paths(&["gone", "link", "mod", "run.sh", "sub/new"]);
    repo.apply(&all, &[]).unwrap();

    assert_eq!(
        git(dir.path(), &["diff", "--cached", "--name-status"]),
        "D\tgone\nA\tlink\nM\tmod\nA\trun.sh\nA\tsub/new"
    );
    let modes = git(dir.path(), &["ls-files", "-s", "link", "run.sh"]);
    assert!(
        modes.starts_with("120000 ") && modes.contains("\n100755 "),
        "{modes}"
    );
    assert_eq!(
        status(dir.path()),
        "D  gone\nA  link\nM  mod\nA  run.sh\nA  sub/new"
    );
    assert!(staged(&repo).iter().all(|(_, s)| *s == Staged::Fully));
}

#[test]
fn apply_runs_the_crlf_filter() {
    let dir = init();
    git(dir.path(), &["config", "core.autocrlf", "true"]);
    fs::write(dir.path().join("a.txt"), "a\r\n").unwrap();

    let repo = Repo::discover(dir.path()).unwrap();
    repo.apply(&paths(&["a.txt"]), &[]).unwrap();
    assert_eq!(git(dir.path(), &["show", ":a.txt"]), "a");
    assert_eq!(
        git(dir.path(), &["cat-file", "-s", ":a.txt"]),
        "2",
        "stored as a\\n"
    );
}

#[test]
fn apply_unstages_back_to_head() {
    let dir = init();
    fs::write(dir.path().join("mod"), "1\n").unwrap();
    fs::write(dir.path().join("gone"), "1\n").unwrap();
    commit(dir.path());
    fs::write(dir.path().join("mod"), "2\n").unwrap();
    fs::write(dir.path().join("new"), "1\n").unwrap();
    git(dir.path(), &["add", "mod", "new"]);
    git(dir.path(), &["rm", "-q", "gone"]);

    let repo = Repo::discover(dir.path()).unwrap();
    repo.apply(&[], &paths(&["gone", "mod", "new"])).unwrap();
    assert_eq!(git(dir.path(), &["diff", "--cached", "--name-only"]), "");
    assert_eq!(status(dir.path()), " D gone\n M mod\n?? new");
}

#[test]
fn commit_records_the_index_on_head() {
    let dir = init();
    configure(dir.path());
    fs::write(dir.path().join("a"), "1\n").unwrap();
    fs::write(dir.path().join("b"), "1\n").unwrap();
    let before = commit(dir.path());
    fs::write(dir.path().join("a"), "2\n").unwrap();
    fs::write(dir.path().join("b"), "2\n").unwrap();

    let repo = Repo::discover(dir.path()).unwrap();
    repo.apply(&paths(&["a"]), &[]).unwrap();
    let id = repo.commit("feat: change a\n\nbody").unwrap();

    assert_eq!(id, git(dir.path(), &["rev-parse", "--short=7", "HEAD"]));
    assert_eq!(
        git(dir.path(), &["log", "-1", "--format=%s|%b|%P"]),
        format!("feat: change a|body\n|{before}")
    );
    assert_eq!(
        git(dir.path(), &["diff", "HEAD~1", "HEAD", "--name-only"]),
        "a"
    );
    assert_eq!(status(dir.path()), " M b", "b stays unstaged");
}

#[test]
fn first_commit_on_an_unborn_branch() {
    let dir = init();
    configure(dir.path());
    fs::write(dir.path().join("a"), "1\n").unwrap();

    let repo = Repo::discover(dir.path()).unwrap();
    repo.apply(&paths(&["a"]), &[]).unwrap();
    repo.commit("first").unwrap();
    assert_eq!(git(dir.path(), &["log", "--format=%s|%P"]), "first|");
    assert_eq!(git(dir.path(), &["ls-tree", "--name-only", "HEAD"]), "a");
}

#[test]
fn commit_refuses_nothing_signing_and_conflicts() {
    let refused = |result: Result<String, Error>| match result {
        Err(Error::Refused(why)) => why,
        other => panic!("expected a refusal, got {other:?}"),
    };
    let dir = init();
    configure(dir.path());
    fs::write(dir.path().join("a"), "1\n").unwrap();
    commit(dir.path());
    let repo = Repo::discover(dir.path()).unwrap();
    assert_eq!(refused(repo.commit(" \n\n")), "the commit message is empty");
    assert_eq!(refused(repo.commit("m")), "nothing to commit");

    git(dir.path(), &["config", "commit.gpgsign", "true"]);
    let repo = Repo::discover(dir.path()).unwrap();
    assert!(refused(repo.commit("m")).contains("signing"));
    git(dir.path(), &["config", "commit.gpgsign", "false"]);

    git(dir.path(), &["checkout", "-q", "-b", "other"]);
    fs::write(dir.path().join("a"), "other\n").unwrap();
    commit(dir.path());
    git(dir.path(), &["checkout", "-q", "-"]);
    fs::write(dir.path().join("a"), "main\n").unwrap();
    commit(dir.path());
    let merge = Command::new("git")
        .args(["merge", "-q", "other"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!merge.status.success(), "the merge conflicts");
    let repo = Repo::discover(dir.path()).unwrap();
    assert_eq!(refused(repo.commit("m")), "resolve conflicts first");
}

#[test]
fn apply_fails_cleanly_while_the_index_is_locked() {
    let dir = init();
    fs::write(dir.path().join("a"), "1\n").unwrap();
    commit(dir.path());
    fs::write(dir.path().join("a"), "2\n").unwrap();
    fs::write(dir.path().join(".git/index.lock"), "").unwrap();

    let repo = Repo::discover(dir.path()).unwrap();
    assert!(matches!(
        repo.apply(&paths(&["a"]), &[]),
        Err(Error::Git(_))
    ));
    assert_eq!(status(dir.path()), " M a", "index untouched");
}

fn numbered(n: u32) -> String {
    (1..=n).fold(String::new(), |mut out, i| {
        writeln!(out, "line {i}").expect("writing to a String cannot fail");
        out
    })
}

fn patch_of(dir: &Path, args: &[&str]) -> Patch {
    Patch::parse(git(dir, args).into_bytes()).expect("git's own diff parses")
}

#[test]
fn a_patch_with_both_blobs_in_the_repo_shows_the_exact_files() {
    let dir = init();
    fs::write(dir.path().join("a.txt"), numbered(30)).unwrap();
    commit(dir.path());
    let v2 = numbered(30).replace("line 15\n", "fifteen\n");
    fs::write(dir.path().join("a.txt"), &v2).unwrap();
    commit(dir.path());
    let patch = patch_of(dir.path(), &["diff", "HEAD~1", "HEAD"]);
    let repo = Repo::discover(dir.path()).unwrap();

    let diff = patch.full_diff(0, Some(&repo));
    assert!(diff.known.is_none(), "whole files, no gaps");
    assert_eq!(diff.new.bytes(), v2.as_bytes());
    assert_eq!(diff.old.len(), 30);
    assert!(
        patch.diff(0).known.is_some(),
        "the hunks-only view is still there"
    );
}

#[test]
fn the_old_blob_plus_the_hunks_rebuild_the_new_file() {
    let dir = init();
    fs::write(dir.path().join("b.txt"), "keep\nold").unwrap();
    commit(dir.path());
    // `git diff` against the worktree writes no blob for the new side.
    fs::write(dir.path().join("b.txt"), "keep\nnew").unwrap();
    let patch = patch_of(dir.path(), &["diff"]);
    fs::write(dir.path().join("b.txt"), "something else entirely\n").unwrap();
    let repo = Repo::discover(dir.path()).unwrap();

    let diff = patch.full_diff(0, Some(&repo));
    assert!(diff.known.is_none());
    assert_eq!(
        diff.new.bytes(),
        b"keep\nnew",
        "no final newline, as in the patch"
    );
}

#[test]
fn a_plain_patch_applies_to_the_worktree_or_falls_back() {
    let dir = init();
    fs::write(dir.path().join("c.txt"), "1\n2\n3\n").unwrap();
    let patch = Patch::parse(b"--- a/c.txt\n+++ b/c.txt\n@@ -2 +2 @@\n-2\n+two\n".to_vec())
        .expect("parses");
    let repo = Repo::discover(dir.path()).unwrap();
    let diff = patch.full_diff(0, Some(&repo));
    assert!(diff.known.is_none());
    assert_eq!(diff.new.bytes(), b"1\ntwo\n3\n");

    fs::write(dir.path().join("c.txt"), "1\nTWO\n3\n").unwrap();
    let diff = patch.full_diff(0, Some(&repo));
    assert!(
        diff.known.is_some(),
        "other content: hunks only, never wrong text"
    );
}

#[test]
fn an_added_file_needs_no_base() {
    let dir = init();
    // `1234567` isn't in this repo, so only the empty old side plus the hunk can rebuild it.
    let patch = Patch::parse(
        b"diff --git a/n.txt b/n.txt\nnew file mode 100644\nindex 0000000..1234567\n\
          --- /dev/null\n+++ b/n.txt\n@@ -0,0 +1,2 @@\n+one\n+two\n"
            .to_vec(),
    )
    .expect("parses");
    let repo = Repo::discover(dir.path()).unwrap();
    let diff = patch.full_diff(0, Some(&repo));
    assert!(diff.known.is_none());
    assert_eq!(diff.new.bytes(), b"one\ntwo\n");
}

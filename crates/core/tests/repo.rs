use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use zdiff_core::{
    Change, Error, Hunk, Patch, RefKind, Repo, Spec, Staged, StashOp, StashPush, Status,
};

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
fn a_worktree_edited_back_to_head_is_no_change() {
    let dir = init();
    fs::write(dir.path().join("f.txt"), "a\n").unwrap();
    commit(dir.path());
    fs::write(dir.path().join("f.txt"), "b\n").unwrap();
    git(dir.path(), &["add", "f.txt"]);
    fs::write(dir.path().join("f.txt"), "a\n").unwrap();
    let repo = Repo::discover(dir.path()).unwrap();
    assert!(list(&repo, &Spec::default()).is_empty(), "HEAD → worktree");
    assert_eq!(
        list(&repo, &Spec::Unstaged),
        [("f.txt".into(), Status::Modified)]
    );
}

#[test]
fn renames_show_once_with_where_they_came_from() {
    let dir = init();
    let body = "one\ntwo\nthree\nfour\nfive\n";
    fs::write(dir.path().join("old.txt"), body).unwrap();
    let c1 = commit(dir.path());
    git(dir.path(), &["mv", "old.txt", "new.txt"]);
    let repo = Repo::discover(dir.path()).unwrap();
    for spec in [Spec::default(), Spec::Staged("HEAD".into())] {
        let changes = repo.changes(&spec).unwrap();
        assert_eq!(summary(&changes), [("new.txt".into(), Status::Renamed)]);
        assert_eq!(changes[0].from.as_deref(), Some(Path::new("old.txt")));
        assert_eq!(
            changes[0].staged(),
            if spec == Spec::default() {
                Staged::Fully
            } else {
                Staged::No
            }
        );
        assert!(
            repo.diff(&changes[0]).unwrap().hunks.is_empty(),
            "same content"
        );
    }

    fs::write(dir.path().join("new.txt"), format!("{body}six\n")).unwrap();
    let c2 = commit(dir.path());
    let changes = repo.changes(&Spec::Revs(c1, c2)).unwrap();
    assert_eq!(summary(&changes), [("new.txt".into(), Status::Renamed)]);
    assert_eq!(repo.diff(&changes[0]).unwrap().stats(), (1, 0));
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
fn merge_base_is_where_a_branch_split_off() {
    let dir = init();
    fs::write(dir.path().join("a.txt"), "a\n").unwrap();
    let fork = commit(dir.path());
    git(dir.path(), &["switch", "-q", "-c", "feature"]);
    fs::write(dir.path().join("feature.txt"), "f\n").unwrap();
    commit(dir.path());
    git(dir.path(), &["switch", "-q", "-"]);
    fs::write(dir.path().join("main.txt"), "m\n").unwrap();
    commit(dir.path());

    let repo = Repo::discover(dir.path()).unwrap();
    let base = repo
        .merge_base("HEAD", "feature")
        .expect("they share a commit");
    assert_eq!(base, fork);
    assert_eq!(
        list(&repo, &Spec::Revs(base, "feature".into())),
        [("feature.txt".into(), Status::Added)],
        "only the branch's own change, not main's"
    );

    git(dir.path(), &["switch", "-q", "--orphan", "lonely"]);
    git(dir.path(), &["commit", "-q", "--allow-empty", "-m", "x"]);
    assert!(matches!(
        repo.merge_base("lonely", "feature"),
        Err(Error::Refused(_))
    ));
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
fn apply_keeps_the_tree_cache_valid_for_git() {
    let dir = init();
    fs::create_dir_all(dir.path().join("src/deep")).unwrap();
    fs::write(dir.path().join("src/deep/a"), "1\n").unwrap();
    fs::write(dir.path().join("src/b"), "1\n").unwrap();
    fs::write(dir.path().join("top"), "1\n").unwrap();
    commit(dir.path());
    // A fresh write-tree fills every cached tree.
    git(dir.path(), &["write-tree"]);
    fs::write(dir.path().join("src/deep/a"), "2\n").unwrap();

    let repo = Repo::discover(dir.path()).unwrap();
    repo.apply(&paths(&["src/deep/a"]), &[]).unwrap();
    let tree = git(dir.path(), &["write-tree"]);
    assert_eq!(
        git(dir.path(), &["rev-parse", &format!("{tree}:src/deep/a")]),
        git(dir.path(), &["rev-parse", ":src/deep/a"]),
        "git rebuilt the stale trees from the index"
    );
    assert_eq!(
        git(dir.path(), &["rev-parse", &format!("{tree}:src/b")]),
        git(dir.path(), &["rev-parse", "HEAD:src/b"])
    );
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

/// Commits everything with message `message`, dated `seconds` after a fixed start so the
/// history's order doesn't depend on how fast the test runs.
fn commit_at(dir: &Path, message: &str, seconds: u32) -> String {
    git(dir, &["add", "-A"]);
    let date = format!("{} +0000", 1_700_000_000 + seconds);
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(["commit", "-q", "--allow-empty", "-m", message])
        .env("GIT_COMMITTER_DATE", &date)
        .env("GIT_AUTHOR_DATE", &date)
        .current_dir(dir)
        .status()
        .expect("git is installed");
    assert!(out.success(), "git commit {message}");
    git(dir, &["rev-parse", "HEAD"])
}

#[test]
fn log_lists_commits_newest_first_with_their_names() {
    let dir = init();
    assert!(
        Repo::discover(dir.path())
            .unwrap()
            .log(10)
            .unwrap()
            .is_empty(),
        "no commits yet"
    );
    fs::write(dir.path().join("a.txt"), "1\n").unwrap();
    let first = commit_at(dir.path(), "first\n\nmore words", 0);
    git(dir.path(), &["tag", "v0.1"]);
    fs::write(dir.path().join("a.txt"), "2\n").unwrap();
    let second = commit_at(dir.path(), "second", 10);
    git(dir.path(), &["branch", "-m", "main"]);

    let repo = Repo::discover(dir.path()).unwrap();
    let log = repo.log(10).unwrap();
    let ids: Vec<_> = log.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, [second.as_str(), first.as_str()]);
    assert!(log[0].head && !log[1].head);
    assert_eq!(&*log[0].refs, ["main"]);
    assert_eq!(&*log[1].refs, ["v0.1"]);
    assert_eq!(
        (log[1].summary.as_str(), log[1].author.as_str()),
        ("first", "t")
    );
    assert_eq!(log[1].message, "first\n\nmore words");
    assert_eq!(log[0].time - log[1].time, 10);
    assert_eq!(&*log[0].parents, std::slice::from_ref(&first));
    assert!(log[1].parents.is_empty());
    assert_eq!(repo.log(1).unwrap().len(), 1, "the limit");
}

#[test]
fn log_walks_other_branches_too() {
    let dir = init();
    fs::write(dir.path().join("a.txt"), "1\n").unwrap();
    commit_at(dir.path(), "base", 0);
    git(dir.path(), &["checkout", "-q", "-b", "side"]);
    fs::write(dir.path().join("b.txt"), "1\n").unwrap();
    let side = commit_at(dir.path(), "side work", 5);
    git(dir.path(), &["checkout", "-q", "-"]);
    let log = Repo::discover(dir.path()).unwrap().log(10).unwrap();
    assert!(
        log.iter().any(|c| c.id == side && *c.refs == ["side"]),
        "{log:#?}"
    );
}

#[test]
fn a_history_pages_on_from_where_it_stopped() {
    let dir = init();
    for n in 0..5 {
        fs::write(dir.path().join("a.txt"), format!("{n}\n")).unwrap();
        commit_at(dir.path(), &format!("c{n}"), n);
    }
    let repo = Repo::discover(dir.path()).unwrap();
    let mut history = repo.history().unwrap();
    let mut paged = history.next(&repo, 2).unwrap();
    paged.extend(history.next(&repo, 2).unwrap());
    paged.extend(history.next(&repo, 2).unwrap());
    assert_eq!(paged, repo.log(10).unwrap());
    assert!(history.next(&repo, 2).unwrap().is_empty());
}

#[test]
fn commit_spec_shows_a_commit_against_its_first_parent() {
    let dir = init();
    fs::write(dir.path().join("a.txt"), "1\n").unwrap();
    fs::write(dir.path().join("b.txt"), "1\n").unwrap();
    commit_at(dir.path(), "root", 0);
    fs::write(dir.path().join("a.txt"), "2\n").unwrap();
    commit_at(dir.path(), "edit a", 5);
    git(dir.path(), &["checkout", "-q", "-b", "side", "HEAD~1"]);
    fs::write(dir.path().join("c.txt"), "1\n").unwrap();
    commit_at(dir.path(), "add c", 6);
    git(dir.path(), &["checkout", "-q", "-"]);
    git(
        dir.path(),
        &["merge", "-q", "--no-ff", "-m", "merge side", "side"],
    );

    let repo = Repo::discover(dir.path()).unwrap();
    let log = repo.log(10).unwrap();
    let find = |summary: &str| log.iter().find(|c| c.summary == summary).expect(summary);
    let files = |summary: &str| list(&repo, &repo.commit_spec(find(summary)));
    assert_eq!(files("edit a"), [("a.txt".into(), Status::Modified)]);
    assert_eq!(
        files("root"),
        [
            ("a.txt".into(), Status::Added),
            ("b.txt".into(), Status::Added)
        ],
        "against the empty tree"
    );
    assert_eq!(
        files("merge side"),
        [("c.txt".into(), Status::Added)],
        "first parent"
    );
}

/// A repo with `a.txt` committed, then `a.txt` edited and stashed twice: `first`, then
/// `second` (stash@{0}).
fn stashed() -> (tempfile::TempDir, Repo) {
    let dir = init();
    fs::write(dir.path().join("a.txt"), "1\n").expect("write");
    commit(dir.path());
    for message in ["first", "second"] {
        fs::write(dir.path().join("a.txt"), format!("{message}\n")).expect("write");
        git(dir.path(), &["stash", "push", "-q", "-m", message]);
    }
    let repo = Repo::discover(dir.path()).expect("a repo");
    (dir, repo)
}

/// Each stash's message, without git's `On <branch>: ` prefix.
fn summaries(repo: &Repo) -> Vec<String> {
    let stashes = repo.stashes().expect("stashes");
    let message = |s: &str| s.split_once(": ").map_or(s, |(_, m)| m).to_owned();
    stashes.iter().map(|s| message(&s.summary)).collect()
}

#[test]
fn no_stash_lists_nothing() {
    let dir = init();
    fs::write(dir.path().join("a.txt"), "1\n").expect("write");
    commit(dir.path());
    assert!(
        Repo::discover(dir.path())
            .expect("a repo")
            .stashes()
            .expect("stashes")
            .is_empty()
    );
}

#[test]
fn stashes_list_newest_first_against_their_base() {
    let (dir, repo) = stashed();
    let stashes = repo.stashes().expect("stashes");
    assert_eq!(summaries(&repo), ["second", "first"]);
    assert_eq!(&*stashes[0].refs, ["stash@{0}"]);
    assert_eq!(&*stashes[1].refs, ["stash@{1}"]);
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    assert_eq!(&*stashes[0].parents, [head], "only the base");
    let files = list(&repo, &repo.commit_spec(&stashes[0]));
    assert_eq!(files, [("a.txt".to_owned(), Status::Modified)]);
}

#[test]
fn apply_keeps_the_stash_and_pop_removes_it() {
    let (dir, repo) = stashed();
    let read = || fs::read_to_string(dir.path().join("a.txt")).expect("read");
    let stashes = repo.stashes().expect("stashes");
    assert!(
        repo.stash(StashOp::Apply, &stashes[1].id)
            .expect("applies")
            .is_empty()
    );
    assert_eq!(read(), "first\n");
    assert_eq!(summaries(&repo).len(), 2);
    git(dir.path(), &["checkout", "--", "a.txt"]);
    assert!(
        repo.stash(StashOp::Pop, &stashes[0].id)
            .expect("runs")
            .is_empty()
    );
    assert_eq!(read(), "second\n");
    assert_eq!(summaries(&repo), ["first"]);
}

#[test]
fn drop_finds_its_stash_by_id_after_the_list_changes() {
    let (dir, repo) = stashed();
    let old = repo.stashes().expect("stashes");
    fs::write(dir.path().join("a.txt"), "third\n").expect("write");
    git(dir.path(), &["stash", "push", "-q", "-m", "third"]);
    repo.stash(StashOp::Drop, &old[0].id).expect("drops");
    assert_eq!(summaries(&repo), ["third", "first"]);
    let gone = repo.stash(StashOp::Drop, &old[0].id);
    assert!(matches!(gone, Err(Error::Refused(_))), "{gone:?}");
}

#[test]
fn a_conflicting_pop_reports_the_path_and_keeps_the_stash() {
    let (dir, repo) = stashed();
    fs::write(dir.path().join("a.txt"), "committed\n").expect("write");
    commit(dir.path());
    let stashes = repo.stashes().expect("stashes");
    let conflicts = repo.stash(StashOp::Pop, &stashes[0].id).expect("runs");
    assert_eq!(conflicts, [PathBuf::from("a.txt")]);
    assert_eq!(summaries(&repo).len(), 2, "kept");
}

#[test]
fn local_changes_in_the_way_are_a_command_error() {
    let (dir, repo) = stashed();
    fs::write(dir.path().join("a.txt"), "dirty\n").expect("write");
    let stashes = repo.stashes().expect("stashes");
    let error = repo
        .stash(StashOp::Apply, &stashes[0].id)
        .expect_err("refused by git");
    assert!(
        matches!(
            error,
            Error::Command {
                cmd: "stash apply",
                ..
            }
        ),
        "{error}"
    );
}

/// A repo with `a.txt` committed, then edited, and an untracked `new.txt`.
fn dirty() -> (tempfile::TempDir, Repo) {
    let dir = init();
    fs::write(dir.path().join("a.txt"), "1\n").expect("write");
    commit(dir.path());
    fs::write(dir.path().join("a.txt"), "2\n").expect("write");
    fs::write(dir.path().join("new.txt"), "fresh\n").expect("write");
    let repo = Repo::discover(dir.path()).expect("a repo");
    (dir, repo)
}

#[test]
fn a_stash_with_untracked_files_lists_and_previews_them() {
    let (dir, repo) = dirty();
    let push = StashPush {
        message: "wip".into(),
        untracked: true,
        ..StashPush::default()
    };
    assert!(repo.stash_push(&push).expect("stashes"));
    assert!(!dir.path().join("new.txt").exists());
    let stash = repo.stashes().expect("stashes").remove(0);
    assert!(stash.summary.ends_with(": wip"), "{}", stash.summary);
    let changes = repo.changes(&repo.commit_spec(&stash)).expect("changes");
    assert_eq!(
        summary(&changes),
        [
            ("a.txt".to_owned(), Status::Modified),
            ("new.txt".to_owned(), Status::Untracked)
        ]
    );
    let diff = repo.diff(&changes[1]).expect("diff");
    assert_eq!(diff.new.bytes(), b"fresh\n");
}

#[test]
fn pushing_nothing_says_so() {
    let dir = init();
    fs::write(dir.path().join("a.txt"), "1\n").expect("write");
    commit(dir.path());
    let repo = Repo::discover(dir.path()).expect("a repo");
    assert!(!repo.stash_push(&StashPush::default()).expect("runs"));
    assert!(repo.stashes().expect("stashes").is_empty());
}

#[test]
fn staged_only_and_keep_index_stash_what_they_say() {
    let (dir, repo) = dirty();
    git(dir.path(), &["add", "a.txt"]);
    let staged = StashPush {
        staged: true,
        ..StashPush::default()
    };
    assert!(repo.stash_push(&staged).expect("stashes"));
    assert_eq!(
        status(dir.path()),
        "?? new.txt",
        "only the staged file went"
    );

    git(dir.path(), &["stash", "pop", "-q", "--index"]);
    let keep = StashPush {
        keep_index: true,
        ..StashPush::default()
    };
    assert!(repo.stash_push(&keep).expect("stashes"));
    assert_eq!(
        status(dir.path()),
        "M  a.txt\n?? new.txt",
        "the index stays"
    );
}

#[test]
fn a_branch_from_a_stash_restores_it_and_drops_it() {
    let (dir, repo) = dirty();
    assert!(repo.stash_push(&StashPush::default()).expect("stashes"));
    let stash = repo.stashes().expect("stashes").remove(0);
    let refused = repo.stash_branch(&stash.id, "-x");
    assert!(matches!(refused, Err(Error::Refused(_))), "{refused:?}");
    assert!(
        repo.stash_branch(&stash.id, "try-it")
            .expect("branches")
            .is_empty()
    );
    assert_eq!(repo.head_name(), "try-it");
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).expect("read"),
        "2\n"
    );
    assert!(repo.stashes().expect("stashes").is_empty());
}

fn names(repo: &Repo) -> Vec<(String, RefKind, bool)> {
    let branches = repo.branches().expect("branches");
    (branches.into_iter())
        .map(|b| (b.name, b.kind, b.head))
        .collect()
}

#[test]
fn branches_list_locals_then_tags_with_head_marked() {
    let dir = init();
    fs::write(dir.path().join("a.txt"), "1\n").expect("write");
    commit(dir.path());
    git(dir.path(), &["branch", "-M", "main"]);
    git(dir.path(), &["tag", "v0.1"]);
    git(dir.path(), &["switch", "-q", "-c", "side"]);
    let repo = Repo::discover(dir.path()).expect("a repo");
    assert_eq!(
        names(&repo),
        [
            ("main".to_owned(), RefKind::Local, false),
            ("side".to_owned(), RefKind::Local, true),
            ("v0.1".to_owned(), RefKind::Tag, false),
        ]
    );
    git(dir.path(), &["switch", "-q", "--detach"]);
    let repo = Repo::discover(dir.path()).expect("a repo");
    assert_eq!(
        names(&repo)[0],
        ("(detached)".to_owned(), RefKind::Local, true)
    );
}

#[test]
fn upstreams_count_ahead_and_behind_and_notice_when_gone() {
    let origin = init();
    fs::write(origin.path().join("a.txt"), "1\n").expect("write");
    commit(origin.path());
    git(origin.path(), &["branch", "-M", "main"]);
    let clone = tempfile::tempdir().expect("temp dir");
    let from = origin.path().to_str().expect("utf-8 path");
    git(clone.path(), &["clone", "-q", from, "."]);
    for n in 2..4 {
        fs::write(clone.path().join("a.txt"), format!("{n}\n")).expect("write");
        commit(clone.path());
    }
    fs::write(origin.path().join("b.txt"), "theirs\n").expect("write");
    commit(origin.path());
    git(clone.path(), &["fetch", "-q"]);

    let repo = Repo::discover(clone.path()).expect("a repo");
    let branches = repo.branches().expect("branches");
    let main = &branches[0];
    assert_eq!((main.name.as_str(), main.kind), ("main", RefKind::Local));
    let up = main.upstream.as_ref().expect("an upstream");
    assert_eq!(
        (up.name.as_str(), up.ahead, up.behind, up.gone),
        ("origin/main", 2, 1, false)
    );
    assert!(
        branches
            .iter()
            .any(|b| b.name == "origin/main" && b.kind == RefKind::Remote),
        "remote listed"
    );
    assert!(!branches.iter().any(|b| b.name.ends_with("/HEAD")));

    git(
        clone.path(),
        &["update-ref", "-d", "refs/remotes/origin/main"],
    );
    let repo = Repo::discover(clone.path()).expect("a repo");
    let up = repo.branches().expect("branches")[0].upstream.clone();
    assert!(up.is_some_and(|u| u.gone));
}

#[test]
fn log_from_walks_only_from_its_tips() {
    let dir = init();
    fs::write(dir.path().join("a.txt"), "1\n").expect("write");
    let root = commit(dir.path());
    git(dir.path(), &["switch", "-q", "-c", "side"]);
    fs::write(dir.path().join("a.txt"), "side\n").expect("write");
    let side = commit(dir.path());
    git(dir.path(), &["switch", "-q", "-"]);
    fs::write(dir.path().join("b.txt"), "main\n").expect("write");
    commit(dir.path());
    let repo = Repo::discover(dir.path()).expect("a repo");
    let ids: Vec<String> = (repo.log_from(std::slice::from_ref(&side), 10).expect("log"))
        .into_iter()
        .map(|c| c.id[..7].to_owned())
        .collect();
    assert_eq!(ids, [side[..7].to_owned(), root[..7].to_owned()]);
    assert_eq!(
        repo.log(10).expect("log").len(),
        3,
        "log still walks every branch"
    );
}

/// A repo on `main` with a merged `done` branch and an unmerged `side` branch.
fn with_branches() -> (tempfile::TempDir, Repo) {
    let dir = init();
    fs::write(dir.path().join("a.txt"), "1\n").expect("write");
    commit(dir.path());
    git(dir.path(), &["branch", "-M", "main"]);
    git(dir.path(), &["branch", "done"]);
    git(dir.path(), &["switch", "-q", "-c", "side"]);
    fs::write(dir.path().join("a.txt"), "side\n").expect("write");
    commit(dir.path());
    git(dir.path(), &["switch", "-q", "main"]);
    git(dir.path(), &["tag", "v0.1"]);
    let repo = Repo::discover(dir.path()).expect("a repo");
    (dir, repo)
}

fn find(repo: &Repo, name: &str) -> zdiff_core::Branch {
    let branches = repo.branches().expect("branches");
    branches.into_iter().find(|b| b.name == name).expect(name)
}

#[test]
fn checkout_switches_to_a_branch_or_detaches_at_a_tag() {
    let (_dir, repo) = with_branches();
    repo.checkout(&find(&repo, "side")).expect("switches");
    assert_eq!(repo.head_name(), "side");
    repo.checkout(&find(&repo, "v0.1")).expect("detaches");
    assert_eq!(
        repo.branches().expect("branches")[0].name,
        zdiff_core::DETACHED
    );
}

#[test]
fn checkout_of_a_remote_branch_makes_a_tracking_one() {
    let (origin, _) = with_branches();
    let clone = tempfile::tempdir().expect("temp dir");
    let from = origin.path().to_str().expect("utf-8 path");
    git(clone.path(), &["clone", "-q", from, "."]);
    let repo = Repo::discover(clone.path()).expect("a repo");
    repo.checkout(&find(&repo, "origin/side")).expect("tracks");
    assert_eq!(repo.head_name(), "side");
    let side = find(&repo, "side");
    assert_eq!(
        side.upstream.map(|u| u.name),
        Some("origin/side".to_owned())
    );
}

#[test]
fn a_new_branch_starts_at_its_commit_and_is_checked_out() {
    let (_dir, repo) = with_branches();
    let side = find(&repo, "side");
    repo.create_branch("try-it", &side.tip).expect("creates");
    assert_eq!(repo.head_name(), "try-it");
    assert_eq!(find(&repo, "try-it").tip, side.tip);
    let refused = repo.create_branch("-x", &side.tip);
    assert!(matches!(refused, Err(Error::Refused(_))), "{refused:?}");
}

#[test]
fn a_branch_merged_into_its_upstream_deletes_without_asking() {
    let (origin, _) = with_branches();
    let clone = tempfile::tempdir().expect("temp dir");
    let from = origin.path().to_str().expect("utf-8 path");
    git(clone.path(), &["clone", "-q", from, "."]);
    git(clone.path(), &["switch", "-q", "--track", "origin/side"]);
    git(clone.path(), &["switch", "-q", "main"]);
    let repo = Repo::discover(clone.path()).expect("a repo");
    assert!(
        repo.delete_branch("side", false).expect("deletes"),
        "merged into origin/side, though not into HEAD"
    );
}

#[test]
fn deleting_asks_before_dropping_unmerged_work() {
    let (_dir, repo) = with_branches();
    assert!(
        repo.delete_branch("done", false).expect("deletes"),
        "merged"
    );
    assert!(
        !repo.delete_branch("side", false).expect("runs"),
        "kept: unmerged"
    );
    let names: Vec<_> = repo
        .branches()
        .expect("branches")
        .into_iter()
        .map(|b| b.name)
        .collect();
    assert!(names.contains(&"side".to_owned()) && !names.contains(&"done".to_owned()));
    assert!(repo.delete_branch("side", true).expect("forced"));
    assert!(
        repo.branches()
            .expect("branches")
            .iter()
            .all(|b| b.name != "side")
    );
}

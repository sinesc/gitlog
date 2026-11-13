//! Integration tests for the git data layer (`gitlog::gitlog`): commit
//! parsing, work-tree queries, committing/amending, pushing and the
//! commit-detail rewrite used by gitrebase.

mod common;

use common::*;
use gitlog::gitlog::*;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

#[test]
fn parses_commits() {
    let dir = fixture_repo("parse");
    let mut commits: Vec<Commit> = Vec::new();
    let cancel = AtomicBool::new(false);
    load_commits(&dir, &cancel, |c| {
        commits.push(c);
        true
    });

    assert_eq!(commits.len(), 3);

    // newest first: the rename commit, tip of main -> marked with the branch
    let c = &commits[0];
    assert_eq!(c.branches, vec!["main"]);
    assert_eq!(c.subject, "third commit: rename b -> b2");
    assert_eq!(c.author, "Tester <t@t.t>");
    assert_eq!(c.committer, "Tester <t@t.t>");
    assert_eq!(c.files.len(), 1);
    assert_eq!(c.files[0].name, "b.txt => b2.txt");
    assert_eq!(c.files[0].action, Action::Renamed);
    assert_eq!(c.files[0].added, Some(0));
    assert_eq!(c.files[0].deleted, Some(0));
    assert!(c.timestamp > 0); // author time, not a formatted string
    assert_eq!(c.committer_timestamp, c.timestamp); // not amended/rebased

    // second commit: modify + delete, not at any branch tip
    let c = &commits[1];
    assert!(c.branches.is_empty());
    assert_eq!(c.subject, "second commit");
    assert_eq!(c.message, "second commit");
    assert_eq!(c.files.len(), 2);
    let a = c.files.iter().find(|f| f.name == "a.txt").unwrap();
    assert_eq!(a.action, Action::Modified);
    assert_eq!(a.added, Some(1));
    assert_eq!(a.deleted, Some(0));
    let d = c.files.iter().find(|f| f.name == "d.txt").unwrap();
    assert_eq!(d.action, Action::Deleted);
    assert_eq!(d.added, Some(0));
    assert_eq!(d.deleted, Some(1));

    // first commit: three adds, message with body
    let c = &commits[2];
    assert_eq!(c.subject, "first commit");
    assert_eq!(c.message, "first commit\n\nbody of first commit");
    assert_eq!(c.files.len(), 3);
    assert!(c.files.iter().all(|f| f.action == Action::Added));
    let a = c.files.iter().find(|f| f.name == "a.txt").unwrap();
    assert_eq!(a.added, Some(2));

    // file sizes at the newest commit
    let head = rev(&dir, "HEAD");
    let second = rev(&dir, "HEAD~1");
    let sizes = file_sizes(&dir, &head);
    assert_eq!(sizes.get("b2.txt"), Some(&2));
    assert_eq!(sizes.get("a.txt"), Some(&18));

    // branch tips: local branch first, then remote-tracking branches
    git(&dir, &["update-ref", "refs/remotes/origin/main", &head]);
    git(&dir, &["update-ref", "refs/remotes/github/main", &second]);
    // remote HEAD symref (as created by git fetch); its refname:short
    // is "origin" and it must not show up as a branch tip
    git(&dir, &[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    let tips = branch_tips(&dir);
    assert_eq!(tips.get(&head), Some(&vec!["main".to_string(), "origin/main".to_string()]));
    assert_eq!(tips.get(&second), Some(&vec!["github/main".to_string()]));
    assert!(!tips.values().flatten().any(|b| b == "origin"));
}

#[test]
fn worktree_changes_and_head() {
    let dir = fixture_repo("worktree");
    // HEAD (rename commit): a.txt (3 lines), b2.txt
    // working-copy changes: a.txt +1 line (staged), b2.txt deleted
    // (unstaged), new.txt untracked
    write(&dir, "a.txt", "line1\nline2\nline3\nline4\n");
    git(&dir, &["add", "-A", "--", "a.txt"]);
    std::fs::remove_file(dir.join("b2.txt")).unwrap();
    write(&dir, "new.txt", "fresh\n");
    write(&dir, "sub/deep/new2.txt", "fresh2\n"); // untracked in a new folder

    let files = worktree_changes(&dir);
    let by_name = |name: &str| files.iter().find(|f| f.path == name).unwrap_or_else(|| {
        panic!("{name} not in worktree_changes: {files:?}")
    });
    assert_eq!(files.len(), 4);
    let a = by_name("a.txt");
    assert_eq!(a.action, Action::Modified);
    assert_eq!(a.added, Some(1));
    assert_eq!(a.deleted, Some(0));
    assert!(a.staged);
    assert!(a.in_head);
    let b = by_name("b2.txt");
    assert_eq!(b.action, Action::Deleted);
    assert!(!b.staged);
    assert!(b.in_head);
    let n = by_name("new.txt");
    assert_eq!(n.action, Action::Added);
    assert_eq!((n.added, n.deleted), (None, None)); // no diff for untracked
    assert!(!n.staged);
    assert!(!n.in_head);
    // untracked files inside a brand-new folder are listed individually,
    // not collapsed into a "sub/deep/" directory entry
    let d = by_name("sub/deep/new2.txt");
    assert_eq!(d.action, Action::Added);
    assert!(!d.in_head);

    // previous commit: the rename, shown as "old => new" with the new path
    let head = head_files(&dir).expect("head_files");
    assert_eq!(head.len(), 1);
    assert_eq!(head[0].path, "b2.txt");
    assert_eq!(head[0].display_name, "b.txt => b2.txt");
    assert_eq!(head[0].action, Action::Renamed);
    assert!(head[0].staged);
    assert!(head[0].in_head);

    assert_eq!(head_message(&dir), Some("third commit: rename b -> b2".to_string()));
    assert!(has_head(&dir));
}

#[test]
fn amend_commit_flow() {
    let dir = fixture_repo("amend");
    // amend the rename commit: keep a.txt (modified in the work tree),
    // drop b2.txt -> it must leave the commit and stay in the work tree
    write(&dir, "a.txt", "line1\nline2\nline3\nline4\n");
    rm_cached_path(&dir, "b2.txt").unwrap();
    stage_path(&dir, "a.txt").unwrap();
    do_commit(&dir, "amended message", true).unwrap();

    // the amended commit no longer contains b2.txt
    let head = head_files(&dir).expect("head_files after amend");
    assert!(!head.iter().any(|f| f.path == "b2.txt"));
    let a = head.iter().find(|f| f.path == "a.txt").expect("a.txt in amended commit");
    assert_eq!(a.action, Action::Modified);
    assert_eq!(a.added, Some(1));
    // b2.txt stays in the work tree as an untracked file
    assert!(dir.join("b2.txt").exists());
    assert!(untracked_files(&dir).contains(&"b2.txt".to_string()));
    assert_eq!(head_message(&dir), Some("amended message".to_string()));
}

#[test]
fn remotes_and_push() {
    let dir = fixture_repo("push");
    // two bare "remote" repositories
    let origin = bare_remote("push-origin");
    let backup = bare_remote("push-backup");

    git(&dir, &["remote", "add", "origin", &origin.to_string_lossy()]);
    assert_eq!(remotes(&dir), vec!["origin".to_string()]);

    // normal push
    run_git(&dir, &["push", "origin", "main"]).unwrap();
    assert_eq!(rev(&origin, "refs/heads/main"), rev(&dir, "HEAD"));

    // a remote that is ahead rejects a non-force push, force succeeds:
    // add a commit, push it, then reset the branch back
    write(&dir, "c.txt", "c\n");
    git(&dir, &["add", "c.txt"]);
    git(&dir, &["commit", "-qm", "fourth commit"]);
    run_git(&dir, &["push", "origin", "main"]).unwrap();
    git(&dir, &["reset", "--hard", "HEAD~1"]);
    assert!(run_git(&dir, &["push", "origin", "main"]).is_err());
    run_git(&dir, &["push", "--force", "origin", "main"]).unwrap();
    let head = rev(&dir, "HEAD");
    assert_eq!(rev(&origin, "refs/heads/main"), head);

    // tags are pushed with --tags
    git(&dir, &["tag", "v1"]);
    run_git(&dir, &["push", "--tags", "origin"]).unwrap();
    assert_eq!(rev(&origin, "refs/tags/v1"), head);

    // a second remote is listed too (`git remote` is alphabetical)
    git(&dir, &["remote", "add", "backup", &backup.to_string_lossy()]);
    assert_eq!(remotes(&dir), vec!["backup".to_string(), "origin".to_string()]);

    // --set-upstream records the tracking branch
    run_git(&dir, &["push", "--set-upstream", "origin", "main"]).unwrap();
    assert_eq!(git_out(&dir, &["config", "branch.main.remote"]).trim(), "origin");
}

/// Fresh repo with identity configured, for the commit-detail tests.
fn rebase_repo(tag: &str) -> PathBuf {
    init_repo(&format!("rebase-{tag}"))
}

#[test]
fn commit_details_fields() {
    let dir = rebase_repo("details");
    write(&dir, "a.txt", "x\n");
    git(&dir, &["add", "a.txt"]);
    git_env(
        &dir,
        &["commit", "-qm", "hello\n\nbody"],
        &[
            ("GIT_AUTHOR_NAME", "Ann Author"),
            ("GIT_AUTHOR_EMAIL", "ann@a.io"),
            ("GIT_AUTHOR_DATE", "2026-01-02 03:04:05"),
            ("GIT_COMMITTER_NAME", "Carl Committer"),
            ("GIT_COMMITTER_EMAIL", "carl@c.io"),
            ("GIT_COMMITTER_DATE", "2026-01-03 04:05:06"),
        ],
    );

    let d = commit_details(&dir, "HEAD").unwrap();
    assert_eq!(d.author, "Ann Author");
    assert_eq!(d.author_email, "ann@a.io");
    assert_eq!(d.author_date, "2026-01-02 03:04:05");
    assert_eq!(d.committer, "Carl Committer");
    assert_eq!(d.committer_email, "carl@c.io");
    assert_eq!(d.committer_date, "2026-01-03 04:05:06");
    assert_eq!(d.message, "hello\n\nbody");

    // unknown hash is an error
    assert!(commit_details(&dir, "deadbeef").is_err());
}

#[test]
fn edit_head_commit_amend() {
    let dir = rebase_repo("head");
    write(&dir, "a.txt", "content\n");
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-qm", "first"]);

    let mut d = commit_details(&dir, "HEAD").unwrap();
    d.author = "New Author".into();
    d.author_email = "na@x.io".into();
    d.author_date = "2026-01-01 02:03:04".into();
    d.committer = "New Committer".into();
    d.committer_email = "nc@x.io".into();
    d.committer_date = "2026-01-02 03:04:05".into();
    d.message = "rewritten\n\nbody".into();

    // a dirty work tree is refused
    write(&dir, "a.txt", "dirty\n");
    let err = edit_commit_details(&dir, "HEAD", &d).unwrap_err();
    assert!(err.contains("uncommitted changes"), "got: {err}");

    git(&dir, &["checkout", "-q", "--", "a.txt"]);
    edit_commit_details(&dir, "HEAD", &d).unwrap();

    let after = commit_details(&dir, "HEAD").unwrap();
    assert_eq!(after.author, "New Author");
    assert_eq!(after.author_email, "na@x.io");
    assert_eq!(after.author_date, "2026-01-01 02:03:04");
    assert_eq!(after.committer, "New Committer");
    assert_eq!(after.committer_email, "nc@x.io");
    assert_eq!(after.committer_date, "2026-01-02 03:04:05");
    assert_eq!(after.message, "rewritten\n\nbody");
    // the file content is untouched
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "content\n");
    assert_eq!(
        git_out(&dir, &["log", "-1", "--format=", "--name-only"]).lines().filter(|l| !l.is_empty()).collect::<Vec<_>>(),
        ["a.txt"]
    );
}

#[test]
fn edit_middle_commit_rebase() {
    let dir = rebase_repo("middle");
    for (f, m) in [("a.txt", "first"), ("b.txt", "second"), ("c.txt", "third")] {
        write(&dir, f, &format!("{f}\n"));
        git(&dir, &["add", f]);
        git(&dir, &["commit", "-qm", m]);
    }
    let middle = rev(&dir, "HEAD~1");

    let mut d = commit_details(&dir, &middle).unwrap();
    assert_eq!(d.message, "second");
    d.author = "New Author".into();
    d.author_email = "na@x.io".into();
    d.author_date = "2026-01-01 02:03:04".into();
    d.committer = "New Committer".into();
    d.committer_email = "nc@x.io".into();
    d.committer_date = "2026-01-02 03:04:05".into();
    d.message = "second (rewritten)".into();

    edit_commit_details(&dir, &middle, &d).unwrap();

    // top and bottom commits keep their identity and message
    assert_eq!(
        git_out(&dir, &["log", "--format=%s|%an", "-3"]).lines().collect::<Vec<_>>(),
        ["third|Tester", "second (rewritten)|New Author", "first|Tester"]
    );

    let after = commit_details(&dir, "HEAD~1").unwrap();
    assert_eq!(after.author_email, "na@x.io");
    assert_eq!(after.author_date, "2026-01-01 02:03:04");
    assert_eq!(after.committer, "New Committer");
    assert_eq!(after.committer_email, "nc@x.io");
    assert_eq!(after.committer_date, "2026-01-02 03:04:05");
    assert_eq!(after.message, "second (rewritten)");

    // work tree clean, all files intact
    assert!(git_out(&dir, &["status", "--porcelain"]).trim().is_empty());
    assert_eq!(
        git_out(&dir, &["ls-tree", "-r", "--name-only", "HEAD"]).lines().collect::<Vec<_>>(),
        ["a.txt", "b.txt", "c.txt"]
    );
}

#[test]
fn edit_preserves_descendant_committers() {
    let dir = rebase_repo("desc");
    // the descendant's author and committer differ from the local
    // identity (Tester <t@t.t>), which rewrites would replace
    write(&dir, "a.txt", "x\n");
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-qm", "first"]);
    write(&dir, "b.txt", "y\n");
    git(&dir, &["add", "b.txt"]);
    git_env(
        &dir,
        &["commit", "-qm", "second"],
        &[
            ("GIT_AUTHOR_NAME", "Old Author"),
            ("GIT_AUTHOR_EMAIL", "old-author@o.io"),
            ("GIT_AUTHOR_DATE", "2026-01-04 05:06:07"),
            ("GIT_COMMITTER_NAME", "Old Committer"),
            ("GIT_COMMITTER_EMAIL", "old@o.io"),
            ("GIT_COMMITTER_DATE", "2026-01-05 06:07:08"),
        ],
    );
    let before = commit_details(&dir, "HEAD").unwrap();

    let first = rev(&dir, "HEAD~1");
    let mut d = commit_details(&dir, &first).unwrap();
    d.message = "first (rewritten)".into();
    edit_commit_details(&dir, &first, &d).unwrap();

    let after = commit_details(&dir, "HEAD").unwrap();
    // the descendant is untouched: same committer (name, email and
    // date) and author, only the hash differs
    assert_eq!(after.committer, before.committer);
    assert_eq!(after.committer_email, before.committer_email);
    assert_eq!(after.committer_date, before.committer_date);
    assert_eq!(after.author, before.author);
    assert_eq!(after.author_email, before.author_email);
    assert_eq!(after.author_date, before.author_date);
    // and the preserved author really is the original one, not the
    // local identity
    assert_eq!(after.author, "Old Author");
    assert_eq!(after.author_email, "old-author@o.io");
    assert_eq!(after.author_date, "2026-01-04 05:06:07");
    assert_eq!(after.message, "second");
    assert_eq!(after.committer_date, "2026-01-05 06:07:08");
    assert!(git_out(&dir, &["status", "--porcelain"]).trim().is_empty());
}

#[test]
fn edit_with_merge_descendant() {
    let dir = rebase_repo("merge");
    write(&dir, "a.txt", "x\n");
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-qm", "first"]);
    write(&dir, "b.txt", "y\n");
    git(&dir, &["add", "b.txt"]);
    git_env(
        &dir,
        &["commit", "-qm", "second"],
        &[
            ("GIT_COMMITTER_NAME", "Old Committer"),
            ("GIT_COMMITTER_EMAIL", "old@o.io"),
            ("GIT_COMMITTER_DATE", "2026-01-05 06:07:08"),
        ],
    );
    // a side branch off "first" and a merge on top of "second"
    git(&dir, &["checkout", "-qb", "side", "HEAD~1"]);
    write(&dir, "c.txt", "z\n");
    git(&dir, &["add", "c.txt"]);
    git(&dir, &["commit", "-qm", "side work"]);
    git(&dir, &["checkout", "-q", "main"]);
    git_env(
        &dir,
        &["merge", "--no-ff", "-q", "-m", "merged", "side"],
        &[
            ("GIT_COMMITTER_NAME", "Merge Bot"),
            ("GIT_COMMITTER_EMAIL", "bot@b.io"),
            ("GIT_COMMITTER_DATE", "2026-01-09 10:11:12"),
        ],
    );

    let first = rev(&dir, "HEAD~2");
    let mut d = commit_details(&dir, &first).unwrap();
    d.message = "first (rewritten)".into();
    edit_commit_details(&dir, &first, &d).unwrap();

    // the merge commit keeps both parents, its committer and its message
    let head_parents = git_out(&dir, &["log", "-1", "--format=%P", "HEAD"]);
    let parents: Vec<&str> = head_parents.split_whitespace().collect();
    assert_eq!(parents.len(), 2);
    let head = commit_details(&dir, "HEAD").unwrap();
    assert_eq!(head.committer, "Merge Bot");
    assert_eq!(head.committer_email, "bot@b.io");
    assert_eq!(head.committer_date, "2026-01-09 10:11:12");
    assert_eq!(head.message, "merged");
    // the "second" descendant (first parent of the merge) keeps its
    // committer
    let second = commit_details(&dir, parents[0]).unwrap();
    assert_eq!(second.committer, "Old Committer");
    assert_eq!(second.committer_date, "2026-01-05 06:07:08");
    // the side branch's commit was re-created too (its parent
    // changed), but author, committer and message are unchanged and
    // the `side` branch ref points at the new commit
    let side = commit_details(&dir, parents[1]).unwrap();
    assert_eq!(side.message, "side work");
    assert_eq!(side.committer, "Tester");
    assert_eq!(side.author, "Tester");
    assert_eq!(git_out(&dir, &["rev-parse", "side"]).trim(), parents[1]);
    // ... and it was re-parented onto the rewritten "first"
    let side_parent = git_out(&dir, &["log", "-1", "--format=%P", parents[1]]);
    assert_eq!(side_parent.trim(), rev(&dir, "HEAD~2"));
    assert!(git_out(&dir, &["status", "--porcelain"]).trim().is_empty());
}

#[test]
fn edit_root_commit() {
    let dir = rebase_repo("root");
    write(&dir, "a.txt", "a\n");
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-qm", "first"]);
    write(&dir, "b.txt", "b\n");
    git(&dir, &["add", "b.txt"]);
    git(&dir, &["commit", "-qm", "second"]);

    let root = rev(&dir, "HEAD~1");
    let mut d = commit_details(&dir, &root).unwrap();
    d.author = "New Author".into();
    d.author_email = "na@x.io".into();
    d.author_date = "2026-01-01 02:03:04".into();
    d.committer = "New Committer".into();
    d.committer_email = "nc@x.io".into();
    d.committer_date = "2026-01-02 03:04:05".into();
    d.message = "first (rewritten)".into();

    edit_commit_details(&dir, &root, &d).unwrap();

    assert_eq!(
        git_out(&dir, &["log", "--format=%s|%an", "-2"]).lines().collect::<Vec<_>>(),
        ["second|Tester", "first (rewritten)|New Author"]
    );
    assert!(git_out(&dir, &["status", "--porcelain"]).trim().is_empty());
}

#[test]
fn is_pushed_check() {
    let dir = fixture_repo("pushed");
    // no remotes configured: nothing is pushed
    assert!(!is_pushed(&dir, "HEAD"));

    let bare = bare_remote("pushed-origin");
    git(&dir, &["remote", "add", "origin", &bare.to_string_lossy()]);
    run_git(&dir, &["push", "origin", "main"]).unwrap();

    // the whole pushed history is reachable from origin/main
    assert!(is_pushed(&dir, "HEAD"));
    assert!(is_pushed(&dir, "HEAD~1"));
    assert!(is_pushed(&dir, "HEAD~2"));
}

// ---- gitpull support (see dev/gitpull-plan.md) -------------------------------

/// Repo with one content commit pushed to a bare origin, plus a second
/// working clone `work2` of that remote. Returns (repo, origin, work2).
fn pull_repo(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
    let dir = init_repo(&format!("pull-{tag}"));
    write(&dir, "a.txt", "base\n");
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-qm", "base"]);
    let origin = bare_remote(&format!("pull-{tag}-origin"));
    git(&dir, &["remote", "add", "origin", &origin.to_string_lossy()]);
    git(&dir, &["push", "-q", "--set-upstream", "origin", "main"]);

    let work2 = temp_dir(&format!("pull-{tag}-work2"));
    std::fs::remove_dir_all(&work2).unwrap();
    let out = std::process::Command::new("git")
        .args([
            "clone",
            "-q",
            &origin.to_string_lossy(),
            &work2.to_string_lossy(),
        ])
        .output()
        .expect("failed to run git clone");
    assert!(
        out.status.success(),
        "git clone failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    configure_identity(&work2);
    (dir, origin, work2)
}

/// Appends `text` to `file` (creating it when missing), commits and
/// pushes in `repo`.
fn commit_and_push(repo: &Path, file: &str, text: &str, msg: &str) {
    let path = repo.join(file);
    let mut content = std::fs::read_to_string(&path).unwrap_or_default();
    content.push_str(text);
    std::fs::write(&path, content).unwrap();
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-qm", msg]);
    run_git(repo, &["push", "-q", "origin", "main"]).unwrap();
}

const EDITOR_TRUE: [(&str, &str); 1] = [("GIT_EDITOR", "true")];

#[test]
fn upstream_of_check() {
    let (dir, _origin, _work2) = pull_repo("upstream");
    // push -u above recorded the tracking branch
    assert_eq!(
        upstream_of(&dir),
        Some(("origin".to_string(), "main".to_string()))
    );
    // detached HEAD has no upstream
    git(&dir, &["checkout", "-q", "--detach"]);
    assert_eq!(upstream_of(&dir), None);
    // a branch without tracking config has none either
    git(&dir, &["checkout", "-qb", "loose"]);
    assert_eq!(upstream_of(&dir), None);
}

#[test]
fn merge_conflict_resolve_and_commit() {
    let (dir, _origin, work2) = pull_repo("conflict");
    // diverging edits of a.txt
    commit_and_push(&work2, "a.txt", "theirs\n", "work2 edit");
    std::fs::write(dir.join("a.txt"), "base\nours\n").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-qm", "ours edit"]);

    run_git(&dir, &["fetch", "origin"]).unwrap();
    assert!(run_git_env(&dir, &["merge", "origin/main"], &EDITOR_TRUE).is_err());
    assert!(merge_in_progress(&dir));
    assert_eq!(
        unmerged_files(&dir),
        vec![("a.txt".to_string(), "UU".to_string())]
    );
    // the three stages are the merge base and both sides
    let (base, ours, theirs) = conflict_blobs(&dir, "a.txt");
    assert_eq!(base.as_deref(), Some("base\n".as_bytes()));
    assert_eq!(ours.as_deref(), Some("base\nours\n".as_bytes()));
    assert_eq!(theirs.as_deref(), Some("base\ntheirs\n".as_bytes()));

    // abort clears the whole merge state
    run_git(&dir, &["merge", "--abort"]).unwrap();
    assert!(!merge_in_progress(&dir));
    assert!(unmerged_files(&dir).is_empty());

    // merge again, resolve the file and create the merge commit
    assert!(run_git_env(&dir, &["merge", "origin/main"], &EDITOR_TRUE).is_err());
    std::fs::write(dir.join("a.txt"), "base\nours\ntheirs\n").unwrap();
    stage_path(&dir, "a.txt").unwrap();
    assert!(unmerged_files(&dir).is_empty());
    assert!(merge_in_progress(&dir)); // MERGE_HEAD stays until the commit
    commit_merge(&dir).unwrap();
    assert!(!merge_in_progress(&dir));
    let parents = git_out(&dir, &["log", "-1", "--format=%P"]);
    assert_eq!(parents.split_whitespace().count(), 2, "two-parent merge commit");
    assert!(git_out(&dir, &["status", "--porcelain"]).trim().is_empty());
}

#[test]
fn unmerged_status_codes() {
    // AA: both sides add the same file (no merge base stage)
    let dir = init_repo("conf-aa");
    write(&dir, "a.txt", "base\n");
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "base"]);
    git(&dir, &["checkout", "-qb", "side"]);
    write(&dir, "n.txt", "theirs\n");
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "side add"]);
    git(&dir, &["checkout", "-q", "main"]);
    write(&dir, "n.txt", "ours\n");
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "ours add"]);
    assert!(run_git_env(&dir, &["merge", "side"], &EDITOR_TRUE).is_err());
    assert_eq!(
        unmerged_files(&dir),
        vec![("n.txt".to_string(), "AA".to_string())]
    );
    let (base, _ours, _theirs) = conflict_blobs(&dir, "n.txt");
    assert_eq!(base, None);
    run_git(&dir, &["merge", "--abort"]).unwrap();

    // DU: we deleted the file, the other side modified it
    let dir = init_repo("conf-du");
    write(&dir, "f.txt", "v1\n");
    write(&dir, "a.txt", "a\n");
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "base"]);
    git(&dir, &["checkout", "-qb", "side"]);
    write(&dir, "f.txt", "v1\nmodified\n");
    git(&dir, &["commit", "-am", "side modify"]);
    git(&dir, &["checkout", "-q", "main"]);
    std::fs::remove_file(dir.join("f.txt")).unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "main delete"]);
    assert!(run_git_env(&dir, &["merge", "side"], &EDITOR_TRUE).is_err());
    assert_eq!(
        unmerged_files(&dir),
        vec![("f.txt".to_string(), "DU".to_string())]
    );
    // deleting the file resolves it (git rm == stage the deletion)
    run_git(&dir, &["rm", "-q", "-f", "f.txt"]).unwrap();
    commit_merge(&dir).unwrap();
    assert!(!dir.join("f.txt").exists());

    // UD: we modified the file, the other side deleted it
    let dir = init_repo("conf-ud");
    write(&dir, "f.txt", "v1\n");
    write(&dir, "a.txt", "a\n");
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "base"]);
    git(&dir, &["checkout", "-qb", "side"]);
    std::fs::remove_file(dir.join("f.txt")).unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "side delete"]);
    git(&dir, &["checkout", "-q", "main"]);
    write(&dir, "f.txt", "v1\nmodified\n");
    git(&dir, &["commit", "-am", "main modify"]);
    assert!(run_git_env(&dir, &["merge", "side"], &EDITOR_TRUE).is_err());
    assert_eq!(
        unmerged_files(&dir),
        vec![("f.txt".to_string(), "UD".to_string())]
    );
    run_git(&dir, &["merge", "--abort"]).unwrap();
    // (a plain "both deleted" DD is not exercised: git auto-resolves
    // symmetric deletions, so it needs a crisscross history)
}

#[test]
fn rebase_progress_and_continue_loop() {
    let (dir, _origin, work2) = pull_repo("rebase-loop");
    // remote and local edits append at the same line -> both local
    // commits conflict with the remote commit
    commit_and_push(&work2, "a.txt", "remote1\n", "remote edit");
    std::fs::write(dir.join("a.txt"), "base\nours1\n").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-qm", "local 1"]);
    std::fs::write(dir.join("a.txt"), "base\nours1-mod\n").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-qm", "local 2"]);

    assert!(
        run_git_env(&dir, &["pull", "--rebase", "origin", "main"], &EDITOR_TRUE).is_err()
    );
    assert_eq!(rebase_progress(&dir), Some((1, 2, "main".to_string())));
    assert_eq!(
        unmerged_files(&dir),
        vec![("a.txt".to_string(), "UU".to_string())]
    );
    // resolve the first step; the continue applies it and hits the
    // second conflict (continue fails, the rebase stays in progress)
    std::fs::write(dir.join("a.txt"), "base\nremote1\nours1\n").unwrap();
    stage_path(&dir, "a.txt").unwrap();
    assert!(rebase_continue(&dir).is_err());
    assert_eq!(rebase_progress(&dir), Some((2, 2, "main".to_string())));
    assert_eq!(
        unmerged_files(&dir),
        vec![("a.txt".to_string(), "UU".to_string())]
    );
    // resolve the second step; the rebase finishes
    std::fs::write(dir.join("a.txt"), "base\nremote1\nours1-mod\n").unwrap();
    stage_path(&dir, "a.txt").unwrap();
    rebase_continue(&dir).unwrap();
    assert_eq!(rebase_progress(&dir), None);
    assert_eq!(
        git_out(&dir, &["log", "--format=%s"]).lines().collect::<Vec<_>>(),
        ["local 2", "local 1", "remote edit", "base"]
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "base\nremote1\nours1-mod\n"
    );
    assert!(git_out(&dir, &["status", "--porcelain"]).trim().is_empty());
}

#[test]
fn rebase_abort_clears_state() {
    let (dir, _origin, work2) = pull_repo("rebase-abort");
    commit_and_push(&work2, "a.txt", "remote1\n", "remote edit");
    std::fs::write(dir.join("a.txt"), "base\nours1\n").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-qm", "local 1"]);

    assert!(
        run_git_env(&dir, &["pull", "--rebase", "origin", "main"], &EDITOR_TRUE).is_err()
    );
    assert!(rebase_progress(&dir).is_some());
    run_git(&dir, &["rebase", "--abort"]).unwrap();
    assert_eq!(rebase_progress(&dir), None);
    assert!(unmerged_files(&dir).is_empty());
    assert_eq!(rev(&dir, "HEAD"), rev(&dir, "refs/heads/main"));
    assert!(git_out(&dir, &["status", "--porcelain"]).trim().is_empty());
}

#[test]
fn pull_fetch_flags_e2e() {
    let (dir, origin, work2) = pull_repo("fetch-e2e");
    // a plain fetch only updates the remote-tracking refs
    commit_and_push(&work2, "a.txt", "remote\n", "remote edit");
    run_git(&dir, &["fetch", "origin"]).unwrap();
    assert_eq!(rev(&dir, "refs/remotes/origin/main"), rev(&work2, "HEAD"));
    // the local branch stayed untouched
    assert_ne!(rev(&dir, "refs/heads/main"), rev(&dir, "refs/remotes/origin/main"));

    // --tags fetches the tags as well
    git(&work2, &["tag", "v9"]);
    git(&work2, &["push", "-q", "--tags", "origin"]);
    run_git(&dir, &["fetch", "--tags", "origin"]).unwrap();
    assert_eq!(rev(&dir, "refs/tags/v9"), rev(&work2, "HEAD"));

    // --prune drops the remote-tracking branch of a deleted remote branch
    git(&work2, &["push", "-q", "origin", "HEAD:refs/heads/topic"]);
    run_git(&dir, &["fetch", "origin"]).unwrap();
    rev(&dir, "refs/remotes/origin/topic");
    git(&origin, &["update-ref", "-d", "refs/heads/topic"]);
    run_git(&dir, &["fetch", "--prune", "origin"]).unwrap();
    assert!(
        git_out(&dir, &["for-each-ref", "refs/remotes/origin/topic"])
            .trim()
            .is_empty()
    );
}

#[test]
fn pull_merge_flags_e2e() {
    let (dir, _origin, work2) = pull_repo("merge-e2e");
    // fast-forward merge pull
    commit_and_push(&work2, "a.txt", "remote\n", "remote edit");
    run_git_env(
        &dir,
        &["pull", "--no-rebase", "--no-edit", "origin", "main"],
        &EDITOR_TRUE,
    )
    .unwrap();
    assert_eq!(rev(&dir, "HEAD"), rev(&work2, "HEAD"));

    // diverging: --ff-only is rejected, --no-ff creates a merge commit
    commit_and_push(&work2, "c.txt", "c\n", "remote c");
    write(&dir, "b.txt", "b\n");
    git(&dir, &["add", "b.txt"]);
    git(&dir, &["commit", "-qm", "local b"]);
    assert!(
        run_git_env(
            &dir,
            &["pull", "--no-rebase", "--ff-only", "--no-edit", "origin", "main"],
            &EDITOR_TRUE
        )
        .is_err()
    );
    run_git_env(
        &dir,
        &["pull", "--no-rebase", "--no-ff", "--no-edit", "origin", "main"],
        &EDITOR_TRUE
    )
    .unwrap();
    let parents = git_out(&dir, &["log", "-1", "--format=%P"]);
    assert_eq!(parents.split_whitespace().count(), 2);
    // git's auto merge message (via pull it names the fetched branch)
    assert!(git_out(&dir, &["log", "--format=%s"])
        .lines()
        .next()
        .unwrap()
        .starts_with("Merge "));
    assert!(dir.join("c.txt").exists());
    assert!(git_out(&dir, &["status", "--porcelain"]).trim().is_empty());
}

#[test]
fn pull_squash_stages_without_commit() {
    let (dir, _origin, work2) = pull_repo("squash-e2e");
    commit_and_push(&work2, "a.txt", "remote\n", "remote edit");
    write(&dir, "b.txt", "b\n");
    git(&dir, &["add", "b.txt"]);
    git(&dir, &["commit", "-qm", "local b"]);
    let head_before = rev(&dir, "HEAD");

    run_git_env(
        &dir,
        &["pull", "--no-rebase", "--squash", "origin", "main"],
        &EDITOR_TRUE
    )
    .unwrap();

    // the combined changes are staged, no commit and no MERGE_HEAD
    assert_eq!(rev(&dir, "HEAD"), head_before);
    assert!(!merge_in_progress(&dir));
    assert!(!git_out(&dir, &["diff", "--cached", "--name-only", "HEAD"])
        .trim()
        .is_empty());
    assert_eq!(unmerged_files(&dir), Vec::new());
}

#[test]
fn pull_rebase_rewrites_local_commits() {
    let (dir, _origin, work2) = pull_repo("rebase-clean");
    commit_and_push(&work2, "c.txt", "c\n", "remote c");
    write(&dir, "b.txt", "b\n");
    git(&dir, &["add", "b.txt"]);
    git(&dir, &["commit", "-qm", "local b"]);

    run_git_env(&dir, &["pull", "--rebase", "origin", "main"], &EDITOR_TRUE).unwrap();

    assert_eq!(
        git_out(&dir, &["log", "--format=%s"]).lines().collect::<Vec<_>>(),
        ["local b", "remote c", "base"]
    );
    assert_eq!(rebase_progress(&dir), None);
    assert!(dir.join("b.txt").exists());
    assert!(dir.join("c.txt").exists());
    assert!(git_out(&dir, &["status", "--porcelain"]).trim().is_empty());
}

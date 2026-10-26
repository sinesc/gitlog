//! Integration tests for the git data layer (`gitlog::gitlog`): commit
//! parsing, work-tree queries, committing/amending, pushing and the
//! commit-detail rewrite used by gitrebase.

mod common;

use common::*;
use gitlog::gitlog::*;
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

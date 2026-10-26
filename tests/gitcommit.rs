//! Integration tests for the amend logic behind the commit window
//! (`gitlog::gitlog::amend_files` / `commit_checked`).

mod common;

use common::*;
use gitlog::gitlog::{self as gl, StageRow};

#[test]
fn amend_files_merges_head_and_worktree() {
    let dir = init_repo("commit-merge");
    write(&dir, "a.txt", "line1\nline2\n");
    write(&dir, "b.txt", "x\n");
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "first"]);

    // working copy: a.txt +1 line, new.txt untracked
    write(&dir, "a.txt", "line1\nline2\nline3\n");
    write(&dir, "new.txt", "fresh\n");

    let files = gl::amend_files(&dir);
    assert_eq!(files.len(), 3);
    // a file in HEAD that changed in the work tree: merged, keeps in_head
    let a = files.iter().find(|f| f.path == "a.txt").expect("a.txt");
    assert_eq!(a.action, gl::Action::Modified);
    assert_eq!(a.added, Some(1));
    assert!(a.in_head);
    // unchanged file from HEAD
    let b = files.iter().find(|f| f.path == "b.txt").expect("b.txt");
    assert_eq!(b.action, gl::Action::Added);
    assert!(b.in_head);
    // untracked work tree file
    let n = files.iter().find(|f| f.path == "new.txt").expect("new.txt");
    assert!(!n.in_head);
}

#[test]
fn amend_files_merges_rename_of_head_file() {
    let dir = init_repo("commit-merge-rename");
    write(&dir, "x.txt", "line1\n");
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "first"]);

    // rename after the commit: must show as a single "old => new" row,
    // not as a head row for the old path plus a rename row
    git(&dir, &["mv", "x.txt", "y.txt"]);
    let files = gl::amend_files(&dir);
    assert_eq!(files.len(), 1);
    let r = &files[0];
    assert_eq!(r.path, "y.txt");
    assert_eq!(r.display_name, "x.txt => y.txt");
    assert_eq!(r.action, gl::Action::Renamed);
    assert!(r.in_head);
}

#[test]
fn amend_rename_commit_flow() {
    let dir = init_repo("commit-rename-flow");
    // parent commit so the amended commit's diff shows the rename
    write(&dir, "z.txt", "z\n");
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "first"]);
    write(&dir, "x.txt", "line1\n");
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "second"]);
    git(&dir, &["mv", "x.txt", "y.txt"]);

    // the single merged row is checked; committing must not fail on the
    // old path anymore
    let files = gl::amend_files(&dir);
    assert_eq!(files.len(), 1);
    let rows = files
        .iter()
        .map(|f| StageRow { path: f.path.clone(), checked: true, in_head: f.in_head, staged: f.staged })
        .collect::<Vec<_>>();
    gl::commit_checked(&dir, &rows, true, "amended with rename").unwrap();
    let head = gl::head_files(&dir).expect("head_files after amend");
    assert_eq!(head.len(), 1);
    assert_eq!(head[0].path, "y.txt");
    // x.txt only existed in the amended-away commit, so relative to the
    // parent the file is a plain add (no rename to show)
    assert_eq!(head[0].action, gl::Action::Added);
}

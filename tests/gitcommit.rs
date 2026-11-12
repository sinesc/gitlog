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
        .map(|f| StageRow { old_path: None, path: f.path.clone(), checked: true, in_head: f.in_head, staged: f.staged })
        .collect::<Vec<_>>();
    gl::commit_checked(&dir, &rows, true, "amended with rename").unwrap();
    let head = gl::head_files(&dir).expect("head_files after amend");
    assert_eq!(head.len(), 1);
    assert_eq!(head[0].path, "y.txt");
    // x.txt only existed in the amended-away commit, so relative to the
    // parent the file is a plain add (no rename to show)
    assert_eq!(head[0].action, gl::Action::Added);
}

#[test]
fn amend_uncheck_modified_file_leaves_it_uncommitted() {
    let dir = init_repo("amend-uncheck-mod");
    write(&dir, "keep.txt", "k\n");
    write(&dir, "mod.txt", "original\n");
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "first"]);
    // second commit modifies mod.txt *and* keep.txt; unchecking mod.txt
    // must not record a deletion in the amended commit
    write(&dir, "mod.txt", "changed in commit\n");
    write(&dir, "keep.txt", "k2\n");
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "second"]);

    let rows: Vec<StageRow> = gl::amend_files(&dir)
        .iter()
        .map(|f| StageRow {
            old_path: None,
            path: f.path.clone(),
            checked: f.path != "mod.txt",
            in_head: f.in_head,
            staged: f.staged,
        })
        .collect();
    gl::commit_checked(&dir, &rows, true, "amended").unwrap();

    // the amended commit only touches keep.txt...
    let head = gl::head_files(&dir).expect("head_files after amend");
    assert!(head.iter().all(|f| f.path != "mod.txt"), "mod.txt left the commit");
    // ...and still carries mod.txt with its parent content (not deleted)
    assert_eq!(git_out(&dir, &["show", "HEAD:mod.txt"]), "original\n");
    // the modification is now an ordinary uncommitted work-tree change
    let wt = gl::worktree_changes(&dir);
    let m = wt.iter().find(|f| f.path == "mod.txt").expect("mod.txt uncommitted");
    assert_eq!(m.action, gl::Action::Modified);
    assert_eq!(m.added, Some(1));
}

#[test]
fn amend_uncheck_added_and_deleted_files() {
    let dir = init_repo("amend-uncheck-add-del");
    write(&dir, "keep.txt", "k\n");
    write(&dir, "gone.txt", "g\n");
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "first"]);
    // second commit adds new.txt and deletes gone.txt
    write(&dir, "gone.txt", "");
    std::fs::remove_file(dir.join("gone.txt")).unwrap();
    write(&dir, "new.txt", "fresh\n");
    write(&dir, "keep.txt", "k2\n");
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "second"]);

    let rows: Vec<StageRow> = gl::amend_files(&dir)
        .iter()
        .map(|f| StageRow {
            old_path: None,
            path: f.path.clone(),
            checked: f.path == "keep.txt",
            in_head: f.in_head,
            staged: f.staged,
        })
        .collect();
    gl::commit_checked(&dir, &rows, true, "amended").unwrap();

    // amended commit only changes keep.txt: new.txt is not in it...
    assert!(git_out(&dir, &["show", "--name-only", "--format=", "HEAD"]).trim() == "keep.txt");
    // ...gone.txt is back from the parent (not deleted)...
    assert_eq!(git_out(&dir, &["show", "HEAD:gone.txt"]), "g\n");
    // ...new.txt stays untracked and gone.txt stays locally deleted
    let wt = gl::worktree_changes(&dir);
    assert!(wt.iter().any(|f| f.path == "gone.txt" && f.action == gl::Action::Deleted));
    let untracked = gl::untracked_files(&dir);
    assert!(untracked.iter().any(|p| p == "new.txt"), "new.txt untracked");
}

#[test]
fn amend_uncheck_committed_rename_restores_old_path() {
    let dir = init_repo("amend-uncheck-rename");
    write(&dir, "x.txt", "line1\n");
    write(&dir, "z.txt", "z\n");
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "first"]);
    // the rename is part of the commit being amended (z.txt changes too,
    // so unchecking the rename does not empty the commit)
    git(&dir, &["mv", "x.txt", "y.txt"]);
    write(&dir, "z.txt", "z2\n");
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "renamed"]);

    let rows: Vec<StageRow> = gl::amend_files(&dir)
        .iter()
        .map(|f| StageRow {
            old_path: (f.action == gl::Action::Renamed)
                .then(|| f.display_name.split("=>").next().unwrap_or("").trim().to_string()),
            path: f.path.clone(),
            checked: f.path != "y.txt",
            in_head: f.in_head,
            staged: f.staged,
        })
        .collect();
    gl::commit_checked(&dir, &rows, true, "amended").unwrap();

    // the amended commit contains neither side of the rename (it is empty),
    // i.e. x.txt simply comes back from the parent instead of y.txt showing
    // up as added + x.txt as deleted
    assert_eq!(git_out(&dir, &["show", "HEAD:x.txt"]), "line1\n");
    assert!(!git_out(&dir, &["ls-tree", "-r", "--name-only", "HEAD"]).lines().any(|p| p == "y.txt"));
    // the rename stays uncommitted in the work tree (x.txt locally deleted,
    // y.txt untracked - unstaged renames always show as two entries)
    let wt = gl::worktree_changes(&dir);
    assert!(wt.iter().any(|f| f.path == "x.txt" && f.action == gl::Action::Deleted));
    assert!(gl::untracked_files(&dir).iter().any(|p| p == "y.txt"));
}

// ---- spreadsheet diffs via the LibreOffice Calc driver ----------------------

#[test]
fn is_spreadsheet_only_for_calc_formats() {
    assert!(gl::is_spreadsheet("isa.ods"));
    assert!(gl::is_spreadsheet("ISA.ODS"));
    assert!(gl::is_spreadsheet("a/b/sheet.XLSX"));
    assert!(gl::is_spreadsheet("legacy.xls"));
    // templates are opened as new documents, so in-place compare would fail
    assert!(!gl::is_spreadsheet("t.ots"));
    assert!(!gl::is_spreadsheet("notes.txt"));
    assert!(!gl::is_spreadsheet("noext"));
}

#[test]
fn file_url_percent_encodes_path() {
    // the URL is part of a command-line macro argument, so blanks, commas,
    // parentheses, dots-for-safety and shell or macro syntax must not
    // survive literally; ':' and '/' keep the scheme readable for LibreOffice
    let u = gl::file_url(std::path::Path::new("/tmp/a b/c(head).ods"));
    assert_eq!(u, "file:///tmp/a%20b/c%28head%29.ods");
    assert!(gl::file_url(std::path::Path::new("/x,y")).contains("%2C"));
}

#[test]
fn install_calc_driver_writes_module_and_registers_it() {
    let dir = temp_dir("calc-driver");
    std::fs::write(
        dir.join("script.xlb"),
        "<library:library>\n <library:element library:name=\"Module1\"/>\n</library:library>",
    )
    .unwrap();

    assert!(gl::install_calc_driver(&dir));
    let xba = std::fs::read_to_string(dir.join("GitcommitDiff.xba")).unwrap();
    assert!(xba.contains("Sub CompareWith(sOtherUrl As String)"));
    assert!(xba.contains(".uno:CompareDocuments"));
    let xlb = std::fs::read_to_string(dir.join("script.xlb")).unwrap();
    assert!(xlb.contains("library:name=\"Module1\""), "existing module kept");

    // a second run refreshes the module but must not register it twice
    assert!(gl::install_calc_driver(&dir));
    let xlb = std::fs::read_to_string(dir.join("script.xlb")).unwrap();
    assert_eq!(xlb.matches("GitcommitDiff").count(), 1);
}

// ---- context menu actions: git restore / .gitignore -------------------------

#[test]
fn restore_path_discards_worktree_and_index_changes() {
    let dir = fixture_repo("ctx-restore");
    // modify and stage a.txt, so index AND worktree differ from HEAD
    write(&dir, "a.txt", "line1\nline2\nline3\nextra\n");
    git(&dir, &["add", "a.txt"]);
    gl::restore_path(&dir, "a.txt").unwrap();
    // index and working tree must match HEAD again
    assert!(gl::worktree_changes(&dir)
        .iter()
        .all(|f| f.path != "a.txt"));
    assert!(gl::index_matches_head(&dir));
    let content = std::fs::read_to_string(dir.join("a.txt")).unwrap();
    // HEAD's a.txt (fixture: "second commit" added line3)
    assert_eq!(content, "line1\nline2\nline3\n");
}

#[test]
fn restore_path_recreates_deleted_file() {
    let dir = fixture_repo("ctx-restore-del");
    std::fs::remove_dir_all(dir.join("x")).ok();
    std::fs::remove_file(dir.join("b2.txt")).unwrap();
    git(&dir, &["rm", "-q", "b2.txt"]);
    gl::restore_path(&dir, "b2.txt").unwrap();
    assert!(dir.join("b2.txt").exists());
    assert!(gl::index_matches_head(&dir));
}

#[test]
fn append_gitignore_creates_appends_and_dedups() {
    let dir = init_repo("ctx-ignore");
    // creates the file when missing
    gl::append_gitignore(&dir, "/a.txt").unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
        "/a.txt\n"
    );
    // appends after content without a trailing newline cleanly
    std::fs::write(dir.join(".gitignore"), "*.tmp").unwrap();
    gl::append_gitignore(&dir, "/b.txt").unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
        "*.tmp\n/b.txt\n"
    );
    // a pattern that is already present is not added again
    gl::append_gitignore(&dir, "/b.txt").unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
        "*.tmp\n/b.txt\n"
    );
}

#[test]
fn gitignored_untracked_file_leaves_worktree_changes() {
    let dir = fixture_repo("ctx-ignore-file");
    write(&dir, "p/secret.txt", "top secret\n");
    assert!(gl::worktree_changes(&dir)
        .iter()
        .any(|f| f.path == "p/secret.txt"));
    gl::append_gitignore(&dir, "/p/secret.txt").unwrap();
    assert!(gl::worktree_changes(&dir)
        .iter()
        .all(|f| f.path != "p/secret.txt"));
}

#[test]
fn gitignored_directory_covers_whole_tree() {
    let dir = fixture_repo("ctx-ignore-dir");
    write(&dir, "p/a.txt", "a\n");
    write(&dir, "p/sub/b.txt", "b\n");
    write(&dir, "q/c.txt", "c\n");
    gl::append_gitignore(&dir, "/p/").unwrap();
    let paths: Vec<String> = gl::worktree_changes(&dir)
        .into_iter()
        .map(|f| f.path)
        .collect();
    assert!(!paths.iter().any(|p| p.starts_with("p/")));
    assert!(paths.iter().any(|p| p == "q/c.txt"));
}

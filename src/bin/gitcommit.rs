//! gitcommit: a commit window for a git repository (see dev/commit-concept.md).
//!
//! Two vertically stacked panes - a commit message text area with an
//! "Amend last commit" checkbox on top, and a checkable file list (stage
//! checkbox, name, status, +/- lines) below - plus Cancel/Commit buttons.
//! Double-clicking a file opens its changes (HEAD vs. working tree) in meld,
//! the same way gitlog's file list opens the changes of a commit.

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

extern crate gtk4 as gtk;

use gtk::prelude::*;
use rust_i18n::t;

rust_i18n::i18n!("locales", fallback = "en");

use gitlog::gitlog as gl;
use gitlog::i18n;


const W_STAGE: u32 = 0;
const W_NAME: u32 = 1;
const W_STATUS: u32 = 2;
const W_ADDED: u32 = 3;
const W_DELETED: u32 = 4;
const W_PATH: u32 = 5; // path for git operations (hidden)
const W_IN_HEAD: u32 = 6; // file exists in the previous commit (hidden)
const W_STAGED: u32 = 7; // file is already in the index (hidden)

/// Localized label for a working-tree change; "new" instead of "added"
/// because untracked files are meant to be added by this commit.
fn status_label(action: gl::Action) -> String {
    match action {
        gl::Action::Added => t!("status.new").into_owned(),
        gl::Action::Modified => t!("action.modified").into_owned(),
        gl::Action::Deleted => t!("action.deleted").into_owned(),
        gl::Action::Renamed => t!("action.renamed").into_owned(),
    }
}

/// Line count for the +/- columns; `None` marks a binary file.
fn num_str(n: Option<i64>) -> String {
    match n {
        Some(0) => "0".into(),
        Some(n) => n.to_string(),
        None => "•".into(),
    }
}

/// Writes the content of `path` at `rev` to `dest` (empty file if it does
/// not exist there, i.e. the file was added or deleted on that side).
fn write_blob(repo: &Path, rev: &str, path: &str, dest: &Path) {
    let Ok(out) = std::process::Command::new("git")
        .args(["-C", &repo.to_string_lossy().into_owned(), "show", &format!("{rev}:{path}")])
        .output()
    else {
        return;
    };
    let _ = std::fs::write(dest, &out.stdout);
}

/// File rows for amend mode: the files of the previous commit merged with
/// the working-copy changes; everything is pre-checked (a file the user
/// unchecks leaves the amended commit).
fn amend_files(repo: &Path) -> Vec<gl::WorkFile> {
    let mut files = gl::head_files(repo).unwrap_or_default();
    for w in gl::worktree_changes(repo) {
        // a rename row carries the *new* path: also match the head row of
        // the old path, otherwise the file appears twice (once from the
        // previous commit, once as "old => new") and committing fails
        // because the old path no longer exists
        let old_path = w.display_name.split("=>").next().unwrap_or("").trim();
        let is_rename = w.action == gl::Action::Renamed;
        match files
            .iter_mut()
            .find(|f| f.path == w.path || (is_rename && f.path == old_path))
        {
            // changed in the work tree: keep the "in head" flag, show the
            // current diff against HEAD
            Some(h) => {
                h.path = w.path.clone();
                h.display_name = w.display_name;
                h.action = w.action;
                h.added = w.added;
                h.deleted = w.deleted;
            }
            None => files.push(w),
        }
    }
    files
}

fn add_file_rows(
    store: &gtk::ListStore,
    files: &[gl::WorkFile],
    precheck: impl Fn(&gl::WorkFile) -> bool,
) {
    store.clear();
    for f in files {
        let iter = store.append();
        store.set(
            &iter,
            &[
                (W_STAGE, &precheck(f)),
                (W_NAME, &f.display_name),
                (W_STATUS, &status_label(f.action)),
                (W_ADDED, &num_str(f.added)),
                (W_DELETED, &num_str(f.deleted)),
                (W_PATH, &f.path),
                (W_IN_HEAD, &f.in_head),
                (W_STAGED, &f.staged),
            ],
        );
    }
}

/// Full visible text of a text buffer.
fn buffer_text(buffer: &gtk::TextBuffer) -> String {
    buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), false)
        .to_string()
}

type Row = (String, bool, bool, bool); // path, checked, in_head, staged

fn read_rows(store: &gtk::ListStore) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut iter = store.iter_first();
    while let Some(mut i) = iter {
        rows.push((
            store.get(&i, W_PATH as i32),
            store.get(&i, W_STAGE as i32),
            store.get(&i, W_IN_HEAD as i32),
            store.get(&i, W_STAGED as i32),
        ));
        iter = store.iter_next(&mut i).then_some(i);
    }
    rows
}

/// Applies the checkbox selection to the index and creates the commit.
/// On error the index may be partially updated, which the user can inspect
/// and repair; the window stays open.
fn commit_checked(repo: &Path, rows: &[Row], amend: bool, message: &str) -> Result<(), String> {
    for (path, checked, in_head, staged) in rows {
        if amend {
            if *in_head && !*checked {
                // remove from the amended commit, keep the work tree file
                gl::rm_cached_path(repo, path)?;
            } else if *checked {
                gl::stage_path(repo, path)?;
            }
        } else if *checked && !*staged {
            gl::stage_path(repo, path)?;
        } else if !*checked && *staged {
            gl::unstage_path(repo, path)?;
        }
    }
    if !amend && gl::index_matches_head(repo) {
        return Err(t!("error.nothing_staged").into_owned());
    }
    gl::do_commit(repo, message, amend)
}

fn error_dialog(parent: &gtk::Window, text: &str) {
    let dialog = gtk::MessageDialog::builder()
        .transient_for(parent)
        .message_type(gtk::MessageType::Error)
        .text(text)
        .buttons(gtk::ButtonsType::Ok)
        .build();
    dialog.set_property("modal", &true);
    dialog.connect_response(|dialog, _response| dialog.close());
    dialog.show();
}

fn main() -> ExitCode {
    // no argument: commit in the current folder (e.g. launched from a
    // terminal that is already inside the project)
    let folder = env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let repo = match PathBuf::from(&folder).canonicalize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("gitcommit: cannot open {folder}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if !gl::is_repo(&repo) {
        eprintln!("gitcommit: {folder} is not a git repository");
        return ExitCode::FAILURE;
    }

    // UI language: detected once from the system locale (as in gitlog)
    let locale = i18n::detect_locale();
    rust_i18n::set_locale(locale);

    let app = gtk::Application::builder()
        .application_id("dev.example.gitcommit")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |app| {
        // Thicker scrollbars: the default theme's 6px overlay sliders are
        // nearly impossible to grab (same as in gitlog).
        if let Some(display) = gtk::gdk::Display::default() {
            let provider = gtk::CssProvider::new();
            provider.load_from_data(
                "scrollbar.horizontal slider { min-height: 12px; margin: 2px; }
                 scrollbar.vertical slider { min-width: 12px; margin: 2px; }
                 .commit-buttons { margin: 6px; }",
            );
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let branch = gl::head_info(&repo);
        let window = gtk::Window::builder()
            .title(t!("commit.title", branch = &branch).as_ref())
            .default_width(720)
            .default_height(560)
            .build();
        window.set_application(Some(app));

        // ---- file list -----------------------------------------------------
        let files_store = gtk::ListStore::new(&[
            glib::Type::BOOL,
            glib::Type::STRING,
            glib::Type::STRING,
            glib::Type::STRING,
            glib::Type::STRING,
            glib::Type::STRING,
            glib::Type::BOOL,
            glib::Type::BOOL,
        ]);
        let files_tree = gtk::TreeView::builder().model(&files_store).build();

        let toggle = gtk::CellRendererToggle::new();
        let stage_col = gtk::TreeViewColumn::builder().title("").build();
        stage_col.pack_start(&toggle, false);
        stage_col.add_attribute(&toggle, "active", W_STAGE as i32);
        stage_col.set_sizing(gtk::TreeViewColumnSizing::Fixed);
        stage_col.set_fixed_width(28);
        files_tree.append_column(&stage_col);

        let name_cell = gtk::CellRendererText::new();
        name_cell.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let name_col = gtk::TreeViewColumn::builder().title(t!("file.name").as_ref()).build();
        name_col.pack_start(&name_cell, true);
        name_col.add_attribute(&name_cell, "text", W_NAME as i32);
        name_col.set_expand(true);
        // fixed base width so long paths do not push the other columns
        // off-screen (same approach as gitlog)
        name_col.set_sizing(gtk::TreeViewColumnSizing::Fixed);
        name_col.set_fixed_width(320);
        files_tree.append_column(&name_col);

        for (col, title) in [
            (W_STATUS, t!("col.status")),
            (W_ADDED, t!("file.added")),
            (W_DELETED, t!("file.deleted")),
        ] {
            let cell = gtk::CellRendererText::new();
            let column = gtk::TreeViewColumn::builder().title(title.as_ref()).build();
            column.pack_start(&cell, false);
            column.add_attribute(&cell, "text", col as i32);
            column.set_sizing(gtk::TreeViewColumnSizing::Fixed);
            column.set_fixed_width(90);
            column.set_alignment(1.0);
            files_tree.append_column(&column);
        }

        let files_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .build();
        files_scroll.set_overlay_scrolling(false);
        files_scroll.set_child(Some(&files_tree));

        // ---- double-click a file: open its changes in meld ----------------------
        // The list shows the working tree against HEAD (that is also where
        // the +/- counts come from), so meld compares the HEAD version with
        // the current working-tree file (same pattern as gitlog's file list).
        {
            let store = files_store.clone();
            let repo = repo.clone();
            files_tree.connect_row_activated(move |_tree, path, _column| {
                let Some(iter) = store.iter(path) else {
                    return;
                };
                let name: String = store.get(&iter, W_NAME as i32);
                // rename rows are displayed as "old => new"
                let new_path = name.rsplit("=>").next().unwrap_or(&name).trim();
                let old_path = name.split("=>").next().unwrap_or(&name).trim();
                // temp files per process; left in the temp dir for the
                // lifetime of the meld instance
                let dir = std::env::temp_dir()
                    .join(format!("gitcommit-{}", std::process::id()));
                if std::fs::create_dir_all(&dir).is_err() {
                    return;
                }
                // flatten the full path so different files with the same
                // base name do not overwrite each other's temp copies
                let tag: String = new_path
                    .chars()
                    .map(|c| if c == '/' { '-' } else { c })
                    .collect();
                let old_file = dir.join(format!("{tag}.old"));
                let new_file = dir.join(format!("{tag}.new"));
                // HEAD version (empty for files not in HEAD, e.g. new files)
                write_blob(&repo, "HEAD", old_path, &old_file);
                // working-tree version (empty for deleted files)
                let _ = std::fs::write(
                    &new_file,
                    std::fs::read(repo.join(new_path)).unwrap_or_default(),
                );
                if let Err(e) = std::process::Command::new("meld")
                    .args([&old_file, &new_file])
                    .spawn()
                {
                    eprintln!("gitcommit: cannot open meld: {e}");
                }
            });
        }

        // ---- message pane + amend checkbox -----------------------------------
        let message_view = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::Word)
            .left_margin(8)
            .right_margin(8)
            .top_margin(8)
            .build();
        // vexpand so the text area fills the top pane (the scrolled window
        // otherwise only requests the text view's minimal height)
        let msg_scroll = gtk::ScrolledWindow::builder().vexpand(true).build();
        msg_scroll.set_child(Some(&message_view));

        let amend_check = gtk::CheckButton::builder()
            .label(t!("commit.amend").as_ref())
            .margin_start(8)
            .margin_bottom(4)
            .build();
        if !gl::has_head(&repo) {
            // nothing to amend yet (empty repository)
            amend_check.set_sensitive(false);
        }

        let top_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        top_box.append(&msg_scroll);
        top_box.append(&amend_check);

        // ---- buttons -----------------------------------------------------------
        let cancel_button = gtk::Button::builder().label(t!("commit.cancel").as_ref()).build();
        let commit_button = gtk::Button::builder().label(t!("commit.commit").as_ref()).build();
        commit_button.add_css_class("suggested-action");
        let button_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        // the `margin` property was removed in GTK 4.18; use CSS instead
        button_box.add_css_class("commit-buttons");
        button_box.set_halign(gtk::Align::End);
        button_box.append(&cancel_button);
        button_box.append(&commit_button);

        let paned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        paned.set_start_child(Some(&top_box));
        paned.set_end_child(Some(&files_scroll));

        let outer = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        outer.append(&paned);
        outer.append(&button_box);
        window.set_child(Some(&outer));
        paned.set_position(200);

        // ---- file list behaviour ------------------------------------------------
        // initial list: working-copy changes vs HEAD, pre-checked if staged
        add_file_rows(&files_store, &gl::worktree_changes(&repo), |f| f.staged);

        {
            let store = files_store.clone();
            toggle.connect_toggled(move |_cell, path| {
                if let Some(row) = store.iter(&path) {
                    let checked: bool = store.get(&row, W_STAGE as i32);
                    store.set(&row, &[(W_STAGE, &!checked)]);
                }
            });
        }

        {
            let store = files_store.clone();
            let buffer = message_view.buffer().clone();
            let repo = repo.clone();
            amend_check.connect_toggled(move |check| {
                if check.is_active() {
                    // pre-fill the previous commit's message, but never
                    // overwrite text the user already typed
                    if buffer_text(&buffer).trim().is_empty() {
                        if let Some(m) = gl::head_message(&repo) {
                            buffer.set_text(&m);
                        }
                    }
                    add_file_rows(&store, &amend_files(&repo), |_| true);
                } else {
                    // back to a normal commit: drop the pre-filled message
                    buffer.set_text("");
                    add_file_rows(&store, &gl::worktree_changes(&repo), |f| f.staged);
                }
            });
        }

        // Commit is only enabled while the message is non-empty
        {
            let buffer = message_view.buffer().clone();
            let button = commit_button.clone();
            let apply = move |b: &gtk::TextBuffer| {
                button.set_sensitive(!buffer_text(b).trim().is_empty());
            };
            apply(&buffer);
            buffer.connect_changed(move |b| apply(b));
        }

        // ---- cancel / commit ------------------------------------------------------
        {
            let store = files_store.clone();
            let amend_check = amend_check.clone();
            let buffer = message_view.buffer().clone();
            let repo = repo.clone();
            let win = window.clone();
            commit_button.connect_clicked(move |_| {
                let message = buffer_text(&buffer);
                if message.trim().is_empty() {
                    error_dialog(&win, &t!("error.empty_message"));
                    return;
                }
                let rows = read_rows(&store);
                let amend = amend_check.is_active();
                match commit_checked(&repo, &rows, amend, &message) {
                    Ok(()) => win.close(),
                    Err(e) => error_dialog(&win, &t!("error.commit_failed", error = &e)),
                }
            });
        }
        {
            let win = window.clone();
            cancel_button.connect_clicked(move |_| win.close());
        }

        window.show();
    });

    // run with only the program name: the folder argument must not be
    // interpreted by GApplication as a file to open (same as gitlog)
    let prog = env::args().next().unwrap_or_default();
    ExitCode::from(app.run_with_args(&[prog]).get())
}

#[cfg(test)]
mod commit_title_tests {
    // Verify the new strings resolve in every locale without touching the
    // global locale (same pattern as the gitlog i18n tests).

    #[test]
    fn commit_title_resolves_in_every_locale() {
        let branch = "main";
        assert_eq!(rust_i18n::t!("commit.title", locale = "en", branch = &branch), "git commit - main");
        assert_eq!(rust_i18n::t!("commit.title", locale = "de", branch = &branch), "git commit - main");
        assert_eq!(rust_i18n::t!("commit.title", locale = "fr", branch = &branch), "git commit - main");
        assert_eq!(rust_i18n::t!("commit.title", locale = "es", branch = &branch), "git commit - main");
        assert_eq!(rust_i18n::t!("commit.title", locale = "it", branch = &branch), "git commit - main");
        assert_eq!(rust_i18n::t!("commit.title", locale = "nl", branch = &branch), "git commit - main");
        assert_eq!(rust_i18n::t!("commit.title", locale = "pt", branch = &branch), "git commit - main");
    }
}

#[cfg(test)]
mod amend_files_tests {
    use super::*;
    use std::process::Command;

    fn test_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gitcommit-test-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn git(repo: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-C", &repo.to_string_lossy()])
            .args(args)
            .output()
            .expect("failed to run git");
        assert!(
            out.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn amend_files_merges_head_and_worktree() {
        let dir = test_dir("merge");
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.email", "t@t.t"]);
        git(&dir, &["config", "user.name", "Tester"]);
        std::fs::write(dir.join("a.txt"), "line1\nline2\n").unwrap();
        std::fs::write(dir.join("b.txt"), "x\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "first"]);

        // working copy: a.txt +1 line, new.txt untracked
        std::fs::write(dir.join("a.txt"), "line1\nline2\nline3\n").unwrap();
        std::fs::write(dir.join("new.txt"), "fresh\n").unwrap();

        let files = amend_files(&dir);
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
        let dir = test_dir("merge-rename");
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.email", "t@t.t"]);
        git(&dir, &["config", "user.name", "Tester"]);
        std::fs::write(dir.join("x.txt"), "line1\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "first"]);

        // rename after the commit: must show as a single "old => new" row,
        // not as a head row for the old path plus a rename row
        git(&dir, &["mv", "x.txt", "y.txt"]);
        let files = amend_files(&dir);
        assert_eq!(files.len(), 1);
        let r = &files[0];
        assert_eq!(r.path, "y.txt");
        assert_eq!(r.display_name, "x.txt => y.txt");
        assert_eq!(r.action, gl::Action::Renamed);
        assert!(r.in_head);
    }

    #[test]
    fn amend_rename_commit_flow() {
        let dir = test_dir("rename-flow");
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.email", "t@t.t"]);
        git(&dir, &["config", "user.name", "Tester"]);
        // parent commit so the amended commit's diff shows the rename
        std::fs::write(dir.join("z.txt"), "z\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "first"]);
        std::fs::write(dir.join("x.txt"), "line1\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "second"]);
        git(&dir, &["mv", "x.txt", "y.txt"]);

        // the single merged row is checked; committing must not fail on the
        // old path anymore
        let files = amend_files(&dir);
        assert_eq!(files.len(), 1);
        commit_checked(
            &dir,
            &[(files[0].path.clone(), true, files[0].in_head, files[0].staged)],
            true,
            "amended with rename",
        )
        .unwrap();
        let head = gl::head_files(&dir).expect("head_files after amend");
        assert_eq!(head.len(), 1);
        assert_eq!(head[0].path, "y.txt");
        // x.txt only existed in the amended-away commit, so relative to the
        // parent the file is a plain add (no rename to show)
        assert_eq!(head[0].action, gl::Action::Added);
    }
}

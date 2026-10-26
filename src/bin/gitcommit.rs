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

/// Appends one row per file to the file list; `precheck` decides the initial
/// state of the stage checkbox.
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

fn read_rows(store: &gtk::ListStore) -> Vec<gl::StageRow> {
    let mut rows = Vec::new();
    let mut iter = store.iter_first();
    while let Some(mut i) = iter {
        rows.push(gl::StageRow {
            path: store.get(&i, W_PATH as i32),
            checked: store.get(&i, W_STAGE as i32),
            in_head: store.get(&i, W_IN_HEAD as i32),
            staged: store.get(&i, W_STAGED as i32),
        });
        iter = store.iter_next(&mut i).then_some(i);
    }
    rows
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
                    add_file_rows(&store, &gl::amend_files(&repo), |_| true);
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
                match gl::commit_checked(&repo, &rows, amend, &message) {
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

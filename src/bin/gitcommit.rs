//! gitcommit: a commit window for a git repository (see dev/commit-concept.md).
//!
//! Two vertically stacked panes - a commit message text area with an
//! "Amend last commit" checkbox on top, and a checkable file list (stage
//! checkbox, name, status, +/- lines) below - plus Cancel/Commit buttons.
//! Double-clicking a file opens its changes (HEAD vs. working tree) in meld,
//! the same way gitlog's file list opens the changes of a commit. Spreadsheets
//! (.ods, .xlsx, ...) use LibreOffice Calc's built-in Compare Sheets feature
//! instead, because meld cannot diff their binary format.

use std::cell::Cell;
use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;

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
const W_OLD_PATH: u32 = 8; // old path of rename rows, "" otherwise (hidden)

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

/// Writes the content of `path` at `rev` to `dest`; false when the file does
/// not exist at that revision (i.e. it was added or deleted on that side).
fn write_blob(repo: &Path, rev: &str, path: &str, dest: &Path) -> bool {
    let Ok(out) = std::process::Command::new("git")
        .args(["-C", &repo.to_string_lossy().into_owned(), "show", &format!("{rev}:{path}")])
        .output()
    else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    std::fs::write(dest, &out.stdout).is_ok()
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
        // rename rows must know the old path to undo the rename when the
        // user unchecks them while amending
        let old_path = if f.action == gl::Action::Renamed {
            f.display_name.split("=>").next().unwrap_or("").trim().to_string()
        } else {
            String::new()
        };
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
                (W_OLD_PATH, &old_path),
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

/// Re-read the working tree into the file list, keeping the checkbox
/// selections the user made for files that are still changed. Used after
/// external edits (meld) so status and +/- counts match the files on disk.
fn refresh_file_rows(store: &gtk::ListStore, repo: &Path) {
    let checked: HashSet<String> = read_rows(store)
        .into_iter()
        .filter(|r| r.checked)
        .map(|r| r.path)
        .collect();
    add_file_rows(store, &gl::worktree_changes(repo), |f| {
        f.staged || checked.contains(&f.path)
    });
}

fn read_rows(store: &gtk::ListStore) -> Vec<gl::StageRow> {
    let mut rows = Vec::new();
    let mut iter = store.iter_first();
    while let Some(mut i) = iter {
        let old: String = store.get(&i, W_OLD_PATH as i32);
        rows.push(gl::StageRow {
            old_path: (!old.is_empty()).then_some(old),
            path: store.get(&i, W_PATH as i32),
            checked: store.get(&i, W_STAGE as i32),
            in_head: store.get(&i, W_IN_HEAD as i32),
            staged: store.get(&i, W_STAGED as i32),
        });
        iter = store.iter_next(&mut i).then_some(i);
    }
    rows
}

/// Visible + hidden data of one file-list row; used to re-sort the list.
#[derive(Clone)]
struct RowData {
    stage: bool,
    name: String,
    status: String,
    added: String,
    deleted: String,
    path: String,
    in_head: bool,
    staged: bool,
    old_path: String,
}

fn read_row_data(store: &gtk::ListStore) -> Vec<RowData> {
    let mut rows = Vec::new();
    let mut iter = store.iter_first();
    while let Some(mut i) = iter {
        rows.push(RowData {
            stage: store.get(&i, W_STAGE as i32),
            name: store.get(&i, W_NAME as i32),
            status: store.get(&i, W_STATUS as i32),
            added: store.get(&i, W_ADDED as i32),
            deleted: store.get(&i, W_DELETED as i32),
            path: store.get(&i, W_PATH as i32),
            in_head: store.get(&i, W_IN_HEAD as i32),
            staged: store.get(&i, W_STAGED as i32),
            old_path: store.get(&i, W_OLD_PATH as i32),
        });
        iter = store.iter_next(&mut i).then_some(i);
    }
    rows
}

fn write_row_data(store: &gtk::ListStore, rows: &[RowData]) {
    store.clear();
    for r in rows {
        let iter = store.append();
        store.set(
            &iter,
            &[
                (W_STAGE, &r.stage),
                (W_NAME, &r.name),
                (W_STATUS, &r.status),
                (W_ADDED, &r.added),
                (W_DELETED, &r.deleted),
                (W_PATH, &r.path),
                (W_IN_HEAD, &r.in_head),
                (W_STAGED, &r.staged),
                (W_OLD_PATH, &r.old_path),
            ],
        );
    }
}

/// Numeric value of an added/deleted cell; "•" (binary file) has no count
/// and sorts before all numbers.
fn parse_num(s: &str) -> i64 {
    s.parse().unwrap_or(i64::MIN)
}

/// Re-sort the file list in place by a store column: name and status compare
/// alphabetically, added/deleted numerically. Ties keep the incoming (git)
/// order, so descending is a reversed stable sort done via a flipped compare.
fn sort_store(store: &gtk::ListStore, col: u32, ascending: bool) {
    let cmp: fn(&RowData, &RowData) -> std::cmp::Ordering = match col {
        W_NAME => |a, b| a.name.cmp(&b.name),
        W_STATUS => |a, b| a.status.cmp(&b.status),
        W_ADDED => |a, b| parse_num(&a.added).cmp(&parse_num(&b.added)),
        W_DELETED => |a, b| parse_num(&a.deleted).cmp(&parse_num(&b.deleted)),
        _ => return,
    };
    let mut rows = read_row_data(store);
    rows.sort_by(|a, b| {
        if ascending {
            cmp(a, b)
        } else {
            cmp(b, a)
        }
    });
    write_row_data(store, &rows);
}

/// The sort column can change while rows are replaced (refresh, amend
/// toggle); re-apply the current sort so the display order is stable. The
/// sort indicator on the column header is unaffected by store changes.
fn apply_sort(store: &gtk::ListStore, state: &Cell<Option<(u32, bool)>>) {
    if let Some((col, ascending)) = state.get() {
        sort_store(store, col, ascending);
    }
}

/// True when the list is non-empty and every row is checked.
fn rows_all_checked(store: &gtk::ListStore) -> bool {
    let Some(mut i) = store.iter_first() else {
        return false;
    };
    loop {
        let checked: bool = store.get(&i, W_STAGE as i32);
        if !checked {
            return false;
        }
        // iter_next advances in place and returns false at the end
        if !store.iter_next(&mut i) {
            return true;
        }
    }
}

/// Sync the stage column's header checkbox after row changes; the header
/// widget is display-only (see the column setup), so no signals to block.
fn sync_head_checkbox(store: &gtk::ListStore, head: &gtk::CheckButton) {
    head.set_active(rows_all_checked(store));
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
            glib::Type::STRING,
        ]);
        let files_tree = gtk::TreeView::builder().model(&files_store).build();
        // current sort of the file list: (store column, ascending); the
        // list opens sorted by file name ascending, the column-header click
        // handlers below change it and refreshes re-apply it
        let sort_state: Rc<Cell<Option<(u32, bool)>>> =
            Rc::new(Cell::new(Some((W_NAME, true))));

        let toggle = gtk::CellRendererToggle::new();
        let stage_col = gtk::TreeViewColumn::builder().title("").build();
        stage_col.pack_start(&toggle, false);
        stage_col.add_attribute(&toggle, "active", W_STAGE as i32);
        stage_col.set_sizing(gtk::TreeViewColumnSizing::Fixed);
        stage_col.set_fixed_width(28);
        // Header checkbox. The TreeView header swallows mouse clicks (a
        // widget inside the header never receives them), so the check button
        // is a pure "all rows checked" indicator and the actual toggling is
        // done by the stage column's own clicked signal (set_clickable); in
        // GTK 4's TreeView a header click cannot sort the list by itself
        // (sorting needs an explicitly attached sorter)
        let head_toggle = gtk::CheckButton::new();
        head_toggle.set_can_target(false);
        stage_col.set_widget(Some(&head_toggle));
        stage_col.set_clickable(true);
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

        // sortable columns: (widget, store column) for the handlers below
        let mut sort_cols = vec![(name_col, W_NAME)];
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
            // NOTE: no set_alignment(1.0): GTK draws the header sort arrow
            // on the side opposite to the column alignment, so keeping all
            // columns left-aligned puts every arrow on the right
            files_tree.append_column(&column);
            sort_cols.push((column, col));
        }

        let files_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .build();
        files_scroll.set_overlay_scrolling(false);
        files_scroll.set_child(Some(&files_tree));

        // ---- double-click a file: open its changes in meld (or Calc) ----------
        // The list shows the working tree against HEAD (that is also where
        // the +/- counts come from), so the tools compare the HEAD version
        // with the current working-tree file (same pattern as gitlog's
        // file list). Spreadsheets go to LibreOffice Calc's Compare Sheets.
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
                // Only the HEAD side needs a temporary copy; the right side
                // of the comparison is the real working-tree file, so edits
                // made (and saved) in meld land in the repository.
                let dir = std::env::temp_dir()
                    .join(format!("gitcommit-{}", std::process::id()));
                if std::fs::create_dir_all(&dir).is_err() {
                    return;
                }
                // flatten the full path so different files with the same
                // base name do not overwrite each other's temp copies; keep
                // the extension so meld picks the right highlighting
                let tag: String = new_path
                    .chars()
                    .map(|c| if c == '/' { '-' } else { c })
                    .collect();
                let head_file = match Path::new(new_path).extension().and_then(|e| e.to_str()) {
                    Some(ext) => dir.join(format!("{tag}.head.{ext}")),
                    None => dir.join(format!("{tag}.head")),
                };
                // HEAD version (missing for files not in HEAD, e.g. new files)
                let in_head = write_blob(&repo, "HEAD", old_path, &head_file);
                // the real file; for deleted files meld creates it when the
                // user saves, which restores it in the working tree
                let work_file = repo.join(new_path);
                if gl::is_spreadsheet(new_path) {
                    // Calc's compare needs two real spreadsheets; added or
                    // deleted rows only have one side, so just open that one
                    let opened = match (in_head, work_file.exists()) {
                        (true, true) => gl::compare_with_calc(&work_file, &head_file),
                        (true, false) => gl::open_in_calc(&head_file),
                        _ => gl::open_in_calc(&work_file),
                    };
                    if let Err(e) = opened {
                        eprintln!("gitcommit: {e}");
                    }
                    return;
                }
                if let Err(e) = std::process::Command::new("meld")
                    .args([&head_file, &work_file])
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
        apply_sort(&files_store, &sort_state);

        // ---- sortable columns ----------------------------------------------
        // Clicking a column header sorts the list by that column (name and
        // status alphabetically, +/- numerically); clicking the same header
        // again reverses the order. GTK 4's TreeView has no built-in header
        // sorting, so the handlers re-order the ListStore itself (see
        // sort_store) and only use the column for the click signal and the
        // sort arrow in the header.
        let all_cols: Rc<Vec<gtk::TreeViewColumn>> =
            Rc::new(sort_cols.iter().map(|(c, _)| c.clone()).collect());
        for (column, col_idx) in sort_cols {
            column.set_clickable(true);
            // the initial sort (file name ascending) gets its header arrow
            if col_idx == W_NAME {
                column.set_sort_indicator(true);
                column.set_sort_order(gtk::SortType::Ascending);
            }
            let store = files_store.clone();
            let state = sort_state.clone();
            let others = all_cols.clone();
            column.connect_clicked(move |col| {
                let ascending = match state.get() {
                    Some((c, asc)) if c == col_idx => !asc,
                    _ => true,
                };
                state.set(Some((col_idx, ascending)));
                sort_store(&store, col_idx, ascending);
                // only the active sort column shows the header arrow
                for c in others.iter() {
                    c.set_sort_indicator(false);
                }
                col.set_sort_indicator(true);
                col.set_sort_order(if ascending {
                    gtk::SortType::Ascending
                } else {
                    gtk::SortType::Descending
                });
            });
        }

        // clicking the (small) stage column header checks all rows, or
        // unchecks all when every row was already checked; store value
        // changes never re-emit CellRendererToggle::toggled, so the row
        // handler below is unaffected
        {
            let store = files_store.clone();
            let head = head_toggle.clone();
            stage_col.connect_clicked(move |_col| {
                if store.iter_first().is_none() {
                    head.set_active(false);
                    return;
                }
                let on = !rows_all_checked(&store);
                if let Some(mut i) = store.iter_first() {
                    loop {
                        store.set(&i, &[(W_STAGE, &on)]);
                        if !store.iter_next(&mut i) {
                            break;
                        }
                    }
                }
                head.set_active(on);
            });
        }

        {
            let store = files_store.clone();
            let head = head_toggle.clone();
            toggle.connect_toggled(move |_cell, path| {
                if let Some(row) = store.iter(&path) {
                    let checked: bool = store.get(&row, W_STAGE as i32);
                    store.set(&row, &[(W_STAGE, &!checked)]);
                }
                sync_head_checkbox(&store, &head);
            });
        }

        {
            let store = files_store.clone();
            let buffer = message_view.buffer().clone();
            let repo = repo.clone();
            let head = head_toggle.clone();
            let sort = sort_state.clone();
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
                apply_sort(&store, &sort);
                sync_head_checkbox(&store, &head);
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

        {
            let store = files_store.clone();
            let amend_check = amend_check.clone();
            let repo = repo.clone();
            let head = head_toggle.clone();
            let sort = sort_state.clone();
            // meld writes into the working tree, so refresh the row data
            // (status, +/- counts) whenever the window regains focus
            let focus = gtk::EventControllerFocus::new();
            focus.connect_enter(move |_| {
                if !amend_check.is_active() {
                    refresh_file_rows(&store, &repo);
                }
                apply_sort(&store, &sort);
                sync_head_checkbox(&store, &head);
            });
            window.add_controller(focus);
        }

        // header checkbox reflects the initial row states
        sync_head_checkbox(&files_store, &head_toggle);
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

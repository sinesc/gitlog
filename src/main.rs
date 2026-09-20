use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

extern crate gtk4 as gtk;

use gtk::glib;
use gtk::prelude::*;
use rust_i18n::t;

rust_i18n::i18n!("locales", fallback = "en");

use gitlog::gitlog as gl;
use gitlog::i18n;

const LOG_HASH: u32 = 0;
const LOG_SUBJECT: u32 = 1;
const LOG_COMMITTER: u32 = 2;
const LOG_DATE: u32 = 3;
const LOG_IDX: u32 = 4; // index into the commit cache (hidden)
const LOG_TIP: u32 = 5; // 1 if the commit is a branch tip (hidden)

const F_NAME: u32 = 0;
const F_ADDED: u32 = 1;
const F_DELETED: u32 = 2;
const F_SIZE: u32 = 3;
const F_ACTION: u32 = 4;

type SizeCache = HashMap<String, HashMap<String, u64>>;

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// Renames are displayed as "old => new"; the tree only knows the new path.
fn lookup_path(display_name: &str) -> &str {
    display_name.rsplit("=>").next().unwrap_or(display_name).trim()
}

/// The pre-rename path of a rename ("a => b" -> "a"); "b" otherwise.
fn lookup_old_path(display_name: &str) -> &str {
    display_name.split("=>").next().unwrap_or(display_name).trim()
}

/// Writes the blob of `path` at `rev` to `dest` (empty file if it does not
/// exist there, i.e. the file was added or deleted at that side).
fn write_blob(repo: &std::path::Path, rev: &str, path: &str, dest: &std::path::Path) {
    let Ok(out) = std::process::Command::new("git")
        .args(["-C", &repo.to_string_lossy().into_owned(), "show", &format!("{rev}:{path}")])
        .output()
    else {
        return;
    };
    let _ = std::fs::write(dest, &out.stdout);
}

fn numstat(n: Option<i64>) -> String {
    match n {
        Some(0) => "0".into(),
        Some(n) => n.to_string(),
        None => "•".into(), // binary file
    }
}

/// Localized label for a file change action, looked up in the locale files.
fn action_label(action: gl::Action) -> String {
    match action {
        gl::Action::Added => t!("action.added").into_owned(),
        gl::Action::Modified => t!("action.modified").into_owned(),
        gl::Action::Deleted => t!("action.deleted").into_owned(),
        gl::Action::Renamed => t!("action.renamed").into_owned(),
    }
}

fn add_text_column(
    tree: &gtk::TreeView,
    title: &str,
    col: u32,
    expand: bool,
    ellipsize: bool,
    cell: &gtk::CellRendererText,
) {
    let column = gtk::TreeViewColumn::builder().title(title).build();
    if ellipsize {
        // keep long text from pushing the other columns off-screen
        cell.set_ellipsize(gtk::pango::EllipsizeMode::End);
    }
    column.pack_start(cell, !expand);
    column.add_attribute(cell, "text", col as i32);
    column.set_expand(expand);
    tree.append_column(&column);
}

/// Expanding text columns use a fixed base width (grow, never shrink the
/// neighbours) instead of their natural (full-text) width.
fn make_expanding(tree: &gtk::TreeView, col: u32, base_width: i32) {
    if let Some(c) = tree.columns().get(col as usize) {
        c.set_sizing(gtk::TreeViewColumnSizing::Fixed);
        c.set_fixed_width(base_width);
    }
}

/// Messages from background threads, drained on the main loop.
enum Msg {
    Commit(gl::Commit),
    Sizes { hash: String, map: HashMap<String, u64> },
}

struct Ui {
    commits: Arc<Mutex<Vec<gl::Commit>>>,
    sizes: Arc<Mutex<SizeCache>>,
    selected_hash: Arc<Mutex<Option<String>>>,
    log_store: gtk::ListStore,
    log_tree: gtk::TreeView,
    // cell of the message column, kept so branch tips can be coloured
    log_subject_cell: gtk::CellRendererText,
    files_store: gtk::ListStore,
    files_tree: gtk::TreeView,
    message_view: gtk::TextView,
    repo: PathBuf,
    locale: &'static str,
}

impl Ui {
    fn insert_commit(&self, commit: gl::Commit) {
        let idx = self.commits.lock().unwrap().len() as i64;
        let iter = self.log_store.append();
        // mark commits whose committer differs from the author (bot, amend…)
        let committer = if commit.committer != commit.author {
            format!("*{}", commit.committer)
        } else {
            commit.committer.clone()
        };
        let date = i18n::format_timestamp(commit.timestamp, self.locale);
        // branch tips are marked in the list, e.g. "[master] [origin/master] msg"
        let subject = if commit.branches.is_empty() {
            commit.subject.clone()
        } else {
            let mut prefixed = String::new();
            for b in &commit.branches {
                prefixed.push_str(&format!("[{b}] "));
            }
            prefixed.push_str(&commit.subject);
            prefixed
        };
        self.log_store.set(
            &iter,
            &[
                (LOG_HASH, &commit.hash),
                (LOG_SUBJECT, &subject),
                (LOG_COMMITTER, &committer),
                (LOG_DATE, &date),
                (LOG_IDX, &idx),
                (LOG_TIP, &i32::from(!commit.branches.is_empty())),
            ],
        );
        self.commits.lock().unwrap().push(commit);
    }

    /// Returns `Some(hash)` if file sizes for the commit still need fetching.
    fn show_commit(&self, idx: i64) -> Option<String> {
        let Some(commit) = self.commits.lock().unwrap().get(idx as usize).cloned() else {
            return None;
        };
        *self.selected_hash.lock().unwrap() = Some(commit.hash.clone());
        // header: committer always, author only when it differs from the
        // committer, plus the short hash
        let mut text = t!("commit.committer", name = &commit.committer).into_owned();
        text.push('\n');
        if commit.committer != commit.author {
            text.push_str(&t!("commit.author", name = &commit.author));
            text.push('\n');
        }
        let short = commit.hash.get(..8).unwrap_or(&commit.hash);
        text.push_str(&t!("commit.hash", hash = short));
        text.push_str("\n\n");
        text.push_str(&commit.message);
        self.message_view.buffer().set_text(&text);

        self.files_store.clear();
        let cached = self
            .sizes
            .lock()
            .unwrap()
            .get(&commit.hash)
            .cloned()
            .unwrap_or_default();
        for f in &commit.files {
            let iter = self.files_store.append();
            let size = cached
                .get(lookup_path(&f.name))
                .map(|s| human_size(*s))
                .unwrap_or_else(|| "…".into());
            self.files_store.set(
                &iter,
                &[
                    (F_NAME, &f.name),
                    (F_ADDED, &numstat(f.added)),
                    (F_DELETED, &numstat(f.deleted)),
                    (F_SIZE, &size),
                    (F_ACTION, &action_label(f.action)),
                ],
            );
        }
        // lazily fetch file sizes for this commit (one `git ls-tree` per commit)
        let needs_sizes = !self.sizes.lock().unwrap().contains_key(&commit.hash);
        needs_sizes.then(|| commit.hash)
    }

    fn update_sizes(&self, hash: String, map: HashMap<String, u64>) {
        self.sizes.lock().unwrap().insert(hash.clone(), map.clone());
        if self.selected_hash.lock().unwrap().as_deref() != Some(hash.as_str()) {
            return;
        }
        let Some(mut iter) = self.files_store.iter_first() else {
            return;
        };
        loop {
            let name: String = self.files_store.get(&iter, F_NAME as i32);
            let size = map
                .get(lookup_path(&name))
                .map(|s| human_size(*s))
                .unwrap_or_else(|| "–".into());
            self.files_store.set(&iter, &[(F_SIZE, &size)]);
            if !self.files_store.iter_next(&mut iter) {
                break;
            }
        }
    }
}

fn main() -> ExitCode {
    let Some(folder) = env::args().nth(1) else {
        eprintln!("usage: gitlog <project-folder>");
        return ExitCode::from(2);
    };
    let repo = match PathBuf::from(&folder).canonicalize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("gitlog: cannot open {folder}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if !gl::is_repo(&repo) {
        eprintln!("gitlog: {folder} is not a git repository");
        return ExitCode::FAILURE;
    }

    // UI language: detected once from the system locale (phase 1; no UI switch)
    let locale = i18n::detect_locale();
    rust_i18n::set_locale(locale);

    let app = gtk::Application::builder()
        .application_id("dev.example.gitlog")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |app| {
        // Thicker scrollbars: the default theme's 6px overlay sliders are
        // nearly impossible to grab (they also overlap the paned handles).
        if let Some(display) = gtk::gdk::Display::default() {
            let provider = gtk::CssProvider::new();
            provider.load_from_data(
                "scrollbar.horizontal slider { min-height: 12px; margin: 2px; }
                 scrollbar.vertical slider { min-width: 12px; margin: 2px; }
                 /* visible separators above and below the middle pane */
                 .pane-separator { border-top: 2px solid @borders; border-bottom: 2px solid @borders; margin: 2px; }",
            );
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        // ---- shared state ----------------------------------------------------
        // commit channel: the only sender lives in the load thread, so the
        // Disconnected error tells us when the load is finished
        let (commit_tx, commit_rx) = std::sync::mpsc::channel::<Msg>();
        // size channel: senders are cloned into selection handlers, long-lived
        let (size_tx, size_rx) = std::sync::mpsc::channel::<Msg>();
        let ui = Arc::new(Ui {
            commits: Arc::new(Mutex::new(Vec::new())),
            sizes: Arc::new(Mutex::new(HashMap::new())),
            selected_hash: Arc::new(Mutex::new(None)),
            log_store: gtk::ListStore::new(&[
                glib::Type::STRING,
                glib::Type::STRING,
                glib::Type::STRING,
                glib::Type::STRING,
                glib::Type::I64,
                glib::Type::I32,
            ]),
            log_tree: gtk::TreeView::builder().build(),
            log_subject_cell: gtk::CellRendererText::new(),
            files_store: gtk::ListStore::new(&[
                glib::Type::STRING,
                glib::Type::STRING,
                glib::Type::STRING,
                glib::Type::STRING,
                glib::Type::STRING,
            ]),
            files_tree: gtk::TreeView::builder().build(),
            message_view: gtk::TextView::builder()
                .editable(false)
                .cursor_visible(false)
                .wrap_mode(gtk::WrapMode::Word)
                .left_margin(8)
                .right_margin(8)
                .top_margin(8)
                .build(),
            repo: repo.clone(),
            locale,
        });
        // attach models now that the stores exist
        let log_filter = gtk::TreeModelFilter::new(&ui.log_store, None);
        ui.log_tree.set_model(Some(&log_filter));
        ui.files_tree.set_model(Some(&ui.files_store));

        // ---- window & layout ---------------------------------------------------
        let repo_name = repo
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let branch = gl::head_info(&repo);
        let title = t!("title.loading", repo = &repo_name, branch = &branch);
        let window = gtk::Window::builder()
            .title(title)
            .default_width(1100)
            .default_height(760)
            .build();
        window.set_application(Some(app));

        let outer = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        let inner = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();

        // top pane: branch label + filter + log list
        let top_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        let filter_entry = gtk::SearchEntry::builder().build();
        let placeholder = t!("filter.placeholder");
        filter_entry.set_placeholder_text(Some(&placeholder));

        add_text_column(
            &ui.log_tree,
            &t!("col.hash"),
            LOG_HASH,
            false,
            false,
            &gtk::CellRendererText::new(),
        );
        if let Some(hash_col) = ui.log_tree.columns().get(0) {
            hash_col.set_sizing(gtk::TreeViewColumnSizing::Fixed);
            hash_col.set_fixed_width(90);
        }
        add_text_column(
            &ui.log_tree,
            &t!("col.message"),
            LOG_SUBJECT,
            true,
            true,
            &ui.log_subject_cell,
        );
        // branch tips are accented so they stand out in the list; the data
        // func runs before every draw, so the colour is reset for normal
        // rows (the named colour is resolved once; Adwaita's accent as fallback)
        let accent = glib::Object::new::<gtk::StyleContext>()
            .lookup_color("accent")
            .unwrap_or(gtk::gdk::RGBA::new(0.208, 0.518, 0.894, 1.0));
        if let Some(column) = ui.log_tree.columns().get(LOG_SUBJECT as usize) {
            let tip_cell = ui.log_subject_cell.clone();
            column.set_cell_data_func(
                &tip_cell,
                move |_column, cell, model, iter| {
                    let tip: i32 = model.get(iter, LOG_TIP as i32);
                    if let Some(text) = cell.downcast_ref::<gtk::CellRendererText>() {
                        match tip {
                            1 => text.set_foreground_rgba(Some(&accent)),
                            _ => text.set_foreground_rgba(None),
                        }
                    }
                },
            );
        }
        make_expanding(&ui.log_tree, LOG_SUBJECT, 320);
        add_text_column(
            &ui.log_tree,
            &t!("col.committer"),
            LOG_COMMITTER,
            false,
            false,
            &gtk::CellRendererText::new(),
        );
        add_text_column(
            &ui.log_tree,
            &t!("col.date"),
            LOG_DATE,
            false,
            false,
            &gtk::CellRendererText::new(),
        );

        let log_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .build();
        // Non-overlay: the scrollbar keeps its own space instead of floating
        // over the content (where it is hard to grab and overlaps the
        // paned handle).
        log_scroll.set_overlay_scrolling(false);
        log_scroll.set_child(Some(&ui.log_tree));

        top_box.append(&filter_entry);
        top_box.append(&log_scroll);

        // middle pane: full commit message
        ui.message_view
            .buffer()
            .set_text(&t!("status.loading"));
        let message_scroll = gtk::ScrolledWindow::builder().build();
        message_scroll.add_css_class("pane-separator");
        message_scroll.set_child(Some(&ui.message_view));

        // bottom pane: changed files
        add_text_column(
            &ui.files_tree,
            &t!("file.name"),
            F_NAME,
            true,
            true,
            &gtk::CellRendererText::new(),
        );
        make_expanding(&ui.files_tree, F_NAME, 400);
        for (col, title, align) in [
            (F_ADDED, t!("file.added"), 1.0),
            (F_DELETED, t!("file.deleted"), 1.0),
            (F_SIZE, t!("file.size"), 1.0),
            (F_ACTION, t!("file.action"), 0.0),
        ] {
            add_text_column(
                &ui.files_tree,
                &title,
                col,
                false,
                false,
                &gtk::CellRendererText::new(),
            );
            if let Some(c) = ui.files_tree.columns().get(col as usize) {
                c.set_sizing(gtk::TreeViewColumnSizing::Fixed);
                c.set_fixed_width(90);
                c.set_alignment(align);
            }
        }
        let files_scroll = gtk::ScrolledWindow::builder().build();
        files_scroll.set_child(Some(&ui.files_tree));

        // double-click a file: open its changes at the selected commit in meld
        {
            let files_store = ui.files_store.clone();
            let selected_hash = ui.selected_hash.clone();
            let repo = ui.repo.clone();
            ui.files_tree.connect_row_activated(move |_tree, path, _column| {
                let Some(iter) = files_store.iter(path) else {
                    return;
                };
                let name: String = files_store.get(&iter, F_NAME as i32);
                let Some(hash) = selected_hash.lock().unwrap().clone() else {
                    return;
                };
                // temp files per (process, commit); left in the temp dir for
                // the lifetime of the meld instance
                let dir = std::env::temp_dir()
                    .join(format!("gitlog-{}-{hash}", std::process::id()));
                if std::fs::create_dir_all(&dir).is_err() {
                    return;
                }
                // flat names so subdirectory paths stay valid
                let old_file = dir.join(format!(
                    "{}.old",
                    lookup_old_path(&name).rsplit('/').next().unwrap_or("file")
                ));
                let new_file = dir.join(lookup_path(&name).rsplit('/').next().unwrap_or("file"));
                // added files have no parent version, deleted files none now
                write_blob(&repo, &format!("{hash}^"), lookup_old_path(&name), &old_file);
                write_blob(&repo, &hash, lookup_path(&name), &new_file);
                if let Err(e) = std::process::Command::new("meld")
                    .args([&old_file, &new_file])
                    .spawn()
                {
                    eprintln!("gitlog: cannot open meld: {e}");
                }
            });
        }

        inner.set_start_child(Some(&top_box));
        inner.set_end_child(Some(&message_scroll));
        outer.set_start_child(Some(&inner));
        outer.set_end_child(Some(&files_scroll));
        window.set_child(Some(&outer));
        outer.set_position(490);
        inner.set_position(300);

        // ---- commit list filtering ---------------------------------------------
        {
            // GTK4 (>=4.18) asserts if set_visible_func is called twice, so the
            // function is installed once and reads the query from shared state;
            // each keystroke only updates the query and calls refilter().
            let query: Arc<std::cell::RefCell<String>> =
                Arc::new(std::cell::RefCell::new(String::new()));
            let q = Arc::clone(&query);
            log_filter.set_visible_func(move |model, iter| {
                // The filter may be consulted while a row is being inserted
                // (before its cells are set), so read tolerantly.
                let subject: String = model
                    .get_value(iter, LOG_SUBJECT as i32)
                    .get()
                    .unwrap_or_default();
                let q = q.borrow();
                q.is_empty() || subject.to_lowercase().contains(q.as_str())
            });
            let lf = log_filter.clone();
            filter_entry.connect_search_changed(move |entry| {
                *query.borrow_mut() = entry.text().to_lowercase();
                lf.refilter();
            });
        }

        // ---- commit selection: message + files ----------------------------------
        {
            let ui = Arc::clone(&ui);
            let log_tree = ui.log_tree.clone();
            let log_store = ui.log_store.clone();
            let log_filter = log_filter.clone();
            let selection = log_tree.selection();
            let sel = selection.clone();
            selection.connect_changed(move |_| {
                // paths from the selection refer to the filtered model,
                // convert to the underlying store first
                let (paths, _model) = sel.selected_rows();
                let Some(path) = paths.into_iter().next() else {
                    return;
                };
                let Some(store_path) = log_filter.convert_path_to_child_path(&path) else {
                    return;
                };
                let Some(store_iter) = log_store.iter(&store_path) else {
                    return;
                };
                let idx: i64 = log_store.get(&store_iter, LOG_IDX as i32);
                let Some(hash) = ui.show_commit(idx) else {
                    return;
                };
                let repo = ui.repo.clone();
                let tx = size_tx.clone();
                std::thread::spawn(move || {
                    let map = gl::file_sizes(&repo, &hash);
                    let _ = tx.send(Msg::Sizes { hash, map });
                });
            });
        }

        // ---- start background load ---------------------------------------------
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let cancel = Arc::clone(&cancel);
            window.connect_destroy(move |_| {
                cancel.store(true, Ordering::Relaxed);
            });
        }
        {
            let commit_rx = Arc::new(Mutex::new(commit_rx));
            let size_rx = Arc::new(Mutex::new(size_rx));
            let ui = Arc::clone(&ui);
            let win = window.clone();
            let repo_name = repo_name.clone();
            let branch = branch.clone();
            let _id = glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
                let rx = commit_rx.lock().unwrap();
                loop {
                    match rx.try_recv() {
                        Ok(Msg::Commit(commit)) => {
                            let first = ui.commits.lock().unwrap().is_empty();
                            ui.insert_commit(commit);
                            if first {
                                let path = gtk::TreePath::from_indices(&[0]);
                                gtk::prelude::TreeViewExt::set_cursor(&ui.log_tree, &path, None, false);
                            }
                        }
                        Ok(_) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            // background load finished
                            let n = ui.commits.lock().unwrap().len();
                            win.set_title(Some(&t!(
                                "title.done",
                                repo = &repo_name,
                                branch = &branch,
                                count = n
                            )));
                            break;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    }
                }
                let rx = size_rx.lock().unwrap();
                loop {
                    match rx.try_recv() {
                        Ok(Msg::Sizes { hash, map }) => {
                            ui.update_sizes(hash, map);
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
                glib::ControlFlow::Continue
            });
        }
        {
            let repo = repo.clone();
            std::thread::spawn(move || {
                gl::load_commits(
                    &repo,
                    &cancel,
                    move |commit| commit_tx.send(Msg::Commit(commit)).is_ok(),
                );
            });
        }

        window.show();
    });

    // run with only the program name: the folder argument must not be
    // interpreted by GApplication as a file to open
    let prog = env::args().next().unwrap_or_default();
    ExitCode::from(app.run_with_args(&[prog]).get())
}

#[cfg(test)]
mod i18n_resolution_tests {
    // Verify every supported locale actually resolves its strings, using the
    // explicit `locale=` argument so the global locale is never touched and
    // tests cannot race each other.

    #[test]
    fn col_date_resolves_in_every_locale() {
        assert_eq!(rust_i18n::t!("col.date", locale = "en"), "Date");
        assert_eq!(rust_i18n::t!("col.date", locale = "de"), "Datum");
        assert_eq!(rust_i18n::t!("col.date", locale = "fr"), "Date");
        assert_eq!(rust_i18n::t!("col.date", locale = "es"), "Fecha");
        assert_eq!(rust_i18n::t!("col.date", locale = "it"), "Data");
        assert_eq!(rust_i18n::t!("col.date", locale = "nl"), "Datum");
        assert_eq!(rust_i18n::t!("col.date", locale = "pt"), "Data");
    }

    #[test]
    fn title_done_resolves_and_interpolates_in_every_locale() {
        let repo = "acme";
        let branch = "main";
        let n: usize = 3;
        assert_eq!(rust_i18n::t!("title.done", locale = "en", repo = &repo, branch = &branch, count = n), "gitlog - acme - main - 3 commits");
        assert_eq!(rust_i18n::t!("title.done", locale = "de", repo = &repo, branch = &branch, count = n), "gitlog - acme - main - 3 Commits");
        assert_eq!(rust_i18n::t!("title.done", locale = "it", repo = &repo, branch = &branch, count = n), "gitlog - acme - main - 3 commit");
        assert_eq!(rust_i18n::t!("title.done", locale = "nl", repo = &repo, branch = &branch, count = n), "gitlog - acme - main - 3 commits");
    }
}


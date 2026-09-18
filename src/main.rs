use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

extern crate gtk4 as gtk;

use gtk::glib;
use gtk::prelude::*;

mod gitlog;

const LOG_HASH: u32 = 0;
const LOG_SUBJECT: u32 = 1;
const LOG_COMMITTER: u32 = 2;
const LOG_AUTHOR: u32 = 3;
const LOG_DATE: u32 = 4;
const LOG_IDX: u32 = 5; // index into the commit cache (hidden)

const F_NAME: u32 = 0;
const F_ADDED: u32 = 1;
const F_DELETED: u32 = 2;
const F_SIZE: u32 = 3;
const F_ACTION: u32 = 4;

type SizeCache = HashMap<String, HashMap<String, u64>>;

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

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

fn numstat(n: Option<i64>) -> String {
    match n {
        Some(0) => "0".into(),
        Some(n) => n.to_string(),
        None => "•".into(), // binary file
    }
}

fn add_text_column(tree: &gtk::TreeView, title: &str, col: u32, expand: bool) {
    let column = gtk::TreeViewColumn::builder().title(title).build();
    let cell = gtk::CellRendererText::new();
    column.pack_start(&cell, !expand);
    column.add_attribute(&cell, "text", col as i32);
    column.set_expand(expand);
    tree.append_column(&column);
}

/// Messages from background threads, drained on the main loop.
enum Msg {
    Commit(gitlog::Commit),
    Sizes { hash: String, map: HashMap<String, u64> },
}

struct Ui {
    commits: Arc<Mutex<Vec<gitlog::Commit>>>,
    sizes: Arc<Mutex<SizeCache>>,
    selected_hash: Arc<Mutex<Option<String>>>,
    log_store: gtk::ListStore,
    log_tree: gtk::TreeView,
    files_store: gtk::ListStore,
    files_tree: gtk::TreeView,
    message_view: gtk::TextView,
    branch_label: gtk::Label,
    repo: PathBuf,
}

impl Ui {
    fn insert_commit(&self, commit: gitlog::Commit) {
        let idx = self.commits.lock().unwrap().len() as i64;
        let iter = self.log_store.append();
        self.log_store.set(
            &iter,
            &[
                (LOG_HASH, &commit.hash),
                (LOG_SUBJECT, &commit.subject),
                (LOG_COMMITTER, &commit.committer),
                (LOG_AUTHOR, &commit.author),
                (LOG_DATE, &commit.date),
                (LOG_IDX, &idx),
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
        self.message_view.buffer().set_text(&commit.message);

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
                    (F_ACTION, &f.action.label()),
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
    if !gitlog::is_repo(&repo) {
        eprintln!("gitlog: {folder} is not a git repository");
        return ExitCode::FAILURE;
    }

    let app = gtk::Application::builder()
        .application_id("dev.example.gitlog")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |app| {
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
                glib::Type::STRING,
                glib::Type::I64,
            ]),
            log_tree: gtk::TreeView::builder().build(),
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
            branch_label: gtk::Label::new(None),
            repo: repo.clone(),
        });
        // attach models now that the stores exist
        let log_filter = gtk::TreeModelFilter::new(&ui.log_store, None);
        ui.log_tree.set_model(Some(&log_filter));
        ui.files_tree.set_model(Some(&ui.files_store));

        // ---- window & layout ---------------------------------------------------
        let title = format!(
            "gitlog — {}",
            repo.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        let window = gtk::Window::builder()
            .title(&title)
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
        let branch = gitlog::head_info(&repo);
        ui.branch_label
            .set_markup(&format!("<b>Branch:</b> {}  <span size=\"small\">loading…</span>", escape(&branch)));
        let filter_entry = gtk::SearchEntry::builder().build();
        filter_entry.set_placeholder_text(Some("Filter by commit message…"));

        add_text_column(&ui.log_tree, "Hash", LOG_HASH, false);
        if let Some(hash_col) = ui.log_tree.columns().get(0) {
            hash_col.set_sizing(gtk::TreeViewColumnSizing::Fixed);
            hash_col.set_fixed_width(90);
        }
        add_text_column(&ui.log_tree, "Message", LOG_SUBJECT, true);
        add_text_column(&ui.log_tree, "Committer", LOG_COMMITTER, false);
        add_text_column(&ui.log_tree, "Author", LOG_AUTHOR, false);
        add_text_column(&ui.log_tree, "Date", LOG_DATE, false);

        let log_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .build();
        log_scroll.set_child(Some(&ui.log_tree));

        top_box.append(&ui.branch_label);
        top_box.append(&filter_entry);
        top_box.append(&log_scroll);

        // middle pane: full commit message
        ui.message_view
            .buffer()
            .set_text("Loading commit history…");
        let message_scroll = gtk::ScrolledWindow::builder().build();
        message_scroll.set_child(Some(&ui.message_view));

        // bottom pane: changed files
        add_text_column(&ui.files_tree, "File", F_NAME, true);
        for (col, title, align) in [
            (F_ADDED, "Added", 1.0),
            (F_DELETED, "Deleted", 1.0),
            (F_SIZE, "Size", 1.0),
            (F_ACTION, "Action", 0.0),
        ] {
            add_text_column(&ui.files_tree, title, col, false);
            if let Some(c) = ui.files_tree.columns().get(col as usize) {
                c.set_sizing(gtk::TreeViewColumnSizing::Fixed);
                c.set_fixed_width(90);
                c.set_alignment(align);
            }
        }
        let files_scroll = gtk::ScrolledWindow::builder().build();
        files_scroll.set_child(Some(&ui.files_tree));

        inner.set_start_child(Some(&top_box));
        inner.set_end_child(Some(&message_scroll));
        outer.set_start_child(Some(&inner));
        outer.set_end_child(Some(&files_scroll));
        window.set_child(Some(&outer));
        outer.set_position(490);
        inner.set_position(300);

        // ---- commit list filtering ---------------------------------------------
        {
            let log_filter = log_filter.clone();
            filter_entry.connect_search_changed(move |entry| {
                let q = entry.text().to_lowercase();
                log_filter.set_visible_func(move |model, iter| {
                    let subject: String = model.get(iter, LOG_SUBJECT as i32);
                    subject.to_lowercase().contains(&q)
                });
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
                let Some(store_path) = log_filter.convert_child_path_to_path(&path) else {
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
                    let map = gitlog::file_sizes(&repo, &hash);
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
                            ui.branch_label.set_markup(&format!(
                                "<b>Branch:</b> {}  <span size=\"small\">{} commits</span>",
                                escape(&branch),
                                n
                            ));
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
                gitlog::load_commits(
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

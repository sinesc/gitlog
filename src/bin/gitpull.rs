//! gitpull: a pull window for a git repository (see dev/gitpull-plan.md).
//!
//! A small form like gitpush: a remote dropdown, a remote-branch entry,
//! three action radios (fetch only / fetch and merge / fetch and rebase)
//! and option checkboxes (squash, no fast-forward, fast-forward only,
//! tags, prune) whose sensitivity follows the selected action. Pulls run
//! in a background thread; when git stops with conflicts a second window
//! lists the conflicted files, opens a 3-way merge in meld (base :1 /
//! real work-tree file / theirs :3) and drives "Commit merge" or the
//! multi-step "Continue rebase" loop (abort is always available).

use std::env;
use std::path::{Path, PathBuf};
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

// conflict list columns
const C_RESOLVED: u32 = 0;
const C_NAME: u32 = 1;
const C_STATUS: u32 = 2;

/// The selected action type.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Fetch,
    Merge,
    Rebase,
}

/// One labelled row of the form (label + widget, as in gitpush).
fn add_row<W: IsA<gtk::Widget>>(form: &gtk::Box, label: &str, widget: &W) {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .margin_start(8)
        .margin_bottom(4)
        .build();
    let label = gtk::Label::builder().label(label).build();
    row.append(&label);
    row.append(widget);
    form.append(&row);
}

/// Error/info dialog with a **selectable** text view (same as gitpush's,
/// copied because binaries cannot import from each other).
fn error_dialog(parent: &gtk::Window, text: &str) {
    let dialog = gtk::Dialog::builder()
        .transient_for(parent)
        .modal(true)
        .default_width(640)
        .default_height(320)
        .build();
    let view = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(true)
        .wrap_mode(gtk::WrapMode::Char)
        .left_margin(8)
        .right_margin(8)
        .top_margin(8)
        .build();
    view.buffer().set_text(&text);
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .build();
    scroll.set_child(Some(&view));
    dialog.content_area().append(&scroll);
    dialog
        .add_button(&t!("error.close"), gtk::ResponseType::Close)
        .grab_focus();
    dialog.connect_response(|dialog, _response| dialog.close());
    dialog.show();
}

/// Localized label for a porcelain unmerged status code.
fn status_label(xy: &str) -> String {
    match xy {
        "UU" => t!("conflicts.status.both_mod").into_owned(),
        "AA" => t!("conflicts.status.both_add").into_owned(),
        "DD" => t!("conflicts.status.both_del").into_owned(),
        "AU" => t!("conflicts.status.added_us").into_owned(),
        // AT/TA are shown as "added by them" by git status as well
        "UA" | "AT" | "TA" => t!("conflicts.status.added_them").into_owned(),
        "DU" => t!("conflicts.status.deleted_us").into_owned(),
        _ => t!("conflicts.status.deleted_them").into_owned(),
    }
}

/// Title of the conflicts window; the rebase title carries the step
/// counter ("step 2 of 5").
fn conflicts_title(repo: &Path, is_rebase: bool) -> String {
    let count = gl::unmerged_files(repo).len();
    if is_rebase {
        match gl::rebase_progress(repo) {
            Some((step, total, _)) => {
                t!("conflicts.title.rebase", step = step, total = total).into_owned()
            }
            None => t!("conflicts.title.merge", count = count).into_owned(),
        }
    } else {
        t!("conflicts.title.merge", count = count).into_owned()
    }
}

/// Re-read the unmerged files into the conflict list and update the
/// window title (the rebase step counter changes after every continue).
fn refresh_conflicts(store: &gtk::ListStore, dialog: &gtk::Window, repo: &Path, is_rebase: bool) {
    store.clear();
    for (path, xy) in gl::unmerged_files(repo) {
        let status = status_label(&xy);
        let iter = store.append();
        store.set(
            &iter,
            &[
                (C_RESOLVED, &false),
                (C_NAME, &path),
                (C_STATUS, &status),
            ],
        );
    }
    dialog.set_title(Some(&conflicts_title(repo, is_rebase)));
}

/// Open one conflicted file as a 3-way merge in meld: base (`:1`) and
/// theirs (`:3`) are temp copies, the local pane is the **real**
/// working-tree file (which already carries the conflict markers), so
/// saving the resolved content in meld writes it back into the
/// repository - same "only the other side is a temp copy" rule as
/// gitcommit. Missing stages (add/add has no base, a delete lacks one
/// side) become empty files so meld always gets three panes.
fn open_conflict_diff(parent: &gtk::Window, repo: &Path, path: &str) {
    if gl::is_spreadsheet(path) {
        // Calc's Compare Sheets is a 2-way diff, not a 3-way merge
        error_dialog(parent, &t!("error.conflict_spreadsheet").into_owned());
        return;
    }
    let (base, _ours, theirs) = gl::conflict_blobs(repo, path);
    let dir = std::env::temp_dir().join(format!("gitpull-{}", std::process::id()));
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    // flatten the full path so same base names cannot collide; keep the
    // extension so meld picks the right highlighting
    let tag: String = path
        .chars()
        .map(|c| if c == '/' { '-' } else { c })
        .collect();
    let side = |name: &str| match Path::new(path).extension().and_then(|e| e.to_str()) {
        Some(ext) => dir.join(format!("{tag}.{name}.{ext}")),
        None => dir.join(format!("{tag}.{name}")),
    };
    let base_file = side("base");
    let theirs_file = side("theirs");
    if std::fs::write(&base_file, base.unwrap_or_default()).is_err()
        || std::fs::write(&theirs_file, theirs.unwrap_or_default()).is_err()
    {
        return;
    }
    let work_file = repo.join(path);
    if let Err(e) = std::process::Command::new("meld")
        .args([&base_file, &work_file, &theirs_file])
        .spawn()
    {
        eprintln!("gitpull: cannot open meld: {e}");
    }
}

/// (path, resolved checkbox) of every conflict-list row.
fn read_rows(store: &gtk::ListStore) -> Vec<(String, bool)> {
    let mut rows = Vec::new();
    let mut iter = store.iter_first();
    while let Some(mut i) = iter {
        let path: String = store.get(&i, C_NAME as i32);
        let resolved: bool = store.get(&i, C_RESOLVED as i32);
        rows.push((path, resolved));
        iter = store.iter_next(&mut i).then_some(i);
    }
    rows
}

/// The conflicts window shared by merge and rebase conflicts: the
/// unmerged files with a "Resolved" checkbox, double-click opens the
/// 3-way meld, and Abort plus "Commit merge"/"Continue rebase" at the
/// bottom. For rebase the window drives the whole multi-step loop:
/// after each continue the list and the step counter are refreshed
/// until the rebase finishes (or is aborted).
fn show_conflicts(win: &gtk::Window, repo: &Path) {
    let is_rebase = gl::rebase_progress(repo).is_some();
    let dialog = gtk::Window::builder()
        .transient_for(win)
        .modal(true)
        .default_width(720)
        .default_height(420)
        .title(&conflicts_title(repo, is_rebase))
        .build();

    let info_text = if is_rebase {
        t!("pull.rebase_in_progress").into_owned()
    } else {
        t!("pull.merge_in_progress").into_owned()
    };
    let info = gtk::Label::builder()
        .label(&info_text)
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .margin_start(8)
        .margin_top(8)
        .margin_end(8)
        .build();

    let store = gtk::ListStore::new(&[glib::Type::BOOL, glib::Type::STRING, glib::Type::STRING]);
    let tree = gtk::TreeView::builder().model(&store).build();

    let toggle = gtk::CellRendererToggle::new();
    let resolved_col = gtk::TreeViewColumn::builder()
        .title(t!("conflicts.resolved").as_ref())
        .build();
    resolved_col.pack_start(&toggle, false);
    resolved_col.add_attribute(&toggle, "active", C_RESOLVED as i32);
    resolved_col.set_sizing(gtk::TreeViewColumnSizing::Fixed);
    resolved_col.set_fixed_width(90);
    tree.append_column(&resolved_col);
    {
        let store = store.clone();
        toggle.connect_toggled(move |_renderer, path| {
            if let Some(iter) = store.iter(&path) {
                let value: bool = store.get(&iter, C_RESOLVED as i32);
                store.set(&iter, &[(C_RESOLVED, &!value)]);
            }
        });
    }

    let name_cell = gtk::CellRendererText::new();
    name_cell.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let name_col = gtk::TreeViewColumn::builder()
        .title(t!("conflicts.file").as_ref())
        .build();
    name_col.pack_start(&name_cell, true);
    name_col.add_attribute(&name_cell, "text", C_NAME as i32);
    name_col.set_expand(true);
    name_col.set_sizing(gtk::TreeViewColumnSizing::Fixed);
    name_col.set_fixed_width(360);
    tree.append_column(&name_col);

    let status_cell = gtk::CellRendererText::new();
    let status_col = gtk::TreeViewColumn::builder()
        .title(t!("col.status").as_ref())
        .build();
    status_col.pack_start(&status_cell, false);
    status_col.add_attribute(&status_cell, "text", C_STATUS as i32);
    status_col.set_sizing(gtk::TreeViewColumnSizing::Fixed);
    status_col.set_fixed_width(160);
    tree.append_column(&status_col);

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .build();
    scroll.set_overlay_scrolling(false);
    scroll.set_child(Some(&tree));

    // ---- double-click: 3-way merge in meld ---------------------------------
    {
        let store = store.clone();
        let repo = repo.to_path_buf();
        let dlg = dialog.clone();
        tree.connect_row_activated(move |_tree, path, _column| {
            let Some(iter) = store.iter(path) else {
                return;
            };
            let path: String = store.get(&iter, C_NAME as i32);
            open_conflict_diff(&dlg, &repo, &path);
        });
    }

    // ---- buttons ------------------------------------------------------------
    let abort_button = gtk::Button::builder().label(t!("conflicts.abort").as_ref()).build();
    let action_label = if is_rebase {
        t!("conflicts.continue_rebase").into_owned()
    } else {
        t!("conflicts.commit_merge").into_owned()
    };
    let action_button = gtk::Button::builder()
        .label(&action_label)
        .build();
    action_button.add_css_class("suggested-action");
    let button_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .build();
    button_box.add_css_class("pull-buttons");
    button_box.set_halign(gtk::Align::End);
    button_box.append(&abort_button);
    button_box.append(&action_button);

    let outer = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    outer.append(&info);
    outer.append(&scroll);
    outer.append(&button_box);
    dialog.set_child(Some(&outer));
    // fill the list once at startup (later continues refresh it)
    refresh_conflicts(&store, &dialog, repo, is_rebase);

    // ---- abort ---------------------------------------------------------------
    {
        let repo = repo.to_path_buf();
        let dlg = dialog.clone();
        abort_button.connect_clicked(move |_| {
            let args: Vec<&str> = if is_rebase {
                vec!["rebase", "--abort"]
            } else {
                vec!["merge", "--abort"]
            };
            if let Err(e) = gl::run_git(&repo, &args) {
                error_dialog(&dlg, &e);
            }
            dlg.close();
        });
    }

    // ---- commit merge / continue rebase --------------------------------------
    {
        let store = store.clone();
        let repo = repo.to_path_buf();
        let dlg = dialog.clone();
        let win = win.clone();
        action_button.connect_clicked(move |_| {
            let rows = read_rows(&store);
            // "resolved" is applied on the button: every row must be ticked
            if rows.iter().any(|(_, resolved)| !resolved) {
                error_dialog(&dlg, &t!("error.conflicts_remain").into_owned());
                return;
            }
            for (path, _) in &rows {
                if let Err(e) = gl::stage_path(&repo, path) {
                    error_dialog(&dlg, &e);
                    return;
                }
            }
            if is_rebase {
                match gl::rebase_continue(&repo) {
                    Ok(()) => {
                        if gl::rebase_progress(&repo).is_none() {
                            // the rebase finished
                            dlg.close();
                            win.close();
                        } else {
                            // next step (conflict or stopped for edit)
                            refresh_conflicts(&store, &dlg, &repo, true);
                        }
                    }
                    Err(e) => {
                        if gl::rebase_progress(&repo).is_some() {
                            // new conflicts in this step: re-fill the list
                            refresh_conflicts(&store, &dlg, &repo, true);
                        } else {
                            // the rebase state vanished (git refused to
                            // continue); report and close
                            error_dialog(&dlg, &t!("error.pull_failed", error = &e).into_owned());
                            dlg.close();
                            win.close();
                        }
                    }
                }
            } else {
                match gl::commit_merge(&repo) {
                    Ok(()) => {
                        if !gl::merge_in_progress(&repo) && gl::unmerged_files(&repo).is_empty() {
                            dlg.close();
                            win.close();
                        } else {
                            refresh_conflicts(&store, &dlg, &repo, false);
                        }
                    }
                    Err(e) => {
                        refresh_conflicts(&store, &dlg, &repo, false);
                        error_dialog(&dlg, &t!("error.commit_failed", error = &e).into_owned());
                    }
                }
            }
        });
    }

    dialog.show();
}

/// Disable the options that do not apply to the selected mode and uncheck
/// them (so hidden state cannot leak into the command); the action-button
/// label follows the mode ("Pull"/"Fetch"). Only merge uses squash,
/// --no-ff and --ff-only; tags and prune are fetch options and stay
/// enabled in every mode.
fn apply_mode(
    mode: Mode,
    squash: &gtk::CheckButton,
    no_ff: &gtk::CheckButton,
    ff_only: &gtk::CheckButton,
    button: &gtk::Button,
) {
    let merge_mode = mode == Mode::Merge;
    for check in [squash, no_ff, ff_only] {
        check.set_sensitive(merge_mode);
        if !merge_mode {
            check.set_active(false);
        }
    }
    let label = if mode == Mode::Fetch {
        t!("pull.fetch").into_owned()
    } else {
        t!("pull.pull").into_owned()
    };
    button.set_label(&label);
}

fn main() -> ExitCode {
    // no argument: pull in the current folder (as in gitlog/gitcommit)
    let folder = env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let repo = match PathBuf::from(&folder).canonicalize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("gitpull: cannot open {folder}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if !gl::is_repo(&repo) {
        eprintln!("gitpull: {folder} is not a git repository");
        return ExitCode::FAILURE;
    }

    // UI language: detected once from the system locale (as in gitlog)
    let locale = i18n::detect_locale();
    rust_i18n::set_locale(locale);

    let app = gtk::Application::builder()
        .application_id("dev.example.gitpull")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |app| {
        // Thicker scrollbars (same as gitlog/gitpush); button margins via
        // CSS (GtkBox lost the `margin` property in GTK 4.18). The class
        // is also used by the conflicts window's buttons.
        if let Some(display) = gtk::gdk::Display::default() {
            let provider = gtk::CssProvider::new();
            provider.load_from_data(
                "scrollbar.horizontal slider { min-height: 12px; margin: 2px; }
                 scrollbar.vertical slider { min-width: 12px; margin: 2px; }
                 .pull-buttons { margin: 6px; }",
            );
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let remotes = gl::remotes(&repo);
        let upstream = gl::upstream_of(&repo);
        let title_branch = gl::head_info(&repo);
        // no default height: the window fits tightly around the content
        let window = gtk::Window::builder()
            .title(t!("pull.title", branch = &title_branch).as_ref())
            .default_width(720)
            .build();
        window.set_application(Some(app));

        // ---- form: four grouped sections -------------------------------------
        // group 1: remote (no "All remotes" entry - pulling from every
        // remote at once is not meaningful)
        let remote_combo = gtk::ComboBoxText::new();
        for (i, r) in remotes.iter().enumerate() {
            remote_combo.append_text(r);
            if upstream.as_ref().map(|(u, _)| u == r).unwrap_or(false) {
                remote_combo.set_active(Some(i as u32));
            }
        }
        if remote_combo.active().is_none() && !remotes.is_empty() {
            remote_combo.set_active(Some(0));
        }
        let remote_frame = gtk::Frame::builder()
            .label(t!("pull.group.remote").as_ref())
            .margin_start(8)
            .margin_top(8)
            .margin_end(8)
            .margin_bottom(8)
            .build();
        let combo_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_start(8)
            .margin_end(8)
            .margin_bottom(8)
            .build();
        combo_box.append(&remote_combo);
        remote_frame.set_child(Some(&combo_box));

        // group 2: remote branch (empty = git's default refspec / all)
        let branch_entry = gtk::Entry::builder().hexpand(true).build();
        if let Some((_, branch)) = &upstream {
            branch_entry.set_text(branch);
        }
        let branch_hint = t!("pull.remote_branch_hint");
        branch_entry.set_placeholder_text(Some(branch_hint.as_ref()));
        let branch_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_start(8)
            .margin_end(8)
            .margin_bottom(8)
            .build();
        add_row(&branch_box, &t!("pull.remote_branch"), &branch_entry);
        let branch_frame = gtk::Frame::builder()
            .label(t!("pull.group.branch").as_ref())
            .margin_start(8)
            .margin_top(8)
            .margin_end(8)
            .margin_bottom(8)
            .build();
        branch_frame.set_child(Some(&branch_box));

        // group 3: action radios (merge is the default)
        let fetch_radio = gtk::CheckButton::builder()
            .label(t!("pull.action.fetch").as_ref())
            .build();
        let merge_radio = gtk::CheckButton::builder()
            .label(t!("pull.action.merge").as_ref())
            .group(&fetch_radio)
            .build();
        let rebase_radio = gtk::CheckButton::builder()
            .label(t!("pull.action.rebase").as_ref())
            .group(&fetch_radio)
            .build();
        merge_radio.set_active(true);
        let action_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .margin_start(8)
            .margin_end(8)
            .margin_bottom(8)
            .build();
        action_box.append(&fetch_radio);
        action_box.append(&merge_radio);
        action_box.append(&rebase_radio);
        let action_frame = gtk::Frame::builder()
            .label(t!("pull.group.action").as_ref())
            .margin_start(8)
            .margin_top(8)
            .margin_end(8)
            .margin_bottom(8)
            .build();
        action_frame.set_child(Some(&action_box));

        // group 4: pull options
        let squash_check = gtk::CheckButton::builder()
            .label(t!("pull.squash").as_ref())
            .margin_start(8)
            .margin_bottom(4)
            .build();
        let no_ff_check = gtk::CheckButton::builder()
            .label(t!("pull.no_ff").as_ref())
            .margin_start(8)
            .margin_bottom(4)
            .build();
        let ff_only_check = gtk::CheckButton::builder()
            .label(t!("pull.ff_only").as_ref())
            .margin_start(8)
            .margin_bottom(4)
            .build();
        let tags_check = gtk::CheckButton::builder()
            .label(t!("pull.tags").as_ref())
            .margin_start(8)
            .margin_bottom(4)
            .build();
        let prune_check = gtk::CheckButton::builder()
            .label(t!("pull.prune").as_ref())
            .margin_start(8)
            .margin_bottom(4)
            .build();
        let merge_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_start(8)
            .margin_bottom(4)
            .build();
        merge_row.append(&no_ff_check);
        merge_row.append(&ff_only_check);
        let fetch_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_start(8)
            .margin_bottom(4)
            .build();
        fetch_row.append(&tags_check);
        fetch_row.append(&prune_check);
        let options_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_start(8)
            .margin_end(8)
            .margin_bottom(8)
            .build();
        options_box.append(&squash_check);
        options_box.append(&merge_row);
        options_box.append(&fetch_row);
        let options_frame = gtk::Frame::builder()
            .label(t!("pull.group.options").as_ref())
            .margin_start(8)
            .margin_top(8)
            .margin_end(8)
            .margin_bottom(8)
            .build();
        options_frame.set_child(Some(&options_box));

        // vexpand so the buttons stay at the bottom of the window
        let form = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .build();
        form.append(&remote_frame);
        form.append(&branch_frame);
        form.append(&action_frame);
        form.append(&options_frame);

        // ---- buttons -----------------------------------------------------------
        let cancel_button = gtk::Button::builder()
            .label(t!("pull.cancel").as_ref())
            .build();
        let pull_button = gtk::Button::builder().label(t!("pull.pull").as_ref()).build();
        pull_button.add_css_class("suggested-action");
        let button_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        button_box.add_css_class("pull-buttons");
        button_box.set_halign(gtk::Align::End);
        button_box.append(&cancel_button);
        button_box.append(&pull_button);

        let outer = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        outer.append(&form);
        outer.append(&button_box);
        window.set_child(Some(&outer));

        // ---- radio behaviour: option sensitivity + button label ---------------
        {
            let fetch = fetch_radio.clone();
            let merge = merge_radio.clone();
            let squash = squash_check.clone();
            let no_ff = no_ff_check.clone();
            let ff_only = ff_only_check.clone();
            let button = pull_button.clone();
            for radio in [&fetch_radio, &merge_radio, &rebase_radio] {
                let (fetch, merge) = (fetch.clone(), merge.clone());
                let (squash, no_ff, ff_only, button) =
                    (squash.clone(), no_ff.clone(), ff_only.clone(), button.clone());
                radio.connect_toggled(move |chk| {
                    // the group fires toggled on the de-activated button
                    // too; only the newly active one changes anything
                    if !chk.is_active() {
                        return;
                    }
                    let mode = if fetch.is_active() {
                        Mode::Fetch
                    } else if merge.is_active() {
                        Mode::Merge
                    } else {
                        Mode::Rebase
                    };
                    apply_mode(mode, &squash, &no_ff, &ff_only, &button);
                });
            }
        }

        // --no-ff and --ff-only are alternatives: checking one unchecks
        // the other (the force/lease pattern from gitpush)
        {
            let ff_only = ff_only_check.clone();
            no_ff_check.connect_toggled(move |check| {
                if check.is_active() {
                    ff_only.set_active(false);
                }
            });
            let no_ff = no_ff_check.clone();
            ff_only_check.connect_toggled(move |check| {
                if check.is_active() {
                    no_ff.set_active(false);
                }
            });
        }

        // ---- pull ---------------------------------------------------------------
        // the pull runs in a background thread (network can take a long
        // time); its result is sent back through a channel polled from a
        // timeout on the main loop (gitpush pattern)
        let busy = Arc::new(AtomicBool::new(false));
        {
            let remotes = remotes.clone();
            let branch_e = branch_entry.clone();
            let combo = remote_combo.clone();
            let squash_c = squash_check.clone();
            let no_ff_c = no_ff_check.clone();
            let ff_only_c = ff_only_check.clone();
            let tags_c = tags_check.clone();
            let prune_c = prune_check.clone();
            let fetch_r = fetch_radio.clone();
            let rebase_r = rebase_radio.clone();
            let win = window.clone();
            let repo = repo.clone();
            let busy = busy.clone();
            let button = pull_button.clone();
            pull_button.connect_clicked(move |_| {
                if busy.load(Ordering::Relaxed) {
                    return;
                }
                if remotes.is_empty() {
                    error_dialog(&win, &t!("error.no_remotes").into_owned());
                    return;
                }
                // a stopped merge/rebase owns the repository state: open
                // the conflicts window instead of running git (git itself
                // refuses to pull into it)
                if gl::merge_in_progress(&repo) || gl::rebase_progress(&repo).is_some() {
                    show_conflicts(&win, &repo);
                    return;
                }
                let remote = combo
                    .active()
                    .and_then(|i| remotes.get(i as usize))
                    .cloned()
                    .unwrap_or_default();
                if remote.is_empty() {
                    error_dialog(&win, &t!("error.no_remotes").into_owned());
                    return;
                }
                let mode = if fetch_r.is_active() {
                    Mode::Fetch
                } else if rebase_r.is_active() {
                    Mode::Rebase
                } else {
                    Mode::Merge
                };
                let squash = squash_c.is_active();
                let args = {
                    let no_ff = no_ff_c.is_active();
                    let ff_only = ff_only_c.is_active();
                    let tags = tags_c.is_active();
                    let prune = prune_c.is_active();
                    let branch = branch_e.text().trim().to_string();
                    let mut args: Vec<String> = Vec::new();
                    match mode {
                        Mode::Fetch => args.push("fetch".into()),
                        Mode::Merge => args.push("pull".into()),
                        Mode::Rebase => {
                            args.push("pull".into());
                            args.push("--rebase".into());
                        }
                    }
                    if mode == Mode::Merge {
                        // explicit merge strategy: git refuses divergent
                        // pulls without one when pull.rebase is unset
                        args.push("--no-rebase".into());
                        if squash {
                            args.push("--squash".into());
                        } else {
                            // --no-ff/default merges create a commit: take
                            // git's prepared message, never spawn an editor
                            // in the background thread
                            args.push("--no-edit".into());
                        }
                        if no_ff {
                            args.push("--no-ff".into());
                        }
                        if ff_only {
                            args.push("--ff-only".into());
                        }
                    }
                    if prune {
                        args.push("--prune".into());
                    }
                    if tags {
                        args.push("--tags".into());
                    }
                    args.push(remote);
                    if !branch.is_empty() {
                        // an empty entry means: git's default refspec
                        // (current branch's upstream) / a full fetch
                        args.push(branch);
                    }
                    args
                };

                // squash leaves the combined changes staged with no commit:
                // open gitcommit on success so the user commits them there
                let spawn_commit = mode == Mode::Merge && squash;
                let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
                let rx = Arc::new(Mutex::new(rx));
                busy.store(true, Ordering::Relaxed);
                button.set_sensitive(false);
                let win2 = win.clone();
                let busy2 = busy.clone();
                let button2 = button.clone();
                let repo2 = repo.clone();
                glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
                    match rx.lock().unwrap().try_recv() {
                        Ok(Ok(())) => {
                            busy2.store(false, Ordering::Relaxed);
                            if spawn_commit {
                                if let Err(e) =
                                    std::process::Command::new("gitcommit").arg(&repo2).spawn()
                                {
                                    eprintln!("gitpull: cannot open gitcommit: {e}");
                                }
                            }
                            win2.close();
                        }
                        Ok(Err(e)) => {
                            busy2.store(false, Ordering::Relaxed);
                            button2.set_sensitive(true);
                            // git stopped on conflicts? Open the conflicts
                            // window; everything else is a plain error
                            if gl::rebase_progress(&repo2).is_some()
                                || gl::merge_in_progress(&repo2)
                            {
                                show_conflicts(&win2, &repo2);
                            } else {
                                error_dialog(
                                    &win2,
                                    &t!("error.pull_failed", error = &e).into_owned(),
                                );
                            }
                        }
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            // the sender went away after delivering nothing
                            busy2.store(false, Ordering::Relaxed);
                            button2.set_sensitive(true);
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    }
                    glib::ControlFlow::Continue
                });
                let repo = repo.clone();
                std::thread::spawn(move || {
                    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                    // GIT_EDITOR=true: git must never open an interactive
                    // editor from this thread (no terminal attached)
                    let _ = tx.send(gl::run_git_env(
                        &repo,
                        &refs,
                        &[("GIT_EDITOR", "true")],
                    ));
                });
            });
        }

        // ---- cancel / close -------------------------------------------------------
        // while a pull is in flight, closing the window would leave the git
        // process running after the app quits (std waits for non-daemon
        // threads at exit), so terminate the process instead
        {
            let win = window.clone();
            let busy = busy.clone();
            cancel_button.connect_clicked(move |_| {
                if busy.load(Ordering::Relaxed) {
                    std::process::exit(0);
                }
                win.close();
            });
        }
        {
            let busy = busy.clone();
            window.connect_destroy(move |_| {
                if busy.load(Ordering::Relaxed) {
                    std::process::exit(0);
                }
            });
        }

        window.show();

        // A merge/rebase left over from an earlier pull (also one started
        // outside gitpull) opens the conflicts window right away, so the
        // pull window doubles as the resume UI.
        if gl::merge_in_progress(&repo) || gl::rebase_progress(&repo).is_some() {
            show_conflicts(&window, &repo);
        }
    });

    // run with only the program name: the folder argument must not be
    // interpreted by GApplication as a file to open (same as gitlog)
    let prog = env::args().next().unwrap_or_default();
    ExitCode::from(app.run_with_args(&[prog]).get())
}

//! gitpush: a push window for a git repository (see dev/push-concept.md).
//!
//! A small form: a remote dropdown (first item "All remotes", then the
//! configured remotes), local/remote branch name entries, checkboxes (push
//! all branches, force, force with lease, include tags, set upstream) and
//! Cancel/Push buttons at the bottom. Pushes run in a background thread
//! (network transfers can take a long time); on error a dialog with a
//! selectable text view is shown.

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

/// One labelled row of the form (label + widget, e.g. entry or combo).
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

/// Error dialog with a **selectable** text view (unlike the gitcommit
/// MessageDialog): the user can select and copy the message, e.g. to search
/// for it.
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

fn main() -> ExitCode {
    // no argument: push in the current folder (as in gitlog/gitcommit)
    let folder = env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let repo = match PathBuf::from(&folder).canonicalize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("gitpush: cannot open {folder}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if !gl::is_repo(&repo) {
        eprintln!("gitpush: {folder} is not a git repository");
        return ExitCode::FAILURE;
    }

    // UI language: detected once from the system locale (as in gitlog)
    let locale = i18n::detect_locale();
    rust_i18n::set_locale(locale);

    let app = gtk::Application::builder()
        .application_id("dev.example.gitpush")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |app| {
        // Thicker scrollbars (same as gitlog); push-button margin via CSS
        // (GtkBox lost the `margin` property in GTK 4.18)
        if let Some(display) = gtk::gdk::Display::default() {
            let provider = gtk::CssProvider::new();
            provider.load_from_data(
                "scrollbar.horizontal slider { min-height: 12px; margin: 2px; }
                 scrollbar.vertical slider { min-width: 12px; margin: 2px; }
                 .push-buttons { margin: 6px; }",
            );
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let remotes = gl::remotes(&repo);
        let title_branch = gl::head_info(&repo);
        // no default height: the window fits tightly around the content
        let window = gtk::Window::builder()
            .title(t!("push.title", branch = &title_branch).as_ref())
            .default_width(720)
            .build();
        window.set_application(Some(app));

        // ---- form: three grouped sections ---------------------------------------
        // group 1: remote
        let remote_combo = gtk::ComboBoxText::new();
        remote_combo.append_text(&t!("push.all_remotes"));
        for r in &remotes {
            remote_combo.append_text(r);
        }
        remote_combo.set_active(Some(0));
        let remote_frame = gtk::Frame::builder()
            .label(t!("push.group.remote").as_ref())
            .margin_start(8)
            .margin_top(8)
            .margin_end(8)
            .margin_bottom(8)
            .build();
        let combo = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_start(8)
            .margin_end(8)
            .margin_bottom(8)
            .build();
        combo.append(&remote_combo);
        remote_frame.set_child(Some(&combo));

        // group 2: branches
        let all_branches = gtk::CheckButton::builder()
            .label(t!("push.all_branches").as_ref())
            .margin_start(8)
            .margin_bottom(4)
            .build();
        let local_entry = gtk::Entry::builder().hexpand(true).build();
        local_entry.set_text(&gl::current_branch(&repo));
        let remote_entry = gtk::Entry::builder().hexpand(true).build();
        // empty = keep the local name; the placeholder documents that
        let placeholder = t!("push.remote_branch");
        remote_entry.set_placeholder_text(Some(placeholder.as_ref()));
        let branches_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_start(8)
            .margin_end(8)
            .margin_bottom(8)
            .build();
        branches_box.append(&all_branches);
        add_row(&branches_box, &t!("push.local_branch"), &local_entry);
        add_row(&branches_box, &t!("push.remote_branch"), &remote_entry);
        let branches_frame = gtk::Frame::builder()
            .label(t!("push.group.branches").as_ref())
            .margin_start(8)
            .margin_top(8)
            .margin_end(8)
            .margin_bottom(8)
            .build();
        branches_frame.set_child(Some(&branches_box));

        // group 3: push options
        let force_check = gtk::CheckButton::builder()
            .label(t!("push.force").as_ref())
            .margin_start(8)
            .margin_bottom(4)
            .build();
        let lease_check = gtk::CheckButton::builder()
            .label(t!("push.force_with_lease").as_ref())
            .margin_start(24)
            .build();
        let tags_check = gtk::CheckButton::builder()
            .label(t!("push.tags").as_ref())
            .margin_start(8)
            .margin_bottom(4)
            .build();
        let upstream_check = gtk::CheckButton::builder()
            .label(t!("push.set_upstream").as_ref())
            .margin_start(24)
            .build();
        let force_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_start(8)
            .margin_bottom(4)
            .build();
        force_row.append(&force_check);
        force_row.append(&lease_check);
        let options_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_start(8)
            .margin_bottom(4)
            .build();
        options_row.append(&tags_check);
        options_row.append(&upstream_check);
        let options_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_start(8)
            .margin_end(8)
            .margin_bottom(8)
            .build();
        options_box.append(&force_row);
        options_box.append(&options_row);
        let options_frame = gtk::Frame::builder()
            .label(t!("push.group.options").as_ref())
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
        form.append(&branches_frame);
        form.append(&options_frame);

        // ---- buttons -----------------------------------------------------------
        let cancel_button = gtk::Button::builder()
            .label(t!("push.cancel").as_ref())
            .build();
        let push_button = gtk::Button::builder()
            .label(t!("push.push").as_ref())
            .build();
        push_button.add_css_class("suggested-action");
        let button_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        button_box.add_css_class("push-buttons");
        button_box.set_halign(gtk::Align::End);
        button_box.append(&cancel_button);
        button_box.append(&push_button);

        let outer = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        outer.append(&form);
        outer.append(&button_box);
        window.set_child(Some(&outer));

        // ---- checkbox behaviour --------------------------------------------------
        // "Push all branches": hide the branch-name fields and uncheck
        // "set upstream" (it only makes sense per branch)
        {
            let local_e = local_entry.clone();
            let remote_e = remote_entry.clone();
            let upstream = upstream_check.clone();
            all_branches.connect_toggled(move |check| {
                let s = !check.is_active();
                if !s {
                    upstream.set_active(false);
                }
                local_e.set_sensitive(s);
                remote_e.set_sensitive(s);
                upstream.set_sensitive(s);
            });
        }

        // force and force-with-lease are alternatives: checking one
        // unchecks the other
        {
            let lease = lease_check.clone();
            force_check.connect_toggled(move |check| {
                if check.is_active() {
                    lease.set_active(false);
                }
            });
            let force = force_check.clone();
            lease_check.connect_toggled(move |check| {
                if check.is_active() {
                    force.set_active(false);
                }
            });
        }

        // ---- push ---------------------------------------------------------------
        // the push runs in a background thread (network can take a long
        // time); its result is sent back through a channel polled from a
        // timeout on the main loop
        let busy = Arc::new(AtomicBool::new(false));
        {
            let remotes = remotes.clone();
            let local_e = local_entry.clone();
            let remote_e = remote_entry.clone();
            let all_b = all_branches.clone();
            let force_c = force_check.clone();
            let lease_c = lease_check.clone();
            let tags_c = tags_check.clone();
            let upstream_c = upstream_check.clone();
            let combo = remote_combo.clone();
            let win = window.clone();
            let repo = repo.clone();
            let busy = busy.clone();
            let button = push_button.clone();
            push_button.connect_clicked(move |_| {
                if busy.load(Ordering::Relaxed) {
                    return;
                }
                // "All remotes" is item 0; the other items follow the
                // gl::remotes() order
                let idx = combo.active().unwrap_or(0);
                let remotes_to_push: Vec<String> = if idx == 0 {
                    remotes.clone()
                } else {
                    remotes
                        .get(idx as usize - 1)
                        .map(|r| vec![r.clone()])
                        .unwrap_or_default()
                };
                if remotes_to_push.is_empty() {
                    error_dialog(&win, &t!("error.no_remotes").into_owned());
                    return;
                }
                let all = all_b.is_active();
                let local = local_e.text().trim().to_string();
                let remote_name = remote_e.text().trim().to_string();
                let force = force_c.is_active();
                let lease = lease_c.is_active();
                let tags = tags_c.is_active();
                let upstream = upstream_c.is_active();

                // one `git push` per remote
                let mut commands: Vec<Vec<String>> = Vec::new();
                for remote in &remotes_to_push {
                    let mut args = vec!["push".to_string()];
                    if tags {
                        args.push("--tags".into());
                    }
                    if force {
                        args.push("--force".into());
                    }
                    if lease {
                        args.push("--force-with-lease".into());
                    }
                    args.push(remote.clone());
                    if all {
                        args.push("--all".into());
                    } else if !local.is_empty() {
                        if upstream {
                            args.push("--set-upstream".into());
                        }
                        // keep the name if the remote-branch entry is empty
                        let refspec = if remote_name.is_empty() {
                            local.clone()
                        } else {
                            format!("{local}:{remote_name}")
                        };
                        args.push(refspec);
                    }
                    // empty local: no refspec, git pushes the current
                    // branch/HEAD (default rules)
                    commands.push(args);
                }

                let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
                let rx = Arc::new(Mutex::new(rx));
                busy.store(true, Ordering::Relaxed);
                button.set_sensitive(false);
                let win2 = win.clone();
                let busy2 = busy.clone();
                let button2 = button.clone();
                glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
                    match rx.lock().unwrap().try_recv() {
                        Ok(Ok(())) => {
                            busy2.store(false, Ordering::Relaxed);
                            win2.close();
                        }
                        Ok(Err(e)) => {
                            busy2.store(false, Ordering::Relaxed);
                            button2.set_sensitive(true);
                            error_dialog(&win2, &t!("error.push_failed", error = &e).into_owned());
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
                    let mut first: Option<String> = None;
                    for (i, args) in commands.into_iter().enumerate() {
                        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                        if let Err(msg) = gl::run_git(&repo, &refs) {
                            // with several remotes attribute each failure to
                            // its remote
                            let block = remotes_to_push
                                .get(i)
                                .map(|r| format!("{r}:\n{msg}"))
                                .unwrap_or(msg);
                            first = Some(match first {
                                Some(prev) => format!("{prev}\n\n{block}"),
                                None => block,
                            });
                        }
                    }
                    let _ = tx.send(match first {
                        None => Ok(()),
                        Some(e) => Err(e),
                    });
                });
            });
        }

        // ---- cancel / close -------------------------------------------------------
        // while a push is in flight, closing the window would leave the git
        // process running after the app quits (std waits for non-daemon
        // threads at exit), so terminate the process instead; an
        // interrupted push is safe (refs are updated only at the end)
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
    });

    // run with only the program name: the folder argument must not be
    // interpreted by GApplication as a file to open (same as gitlog)
    let prog = env::args().next().unwrap_or_default();
    ExitCode::from(app.run_with_args(&[prog]).get())
}

#[cfg(test)]
mod push_title_tests {
    // Verify the new strings resolve in every locale without touching the
    // global locale (same pattern as the gitcommit i18n tests).

    #[test]
    fn push_title_resolves_in_every_locale() {
        let branch = "main";
        assert_eq!(rust_i18n::t!("push.title", locale = "en", branch = &branch), "git push - main");
        assert_eq!(rust_i18n::t!("push.title", locale = "de", branch = &branch), "git push - main");
        assert_eq!(rust_i18n::t!("push.title", locale = "fr", branch = &branch), "git push - main");
        assert_eq!(rust_i18n::t!("push.title", locale = "es", branch = &branch), "git push - main");
        assert_eq!(rust_i18n::t!("push.title", locale = "it", branch = &branch), "git push - main");
        assert_eq!(rust_i18n::t!("push.title", locale = "nl", branch = &branch), "git push - main");
        assert_eq!(rust_i18n::t!("push.title", locale = "pt", branch = &branch), "git push - main");
    }
}

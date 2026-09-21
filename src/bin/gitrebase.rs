//! gitrebase: a window to edit the details of an existing commit
//! (see dev/rebase-concept.md).
//!
//! Author and committer (name, e-mail, date in local time) plus the commit
//! message, with Cancel/Apply buttons. The commit's file changes are not
//! touched.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

extern crate gtk4 as gtk;

use gtk::prelude::*;
use rust_i18n::t;

rust_i18n::i18n!("locales", fallback = "en");

use gitlog::gitlog as gl;
use gitlog::i18n;

/// Structural check of "YYYY-MM-DD HH:MM:SS" (the values themselves are not
/// checked - git rejects dates that do not exist).
fn is_date_string(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 19 {
        return false;
    }
    (0..4).all(|i| b[i].is_ascii_digit())
        && b[4] == b'-'
        && (5..7).all(|i| b[i].is_ascii_digit())
        && b[7] == b'-'
        && (8..10).all(|i| b[i].is_ascii_digit())
        && b[10] == b' '
        && (11..13).all(|i| b[i].is_ascii_digit())
        && b[13] == b':'
        && (14..16).all(|i| b[i].is_ascii_digit())
        && b[16] == b':'
        && (17..19).all(|i| b[i].is_ascii_digit())
}

/// One "name + e-mail + date" row of editable entries.
fn person_row(
    name_ph: &str,
    email_ph: &str,
    date_ph: &str,
) -> (gtk::Box, gtk::Entry, gtk::Entry, gtk::Entry) {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .build();
    let make = |ph: &str| {
        let e = gtk::Entry::builder().hexpand(true).build();
        e.set_placeholder_text(Some(ph));
        e
    };
    let name = make(name_ph);
    let email = make(email_ph);
    let date = make(date_ph);
    // the date has a fixed "YYYY-MM-DD HH:MM:SS" format; only that much
    // width is needed, and it must not claim a third of the row
    date.set_hexpand(false);
    date.set_width_chars(19);
    row.append(&name);
    row.append(&email);
    row.append(&date);
    (row, name, email, date)
}

fn entry_text(e: &gtk::Entry) -> String {
    e.text().to_string()
}

fn error_dialog(parent: &gtk::Window, text: &str) {
    let dialog = gtk::MessageDialog::builder()
        .transient_for(parent)
        .message_type(gtk::MessageType::Error)
        .text(text)
        .buttons(gtk::ButtonsType::Ok)
        .build();
    dialog.set_property("modal", true);
    dialog.connect_response(|dialog, _response| dialog.close());
    dialog.show();
}

fn main() -> ExitCode {
    let folder = env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let hash = match env::args().nth(2) {
        Some(h) if !h.is_empty() => h,
        _ => {
            eprintln!("gitrebase: usage: gitrebase <folder> <commit-hash>");
            return ExitCode::FAILURE;
        }
    };
    let repo = match PathBuf::from(&folder).canonicalize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("gitrebase: cannot open {folder}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if !gl::is_repo(&repo) {
        eprintln!("gitrebase: {folder} is not a git repository");
        return ExitCode::FAILURE;
    }
    let details = match gl::commit_details(&repo, &hash) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("gitrebase: cannot read commit {hash}: {e}");
            return ExitCode::FAILURE;
        }
    };
    // brief warning for commits that may have been pushed
    let pushed = gl::is_pushed(&repo, &hash);

    // UI language: detected once from the system locale (as in gitlog)
    let locale = i18n::detect_locale();
    rust_i18n::set_locale(locale);

    let app = gtk::Application::builder()
        .application_id("dev.example.gitrebase")
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
                 .rebase-warning { color: @warning_color; }
                 .rebase-buttons { margin: 6px; }",
            );
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let window = gtk::Window::builder()
            .title(t!("rebase.title", hash = &hash).as_ref())
            .default_width(720)
            // the message box keeps vexpand, so this controls its size:
            // the window is shorter than before so the message area ends up
            // at roughly half its old height
            .default_height(370)
            .build();
        window.set_application(Some(app));

        // ---- author / committer rows -----------------------------------------
        let name_ph = t!("rebase.name").into_owned();
        let email_ph = t!("rebase.email").into_owned();
        let date_ph = t!("rebase.date").into_owned();
        let (a_row, a_name, a_email, a_date) = person_row(&name_ph, &email_ph, &date_ph);
        let (c_row, c_name, c_email, c_date) = person_row(&name_ph, &email_ph, &date_ph);
        // each row gets a header label so it is clear which fields belong
        // to the author and which to the committer
        let person_section = |header: &str, row: &gtk::Box| {
            let section = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .margin_start(8)
                .margin_end(8)
                .build();
            let label = gtk::Label::builder()
                .label(header)
                .xalign(0.0)
                .margin_bottom(2)
                .build();
            label.add_css_class("heading");
            section.append(&label);
            section.append(row);
            section
        };
        let a_section = person_section(t!("rebase.author").as_ref(), &a_row);
        let c_section = person_section(t!("rebase.committer").as_ref(), &c_row);
        a_name.set_text(&details.author);
        a_email.set_text(&details.author_email);
        a_date.set_text(&details.author_date);
        c_name.set_text(&details.committer);
        c_email.set_text(&details.committer_email);
        c_date.set_text(&details.committer_date);
        // group frame around both rows (same structure/padding as gitpush)
        let details_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_bottom(8)
            .build();
        details_box.append(&a_section);
        details_box.append(&c_section);
        let details_frame = gtk::Frame::builder()
            .label(t!("rebase.group.details").as_ref())
            .margin_start(8)
            .margin_top(8)
            .margin_end(8)
            .margin_bottom(8)
            .build();
        details_frame.set_child(Some(&details_box));

        // ---- pushed warning ----------------------------------------------------
        let warning = gtk::Label::builder()
            .label(t!("rebase.pushed").as_ref())
            .margin_start(10)
            .margin_end(10)
            .margin_top(8)
            .build();
        warning.add_css_class("rebase-warning");
        warning.set_visible(pushed);

        // ---- message -----------------------------------------------------------
        let message_view = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::Word)
            .left_margin(8)
            .right_margin(8)
            .top_margin(8)
            .build();
        message_view.buffer().set_text(&details.message);
        let msg_scroll = gtk::ScrolledWindow::builder().vexpand(true).build();
        msg_scroll.set_child(Some(&message_view));
        // group frame around the message (same structure/padding as gitpush)
        let msg_frame = gtk::Frame::builder()
            .label(t!("rebase.message").as_ref())
            .margin_start(8)
            .margin_top(8)
            .margin_end(8)
            .margin_bottom(8)
            .build();
        msg_frame.set_child(Some(&msg_scroll));

        // ---- buttons -------------------------------------------------------------
        let cancel_button = gtk::Button::builder()
            .label(t!("rebase.cancel").as_ref())
            .build();
        let apply_button = gtk::Button::builder()
            .label(t!("rebase.apply").as_ref())
            .build();
        apply_button.add_css_class("suggested-action");
        let button_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        button_box.add_css_class("rebase-buttons");
        button_box.set_halign(gtk::Align::End);
        button_box.append(&cancel_button);
        button_box.append(&apply_button);

        let outer = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        outer.append(&warning);
        outer.append(&details_frame);
        outer.append(&msg_frame);
        outer.append(&button_box);
        window.set_child(Some(&outer));

        {
            let win = window.clone();
            cancel_button.connect_clicked(move |_| win.close());
        }
        {
            let entries = (
                a_name.clone(),
                a_email.clone(),
                a_date.clone(),
                c_name.clone(),
                c_email.clone(),
                c_date.clone(),
            );
            let buffer = message_view.buffer().clone();
            let repo = repo.clone();
            let hash = hash.clone();
            let win = window.clone();
            apply_button.connect_clicked(move |_| {
                let (a_name, a_email, a_date, c_name, c_email, c_date) = &entries;
                let message = buffer
                    .text(&buffer.start_iter(), &buffer.end_iter(), false)
                    .to_string();
                if message.trim().is_empty() {
                    error_dialog(&win, &t!("error.empty_message"));
                    return;
                }
                let author_date = entry_text(a_date);
                let committer_date = entry_text(c_date);
                if !is_date_string(&author_date) || !is_date_string(&committer_date) {
                    error_dialog(&win, &t!("error.invalid_date"));
                    return;
                }
                let details = gl::CommitDetails {
                    author: entry_text(a_name),
                    author_email: entry_text(a_email),
                    author_date,
                    committer: entry_text(c_name),
                    committer_email: entry_text(c_email),
                    committer_date,
                    message,
                };
                match gl::edit_commit_details(&repo, &hash, &details) {
                    Ok(()) => win.close(),
                    Err(e) => error_dialog(&win, &t!("error.rebase_failed", error = &e)),
                }
            });
        }

        window.show();
    });

    // run with only the program name: the folder and hash arguments must
    // not be interpreted by GApplication as files to open (same as gitlog)
    let prog = env::args().next().unwrap_or_default();
    ExitCode::from(app.run_with_args(&[prog]).get())
}

#[cfg(test)]
mod rebase_date_tests {
    use super::is_date_string;

    #[test]
    fn date_structure() {
        assert!(is_date_string("2026-09-20 17:36:12"));
        assert!(!is_date_string(""));
        assert!(!is_date_string("2026/09/20 17:36:12"));
        assert!(!is_date_string("2026-09-20T17:36:12"));
        assert!(!is_date_string("2026-09-20 17:36"));
        assert!(!is_date_string("abcd-ef-gh ij:kl:mn"));
    }
}

#[cfg(test)]
mod rebase_title_tests {
    // Verify the new strings resolve in every locale without touching the
    // global locale (same pattern as the gitlog i18n tests).

    #[test]
    fn rebase_title_resolves_in_every_locale() {
        let hash = "5f5d34f";
        let expected = "git rebase - 5f5d34f";
        assert_eq!(rust_i18n::t!("rebase.title", locale = "en", hash = &hash), expected);
        assert_eq!(rust_i18n::t!("rebase.title", locale = "de", hash = &hash), expected);
        assert_eq!(rust_i18n::t!("rebase.title", locale = "fr", hash = &hash), expected);
        assert_eq!(rust_i18n::t!("rebase.title", locale = "es", hash = &hash), expected);
        assert_eq!(rust_i18n::t!("rebase.title", locale = "it", hash = &hash), expected);
        assert_eq!(rust_i18n::t!("rebase.title", locale = "nl", hash = &hash), expected);
        assert_eq!(rust_i18n::t!("rebase.title", locale = "pt", hash = &hash), expected);
    }
}

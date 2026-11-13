//! Integration tests for the translated UI strings of all windows
//! (gitlog, gitcommit, gitpush, gitrebase).
//!
//! Every `locale = "..."` lookup is passed explicitly, so the global locale
//! is never touched and the tests cannot race each other.

rust_i18n::i18n!("locales", fallback = "en");

const LOCALES: [&str; 7] = ["en", "de", "fr", "es", "it", "nl", "pt"];

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
    assert_eq!(rust_i18n::t!("title.done", locale = "en", repo = &repo, branch = &branch, count = n), "git log - acme - main - 3 commits");
    assert_eq!(rust_i18n::t!("title.done", locale = "de", repo = &repo, branch = &branch, count = n), "git log - acme - main - 3 Commits");
    assert_eq!(rust_i18n::t!("title.done", locale = "it", repo = &repo, branch = &branch, count = n), "git log - acme - main - 3 commit");
    assert_eq!(rust_i18n::t!("title.done", locale = "nl", repo = &repo, branch = &branch, count = n), "git log - acme - main - 3 commits");
}

#[test]
fn rebase_edit_resolves_in_every_locale() {
    assert_eq!(rust_i18n::t!("rebase.edit", locale = "en"), "Edit commit details");
    assert_eq!(rust_i18n::t!("rebase.edit", locale = "de"), "Commit-Details bearbeiten");
    assert_eq!(rust_i18n::t!("rebase.edit", locale = "fr"), "Modifier les détails du commit");
    assert_eq!(rust_i18n::t!("rebase.edit", locale = "es"), "Editar detalles del commit");
    assert_eq!(rust_i18n::t!("rebase.edit", locale = "it"), "Modifica dettagli del commit");
    assert_eq!(rust_i18n::t!("rebase.edit", locale = "nl"), "Commitdetails bewerken");
    assert_eq!(rust_i18n::t!("rebase.edit", locale = "pt"), "Editar detalhes do commit");
}

#[test]
fn commit_title_resolves_in_every_locale() {
    let branch = "main";
    let expected = "git commit - main";
    for locale in LOCALES {
        assert_eq!(rust_i18n::t!("commit.title", locale = locale, branch = &branch), expected);
    }
}

#[test]
fn push_title_resolves_in_every_locale() {
    let branch = "main";
    let expected = "git push - main";
    for locale in LOCALES {
        assert_eq!(rust_i18n::t!("push.title", locale = locale, branch = &branch), expected);
    }
}

#[test]
fn rebase_title_resolves_in_every_locale() {
    let hash = "5f5d34f";
    let expected = "git rebase - 5f5d34f";
    for locale in LOCALES {
        assert_eq!(rust_i18n::t!("rebase.title", locale = locale, hash = &hash), expected);
    }
}

#[test]
fn context_menu_keys_resolve_in_every_locale() {
    for key in ["ctx.open_diff", "ctx.revert", "ctx.ignore_file", "ctx.ignore_dir"] {
        for locale in LOCALES {
            let s = rust_i18n::t!(key, locale = locale);
            assert!(!s.is_empty(), "{key} missing in {locale}");
            assert_ne!(s, key, "{key} missing in {locale}");
        }
    }
    assert_eq!(rust_i18n::t!("ctx.ignore_dir", locale = "en"), "Add directory to .gitignore");
    assert_eq!(rust_i18n::t!("ctx.ignore_dir", locale = "de"), "Ordner zu .gitignore hinzufügen");
}

#[test]
fn restore_error_resolves_and_interpolates_in_every_locale() {
    let e = "boom";
    assert_eq!(rust_i18n::t!("error.restore_failed", locale = "en", error = &e), "Revert failed: boom");
    assert_eq!(rust_i18n::t!("error.restore_failed", locale = "de", error = &e), "Verwerfen fehlgeschlagen: boom");
    assert_eq!(rust_i18n::t!("error.restore_failed", locale = "fr", error = &e), "Échec de l'annulation : boom");
    assert_eq!(rust_i18n::t!("error.restore_failed", locale = "nl", error = &e), "Verwerpen mislukt: boom");
    // the gitignore failure message exists in every locale too
    for locale in LOCALES {
        let s = rust_i18n::t!("error.gitignore_failed", locale = locale, error = &e);
        assert!(s.contains("boom"), "error.gitignore_failed missing in {locale}");
    }
}

#[test]
fn pull_title_resolves_in_every_locale() {
    for locale in LOCALES {
        let text = rust_i18n::t!("pull.title", locale = locale, branch = "main");
        assert!(
            text.starts_with("git pull - ") && text.ends_with("main"),
            "{locale}: {text}"
        );
    }
}

#[test]
fn pull_action_labels_resolve_in_every_locale() {
    for locale in LOCALES {
        for key in [
            "pull.action.fetch",
            "pull.action.merge",
            "pull.action.rebase",
            "pull.squash",
            "pull.no_ff",
            "pull.ff_only",
            "pull.tags",
            "pull.prune",
            "pull.cancel",
            "pull.pull",
            "pull.fetch",
            "pull.group.remote",
            "pull.group.branch",
            "pull.group.action",
            "pull.group.options",
            "pull.remote_branch",
            "pull.remote_branch_hint",
            "pull.rebase_in_progress",
            "pull.merge_in_progress",
        ] {
            let text = rust_i18n::t!(key, locale = locale);
            assert!(!text.is_empty() && text != key, "{locale}: {key} -> {text}");
        }
    }
}

#[test]
fn conflict_labels_resolve_in_every_locale() {
    for locale in LOCALES {
        for key in [
            "conflicts.file",
            "conflicts.resolved",
            "conflicts.status.both_mod",
            "conflicts.status.both_add",
            "conflicts.status.both_del",
            "conflicts.status.added_us",
            "conflicts.status.added_them",
            "conflicts.status.deleted_us",
            "conflicts.status.deleted_them",
            "conflicts.abort",
            "conflicts.commit_merge",
            "conflicts.continue_rebase",
            "error.conflicts_remain",
            "error.conflict_spreadsheet",
        ] {
            let text = rust_i18n::t!(key, locale = locale);
            assert!(!text.is_empty() && text != key, "{locale}: {key} -> {text}");
        }
    }
}

#[test]
fn conflicts_titles_interpolate_in_every_locale() {
    for locale in LOCALES {
        let merge = rust_i18n::t!("conflicts.title.merge", locale = locale, count = 3);
        assert!(merge.contains('3'), "{locale}: {merge}");
        let rebase = rust_i18n::t!("conflicts.title.rebase", locale = locale, step = 2, total = 5);
        assert!(rebase.contains('2') && rebase.contains('5'), "{locale}: {rebase}");
    }
    assert_eq!(
        rust_i18n::t!("conflicts.title.rebase", locale = "en", step = 2, total = 5),
        "Rebase conflicts - step 2 of 5"
    );
    assert_eq!(
        rust_i18n::t!("conflicts.title.merge", locale = "de", count = 3),
        "Merge-Konflikte - 3 Dateien"
    );
}

#[test]
fn pull_error_messages_interpolate_in_every_locale() {
    for locale in LOCALES {
        let text = rust_i18n::t!("error.pull_failed", locale = locale, error = "boom");
        assert!(text.contains("boom"), "{locale}: {text}");
    }
}

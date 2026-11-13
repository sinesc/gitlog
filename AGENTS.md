# gitlog

GTK4 toolset for working with git. The name "gitlog" is historic: the
crate ships several binaries (`src/bin/`): `gitlog` (history viewer), `gitcommit`,
`gitpush`, `gitrebase`. Shared logic lives in the library crate (`src/`).

## Stack & Gotchas

- Rust edition 2024, gtk4 crate 0.11.4 + glib 0.22.9 (API differs from old gtk-rs
  docs; GTK 4.18 has no `GtkKeyShortcut` - bind keys via `EventControllerKey`).
- The lib crate is named `gitlog` - watch for crate-name import shadowing.
- `glib` `day_of_week()` returns 1=Monday..7=Sunday (not 0-6).
- Untracked files: use `git status --porcelain=v1 --untracked-files=all`.
- Amend mode: unchecking a file resets its index entry to HEAD^ via
  `uncommit_path` (src/gitlog.rs) - never bare `git rm --cached`.
- `git commit-tree` does not preserve the original author; set `GIT_AUTHOR_*`.

## i18n & Misc

- rust-i18n with 7 locales in `locales/*.json` (en, de, fr, es, it, nl, pt); all
  7 stay in lockstep, placeholders use `%{name}`, 2-space indent; see `src/i18n.rs`.
- Plans/concepts live in `dev/*.md`.
- Headless GUI tests: Xvfb + `import` screenshots (xdotool keys don't reach the GTK4
  window).

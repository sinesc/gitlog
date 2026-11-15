# gitlog

GTK4 toolset for working with git. The name "gitlog" is historic: the
crate ships several binaries (`src/bin/`): `gitlog` (history viewer), `gitcommit`,
`gitpush`, `gitpull`, `gitrebase`. Shared logic lives in the library crate (`src/`).

## Stack & Gotchas

- Rust edition 2024, gtk4 crate 0.11.4 + glib 0.22.9 (API differs from old gtk-rs
  docs; GTK 4.18 has no `GtkKeyShortcut` - bind keys via `EventControllerKey`).
- The lib crate is named `gitlog` - watch for crate-name import shadowing.
- `glib` `day_of_week()` returns 1=Monday..7=Sunday (not 0-6).
- Untracked files: use `git status --porcelain=v1 --untracked-files=all`.
- Amend mode: unchecking a file resets its index entry to HEAD^ via
  `uncommit_path` (src/gitlog.rs) - never bare `git rm --cached`.
- `git commit-tree` does not preserve the original author; set `GIT_AUTHOR_*`.
- `git rebase` silently DROPS local commits whose replay becomes empty or
  duplicate (amending a pushed commit does exactly that) and still exits 0;
  gitpull detects it by comparing the local-only commit lists around the pull
  (`dropped_rebase_commits` in src/gitlog.rs, counterpart = patch-id then
  subject) and offers an undo (`git reset --hard <pre-pull head>`).
- A pull must never let git open an editor: pass `GIT_EDITOR=true` and
  `--no-edit`/`--no-rebase` (divergent pulls otherwise also need the
  strategy spelled out).

## i18n & Misc

- rust-i18n with 7 locales in `locales/*.json` (en, de, fr, es, it, nl, pt); all
  7 stay in lockstep, placeholders use `%{name}`, 2-space indent; see `src/i18n.rs`.
- Plans/concepts live in `dev/*.md`.
- Headless GUI tests: Xvfb + `import` screenshots (xdotool keys don't reach the GTK4
  window).

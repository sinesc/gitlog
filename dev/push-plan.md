# Plan: `gitpush` (push window)

Implements `dev/push-concept.md` as a third binary in this crate, reusing
`src/gitlog.rs` (git access), `src/i18n.rs` (locale detection), the locale
files, and the nemo/build/install plumbing established by `gitlog` and
`gitcommit`.

## 1. Crate layout

- New `src/bin/gitpush.rs` → the push window (auto-discovered, no
  `Cargo.toml` change, as with `gitcommit`).
- Window app id: `dev.example.gitpush`, `NON_UNIQUE` flag as with the other
  binaries.
- Same CLI pattern: `gitpush <project-folder>`, canonicalize + `is_repo`
  check, same error style.
- Imports: `use gitlog::gitlog as gl; use gitlog::i18n;` (the `as gl`
  shadowing trick from gitcommit).
- Same CSS provider (12px scrollbar sliders).
- Title: `t!("push.title", branch = …)` with `gl::head_info(repo)`, e.g.
  `git push - main`.

## 2. Data layer (new in `src/gitlog.rs`)

- `remotes(repo) -> Vec<String>`:
  - `git remote` (one name per line). Used for the remote dropdown.
  - Empty list = repository without remotes (dropdown shows only
    "All remotes"; pushing then fails with git's own message in the dialog).
- Push itself goes through the existing `run_git(repo, args)` (already
  public, returns trimmed stderr on failure). The binary builds the arg
  list, `run_git` executes `git push …`.

Argument construction (in the binary):

| state | refspec / command |
|---|---|
| all branches, one remote | `push [--force\|--force-with-lease] [--tags] <remote> --all` |
| all branches, all remotes | loop `push … <remote> --all` per remote |
| single branch | `push [flags] <remote> <refspec>` / loop over remotes |

- Refspec: `local` if the remote-branch entry is empty (git keeps the name),
  else `local:remote`.
- Flags: `--force` **or** `--force-with-lease` (mutually exclusive in the UI),
  `--tags`, `--set-upstream` (only in single-branch mode; `--set-upstream`
  with `--all` is disabled in the UI).
- "All remotes": loop over `remotes(repo)`; concatenate the failures of all
  remotes into one error message (`"<remote>: <stderr>"` blocks separated by
  a blank line) so a single dialog shows everything.

## 3. UI layout (in `src/bin/gitpush.rs`)

Simple form window (no panes), default ~720x420:

```
Window  title: "git push - <branch>"
└─ Box(vertical)
   ├─ CheckButton  "Push all branches"
   ├─ Box(horizontal): Label "Remote"   > ComboBoxText
   ├─ Box(horizontal): Label "Local branch" > Entry   (prefilled, current branch)
   ├─ Box(horizontal): Label "Remote branch" > Entry  (empty = same name)
   ├─ Box(horizontal): CheckButton "Force"   | CheckButton "Force with lease"
   ├─ Box(horizontal): CheckButton "Include tags" | CheckButton "Set upstream branch"
   └─ Box(horizontal, end-aligned): Button "Cancel" | Button "Push" (suggested-action)
```

- Remote dropdown = `gtk::ComboBoxText` (Gtk4 has no native combo in the
  gtk-rs 0.11 API; `ComboBoxText::new()` + `append_text`):
  first item `t!("push.all_remotes")`, then each name from
  `gl::remotes(repo)`.
- Local branch entry prefilled via `gl::head_info(repo)` (empty in detached
  HEAD).
- Sensitivity rules:
  - "Push all branches" on → local/remote branch entries and the
    set-upstream checkbox become `sensitive(false)` (and back on toggle-off).
  - "Force" on → uncheck "Force with lease", and vice versa (enforced in
    both `connect_toggled` handlers; no shared state needed).

## 4. Push flow

- **Push** (button):
  1. Read all widget state; build the argument list (§2).
  2. Run in a background thread (network pushes can take minutes; the UI
     must not freeze). Pattern from gitlog: `mpsc::channel` +
     `glib::timeout_add_local` drain, or simpler `glib::idle_add_local`
     with a one-shot channel — decide while coding, keep it small.
  3. While running: guard with an `AtomicBool` so Cancel/close can't
     double-trigger; success → close window (exit 0); failure → error
     dialog, window stays open for retry.
- **Cancel**: close (same as gitcommit).

## 5. Error dialog (selectable message)

`MessageDialog` text is **not** selectable, and the concept requires the
user to be able to select/copy the error (e.g. to search for it). So
gitpush uses a small custom dialog instead of gitcommit's `error_dialog`:

- `gtk::Dialog` (or `gtk::Window`) transient for the main window, modal,
  containing a `TextView` (`editable(false)`, `cursor_visible(true)`,
  word wrap, same styling as gitlog's message pane) with the error text,
  and one "Close" button (`gtk::ButtonsType::Close` or a plain Button).
- The text view makes the message selectable and copyable.

## 6. i18n (all 7 locales: en, de, fr, es, it, nl, pt)

New keys (en values):

```json
"push.title":           "git push - %{branch}",
"push.all_branches":    "Push all branches",
"push.remote":          "Remote",
"push.all_remotes":     "All remotes",
"push.local_branch":    "Local branch",
"push.remote_branch":   "Remote branch",
"push.force":           "Force",
"push.force_with_lease":"Force with lease",
"push.tags":            "Include tags",
"push.set_upstream":    "Set upstream branch",
"push.cancel":          "Cancel",
"push.push":            "Push",
"error.push_failed":    "Push failed: %{error}"
```

- Translate into all 7 locale files (each file goes 29 → 42 keys, must stay
  in lockstep — the existing tests only check per-locale resolution).
- Test (gitcommit pattern): `push.title` resolves in every locale.

## 7. Tests

Unit tests in `src/gitlog.rs` (extend the existing fixture helpers):

- `remotes()`: fixture repo, `git remote add origin <other tmp repo>` →
  `["origin"]`; two remotes → both listed.
- Push end-to-end against a local **bare** repo (fast, no network):
  - `git init --bare dest`, `git remote add origin dest` in the fixture,
  - `run_git(&repo, &["push", "origin", "main"])` → succeeds; the bare repo
    has branch `main` at the same hash.
  - `--force` push after moving a branch back (works where a plain push
    would be rejected as non-fast-forward).
  - `--tags`: `git tag v1`, push with `--tags`, tag exists in the bare repo.
  - `--set-upstream`: after `push -u origin main`, `git branch --show-current`
    tracks `origin/main` (verify via `git config` / `for-each-ref`).
  - Failure case: push a deleted remote branch → `Err` contains a useful
    stderr snippet (sanity check only, message wording is git's).

Manual test checklist (on a scratch repo + local bare "remote"):
normal push, rename on remote, force, force-with-lease, tags, upstream,
all branches, all remotes, error path (push to a bare repo with
`receive.denyNonFastForwards` / wrong force → selectable error dialog).

## 8. Nemo / build / docs

- New `res/gitpush.nemo_action`: clone of `gitcommit.nemo_action` with
  `Name=Git Push`, `Exec=gitpush %F`, `Icon-Name=mintgit`, same
  `Conditions=exec <check-git-dir.sh "%F">`.
- `install.sh`: install `target/release/gitpush` to `$BIN_DIR`, install
  `res/gitpush.nemo_action` via `install_action`, add a 7th
  `gitpush_comment` to the locale `case` (e.g. en: "Opens the git push
  window for the folder").
- `build.sh`: add `target/release/gitpush [path-to-git-repo]  # push window`
  to the "Run with" block.
- `README.md`: add `gitpush` to the Run, Install, and Implementation
  sections (third binary).

## 9. Milestones

1. **Data layer**: `remotes()` + push e2e tests (bare-remote fixture) green.
2. **i18n**: keys in all 7 locales + `push.title` resolution test.
3. **UI**: window layout, dropdown, entries, checkboxes, mutual exclusion,
   sensitivity rules.
4. **Wiring**: refspec/flag construction, all-remotes loop, background push,
   Cancel/close guard, success close.
5. **Error dialog** with selectable text view.
6. **Packaging**: nemo action, install.sh, build.sh, README; manual test
   run of all concept scenarios.

## 10. Open questions (resolved during implementation)

- Button label: confirmed **Push** (concept's "Commit" was a copy-paste).
- "Push all branches" + "Set upstream": checkbox disabled (and unchecked)
  in all-branches mode.
- Empty remote-branch entry = keep the local name; documented via the
  entry's placeholder text.
- Repo without remotes: dropdown shows only "All remotes"; Push shows a
  dedicated `error.no_remotes` dialog instead of a no-op success.

## 11. Implementation notes (deviations discovered while coding)

- The form box is `vexpand` so the Cancel/Push buttons stay at the bottom
  of the window (a plain vertical box top-aligns its children).
- Empty local branch entry (e.g. detached HEAD) pushes *no refspec*:
  git's default rules (current branch/HEAD). It never produces a
  `"<empty>:<remote>"` refspec, which would *delete* the remote branch.
- The push thread is non-daemon, so a window close while a push is in
  flight would leave git running after `app.run` returns; Cancel/close
  therefore `std::process::exit(0)` while busy (an interrupted push is
  safe: refs update only at the end). Cancel uses the same handler.
- `remotes_to_push` is a `Vec<String>` owned by the thread (a `Vec<&str>`
  borrowed from the click closure does not outlive the handler).
- `git remote` output is alphabetical; the combo follows that order.
- `str::as_str()` is unstable on this toolchain: use `&t!(...)` /
  `Cow::as_ref()` instead. gtk-rs 0.11: `ComboBoxText::set_active`
  takes `Option<i32>`, `Dialog::content_area()` is a `Box` (`append`).
- Error blocks from several remotes are concatenated in the single dialog,
  each prefixed with its remote name.

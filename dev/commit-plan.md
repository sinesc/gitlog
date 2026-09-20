# Plan: `gitcommit` (commit window)

Implements `dev/commit-concept.md` as a second binary in this crate, reusing
`src/gitlog.rs` (git access), `src/i18n.rs` (locale detection), and the locale
files.

## 1. Crate layout: shared lib + two binaries

Currently all shared code is pulled in from `src/main.rs` (`mod gitlog; mod
i18n;`). Restructure so both binaries share it:

- New `src/lib.rs`: `pub mod gitlog; pub mod i18n;`
- `src/main.rs` → thin `gitlog` binary: replace `mod gitlog; mod i18n;` with
  `use gitlog::…` (crate name is `gitlog`), keep the UI in `main.rs` (rename to
  `gitlog_app` module if desired, not required).
- New `src/bin/gitcommit.rs` → the commit window.
- `Cargo.toml`: no change needed (bins in `src/bin/` are auto-discovered);
  `cargo build --release` (already used by `build.sh`) builds both.
- Window app id: `dev.example.gitcommit`, same `NON_UNIQUE` flag as gitlog.
- Same CLI pattern as gitlog: `gitcommit <project-folder>`, canonicalize +
  `is_repo` check, same usage/error style.

## 2. Data layer (new functions in `src/gitlog.rs`)

Follow the existing style: shell out to `git` with `-C repo`, `LC_ALL=C`.

### `worktree_changes(repo) -> Vec<WorkFile>`

Files that differ between the working tree and `HEAD`, plus untracked files:

- `git -c core.quotepath=false diff -M --name-status HEAD`
  → `A|M|D|R<xx>` per path (renames: `R100\told\tnew`).
- `git -c core.quotepath=false diff -M --numstat HEAD`
  → added/deleted line counts (new path for renames); `-` = binary → `None`,
  displayed as `•` like `numstat()` in main.rs.
- `git diff --cached -M --name-status HEAD`
  → set of **staged** paths, used to pre-check the checkbox.
- `git status --porcelain=v1` lines starting with `??`
  → untracked files (status "new", not staged).

Merge into `WorkFile { path: String, display_name: String, action: Action,
added: Option<i64>, deleted: Option<i64>, staged: bool, in_head: bool }`.
`display_name` uses `"old => new"` for renames (consistent with gitlog).
`in_head` = tracked at `HEAD` (i.e. appears in `git diff HEAD`, not untracked).

### `head_files(repo) -> Option<Vec<WorkFile>>`

The files of the previous commit (`None` if there is no commit yet → amend
checkbox stays disabled):

- `git -c core.quotepath=false diff -M --name-status --numstat HEAD^ HEAD`
- Same `WorkFile` shape, all `staged = true` (they are in the index),
  `in_head = true`.

### `head_message(repo) -> Option<String>`

- `git log -1 --format=%B HEAD` (trim trailing newlines) — used to pre-fill
  the message when amending.

### commit/cancel helpers

Small wrappers that run and return `io::Result<bool>/status`:

- `stage_path(repo, path)` → `git add -A -- <path>`
- `unstage_path(repo, path)` → `git restore --staged -- <path>`
- `rm_cached_path(repo, path)` → `git rm --cached -q -r -- <path>`
- `index_matches_head(repo) -> bool` → `git diff --cached --quiet HEAD`
  (nothing staged to commit)
- `do_commit(repo, message, amend) -> io::Result<()>` →
  `git commit [-m message] [--amend]`

Quoting: pass paths as separate argv elements (already the pattern in
`write_blob`), so no shell-escaping issues.

## 3. UI layout (in `src/bin/gitcommit.rs`)

```
Window  title: "git commit - <branch>"   (default 700x560)
└─ Box(vertical)
   ├─ Paned(vertical)
   │  ├─ start: Box(vertical)
   │  │  ├─ ScrolledWindow > TextView        (commit message, editable, wrap)
   │  │  └─ CheckButton "Amend last commit"
   │  └─ end:  ScrolledWindow > TreeView     (file list)
   └─ Box(horizontal, end-aligned)
      ├─ Button "Cancel"
      └─ Button "Commit"  (css class "suggested-action")
```

- File list columns (ListStore + `CellRendererText`, `CellRendererToggle`):
  | column | type | width |
  |---|---|---|
  | stage | toggle, editable | ~24, fixed |
  | file name | text, expand, ellipsize | fixed base 400 (like gitlog) |
  | status | text | ~90 |
  | lines added | text, right-aligned | ~90 |
  | lines removed | text, right-aligned | ~90 |
- ListStore types: `[BOOL, STRING, STRING, STRING, STRING]`.
- Toggle handler: `connect_toggled` → read row index, write new `active` back
  into the store (no git side effects yet — git ops happen on Commit only).
- Title: `t!("commit.title", branch = …)` with `gitlog::head_info(repo)`, e.g.
  `git commit - main`.
- Same CSS provider trick as gitlog (12px scrollbar sliders).
- Window close = Cancel (just quit).

## 4. Non-amend mode (default)

- File list = `worktree_changes(repo)`.
- Checkbox pre-checked iff the path is already staged (`staged == true`).
- Status labels: A → `t!("action.added")`-style new/modified/deleted/renamed
  (reuse `action.*` keys: added → "new"? no — see §7, add
  `status.new/modified/deleted/renamed` keys reusing `action.*` values if
  wording matches).
- **Commit** (message non-empty, else show error dialog):
  1. For each row: checked & not staged → `stage_path`; unchecked & staged →
     `unstage_path`. (Checked & staged / unchecked & unstaged: nothing.)
  2. `index_matches_head(repo)`? → error dialog "nothing to commit" (nothing
     ended up staged).
  3. `do_commit(repo, message, amend=false)`; on success: quit (exit 0);
     on failure: error dialog with git stderr, window stays open.
- **Cancel**: quit.

## 5. Amend mode

Toggling the checkbox on (and back off): reload the file list, and when
switching on, pre-fill the message box with `head_message(repo)`.

- File list = `head_files(repo)` (all pre-checked) +
  `worktree_changes(repo)` (pre-checked too — work changes while amending are
  meant for this commit; the user can uncheck).
  - Rows are keyed by path; worktree entries for paths also in `head_files`
    are **merged** (status reflects the worktree-vs-HEAD diff, added/deleted
    counts from `diff HEAD`).
- **Commit**:
  1. For each row:
     - unchecked & `in_head` → `rm_cached_path` (removes from the index, which
       is the base for the amend → file leaves the amended commit; the
       worktree file is untouched → shows as new/unstaged afterwards).
     - checked → `stage_path` (picks up the current worktree state).
     - unchecked & not in head → nothing (left unstaged).
  2. `do_commit(repo, message, amend=true)`; same success/failure handling.
- Amend is disabled (checkbox insensitive) when `head_files` returns `None`
  (empty repo / no HEAD).

### Edge cases

- **Rename in `head_files`**: operate on the *new* path only (the index holds
  the new name).
- **Detached HEAD**: `head_info` shows `(detached @ <sha>)`; commit/amend
  work normally.
- **Empty message**: `Commit` button insensitive (update on buffer
  `changed` signal); amend reuses HEAD's message by default so it is rarely
  empty.
- **Binary files**: counts `None` → `•` (reuse the `numstat` display logic;
  move it to `gitlog.rs` or duplicate the 6-line helper).
- **Errors** (git missing, lock held, etc.): `gtk::MessageDialog`
  (`MessageType::Error`), window stays open so the user can retry. gitlog
  currently only `eprintln`s; gitcommit needs the dialog because the user is
  mid-action.

## 6. i18n (all 7 locales: en, de, fr, es, it, nl, pt)

New keys (en values):

```json
"commit.title":      "git commit - %{branch}",
"commit.amend":      "Amend last commit",
"commit.cancel":     "Cancel",
"commit.commit":     "Commit",
"col.status":        "Status",
"status.new":        "new",
"error.empty_message":  "Please enter a commit message.",
"error.nothing_staged": "Nothing to commit (no files are checked/staged).",
"error.commit_failed":  "Commit failed: %{error}"
```

Add a test in `main.rs`-style pattern: `commit.title` resolves in every
locale (mirrors the existing `title_done` test).

## 7. Tests

- Unit tests in `src/gitlog.rs` (reuse the existing `fixture_repo()` helpers):
  - `worktree_changes`: after modifying a.txt, deleting d.txt, adding
    new.txt (untracked), moving a file → correct names, actions, staged
    flags, added/deleted counts; binary file → `None`.
  - `head_files` / `head_message` on the fixture repo.
  - End-to-end amend test in a temp repo: commit 2 files, modify one,
    `rm --cached` the other, `commit --amend` → amended commit contains the
    right files, the removed file still exists in the worktree untracked.
- i18n resolution test for `commit.title` in all locales.
- Manual test checklist (dev/commit-concept.md scenarios) verified by running
  `target/release/gitcommit` on a scratch repo.

## 8. Milestones

1. **Crate restructure**: `src/lib.rs`, `gitlog` binary unchanged behaviour;
   `cargo build --release` + existing tests green.
2. **Data layer**: `worktree_changes`, `head_files`, `head_message`, commit
   helpers + unit tests.
3. **UI**: window layout, message box, file list with toggles, branch title,
   i18n keys in all 7 locales + test.
4. **Wiring**: non-amend commit/cancel flow with error dialogs.
5. **Amend**: checkbox, list reload, pre-filled message, amend commit flow.
6. **Polish**: `build.sh`/`README.md` mention both binaries; manual test run
   of all concept scenarios (amend uncheck leaves file unstaged, etc.).

## 9. Open questions (resolved during implementation)

- Pre-checking worktree changes in amend mode: implemented as **yes**.
- `gitcommit` with no argument: implemented (uses `$PWD`).

## 10. Implementation notes (deviations discovered while coding)

- gtk-rs 0.11 / GTK 4.18: `CellRendererToggle` has no `editable` property
  (toggle cells are activatable by default), `GtkBox` lost the `margin`
  property (CSS class `commit-buttons` used instead), `Dialog::run()` does
  not exist (`connect_response` + `show()`), `TextBuffer::text()` needs
  start/end iters, `connect_toggled` passes `(cell, TreePath)`.
- Import gotcha: `use gitlog::gitlog;` binds a name `gitlog` at the crate
  root that shadows the crate in *subsequent* use paths (edition 2024).
  The binaries therefore import it as `use gitlog::gitlog as gl;`.
- `git show` shows only one diff format at a time, so `head_files` runs
  two commands (`--name-status` and `--numstat`).

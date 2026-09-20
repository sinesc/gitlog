# Implementation plan: `gitrebase` (commit detail editing)

Goal: a separate binary `gitrebase` (no nemo integration) that opens a window
titled `git rebase - <abbreviated hash>` and lets the user edit the author,
committer and message of one existing commit. No file changes, no reordering.
A context-menu entry in `gitlog`'s commit list opens it.

Scope: commit-detail editing only. The rewritten commit's tree is unchanged.

## 1. Shared library: `src/gitlog.rs`

### 1.1 `CommitDetails` + `commit_details()`

```rust
pub struct CommitDetails {
    pub author: String,
    pub author_email: String,
    pub author_date: String,       // "YYYY-MM-DD HH:MM:SS" (local time)
    pub committer: String,
    pub committer_email: String,
    pub committer_date: String,    // "YYYY-MM-DD HH:MM:SS" (local time)
    pub message: String,
}

pub fn commit_details(repo: &Path, hash: &str) -> Result<CommitDetails, String>
```

Single command, NUL-separated so names/emails with spaces or `<>` are safe:

```
git log -1 --date=format-local:%Y-%m-%d\ %H:%M:%S \
    --format=%an%x00%ae%x00%ad%x00%cn%x00%ce%x00%cd%x00%B <hash>
```

(`--date=format-local:…` is passed as one argv item; `%B` is last because it
contains newlines.) Split on `\0`, take the 6 fields, the rest is the message
(trimmed of trailing newlines, as `load_commits` does).

### 1.2 `is_pushed()`

```rust
pub fn is_pushed(repo: &Path, hash: &str) -> bool
```

`git branch -r --contains <hash>` — non-empty output means the commit is
reachable from a remote-tracking branch.

### 1.3 `edit_commit_details()`

```rust
pub fn edit_commit_details(repo: &Path, hash: &str, d: &CommitDetails)
    -> Result<(), String>
```

Steps (all git calls capture stderr into the error string, like `do_commit`):

1. Resolve: `git rev-parse -q --verify <hash>^{commit}`.
2. Require a clean tree: `git status --porcelain` must be empty
   (needed for the rebase path; keeps the HEAD path uniform).
3. Build the amend command, run with extra environment:

   ```
   env GIT_COMMITTER_NAME   = d.committer
       GIT_COMMITTER_EMAIL  = d.committer_email
       GIT_COMMITTER_DATE   = d.committer_date   # local time, git parses it
   git commit --amend -m <d.message>
        --author "<d.author> <d.author_email>"
        --date <d.author_date>
   ```

   (`--author` + `--date` set the author fields; the three `GIT_COMMITTER_*`
   vars set the committer fields; `-m` sets the message exactly as `do_commit`
   uses it.)

4. Two paths (mechanism verified against a temp repo):
   - **`hash` == `git rev-parse HEAD`**: run the amend command directly.
   - **Otherwise** (works for any non-HEAD ancestor of the current HEAD):
     - Base: `git rebase -i <hash>^`; if `<hash>^` does not resolve (root
       commit) use `git rebase -i --root`.
     - Env `GIT_SEQUENCE_EDITOR="sed -i '1s/^pick/edit/'"` so the first todo
       line becomes `edit <hash>` and the rebase stops at that commit
       (verified: git exits 0 while stopped).
     - Run the amend command.
     - Run `git rebase --continue` to replay the commits above (their trees
       are untouched, so this applies cleanly).
5. Failure semantics: if `rebase --continue` or any step fails, return git's
   stderr. A rebase stopped part-way leaves the repo in a rebase state that
   the user can `git rebase --abort` — acceptable for v1, the error dialog
   shows git's own message.

## 2. New binary: `src/bin/gitrebase.rs`

Auto-discovered by Cargo (`src/bin/*.rs`), same skeleton as
`src/bin/gitcommit.rs`: `extern crate gtk4 as gtk`, `rust_i18n::i18n!("locales",
fallback = "en")`, locale via `i18n::detect_locale()`, app id
`dev.example.gitrebase`, `NON_UNIQUE` flag, run via `app.run_with_args(&[prog])`
so the hash argument is not treated as a file to open.

- Arguments: `gitrebase <folder> <hash>`. Missing hash → `eprintln!` +
  `ExitCode::FAILURE`. Folder canonicalized and checked with `gl::is_repo`
  (same as gitcommit).
- At startup: `gl::commit_details(&repo, &hash)` — failure → `eprintln!` +
  `ExitCode::FAILURE` (before any GTK window).
- Window title: `t!("rebase.title", hash = &hash)`.
- Layout (vertical `Box`, default size ~720×480):
  1. Pushed-warning `Label` (top margin, `visible` only when
     `gl::is_pushed(&repo, &hash)`), text `t!("rebase.pushed")`, CSS class
     `rebase-warning` with `color: @warning_color;` loaded in the same
     `CssProvider` the other binaries use for scrollbars.
  2. Author row: horizontal `Box` of three `Entry`s (name, email, date),
     equal weight (`set_hexpand(true)`), pre-filled from `CommitDetails`,
     placeholders `t!("rebase.author")` / `t!("rebase.email")` /
     `t!("rebase.date")`.
  3. Committer row: same shape.
  4. Commit message: `TextView` (word wrap, editable) in a
     `ScrolledWindow` with `vexpand(true)`, pre-filled with
     `details.message`.
  5. Button box (`halign` End): Cancel, Apply (`suggested-action`).
- Validation helper `fn is_date_string(s: &str) -> bool`: 19 chars, digits in
  the digit positions, `-` at 4/7, `:` at 11/14 (structure only — the
  semantic check is left to git).
- Apply:
  - Collect the 5 text fields + message.
  - If message is empty → error dialog `t!("error.empty_message")` (existing
    key, reused).
  - If either date fails `is_date_string` → error dialog
    `t!("error.invalid_date")`.
  - Otherwise `gl::edit_commit_details(...)`: `Ok` → `win.close()`;
    `Err(e)` → error dialog `t!("error.rebase_failed", error = &e)`, window
    stays open (same pattern as gitcommit's commit button).
- Cancel: close the window.
- Error dialog: copy the `error_dialog` helper from `gitcommit.rs`.

## 3. Locale files (all 7, kept in lockstep, 2-space indent, `%{name}`
placeholders)

Add to each of `locales/{en,de,fr,es,it,nl,pt}.json`:

| Key | en |
|---|---|
| `rebase.title` | `git rebase - %{hash}` |
| `rebase.author` | `Author` |
| `rebase.email` | `Email` |
| `rebase.date` | `Date (YYYY-MM-DD HH:MM:SS)` |
| `rebase.committer` | `Committer` |
| `rebase.message` | `Commit message` |
| `rebase.cancel` | `Cancel` |
| `rebase.apply` | `Apply` |
| `rebase.pushed` | `Warning: this commit may have been pushed. Rewriting pushed commits can cause problems for other contributors.` |
| `rebase.edit` | `Edit commit details` |
| `error.rebase_failed` | `Rewriting the commit failed: %{error}` |
| `error.invalid_date` | `Invalid date: expected format YYYY-MM-DD HH:MM:SS.` |

## 4. `gitlog` context menu: `src/main.rs`

Connect on the commit list tree view (`ui.log_tree`):

```rust
ui.log_tree.connect_populate_popup_menu(|_tree, path| -> gtk::PopoverMenu { … })
```

- If `path` is `None` (click on empty area), return an empty
  `gtk::PopoverMenu::builder().build()`.
- Otherwise resolve the hash exactly like the selection-changed handler:
  `log_filter.convert_path_to_child_path(&path)` → `log_store.iter(&store_path)`
  → `LOG_HASH`. Store it in an `Rc<RefCell<Option<String>>>` created with the
  menu setup.
- Build a `PopoverMenu` with one item: `item = "rebase-edit"`, label
  `t!("rebase.edit")`; `connect_activated` → read the stashed hash and
  `Command::new("gitrebase").args([&repo, &hash]).spawn()`
  (`eprintln!` on spawn failure — `gitrebase` is expected on `PATH`,
  installed like the other binaries; same lookup style as the existing
  `meld` launch).

## 5. Packaging & docs

- `install.sh`: add `install -m 0755 target/release/gitrebase "$BIN_DIR/gitrebase"`
  next to the other binaries (no nemo action).
- `build.sh`: add the `target/release/gitrebase <path-to-git-repo> <hash>`
  line to the "Run with:" block.
- `README.md`: add `gitrebase` to the installed-binaries list, the usage
  examples, and the binary descriptions.

## 6. Tests

In `src/gitlog.rs` (temp-dir repos, using the existing `git()` test helper):

- `commit_details_fields`: repo with a commit made under custom
  `GIT_AUTHOR_NAME/EMAIL/DATE` and `GIT_COMMITTER_NAME/EMAIL/DATE`; assert
  all 7 fields come back, dates in `YYYY-MM-DD HH:MM:SS`.
- `edit_head_amend`: single commit, change all fields, assert via
  `git log -1 --format=…` that author, committer, both dates and message
  changed and the file content is untouched.
- `edit_middle_commit_rebase`: 3 commits, edit the middle one (author, dates,
  message); assert the new middle commit has the new fields, the first and
  third commits are intact (same subjects, original author), the branch tip
  moved, working tree clean.
- `edit_root_commit`: 2 commits, edit the root (exercises the `--root` path).
- `is_pushed`: bare repo as local "remote" + `git push` (pattern of the
  existing push test) → `true`; no remote configured → `false`.

In `src/bin/gitrebase.rs`: locale-resolution test for `rebase.title`
(interpolating a hash) across all 7 locales, same style as the existing
`commit_title_resolves_in_every_locale` test.

## Verification

- `~/.cargo/bin/cargo build` (cargo is not on PATH).
- `~/.cargo/bin/cargo test`.
- Manual: run `gitlog` (shared Xvfb display, see project notes), right-click a
  commit → "Edit commit details", change fields, Apply; confirm new history
  with `git log --format=fuller`.

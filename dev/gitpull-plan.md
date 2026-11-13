# Plan: `gitpull` (pull window)

Adds a fifth binary `src/bin/gitpull.rs` (auto-discovered by cargo, no
`Cargo.toml` change), UI layout cloned from `gitpush`: a small form window
with grouped `GtkFrame` sections and Cancel/Pull buttons at the bottom.
All git logic goes into `src/gitlog.rs` (testable), the binary only wires
widgets to it (existing rule of thumb).

## 1. UI layout

Mirrors `gitpush` exactly: `default_width(720)`, no default height,
`vexpand` form box so the buttons stay at the bottom, same CSS provider
(12px scrollbar sliders + `.pull-buttons { margin: 6px; }`), app id
`dev.example.gitpull`, `NON_UNIQUE` flag, same CLI pattern
(`gitpull <folder>`, canonicalize + `is_repo`, `run_with_args(&[prog])`),
same locale detection (`i18n::detect_locale`).

```
Window  title: t!("pull.title", branch)  -> "git pull - main"
└─ Box(vertical)
   ├─ Frame "Remote"
   │  └─ ComboBoxText  (the remotes from gl::remotes(); NO "All remotes"
   │     entry - pulling from all remotes at once is not meaningful)
   ├─ Frame "Branch"
   │  └─ Row: Label "Remote branch" > Entry (hexpand)
   ├─ Frame "Action"
   │  ├─ RadioButton "Fetch only"
   │  ├─ RadioButton "Fetch and merge"   <- default active
   │  └─ RadioButton "Fetch and rebase"
   ├─ Frame "Pull options"
   │  ├─ CheckButton "Squash commits"
   │  ├─ Box(horizontal): CheckButton "No fast-forward" | CheckButton "Fast-forward only"
   │  └─ Box(horizontal): CheckButton "Fetch tags" | CheckButton "Prune deleted branches"
   └─ Box(horizontal, end-aligned): Button "Cancel" | Button "Pull"
      (suggested-action; label switches to "Fetch" in fetch-only mode)
```

- Radio buttons: `gtk::CheckButton::builder().label(...)` and joining the
  group: `btn2.set_group(&btn1)` (a joined CheckButton renders as a radio
  in GTK 4). Default active = "Fetch and merge".
- Remote combo: items in `gl::remotes()` order; active = the remote of the
  current branch's upstream when one exists, else index 0. Repo without
  remotes: Push-button click shows the existing `error.no_remotes` dialog
  (key already exists).
- Branch entry: prefilled with the branch part of the current upstream
  (see `upstream_of` below); placeholder `t!("pull.remote_branch_hint")`
  ("empty = upstream of the current branch; fetch: everything").
  Empty entry means: no refspec for pull (`git pull <remote>`, git uses
  the current branch's configured refspec) and plain `git fetch <remote>`
  in fetch-only mode.

### Option sensitivity matrix (the core UI rule)

| action     | squash | no-ff | ff-only | tags | prune |
|------------|--------|-------|---------|------|-------|
| fetch only | off    | off   | off     | on   | on    |
| fetch+merge| on     | on    | on      | on   | on    |
| fetch+rebase| off   | off   | off     | on   | on    |

Rationale: `--squash/--no-ff/--ff-only` are merge options; git rejects
`--squash` combined with `--rebase` and ignores `--ff-only/--no-ff` with
`--rebase`; a fetch alone merges nothing. `--tags`/`--prune` are fetch
options and work in every mode.

Implementation: on every radio `connect_toggled`, recompute the six
`sensitive()` flags and **uncheck** every checkbox that just became
insensitive (so hidden state can never leak into the command):

```rust
fn apply_option_sensitivity(mode: Mode, opts: &[(&gtk::CheckButton, bool); 5]) {
    for (check, enabled) in opts {
        check.set_sensitive(enabled);
        if !enabled {
            check.set_active(false);
        }
    }
}
```

`no-ff` and `ff-only` are mutually exclusive while both enabled
(checking one unchecks the other - the exact force/lease pattern from
gitpush).

## 2. Command construction (in the binary)

| mode | command |
|---|---|
| fetch only | `fetch [--prune] [--tags] <remote> [<branch>]` |
| fetch+merge | `pull [--squash] [--no-ff\|--ff-only] [--prune] [--tags] <remote> [<branch>]` |
| fetch+rebase | `pull --rebase [--prune] [--tags] <remote> [<branch>]` |

Flags map 1:1 from the checkboxes; `<branch>` is omitted when the entry
is empty. Execution uses the gitpush threading pattern verbatim:
`std::thread::spawn` + `mpsc::channel` drained by
`glib::timeout_add_local(500ms)`, `AtomicBool busy` guard, button
insensitive while running, Cancel/close during a running pull calls
`std::process::exit(0)` (the git child must not outlive the app; an
interrupted fetch/pull is safe - git updates refs only at the end).

Success handling per mode:

- fetch only: close the window (exit 0), like a successful push.
- fetch+merge **without** squash: close the window.
- fetch+merge **with** squash: git leaves the combined changes staged and
  creates no commit. Close the window and spawn `gitcommit` on the repo
  (`std::process::Command::new("gitcommit").arg(&repo).spawn()`,
  `eprintln!` on failure - same "sibling binary on PATH" pattern gitlog
  uses to spawn `gitrebase`), so the user writes the commit message and
  picks files there.
- fetch+rebase: close the window.

Failure handling: see §4 (conflict detection) - everything else goes
into gitpush's selectable-text `error_dialog` (copy that ~30-line helper
into gitpull; it cannot be imported across binaries).

## 3. Data layer additions (`src/gitlog.rs`)

```rust
/// Remote + branch of the current branch's upstream, e.g.
/// ("origin", "main"). None on a detached HEAD or without upstream.
pub fn upstream_of(repo: &Path) -> Option<(String, String)> {
    let out = std::process::Command::new("git")
        .args(["-C", &repo.to_string_lossy(),
               "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"])
        .output().ok().filter(|o| o.status.success())?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    // "origin/main" -> remote is the first path component (git forbids
    // slashes in remote names, branch parts may contain them)
    let (remote, branch) = text.trim().split_once('/')?;
    Some((remote.to_string(), branch.to_string()))
}

/// Absolute path of the .git directory (also correct for worktrees,
/// where .git is a file).
/// `git rev-parse --absolute-git-dir`, trimmed stdout.
pub fn git_dir(repo: &Path) -> Option<PathBuf>

/// Unmerged (conflicted) paths from `git status --porcelain=v1 -uall`.
/// A row is conflicted when either letter is 'U', or for the
/// both-deleted case "DD". Returns (path, xy) e.g. ("a.txt", "UU").
pub fn unmerged_files(repo: &Path) -> Vec<(String, String)>

/// The three conflict stages of a path: base (:1), ours (:2),
/// theirs (:3). Missing stages (add/add has no base, deletes lack
/// one side) come back as None.
pub fn conflict_blobs(repo: &Path, path: &str) -> (Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>)

/// True while a merge is stopped mid-conflict (.git/MERGE_HEAD exists).
pub fn merge_in_progress(repo: &Path) -> bool  // git_dir(repo)/MERGE_HEAD

/// Progress of an in-progress rebase: (step, total, head branch).
/// Reads <git-dir>/rebase-merge/{msgnum,end,head-name}; falls back to
/// the legacy <git-dir>/rebase-apply/{num,total,head-name}. None when
/// no rebase is running.
pub fn rebase_progress(repo: &Path) -> Option<(u32, u32, String)>

/// Run a git command with extra environment variables. Extracted from
/// run_git, which delegates to it (no behavior change for callers).
pub fn run_git_env(repo: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<(), String>

/// Finish a resolved merge: `git commit --no-edit` (uses git's auto
/// merge message; --no-edit avoids spawning an editor).
pub fn commit_merge(repo: &Path) -> Result<(), String>

/// Continue a resolved rebase step. GIT_EDITOR=true auto-confirms the
/// pre-filled commit message (no interactive editor).
pub fn rebase_continue(repo: &Path) -> Result<(), String>
    // run_git_env(repo, &["rebase", "--continue"], &[("GIT_EDITOR", "true")])
```

Marking a file resolved reuses the existing `stage_path` (`git add -A --`).

## 4. Conflict handling - solution proposal

**Proposal: a dedicated conflicts dialog inside gitpull; do NOT reuse
gitcommit.** Rationale (and the rejected alternative):

- *Rejected - reuse gitcommit:* after a conflicted merge gitcommit would
  list the whole worktree (not just conflicts), its double-click diff is
  a 2-way "HEAD vs worktree" comparison (no merge base / no "theirs"
  pane), and committing a merge requires MERGE_HEAD to stay intact while
  the user edits a message for an auto-generated merge commit. All three
  are fixable in gitcommit, but they add merge-state complexity to the
  commit window for a job it was not designed for.
- *Chosen:* gitpull owns the pull, so gitpull owns the stop state. A
  small dialog reuses the two proven primitives: gitcommit's temp-file
  + `meld` launch pattern and its checkbox file-list pattern.

### Dialog (shared by merge and rebase conflicts)

`gtk::Window` (transient-for the main window, modal, ~720x420):

```
Title: t!("conflicts.title.merge", count)  -> "Merge conflicts - 3 files"
       t!("conflicts.title.rebase", step, total) -> "Rebase conflicts - step 2 of 5"
└─ TreeView (ListStore)
   ├─ CheckButton col "Resolved"
   ├─ text col "File"
   └─ text col "Status"  (the porcelain XY code, e.g. "UU")
└─ Box(horizontal, end): Button "Abort" | Button "Continue"/"Commit merge"
   (suggested-action)
```

- Rows from `unmerged_files()`; `status_label` maps the XY code
  (`UU` -> "both modified", `AA` -> "both added", `DD` -> "both deleted",
  `AU/UA` -> "added by us/them", `DU/UD` -> "deleted by us/them",
  `AT/TA` -> "added by them/us, modified").
- **Double-click a row: 3-way meld.** `gl::conflict_blobs()` writes base
  (:1) and theirs (:3) to `$TMPDIR/gitpull-<pid>/` (flattened path,
  extension preserved - the gitcommit temp-file pattern; a missing stage
  becomes an empty file so meld always gets 3 panes). The **local pane is
  the real working-tree file** (`repo.join(path)`, which already carries
  the `<<<<<<<` conflict markers), so saving in meld writes back into
  the repository - same "only the other side is a temp copy" rule as
  gitcommit:

  ```rust
  std::process::Command::new("meld")
      .args([&base_tmp, &work_file, &theirs_tmp])
      .spawn()
  ```

  Spreadsheets: conflicts in `.ods/.xlsx` cannot be merged in Calc
  compare (it is a diff, not a 3-way merge) - show the selectable-text
  error dialog with a short "resolve this file manually" hint
  (`gl::is_spreadsheet()` check on the row).
- "Resolved" = user checkbox; applying happens on the action button
  (see below), not on every toggle.

### Merge flow

1. `pull` (merge mode) fails in the worker thread; the main-loop
   callback checks `merge_in_progress() && !unmerged_files().is_empty()`
   -> opens the conflicts dialog (title "merge"); otherwise plain
   `error_dialog`.
2. User resolves files in meld, ticks the checkboxes.
3. **Commit merge**: `gl::stage_path()` for every checked row, then
   `gl::commit_merge()`. Afterwards re-scan `unmerged_files()`: if any
   row is still unchecked, show the `error.conflicts_remain` dialog and
   stay; if git reports remaining conflicts (a row was resolved badly),
   refresh the list. Success -> close dialog, close window.
4. **Abort**: `git merge --abort` via `run_git`, close the dialog,
   window stays open for a retry (e.g. switch to fetch+rebase).

### Rebase flow (multi-step conflicts)

1. A conflicted `pull --rebase` leaves a rebase in progress; the failure
   callback reads `rebase_progress()` + `unmerged_files()` and opens the
   same dialog with the rebase title/step counter.
2. **Continue rebase**: `git add` the checked rows, then
   `gl::rebase_continue()` (`GIT_EDITOR=true`, auto message). Its
   failure re-enters the loop: re-read `unmerged_files()` and
   `rebase_progress()`; if new conflicts exist, refresh the list and the
   "step M of N" title and wait for the user again - conflicts in later
   commits are handled by the same dialog instance. Success with
   `rebase_progress() == None` -> rebase finished -> close dialog +
   window.
3. **Abort**: `git rebase --abort`, close dialog, main window stays.
4. **Startup resume**: `connect_activate` checks
   `rebase_progress()`/`merge_in_progress()` first. If a rebase is
   stopped (conflicted, or an earlier `edit`/`stop`), the gitpull window
   opens directly on top of the conflicts/continue dialog (step counter
   from `rebase_progress()`, list from `unmerged_files()` - an empty
   list + "Continue" covers the stopped-without-conflict case). While
   that state exists, clicking Pull shows the `pull.rebase_in_progress`
   info dialog instead of running git (git itself refuses to pull into
   an unfinished rebase). This makes gitpull a resume UI for interrupted
   pulls; plain `git rebase` runs started elsewhere are picked up too.

## 5. i18n (all 7 locales in lockstep: en, de, fr, es, it, nl, pt)

New keys (en values):

```json
"pull.title":               "git pull - %{branch}",
"pull.group.remote":        "Remote",
"pull.group.branch":        "Branch",
"pull.group.action":        "Action",
"pull.group.options":       "Pull options",
"pull.remote_branch":       "Remote branch",
"pull.remote_branch_hint":  "empty = upstream of the current branch",
"pull.action.fetch":        "Fetch only",
"pull.action.merge":        "Fetch and merge",
"pull.action.rebase":       "Fetch and rebase",
"pull.squash":              "Squash commits",
"pull.no_ff":               "No fast-forward",
"pull.ff_only":             "Fast-forward only",
"pull.tags":                "Fetch tags",
"pull.prune":               "Prune deleted branches",
"pull.cancel":              "Cancel",
"pull.pull":                "Pull",
"pull.fetch":               "Fetch",
"pull.rebase_in_progress":  "A rebase is already in progress. Continue or abort it below before pulling.",
"error.pull_failed":        "Pull failed: %{error}",
"conflicts.title.merge":    "Merge conflicts - %{count} files",
"conflicts.title.rebase":   "Rebase conflicts - step %{step} of %{total}",
"conflicts.file":           "File",
"conflicts.resolved":       "Resolved",
"conflicts.status.both_mod": "both modified",
"conflicts.status.both_add": "both added",
"conflicts.status.both_del": "both deleted",
"conflicts.status.added_us": "added by us",
"conflicts.status.added_them":"added by them",
"conflicts.status.deleted_us":"deleted by us",
"conflicts.status.deleted_them":"deleted by them",
"conflicts.abort":          "Abort",
"conflicts.commit_merge":   "Commit merge",
"conflicts.continue_rebase":"Continue rebase",
"error.conflicts_remain":   "Some files are not resolved yet.",
"error.conflict_spreadsheet":"Conflicts in spreadsheets must be resolved manually."
```

Reuse existing keys where possible (`error.close`, `error.no_remotes`).
Existing test pattern: add `pull_title_resolves_in_every_locale` and
`conflicts_title_rebase_resolves_in_every_locale` style tests in
`tests/translations.rs` (explicit `locale = "..."`, never touch the
global locale).

## 6. Tests (`tests/gitlog.rs` + shared fixtures in `tests/common/`)

Use the local bare-remote fixtures (`bare_remote`, `git_env`):

- `upstream_of`: after `push -u origin main` -> `("origin","main")`;
  fresh repo / detached HEAD -> `None`.
- `unmerged_files`: diverge two branches, `git merge` the other -> the
  modified file; add/add -> `AA`; delete-vs-modify -> `DU`/`UD`;
  both deleted -> `DD`.
- `conflict_blobs`: UU file returns all three sides with the expected
  contents; AA returns base `None`.
- `merge_in_progress` / `commit_merge`: failed merge -> true; `merge
  --abort` -> false; resolve + `commit_merge` -> merge commit with two
  parents (`git log -1 --format=%P`).
- `rebase_progress` / `rebase_continue`: local commits + diverged bare
  remote -> `pull --rebase` fails -> `Some((step,total,head))`; resolve,
  `rebase_continue` -> advances to the next conflict (multi-step repo:
  2 conflicting commits -> two continues), final state `None` and the
  remote's commits all present (`git log --oneline` count check).
- Pull e2e against the bare remote: plain fetch updates
  `origin/main` `for-each-ref`; `--prune` drops a remote-tracking branch
  deleted in the bare repo; `--tags` fetches a tag; merge pull
  fast-forwards; `--no-ff` produces a merge commit; `--ff-only` on a
  diverged repo returns `Err` with git's message; `--squash` leaves
  staged changes and **no** new commit (`has_head` unchanged,
  `git diff --cached HEAD` non-empty); `pull --rebase` rewrites the
  local commit on top of the remote tip.

Manual GUI checklist (Xvfb + `import` screenshots; fake `meld` script on
PATH logging its args, see project notes): normal pull, fetch-only,
rebase pull, option sensitivity switching per radio (incl. uncheck on
disable), no-ff/ff-only exclusivity, conflict dialog -> fake meld ->
resolve -> commit merge, multi-step rebase continue loop, abort paths,
squash -> gitcommit spawn, rebase-in-progress startup dialog.

## 7. Packaging & docs

- `res/gitpull.nemo_action`: clone of `gitpush.nemo_action` with
  `Name=Git Pull`, `Exec=gitpull %F`, same `Conditions=exec
  <check-git-dir.sh "%F">`.
- `install.sh`: `install -m 0755 target/release/gitpull "$BIN_DIR/gitpull"`,
  a `gitpull_comment` in every locale `case` arm (en: "Opens the git
  pull window for the folder"; translate like the existing push comments),
  `install_action res/gitpull.nemo_action "$gitpull_comment"`, and the
  two summary `echo` lines.
- `build.sh`: `target/release/gitpull [path-to-git-repo]  # pull window`
  in the "Run with" block.
- `README.md`: `gitpull` in the binary list, install list, nemo action
  list, and the Run-with example.
- `AGENTS.md`: binary list sentence (`gitlog`, `gitcommit`, `gitpush`,
  `gitrebase`, `gitpull`).

## 8. Milestones

1. **Data layer** in `src/gitlog.rs`: `upstream_of`, `git_dir`,
   `unmerged_files`, `conflict_blobs`, `merge_in_progress`,
   `rebase_progress`, `run_git_env` (refactor `run_git` to delegate),
   `commit_merge`, `rebase_continue` + all §6 tests green.
2. **i18n**: keys in all 7 locales + translation tests.
3. **UI**: form layout, combo, branch entry prefill, radios,
   sensitivity matrix + uncheck-on-disable, no-ff/ff-only exclusivity,
   dynamic Pull/Fetch button label.
4. **Wiring**: command construction, background thread + busy guard
   (gitpush pattern), success paths (close / squash -> gitcommit
   spawn), Cancel/close exit guard.
5. **Conflicts dialog**: list, 3-way meld on double-click, resolve
   checkboxes, Commit merge / Continue rebase / Abort, rebase loop with
   step counter, startup resume state.
6. **Packaging & docs** (§7) + manual GUI test run.

## 9. Open questions (resolved by this plan)

- **Conflicts + meld:** dedicated conflicts dialog in gitpull with a
  true 3-way meld (`:1` temp / real worktree file / `:3` temp); gitcommit
  is *not* reused for conflict resolution (2-way diff, whole-worktree
  list, merge-message friction), only *after* a successful `--squash`
  pull (spawned so the user commits the squashed result).
- **Multi-step rebase conflicts:** one dialog instance drives a loop -
  after each `rebase --continue` re-scan conflicts and step counter;
  plus startup resume (`rebase_progress()`) so an interrupted rebase is
  continuable/abortable from a freshly launched gitpull.
- No "All remotes" entry in the remote dropdown (unlike push).
- Empty branch entry = default refspec / full fetch (documented via the
  entry placeholder).
- Conflicted spreadsheets are listed but flagged "resolve manually"
  (Calc compare is a 2-way diff, not a 3-way merge).

## 10. Implementation notes (deviations discovered while coding)

- **Divergent pulls need an explicit strategy.** git >= 2.33 refuses
  `git pull` on divergent branches when `pull.rebase` is unset ("Need to
  specify how to reconcile divergent branches"). Merge mode therefore
  always passes `--no-rebase` explicitly.
- **No editor may ever spawn from the pull thread** (the git child has
  no terminal): merge mode passes `--no-edit` when not squashing (git's
  prepared merge message), and the thread runs git through
  `run_git_env` with `GIT_EDITOR=true`. `rebase_continue` uses the same
  env to auto-confirm the pre-filled commit message.
- **`git rebase --continue` returns Err when the *next* pick
  conflicts** (it commits the resolved step, then stops). The dialog
  loop keys on state, not the result code: `Err` +
  `rebase_progress().is_some()` is the normal "next conflict" path that
  refreshes list and step counter; `Ok` + `None` is "rebase finished".
- The conflicts window must fill its list right at startup (initial
  `refresh_conflicts()` call), the title alone was built first.
- `git merge origin/main` merges the *remote-tracking* ref: tests fetch
  explicitly first (plain `git pull` fetches by itself).
- Porcelain status `AA` contains no `U`; the `unmerged_files` filter
  matches `U`, `AA`, `DD`, `AT`, `TA` explicitly. AT/TA display as
  "added by them" (same as git status).
- A plain `DD` (both deleted) auto-resolves in straight histories with
  ort, so it is not exercised by the tests (the code maps the code).
- Test fixture: `bare_remote` now points the bare repo's HEAD at
  `refs/heads/main` via `symbolic-ref` - the legacy default `master`
  made `git clone` check out an empty unborn branch.
- The conflict list uses the gitcommit `CellRendererToggle` pattern (no
  CheckColumn); the "Resolved" ticks are applied (`git add`) when the
  action button runs, not per toggle.
- Startup resume and a Pull click in a merge/rebase state both open the
  conflicts window directly; `pull.rebase_in_progress` /
  `pull.merge_in_progress` (extra key vs the original list) show as the
  selectable info label at the top of that window.
- Verified headlessly (Xvfb :42 + `import`, fake `meld` on PATH):
  fast-forward pull (window closes, exit 0), merge-conflict startup
  window with the `a.txt / both modified` row, staged-resolution ->
  Commit merge (two-parent commit, windows close), staged-rebase
  resolution -> Continue rebase (finishes, exit 0), 3-way meld args
  `base.txt / real work file / theirs.txt`, fetch-mode disables
  squash/no-ff/ff-only and re-labels the button "Fetch".

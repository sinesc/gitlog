# Plan: report commits a rebase pull silently drops (+ amend warning)

## Problem

`gitpull` runs `git pull --rebase …` as one batch. git **drops local
commits whose replay becomes empty or duplicate** (the usual cause: the
user amended a commit after pushing it - e.g. gitcommit's amend with an
unchecked file, which removes that file's change from the pushed commit).
git prints only a stderr warning
(`dropping <sha> … patch contents already upstream` or
`warning: skipped previously applied commit <sha>`), **exits 0**, and
moves the branch to the remote tip - so gitpull reports success, closes
the window and the user's local commit vanishes.

Verified facts (git 2.55, scratch repos):

- The drop needs neither patch-id equality nor a message-only amend: a
  commit whose remaining changes are a *subset* of what upstream already
  has becomes empty on replay and is dropped.
- No rebase flag prevents it (`--reapply-cherry-picks`, `--allow-empty`,
  `--keep-empty`, `rebase.*` config all still drop it).
- Merge mode never drops (the amended commit survives in the merge).
- A dirty work tree makes `pull --rebase` fail loudly ("cannot pull with
  rebase: You have unstaged changes" / untracked-overwrite error), which
  already reaches the error dialog; the silent drop needs a clean tree
  (or `rebase.autoStash`).
- No hook exists for skipped commits *during* a rebase (`--exec` runs
  after real picks only), so a per-commit prompt mid-rebase is not
  possible. Post-pull report + Undo is the robust design; a pre-flight
  simulation (`git worktree add --detach` + rebase there) is the
  alternative if a before-the-fact gate is ever wanted - not implemented
  here.
- `git cherry` (patch-id vs upstream) does NOT detect the subset case;
  comparing local-only commit lists before/after does.

## 1. Detection (data layer, `src/gitlog.rs`)

```rust
/// A local commit: sha, subject and patch-id (`""` when the commit has
/// no diff of its own, e.g. a merge commit).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalCommit {
    pub sha: String,
    pub subject: String,
    pub patch_id: String,
}

/// Local-only commits reachable from `rev` but not from `upstream`
/// (newest first), each with its patch-id.
pub fn local_commits(repo: &Path, upstream: &str, rev: &str) -> Vec<LocalCommit>
// shas+subjects: git show -s --format=%H%x1f%s <upstream>..<rev>
// patch ids:    `git show <sha>` output piped into `git patch-id --stable`

/// The commits of `old_head` that the rebase dropped: local-only commits
/// with no counterpart in the new local-only set. Counterpart = same
/// patch-id (when non-empty), else same subject (a replayed commit keeps
/// both, while its sha changes).
pub fn dropped_rebase_commits(
    repo: &Path, upstream: &str, old_head: &str, new_head: &str,
) -> Vec<LocalCommit>

/// Ref the pull rebased onto, best effort: the first candidate that
/// resolves. Candidates, in order: `<remote>/<branch>` (when the branch
/// entry is not empty), `<remote>/<current branch>`, the current
/// branch's own upstream `<up_remote>/<up_branch>` (see `upstream_of`),
/// `FETCH_HEAD`. None when nothing resolves.
pub fn pull_upstream_ref(repo: &Path, remote: &str, branch: &str) -> Option<String>

/// Run a git command, returning stdout+stderr (trimmed) on success.
/// `run_git` stays as-is (it discards the output); the pull uses this so
/// git's own drop warnings can be shown.
pub fn run_git_capture(repo: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<String, String>
```

Undo is plain plumbing: `run_git(repo, &["reset", "--hard", <old_head>])`
(no new helper).

Known limitation: two local commits with the *same subject* can mask each
other (patch-id matching covers most real cases). Documented in the code.

## 2. gitpull wiring (`src/bin/gitpull.rs`)

- Click handler (rebase mode only): before spawning the thread, capture
  `old_head = git rev-parse HEAD` (via `run_git_capture(&repo,
  &["rev-parse", "HEAD"], &[])`).
- The worker thread uses `run_git_capture(..., GIT_EDITOR=true)` and
  sends `Result<String, String>` (output text on success) through the
  existing channel.
- In the success arm of the timeout callback, rebase mode:
  `pull_upstream_ref(repo, remote, branch)` + `dropped_rebase_commits(
  repo, &upstream_ref, &old_head, "HEAD")`.
  - Empty → current behaviour (close window, or spawn gitcommit in
    squash mode).
  - Non-empty → **do not close the window**; open a
    `dropped_dialog(parent, text, old_head)`.
- `dropped_dialog`: same shape as `error_dialog` (selectable `TextView`,
  modal, transient-for) plus an extra **Undo** button. Body text is built
  from i18n keys + the list lines (`  <short sha> <subject>`) + git's own
  drop lines filtered from the captured output (lines containing
  `dropping` or `skipped previously applied commit`).
  - Undo → `run_git(&repo, &["reset", "--hard", &old_head])`; on error
    `error_dialog` and the report stays open for a retry; on success the
    report closes and the **pull window stays open** (the user may retry
    with fetch+merge or push the amended commit with force-with-lease).
    Reset only moves the branch back; the remote tracking refs stay updated.
  - Close (`error.close`) → closes the report **and the pull window**:
    accepting the pulled state leaves nothing to do in the pull window.
    Closing a `GtkDialog` re-enters the response handler with
    `DELETE_EVENT`, so an `undone` flag keeps the dialog's own close event
    after a successful undo from closing the pull window too.
- Merge/fetch modes are unchanged (they cannot drop commits).

## 3. gitcommit prevention (`src/bin/gitcommit.rs`)

Warning label (same pattern as gitrebase's `rebase.pushed` label,
`set_visible(pushed)`): when **Amend last commit** is active *and*
`gl::is_pushed(&repo, "HEAD")` is true, show a selectable red-ish label
under the amend checkbox. `is_pushed` is evaluated once at startup (the
pre-amend HEAD is the pushed one). No behaviour change otherwise.

## 4. i18n (all 7 locales: en, de, fr, es, it, nl, pt; `%{name}` placeholders)

```json
"pull.dropped.title": "Rebase dropped local commits",
"pull.dropped.intro": "%{count} of your local commits were dropped because their changes were already in the remote (this usually happens when a commit is amended after it was pushed):",
"pull.dropped.git":     "git reported:",
"pull.dropped.hint":    "Nothing is lost yet - the commits are still in the reflog. Undo the pull with the button below, then either push the amended commit with \"force with lease\" (gitpush) or make the removal a separate commit.",
"pull.undo":            "Undo pull",
"error.undo_failed":    "Undoing the pull failed: %{error}",
"commit.pushed_amend":  "Warning: the last commit has already been pushed. Amending it rewrites published history - you need to force-push it (gitpush, force with lease), and a later pull --rebase would drop the amended commit."
```

Existing keys reused: `error.close`.

## 5. Tests

`tests/gitlog.rs` (fixtures from `tests/common`, extend the existing
`pull_repo` / `commit_and_push` helpers):

- `local_commits_lists_subjects_and_patch_ids`: two local commits ahead
  of a bare remote → list length 2, newest first, non-empty patch-ids;
  `git show <c> | git patch-id --stable` matches.
- `dropped_commits_plain_rebase_reports_nothing`: ordinary rebase of a
  local commit onto a moved remote → empty vec (no false positive; the
  replayed commit matched by patch-id).
- `dropped_commits_message_only_amend`: push, `commit --amend -m` →
  after `pull --rebase` the amended commit is reported (sha = the old
  amended sha, subject "feature (amended)").
- `dropped_commits_subset_amend` (the gitcommit uncheck case): commit
  modifies `a.txt` and `b.txt`, push, emulate `uncommit_path` +
  `--amend`, clean the work tree, `pull --rebase --autostash` (or a
  clean tree) → the commit is reported; assert additionally that
  `run_git(&["reset", "--hard", <old_head>])` restores it and that
  `b.txt` goes back to the amended (unchanged) content - the Undo path.
- `dropped_commits_mixed_history`: dropped commit + a genuine new local
  commit → exactly one reported entry (the genuine one is not listed).
- `pull_upstream_ref_candidates`: entry given → `origin/main`; empty
  entry → the branch's upstream; no remotes → `None`.
- `run_git_capture_returns_output`: `pull --rebase` in the message-only
  scenario returns the `skipped previously applied commit` line in the
  Ok payload (proves the decoration source; string comes from git).

`tests/translations.rs`: new keys resolve (non-empty, != key) in all 7
locales; `pull.dropped.intro` interpolates `%{count}`;
`error.undo_failed` interpolates `%{error}`; `commit.pushed_amend`
resolves everywhere.

## 6. Docs

- `README.md`: extend the `gitpull` bullet ("warns when the rebase
  dropped local commits, with an undo") and the `gitcommit` bullet is
  untouched.
- `AGENTS.md`: add one gotcha line under Stack & Gotchas: git rebase
  silently drops commits whose replay is empty/duplicate - gitpull
  reports them; counterpart matching is patch-id then subject.

## 7. Milestones

1. Data layer: `LocalCommit`, `local_commits`, `dropped_rebase_commits`,
   `pull_upstream_ref`, `run_git_capture` + their tests green.
2. i18n keys in all 7 locales + translation tests.
3. gitpull: capture `old_head`, switch the thread to `run_git_capture`,
   success path detects drops, `dropped_dialog` with Undo/Close.
4. gitcommit: pushed-amend warning label.
5. Docs (README, AGENTS.md) + headless GUI verification (autoclick hack:
   message-only amend repo → dialog appears, Undo restores the commit;
   clean pull → no dialog; amend-on-pushed warning visible in gitcommit).

## 8. Design decisions resolved

- Report **after** the pull rather than simulating it in a temp worktree:
  one replay instead of two, no conflict-state handling in the
  simulation, and the Undo button gives the user back exactly what the
  rebase took.
- Accepting the report closes the app (nothing left to do in the pull
  window after a completed pull), while Undo leaves the pull window open
  so the pull can be retried differently.
- Detection by commit-list comparison (patch-id, fallback subject), not
  by scraping git's message wording; git's own lines are shown only as
  decoration when present.
- Undo = `git reset --hard <old_head>`: safe here because the pull had
  already reset the work tree to the new HEAD, and the dialog appears
  immediately after it.
- `gitcommit`'s pushed warning is prevention, not a fix: the real cure
  for "un-commit a file from a pushed commit" is force-with-lease push
  or a separate removal commit - both are named in the dialog hint.

## 9. Implementation notes (deviations discovered while coding)

- The drop report stops the result-polling timeout (`ControlFlow::Break`)
  and re-enables the Pull button, so the pull window is still usable when
  the user picks Undo (accepting the report closes the whole app).
- `error_dialog` became generic (`<W: IsA<gtk::Window>>`) because the drop
  dialog's response handler (a `&Dialog`) has to be able to show errors.
- `remote`/`branch` are moved into the git argument vector, so the drop
  check keeps its own copies (`remote_for_ref`, `branch_for_ref`).
- `run_git_capture` appends stderr after stdout (git writes the fetch
  progress to stderr and its drop warnings there too); the pull thread
  uses it instead of `run_git_env`, still with `GIT_EDITOR=true`.
- The reported sha is truncated to 8 chars for the list (guarded, in case
  a caller ever passes an abbreviated sha).
- Test fixture detail: the "uncheck a file" scenario needs a file that the
  pushed commit *modifies*. If the commit added the file, unchecking it
  leaves the file untracked and `pull --rebase` aborts loudly over the
  untracked file instead of dropping silently - that is a different code
  path (the plain error dialog).
- `git cherry origin/main <head>` is asserted NOT to flag the subset case,
  which keeps the reason for choosing list comparison over patch-id
  matching pinned by a test.
- Headless GUI verification (container Xvfb; the shared `:42` socket was
  stale this session, Xvfb was started on `:18`): a rebase-mode pull in a
  message-only-amend repository shows the "Rebase dropped local commits"
  dialog with the dropped entry, git's own warning line and the Close /
  Undo pull buttons. Both response paths were driven with
  `DialogExtManual::response(...)`: **Close** quit the app by itself (exit
  0; the pulled state stands - branch at the remote commit, the amended
  message gone), **Undo** kept the app running (exit 124 = killed by the
  test timeout) and left the branch at the pre-pull sha with `b.txt` back
  to the amended content and a clean status. The gitcommit warning label
  appears only with "Amend last commit" active and a pushed HEAD
  (verified visible vs. hidden by screenshots); the temporary autoclick
  hacks were removed afterwards (absent from the release binaries).
- `GtkDialog::close()` re-enters `connect_response` with `DELETE_EVENT`, so
  the undo path needs a flag to stop its second pass from closing the pull
  window as well.

# gitcommit file-list context menu - plan

Right-clicking a row in the gitcommit file list (`src/bin/gitcommit.rs`) opens a
context menu whose top-left corner is at the mouse cursor, with four entries:

| Entry                       | Action                                                     | Enabled when |
|-----------------------------|------------------------------------------------------------|--------------|
| Open diff                   | same as double-click (meld, or Calc for spreadsheets)      | always |
| Revert changes              | `git restore --source=HEAD --staged --worktree -- <path>`  | file exists in HEAD, row is not a rename, amend mode off |
| Add file to .gitignore      | append `/<repo-relative path>` to `<repo>/.gitignore`      | amend mode off |
| Add directory to .gitignore | append the file's **parent directory** `/<dir>/`           | path contains `/` (file in a subdirectory), amend mode off |

Patterns are anchored with a leading `/` so they match exactly this file /
exactly this directory tree (e.g. for the row `path/to/file.txt` the entries
append `/path/to/file.txt` and `/path/to/`).

Rationale for the enable rules:
- `git restore` has nothing to restore for untracked files (deleting a brand-new
  user file would be too destructive) and the two-sided rename rows are ambiguous;
  those rows keep the entry insensitive.
- In "Amend last commit" mode the list shows HEAD's files, not working-tree
  changes, so only "Open diff" makes sense there.
- A file directly at the repo root has the repo root as its parent directory,
  which has no meaningful ignore pattern (it would cover the whole repo), hence
  "Add directory" is only meaningful (and only enabled) for files in a
  subdirectory.

## 1. New git helpers in `src/gitlog.rs`

Add next to the existing `stage_path` / `unstage_path` helpers (uses the
existing `run_git`):

```rust
/// Reset index and working tree of one path back to HEAD (discards the
/// uncommitted changes of that file; recreates it when it was deleted).
pub fn restore_path(repo: &Path, path: &str) -> Result<(), String> {
    run_git(repo, &["restore", "--source=HEAD", "--staged", "--worktree", "--", path])
}

/// Append one pattern line to `<repo>/.gitignore` (the file is created when
/// missing); a pattern that is already present verbatim is not added again.
pub fn append_gitignore(repo: &Path, pattern: &str) -> Result<(), String> {
    let file = repo.join(".gitignore");
    let existing = match std::fs::read_to_string(&file) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    if existing.lines().any(|l| l.trim() == pattern) {
        return Ok(());
    }
    let mut out = existing;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(pattern);
    out.push('\n');
    std::fs::write(&file, out).map_err(|e| e.to_string())
}
```

## 2. Extract the double-click handler body (`src/bin/gitcommit.rs`)

Move the body of the current `connect_row_activated` closure into a free
function:

```rust
/// Open one file list row's changes (HEAD version vs. working tree) in meld,
/// or in LibreOffice Calc for spreadsheet rows. `name` is the display name
/// (rename rows are "old => new").
fn open_diff(repo: &Path, name: &str) { /* existing temp-file/meld/Calc logic */ }
```

`connect_row_activated` then only looks up `W_NAME` and calls
`open_diff(&repo, &name)`; the context menu's "diff" action calls the same
function with the snapshot below.

## 3. Row snapshot for the menu

Extract a per-row reader from `read_row_data` (which already builds a
`RowData` per iter):

```rust
fn row_data(store: &gtk::ListStore, iter: &gtk::TreeIter) -> RowData { /* same fields */ }
```

`read_row_data` calls it in its loop. The menu handlers communicate through:

```rust
let ctx_row: Rc<std::cell::RefCell<Option<RowData>>> = Rc::default();
```

## 4. Menu + actions (in `connect_activate`, after the tree is built)

Use a plain `gtk::Popover` with a vertical `gtk::Box` (css class `menu`) of
flat `gtk::Button` rows bound to a `gio::SimpleActionGroup` (API verified
against gtk4-rs 0.11.4 / GTK 4.18):

GTK4 specifics that force this shape:
- `gtk::ModelButton` is not exported by GTK4's public API, so gtk4-rs has no
  binding for it; plain buttons with `set_action_name` are the substitute.
- `PopoverMenu` pre-selects its first item (keyboard navigation) and its
  arrow-key handling interferes with the tree list; plain buttons highlight
  on hover only and leave focus (and the arrow keys) with the tree.
- A no-arrow popover is positioned centered on its pointing rectangle
  (`left = rect.center.x - width/2`, `top = rect.bottom`) and *ignores*
  `set_offset` in that mode. To pin the popover's top-left corner at the
  cursor, point to the rectangle `(x, y - 1, w, 1)` where
  `w = popover.measure(gtk::Orientation::Horizontal, -1).1`; the centering
  math then lands the corner exactly at the mouse (GTK still keeps the final
  position on screen near window edges).

```rust
let popover = gtk::Popover::builder().has_arrow(false).build();
popover.set_parent(&files_tree);
popover.insert_action_group("ctx", Some(&actions));
let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
menu_box.add_css_class("menu");
popover.set_child(Some(&menu_box));
for (label, action) in [
    (t!("ctx.open_diff"), "ctx.diff"),
    (t!("ctx.revert"), "ctx.revert"),
    (t!("ctx.ignore_file"), "ctx.ignore-file"),
    (t!("ctx.ignore_dir"), "ctx.ignore-dir"),
] {
    let item = gtk::Button::builder().label(&*label).build();
    item.add_css_class("flat");
    // left-align the label like a menu row (the label is the button's child)
    if let Some(child) = item.child().and_then(|w| w.downcast::<gtk::Label>().ok()) {
        child.set_xalign(0.0);
    }
    item.set_action_name(Some(action));
    let pop = popover.clone();
    item.connect_clicked(move |_b| pop.popdown());
    menu_box.append(&item);
}
```

Build `actions: gio::SimpleActionGroup` with four `gio::SimpleAction`s
(`diff`, `revert`, `ignore-file`, `ignore-dir`); keep clones of the
`SimpleAction`s so the gesture handler can `set_enabled(..)` them per row
(the buttons mirror the actions' enabled state automatically).

The box gets an extra css class `ctx-menu`; the app's existing CssProvider
(scrollbar rules) gains menu-row styling, because theme styling targets the
unexported modelbutton nodes:

```css
.ctx-menu > button.flat { padding: 6px 10px; min-height: 0; }
.ctx-menu > button.flat:active:not(:disabled) { background-color: alpha(currentColor, 0.18); }
.ctx-menu > button.flat:focus-visible { outline: 2px solid accent-color; outline-offset: -2px; }
```

GTK 4.18 does not apply `:hover` to these rows on pointer motion (menus
highlight via keyboard selection only; the PRELIGHT state flag no longer
drives `:hover` either - verified with GTK 4.18.6; EventControllerMotion
class-toggling and press-drag-release variants were tried and removed as
undesired). The final menu has NO hover highlight at all: buttons activate
via `set_action_name` + close the popover on click (`connect_clicked`),
keyboard Enter activates a focused row, and sensitivity follows the actions.
Styling keeps menu-like padding, a pressed (`:active`) highlight and a
`:focus-visible` outline.
Each activate handler borrows `ctx_row` (no-op while it is `None`) and runs:

- `diff`: `open_diff(&repo, &row.name)`
- `revert`: `gl::restore_path(&repo, &row.path)`; on `Err(e)` show
  `error_dialog(&win, &t!("error.restore_failed", error = &e))`. On success:
  `refresh_file_rows(&store, &repo); apply_sort(&store, &sort);
  sync_head_checkbox(&store, &head);` (files reverted cleanly disappear from
  the list; the existing refresh keeps checkbox states).
- `ignore-file`: pattern = `format!("/{}", row.path)` (anchored exact file;
  git paths always use `/`); `gl::append_gitignore`, then the same refresh
  sequence (untracked files vanish from the list because `git status` honours
  .gitignore; tracked files stay, matching git semantics).
- `ignore-dir`: pattern = parent directory of `row.path` with a trailing slash,
  e.g. `path/to/file.txt` -> `/path/to/`:

  ```rust
  let dir = Path::new(&row.path).parent().unwrap_or_default();
  let pattern = format!("/{}/", dir.to_string_lossy());
  ```

  same refresh sequence. (`row.path` always contains a `/` when the action is
  enabled, so the parent is never empty.)

## 5. Right-click gesture

```rust
let gesture = gtk::GestureClick::new();
gesture.set_button(gdk::BUTTON_SECONDARY);
gesture.connect_released(move |_g, _seq, x, y| {
    let Some((path, _pos)) = tree.dest_row_at_pos(x as i32, y as i32) else { return };
    let Some(iter) = store.iter(&path) else { return };
    let row = row_data(&store, &iter);
    let amend_on = amend_check.is_active();
    *ctx.borrow_mut() = Some(row.clone());
    // sensitivity, see the table at the top
    act_diff.set_enabled(true);
    act_revert.set_enabled(!amend_on && row.in_head && row.old_path.is_empty());
    act_ignore_file.set_enabled(!amend_on);
    act_ignore_dir.set_enabled(!amend_on && row.path.contains('/'));
    // centering on a (x, y-1, w, 1) rectangle puts the popover's top-left
    // corner exactly at the cursor (GTK keeps the final position on screen)
    let w = pop.measure(gtk::Orientation::Horizontal, -1).1;
    pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32 - 1, w, 1)));
    pop.popup();
});
files_tree.add_controller(gesture);   // add_controller takes the controller by value
```

(`tree` here is a clone of `files_tree` captured by the closure.)

## 6. Localization

Add these keys to **all seven** files in `locales/` (`en de fr es it nl pt`,
2-space JSON indent, `%{...}` placeholders), matching the existing key style:

- `ctx.open_diff` (en: "Open diff", de: "Diff öffnen")
- `ctx.revert` (en: "Revert changes", de: "Änderungen verwerfen")
- `ctx.ignore_file` (en: "Add file to .gitignore", de: "Datei zu .gitignore hinzufügen")
- `ctx.ignore_dir` (en: "Add directory to .gitignore", de: "Ordner zu .gitignore hinzufügen")
- `error.restore_failed` (en: "Revert failed: %{error}", cf. `error.commit_failed`)

`tests/translations.rs` style: add a lookup test for the new keys in every locale.

## 7. Tests (tests/gitcommit.rs, using tests/common helpers)

Logic-level tests (no GUI needed, mirrors existing style):

- `restore_path_discards_worktree_and_index_changes`: modify + `git add` a
  tracked file in a `fixture_repo`, `restore_path` -> worktree and index match HEAD.
- `restore_path_recreates_deleted_file`.
- `append_gitignore_creates_appends_and_dedups`: creates the file, appends after
  content without trailing newline correctly, skips a repeated pattern.
- `gitignored_untracked_file_leaves_worktree_changes`: add `p/secret.txt` as
  untracked, `append_gitignore(repo, "/p/secret.txt")`, then
  `worktree_changes(repo)` no longer lists it.
- `gitignored_directory_covers_whole_tree`: add untracked files `p/a.txt` and
  `p/sub/b.txt`, `append_gitignore(repo, "/p/")`, then neither is listed by
  `worktree_changes(repo)`; a file `q/c.txt` outside the directory is still
  listed.

GUI smoke check: run gitcommit headless on the shared Xvfb display `:42`
(matchbox WM, `import` for screenshots, per project convention) and verify the
menu visually via a temporary `glib::timeout_add_local` that seeds `ctx_row` and
pops the menu at a fixed position (xdotool right-clicks do not reliably reach
the window; remove the hack afterwards). Verify: menu at cursor position,
"Add directory" insensitive for root-level files, Revert insensitive for
untracked and rename rows.

## 8. Docs

Update the module doc at the top of `src/bin/gitcommit.rs` to mention the
right-click context menu next to the double-click sentence.

## Commit

Single commit once reviewed (message past tense, e.g. "Added right-click
context menu to the gitcommit file list"); do not commit before the user asks.

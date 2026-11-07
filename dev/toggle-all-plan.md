# Implementation plan: toggle-all checkbox in the gitcommit file list

Goal: in the `gitcommit` window (`src/bin/gitcommit.rs`), let the user check or
uncheck every file row at once.

## Verified approach (spike results)

Spikes were run against this crate's gtk4 0.11 / GTK 4.18:

1. A `gtk::CheckButton` installed into the stage column's header via
   `TreeViewColumn::set_widget(Some(&btn))` renders correctly in the header
   row.
2. However, real mouse clicks never reach that check button: walking the
   TreeView widget tree shows each column header is a `GtkButton`, and the
   header widget (the CheckButton) is nested inside it — the header button
   owns all header mouse events. Activating the header button emits the
   column's own `clicked` signal (verified headlessly via `button.emit_clicked()`
   and `stage_col.emit_by_name::<()>("clicked", &[])`).

Final design: the header CheckButton is a **display-only** "all rows checked"
indicator (`set_can_target(false)`), and the toggle-all action is bound to the
stage column's `clicked` signal, which requires `set_clickable(true)`. In
GTK 4's TreeView a header click cannot sort the list by itself (sorting needs
an explicitly attached sorter; this window has none), and `set_sortable` is not
even exposed in the gtk4 0.11 bindings. No bottom button is needed.

## Implementation

All changes are in `src/bin/gitcommit.rs`.

### 1. Header checkbox on the stage column

Where `stage_col` is built:

```rust
let head_toggle = gtk::CheckButton::new();
head_toggle.set_can_target(false); // display-only; the header button eats clicks
stage_col.set_widget(Some(&head_toggle));
stage_col.set_clickable(true); // so the header GtkButton receives clicks
files_tree.append_column(&stage_col);
```

### 2. Shared helpers (top-level functions)

The header checkbox mirrors "every row is checked" (empty list => unchecked).
The header widget has no signal handler, so no signal blocking is needed:

```rust
/// True when the list is non-empty and every row is checked.
fn rows_all_checked(store: &gtk::ListStore) -> bool {
    let Some(mut i) = store.iter_first() else { return false };
    loop {
        let checked: bool = store.get(&i, W_STAGE as i32);
        if !checked { return false; }
        // iter_next advances in place and returns false at the end
        if !store.iter_next(&mut i) { return true; }
    }
}

fn sync_head_checkbox(store: &gtk::ListStore, head: &gtk::CheckButton) {
    head.set_active(rows_all_checked(store));
}
```

### 3. Stage column click handler

Clicking the (small) stage header checks all rows, or unchecks all when every
row was already checked:

```rust
{
    let store = files_store.clone();
    let head = head_toggle.clone();
    stage_col.connect_clicked(move |_col| {
        if store.iter_first().is_none() {
            head.set_active(false);
            return;
        }
        let on = !rows_all_checked(&store);
        if let Some(mut i) = store.iter_first() {
            loop {
                store.set(&i, &[(W_STAGE, &on)]);
                if !store.iter_next(&mut i) { break; }
            }
        }
        head.set_active(on);
    });
}
```

Changing the `W_STAGE` column value never re-emits `CellRendererToggle::toggled`
(that signal is user-interaction only), so the row handler below is unaffected.

### 4. Keep the header in sync everywhere the rows change

- In the existing `toggle.connect_toggled` row handler, after flipping a single
  row, call `sync_head_checkbox(&store, &head)`.
- Call `sync_head_checkbox(...)` after each of the three `add_file_rows(...)`
  call sites (initial fill, amend-toggled on/off branch) and after
  `refresh_file_rows(...)` in the focus handler; also once right after the
  initial fill.

Amend semantics are unaffected: only the `W_STAGE` column is flipped; the
uncommit-to-HEAD^ logic lives in `gl::commit_checked` at commit time and works
from the row's `checked`/`staged`/`in_head` columns as before.

## i18n

None needed — the header checkbox has no label. (The fallback below would add
new keys to all 7 `locales/*.json` files.)

## Testing (headless)

1. `~/.cargo/bin/cargo build`
2. Create/refresh a test repo: modified + new files under `/tmp`
   (`git init`, commit, then edit + add a file).
3. Seed UI state temporarily (synthetic X clicks do not reach GTK4 windows in
   this container): in a `glib::timeout_add_local` callback, walk the TreeView
   widget tree and call `emit_clicked()` on the stage column's header
   `GtkButton` — that makes the column emit `clicked` and exercises the full
   toggle-all path. Alternatively call `stage_col.emit_by_name::<()>("clicked", &[])`.
4. `DISPLAY=:42 GITCOMMIT_SEED=1 nohup timeout 4 target/debug/gitcommit /tmp/testrepo &`;
   screenshot with `DISPLAY=:42 import -window root` before and after the seed:
   header renders, all rows + header check on first synthetic click, all
   uncheck on a second. Read the PNGs.
5. Remove the seed hack, rebuild.
6. Regression: single row toggles still work; unchecking one row unchecks the
   header indicator; checking all rows again re-checks it; amend toggle and
   window refocus refresh keep it consistent; commit behavior unchanged.
7. Manual click check (by the user in a real session): clicking the stage
   column header must check/uncheck all rows. If clicks still register
   nowhere, fall back to the bottom-left button below.

## Fallback (only if header clicks do not register)

Button in the bottom-left corner: `button_box` currently `set_halign(End)` with
Cancel/Commit. Put a wrapper `Box` (fill) around it instead and prepend a
`gtk::Button` at its start. The button toggles: if any row is unchecked it
checks all rows, else it unchecks all; label switches between
t!("select.all") / t!("select.none"), updated by the same sync logic.
Add `select.all` / `select.none` keys to all 7 locale files
(en/de/fr/es/it/nl/pt) in lockstep, 2-space indent.

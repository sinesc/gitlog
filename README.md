# gitlog

A GTK4 tool to inspect the git history of a project.

## Build

```sh
./build.sh
```

Installs the Rust toolchain if missing, then runs `cargo build --release`.
Fails with a hint if the GTK4 development libraries (`libgtk-4-dev`)
are not installed; add `--install-system-deps` to install them via apt
(needs sudo):

```sh
./build.sh --install-system-deps
```

## Install (binaries + Nemo context menu)

```sh
./install.sh
```

Builds the project (arguments are passed to `build.sh`, e.g.
`./install.sh --install-system-deps`), then installs:

- `gitlog` / `gitcommit` / `gitpush` / `gitrebase` → `~/.local/bin`
- `gitlog.nemo_action` / `gitcommit.nemo_action` /
  `gitpush.nemo_action` / `check-git-dir.sh` →
  `~/.local/share/nemo/actions`

The Nemo context menu actions (right-click a directory) run `gitlog %F`,
`gitcommit %F` and `gitpush %F`. They only show up on directories that contain a
`.git` entry, checked by `check-git-dir.sh` via the actions' `exec`
condition. Make sure `~/.local/bin` is in your `PATH` and restart Nemo
(e.g. log out/in) after installing.

## Run

```sh
target/release/gitlog [/path/to/project]      # commit history (default: current folder)
target/release/gitcommit [/path/to/project]   # commit window
target/release/gitpush [/path/to/project]     # push window
target/release/gitrebase [/path/to/project] <commit-hash>   # edit commit details
```

`gitrebase` is also opened from `gitlog` by double-clicking a commit in the
commit list.

## Features

- Three vertically stacked, resizable panes:
  - **Commit list**: abbreviated hash, first line of the message, committer,
    author and date (locale-aware, incl. week day), current branch on top,
    and a filter box for commit messages.
  - **Commit message**: full message, selectable and copyable.
  - **Changed files**: file name, lines added/deleted, file size at that
    commit, and the action (added / modified / deleted / renamed).
- Opens the window immediately with a loading indicator; history is parsed
  in a background thread from a single `git log --raw --numstat` pass.
- Press F5 to refresh the commit list (picks up new commits and branch
  movements; the old load is cancelled first).
- Closing the window while the history is still loading cancels the load
  safely.

## Implementation

- Rust, `gtk4` (4.x) + `glib` crates; plain `git` is used via CLI
  (no libgit2 dependency).
- Four binaries share a lib target (`src/lib.rs`):
  - `gitlog` (`src/main.rs`): the history viewer.
  - `gitcommit` (`src/bin/gitcommit.rs`): the commit window (see
    `dev/commit-concept.md`) with a stageable file list and an "amend last
    commit" mode; unchecking a file while amending removes it from the
    amended commit and leaves it as an unstaged file.
  - `gitpush` (`src/bin/gitpush.rs`): the push window (see
    `dev/push-concept.md`): remote dropdown (first item: all remotes),
    local/remote branch names, and force / force-with-lease / tags /
    set-upstream / all-branches options; pushes run in a background thread
    and errors are shown in a dialog with a selectable text view.
  - `gitrebase` (`src/bin/gitrebase.rs`): the rebase window (see
    `dev/rebase-concept.md`): edits author/committer (name, e-mail, date) and
    the message of one commit; HEAD commits are amended, other commits are
    rewritten via a stopped non-interactive `git rebase -i`. Opened from the
    `gitlog` commit list by double-clicking a commit.
- File sizes are fetched lazily per selected commit via
  `git ls-tree -r -l` and cached.
- Unit-tested git parsers: `cargo test`.

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

- `gitlog` / `gitcommit` → `~/.local/bin`
- `gitlog.nemo_action` / `gitcommit.nemo_action` / `check-git-dir.sh`
  → `~/.local/share/nemo/actions`

The Nemo context menu actions (right-click a directory) run `gitlog %F`
and `gitcommit %F`. They only show up on directories that contain a
`.git` entry, checked by `check-git-dir.sh` via the actions' `exec`
condition. Make sure `~/.local/bin` is in your `PATH` and restart Nemo
(e.g. log out/in) after installing.

## Run

```sh
target/release/gitlog [/path/to/project]      # commit history (default: current folder)
target/release/gitcommit [/path/to/project]   # commit window
```

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
- Closing the window while the history is still loading cancels the load
  safely.

## Implementation

- Rust, `gtk4` (4.x) + `glib` crates; plain `git` is used via CLI
  (no libgit2 dependency).
- Two binaries share a lib target (`src/lib.rs`):
  - `gitlog` (`src/main.rs`): the history viewer.
  - `gitcommit` (`src/bin/gitcommit.rs`): the commit window (see
    `dev/commit-concept.md`) with a stageable file list and an "amend last
    commit" mode; unchecking a file while amending removes it from the
    amended commit and leaves it as an unstaged file.
- File sizes are fetched lazily per selected commit via
  `git ls-tree -r -l` and cached.
- Unit-tested git parsers: `cargo test`.

# gitlog

A GTK4 tool to inspect the git history of a project.

## Build

```sh
./build.sh
```

Installs `libgtk-4-dev` and the Rust toolchain if missing (needs sudo),
then runs `cargo build --release`.

## Run

```sh
target/release/gitlog /path/to/project
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
- File sizes are fetched lazily per selected commit via
  `git ls-tree -r -l` and cached.
- Unit-tested parser: `cargo test`.

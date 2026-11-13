# gitlog

A set of Linux UI tools with file manager integration to work with git,
inspired by TortoiseGit. Currently included tools:

- `gitlog`: Searchable git history, shows details for selected commit.
- `gitcommit`: Commit window, supports amending.
- `gitpush`: Push to remotes, supports including tags, force [with lease], ...
- `gitrebase`: Currently only used for editing commit details (message/author/...)

Currently supported file managers:
- `nemo`

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
`.git` entry. Make sure `~/.local/bin` is in your `PATH` and restart Nemo
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

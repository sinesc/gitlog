#!/usr/bin/env bash
# Nemo action condition (see res/gitlog.nemo_action / res/gitcommit.nemo_action).
#
# Exit 0 if the selected directory is a git repository, i.e. it contains a
# .git entry (a folder for normal repos, a file for worktrees/submodules),
# exit 1 otherwise. Nemo runs this via the "Conditions=exec" mechanism and
# passes the selected path(s) as arguments (from the %F token).
#
# The arguments are joined back together so paths with spaces keep working.
path="$*"

if [ -d "$path/.git" ] || [ -f "$path/.git" ]; then
    exit 0
fi
exit 1

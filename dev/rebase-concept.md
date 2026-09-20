# rebase window

- implemented as separate binary `gitrebase` (no nemo integration) 
- title: `git rebase - <abbreviated commit hash>`, e.g. `git rebase - 5f5d34f`
- inputs for 
    - author, author-email, author-date (in one row, date uses non-localized date with 24h time, `YYYY-MM-DD HH:MM:SS`, e.g. '2026-09-20 17:36:12' interpreted as local time)
    - committer, committer-email, committer-date (as above)
    - commit message (text area)
- note: we're not implementing file changes/commit reording etc for now, just commit detail editing
- if the commit branch has already been pushed: brief warning message that editing already pushed commits may cause issues for other contributors (if hard to identify defer it for now)
- 'Cancel' and 'Apply' buttons at the bottom
- add context menu entry to the `gitlog` binary's commit list to open `gitrebase` on the selected commit (label: Edit commit details)

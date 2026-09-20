# Commit window

- implemented as separate binary `gitcommit`
- title: `git commit - <branch name>`, e.g. `git commit - main`
- two vertically stacked panes and 'Cancel' and 'Commit' buttons at the bottom
    - top pane: text area for commit message with a checkbox 'Amend last commit' below
    - bottom pane: file list with checkboxes (stage file) and columns: file name, file status (e.g. modified, new), lines added, lines removed
- when amending a commit the file list should show files from the previous commit and the current working copy changes, unchecking a file before committing should remove the file from the amended commit and leave it as unstaged file (e.g. for a later commit)

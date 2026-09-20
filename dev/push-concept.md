# Push window

- implemented as separate binary `gitpush` with nemo integration like the the existing binaries 
- title: `git push - <branch name>`, e.g. `git push - main`
- checkbox to push all branches
- inputs for local/remote branch name
- dropdown for git remote, first item is all remotes followed by the configured git remotes
- checkboxes for force and force-with-lease (only one can be active at a time)
- checkbox to include tags
- checkbox to set upstream/remote branch
- 'Cancel' and 'Commit' buttons at the bottom
- on error: shows a dialog containing the error message and a close button, user should be able to select/copy this message (e.g. to google it) 
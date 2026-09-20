# gitlog

A tool to inspect the git history of a project.

## Requirements

- graphical user interface that visually integrates nicely with Linux Mint's default desktop environment
- accept project folder name from command line
- ui vertically split (adjustable splitter)
    - top pane shows git log as a list with columns for abbreviated hash, first line of commit message, committer, author, date (using system locale language, include name of the week day), also shows current branch above the list and an input box to filter by commit message
    - middle pane: full commit message, text area that supports selecting/copying the message into the clipboard
    - bottom pane: list of files in the commit with columns for file name, added/changed/deleted lines, file size, action (added, modified, deleted, renamed)

## Implementation

- compiled language with built script that runs in this container (debian_version=13.6) as well as the host (debian_version=trixie/sid)
- fast startup, open window and show loading message in text area first, process log in background thread
- user should be able to close window while log is still loading (e.g. if they accidentially opened the wrong git repository with a potentially large history)
- language/toolkits etc TBD
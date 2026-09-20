# Nemo context menu support and installer

- user should be able to use `gitlog` and `gitcommit` via nemo context menu action on directories
- example/base nemo action provided in `res/gitlog.nemo_action` (doesn't check if directory contains a repository yet)
- actions should only show for git directories e.g. by implementing a check for a contained `.git` folder and specifying it as action condition
- installer script should install project binaries to ~/.local/bin, nemo actions to ~/.local/share/nemo/actions
- user will test nemo integration, don't install nemo in this environment
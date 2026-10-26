//! Git data access: parses `git log` output, queries file sizes and runs the
//! git commands behind the commit, push and rebase windows. Tested by the
//! integration tests in `tests/` (no tests inside this file).

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

const SEP: char = '\x1f'; // unit separator inside one record
const MARK: char = '\x01'; // start-of-record marker

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Clone)]
pub struct FileStat {
    pub name: String,
    /// None for binary files
    pub added: Option<i64>,
    /// None for binary files
    pub deleted: Option<i64>,
    pub action: Action,
}

#[derive(Clone)]
pub struct Commit {
    /// Abbreviated hash, as shown in the commit list
    pub hash: String,
    /// Full 40-char hash
    pub full_hash: String,
    /// Names of local/remote branches whose tip is this commit (empty for
    /// commits that are not at the tip of any branch)
    pub branches: Vec<String>,
    pub subject: String,
    pub message: String,
    pub author: String,
    pub committer: String,
    /// Author unix timestamp; localized in the UI (see `i18n::format_timestamp`).
    pub timestamp: i64,
    /// Committer unix timestamp; differs from `timestamp` when the commit was
    /// amended, rebased or produced by a bot (marked with `*` in the list).
    pub committer_timestamp: i64,
    pub files: Vec<FileStat>,
}

fn git(repo: &Path, args: &[&str]) -> std::io::Result<std::process::Child> {
    let mut cmd = Command::new("git");
    cmd.args(["-C", &repo.to_string_lossy()]);
    cmd.args(args);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::null());
    cmd.env("LC_ALL", "C");
    cmd.spawn()
}

pub fn is_repo(repo: &Path) -> bool {
    Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// "branch name" or "(detached @ short-hash)"
/// Name of the checked-out branch ("" in detached HEAD).
pub fn current_branch(repo: &Path) -> String {
    let output = Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "branch", "--show-current"])
        .output()
        .ok()
        .map(|o| o.stdout)
        .unwrap_or_default();
    String::from_utf8(output).unwrap_or_default().trim().to_string()
}

/// "branch name" or "(detached @ short-hash)"
pub fn head_info(repo: &Path) -> String {
    let branch = current_branch(repo);
    if branch.is_empty() {
        let sha = std::str::from_utf8(
            &Command::new("git")
                .args(["-C", &repo.to_string_lossy(), "rev-parse", "--short", "HEAD"])
                .output()
                .ok()
                .map(|o| o.stdout)
                .unwrap_or_default(),
        )
        .unwrap_or("?")
        .trim()
        .to_string();
        format!("(detached @ {sha})")
    } else {
        branch
    }
}

/// Names of the configured remotes, in `git remote` order.
pub fn remotes(repo: &Path) -> Vec<String> {
    let Ok(output) = Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "remote"])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Map of full commit hash -> branch names (local and remote) whose tip
/// is that commit, in refname order.
pub fn branch_tips(repo: &Path) -> HashMap<String, Vec<String>> {
    let Ok(output) = Command::new("git")
        .args([
            "-C",
            &repo.to_string_lossy(),
            "for-each-ref",
            "refs/heads",
            "refs/remotes",
            "--format=%(refname)\x01%(objectname)\x01%(refname:short)",
        ])
        .output()
    else {
        return HashMap::new();
    };
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some((refname, rest)) = line.split_once('\x01') else { continue };
        let Some((hash, name)) = rest.split_once('\x01') else { continue };
        // Skip symbolic HEAD refs (e.g. refs/remotes/origin/HEAD); their
        // refname:short collapses to just the remote name ("origin"), which
        // would show up as a phantom branch tip. Their target branch is
        // listed on its own.
        if refname.trim().ends_with("/HEAD") {
            continue;
        }
        map.entry(hash.trim().to_string())
            .or_default()
            .push(name.to_string());
    }
    map
}

/// File sizes (bytes) of all files at the given commit, keyed by path.
pub fn file_sizes(repo: &Path, hash: &str) -> HashMap<String, u64> {
    let Ok(output) = Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "ls-tree", "-r", "-l", hash])
        .output()
    else {
        return HashMap::new();
    };
    let mut map = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        // "<mode> <type> <size>\t<path>"
        let (meta, path) = match line.split_once('\t') {
            Some(p) => p,
            None => continue,
        };
        if let Some(size) = meta.split_whitespace().last() {
            if let Ok(size) = size.parse() {
                map.insert(path.to_string(), size);
            }
        }
    }
    map
}

/// A file entry for the commit UI: a working-tree change against `HEAD`
/// (or, in amend mode, a file of the previous commit).
#[derive(Clone, Debug)]
pub struct WorkFile {
    /// Path used for git operations (the new path for renames)
    pub path: String,
    /// Display name; renames are shown as "old => new"
    pub display_name: String,
    pub action: Action,
    /// None for binary files or files with no line-count diff (untracked)
    pub added: Option<i64>,
    pub deleted: Option<i64>,
    /// Already in the index (used to pre-check the stage checkbox)
    pub staged: bool,
    /// File exists at `HEAD` (false for untracked files)
    pub in_head: bool,
}

/// numstat shows renames as "old => new"; the index/tree only know the new path.
fn numstat_path(p: &str) -> &str {
    p.rsplit("=>").next().unwrap_or(p).trim()
}

fn is_count(t: &str) -> bool {
    !t.is_empty() && t.chars().all(|c| c.is_ascii_digit() || c == '-')
}

/// `git diff --name-status [args]`; returns (path, action, display name).
fn name_status(repo: &Path, args: &[&str]) -> Vec<(String, Action, String)> {
    let Ok(output) = Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "-c", "core.quotepath=false", "diff", "--name-status"])
        .args(args)
        .output()
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        // "<X>[<score>]\t<file>" or "<R/C><score>\t<old>\t<new>"
        let mut f = line.splitn(3, '\t');
        let (Some(code), Some(first), new) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let letter = code.chars().next().unwrap();
        let action = match letter {
            'A' | 'C' => Action::Added,
            'D' => Action::Deleted,
            'R' => Action::Renamed,
            _ => Action::Modified,
        };
        let (path, display) = match new {
            Some(new) => (new.to_string(), format!("{first} => {new}")),
            None => (first.to_string(), first.to_string()),
        };
        out.push((path, action, display));
    }
    out
}

/// `git diff --numstat [args]`; returns path -> (added, deleted).
fn numstat_map(repo: &Path, args: &[&str]) -> HashMap<String, (Option<i64>, Option<i64>)> {
    let Ok(output) = Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "-c", "core.quotepath=false", "diff", "--numstat"])
        .args(args)
        .output()
    else {
        return HashMap::new();
    };
    let count = |s: &str| if s == "-" { None } else { s.parse().ok() };
    let mut map = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut f = line.splitn(3, '\t');
        let (Some(a), Some(d), Some(p)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        if is_count(a) && is_count(d) {
            map.insert(numstat_path(p).to_string(), (count(a), count(d)));
        }
    }
    map
}

/// Untracked files (from `git status --porcelain=v1 -uall`).
/// `-uall` matters: with the default `-unormal` git collapses a directory
/// whose content is entirely untracked into a single `dir/` entry, so the
/// commit window would show the folder instead of the files inside it.
pub fn untracked_files(repo: &Path) -> Vec<String> {
    let Ok(output) = Command::new("git")
        .args([
            "-C",
            &repo.to_string_lossy(),
            "-c",
            "core.quotepath=false",
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
        ])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix("?? ").map(|p| p.to_string()))
        .collect()
}

/// Files that differ between the working tree and `HEAD`, plus untracked
/// files. Renames are shown as "old => new" and carry the new path.
pub fn worktree_changes(repo: &Path) -> Vec<WorkFile> {
    let counts = numstat_map(repo, &["-M", "HEAD"]);
    let staged: HashSet<String> = name_status(repo, &["-M", "--cached", "HEAD"])
        .into_iter()
        .map(|(p, _, _)| p)
        .collect();
    let mut files: Vec<WorkFile> = name_status(repo, &["-M", "HEAD"])
        .into_iter()
        .map(|(path, action, display_name)| {
            let (added, deleted) = counts.get(&path).copied().unwrap_or((None, None));
            WorkFile {
                path: path.clone(),
                display_name,
                action,
                added,
                deleted,
                staged: staged.contains(&path),
                in_head: true,
            }
        })
        .collect();
    for path in untracked_files(repo) {
        files.push(WorkFile {
            display_name: path.clone(),
            path,
            action: Action::Added,
            added: None,
            deleted: None,
            staged: false,
            in_head: false,
        });
    }
    files
}

/// True if the repository has at least one commit.
pub fn has_head(repo: &Path) -> bool {
    Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "rev-parse", "--verify", "-q", "HEAD"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Files of the previous commit (`None` if there is no commit yet). All are
/// in the index, so `staged` and `in_head` are set.
pub fn head_files(repo: &Path) -> Option<Vec<WorkFile>> {
    if !has_head(repo) {
        return None;
    }
    // git shows only one diff format at a time, and `show` (instead of
    // `diff HEAD^ HEAD`) also works for the root commit
    let mut output = String::new();
    for format in ["--name-status", "--numstat"] {
        if let Ok(out) = Command::new("git")
            .args([
                "-C", &repo.to_string_lossy(), "-c", "core.quotepath=false",
                "show", "-M", format, "--format=", "HEAD",
            ])
            .output()
        {
            output.push_str(&String::from_utf8_lossy(&out.stdout));
        }
    }
    let mut files: Vec<WorkFile> = Vec::new();
    let mut counts: HashMap<String, (Option<i64>, Option<i64>)> = HashMap::new();
    for line in output.lines() {
        let mut f = line.splitn(3, '\t');
        let (Some(a), Some(b), rest) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        if is_count(a) && is_count(b) {
            // numstat: "<added>\t<deleted>\t<path>"
            if let Some(path) = rest {
                counts.insert(
                    numstat_path(path).to_string(),
                    (a.parse().ok(), b.parse().ok()),
                );
            }
        } else if matches!(a.chars().next(), Some('A' | 'M' | 'D' | 'R' | 'C' | 'T')) {
            // name-status: "<X>[<score>]\t<file>" or "<R/C><score>\t<old>\t<new>"
            let action = match a.chars().next() {
                Some('A') | Some('C') => Action::Added,
                Some('D') => Action::Deleted,
                Some('R') => Action::Renamed,
                _ => Action::Modified,
            };
            let (path, display_name) = match rest {
                Some(new) => (new.to_string(), format!("{b} => {new}")),
                None => (b.to_string(), b.to_string()),
            };
            files.push(WorkFile {
                path: path.clone(),
                display_name,
                action,
                added: None,
                deleted: None,
                staged: true,
                in_head: true,
            });
        }
    }
    for f in &mut files {
        if let Some((added, deleted)) = counts.get(&f.path) {
            f.added = *added;
            f.deleted = *deleted;
        }
    }
    Some(files)
}

/// Full message of the previous commit (trailing newlines trimmed).
pub fn head_message(repo: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "log", "-1", "--format=%B", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let message = String::from_utf8_lossy(&output.stdout);
    Some(message.trim_end_matches('\n').to_string())
}

/// Runs a git command; returns the trimmed stderr on failure.
pub fn run_git(repo: &Path, args: &[&str]) -> Result<(), String> {
    let output = Command::new("git")
        .args(["-C", &repo.to_string_lossy()])
        .args(args)
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        let msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if msg.is_empty() { "git command failed".into() } else { msg })
    }
}

/// Stage the working-tree state of one path.
pub fn stage_path(repo: &Path, path: &str) -> Result<(), String> {
    run_git(repo, &["add", "-A", "--", path])
}

/// Remove one path from the index, keeping the working-tree file.
pub fn unstage_path(repo: &Path, path: &str) -> Result<(), String> {
    run_git(repo, &["restore", "--staged", "--", path])
}

/// Remove one path from the index only (the working-tree file stays) -
/// used to drop a file from an amended commit.
pub fn rm_cached_path(repo: &Path, path: &str) -> Result<(), String> {
    run_git(repo, &["rm", "-q", "-r", "--cached", "--", path])
}

/// True when the index matches `HEAD`, i.e. nothing is staged to commit.
pub fn index_matches_head(repo: &Path) -> bool {
    Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "diff", "--cached", "--quiet", "HEAD"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// `git commit [-m message] [--amend]`; returns the trimmed stderr on failure.
pub fn do_commit(repo: &Path, message: &str, amend: bool) -> Result<(), String> {
    let mut args: Vec<String> = vec!["commit".into()];
    if amend {
        args.push("--amend".into());
    }
    args.push("-m".into());
    args.push(message.to_string());
    let output = Command::new("git")
        .args(["-C", &repo.to_string_lossy()])
        .args(&args)
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        let msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if msg.is_empty() { "git command failed".into() } else { msg })
    }
}

/// File rows for amend mode: the files of the previous commit merged with
/// the working-copy changes; everything is pre-checked (a file the user
/// unchecks leaves the amended commit).
pub fn amend_files(repo: &Path) -> Vec<WorkFile> {
    let mut files = head_files(repo).unwrap_or_default();
    for w in worktree_changes(repo) {
        // a rename row carries the *new* path: also match the head row of
        // the old path, otherwise the file appears twice (once from the
        // previous commit, once as "old => new") and committing fails
        // because the old path no longer exists
        let old_path = w.display_name.split("=>").next().unwrap_or("").trim();
        let is_rename = w.action == Action::Renamed;
        match files
            .iter_mut()
            .find(|f| f.path == w.path || (is_rename && f.path == old_path))
        {
            // changed in the work tree: keep the "in head" flag, show the
            // current diff against HEAD
            Some(h) => {
                h.path = w.path.clone();
                h.display_name = w.display_name;
                h.action = w.action;
                h.added = w.added;
                h.deleted = w.deleted;
            }
            None => files.push(w),
        }
    }
    files
}

/// One row of the commit window's file list: the file, whether its checkbox
/// is set, whether it is part of the previous commit and whether it is
/// already staged.
pub struct StageRow {
    pub path: String,
    pub checked: bool,
    pub in_head: bool,
    pub staged: bool,
}

/// Applies the checkbox selection to the index and creates the commit.
/// On error the index may be partially updated, which the user can inspect
/// and repair; the window stays open.
pub fn commit_checked(repo: &Path, rows: &[StageRow], amend: bool, message: &str) -> Result<(), String> {
    for row in rows {
        if amend {
            if row.in_head && !row.checked {
                // remove from the amended commit, keep the work tree file
                rm_cached_path(repo, &row.path)?;
            } else if row.checked {
                stage_path(repo, &row.path)?;
            }
        } else if row.checked && !row.staged {
            stage_path(repo, &row.path)?;
        } else if !row.checked && row.staged {
            unstage_path(repo, &row.path)?;
        }
    }
    if !amend && index_matches_head(repo) {
        return Err(rust_i18n::t!("error.nothing_staged").into_owned());
    }
    do_commit(repo, message, amend)
}

/// Structural check of "YYYY-MM-DD HH:MM:SS" (the values themselves are not
/// checked - git rejects dates that do not exist).
pub fn is_date_string(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 19 {
        return false;
    }
    (0..4).all(|i| b[i].is_ascii_digit())
        && b[4] == b'-'
        && (5..7).all(|i| b[i].is_ascii_digit())
        && b[7] == b'-'
        && (8..10).all(|i| b[i].is_ascii_digit())
        && b[10] == b' '
        && (11..13).all(|i| b[i].is_ascii_digit())
        && b[13] == b':'
        && (14..16).all(|i| b[i].is_ascii_digit())
        && b[16] == b':'
        && (17..19).all(|i| b[i].is_ascii_digit())
}

/// The editable fields of one existing commit (the rebase window).
pub struct CommitDetails {
    pub author: String,
    pub author_email: String,
    /// "YYYY-MM-DD HH:MM:SS" in local time
    pub author_date: String,
    pub committer: String,
    pub committer_email: String,
    /// "YYYY-MM-DD HH:MM:SS" in local time
    pub committer_date: String,
    pub message: String,
}

/// Reads the author/committer fields and the message of one commit.
/// Dates come back in local time; the NUL-separated format keeps names
/// and emails with spaces or `<>` intact.
pub fn commit_details(repo: &Path, hash: &str) -> Result<CommitDetails, String> {
    let output = Command::new("git")
        .args(["-C", &repo.to_string_lossy()])
        .args([
            "log",
            "-1",
            "--date=format-local:%Y-%m-%d %H:%M:%S",
            "--format=%an%x00%ae%x00%ad%x00%cn%x00%ce%x00%cd%x00%B",
            hash,
        ])
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        let msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if msg.is_empty() { "git command failed".into() } else { msg });
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let fields: Vec<&str> = text.split('\0').collect();
    if fields.len() < 7 {
        return Err("unexpected git output".into());
    }
    Ok(CommitDetails {
        author: fields[0].to_string(),
        author_email: fields[1].to_string(),
        author_date: fields[2].to_string(),
        committer: fields[3].to_string(),
        committer_email: fields[4].to_string(),
        committer_date: fields[5].to_string(),
        // %B carries trailing newlines; trim them
        message: fields[6].trim_end_matches('\n').to_string(),
    })
}

/// True if the commit is reachable from any remote-tracking branch.
pub fn is_pushed(repo: &Path, hash: &str) -> bool {
    let output = Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "branch", "-r", "--contains", hash])
        .output()
        .ok();
    output
        .filter(|o| o.status.success())
        .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
        .unwrap_or(false)
    }

/// Rewrites the details of one commit (the tree is not changed).
/// The HEAD commit is amended in place; other ancestors of the current
/// HEAD are re-created with `git commit-tree` together with every
/// descendant (each descendant keeps its tree, message, author and
/// original committer — only the hashes change; a `git rebase` replay
/// would replace the committer of the descendants with the local
/// identity). Local branches whose tip lies inside the rewritten history
/// move with it (tags are left alone). Requires a clean work tree.
pub fn edit_commit_details(repo: &Path, hash: &str, d: &CommitDetails) -> Result<(), String> {
    // the rebase requires a clean work tree; keeping it uniform for the
    // plain-amend path too
    let status = Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "status", "--porcelain"])
        .output()
        .map_err(|e| e.to_string())?;
    if !status
        .status
        .success()
        || !String::from_utf8_lossy(&status.stdout).trim().is_empty()
    {
        return Err("uncommitted changes in the work tree".into());
    }

    // full hash (also verifies the argument)
    let full = Command::new("git")
        .args([
            "-C",
            &repo.to_string_lossy(),
            "rev-parse",
            "-q",
            "--verify",
            &format!("{hash}^{{commit}}"),
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !full.status.success() {
        return Err(format!("commit {hash} not found"));
    }
    let full = String::from_utf8_lossy(&full.stdout).trim().to_string();

    // `git commit --amend` with the new identity: `--author` and `--date`
    // set the author fields, the GIT_COMMITTER_* environment sets the
    // committer fields (dates in local time, as git reads them)
    let amend = || {
        let author = format!("{} <{}>", d.author, d.author_email);
        let mut cmd = Command::new("git");
        cmd.args(["-C", &repo.to_string_lossy()]);
        cmd.args([
            "commit",
            "--amend",
            "-m",
            &d.message,
            "--author",
            &author,
            "--date",
            &d.author_date,
        ]);
        cmd.env("GIT_COMMITTER_NAME", &d.committer);
        cmd.env("GIT_COMMITTER_EMAIL", &d.committer_email);
        cmd.env("GIT_COMMITTER_DATE", &d.committer_date);
        cmd.stderr(Stdio::piped());
        let output = cmd.output().map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            let msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if msg.is_empty() { "git commit --amend failed".into() } else { msg })
        }
    };

    let head = Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "rev-parse", "-q", "HEAD"])
        .output()
        .map_err(|e| e.to_string())?;
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
    if full == head {
        // checked-out tip: a plain amend is enough
        return amend();
    }

    // git with captured stdout, like `run_git` but returning the output
    let git_out = |args: &[&str]| -> Result<String, String> {
        let output = Command::new("git")
            .args(["-C", &repo.to_string_lossy()])
            .args(args)
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            let msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if msg.is_empty() { "git command failed".into() } else { msg })
        }
    };

    // 1. the edited commit: same tree and parents, new details
    let tree = git_out(&["rev-parse", "-q", "--verify", &format!("{full}^{{tree}}")])?;
    let parents = git_out(&["log", "-1", "--format=%P", &full])?;
    let mut cmd = Command::new("git");
    cmd.args(["-C", &repo.to_string_lossy(), "commit-tree", &tree]);
    for p in parents.split_whitespace() {
        cmd.arg("-p").arg(p);
    }
    cmd.arg("-m").arg(&d.message);
    cmd.env("GIT_AUTHOR_NAME", &d.author);
    cmd.env("GIT_AUTHOR_EMAIL", &d.author_email);
    cmd.env("GIT_AUTHOR_DATE", &d.author_date);
    cmd.env("GIT_COMMITTER_NAME", &d.committer);
    cmd.env("GIT_COMMITTER_EMAIL", &d.committer_email);
    cmd.env("GIT_COMMITTER_DATE", &d.committer_date);
    cmd.stderr(Stdio::piped());
    let output = cmd.output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        let msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if msg.is_empty() { "git commit-tree failed".into() } else { msg });
    }
    let edited = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // git with captured, untrimmed stdout (for the commit messages, whose
    // trailing newline is part of the data)
    let git_out_raw = |args: &[&str]| -> Result<String, String> {
        let output = Command::new("git")
            .args(["-C", &repo.to_string_lossy()])
            .args(args)
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if msg.is_empty() { "git command failed".into() } else { msg })
        }
    };

    // 2. the descendant commits of the current tip, oldest first (topo
    // order guarantees parents come before children)
    let list = git_out(&["rev-list", "--topo-order", "--reverse", &format!("{full}..{head}")])?;
    let mut rewritten: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    rewritten.insert(full.clone(), edited);
    for h in list.lines() {
        let fields = git_out(&[
            "log",
            "-1",
            "--format=%T%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct",
            h,
        ])?;
        let f: Vec<&str> = fields.split('\0').collect();
        if f.len() != 8 {
            return Err("unexpected git output".into());
        }
        let (tree, ps, an, ae, at, cn, ce, ct) = (f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7]);
        let message = git_out_raw(&["log", "-1", "--format=%B", h])?;
        let mut cmd = Command::new("git");
        cmd.args(["-C", &repo.to_string_lossy(), "commit-tree", tree]);
        for p in ps.split_whitespace() {
            // parents that were re-created point at their new hashes
            let np = rewritten.get(p).cloned().unwrap_or_else(|| p.to_string());
            cmd.arg("-p").arg(&np);
        }
        // author and committer come from the original commit (raw
        // timestamps); `git commit-tree` would otherwise take both from
        // the local identity; the message is piped in verbatim
        cmd.env("GIT_AUTHOR_NAME", an);
        cmd.env("GIT_AUTHOR_EMAIL", ae);
        cmd.env("GIT_AUTHOR_DATE", at);
        cmd.env("GIT_COMMITTER_NAME", cn);
        cmd.env("GIT_COMMITTER_EMAIL", ce);
        cmd.env("GIT_COMMITTER_DATE", ct);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        std::io::Write::write_all(child.stdin.as_mut().unwrap(), message.as_bytes())
            .map_err(|e| e.to_string())?;
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        if !output.status.success() {
            let msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if msg.is_empty() { "git commit-tree failed".into() } else { msg });
        }
        let new = String::from_utf8_lossy(&output.stdout).trim().to_string();
        rewritten.insert(h.to_string(), new);
    }
    let Some(new_tip) = rewritten.get(&head) else {
        // empty range: the commit is not an ancestor of the current tip
        return Err("commit is not on the current branch".into());
    };

    // 3. point the local branches at their new tips. Branches whose tip
    // is inside the rewritten history (e.g. a merged side branch) move
    // too; tags are left alone. The current branch is last, guarded by
    // the old tip so a concurrent update is not clobbered.
    let symref = Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "symbolic-ref", "-q", "HEAD"])
        .output()
        .map_err(|e| e.to_string())?;
    let refname = if symref.status.success() {
        String::from_utf8_lossy(&symref.stdout).trim().to_string()
    } else {
        "HEAD".into()
    };
    let heads = git_out(&["for-each-ref", "--format=%(refname)%00%(objectname)", "refs/heads"])?;
    for line in heads.lines() {
        let mut it = line.split('\0');
        let Some((name, obj)) = it.next().zip(it.next()) else {
            continue;
        };
        if name == refname {
            continue;
        }
        if let Some(new) = rewritten.get(obj) {
            run_git(
                repo,
                &["update-ref", "-m", "gitrebase: edit commit details", name, new],
            )?;
        }
    }
    if refname == "HEAD" {
        run_git(repo, &["update-ref", "-m", "gitrebase: edit commit details", "HEAD", new_tip, &head])
    } else {
        run_git(
            repo,
            &["update-ref", "-m", "gitrebase: edit commit details", &refname, new_tip, &head],
        )
    }
}

/// Streams commits (newest first) into `on_commit`.
/// `on_commit` returning `false` stops the load.
pub fn load_commits<F: FnMut(Commit) -> bool>(
    repo: &Path,
    cancel: &AtomicBool,
    mut on_commit: F,
) {
    // committer/author as "name <email>" so a rename or amending bot can be
    // spotted at a glance
    let tips = branch_tips(repo);
    let pretty =
        format!("{MARK}%h{SEP}%H{SEP}%an <%ae>{SEP}%cn <%ce>{SEP}%at{SEP}%ct{SEP}%s{SEP}%B");
    let mut child = match git(
        repo,
        &[
            "-c",
            "core.quotepath=false",
            "log",
            "-M",
            "--raw",
            "--numstat",
            &format!("--pretty=format:{pretty}"),
        ],
    ) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("gitlog: failed to run git log: {e}");
            return;
        }
    };
    let stdout = child.stdout.take().expect("stdout piped");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();

    // record in flight
    let mut pending: Option<(String, String, String, String, i64, i64)> =
        None; // short-hash, full-hash, author, committer, author-ts, committer-ts
    let mut subject = String::new();
    let mut message: Vec<String> = Vec::new();
    let mut files: Vec<FileStat> = Vec::new();
    // status letters from --raw lines, consumed in order by --numstat lines
    let mut actions: VecDeque<Action> = VecDeque::new();

    let mut send = |c: Commit| on_commit(c);

    let mut flush = |pending: &mut Option<(String, String, String, String, i64, i64)>,
                    subject: &str,
                    message: &mut Vec<String>,
                    files: &mut Vec<FileStat>,
                    actions: &mut VecDeque<Action>|
     -> bool {
        if let Some((hash, full, author, committer, ts, cts)) = pending.take() {
            let branches = tips.get(&full).cloned().unwrap_or_default();
            let commit = Commit {
                hash,
                full_hash: full,
                branches,
                subject: subject.to_string(),
                // %B carries trailing newlines; trim them
                message: message.join("\n").trim_end_matches('\n').to_string(),
                author,
                committer,
                timestamp: ts,
                committer_timestamp: cts,
                files: std::mem::take(files),
            };
            actions.clear();
            message.clear();
            send(commit)
        } else {
            true
        }
    };

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                eprintln!("gitlog: error reading git output: {e}");
                break;
            }
        }
        let line = line.trim_end_matches(['\n', '\r']);

        if let Some(rest) = line.strip_prefix(MARK) {
            if !flush(
                &mut pending,
                &subject,
                &mut message,
                &mut files,
                &mut actions,
            )
                || cancel.load(Ordering::Relaxed)
            {
                break;
            }
            let fields: Vec<&str> = rest.split(SEP).collect();
            // fields: 0=short-hash 1=full-hash 2=author 3=committer 4=author-ts
            //         5=committer-ts 6=subject 7=%B body
            if fields.len() < 8 {
                continue;
            }
            pending = Some((
                fields[0].to_string(),
                fields[1].to_string(),
                fields[2].to_string(),
                fields[3].to_string(),
                fields[4].parse().unwrap_or(0),
                fields[5].parse().unwrap_or(0),
            ));
            subject = fields[6].to_string();
            message = vec![fields[7].to_string()];
            continue;
        }

        if pending.is_none() {
            continue;
        }

        // raw line: ":<mode> <mode> <oldsha> <newsha>\t<STATUS>\t<path>"
        if line.starts_with(':') {
            if let Some((head, _rest)) = line[1..].split_once('\t') {
                // STATUS (A/M/D/R100/C/T) is the last space-separated token
                if let Some(status) = head.split_whitespace().last() {
                    let letter = status.chars().next().unwrap();
                    let action = match letter {
                        'A' | 'C' => Action::Added,
                        'D' => Action::Deleted,
                        'R' => Action::Renamed,
                        _ => Action::Modified,
                    };
                    actions.push_back(action);
                }
            }
            continue;
        }

        // numstat line: "<added>\t<deleted>\t<path>"  (dashes for binary)
        let mut parts = line.splitn(3, '\t');
        if let (Some(added_s), Some(deleted_s), Some(path)) =
            (parts.next(), parts.next(), parts.next())
        {
            // accept if both parse as numbers, or both are dashes (binary)
            let is_count = |s: &str| s.is_empty() || s.chars().all(|c| c.is_ascii_digit() || c == '-');
            if is_count(added_s) && is_count(deleted_s) {
                let action = actions.pop_front().unwrap_or(Action::Modified);
                files.push(FileStat {
                    name: path.to_string(),
                    added: added_s.parse().ok(),
                    deleted: deleted_s.parse().ok(),
                    action,
                });
                continue;
            }
        }

        // otherwise: continuation of the commit message
        message.push(line.to_string());
    }

    if !flush(
        &mut pending,
        &subject,
        &mut message,
        &mut files,
        &mut actions,
    ) {
        // receiver gone, fine
    }
    let _ = child.wait();
}

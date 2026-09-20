//! Git data access: parses `git log` output and queries file sizes.

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
    /// Committer unix timestamp; localized in the UI (see `i18n::format_timestamp`).
    pub timestamp: i64,
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

/// Untracked files (from `git status --porcelain=v1`).
fn untracked_files(repo: &Path) -> Vec<String> {
    let Ok(output) = Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "-c", "core.quotepath=false", "status", "--porcelain=v1"])
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
        format!("{MARK}%h{SEP}%H{SEP}%an <%ae>{SEP}%cn <%ce>{SEP}%at{SEP}%s{SEP}%B");
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
    let mut pending: Option<(String, String, String, String, i64)> = None; // short-hash, full-hash, author, committer, ts
    let mut subject = String::new();
    let mut message: Vec<String> = Vec::new();
    let mut files: Vec<FileStat> = Vec::new();
    // status letters from --raw lines, consumed in order by --numstat lines
    let mut actions: VecDeque<Action> = VecDeque::new();

    let mut send = |c: Commit| on_commit(c);

    let mut flush = |pending: &mut Option<(String, String, String, String, i64)>,
                    subject: &str,
                    message: &mut Vec<String>,
                    files: &mut Vec<FileStat>,
                    actions: &mut VecDeque<Action>|
     -> bool {
        if let Some((hash, full, author, committer, ts)) = pending.take() {
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
            // fields: 0=short-hash 1=full-hash 2=author 3=committer 4=timestamp 5=subject 6=%B body
            if fields.len() < 7 {
                continue;
            }
            pending = Some((
                fields[0].to_string(),
                fields[1].to_string(),
                fields[2].to_string(),
                fields[3].to_string(),
                fields[4].parse().unwrap_or(0),
            ));
            subject = fields[5].to_string();
            message = vec![fields[6].to_string()];
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn git(repo: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-C", &repo.to_string_lossy()])
            .args(args)
            .output()
            .expect("failed to run git");
        assert!(
            out.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn write(repo: &Path, name: &str, content: &str) {
        let path = repo.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    /// Builds a repo with add / modify / delete / rename commits.
    /// `tag` keeps the temp dirs of tests running in parallel apart.
    fn fixture_repo(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gitlog-test-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.email", "t@t.t"]);
        git(&dir, &["config", "user.name", "Tester"]);

        write(&dir, "a.txt", "line1\nline2\n");
        write(&dir, "b.txt", "x\n");
        write(&dir, "d.txt", "del\n");
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "first commit\n\nbody of first commit"]);

        write(&dir, "a.txt", "line1\nline2\nline3\n");
        std::fs::remove_file(dir.join("d.txt")).unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "second commit"]);

        git(&dir, &["mv", "b.txt", "b2.txt"]);
        git(&dir, &["commit", "-qm", "third commit: rename b -> b2"]);

        dir
    }

    #[test]
    fn parses_commits() {
        let dir = fixture_repo("parse");
        let mut commits: Vec<Commit> = Vec::new();
        let cancel = AtomicBool::new(false);
        load_commits(&dir, &cancel, |c| {
            commits.push(c);
            true
        });

        assert_eq!(commits.len(), 3);

        // newest first: the rename commit, tip of main -> marked with the branch
        let c = &commits[0];
        assert_eq!(c.branches, vec!["main"]);
        assert_eq!(c.subject, "third commit: rename b -> b2");
        assert_eq!(c.author, "Tester <t@t.t>");
        assert_eq!(c.committer, "Tester <t@t.t>");
        assert_eq!(c.files.len(), 1);
        assert_eq!(c.files[0].name, "b.txt => b2.txt");
        assert_eq!(c.files[0].action, Action::Renamed);
        assert_eq!(c.files[0].added, Some(0));
        assert_eq!(c.files[0].deleted, Some(0));
        assert!(c.timestamp > 0); // committer time, not a formatted string

        // second commit: modify + delete, not at any branch tip
        let c = &commits[1];
        assert!(c.branches.is_empty());
        assert_eq!(c.subject, "second commit");
        assert_eq!(c.message, "second commit");
        assert_eq!(c.files.len(), 2);
        let a = c.files.iter().find(|f| f.name == "a.txt").unwrap();
        assert_eq!(a.action, Action::Modified);
        assert_eq!(a.added, Some(1));
        assert_eq!(a.deleted, Some(0));
        let d = c.files.iter().find(|f| f.name == "d.txt").unwrap();
        assert_eq!(d.action, Action::Deleted);
        assert_eq!(d.added, Some(0));
        assert_eq!(d.deleted, Some(1));

        // first commit: three adds, message with body
        let c = &commits[2];
        assert_eq!(c.subject, "first commit");
        assert_eq!(c.message, "first commit\n\nbody of first commit");
        assert_eq!(c.files.len(), 3);
        assert!(c.files.iter().all(|f| f.action == Action::Added));
        let a = c.files.iter().find(|f| f.name == "a.txt").unwrap();
        assert_eq!(a.added, Some(2));

        // file sizes at the newest commit
        let head = std::str::from_utf8(
            &Command::new("git")
                .args(["-C", &dir.to_string_lossy(), "rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        let second = std::str::from_utf8(
            &Command::new("git")
                .args(["-C", &dir.to_string_lossy(), "rev-parse", "HEAD~1"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        let sizes = file_sizes(&dir, &head);
        assert_eq!(sizes.get("b2.txt"), Some(&2));
        assert_eq!(sizes.get("a.txt"), Some(&18));

        // branch tips: local branch first, then remote-tracking branches
        git(&dir, &["update-ref", "refs/remotes/origin/main", &head]);
        git(&dir, &["update-ref", "refs/remotes/github/main", &second]);
        // remote HEAD symref (as created by git fetch); its refname:short
        // is "origin" and it must not show up as a branch tip
        git(&dir, &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ]);
        let tips = branch_tips(&dir);
        assert_eq!(tips.get(&head), Some(&vec!["main".to_string(), "origin/main".to_string()]));
        assert_eq!(tips.get(&second), Some(&vec!["github/main".to_string()]));
        assert!(!tips.values().flatten().any(|b| b == "origin"));
    }

    #[test]
    fn worktree_changes_and_head() {
        let dir = fixture_repo("worktree");
        // HEAD (rename commit): a.txt (3 lines), b2.txt
        // working-copy changes: a.txt +1 line (staged), b2.txt deleted
        // (unstaged), new.txt untracked
        write(&dir, "a.txt", "line1\nline2\nline3\nline4\n");
        git(&dir, &["add", "-A", "--", "a.txt"]);
        std::fs::remove_file(dir.join("b2.txt")).unwrap();
        write(&dir, "new.txt", "fresh\n");

        let files = worktree_changes(&dir);
        let by_name = |name: &str| files.iter().find(|f| f.path == name).unwrap_or_else(|| {
            panic!("{name} not in worktree_changes: {files:?}")
        });
        assert_eq!(files.len(), 3);
        let a = by_name("a.txt");
        assert_eq!(a.action, Action::Modified);
        assert_eq!(a.added, Some(1));
        assert_eq!(a.deleted, Some(0));
        assert!(a.staged);
        assert!(a.in_head);
        let b = by_name("b2.txt");
        assert_eq!(b.action, Action::Deleted);
        assert!(!b.staged);
        assert!(b.in_head);
        let n = by_name("new.txt");
        assert_eq!(n.action, Action::Added);
        assert_eq!((n.added, n.deleted), (None, None)); // no diff for untracked
        assert!(!n.staged);
        assert!(!n.in_head);

        // previous commit: the rename, shown as "old => new" with the new path
        let head = head_files(&dir).expect("head_files");
        assert_eq!(head.len(), 1);
        assert_eq!(head[0].path, "b2.txt");
        assert_eq!(head[0].display_name, "b.txt => b2.txt");
        assert_eq!(head[0].action, Action::Renamed);
        assert!(head[0].staged);
        assert!(head[0].in_head);

        assert_eq!(head_message(&dir), Some("third commit: rename b -> b2".to_string()));
        assert!(has_head(&dir));
    }

    #[test]
    fn amend_commit_flow() {
        let dir = fixture_repo("amend");
        // amend the rename commit: keep a.txt (modified in the work tree),
        // drop b2.txt -> it must leave the commit and stay in the work tree
        write(&dir, "a.txt", "line1\nline2\nline3\nline4\n");
        rm_cached_path(&dir, "b2.txt").unwrap();
        stage_path(&dir, "a.txt").unwrap();
        do_commit(&dir, "amended message", true).unwrap();

        // the amended commit no longer contains b2.txt
        let head = head_files(&dir).expect("head_files after amend");
        assert!(!head.iter().any(|f| f.path == "b2.txt"));
        let a = head.iter().find(|f| f.path == "a.txt").expect("a.txt in amended commit");
        assert_eq!(a.action, Action::Modified);
        assert_eq!(a.added, Some(1));
        // b2.txt stays in the work tree as an untracked file
        assert!(dir.join("b2.txt").exists());
        assert!(untracked_files(&dir).contains(&"b2.txt".to_string()));
        assert_eq!(head_message(&dir), Some("amended message".to_string()));
    }

    /// `git -C repo rev-parse --verify <what>` (panics on failure).
    fn rev(repo: &Path, what: &str) -> String {
        let out = Command::new("git")
            .args(["-C", &repo.to_string_lossy(), "rev-parse", "--verify", what])
            .output()
            .expect("failed to run git");
        assert!(
            out.status.success(),
            "git rev-parse {what} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn remotes_and_push() {
        let dir = fixture_repo("push");
        // two bare "remote" repositories
        let origin = std::env::temp_dir()
            .join(format!("gitlog-test-push-origin-{}", std::process::id()));
        let backup = std::env::temp_dir()
            .join(format!("gitlog-test-push-backup-{}", std::process::id()));
        for bare in [&origin, &backup] {
            let _ = std::fs::remove_dir_all(bare);
            let out = Command::new("git")
                .args(["init", "--bare", "-q", &bare.to_string_lossy()])
                .output()
                .expect("failed to run git");
            assert!(
                out.status.success(),
                "git init --bare failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        git(&dir, &["remote", "add", "origin", &origin.to_string_lossy()]);
        assert_eq!(remotes(&dir), vec!["origin".to_string()]);

        // normal push
        run_git(&dir, &["push", "origin", "main"]).unwrap();
        assert_eq!(rev(&origin, "refs/heads/main"), rev(&dir, "HEAD"));

        // a remote that is ahead rejects a non-force push, force succeeds:
        // add a commit, push it, then reset the branch back
        write(&dir, "c.txt", "c\n");
        git(&dir, &["add", "c.txt"]);
        git(&dir, &["commit", "-qm", "fourth commit"]);
        run_git(&dir, &["push", "origin", "main"]).unwrap();
        git(&dir, &["reset", "--hard", "HEAD~1"]);
        assert!(run_git(&dir, &["push", "origin", "main"]).is_err());
        run_git(&dir, &["push", "--force", "origin", "main"]).unwrap();
        let head = rev(&dir, "HEAD");
        assert_eq!(rev(&origin, "refs/heads/main"), head);

        // tags are pushed with --tags
        git(&dir, &["tag", "v1"]);
        run_git(&dir, &["push", "--tags", "origin"]).unwrap();
        assert_eq!(rev(&origin, "refs/tags/v1"), head);

        // a second remote is listed too (`git remote` is alphabetical)
        git(&dir, &["remote", "add", "backup", &backup.to_string_lossy()]);
        assert_eq!(
            remotes(&dir),
            vec!["backup".to_string(), "origin".to_string()]
        );

        // --set-upstream records the tracking branch
        run_git(&dir, &["push", "--set-upstream", "origin", "main"]).unwrap();
        let out = Command::new("git")
            .args(["-C", &dir.to_string_lossy(), "config", "branch.main.remote"])
            .output()
            .expect("failed to run git");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "origin"
        );
    }

    /// Fresh repo with identity configured, for the rebase tests.
    fn rebase_repo(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gitlog-test-rebase-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.email", "t@t.t"]);
        git(&dir, &["config", "user.name", "Tester"]);
        dir
    }

    fn git_out(repo: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(["-C", &repo.to_string_lossy()])
            .args(args)
            .output()
            .expect("failed to run git");
        assert!(
            out.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    #[test]
    fn commit_details_fields() {
        let dir = rebase_repo("details");
        write(&dir, "a.txt", "x\n");
        git(&dir, &["add", "a.txt"]);
        let out = Command::new("git")
            .args(["-C", &dir.to_string_lossy(), "commit", "-qm", "hello\n\nbody"])
            .env("GIT_AUTHOR_NAME", "Ann Author")
            .env("GIT_AUTHOR_EMAIL", "ann@a.io")
            .env("GIT_AUTHOR_DATE", "2026-01-02 03:04:05")
            .env("GIT_COMMITTER_NAME", "Carl Committer")
            .env("GIT_COMMITTER_EMAIL", "carl@c.io")
            .env("GIT_COMMITTER_DATE", "2026-01-03 04:05:06")
            .output()
            .expect("failed to run git");
        assert!(
            out.status.success(),
            "commit failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let d = commit_details(&dir, "HEAD").unwrap();
        assert_eq!(d.author, "Ann Author");
        assert_eq!(d.author_email, "ann@a.io");
        assert_eq!(d.author_date, "2026-01-02 03:04:05");
        assert_eq!(d.committer, "Carl Committer");
        assert_eq!(d.committer_email, "carl@c.io");
        assert_eq!(d.committer_date, "2026-01-03 04:05:06");
        assert_eq!(d.message, "hello\n\nbody");

        // unknown hash is an error
        assert!(commit_details(&dir, "deadbeef").is_err());
    }

    #[test]
    fn edit_head_commit_amend() {
        let dir = rebase_repo("head");
        write(&dir, "a.txt", "content\n");
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-qm", "first"]);

        let mut d = commit_details(&dir, "HEAD").unwrap();
        d.author = "New Author".into();
        d.author_email = "na@x.io".into();
        d.author_date = "2026-01-01 02:03:04".into();
        d.committer = "New Committer".into();
        d.committer_email = "nc@x.io".into();
        d.committer_date = "2026-01-02 03:04:05".into();
        d.message = "rewritten\n\nbody".into();

        // a dirty work tree is refused
        write(&dir, "a.txt", "dirty\n");
        let err = edit_commit_details(&dir, "HEAD", &d).unwrap_err();
        assert!(err.contains("uncommitted changes"), "got: {err}");

        git(&dir, &["checkout", "-q", "--", "a.txt"]);
        edit_commit_details(&dir, "HEAD", &d).unwrap();

        let after = commit_details(&dir, "HEAD").unwrap();
        assert_eq!(after.author, "New Author");
        assert_eq!(after.author_email, "na@x.io");
        assert_eq!(after.author_date, "2026-01-01 02:03:04");
        assert_eq!(after.committer, "New Committer");
        assert_eq!(after.committer_email, "nc@x.io");
        assert_eq!(after.committer_date, "2026-01-02 03:04:05");
        assert_eq!(after.message, "rewritten\n\nbody");
        // the file content is untouched
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "content\n");
        assert_eq!(
            git_out(&dir, &["log", "-1", "--format=", "--name-only"]).lines().filter(|l| !l.is_empty()).collect::<Vec<_>>(),
            ["a.txt"]
        );
    }

    #[test]
    fn edit_middle_commit_rebase() {
        let dir = rebase_repo("middle");
        for (f, m) in [("a.txt", "first"), ("b.txt", "second"), ("c.txt", "third")] {
            write(&dir, f, &format!("{f}\n"));
            git(&dir, &["add", f]);
            git(&dir, &["commit", "-qm", m]);
        }
        let middle = rev(&dir, "HEAD~1");

        let mut d = commit_details(&dir, &middle).unwrap();
        assert_eq!(d.message, "second");
        d.author = "New Author".into();
        d.author_email = "na@x.io".into();
        d.author_date = "2026-01-01 02:03:04".into();
        d.committer = "New Committer".into();
        d.committer_email = "nc@x.io".into();
        d.committer_date = "2026-01-02 03:04:05".into();
        d.message = "second (rewritten)".into();

        edit_commit_details(&dir, &middle, &d).unwrap();

        // top and bottom commits keep their identity and message
        assert_eq!(
            git_out(&dir, &["log", "--format=%s|%an", "-3"]).lines().collect::<Vec<_>>(),
            ["third|Tester", "second (rewritten)|New Author", "first|Tester"]
        );

        let after = commit_details(&dir, "HEAD~1").unwrap();
        assert_eq!(after.author_email, "na@x.io");
        assert_eq!(after.author_date, "2026-01-01 02:03:04");
        assert_eq!(after.committer, "New Committer");
        assert_eq!(after.committer_email, "nc@x.io");
        assert_eq!(after.committer_date, "2026-01-02 03:04:05");
        assert_eq!(after.message, "second (rewritten)");

        // work tree clean, all files intact
        assert!(git_out(&dir, &["status", "--porcelain"]).trim().is_empty());
        assert_eq!(
            git_out(&dir, &["ls-tree", "-r", "--name-only", "HEAD"]).lines().collect::<Vec<_>>(),
            ["a.txt", "b.txt", "c.txt"]
        );
    }

    #[test]
    fn edit_preserves_descendant_committers() {
        let dir = rebase_repo("desc");
        // the descendant's author and committer differ from the local
        // identity (Tester <t@t.t>), which rewrites would replace
        write(&dir, "a.txt", "x\n");
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-qm", "first"]);
        write(&dir, "b.txt", "y\n");
        git(&dir, &["add", "b.txt"]);
        let out = Command::new("git")
            .args(["-C", &dir.to_string_lossy(), "commit", "-qm", "second"])
            .env("GIT_AUTHOR_NAME", "Old Author")
            .env("GIT_AUTHOR_EMAIL", "old-author@o.io")
            .env("GIT_AUTHOR_DATE", "2026-01-04 05:06:07")
            .env("GIT_COMMITTER_NAME", "Old Committer")
            .env("GIT_COMMITTER_EMAIL", "old@o.io")
            .env("GIT_COMMITTER_DATE", "2026-01-05 06:07:08")
            .output()
            .expect("failed to run git");
        assert!(
            out.status.success(),
            "commit failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let before = commit_details(&dir, "HEAD").unwrap();

        let first = rev(&dir, "HEAD~1");
        let mut d = commit_details(&dir, &first).unwrap();
        d.message = "first (rewritten)".into();
        edit_commit_details(&dir, &first, &d).unwrap();

        let after = commit_details(&dir, "HEAD").unwrap();
        // the descendant is untouched: same committer (name, email and
        // date) and author, only the hash differs
        assert_eq!(after.committer, before.committer);
        assert_eq!(after.committer_email, before.committer_email);
        assert_eq!(after.committer_date, before.committer_date);
        assert_eq!(after.author, before.author);
        assert_eq!(after.author_email, before.author_email);
        assert_eq!(after.author_date, before.author_date);
        // and the preserved author really is the original one, not the
        // local identity
        assert_eq!(after.author, "Old Author");
        assert_eq!(after.author_email, "old-author@o.io");
        assert_eq!(after.author_date, "2026-01-04 05:06:07");
        assert_eq!(after.message, "second");
        assert_eq!(after.committer_date, "2026-01-05 06:07:08");
        assert!(git_out(&dir, &["status", "--porcelain"]).trim().is_empty());
    }

    #[test]
    fn edit_with_merge_descendant() {
        let dir = rebase_repo("merge");
        write(&dir, "a.txt", "x\n");
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-qm", "first"]);
        write(&dir, "b.txt", "y\n");
        git(&dir, &["add", "b.txt"]);
        let out = Command::new("git")
            .args(["-C", &dir.to_string_lossy(), "commit", "-qm", "second"])
            .env("GIT_COMMITTER_NAME", "Old Committer")
            .env("GIT_COMMITTER_EMAIL", "old@o.io")
            .env("GIT_COMMITTER_DATE", "2026-01-05 06:07:08")
            .output()
            .expect("failed to run git");
        assert!(
            out.status.success(),
            "commit failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        // a side branch off "first" and a merge on top of "second"
        git(&dir, &["checkout", "-qb", "side", "HEAD~1"]);
        write(&dir, "c.txt", "z\n");
        git(&dir, &["add", "c.txt"]);
        git(&dir, &["commit", "-qm", "side work"]);
        git(&dir, &["checkout", "-q", "main"]);
        let out = Command::new("git")
            .args(["-C", &dir.to_string_lossy(), "merge", "--no-ff", "-q", "-m", "merged", "side"])
            .env("GIT_COMMITTER_NAME", "Merge Bot")
            .env("GIT_COMMITTER_EMAIL", "bot@b.io")
            .env("GIT_COMMITTER_DATE", "2026-01-09 10:11:12")
            .output()
            .expect("failed to run git");
        assert!(
            out.status.success(),
            "merge failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let first = rev(&dir, "HEAD~2");
        let mut d = commit_details(&dir, &first).unwrap();
        d.message = "first (rewritten)".into();
        edit_commit_details(&dir, &first, &d).unwrap();

        // the merge commit keeps both parents, its committer and its
        // message
        let parent_str = git_out(&dir, &["log", "-1", "--format=%P", "HEAD"]);
        let parents: Vec<&str> = parent_str.split_whitespace().collect();
        assert_eq!(parents.len(), 2);
        let head = commit_details(&dir, "HEAD").unwrap();
        assert_eq!(head.committer, "Merge Bot");
        assert_eq!(head.committer_email, "bot@b.io");
        assert_eq!(head.committer_date, "2026-01-09 10:11:12");
        assert_eq!(head.message, "merged");
        // the "second" descendant (first parent of the merge) keeps its
        // committer
        let second = commit_details(&dir, parents[0]).unwrap();
        assert_eq!(second.committer, "Old Committer");
        assert_eq!(second.committer_date, "2026-01-05 06:07:08");
        // the side branch's commit was re-created too (its parent
        // changed), but author, committer and message are unchanged and
        // the `side` branch ref points at the new commit
        let side = commit_details(&dir, parents[1]).unwrap();
        assert_eq!(side.message, "side work");
        assert_eq!(side.committer, "Tester");
        assert_eq!(side.author, "Tester");
        assert_eq!(git_out(&dir, &["rev-parse", "side"]).trim(), parents[1]);
        // ... and it was re-parented onto the rewritten "first"
        let side_parent = git_out(&dir, &["log", "-1", "--format=%P", parents[1]]);
        assert_eq!(side_parent.trim(), rev(&dir, "HEAD~2"));
        assert!(git_out(&dir, &["status", "--porcelain"]).trim().is_empty());
    }

    #[test]
    fn edit_root_commit() {
        let dir = rebase_repo("root");
        write(&dir, "a.txt", "a\n");
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-qm", "first"]);
        write(&dir, "b.txt", "b\n");
        git(&dir, &["add", "b.txt"]);
        git(&dir, &["commit", "-qm", "second"]);

        let root = rev(&dir, "HEAD~1");
        let mut d = commit_details(&dir, &root).unwrap();
        d.author = "New Author".into();
        d.author_email = "na@x.io".into();
        d.author_date = "2026-01-01 02:03:04".into();
        d.committer = "New Committer".into();
        d.committer_email = "nc@x.io".into();
        d.committer_date = "2026-01-02 03:04:05".into();
        d.message = "first (rewritten)".into();

        edit_commit_details(&dir, &root, &d).unwrap();

        assert_eq!(
            git_out(&dir, &["log", "--format=%s|%an", "-2"]).lines().collect::<Vec<_>>(),
            ["second|Tester", "first (rewritten)|New Author"]
        );
        assert!(git_out(&dir, &["status", "--porcelain"]).trim().is_empty());
    }

    #[test]
    fn is_pushed_check() {
        let dir = fixture_repo("pushed");
        // no remotes configured: nothing is pushed
        assert!(!is_pushed(&dir, "HEAD"));

        let bare = std::env::temp_dir().join(format!("gitlog-test-rebase-origin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&bare);
        let out = Command::new("git")
            .args(["init", "--bare", "-q", &bare.to_string_lossy()])
            .output()
            .expect("failed to run git");
        assert!(
            out.status.success(),
            "git init --bare failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        git(&dir, &["remote", "add", "origin", &bare.to_string_lossy()]);
        run_git(&dir, &["push", "origin", "main"]).unwrap();

        // the whole pushed history is reachable from origin/main
        assert!(is_pushed(&dir, "HEAD"));
        assert!(is_pushed(&dir, "HEAD~1"));
        assert!(is_pushed(&dir, "HEAD~2"));
    }
}

//! Git data access: parses `git log` output and queries file sizes.

use std::collections::{HashMap, VecDeque};
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
    pub hash: String,
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
pub fn head_info(repo: &Path) -> String {
    let branch = std::str::from_utf8(
        &Command::new("git")
            .args(["-C", &repo.to_string_lossy(), "branch", "--show-current"])
            .output()
            .ok()
            .map(|o| o.stdout)
            .unwrap_or_default(),
    )
    .unwrap_or("")
    .trim()
    .to_string();
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
            "--format=%(objectname)\x01%(refname:short)",
        ])
        .output()
    else {
        return HashMap::new();
    };
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some((hash, name)) = line.split_once('\x01') {
            map.entry(hash.trim().to_string())
                .or_default()
                .push(name.to_string());
        }
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
    fn fixture_repo() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gitlog-test-{}", std::process::id()));
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
        let dir = fixture_repo();
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
        let tips = branch_tips(&dir);
        assert_eq!(tips.get(&head), Some(&vec!["main".to_string(), "origin/main".to_string()]));
        assert_eq!(tips.get(&second), Some(&vec!["github/main".to_string()]));
    }
}

//! Helpers shared by the integration tests: temporary git repositories and
//! small wrappers around `git`.
//!
//! Not every test binary uses every helper, so the whole module is marked
//! `allow(dead_code)`.

#![allow(dead_code)]

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

/// Runs git in `repo`, panicking with the stderr on failure.
pub fn git(repo: &Path, args: &[&str]) {
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

/// Runs git in `repo` and returns stdout, panicking with the stderr on failure.
pub fn git_out(repo: &Path, args: &[&str]) -> String {
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

/// `git -C repo rev-parse --verify <what>` (panics on failure).
pub fn rev(repo: &Path, what: &str) -> String {
    git_out(repo, &["rev-parse", "--verify", what]).trim().to_string()
}

/// Runs git with extra environment variables (used to commit with a
/// specific author/committer identity), panicking on failure.
pub fn git_env(repo: &Path, args: &[&str], envs: &[(&str, &str)]) {
    let out = Command::new("git")
        .args(["-C", &repo.to_string_lossy()])
        .args(args)
        .envs(envs.iter().copied())
        .output()
        .expect("failed to run git");
    assert!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Creates a file (with parent directories) in `repo`.
pub fn write(repo: &Path, name: &str, content: &str) {
    let path = repo.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

/// Empty temporary directory, named after `tag` so tests running in parallel
/// (each test binary is a separate process) keep their folders apart.
pub fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "gitlog-test-{}-{}",
        std::process::id(),
        tag
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Configures the committer identity git refuses to commit without.
pub fn configure_identity(repo: &Path) {
    git(repo, &["config", "user.email", "t@t.t"]);
    git(repo, &["config", "user.name", "Tester"]);
}

/// Empty initialized repository (branch `main`, identity configured).
pub fn init_repo(tag: &str) -> PathBuf {
    let dir = temp_dir(tag);
    git(&dir, &["init", "-q", "-b", "main"]);
    configure_identity(&dir);
    dir
}

/// Repository with add / modify / delete / rename commits:
/// - `first commit` adds a.txt (2 lines), b.txt, d.txt
/// - `second commit` adds a line to a.txt, deletes d.txt
/// - `third commit: rename b -> b2` renames b.txt to b2.txt
pub fn fixture_repo(tag: &str) -> PathBuf {
    let dir = init_repo(tag);

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

/// Bare repository used as a "remote" (temporary directory suffixed `-<tag>`).
pub fn bare_remote(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "gitlog-test-{}-{}-bare",
        std::process::id(),
        tag
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let out = Command::new("git")
        .args(["init", "--bare", "-q", &dir.to_string_lossy()])
        .output()
        .expect("failed to run git");
    assert!(
        out.status.success(),
        "git init --bare failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    dir
}

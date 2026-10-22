use std::path::Path;
use std::process::Command;

pub fn git_ok(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

pub fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("spawn git");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repo on `main` with one commit, ready to be cloned as an origin.
pub fn init_repo_with_commit(dir: &Path) {
    let out = Command::new("git")
        .args(["init", "-b", "main"])
        .arg(dir)
        .output()
        .expect("spawn git init");
    assert!(
        out.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    git_ok(dir, &["config", "user.email", "test@example.com"]);
    git_ok(dir, &["config", "user.name", "Test"]);
    std::fs::write(dir.join("README.md"), "hi").unwrap();
    git_ok(dir, &["add", "."]);
    git_ok(dir, &["commit", "-m", "init"]);
}

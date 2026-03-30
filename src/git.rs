use std::path::Path;
use std::process::Command;

/// Stage a file and commit with a generated message.
pub fn add_and_commit(repo_dir: &Path, file_path: &Path, message: &str) -> anyhow::Result<()> {
    let file_rel = file_path
        .strip_prefix(repo_dir)
        .unwrap_or(file_path)
        .to_string_lossy();

    let git = |args: &[&str]| -> anyhow::Result<()> {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo_dir)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git {} failed: {}", args.join(" "), stderr.trim());
        }
        Ok(())
    };

    git(&["add", &file_rel])?;
    git(&["commit", "-m", message])?;

    Ok(())
}

/// Check whether the given directory is inside a git repository.
pub fn is_git_repo(dir: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(dir)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Initialise a git repo in `dir` with a test user identity.
    fn git_init(dir: &Path) {
        Command::new("git")
            .args(["init"])
            .current_dir(dir)
            .output()
            .expect("git init failed");
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir)
            .output()
            .expect("git config user.email failed");
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir)
            .output()
            .expect("git config user.name failed");
    }

    #[test]
    fn is_git_repo_true_for_initialised_dir() {
        let tmp = tempdir().unwrap();
        git_init(tmp.path());
        assert!(is_git_repo(tmp.path()));
    }

    #[test]
    fn is_git_repo_false_for_plain_dir() {
        let tmp = tempdir().unwrap();
        assert!(!is_git_repo(tmp.path()));
    }

    #[test]
    fn add_and_commit_creates_commit() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        let file = dir.join("hello.txt");
        fs::write(&file, "hello world").unwrap();

        add_and_commit(dir, &file, "initial commit").unwrap();

        let output = Command::new("git")
            .args(["log", "--oneline"])
            .current_dir(dir)
            .output()
            .unwrap();

        let log = String::from_utf8_lossy(&output.stdout);
        assert!(
            log.contains("initial commit"),
            "git log should contain the commit message, got: {log}"
        );
    }

    #[test]
    fn add_and_commit_fails_for_nonexistent_file() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        let bogus = dir.join("does_not_exist.txt");
        let result = add_and_commit(dir, &bogus, "should fail");
        assert!(result.is_err());
    }
}

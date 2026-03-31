use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

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

/// Guard that removes a file on drop, ensuring cleanup on all code paths.
struct TempFileGuard<'a>(&'a Path);

impl Drop for TempFileGuard<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0);
    }
}

/// Run a git command and return its trimmed stdout on success.
fn git_output(repo_dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_dir)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run a git command, piping `input` to stdin, and return trimmed stdout.
fn git_stdin(repo_dir: &Path, args: &[&str], input: &[u8]) -> anyhow::Result<String> {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(repo_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(ref mut stdin) = child.stdin {
        stdin.write_all(input)?;
    }
    // Drop stdin so the child sees EOF.
    drop(child.stdin.take());

    let output = child.wait_with_output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Generate a unique branch name by appending a numeric suffix when needed.
///
/// Tries `<prefix><slug>`, then `<prefix><slug>-2`, `-3`, etc. up to 100.
fn generate_branch_name(repo_dir: &Path, prefix: &str, slug: &str) -> anyhow::Result<String> {
    let base = format!("{prefix}{slug}");
    let ref_path = format!("refs/heads/{base}");

    let exists = Command::new("git")
        .args(["show-ref", "--verify", "--quiet", &ref_path])
        .current_dir(repo_dir)
        .output()?
        .status
        .success();

    if !exists {
        return Ok(base);
    }

    for n in 2..=100 {
        let candidate = format!("{base}-{n}");
        let candidate_ref = format!("refs/heads/{candidate}");
        let exists = Command::new("git")
            .args(["show-ref", "--verify", "--quiet", &candidate_ref])
            .current_dir(repo_dir)
            .output()?
            .status
            .success();
        if !exists {
            return Ok(candidate);
        }
    }

    anyhow::bail!("could not find an available branch name for {base} after 100 attempts");
}

/// Create a commit on a new branch forked from HEAD without touching
/// the working tree, index, or HEAD.
///
/// Returns the branch name that was created.
pub fn commit_to_new_branch(
    repo_dir: &Path,
    prefix: &str,
    slug: &str,
    file_path: &str,
    content: &str,
    message: &str,
) -> anyhow::Result<String> {
    // a. Generate unique branch name.
    let branch_name = generate_branch_name(repo_dir, prefix, slug)?;

    // b. Create blob from content.
    let blob_sha = git_stdin(
        repo_dir,
        &["hash-object", "-w", "--stdin"],
        content.as_bytes(),
    )?;

    // c. Resolve HEAD commit.
    let parent_sha = git_output(repo_dir, &["rev-parse", "HEAD"])?;

    // d. Build tree using a temporary index unique to this process.
    let tmp_index = repo_dir.join(format!(
        ".git/lore-tmp-index-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let tmp_index_str = tmp_index.to_string_lossy().to_string();

    // Guard ensures the temp index is cleaned up even on early return.
    let tree_sha = {
        let _guard = TempFileGuard(&tmp_index);

        let read_tree = Command::new("git")
            .args(["read-tree", "HEAD^{tree}"])
            .env("GIT_INDEX_FILE", &tmp_index_str)
            .current_dir(repo_dir)
            .output()?;
        if !read_tree.status.success() {
            let stderr = String::from_utf8_lossy(&read_tree.stderr);
            anyhow::bail!("git read-tree failed: {}", stderr.trim());
        }

        let cacheinfo = format!("100644,{blob_sha},{file_path}");
        let update_index = Command::new("git")
            .args(["update-index", "--add", "--cacheinfo", &cacheinfo])
            .env("GIT_INDEX_FILE", &tmp_index_str)
            .current_dir(repo_dir)
            .output()?;
        if !update_index.status.success() {
            let stderr = String::from_utf8_lossy(&update_index.stderr);
            anyhow::bail!("git update-index failed: {}", stderr.trim());
        }

        let write_tree = Command::new("git")
            .args(["write-tree"])
            .env("GIT_INDEX_FILE", &tmp_index_str)
            .current_dir(repo_dir)
            .output()?;
        if !write_tree.status.success() {
            let stderr = String::from_utf8_lossy(&write_tree.stderr);
            anyhow::bail!("git write-tree failed: {}", stderr.trim());
        }
        String::from_utf8_lossy(&write_tree.stdout)
            .trim()
            .to_string()
    }; // _guard drops here, removing the temp index file

    // e. Create the commit object.
    let commit_sha = git_output(
        repo_dir,
        &["commit-tree", &tree_sha, "-p", &parent_sha, "-m", message],
    )?;

    // f. Create the branch ref.
    let ref_name = format!("refs/heads/{branch_name}");
    git_output(repo_dir, &["update-ref", &ref_name, &commit_sha])?;

    Ok(branch_name)
}

/// Push a branch to the `origin` remote.
pub fn push_branch(repo_dir: &Path, branch: &str) -> anyhow::Result<()> {
    let output = Command::new("git")
        .args(["push", "origin", branch])
        .current_dir(repo_dir)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git push origin {} failed: {}", branch, stderr.trim());
    }

    Ok(())
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
        // Disable GPG signing for test repos so commits don't require a key.
        Command::new("git")
            .args(["config", "commit.gpgsign", "false"])
            .current_dir(dir)
            .output()
            .expect("git config commit.gpgsign failed");
    }

    /// Initialise a repo with a bare remote named `origin`.
    ///
    /// Creates an initial commit so HEAD exists, and pushes to the bare remote.
    /// Returns the `TempDir` holding the bare repo (must be kept alive).
    fn git_init_with_remote(dir: &Path) -> tempfile::TempDir {
        git_init(dir);

        // Create a bare repo as remote.
        let bare_tmp = tempdir().unwrap();
        Command::new("git")
            .args(["init", "--bare"])
            .current_dir(bare_tmp.path())
            .output()
            .expect("git init --bare failed");

        // Add it as origin.
        let bare_path = bare_tmp.path().to_string_lossy().to_string();
        Command::new("git")
            .args(["remote", "add", "origin", &bare_path])
            .current_dir(dir)
            .output()
            .expect("git remote add failed");

        // Create an initial commit so HEAD exists.
        let init_file = dir.join("README");
        fs::write(&init_file, "init").unwrap();
        Command::new("git")
            .args(["add", "README"])
            .current_dir(dir)
            .output()
            .expect("git add failed");
        Command::new("git")
            .args(["commit", "-m", "initial commit"])
            .current_dir(dir)
            .output()
            .expect("git commit failed");

        // Push to bare remote so it has the initial state.
        Command::new("git")
            .args(["push", "-u", "origin", "HEAD"])
            .current_dir(dir)
            .output()
            .expect("git push failed");

        bare_tmp
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

    #[test]
    fn commit_to_new_branch_creates_branch() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let _bare = git_init_with_remote(dir);

        // Record HEAD before the operation.
        let head_before = git_output(dir, &["rev-parse", "HEAD"]).unwrap();

        let branch = commit_to_new_branch(
            dir,
            "test/",
            "hello",
            "hello.txt",
            "hello content",
            "add hello",
        )
        .unwrap();

        assert_eq!(branch, "test/hello");

        // The branch commit should contain the file.
        let show = git_output(dir, &["show", "test/hello:hello.txt"]).unwrap();
        assert_eq!(show, "hello content");

        // HEAD should be unchanged.
        let head_after = git_output(dir, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(head_before, head_after);

        // Working tree should not contain hello.txt.
        assert!(!dir.join("hello.txt").exists());
    }

    #[test]
    fn commit_to_new_branch_two_different_slugs() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let _bare = git_init_with_remote(dir);

        let branch1 =
            commit_to_new_branch(dir, "test/", "alpha", "a.txt", "aaa", "add alpha").unwrap();

        let branch2 =
            commit_to_new_branch(dir, "test/", "beta", "b.txt", "bbb", "add beta").unwrap();

        assert_eq!(branch1, "test/alpha");
        assert_eq!(branch2, "test/beta");

        // Each branch should have its own file.
        let a = git_output(dir, &["show", "test/alpha:a.txt"]).unwrap();
        assert_eq!(a, "aaa");

        let b = git_output(dir, &["show", "test/beta:b.txt"]).unwrap();
        assert_eq!(b, "bbb");
    }

    #[test]
    fn branch_name_collision_disambiguates() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let _bare = git_init_with_remote(dir);

        let branch1 =
            commit_to_new_branch(dir, "test/", "foo", "one.txt", "first", "first commit").unwrap();
        assert_eq!(branch1, "test/foo");

        let branch2 =
            commit_to_new_branch(dir, "test/", "foo", "two.txt", "second", "second commit")
                .unwrap();
        assert_eq!(branch2, "test/foo-2");
    }

    #[test]
    fn push_branch_to_bare_remote() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let bare = git_init_with_remote(dir);

        let branch = commit_to_new_branch(
            dir,
            "test/",
            "pushed",
            "data.txt",
            "remote content",
            "push test",
        )
        .unwrap();

        push_branch(dir, &branch).unwrap();

        // Verify content on the bare remote.
        let bare_dir = bare.path();
        let show_ref = format!("{branch}:data.txt");
        let output = Command::new("git")
            .args(["--git-dir", &bare_dir.to_string_lossy(), "show", &show_ref])
            .output()
            .unwrap();

        let content = String::from_utf8_lossy(&output.stdout);
        assert_eq!(content.trim(), "remote content");
    }

    #[test]
    fn push_branch_fails_without_remote() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        git_init(dir);

        // Create an initial commit so HEAD exists.
        let init_file = dir.join("README");
        fs::write(&init_file, "init").unwrap();
        Command::new("git")
            .args(["add", "README"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(dir)
            .output()
            .unwrap();

        let branch =
            commit_to_new_branch(dir, "test/", "nopush", "file.txt", "data", "no remote").unwrap();

        let result = push_branch(dir, &branch);
        assert!(result.is_err());
    }

    #[test]
    fn commit_to_new_branch_fails_on_non_git_dir() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();

        let result = commit_to_new_branch(dir, "test/", "nope", "file.txt", "data", "should fail");
        assert!(result.is_err());
    }
}

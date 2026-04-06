---
title: "Tests panic with cryptic error if git is not on PATH"
priority: P2
category: testing
status: ready
created: 2026-04-06
source: ce-review (feat/git-optional-knowledge-base second pass)
files:
  - tests/hook.rs:303-335
  - tests/smoke.rs:148-180
  - src/ingest.rs::tests (git-init helper sites)
related_pr: feat/git-optional-knowledge-base
---

# Tests panic with cryptic error if git is not on PATH

## Context

Several tests added in this branch shell out to `git init` via:

```rust
std::process::Command::new("git")
    .arg("init")
    .arg("--quiet")
    .current_dir(&dir)
    .status()
    .unwrap();
```

The `.unwrap()` panics if git is not on PATH. The panic message looks like:

```
thread 'tests::xyz' panicked at 'No such file or directory (os error 2)'
```

A maintainer reading the CI log sees a panic with no obvious connection to git. The actual fix —
install git in the CI environment — is not discoverable from the test failure.

The lore project's testing strategy explicitly assumes git is available ("git: real, in tempdir" —
see the rust/testing-strategy convention), but the assumption is not enforced or documented at the
test level.

## Proposed fix

Add a single helper that wraps the git_init call and produces a clear error:

```rust
// in src/ingest.rs::tests, tests/hook.rs, tests/smoke.rs as a shared helper
fn git_init_or_skip(dir: &Path) {
    let status = std::process::Command::new("git")
        .arg("init")
        .arg("--quiet")
        .current_dir(dir)
        .status()
        .expect(
            "git binary not found on PATH — lore tests assume a git \
             installation is available; install git or skip these tests",
        );
    assert!(status.success(), "git init failed in {}", dir.display());
}
```

Replace the inline git_init calls with this helper at all sites.

Optionally, also document the assumption in `Cargo.toml` `[package.metadata]` or in a top-level
`tests/README.md`:

```
Tests assume the `git` binary is available on PATH. CI environments must
install git before running `cargo test` or `just ci`.
```

## Alternative: skip tests if git is missing

If running on a sealed CI environment where git installation is not feasible, add a skip helper:

```rust
fn require_git() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn lore_status_reports_git_state() {
    if !require_git() {
        eprintln!("skipping: git not available");
        return;
    }
    // ... existing test
}
```

The "expect with helpful message" approach is simpler and matches the project's stated assumption
that git is always present. The skip approach is a last resort.

## References

- Adversarial finding (confidence 0.75): git binary missing causes panic
- Testing finding (confidence 0.68): hook test isolation
- Project testing strategy: "git: real (tempdir + git init)"

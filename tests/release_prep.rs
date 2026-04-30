//! Integration tests for `scripts/release-prep.sh`.
//!
//! Each test sets up a scratch directory with synthetic `Cargo.toml` and
//! `CHANGELOG.md` fixtures, invokes the script with a chosen VERSION, and
//! asserts on exit code, file mutations, and stderr messages.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use predicates::Predicate;
use predicates::str::contains;
use tempfile::TempDir;

fn script_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/release-prep.sh")
}

fn write_cargo_toml(dir: &TempDir, version: &str) {
    let body = format!("[package]\nname = \"lore\"\nversion = \"{version}\"\nedition = \"2024\"\n");
    fs::write(dir.path().join("Cargo.toml"), body).unwrap();
}

fn write_changelog(dir: &TempDir, body: &str) {
    fs::write(dir.path().join("CHANGELOG.md"), body).unwrap();
}

fn run_release_prep(dir: &TempDir, version: &str) -> std::process::Output {
    Command::new("bash")
        .arg(script_path())
        .arg(version)
        .current_dir(dir.path())
        .output()
        .expect("failed to spawn bash")
}

fn populated_changelog() -> &'static str {
    "\
# Changelog

## [Unreleased]

### Added

- A shiny new feature
- Another notable change

### Changed

- Some behaviour tweaked
"
}

#[test]
fn happy_path_bumps_cargo_and_rotates_changelog() {
    // Arrange
    let dir = TempDir::new().unwrap();
    write_cargo_toml(&dir, "0.1.0");
    write_changelog(&dir, populated_changelog());

    // Act
    let output = run_release_prep(&dir, "0.1.0-alpha.1");

    // Assert
    assert!(
        output.status.success(),
        "script failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cargo = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    assert!(
        cargo.contains("version = \"0.1.0-alpha.1\""),
        "Cargo.toml not bumped: {cargo}"
    );

    let changelog = fs::read_to_string(dir.path().join("CHANGELOG.md")).unwrap();
    assert!(
        changelog.contains("## [Unreleased]"),
        "[Unreleased] heading missing: {changelog}"
    );
    assert!(
        changelog.contains("## [0.1.0-alpha.1] - "),
        "new VERSION heading missing: {changelog}"
    );
    let unreleased_idx = changelog.find("## [Unreleased]").unwrap();
    let version_idx = changelog.find("## [0.1.0-alpha.1]").unwrap();
    assert!(
        unreleased_idx < version_idx,
        "[Unreleased] should precede the new version heading"
    );
    assert!(
        changelog.contains("A shiny new feature"),
        "previous Unreleased entries should now sit under the dated heading: {changelog}"
    );
}

#[test]
fn rejects_invalid_version_format() {
    // Arrange
    let dir = TempDir::new().unwrap();
    write_cargo_toml(&dir, "0.1.0");
    write_changelog(&dir, populated_changelog());

    // Act
    let output = run_release_prep(&dir, "1.0");

    // Assert
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        contains("not a valid semver").eval(&stderr),
        "stderr should mention semver: {stderr}"
    );

    let cargo = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    assert!(
        cargo.contains("version = \"0.1.0\""),
        "Cargo.toml must not be mutated on error"
    );
}

#[test]
fn rejects_v_prefixed_version() {
    // Arrange
    let dir = TempDir::new().unwrap();
    write_cargo_toml(&dir, "0.1.0");
    write_changelog(&dir, populated_changelog());

    // Act
    let output = run_release_prep(&dir, "v0.1.0");

    // Assert
    assert!(!output.status.success());
    assert!(contains("not a valid semver").eval(&String::from_utf8_lossy(&output.stderr)));
}

#[test]
fn rejects_when_changelog_lacks_unreleased_heading() {
    // Arrange
    let dir = TempDir::new().unwrap();
    write_cargo_toml(&dir, "0.1.0");
    write_changelog(&dir, "# Changelog\n\n## [0.0.1]\n\n- prior release\n");

    // Act
    let output = run_release_prep(&dir, "0.1.0-alpha.1");

    // Assert
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        contains("no '## [Unreleased]' heading").eval(&stderr),
        "stderr should mention missing Unreleased heading: {stderr}"
    );
}

#[test]
fn rejects_when_version_already_exists_in_changelog() {
    // Arrange
    let existing = "\
# Changelog

## [Unreleased]

- pending entry

## [0.1.0-alpha.1] - 2026-04-30

- already released
";
    let dir = TempDir::new().unwrap();
    write_cargo_toml(&dir, "0.1.0");
    write_changelog(&dir, existing);

    // Act
    let output = run_release_prep(&dir, "0.1.0-alpha.1");

    // Assert
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        contains("already has a '## [0.1.0-alpha.1]' heading").eval(&stderr),
        "stderr should name the conflicting heading: {stderr}"
    );
}

#[test]
fn rejects_empty_unreleased_block() {
    // Arrange
    let empty_unreleased = "\
# Changelog

## [Unreleased]

## [0.0.1] - 2026-01-01

- prior release
";
    let dir = TempDir::new().unwrap();
    write_cargo_toml(&dir, "0.1.0");
    write_changelog(&dir, empty_unreleased);

    // Act
    let output = run_release_prep(&dir, "0.1.0-alpha.1");

    // Assert
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        contains("has no entries").eval(&stderr),
        "stderr should explain there is nothing to release: {stderr}"
    );
}

#[test]
fn second_run_with_same_version_fails() {
    // Arrange
    let dir = TempDir::new().unwrap();
    write_cargo_toml(&dir, "0.1.0");
    write_changelog(&dir, populated_changelog());

    let first = run_release_prep(&dir, "0.1.0-alpha.1");
    assert!(first.status.success(), "first run should succeed");

    // Act — second invocation against the rotated CHANGELOG
    let second = run_release_prep(&dir, "0.1.0-alpha.1");

    // Assert
    assert!(!second.status.success(), "second run must refuse");
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        contains("already has a '## [0.1.0-alpha.1]'").eval(&stderr),
        "stderr should name the conflicting heading on rerun: {stderr}"
    );
}

#[test]
fn missing_version_argument_is_rejected() {
    // Arrange
    let dir = TempDir::new().unwrap();
    write_cargo_toml(&dir, "0.1.0");
    write_changelog(&dir, populated_changelog());

    // Act
    let output = Command::new("bash")
        .arg(script_path())
        .current_dir(dir.path())
        .output()
        .expect("failed to spawn bash");

    // Assert
    assert!(!output.status.success());
    assert!(
        contains("VERSION argument is required").eval(&String::from_utf8_lossy(&output.stderr))
    );
}

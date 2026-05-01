//! Integration tests for `scripts/release-prep.sh`.
//!
//! Each test sets up a scratch directory with synthetic `Cargo.toml` and
//! `CHANGELOG.md` fixtures, invokes the script with a chosen VERSION, and
//! asserts on exit code, file mutations, and stderr messages.

use std::fs;
use std::path::PathBuf;

use assert_cmd::Command;
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

fn release_prep(dir: &TempDir, version: &str) -> Command {
    let mut cmd = Command::new("bash");
    cmd.arg(script_path()).arg(version).current_dir(dir.path());
    cmd
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
    let dir = TempDir::new().unwrap();
    write_cargo_toml(&dir, "0.1.0");
    write_changelog(&dir, populated_changelog());

    release_prep(&dir, "0.1.0-alpha.1").assert().success();

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
    // Pin rotation semantics: previous Unreleased entries must sit UNDER the
    // new dated heading, not remain under the now-empty [Unreleased].
    let between_unreleased_and_version = &changelog[unreleased_idx..version_idx];
    assert!(
        !between_unreleased_and_version.contains("A shiny new feature"),
        "previous Unreleased entries leaked into new [Unreleased]: {between_unreleased_and_version}"
    );
    assert!(
        changelog[version_idx..].contains("A shiny new feature"),
        "previous Unreleased entries should now sit under the dated heading: {changelog}"
    );
}

#[test]
fn happy_path_accepts_rc_suffix() {
    let dir = TempDir::new().unwrap();
    write_cargo_toml(&dir, "0.1.0");
    write_changelog(&dir, populated_changelog());

    release_prep(&dir, "0.1.0-rc.2").assert().success();

    let cargo = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    assert!(cargo.contains("version = \"0.1.0-rc.2\""));
}

#[test]
fn happy_path_accepts_beta_double_digit_suffix() {
    let dir = TempDir::new().unwrap();
    write_cargo_toml(&dir, "0.1.0");
    write_changelog(&dir, populated_changelog());

    release_prep(&dir, "0.1.0-beta.10").assert().success();

    let cargo = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    assert!(cargo.contains("version = \"0.1.0-beta.10\""));
}

#[test]
fn rejects_invalid_version_format() {
    let dir = TempDir::new().unwrap();
    write_cargo_toml(&dir, "0.1.0");
    write_changelog(&dir, populated_changelog());

    release_prep(&dir, "1.0")
        .assert()
        .failure()
        .stderr(contains("not a valid semver"));

    let cargo = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    assert!(
        cargo.contains("version = \"0.1.0\""),
        "Cargo.toml must not be mutated on error"
    );
}

#[test]
fn rejects_v_prefixed_version() {
    let dir = TempDir::new().unwrap();
    write_cargo_toml(&dir, "0.1.0");
    write_changelog(&dir, populated_changelog());

    release_prep(&dir, "v0.1.0")
        .assert()
        .failure()
        .stderr(contains("not a valid semver"));
}

#[test]
fn rejects_when_changelog_lacks_unreleased_heading() {
    let dir = TempDir::new().unwrap();
    write_cargo_toml(&dir, "0.1.0");
    write_changelog(&dir, "# Changelog\n\n## [0.0.1]\n\n- prior release\n");

    release_prep(&dir, "0.1.0-alpha.1")
        .assert()
        .failure()
        .stderr(contains("no '## [Unreleased]' heading"));
}

#[test]
fn rejects_when_version_already_exists_in_changelog() {
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

    release_prep(&dir, "0.1.0-alpha.1")
        .assert()
        .failure()
        .stderr(contains("already has a '## [0.1.0-alpha.1]' heading"));
}

#[test]
fn rejects_empty_unreleased_block() {
    let empty_unreleased = "\
# Changelog

## [Unreleased]

## [0.0.1] - 2026-01-01

- prior release
";
    let dir = TempDir::new().unwrap();
    write_cargo_toml(&dir, "0.1.0");
    write_changelog(&dir, empty_unreleased);

    release_prep(&dir, "0.1.0-alpha.1")
        .assert()
        .failure()
        .stderr(contains("has no entries"));
}

#[test]
fn second_run_with_same_version_fails() {
    let dir = TempDir::new().unwrap();
    write_cargo_toml(&dir, "0.1.0");
    write_changelog(&dir, populated_changelog());

    release_prep(&dir, "0.1.0-alpha.1").assert().success();

    release_prep(&dir, "0.1.0-alpha.1")
        .assert()
        .failure()
        .stderr(contains("already has a '## [0.1.0-alpha.1]'"));
}

#[test]
fn rejects_when_cargo_toml_is_missing() {
    let dir = TempDir::new().unwrap();
    write_changelog(&dir, populated_changelog());

    release_prep(&dir, "0.1.0-alpha.1")
        .assert()
        .failure()
        .stderr(contains("Cargo.toml not found"));
}

#[test]
fn rejects_when_changelog_is_missing() {
    let dir = TempDir::new().unwrap();
    write_cargo_toml(&dir, "0.1.0");

    release_prep(&dir, "0.1.0-alpha.1")
        .assert()
        .failure()
        .stderr(contains("CHANGELOG.md not found"));
}

#[test]
fn rejects_when_cargo_toml_lacks_top_level_version() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\n",
    )
    .unwrap();
    write_changelog(&dir, populated_changelog());

    release_prep(&dir, "0.1.0-alpha.1")
        .assert()
        .failure()
        .stderr(contains("no top-level 'version = ...' line"));
}

#[test]
fn missing_version_argument_is_rejected() {
    let dir = TempDir::new().unwrap();
    write_cargo_toml(&dir, "0.1.0");
    write_changelog(&dir, populated_changelog());

    Command::new("bash")
        .arg(script_path())
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(contains("VERSION argument is required"));
}

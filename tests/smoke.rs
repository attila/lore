use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn help_exits_successfully() {
    Command::cargo_bin("lore")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("lore"));
}

//! Smoke tests for the `md` binary's CLI surface (requires `--features tui`).

#![cfg(feature = "tui")]

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn md_version_reports_crate_version() {
    Command::cargo_bin("md")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn md_help_locks_cli_surface() {
    let assert = Command::cargo_bin("md").unwrap().arg("--help").assert();
    let out = assert.success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    for flag in [
        "--width",
        "--line-numbers",
        "--style",
        "--pager",
        "--tui",
        "[PATH]",
    ] {
        assert!(stdout.contains(flag), "missing {flag} in --help:\n{stdout}");
    }
}

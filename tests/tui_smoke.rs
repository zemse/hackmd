//! Smoke tests for the `hackmd` binary's TUI surface (requires
//! `--features tui`). The `md` binary was dropped for now (may return
//! later); the TUI ships behind `hackmd tui` only.

#![cfg(feature = "tui")]

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn hackmd_version_reports_crate_version() {
    Command::cargo_bin("hackmd")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn hackmd_help_lists_tui_subcommand() {
    Command::cargo_bin("hackmd")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("tui"));
}

#[test]
fn hackmd_tui_help_succeeds() {
    // `tui --help` exercises the subcommand's clap wiring without
    // touching the terminal.
    Command::cargo_bin("hackmd")
        .unwrap()
        .args(["tui", "--help"])
        .assert()
        .success();
}

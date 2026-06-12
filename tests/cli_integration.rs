//! End-to-end CLI tests: spawn the `hackmd` binary and assert exit codes /
//! stdout / stderr.
//!
//! For network-touching commands we point `--endpoint` at a wiremock server
//! and pass `--token test`, so no real HackMD account is required.

use std::process::Command;

use assert_cmd::prelude::*;
use predicates::prelude::*;
use tempfile::TempDir;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn bin() -> Command {
    Command::cargo_bin("hackmd").expect("hackmd binary")
}

#[test]
fn version_prints_semver() {
    bin()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("hackmd"));
}

#[test]
fn top_level_help_lists_all_subcommands() {
    bin()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("login"))
        .stdout(predicate::str::contains("logout"))
        .stdout(predicate::str::contains("whoami"))
        .stdout(predicate::str::contains("history"))
        .stdout(predicate::str::contains("export"))
        .stdout(predicate::str::contains("teams"))
        .stdout(predicate::str::contains("notes"))
        .stdout(predicate::str::contains("team-notes"))
        .stdout(predicate::str::contains("new"))
        .stdout(predicate::str::contains("tui"));
}

#[test]
fn each_subcommand_help_succeeds() {
    for sub in &[
        "login",
        "logout",
        "whoami",
        "history",
        "export",
        "teams",
        "notes",
        "team-notes",
        "new",
        "tui",
    ] {
        bin().args([sub, "--help"]).assert().success();
    }
}

#[test]
fn notes_subcommand_help_lists_actions() {
    bin()
        .args(["notes", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("get"))
        .stdout(predicate::str::contains("create"))
        .stdout(predicate::str::contains("update"))
        .stdout(predicate::str::contains("delete"));
}

#[test]
fn team_notes_subcommand_help_lists_actions() {
    bin()
        .args(["team-notes", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("create"))
        .stdout(predicate::str::contains("update"))
        .stdout(predicate::str::contains("delete"));
}

#[test]
fn unknown_command_fails() {
    bin()
        .arg("definitely-not-a-real-command")
        .assert()
        .failure();
}

#[test]
fn notes_update_missing_required_flag_fails() {
    // `--note-id` is required.
    bin()
        .args(["notes", "update"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("note-id"));
}

#[test]
fn notes_delete_missing_required_flag_fails() {
    bin()
        .args(["notes", "delete"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("note-id"));
}

#[test]
fn export_missing_required_flag_fails() {
    bin()
        .args(["export"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("note-id"));
}

#[test]
fn team_notes_update_missing_required_flag_fails() {
    bin()
        .args(["team-notes", "--team-path", "demo", "update"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("note-id"));
}

#[tokio::test]
async fn whoami_hits_mock_endpoint_and_renders_table() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me"))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "u-1",
            "email": "alice@example.com",
            "name": "Alice",
            "userPath": "alice",
            "photo": "p.png",
            "teams": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().expect("tmp");

    bin()
        .args([
            "--config-dir",
            dir.path().to_str().expect("path utf-8"),
            "--endpoint",
            &server.uri(),
            "--token",
            "test-token",
            "whoami",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("alice@example.com"))
        .stdout(predicate::str::contains("Alice"))
        .stdout(predicate::str::contains("u-1"));
}

#[tokio::test]
async fn teams_hits_mock_endpoint_and_renders_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/teams"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {
                "id": "team-1",
                "ownerId": "owner-1",
                "name": "Demo",
                "logo": "l.png",
                "path": "demo",
                "description": "",
                "hardBreaks": false,
                "visibility": "private",
                "createdAt": "2024-01-01T00:00:00.000Z"
            }
        ])))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().expect("tmp");

    bin()
        .args([
            "--config-dir",
            dir.path().to_str().expect("path utf-8"),
            "--endpoint",
            &server.uri(),
            "--token",
            "test-token",
            "teams",
            "--output",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"id\": \"team-1\""))
        .stdout(predicate::str::contains("\"path\": \"demo\""));
}

#[tokio::test]
async fn logout_clears_token_in_config() {
    let dir = TempDir::new().expect("tmp");

    // Pre-populate a config file with a token.
    let config_path = dir.path().join("config.json");
    std::fs::write(
        &config_path,
        r#"{"hackmdAPIEndpointURL":"https://example.test/v1","accessToken":"old-token"}"#,
    )
    .expect("write config");

    bin()
        .args([
            "--config-dir",
            dir.path().to_str().expect("path utf-8"),
            "logout",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("logged out"));

    let text = std::fs::read_to_string(&config_path).expect("read config");
    let v: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert!(
        v.get("accessToken").is_none() || v.get("accessToken") == Some(&serde_json::Value::Null),
        "expected accessToken cleared, got: {text}"
    );
}

/// When the binary is built without the `tui` feature (the default for
/// `cargo test`), the subcommand explains how to opt in.
#[cfg(not(feature = "tui"))]
#[tokio::test]
async fn tui_subcommand_explains_missing_feature() {
    bin()
        .args(["tui"])
        .assert()
        .success()
        .stdout(predicate::str::contains("built without the `tui` feature"));
}

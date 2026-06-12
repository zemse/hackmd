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
        .stdout(predicate::str::contains("folders"))
        .stdout(predicate::str::contains("team-folders"))
        .stdout(predicate::str::contains("new"))
        .stdout(predicate::str::contains("version"))
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
        "folders",
        "team-folders",
        "new",
        "version",
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

// ─── Original `hackmd-cli` compatibility ────────────────────────────────────

fn note_json(id: &str, title: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "title": title,
        "tags": [],
        "lastChangedAt": "2024-01-01T00:00:00.000Z",
        "createdAt": "2024-01-01T00:00:00.000Z",
        "lastChangeUser": null,
        "publishType": "view",
        "publishedAt": null,
        "userPath": "alice",
        "teamPath": null,
        "permalink": null,
        "shortId": "s1",
        "publishLink": format!("https://hackmd.io/{id}"),
        "readPermission": "owner",
        "writePermission": "owner"
    })
}

fn args_with_server(server: &MockServer, dir: &TempDir, rest: &[&str]) -> Vec<String> {
    let mut v = vec![
        "--config-dir".to_string(),
        dir.path().to_str().expect("path utf-8").to_string(),
        "--endpoint".to_string(),
        server.uri(),
        "--token".to_string(),
        "test-token".to_string(),
    ];
    v.extend(rest.iter().map(|s| s.to_string()));
    v
}

/// `hackmd notes` with no subcommand lists, exactly like the original CLI.
#[tokio::test]
async fn bare_notes_lists_like_original_cli() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/notes"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!([note_json("n1", "First note")])),
        )
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().expect("tmp");
    bin()
        .args(args_with_server(&server, &dir, &["notes"]))
        .assert()
        .success()
        .stdout(predicate::str::contains("First note"));
}

/// `hackmd notes --noteId=<id>` (camelCase, no subcommand) fetches one note.
#[tokio::test]
async fn notes_with_camelcase_note_id_fetches_single_note() {
    let server = MockServer::start().await;
    let mut single = note_json("n1", "Solo note");
    single["content"] = serde_json::json!("# Solo note");
    Mock::given(method("GET"))
        .and(path("/notes/n1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(single))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().expect("tmp");
    bin()
        .args(args_with_server(&server, &dir, &["notes", "--noteId=n1"]))
        .assert()
        .success()
        .stdout(predicate::str::contains("Solo note"));
}

/// `hackmd export --noteId=<id>` — camelCase alias on export.
#[tokio::test]
async fn export_accepts_camelcase_note_id() {
    let server = MockServer::start().await;
    let mut single = note_json("n9", "Exported");
    single["content"] = serde_json::json!("# Exported body");
    Mock::given(method("GET"))
        .and(path("/notes/n9"))
        .respond_with(ResponseTemplate::new(200).set_body_json(single))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().expect("tmp");
    bin()
        .args(args_with_server(&server, &dir, &["export", "--noteId=n9"]))
        .assert()
        .success()
        .stdout(predicate::str::contains("# Exported body"));
}

/// `notes create` passes tags and parentFolderId through to the API body.
#[tokio::test]
async fn notes_create_sends_tags_and_parent_folder() {
    use wiremock::matchers::body_partial_json;
    let server = MockServer::start().await;
    let mut created = note_json("n2", "Tagged");
    created["content"] = serde_json::json!("x");
    Mock::given(method("POST"))
        .and(path("/notes"))
        .and(body_partial_json(serde_json::json!({
            "tags": ["a", "b"],
            "parentFolderId": "f-1"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(created))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().expect("tmp");
    bin()
        .args(args_with_server(
            &server,
            &dir,
            &[
                "notes",
                "create",
                "--title=Tagged",
                "--content=x",
                "--tags=a,b",
                "--parentFolderId=f-1",
            ],
        ))
        .assert()
        .success()
        .stdout(predicate::str::contains("Tagged"));
}

/// `notes update` sends the full metadata PATCH the original CLI supports.
#[tokio::test]
async fn notes_update_sends_metadata_fields() {
    use wiremock::matchers::body_partial_json;
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/notes/n1"))
        .and(body_partial_json(serde_json::json!({
            "readPermission": "owner",
            "permalink": "my-link",
            "tags": ["t1"]
        })))
        .respond_with(ResponseTemplate::new(202))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().expect("tmp");
    bin()
        .args(args_with_server(
            &server,
            &dir,
            &[
                "notes",
                "update",
                "--noteId=n1",
                "--readPermission=owner",
                "--permalink=my-link",
                "--tags=t1",
            ],
        ))
        .assert()
        .success();
}

fn folder_json(id: &str, name: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "description": "docs",
        "icon": "1F600",
        "color": "#4F46E5",
        "parentFolderId": null,
        "createdAt": 1700000000000_i64,
        "updatedAt": 1700000001000_i64
    })
}

#[tokio::test]
async fn bare_folders_lists() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/folders"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!([folder_json("f1", "engineering")])),
        )
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().expect("tmp");
    bin()
        .args(args_with_server(&server, &dir, &["folders"]))
        .assert()
        .success()
        .stdout(predicate::str::contains("engineering"));
}

#[tokio::test]
async fn folders_create_posts_body_and_prints_row() {
    use wiremock::matchers::body_partial_json;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/folders"))
        .and(body_partial_json(serde_json::json!({
            "name": "docs",
            "parentFolderId": "f-root"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(folder_json("f2", "docs")))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().expect("tmp");
    bin()
        .args(args_with_server(
            &server,
            &dir,
            &[
                "folders",
                "create",
                "--name=docs",
                "--parentFolderId=f-root",
            ],
        ))
        .assert()
        .success()
        .stdout(predicate::str::contains("docs"));
}

#[tokio::test]
async fn team_folders_lists_with_camelcase_team_path() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/teams/demo/folders"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!([folder_json("f3", "team-docs")])),
        )
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().expect("tmp");
    bin()
        .args(args_with_server(
            &server,
            &dir,
            &["team-folders", "--teamPath=demo"],
        ))
        .assert()
        .success()
        .stdout(predicate::str::contains("team-docs"));
}

#[tokio::test]
async fn folders_order_prints_current_order_as_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/folders/folder-order"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"root": ["f1", "f2"]})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().expect("tmp");
    bin()
        .args(args_with_server(&server, &dir, &["folders", "order"]))
        .assert()
        .success()
        .stdout(predicate::str::contains("root"))
        .stdout(predicate::str::contains("f1"));
}

/// `team-notes --teamPath=<p>` with no subcommand lists, like the original.
#[tokio::test]
async fn bare_team_notes_lists_with_camelcase_team_path() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/teams/demo/notes"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!([note_json("tn1", "Team note")])),
        )
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().expect("tmp");
    bin()
        .args(args_with_server(
            &server,
            &dir,
            &["team-notes", "--teamPath=demo"],
        ))
        .assert()
        .success()
        .stdout(predicate::str::contains("Team note"));
}

/// `--csv` is a shorthand for `--output=csv`, like oclif's table flag.
#[tokio::test]
async fn csv_flag_outputs_csv() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/notes"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!([note_json("n1", "CSV note")])),
        )
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().expect("tmp");
    bin()
        .args(args_with_server(&server, &dir, &["notes", "--csv"]))
        .assert()
        .success()
        .stdout(predicate::str::contains("id,title"))
        .stdout(predicate::str::contains("n1,CSV note"));
}

#[test]
fn version_subcommand_and_short_v_flag_work() {
    bin()
        .args(["version"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hackmd"));
    bin()
        .args(["-v"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hackmd"));
}

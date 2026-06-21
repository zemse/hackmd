//! Integration tests for the TUI's async cloud bridge: a real tokio runtime
//! plus a wiremock server, asserting both the HTTP wire shapes (PATCH
//! bodies, routes) and the `CloudMsg` channel delivery.

#![cfg(feature = "tui")]

use std::time::Duration;

use hackmd::Client;
use hackmd::client::{ClientConfig, RetryConfig};
use hackmd::tui::cloud::{CloudContext, CloudMsg, FetchIntent, FetchedNote};
use hackmd::types::NotePermissionRole;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fast_client(uri: &str) -> Client {
    Client::with_config(
        "test-token",
        uri,
        ClientConfig {
            timeout: Duration::from_secs(5),
            retry: RetryConfig {
                max_retries: 0,
                base_delay: Duration::from_millis(1),
            },
        },
    )
    .expect("client builds")
}

fn ctx(uri: &str) -> CloudContext {
    CloudContext::with_client(fast_client(uri), tokio::runtime::Handle::current())
}

/// Poll the bridge channel the way the TUI tick does, with a test timeout.
async fn recv(ctx: &mut CloudContext) -> CloudMsg {
    for _ in 0..500 {
        if let Some(m) = ctx.try_recv() {
            return m;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no CloudMsg within 5s");
}

fn list_note_json(id: &str, title: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "title": title,
        "tags": [],
        "lastChangedAt": "2024-01-01T00:00:00.000Z",
        "createdAt": "2024-01-01T00:00:00.000Z",
        "lastChangeUser": null,
        "publishType": "view",
        "publishedAt": null,
        "userPath": null,
        "teamPath": null,
        "permalink": null,
        "shortId": "abc",
        "publishLink": format!("https://hackmd.io/{id}"),
        "readPermission": "owner",
        "writePermission": "owner"
    })
}

fn team_json(path: &str, name: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "team-1",
        "ownerId": "u1",
        "name": name,
        "logo": "logo.png",
        "path": path,
        "description": "",
        "hardBreaks": false,
        "visibility": "private",
        "createdAt": "2024-01-01T00:00:00.000Z"
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn save_patches_content_and_delivers_saved() {
    let server = MockServer::start().await;
    // Live API behavior: 202 Accepted, empty body.
    Mock::given(method("PATCH"))
        .and(path("/notes/n1"))
        .and(body_json(serde_json::json!({ "content": "new body" })))
        .respond_with(ResponseTemplate::new(202))
        .expect(1)
        .mount(&server)
        .await;

    let mut ctx = ctx(&server.uri());
    assert!(ctx.spawn_save("n1".into(), None, "new body".into(), None));

    match recv(&mut ctx).await {
        CloudMsg::Saved { id, result, .. } => {
            assert_eq!(id, "n1");
            let accepted = result.expect("save ok");
            assert_eq!(accepted, "new body");
        }
        other => panic!("expected Saved, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn team_save_uses_team_route() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/teams/demo/notes/n2"))
        .and(body_json(serde_json::json!({ "content": "team body" })))
        .respond_with(ResponseTemplate::new(202))
        .expect(1)
        .mount(&server)
        .await;

    let mut ctx = ctx(&server.uri());
    assert!(ctx.spawn_save("n2".into(), Some("demo".into()), "team body".into(), None));

    match recv(&mut ctx).await {
        CloudMsg::Saved { id, result, .. } => {
            assert_eq!(id, "n2");
            result.expect("team save ok");
        }
        other => panic!("expected Saved, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_lists_merges_own_and_team_notes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/notes"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!([list_note_json("n1", "Mine")])),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/teams"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!([team_json("demo", "Demo")])),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/teams/demo/notes"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!([list_note_json("n2", "Team note")])),
        )
        .expect(1)
        .mount(&server)
        .await;

    let mut ctx = ctx(&server.uri());
    assert!(ctx.spawn_fetch_lists());

    match recv(&mut ctx).await {
        CloudMsg::Lists(Ok(lists)) => {
            assert_eq!(lists.notes.len(), 1);
            assert_eq!(lists.notes[0].id, "n1");
            assert_eq!(lists.teams.len(), 1);
            assert_eq!(lists.teams[0].team.path, "demo");
            assert_eq!(lists.teams[0].notes.len(), 1);
            assert_eq!(lists.teams[0].notes[0].id, "n2");
        }
        other => panic!("expected Lists(Ok), got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn publish_patch_sends_read_permission_guest() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/notes/n1"))
        .and(body_json(serde_json::json!({ "readPermission": "guest" })))
        .respond_with(ResponseTemplate::new(202))
        .expect(1)
        .mount(&server)
        .await;

    let mut ctx = ctx(&server.uri());
    assert!(ctx.spawn_set_read_permission("n1".into(), None, NotePermissionRole::Guest));

    match recv(&mut ctx).await {
        CloudMsg::PermissionSet { id, result } => {
            assert_eq!(id, "n1");
            let perm = result.expect("publish ok");
            assert_eq!(perm, NotePermissionRole::Guest);
        }
        other => panic!("expected PermissionSet, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn revalidate_304_delivers_not_modified() {
    let server = MockServer::start().await;
    let etag = "W/\"abc\"";
    Mock::given(method("GET"))
        .and(path("/notes/n1"))
        .and(header("if-none-match", etag))
        .respond_with(ResponseTemplate::new(304).insert_header("etag", etag))
        .expect(1)
        .mount(&server)
        .await;

    let mut ctx = ctx(&server.uri());
    assert!(ctx.spawn_fetch_note(
        "n1".into(),
        FetchIntent::Revalidate {
            etag: etag.to_string(),
        },
    ));

    match recv(&mut ctx).await {
        CloudMsg::Note { id, result, .. } => {
            assert_eq!(id, "n1");
            assert!(matches!(result, Ok(FetchedNote::NotModified)));
        }
        other => panic!("expected Note, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_error_crosses_channel_as_string() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/notes/gone"))
        .respond_with(ResponseTemplate::new(404).set_body_string("{}"))
        .expect(1)
        .mount(&server)
        .await;

    let mut ctx = ctx(&server.uri());
    assert!(ctx.spawn_fetch_note("gone".into(), FetchIntent::OpenReader { scroll: 0 }));

    match recv(&mut ctx).await {
        CloudMsg::Note { id, result, .. } => {
            assert_eq!(id, "gone");
            assert!(result.is_err(), "404 must surface as Err(String)");
        }
        other => panic!("expected Note, got {other:?}"),
    }
}

#[test]
fn disconnected_context_spawns_nothing() {
    // No runtime at all — every spawn helper must decline instead of panic.
    let ctx = CloudContext::disconnected();
    assert!(!ctx.is_connected());
    assert!(!ctx.spawn_fetch_lists());
    assert!(!ctx.spawn_save("n1".into(), None, "x".into(), None));
    assert!(!ctx.spawn_fetch_note("n1".into(), FetchIntent::OpenReader { scroll: 0 }));
}

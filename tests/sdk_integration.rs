//! Cross-module SDK integration tests, all wiremock-driven (no live API).
//!
//! These are not unit tests of individual endpoints — those live alongside
//! the modules in `src/`. The point here is to verify that the typical user
//! flows compose correctly: create → update → read → delete chains, ETag
//! caching across calls, retry policy meeting rate-limit policy, folder
//! PATCH-with-null semantics.

use std::collections::HashMap;
use std::time::Duration;

use hackmd::{
    ApiFolderOrder, CachedResponse, Client, ClientConfig, CreateNoteOptions, CreateUserFolderBody,
    Error, RetryConfig, UpdateNoteOptions, UpdateUserFolderBody,
};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fast_config() -> ClientConfig {
    ClientConfig {
        timeout: Duration::from_secs(5),
        retry: RetryConfig {
            max_retries: 0,
            base_delay: Duration::from_millis(1),
        },
    }
}

fn note_json(id: &str, title: &str, content: &str, team_path: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "title": title,
        "tags": [],
        "lastChangedAt": "2024-01-01T00:00:00.000Z",
        "createdAt": "2024-01-01T00:00:00.000Z",
        "lastChangeUser": null,
        "publishType": "edit",
        "publishedAt": null,
        "userPath": "alice",
        "teamPath": team_path,
        "permalink": null,
        "shortId": id,
        "publishLink": format!("https://hackmd.io/{id}"),
        "readPermission": "owner",
        "writePermission": "owner",
        "content": content
    })
}

fn note_list_json(id: &str, title: &str, team_path: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "title": title,
        "tags": [],
        "lastChangedAt": "2024-01-01T00:00:00.000Z",
        "createdAt": "2024-01-01T00:00:00.000Z",
        "lastChangeUser": null,
        "publishType": "edit",
        "publishedAt": null,
        "userPath": "alice",
        "teamPath": team_path,
        "permalink": null,
        "shortId": id,
        "publishLink": format!("https://hackmd.io/{id}"),
        "readPermission": "owner",
        "writePermission": "owner"
    })
}

fn folder_json(id: &str, name: &str, description: Option<&str>, color: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "description": description,
        "icon": null,
        "color": color,
        "parentFolderId": null,
        "createdAt": 1700000000000_i64,
        "updatedAt": 1700000000000_i64,
    })
}

/// Full user-note lifecycle: create → update → fetch (cache primed) →
/// second fetch returns 304 → delete.
#[tokio::test]
async fn user_note_full_lifecycle_with_etag_cache() {
    let server = MockServer::start().await;
    let etag = "W/\"v2\"";

    // 1. CREATE
    Mock::given(method("POST"))
        .and(path("/notes"))
        .and(body_json(serde_json::json!({
            "title": "Hello",
            "content": "# v1"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(note_json("n1", "Hello", "# v1", None)))
        .expect(1)
        .mount(&server)
        .await;

    // 2. UPDATE content + title
    Mock::given(method("PATCH"))
        .and(path("/notes/n1"))
        .and(body_json(serde_json::json!({
            "content": "# v2",
            "title": "Hello v2"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(note_json("n1", "Hello v2", "# v2", None)))
        .expect(1)
        .mount(&server)
        .await;

    // 3. GET (no etag → 200 + etag header)
    Mock::given(method("GET"))
        .and(path("/notes/n1"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", etag)
                .set_body_json(note_json("n1", "Hello v2", "# v2", None)),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;

    // 4. GET with etag → 304
    Mock::given(method("GET"))
        .and(path("/notes/n1"))
        .and(header("if-none-match", etag))
        .respond_with(ResponseTemplate::new(304).insert_header("etag", etag))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    // 5. DELETE → returns the deleted note
    Mock::given(method("DELETE"))
        .and(path("/notes/n1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(note_json("n1", "Hello v2", "# v2", None)))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::with_config("t", server.uri(), fast_config()).expect("client");

    let created = client
        .create_note(CreateNoteOptions {
            title: Some("Hello".into()),
            content: Some("# v1".into()),
            ..Default::default()
        })
        .await
        .expect("create");
    assert_eq!(created.id, "n1");

    let updated = client
        .update_note(
            "n1",
            UpdateNoteOptions {
                content: Some("# v2".into()),
                title: Some("Hello v2".into()),
                ..Default::default()
            },
        )
        .await
        .expect("update");
    assert_eq!(updated.title, "Hello v2");

    let first = client.note("n1", None).await.expect("get fresh");
    let captured_etag = match first {
        CachedResponse::Modified { body, etag } => {
            assert_eq!(body.content, "# v2");
            etag
        }
        CachedResponse::NotModified => panic!("expected Modified, got NotModified"),
    };
    assert_eq!(captured_etag.as_deref(), Some(etag));

    let second = client
        .note("n1", captured_etag.as_deref())
        .await
        .expect("get cached");
    assert!(matches!(second, CachedResponse::NotModified));

    let deleted = client.delete_note("n1").await.expect("delete");
    assert_eq!(deleted.id, "n1");
}

/// Team-note lifecycle: create → update content → list shows it → delete.
#[tokio::test]
async fn team_note_flow_create_update_list_delete() {
    let server = MockServer::start().await;

    // CREATE
    Mock::given(method("POST"))
        .and(path("/teams/demo/notes"))
        .and(body_json(serde_json::json!({ "title": "Spec" })))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(note_json("tn1", "Spec", "", Some("demo"))),
        )
        .expect(1)
        .mount(&server)
        .await;

    // UPDATE content
    Mock::given(method("PATCH"))
        .and(path("/teams/demo/notes/tn1"))
        .and(body_json(serde_json::json!({ "content": "## body" })))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(note_json("tn1", "Spec", "## body", Some("demo"))),
        )
        .expect(1)
        .mount(&server)
        .await;

    // LIST
    Mock::given(method("GET"))
        .and(path("/teams/demo/notes"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!([note_list_json("tn1", "Spec", Some("demo"))])),
        )
        .expect(1)
        .mount(&server)
        .await;

    // DELETE
    Mock::given(method("DELETE"))
        .and(path("/teams/demo/notes/tn1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(note_json("tn1", "Spec", "## body", Some("demo"))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::with_config("t", server.uri(), fast_config()).expect("client");

    let created = client
        .create_team_note(
            "demo",
            CreateNoteOptions {
                title: Some("Spec".into()),
                ..Default::default()
            },
        )
        .await
        .expect("create team note");
    assert_eq!(created.id, "tn1");

    let updated = client
        .update_team_note_content("demo", "tn1", Some("## body".into()))
        .await
        .expect("update team note");
    assert_eq!(updated.content, "## body");

    let listed = client.team_notes("demo").await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "tn1");
    assert_eq!(listed[0].team_path.as_deref(), Some("demo"));

    let deleted = client.delete_team_note("demo", "tn1").await.expect("delete");
    assert_eq!(deleted.id, "tn1");
}

/// Folder flow: create → update (null one field via double-option, set
/// another) → list shows it → delete.
#[tokio::test]
async fn folder_flow_create_update_list_delete_with_double_option() {
    let server = MockServer::start().await;

    // CREATE
    Mock::given(method("POST"))
        .and(path("/folders"))
        .and(body_json(serde_json::json!({
            "name": "Research",
            "color": "#abcdef"
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(folder_json("f1", "Research", None, Some("#abcdef"))),
        )
        .expect(1)
        .mount(&server)
        .await;

    // UPDATE: clear color (Some(None)) and set description (Some(Some))
    Mock::given(method("PATCH"))
        .and(path("/folders/f1"))
        .and(body_json(serde_json::json!({
            "description": "now with words",
            "color": null
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(folder_json("f1", "Research", Some("now with words"), None)),
        )
        .expect(1)
        .mount(&server)
        .await;

    // LIST
    Mock::given(method("GET"))
        .and(path("/folders"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!([folder_json(
                "f1", "Research", Some("now with words"), None
            )])),
        )
        .expect(1)
        .mount(&server)
        .await;

    // DELETE (void body, 204)
    Mock::given(method("DELETE"))
        .and(path("/folders/f1"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::with_config("t", server.uri(), fast_config()).expect("client");

    let created = client
        .create_folder(CreateUserFolderBody {
            name: Some("Research".into()),
            color: Some("#abcdef".into()),
            ..Default::default()
        })
        .await
        .expect("create folder");
    assert_eq!(created.id, "f1");

    let updated = client
        .update_folder(
            "f1",
            UpdateUserFolderBody {
                // Note: `description` is Option<Option<String>> via double_option.
                description: Some(Some("now with words".into())),
                color: Some(None), // explicit null → clear
                ..Default::default()
            },
        )
        .await
        .expect("update folder");
    assert_eq!(updated.description.as_deref(), Some("now with words"));
    assert!(updated.color.is_none());

    let listed = client.folders().await.expect("list folders");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "f1");

    client.delete_folder("f1").await.expect("delete folder");
}

/// Retry-meets-rate-limit: server returns 429 with `x-ratelimit-userremaining: 0`.
/// Even with retries enabled, the client must stop after a single attempt
/// because remaining == 0.
#[tokio::test]
async fn retry_halts_immediately_when_rate_limit_remaining_zero() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::{Request, Respond};

    let server = MockServer::start().await;
    let counter = Arc::new(AtomicUsize::new(0));

    struct Counting(Arc<AtomicUsize>);
    impl Respond for Counting {
        fn respond(&self, _: &Request) -> ResponseTemplate {
            self.0.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(429)
                .insert_header("x-ratelimit-userlimit", "100")
                .insert_header("x-ratelimit-userremaining", "0")
                .insert_header("x-ratelimit-userreset", "1700000000")
                .set_body_string("{}")
        }
    }

    Mock::given(method("GET"))
        .and(path("/me"))
        .respond_with(Counting(counter.clone()))
        .mount(&server)
        .await;

    let cfg = ClientConfig {
        timeout: Duration::from_secs(5),
        retry: RetryConfig {
            max_retries: 3,
            base_delay: Duration::from_millis(10),
        },
    };
    let client = Client::with_config("t", server.uri(), cfg).expect("client");
    let err = client.me().await.expect_err("expected 429");
    match err {
        Error::RateLimit { user_remaining, .. } => assert_eq!(user_remaining, 0),
        other => panic!("expected RateLimit, got {other:?}"),
    }
    // Exactly one hit — retries with userremaining==0 are forbidden.
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

/// Folder order PUT round-trip — extends the lifecycle coverage to include
/// the somewhat unusual `folder-order` endpoint.
#[tokio::test]
async fn folder_order_get_then_put_round_trip() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/folders/folder-order"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "root": ["f1"]
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("PUT"))
        .and(path("/folders/folder-order"))
        .and(body_json(serde_json::json!({
            "order": { "root": ["f1", "f2"] }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(folder_json("f1", "Research", None, None)))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
    let order = client.folder_order().await.expect("get order");
    assert_eq!(order.get("root"), Some(&vec!["f1".to_string()]));

    let mut new_order: ApiFolderOrder = HashMap::new();
    new_order.insert("root".into(), vec!["f1".into(), "f2".into()]);
    let body = hackmd::UpdateFolderOrderBody { order: new_order };
    let returned = client.update_folder_order(body).await.expect("put order");
    assert_eq!(returned.id, "f1");
}

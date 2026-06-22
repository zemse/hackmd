//! User-scoped note endpoints.
//!
//! Ports from `_ref/api-client/nodejs/src/index.ts:181-214`.

use reqwest::Method;

use crate::api::UpdateContentBody;
use crate::client::{CachedResponse, Client};
use crate::error::Result;
use crate::types::{CreateNoteOptions, Note, SingleNote, UpdateNoteOptions};

impl Client {
    /// `GET /notes` — list all notes owned by the authenticated user.
    pub async fn notes(&self) -> Result<Vec<Note>> {
        self.request_json::<(), Vec<Note>>(Method::GET, "notes", None)
            .await
    }

    /// `GET /notes/{id}` — fetch a single note.
    ///
    /// Pass `etag = Some(previous_etag)` to opt into conditional-GET
    /// semantics: a 304 will surface as [`CachedResponse::NotModified`].
    pub async fn note(
        &self,
        note_id: &str,
        etag: Option<&str>,
    ) -> Result<CachedResponse<SingleNote>> {
        let path = format!("notes/{note_id}");
        self.request_with_etag::<SingleNote>(&path, etag).await
    }

    /// `POST /notes` — create a note from the given options.
    pub async fn create_note(&self, opts: CreateNoteOptions) -> Result<SingleNote> {
        self.request_json::<CreateNoteOptions, SingleNote>(Method::POST, "notes", Some(&opts))
            .await
    }

    /// `PATCH /notes/{id}` with body `{ content }` — shorthand for updating
    /// only the body of a note.
    ///
    /// The live API answers `202 Accepted` with an empty body, so this
    /// returns `None` on success; a deployment that echoes the updated note
    /// back surfaces as `Some`.
    pub async fn update_note_content(
        &self,
        note_id: &str,
        content: Option<String>,
    ) -> Result<Option<SingleNote>> {
        let path = format!("notes/{note_id}");
        let body = UpdateContentBody {
            content: content.as_deref(),
        };
        self.request_json_opt::<UpdateContentBody<'_>, SingleNote>(
            Method::PATCH,
            &path,
            Some(&body),
        )
        .await
    }

    /// `PATCH /notes/{id}` — update arbitrary note metadata + content.
    /// Same empty-body contract as [`Client::update_note_content`].
    pub async fn update_note(
        &self,
        note_id: &str,
        opts: UpdateNoteOptions,
    ) -> Result<Option<SingleNote>> {
        let path = format!("notes/{note_id}");
        self.request_json_opt::<UpdateNoteOptions, SingleNote>(Method::PATCH, &path, Some(&opts))
            .await
    }

    /// `DELETE /notes/{id}` — the live API answers `204 No Content`.
    pub async fn delete_note(&self, note_id: &str) -> Result<()> {
        let path = format!("notes/{note_id}");
        self.request_empty::<()>(Method::DELETE, &path, None).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Client;
    use crate::client::{ClientConfig, RetryConfig};
    use crate::types::{CommentPermissionType, NotePermissionRole};
    use std::time::Duration;
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

    fn note_json(id: &str, title: &str) -> serde_json::Value {
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
            "teamPath": null,
            "permalink": null,
            "shortId": "abc",
            "publishLink": "https://hackmd.io/abc",
            "readPermission": "owner",
            "writePermission": "owner"
        })
    }

    fn single_note_json(id: &str, content: &str) -> serde_json::Value {
        let mut v = note_json(id, "T");
        v["content"] = serde_json::Value::String(content.to_string());
        v
    }

    #[tokio::test]
    async fn notes_returns_list() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/notes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                note_json("n1", "T1"),
                note_json("n2", "T2")
            ])))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let notes = client.notes().await.expect("notes ok");
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].id, "n1");
        assert_eq!(notes[1].id, "n2");
    }

    #[tokio::test]
    async fn note_returns_single_note_without_etag() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/notes/n1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("etag", "W/\"v1\"")
                    .set_body_json(single_note_json("n1", "# hello")),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let resp = client.note("n1", None).await.expect("note ok");
        match resp {
            CachedResponse::Modified { body, etag } => {
                assert_eq!(body.id, "n1");
                assert_eq!(body.content, "# hello");
                assert_eq!(etag.as_deref(), Some("W/\"v1\""));
            }
            CachedResponse::NotModified => panic!("did not expect 304"),
        }
    }

    #[tokio::test]
    async fn note_etag_round_trip_200_then_304() {
        let server = MockServer::start().await;
        let etag = "W/\"abc\"";
        Mock::given(method("GET"))
            .and(path("/notes/n1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("etag", etag)
                    .set_body_json(single_note_json("n1", "body v1")),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/notes/n1"))
            .and(header("if-none-match", etag))
            .respond_with(ResponseTemplate::new(304).insert_header("etag", etag))
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");

        let first = client.note("n1", None).await.expect("first ok");
        let got_etag = match first {
            CachedResponse::Modified { body, etag } => {
                assert_eq!(body.content, "body v1");
                etag
            }
            CachedResponse::NotModified => panic!("first call should not be 304"),
        };
        assert_eq!(got_etag.as_deref(), Some(etag));

        let second = client
            .note("n1", got_etag.as_deref())
            .await
            .expect("second ok");
        assert!(matches!(second, CachedResponse::NotModified));
    }

    #[tokio::test]
    async fn create_note_posts_body_and_returns_single_note() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/notes"))
            .and(body_json(serde_json::json!({
                "title": "Hi",
                "content": "# Hi",
                "readPermission": "owner",
                "commentPermission": "signed_in_users"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(single_note_json("n1", "# Hi")))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let opts = CreateNoteOptions {
            title: Some("Hi".into()),
            content: Some("# Hi".into()),
            read_permission: Some(NotePermissionRole::Owner),
            comment_permission: Some(CommentPermissionType::SignedInUsers),
            ..Default::default()
        };
        let note = client.create_note(opts).await.expect("create ok");
        assert_eq!(note.id, "n1");
        assert_eq!(note.content, "# Hi");
    }

    // The live API answers PATCH with `202 Accepted` and an empty body.
    #[tokio::test]
    async fn update_note_content_sends_content_body_202_empty() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path("/notes/n1"))
            .and(body_json(serde_json::json!({ "content": "new body" })))
            .respond_with(ResponseTemplate::new(202))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let note = client
            .update_note_content("n1", Some("new body".into()))
            .await
            .expect("update ok");
        assert!(note.is_none(), "202 empty body → None");
    }

    // A deployment that echoes the entity back still parses (`Some`).
    #[tokio::test]
    async fn update_note_parses_body_when_present() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path("/notes/n1"))
            .and(body_json(serde_json::json!({
                "title": "T2",
                "tags": ["x"]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(single_note_json("n1", "body")))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let opts = UpdateNoteOptions {
            title: Some("T2".into()),
            tags: Some(vec!["x".into()]),
            ..Default::default()
        };
        let note = client.update_note("n1", opts).await.expect("update ok");
        assert_eq!(note.expect("body present").id, "n1");
    }

    // The live API answers DELETE with `204 No Content`.
    #[tokio::test]
    async fn delete_note_accepts_204_no_content() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/notes/n1"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        client.delete_note("n1").await.expect("delete ok");
    }
}

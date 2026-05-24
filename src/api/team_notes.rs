//! Team-scoped note endpoints.
//!
//! Ports from `_ref/api-client/nodejs/src/index.ts:216-234`.
//!
//! Upstream's team note update/delete methods return the raw `AxiosResponse`
//! instead of the unwrapped note (an inconsistency vs the user-scoped
//! counterparts). The Rust port makes these consistent: every method here
//! returns the same [`SingleNote`] / `Vec<Note>` shape as the user methods.

use reqwest::Method;
use serde::Serialize;

use crate::client::Client;
use crate::error::Result;
use crate::types::{CreateNoteOptions, Note, SingleNote, UpdateNoteOptions};

/// PATCH body for `update_team_note_content`. Identical wire shape to the
/// user-note variant — kept private to mirror the upstream surface.
#[derive(Debug, Serialize)]
struct UpdateContentBody<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<&'a str>,
}

impl Client {
    /// `GET /teams/{teamPath}/notes` — list notes owned by a team.
    pub async fn team_notes(&self, team_path: &str) -> Result<Vec<Note>> {
        let path = format!("teams/{team_path}/notes");
        self.request_json::<(), Vec<Note>>(Method::GET, &path, None)
            .await
    }

    /// `POST /teams/{teamPath}/notes` — create a team note.
    pub async fn create_team_note(
        &self,
        team_path: &str,
        opts: CreateNoteOptions,
    ) -> Result<SingleNote> {
        let path = format!("teams/{team_path}/notes");
        self.request_json::<CreateNoteOptions, SingleNote>(Method::POST, &path, Some(&opts))
            .await
    }

    /// `PATCH /teams/{teamPath}/notes/{noteId}` with body `{ content }`.
    // Diverges from upstream: returns parsed `SingleNote` (upstream returns raw response).
    pub async fn update_team_note_content(
        &self,
        team_path: &str,
        note_id: &str,
        content: Option<String>,
    ) -> Result<SingleNote> {
        let path = format!("teams/{team_path}/notes/{note_id}");
        let body = UpdateContentBody {
            content: content.as_deref(),
        };
        self.request_json::<UpdateContentBody<'_>, SingleNote>(Method::PATCH, &path, Some(&body))
            .await
    }

    /// `PATCH /teams/{teamPath}/notes/{noteId}` — update arbitrary metadata.
    // Diverges from upstream: returns parsed `SingleNote` (upstream returns raw response).
    pub async fn update_team_note(
        &self,
        team_path: &str,
        note_id: &str,
        opts: UpdateNoteOptions,
    ) -> Result<SingleNote> {
        let path = format!("teams/{team_path}/notes/{note_id}");
        self.request_json::<UpdateNoteOptions, SingleNote>(Method::PATCH, &path, Some(&opts))
            .await
    }

    /// `DELETE /teams/{teamPath}/notes/{noteId}` — returns the deleted note.
    // Diverges from upstream: returns parsed `SingleNote` (upstream returns raw response).
    pub async fn delete_team_note(&self, team_path: &str, note_id: &str) -> Result<SingleNote> {
        let path = format!("teams/{team_path}/notes/{note_id}");
        self.request_json::<(), SingleNote>(Method::DELETE, &path, None)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Client;
    use crate::client::{ClientConfig, RetryConfig};
    use std::time::Duration;
    use wiremock::matchers::{body_json, method, path};
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
            "userPath": null,
            "teamPath": "demo",
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
    async fn team_notes_returns_list() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/teams/demo/notes"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([note_json("tn1", "T1")])),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let notes = client.team_notes("demo").await.expect("team_notes ok");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].id, "tn1");
        assert_eq!(notes[0].team_path.as_deref(), Some("demo"));
    }

    #[tokio::test]
    async fn create_team_note_posts_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/teams/demo/notes"))
            .and(body_json(serde_json::json!({
                "title": "Hi",
                "content": "# Hi"
            })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(single_note_json("tn1", "# Hi")),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let opts = CreateNoteOptions {
            title: Some("Hi".into()),
            content: Some("# Hi".into()),
            ..Default::default()
        };
        let note = client
            .create_team_note("demo", opts)
            .await
            .expect("create ok");
        assert_eq!(note.id, "tn1");
    }

    #[tokio::test]
    async fn update_team_note_content_sends_content_body_and_returns_note() {
        // Verifies our divergence from upstream: we DO parse the response body
        // into a `SingleNote` instead of returning the raw response.
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path("/teams/demo/notes/tn1"))
            .and(body_json(serde_json::json!({ "content": "new" })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(single_note_json("tn1", "new")),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let note = client
            .update_team_note_content("demo", "tn1", Some("new".into()))
            .await
            .expect("update ok");
        assert_eq!(note.content, "new");
    }

    #[tokio::test]
    async fn update_team_note_sends_partial_body_and_returns_note() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path("/teams/demo/notes/tn1"))
            .and(body_json(serde_json::json!({ "title": "T2" })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(single_note_json("tn1", "body")),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let opts = UpdateNoteOptions {
            title: Some("T2".into()),
            ..Default::default()
        };
        let note = client
            .update_team_note("demo", "tn1", opts)
            .await
            .expect("update ok");
        assert_eq!(note.id, "tn1");
    }

    #[tokio::test]
    async fn delete_team_note_returns_deleted_note() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/teams/demo/notes/tn1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(single_note_json("tn1", "gone")),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let note = client
            .delete_team_note("demo", "tn1")
            .await
            .expect("delete ok");
        assert_eq!(note.id, "tn1");
    }
}

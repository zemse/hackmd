//! User-scoped endpoints: `me`, `history`.
//!
//! Ports `getMe` / `getHistory` from
//! `_ref/api-client/nodejs/src/index.ts:173-179`.

use reqwest::Method;

use crate::client::Client;
use crate::error::Result;
use crate::types::{Note, User};

impl Client {
    /// `GET /me` — the authenticated user profile.
    pub async fn me(&self) -> Result<User> {
        self.request_json::<(), User>(Method::GET, "me", None).await
    }

    /// `GET /history` — the authenticated user's note history.
    pub async fn history(&self) -> Result<Vec<Note>> {
        self.request_json::<(), Vec<Note>>(Method::GET, "history", None)
            .await
    }
}

#[cfg(test)]
mod tests {
    use crate::Client;
    use crate::client::{ClientConfig, RetryConfig};
    use std::time::Duration;
    use wiremock::matchers::{header, method, path};
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

    #[tokio::test]
    async fn me_returns_user() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me"))
            .and(header("authorization", "Bearer tok"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "u1",
                "email": "a@b.com",
                "name": "Alice",
                "userPath": "alice",
                "photo": "p.png",
                "teams": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("tok", server.uri(), fast_config()).expect("client");
        let me = client.me().await.expect("me ok");
        assert_eq!(me.id, "u1");
        assert_eq!(me.user_path, "alice");
    }

    #[tokio::test]
    async fn history_returns_notes() {
        let server = MockServer::start().await;
        let body = serde_json::json!([
            {
                "id": "n1",
                "title": "First",
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
            }
        ]);
        Mock::given(method("GET"))
            .and(path("/history"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("tok", server.uri(), fast_config()).expect("client");
        let history = client.history().await.expect("history ok");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id, "n1");
        assert_eq!(history[0].title, "First");
    }
}

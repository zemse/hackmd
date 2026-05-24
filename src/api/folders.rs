//! User-scoped folder endpoints.
//!
//! Ports from `_ref/api-client/nodejs/src/index.ts:236-263`.
//!
//! Note: some HackMD-compatible hosts (notably self-hosted CodiMD forks)
//! don't implement the folder API and respond `404`. The SDK surfaces that
//! transparently as [`Error::Http`](crate::Error::Http) — interpreting it as
//! "feature unavailable" is the CLI's job, not the SDK's.

use reqwest::Method;

use crate::client::Client;
use crate::error::Result;
use crate::types::{
    ApiFolder, ApiFolderOrder, CreateUserFolderBody, UpdateFolderOrderBody, UpdateUserFolderBody,
};

impl Client {
    /// `GET /folders` — list folders owned by the authenticated user.
    pub async fn folders(&self) -> Result<Vec<ApiFolder>> {
        self.request_json::<(), Vec<ApiFolder>>(Method::GET, "folders", None)
            .await
    }

    /// `POST /folders` — create a new user folder.
    pub async fn create_folder(&self, body: CreateUserFolderBody) -> Result<ApiFolder> {
        self.request_json::<CreateUserFolderBody, ApiFolder>(Method::POST, "folders", Some(&body))
            .await
    }

    /// `GET /folders/{id}` — fetch a single folder by id.
    pub async fn folder(&self, id: &str) -> Result<ApiFolder> {
        let path = format!("folders/{id}");
        self.request_json::<(), ApiFolder>(Method::GET, &path, None)
            .await
    }

    /// `PATCH /folders/{id}` — update folder metadata. Absent fields are
    /// untouched; fields set to `Some(None)` are explicitly cleared on the
    /// server side.
    pub async fn update_folder(&self, id: &str, body: UpdateUserFolderBody) -> Result<ApiFolder> {
        let path = format!("folders/{id}");
        self.request_json::<UpdateUserFolderBody, ApiFolder>(Method::PATCH, &path, Some(&body))
            .await
    }

    /// `DELETE /folders/{id}` — upstream contract is `Promise<void>`.
    pub async fn delete_folder(&self, id: &str) -> Result<()> {
        let path = format!("folders/{id}");
        self.request_empty::<()>(Method::DELETE, &path, None).await
    }

    /// `GET /folders/folder-order` — fetch the current folder ordering.
    pub async fn folder_order(&self) -> Result<ApiFolderOrder> {
        self.request_json::<(), ApiFolderOrder>(Method::GET, "folders/folder-order", None)
            .await
    }

    /// `PUT /folders/folder-order` — replace the folder ordering. Upstream
    /// returns an [`ApiFolder`] on success (slightly odd, but documented).
    pub async fn update_folder_order(&self, body: UpdateFolderOrderBody) -> Result<ApiFolder> {
        self.request_json::<UpdateFolderOrderBody, ApiFolder>(
            Method::PUT,
            "folders/folder-order",
            Some(&body),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Client;
    use crate::client::{ClientConfig, RetryConfig};
    use crate::error::Error;
    use std::collections::HashMap;
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

    fn folder_json(id: &str, name: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "name": name,
            "description": null,
            "icon": null,
            "color": null,
            "parentFolderId": null,
            "createdAt": 1700000000000_i64,
            "updatedAt": 1700000000000_i64,
        })
    }

    #[tokio::test]
    async fn folders_returns_list() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/folders"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                folder_json("f1", "Alpha"),
                folder_json("f2", "Beta"),
            ])))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let folders = client.folders().await.expect("folders ok");
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].id, "f1");
        assert_eq!(folders[1].name, "Beta");
    }

    #[tokio::test]
    async fn folders_surfaces_404_as_http_error() {
        // Some hosts don't support the folder API. We surface the raw 404 so the
        // CLI layer can decide how to phrase it — we do not try to translate it
        // here.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/folders"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let err = client.folders().await.expect_err("expected error");
        match err {
            Error::Http { status, message } => {
                assert_eq!(status, 404);
                assert_eq!(message, "not found");
            }
            other => panic!("expected Http 404, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn create_folder_posts_body_and_returns_folder() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/folders"))
            .and(body_json(serde_json::json!({
                "name": "Project X",
                "color": "#123456"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(folder_json("f1", "Project X")))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let body = CreateUserFolderBody {
            name: Some("Project X".into()),
            color: Some("#123456".into()),
            ..Default::default()
        };
        let folder = client.create_folder(body).await.expect("create ok");
        assert_eq!(folder.id, "f1");
        assert_eq!(folder.name, "Project X");
    }

    #[tokio::test]
    async fn folder_returns_single_folder() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/folders/f1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(folder_json("f1", "Alpha")))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let folder = client.folder("f1").await.expect("folder ok");
        assert_eq!(folder.id, "f1");
        assert_eq!(folder.name, "Alpha");
    }

    #[tokio::test]
    async fn update_folder_sends_only_explicit_fields() {
        let server = MockServer::start().await;
        // We expect exactly `{ "name": "New", "description": null }` — no
        // accidental nulls for the unspecified fields.
        Mock::given(method("PATCH"))
            .and(path("/folders/f1"))
            .and(body_json(serde_json::json!({
                "name": "New",
                "description": null
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(folder_json("f1", "New")))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let body = UpdateUserFolderBody {
            name: Some("New".into()),
            description: Some(None),
            ..Default::default()
        };
        let folder = client.update_folder("f1", body).await.expect("update ok");
        assert_eq!(folder.id, "f1");
        assert_eq!(folder.name, "New");
    }

    #[tokio::test]
    async fn delete_folder_returns_unit_on_2xx() {
        let server = MockServer::start().await;
        // Upstream returns 204 with no body for void responses; accept any 2xx.
        Mock::given(method("DELETE"))
            .and(path("/folders/f1"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        client.delete_folder("f1").await.expect("delete ok");
    }

    #[tokio::test]
    async fn folder_order_returns_map() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/folders/folder-order"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "root": ["f1", "f2"],
                "f1": ["f3"]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let order = client.folder_order().await.expect("order ok");
        assert_eq!(order.get("root"), Some(&vec!["f1".into(), "f2".into()]));
        assert_eq!(order.get("f1"), Some(&vec!["f3".into()]));
    }

    #[tokio::test]
    async fn update_folder_order_puts_body_and_returns_folder() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/folders/folder-order"))
            .and(body_json(serde_json::json!({
                "order": { "root": ["f1", "f2"] }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(folder_json("f1", "Alpha")))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let mut order: ApiFolderOrder = HashMap::new();
        order.insert("root".into(), vec!["f1".into(), "f2".into()]);
        let body = UpdateFolderOrderBody { order };
        let folder = client
            .update_folder_order(body)
            .await
            .expect("update order ok");
        // Upstream returns an ApiFolder here — somewhat odd, but documented.
        assert_eq!(folder.id, "f1");
    }
}

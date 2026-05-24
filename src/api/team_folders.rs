//! Team-scoped folder endpoints.
//!
//! Ports from `_ref/api-client/nodejs/src/index.ts:265-291`. Same surface as
//! the user-scoped variants in [`super::folders`], just prefixed by
//! `/teams/{teamPath}`.

use reqwest::Method;

use crate::client::Client;
use crate::error::Result;
use crate::types::{
    ApiFolder, ApiFolderOrder, CreateTeamFolderBody, UpdateFolderOrderBody, UpdateTeamFolderBody,
};

impl Client {
    /// `GET /teams/{teamPath}/folders` — list folders for a team workspace.
    pub async fn team_folders(&self, team_path: &str) -> Result<Vec<ApiFolder>> {
        let path = format!("teams/{team_path}/folders");
        self.request_json::<(), Vec<ApiFolder>>(Method::GET, &path, None)
            .await
    }

    /// `POST /teams/{teamPath}/folders` — create a folder in a team workspace.
    pub async fn create_team_folder(
        &self,
        team_path: &str,
        body: CreateTeamFolderBody,
    ) -> Result<ApiFolder> {
        let path = format!("teams/{team_path}/folders");
        self.request_json::<CreateTeamFolderBody, ApiFolder>(Method::POST, &path, Some(&body))
            .await
    }

    /// `GET /teams/{teamPath}/folders/{folderId}` — fetch a single team folder.
    pub async fn team_folder(&self, team_path: &str, folder_id: &str) -> Result<ApiFolder> {
        let path = format!("teams/{team_path}/folders/{folder_id}");
        self.request_json::<(), ApiFolder>(Method::GET, &path, None)
            .await
    }

    /// `PATCH /teams/{teamPath}/folders/{folderId}` — update team folder
    /// metadata. Same `Option<Option<T>>` semantics as [`Client::update_folder`].
    pub async fn update_team_folder(
        &self,
        team_path: &str,
        folder_id: &str,
        body: UpdateTeamFolderBody,
    ) -> Result<ApiFolder> {
        let path = format!("teams/{team_path}/folders/{folder_id}");
        self.request_json::<UpdateTeamFolderBody, ApiFolder>(Method::PATCH, &path, Some(&body))
            .await
    }

    /// `DELETE /teams/{teamPath}/folders/{folderId}` — upstream returns
    /// `Promise<void>`.
    pub async fn delete_team_folder(&self, team_path: &str, folder_id: &str) -> Result<()> {
        let path = format!("teams/{team_path}/folders/{folder_id}");
        self.request_empty::<()>(Method::DELETE, &path, None).await
    }

    /// `GET /teams/{teamPath}/folders/folder-order` — fetch a team's folder
    /// ordering.
    pub async fn team_folder_order(&self, team_path: &str) -> Result<ApiFolderOrder> {
        let path = format!("teams/{team_path}/folders/folder-order");
        self.request_json::<(), ApiFolderOrder>(Method::GET, &path, None)
            .await
    }

    /// `PUT /teams/{teamPath}/folders/folder-order` — replace a team's folder
    /// ordering. Upstream returns an [`ApiFolder`] on success.
    pub async fn update_team_folder_order(
        &self,
        team_path: &str,
        body: UpdateFolderOrderBody,
    ) -> Result<ApiFolder> {
        let path = format!("teams/{team_path}/folders/folder-order");
        self.request_json::<UpdateFolderOrderBody, ApiFolder>(Method::PUT, &path, Some(&body))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Client;
    use crate::client::{ClientConfig, RetryConfig};
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
    async fn team_folders_returns_list() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/teams/demo/folders"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([folder_json("tf1", "Specs")])),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let folders = client.team_folders("demo").await.expect("team_folders ok");
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].id, "tf1");
        assert_eq!(folders[0].name, "Specs");
    }

    #[tokio::test]
    async fn create_team_folder_posts_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/teams/demo/folders"))
            .and(body_json(serde_json::json!({ "name": "Specs" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(folder_json("tf1", "Specs")))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let body = CreateTeamFolderBody {
            name: Some("Specs".into()),
            ..Default::default()
        };
        let folder = client
            .create_team_folder("demo", body)
            .await
            .expect("create ok");
        assert_eq!(folder.id, "tf1");
    }

    #[tokio::test]
    async fn team_folder_returns_single_folder() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/teams/demo/folders/tf1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(folder_json("tf1", "Specs")))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let folder = client.team_folder("demo", "tf1").await.expect("ok");
        assert_eq!(folder.id, "tf1");
    }

    #[tokio::test]
    async fn update_team_folder_sends_partial_body() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path("/teams/demo/folders/tf1"))
            .and(body_json(serde_json::json!({ "icon": "rocket" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(folder_json("tf1", "Specs")))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let body = UpdateTeamFolderBody {
            icon: Some(Some("rocket".into())),
            ..Default::default()
        };
        let folder = client
            .update_team_folder("demo", "tf1", body)
            .await
            .expect("update ok");
        assert_eq!(folder.id, "tf1");
    }

    #[tokio::test]
    async fn delete_team_folder_returns_unit_on_2xx() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/teams/demo/folders/tf1"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        client
            .delete_team_folder("demo", "tf1")
            .await
            .expect("delete ok");
    }

    #[tokio::test]
    async fn team_folder_order_returns_map() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/teams/demo/folders/folder-order"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "root": ["tf1"]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let order = client.team_folder_order("demo").await.expect("order ok");
        assert_eq!(order.get("root"), Some(&vec!["tf1".into()]));
    }

    #[tokio::test]
    async fn update_team_folder_order_puts_body() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/teams/demo/folders/folder-order"))
            .and(body_json(serde_json::json!({
                "order": { "root": ["tf1", "tf2"] }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(folder_json("tf1", "Specs")))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let mut order: ApiFolderOrder = HashMap::new();
        order.insert("root".into(), vec!["tf1".into(), "tf2".into()]);
        let body = UpdateFolderOrderBody { order };
        let folder = client
            .update_team_folder_order("demo", body)
            .await
            .expect("update order ok");
        assert_eq!(folder.id, "tf1");
    }
}

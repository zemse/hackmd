//! Team listing endpoint.
//!
//! Ports `getTeams` from `_ref/api-client/nodejs/src/index.ts:212-214`.

use reqwest::Method;

use crate::client::Client;
use crate::error::Result;
use crate::types::Team;

impl Client {
    /// `GET /teams` — list teams the authenticated user belongs to.
    pub async fn teams(&self) -> Result<Vec<Team>> {
        self.request_json::<(), Vec<Team>>(Method::GET, "teams", None)
            .await
    }
}

#[cfg(test)]
mod tests {
    use crate::Client;
    use crate::client::{ClientConfig, RetryConfig};
    use crate::types::TeamVisibilityType;
    use std::time::Duration;
    use wiremock::matchers::{method, path};
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
    async fn teams_returns_list() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/teams"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "id": "team-1",
                    "ownerId": "u1",
                    "name": "Demo",
                    "logo": "logo.png",
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

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client");
        let teams = client.teams().await.expect("teams ok");
        assert_eq!(teams.len(), 1);
        assert_eq!(teams[0].path, "demo");
        assert_eq!(teams[0].visibility, TeamVisibilityType::Private);
    }
}

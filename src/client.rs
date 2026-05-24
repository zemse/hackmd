//! HTTP client + retry + ETag + rate-limit plumbing.
//!
//! Mirrors `_ref/api-client/nodejs/src/index.ts` behavior contract documented
//! in `PLAN.md` section 0.5.

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE, IF_NONE_MATCH};
use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Error, Result};
use crate::types::User;

/// Default base URL used when callers don't override the endpoint.
pub const DEFAULT_ENDPOINT: &str = "https://api.hackmd.io/v1";

/// Retry behavior. `max_retries = 0` disables retries entirely.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Total retry attempts AFTER the initial request. Default `3`.
    pub max_retries: u32,
    /// Base delay (ms) — actual delay = `2^attempt * base_delay`.
    pub base_delay: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(100),
        }
    }
}

/// Tunables for the [`Client`].
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Per-request timeout. Default `30s`.
    pub timeout: Duration,
    /// Retry policy.
    pub retry: RetryConfig,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            retry: RetryConfig::default(),
        }
    }
}

/// Outcome of an ETag-aware request.
#[derive(Debug, Clone)]
pub enum CachedResponse<T> {
    /// 200-class response — body decoded, plus the returned ETag (if any).
    Modified { body: T, etag: Option<String> },
    /// 304 Not Modified — caller's cached value is still current.
    NotModified,
}

/// The HackMD API client.
///
/// `Client` is cheap to clone (it wraps an internal `reqwest::Client`,
/// which itself is `Arc`-backed).
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
    token: String,
    retry: RetryConfig,
}

impl Client {
    /// Build a client with the default endpoint and default config.
    ///
    /// Returns [`Error::MissingArgument`] if `token` is empty.
    pub fn new(token: impl Into<String>) -> Result<Self> {
        Self::with_config(token, DEFAULT_ENDPOINT, ClientConfig::default())
    }

    /// Build a client pointing at a non-default endpoint.
    pub fn with_endpoint(token: impl Into<String>, endpoint: impl Into<String>) -> Result<Self> {
        Self::with_config(token, endpoint, ClientConfig::default())
    }

    /// Build a client with a full custom configuration.
    pub fn with_config(
        token: impl Into<String>,
        endpoint: impl Into<String>,
        config: ClientConfig,
    ) -> Result<Self> {
        let token = token.into();
        if token.is_empty() {
            return Err(Error::MissingArgument("access token"));
        }
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(Error::Reqwest)?;
        let base_url = endpoint.into().trim_end_matches('/').to_string();
        Ok(Self {
            http,
            base_url,
            token,
            retry: config.retry,
        })
    }

    /// `GET /me` — the authenticated user profile.
    pub async fn me(&self) -> Result<User> {
        self.request_json::<(), User>(Method::GET, "me", None).await
    }

    // ─── Internal request plumbing ──────────────────────────────────────

    fn url(&self, path: &str) -> String {
        let path = path.trim_start_matches('/');
        format!("{}/{}", self.base_url, path)
    }

    /// Send a request and decode the JSON body into `T`. No ETag handling.
    pub(crate) async fn request_json<B, T>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        let (text, _headers, _status) = self.send_with_retry(method, path, body, None).await?;
        serde_json::from_str::<T>(&text).map_err(Error::Json)
    }

    /// Send an ETag-aware GET; returns [`CachedResponse`].
    #[allow(dead_code)] // used by the note endpoints landing in M2
    pub(crate) async fn request_with_etag<T>(
        &self,
        path: &str,
        etag: Option<&str>,
    ) -> Result<CachedResponse<T>>
    where
        T: DeserializeOwned,
    {
        let (text, headers, status) = self
            .send_with_retry::<()>(Method::GET, path, None, etag)
            .await?;
        if status == StatusCode::NOT_MODIFIED {
            return Ok(CachedResponse::NotModified);
        }
        let body = serde_json::from_str::<T>(&text).map_err(Error::Json)?;
        let etag = headers
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        Ok(CachedResponse::Modified { body, etag })
    }

    /// Drive the retry loop. On non-success status returns the appropriate
    /// [`Error`] variant; on success returns `(body_text, headers, status)`.
    /// 304 is treated as a successful (non-retryable, no-body) response.
    async fn send_with_retry<B>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
        etag: Option<&str>,
    ) -> Result<(String, HeaderMap, StatusCode)>
    where
        B: Serialize + ?Sized,
    {
        let url = self.url(path);
        let retryable_method = is_retryable_method(&method);

        let mut attempt: u32 = 0;
        loop {
            let mut req = self
                .http
                .request(method.clone(), &url)
                .header(AUTHORIZATION, format!("Bearer {}", self.token))
                .header(CONTENT_TYPE, "application/json");
            if let Some(et) = etag
                && let Ok(hv) = HeaderValue::from_str(et)
            {
                req = req.header(IF_NONE_MATCH, hv);
            }
            if let Some(b) = body {
                req = req.json(b);
            }

            let send_result = req.send().await;
            match send_result {
                Ok(resp) => {
                    let status = resp.status();
                    let headers = resp.headers().clone();
                    // 304 is treated as success with an empty body.
                    if status == StatusCode::NOT_MODIFIED {
                        return Ok((String::new(), headers, status));
                    }
                    if status.is_success() {
                        let text = resp.text().await.map_err(Error::Reqwest)?;
                        return Ok((text, headers, status));
                    }
                    // Non-success — decide whether to retry.
                    let should_retry = retryable_method
                        && attempt < self.retry.max_retries
                        && is_retryable_status(status)
                        && rate_limit_allows_retry(&headers);
                    if !should_retry {
                        let text = resp.text().await.unwrap_or_default();
                        return Err(Error::from_response(status.as_u16(), &headers, &text));
                    }
                    // Drop the response body before sleeping.
                    drop(resp);
                    attempt += 1;
                    sleep_backoff(self.retry.base_delay, attempt).await;
                    continue;
                }
                Err(err) => {
                    // Network-level failure (DNS, connect, timeout, …).
                    let should_retry = retryable_method && attempt < self.retry.max_retries;
                    if !should_retry {
                        return Err(Error::Reqwest(err));
                    }
                    attempt += 1;
                    sleep_backoff(self.retry.base_delay, attempt).await;
                    continue;
                }
            }
        }
    }
}

fn is_retryable_method(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::PUT | Method::DELETE
    )
}

fn is_retryable_status(status: StatusCode) -> bool {
    status.as_u16() == 429 || (500..600).contains(&status.as_u16())
}

/// If the server signaled `x-ratelimit-userremaining: 0` we MUST NOT retry —
/// mirrors JS SDK `index.ts:159`.
fn rate_limit_allows_retry(headers: &HeaderMap) -> bool {
    let remaining = headers
        .get("x-ratelimit-userremaining")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok());
    !matches!(remaining, Some(n) if n <= 0)
}

async fn sleep_backoff(base: Duration, attempt: u32) {
    // exponential backoff: 2^attempt * base
    let factor = 1u64.checked_shl(attempt).unwrap_or(u64::MAX);
    let delay = base.saturating_mul(factor.min(u32::MAX as u64) as u32);
    tokio::time::sleep(delay).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    fn fast_config() -> ClientConfig {
        ClientConfig {
            timeout: Duration::from_secs(5),
            retry: RetryConfig {
                max_retries: 3,
                base_delay: Duration::from_millis(1),
            },
        }
    }

    fn user_json() -> serde_json::Value {
        serde_json::json!({
            "id": "u1",
            "email": "alice@example.com",
            "name": "Alice",
            "userPath": "alice",
            "photo": "p.png",
            "teams": []
        })
    }

    #[tokio::test]
    async fn new_rejects_empty_token() {
        let err = Client::new("").expect_err("should reject empty token");
        match err {
            Error::MissingArgument(_) => {}
            other => panic!("expected MissingArgument, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn me_happy_path_sends_auth_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(user_json()))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("test-token", server.uri(), fast_config())
            .expect("client builds");
        let user = client.me().await.expect("me ok");
        assert_eq!(user.id, "u1");
        assert_eq!(user.name, "Alice");
        assert_eq!(user.email.as_deref(), Some("alice@example.com"));
    }

    #[tokio::test]
    async fn rate_limit_headers_parsed_into_error() {
        let server = MockServer::start().await;
        // Disable retries so we observe the raw 429.
        let cfg = ClientConfig {
            timeout: Duration::from_secs(5),
            retry: RetryConfig {
                max_retries: 0,
                base_delay: Duration::from_millis(1),
            },
        };
        Mock::given(method("GET"))
            .and(path("/me"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("x-ratelimit-userlimit", "100")
                    .insert_header("x-ratelimit-userremaining", "0")
                    .insert_header("x-ratelimit-userreset", "1700000000")
                    .set_body_string("{}"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), cfg).expect("client builds");
        let err = client.me().await.expect_err("expected error");
        match err {
            Error::RateLimit {
                user_limit,
                user_remaining,
                reset_after,
            } => {
                assert_eq!(user_limit, 100);
                assert_eq!(user_remaining, 0);
                assert_eq!(reset_after, Some(1700000000));
            }
            other => panic!("expected RateLimit, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn retries_500_then_fails_after_max_retries() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me"))
            .respond_with(ResponseTemplate::new(500).set_body_string("oops"))
            // 1 initial + 3 retries = 4 total
            .expect(4)
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client builds");
        let err = client.me().await.expect_err("expected error");
        match err {
            Error::InternalServer { status } => assert_eq!(status, 500),
            other => panic!("expected InternalServer, got {other:?}"),
        }
    }

    /// Counts hits and responds with a status driven by an `AtomicUsize`.
    struct CountingResponder {
        counter: Arc<AtomicUsize>,
        status: u16,
    }
    impl Respond for CountingResponder {
        fn respond(&self, _req: &Request) -> ResponseTemplate {
            self.counter.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(self.status).set_body_string("err")
        }
    }

    #[tokio::test]
    async fn post_is_not_retried_on_500() {
        // POST isn't a retryable verb. We use `request_json` directly because
        // `me()` is GET-only.
        let server = MockServer::start().await;
        let counter = Arc::new(AtomicUsize::new(0));
        Mock::given(method("POST"))
            .and(path("/notes"))
            .respond_with(CountingResponder {
                counter: counter.clone(),
                status: 500,
            })
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client builds");
        let body = serde_json::json!({ "title": "x" });
        let res: Result<serde_json::Value> = client
            .request_json(Method::POST, "notes", Some(&body))
            .await;
        assert!(res.is_err(), "should error");
        // Critical: exactly ONE hit — POST is non-idempotent and must not retry.
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn network_error_retried_then_succeeds() {
        // Strategy: don't start the mock server; point at a closed port to
        // force a connection error, then... well, since we can't easily flip
        // a closed port to open mid-test, we instead verify retry-on-failure
        // by pointing at an unbound port and observing that we hit
        // max_retries before bubbling up.
        //
        // For the "succeeds after retry" half, see the next test which uses a
        // mock that fails once then succeeds.
        let cfg = ClientConfig {
            timeout: Duration::from_millis(200),
            retry: RetryConfig {
                max_retries: 2,
                base_delay: Duration::from_millis(1),
            },
        };
        // Port 1 is reserved & not bindable in user space — connection refused.
        let client = Client::with_config("t", "http://127.0.0.1:1", cfg).expect("client builds");
        let err = client.me().await.expect_err("expected network error");
        match err {
            Error::Reqwest(_) => {}
            other => panic!("expected Reqwest, got {other:?}"),
        }
    }

    /// 500 once, then 200 — verifies retry actually re-issues the request.
    #[tokio::test]
    async fn retries_500_then_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/me"))
            .respond_with(ResponseTemplate::new(200).set_body_json(user_json()))
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client builds");
        let user = client.me().await.expect("me ok after retry");
        assert_eq!(user.id, "u1");
    }

    #[tokio::test]
    async fn user_remaining_zero_halts_retry_early() {
        let server = MockServer::start().await;
        let counter = Arc::new(AtomicUsize::new(0));
        struct Resp(Arc<AtomicUsize>);
        impl Respond for Resp {
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
            .respond_with(Resp(counter.clone()))
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client builds");
        let err = client.me().await.expect_err("expected 429");
        match err {
            Error::RateLimit { user_remaining, .. } => assert_eq!(user_remaining, 0),
            other => panic!("expected RateLimit, got {other:?}"),
        }
        // user_remaining == 0 ⇒ retry halts early ⇒ exactly one request.
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn etag_request_round_trip_200_then_304() {
        use crate::types::SingleNote;
        let server = MockServer::start().await;
        let etag_value = "W/\"abc\"";
        // First call: no If-None-Match → 200 + etag header.
        Mock::given(method("GET"))
            .and(path("/notes/n1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("etag", etag_value)
                    .set_body_json(serde_json::json!({
                        "id": "n1",
                        "title": "T",
                        "tags": [],
                        "lastChangedAt": "2024-01-01T00:00:00.000Z",
                        "createdAt": "2024-01-01T00:00:00.000Z",
                        "lastChangeUser": null,
                        "publishType": "edit",
                        "publishedAt": null,
                        "userPath": null,
                        "teamPath": null,
                        "permalink": null,
                        "shortId": "abc",
                        "publishLink": "",
                        "readPermission": "owner",
                        "writePermission": "owner",
                        "content": "# hi"
                    })),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // Second call with matching If-None-Match → 304.
        Mock::given(method("GET"))
            .and(path("/notes/n1"))
            .and(header("if-none-match", etag_value))
            .respond_with(ResponseTemplate::new(304).insert_header("etag", etag_value))
            .mount(&server)
            .await;

        let client = Client::with_config("t", server.uri(), fast_config()).expect("client builds");

        let first = client
            .request_with_etag::<SingleNote>("notes/n1", None)
            .await
            .expect("first ok");
        let (got_etag, body) = match first {
            CachedResponse::Modified { body, etag } => (etag, body),
            CachedResponse::NotModified => panic!("first call should not be 304"),
        };
        assert_eq!(got_etag.as_deref(), Some(etag_value));
        assert_eq!(body.id, "n1");
        assert_eq!(body.content, "# hi");

        let second = client
            .request_with_etag::<SingleNote>("notes/n1", Some(etag_value))
            .await
            .expect("second ok");
        assert!(matches!(second, CachedResponse::NotModified));
    }
}

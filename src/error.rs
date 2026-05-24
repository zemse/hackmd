//! Error types for the HackMD SDK.

use reqwest::header::HeaderMap;

/// The result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// All errors the SDK surfaces.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// A required argument was empty or missing.
    #[error("missing required argument: {0}")]
    MissingArgument(&'static str),

    /// A 4xx (other than 429) response — body is captured for diagnostics.
    #[error("HTTP {status}: {message}")]
    Http { status: u16, message: String },

    /// A 5xx response from the server.
    #[error("server error (HTTP {status})")]
    InternalServer { status: u16 },

    /// HTTP 429 with parsed `x-ratelimit-user*` headers when available.
    #[error("rate limited (HTTP 429)")]
    RateLimit {
        user_limit: u32,
        user_remaining: u32,
        reset_after: Option<i64>,
    },

    /// Transport-level reqwest error (connection refused, DNS, TLS, etc.).
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),

    /// JSON serialization / deserialization error.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// CLI / config-level error (never produced from the SDK core itself).
    #[error("config error: {0}")]
    Config(String),

    /// Local I/O error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl Error {
    /// Map an HTTP response (status + headers + body) to the appropriate
    /// [`Error`] variant. Mirrors the JS SDK behavior:
    ///
    /// * `5xx`  → [`Error::InternalServer`]
    /// * `429`  → [`Error::RateLimit`] (parsed from `x-ratelimit-user*` headers)
    /// * other  → [`Error::Http`]
    pub fn from_response(status: u16, headers: &HeaderMap, body: &str) -> Error {
        if (500..600).contains(&status) {
            return Error::InternalServer { status };
        }
        if status == 429 {
            let user_limit = header_u32(headers, "x-ratelimit-userlimit").unwrap_or(0);
            let user_remaining = header_u32(headers, "x-ratelimit-userremaining").unwrap_or(0);
            let reset_after = header_i64(headers, "x-ratelimit-userreset");
            return Error::RateLimit {
                user_limit,
                user_remaining,
                reset_after,
            };
        }
        Error::Http {
            status,
            message: body.to_string(),
        }
    }
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn header_u32(headers: &HeaderMap, name: &str) -> Option<u32> {
    header_str(headers, name).and_then(|s| s.parse::<u32>().ok())
}

fn header_i64(headers: &HeaderMap, name: &str) -> Option<i64> {
    header_str(headers, name).and_then(|s| s.parse::<i64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};

    #[test]
    fn maps_5xx_to_internal_server() {
        let hm = HeaderMap::new();
        let err = Error::from_response(503, &hm, "boom");
        match err {
            Error::InternalServer { status } => assert_eq!(status, 503),
            other => panic!("expected InternalServer, got {other:?}"),
        }
    }

    #[test]
    fn maps_429_with_headers_to_rate_limit() {
        let mut hm = HeaderMap::new();
        hm.insert("x-ratelimit-userlimit", HeaderValue::from_static("100"));
        hm.insert("x-ratelimit-userremaining", HeaderValue::from_static("0"));
        hm.insert(
            "x-ratelimit-userreset",
            HeaderValue::from_static("1700000000"),
        );
        let err = Error::from_response(429, &hm, "{}");
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

    #[test]
    fn maps_429_with_missing_headers_to_rate_limit_with_zeros() {
        let hm = HeaderMap::new();
        let err = Error::from_response(429, &hm, "");
        match err {
            Error::RateLimit {
                user_limit,
                user_remaining,
                reset_after,
            } => {
                assert_eq!(user_limit, 0);
                assert_eq!(user_remaining, 0);
                assert!(reset_after.is_none());
            }
            other => panic!("expected RateLimit, got {other:?}"),
        }
    }

    #[test]
    fn maps_other_4xx_to_http() {
        let hm = HeaderMap::new();
        let err = Error::from_response(404, &hm, "not found");
        match err {
            Error::Http { status, message } => {
                assert_eq!(status, 404);
                assert_eq!(message, "not found");
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }
}

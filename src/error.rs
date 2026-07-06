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
    ///
    /// The message decodes `reset_after` (Unix seconds) into a "retry in …"
    /// hint and, when the reset is well past the 5-minute window, notes that
    /// the monthly quota is the likely cause rather than a short burst.
    #[error("{}", format_rate_limit(*user_limit, *user_remaining, *reset_after))]
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

/// Build the human-facing 429 message from the parsed `x-ratelimit-user*`
/// headers. `reset_after` is a Unix timestamp (seconds) for when the limit
/// clears; we render it both relatively ("retry in 4m 12s") and as a local
/// wall-clock time so the caller can tell the 5-minute window apart from the
/// monthly quota.
fn format_rate_limit(user_limit: u32, user_remaining: u32, reset_after: Option<i64>) -> String {
    use std::fmt::Write;

    let mut msg = String::from("rate limited (HTTP 429)");
    if user_limit > 0 {
        let _ = write!(
            msg,
            " — {user_remaining}/{user_limit} calls remaining in the current window"
        );
    }

    let Some(reset) = reset_after else {
        return msg;
    };

    let secs = reset - chrono::Utc::now().timestamp();
    if secs <= 0 {
        let _ = write!(msg, "; the limit should have reset already — retry now");
        return msg;
    }

    let _ = write!(msg, "; retry in {}", human_duration(secs));
    if let Some(dt) = chrono::DateTime::from_timestamp(reset, 0) {
        let _ = write!(
            msg,
            " (resets at {})",
            dt.with_timezone(&chrono::Local).format("%H:%M:%S")
        );
    }
    // The per-window limit is 100 calls / 5 min, so a reset far beyond that
    // window means the monthly quota is exhausted (2,000 on Free, 20,000 on
    // Prime), which needs a plan upgrade or the month rollover, not a wait.
    if secs > 15 * 60 {
        let _ = write!(
            msg,
            ". This is well past the 5-minute window, so the monthly quota is the likely cause (2,000 calls/month on Free, 20,000 on Prime)"
        );
    }
    msg
}

/// Render a positive duration in seconds as a compact "1d 2h 3m" / "4m 12s"
/// string. Seconds are shown only when the total is under an hour.
fn human_duration(secs: i64) -> String {
    let secs = secs.max(0);
    let (days, hours, mins, rem) = (
        secs / 86_400,
        (secs % 86_400) / 3600,
        (secs % 3600) / 60,
        secs % 60,
    );
    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if mins > 0 {
        parts.push(format!("{mins}m"));
    }
    if rem > 0 && days == 0 && hours == 0 {
        parts.push(format!("{rem}s"));
    }
    if parts.is_empty() {
        "moments".to_string()
    } else {
        parts.join(" ")
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
    fn human_duration_formats_compactly() {
        assert_eq!(human_duration(0), "moments");
        assert_eq!(human_duration(42), "42s");
        assert_eq!(human_duration(252), "4m 12s");
        assert_eq!(human_duration(3600), "1h");
        // Seconds are dropped once we're past an hour.
        assert_eq!(human_duration(90_061), "1d 1h 1m");
    }

    #[test]
    fn rate_limit_message_shows_window_reset() {
        // Reset ~5 minutes out ⇒ the 5-minute window, no monthly-quota note.
        let reset = chrono::Utc::now().timestamp() + 300;
        let err = Error::RateLimit {
            user_limit: 100,
            user_remaining: 0,
            reset_after: Some(reset),
        };
        let msg = err.to_string();
        assert!(msg.contains("0/100 calls remaining"), "got: {msg}");
        assert!(msg.contains("retry in"), "got: {msg}");
        assert!(msg.contains("resets at"), "got: {msg}");
        assert!(!msg.contains("monthly quota"), "got: {msg}");
    }

    #[test]
    fn rate_limit_message_flags_monthly_quota_for_distant_reset() {
        // Reset a day out ⇒ well past the window ⇒ flag the monthly quota.
        let reset = chrono::Utc::now().timestamp() + 86_400;
        let err = Error::RateLimit {
            user_limit: 100,
            user_remaining: 0,
            reset_after: Some(reset),
        };
        let msg = err.to_string();
        assert!(msg.contains("monthly quota"), "got: {msg}");
    }

    #[test]
    fn rate_limit_message_without_reset_is_bare() {
        let err = Error::RateLimit {
            user_limit: 0,
            user_remaining: 0,
            reset_after: None,
        };
        assert_eq!(err.to_string(), "rate limited (HTTP 429)");
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

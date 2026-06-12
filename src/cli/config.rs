//! CLI configuration — `~/.hackmd/config.json` + env overrides.
//!
//! Ports `_ref/hackmd-cli/src/{config,utils}.ts`.
//!
//! Precedence (highest to lowest):
//!   1. environment variables (`HMD_API_ACCESS_TOKEN`, `HMD_API_ENDPOINT_URL`)
//!   2. file contents
//!   3. built-in defaults (`hackmd::DEFAULT_ENDPOINT`)

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Environment variable for the access token. Mirrors upstream.
pub const ENV_ACCESS_TOKEN: &str = "HMD_API_ACCESS_TOKEN";
/// Environment variable for the API endpoint URL. Mirrors upstream.
pub const ENV_ENDPOINT_URL: &str = "HMD_API_ENDPOINT_URL";
/// Environment variable for the config directory. Mirrors upstream.
pub const ENV_CONFIG_DIR: &str = "HMD_CLI_CONFIG_DIR";

/// On-disk representation of the CLI config (`~/.hackmd/config.json`).
///
/// Mirrors the upstream Node CLI schema (camelCase wire keys). The file
/// is shared with `@hackmd/hackmd-cli` — both CLIs read and write the
/// same path, so `hackmd login` here is visible to the Node CLI and
/// vice versa.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Stored API endpoint (`hackmdAPIEndpointURL` on the wire).
    #[serde(
        default,
        rename = "hackmdAPIEndpointURL",
        skip_serializing_if = "Option::is_none"
    )]
    pub hackmd_api_endpoint_url: Option<String>,
    /// Stored access token (`accessToken` on the wire).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    /// Any other keys found in the file. The upstream CLI rewrites the
    /// whole JSON object on `login`/`logout`, preserving keys it doesn't
    /// know — carrying them here makes our load→save round-trip do the
    /// same instead of silently deleting them.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Resolve the config directory using the standard precedence.
///
/// `override_dir` takes priority (set from the `--config-dir` CLI flag);
/// otherwise `$HMD_CLI_CONFIG_DIR`; otherwise `~/.hackmd`.
pub fn resolve_config_dir(override_dir: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = override_dir {
        return Ok(p.to_path_buf());
    }
    if let Ok(dir) = std::env::var(ENV_CONFIG_DIR)
        && !dir.is_empty()
    {
        return Ok(PathBuf::from(dir));
    }
    let home = dirs::home_dir().ok_or_else(|| Error::Config("cannot find home dir".into()))?;
    Ok(home.join(".hackmd"))
}

/// Path of the config file under the given config directory.
pub fn config_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join("config.json")
}

impl Config {
    /// Load the config from `~/.hackmd/config.json` (or the override).
    /// Auto-creates the parent directory and file (with `{}`) if missing.
    pub fn load_from(config_dir: &Path) -> Result<Self> {
        let path = config_file_path(config_dir);
        if !path.exists() {
            // Auto-create parent + an empty config file. Mirrors upstream
            // `utils.ts:5-15` behavior.
            std::fs::create_dir_all(config_dir).map_err(Error::Io)?;
            std::fs::write(&path, "{}").map_err(Error::Io)?;
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(&path).map_err(Error::Io)?;
        if text.trim().is_empty() {
            return Ok(Config::default());
        }
        serde_json::from_str(&text).map_err(Error::Json)
    }

    /// Save the config back to disk at the given directory.
    pub fn save_to(&self, config_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(config_dir).map_err(Error::Io)?;
        let path = config_file_path(config_dir);
        let json = serde_json::to_string_pretty(self).map_err(Error::Json)?;
        std::fs::write(&path, json).map_err(Error::Io)?;
        Ok(())
    }
}

/// Effective settings after applying env overrides on top of the file config.
#[derive(Debug, Clone)]
pub struct Effective {
    /// Final endpoint used to build the [`Client`](crate::Client).
    pub endpoint: String,
    /// Final access token. `None` means "not logged in".
    pub token: Option<String>,
    /// Resolved config directory (where future writes go).
    pub config_dir: PathBuf,
}

/// Compute the effective endpoint/token after applying env + CLI overrides
/// on top of the on-disk file config.
///
/// CLI overrides win over env, env wins over file, file wins over the
/// built-in default endpoint.
pub fn effective(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
) -> Result<Effective> {
    let dir = resolve_config_dir(config_dir)?;
    let file = Config::load_from(&dir)?;

    let endpoint = cli_endpoint
        .map(|s| s.to_string())
        .or_else(|| {
            std::env::var(ENV_ENDPOINT_URL)
                .ok()
                .filter(|s| !s.is_empty())
        })
        .or(file.hackmd_api_endpoint_url)
        .unwrap_or_else(|| crate::client::DEFAULT_ENDPOINT.to_string());

    let token = cli_token
        .map(|s| s.to_string())
        .or_else(|| {
            std::env::var(ENV_ACCESS_TOKEN)
                .ok()
                .filter(|s| !s.is_empty())
        })
        .or(file.access_token)
        .filter(|s| !s.is_empty());

    Ok(Effective {
        endpoint,
        token,
        config_dir: dir,
    })
}

/// Convenience: load → mutate → save the `accessToken` field.
pub fn set_access_token(config_dir: &Path, token: &str) -> Result<()> {
    let mut cfg = Config::load_from(config_dir)?;
    if token.is_empty() {
        cfg.access_token = None;
    } else {
        cfg.access_token = Some(token.to_string());
    }
    cfg.save_to(config_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_creates_missing_file_with_empty_object() {
        let dir = TempDir::new().expect("tmp");
        let cfg = Config::load_from(dir.path()).expect("load");
        assert!(cfg.access_token.is_none());
        assert!(cfg.hackmd_api_endpoint_url.is_none());
        let path = config_file_path(dir.path());
        assert!(path.exists(), "config file should be created");
        let contents = std::fs::read_to_string(&path).expect("read");
        assert_eq!(contents, "{}");
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = TempDir::new().expect("tmp");
        let cfg = Config {
            hackmd_api_endpoint_url: Some("https://example.test/v1".into()),
            access_token: Some("tok-xyz".into()),
            extra: Default::default(),
        };
        cfg.save_to(dir.path()).expect("save");
        let back = Config::load_from(dir.path()).expect("load");
        assert_eq!(back.access_token.as_deref(), Some("tok-xyz"));
        assert_eq!(
            back.hackmd_api_endpoint_url.as_deref(),
            Some("https://example.test/v1")
        );
    }

    #[test]
    fn save_uses_camelcase_wire_keys() {
        let dir = TempDir::new().expect("tmp");
        let cfg = Config {
            hackmd_api_endpoint_url: Some("https://example.test/v1".into()),
            access_token: Some("tok".into()),
            extra: Default::default(),
        };
        cfg.save_to(dir.path()).expect("save");
        let raw = std::fs::read_to_string(config_file_path(dir.path())).expect("read");
        assert!(
            raw.contains("hackmdAPIEndpointURL"),
            "expected camelCase wire key: {raw}"
        );
        assert!(raw.contains("accessToken"), "expected camelCase: {raw}");
    }

    #[test]
    fn set_access_token_empty_clears_field() {
        let dir = TempDir::new().expect("tmp");
        set_access_token(dir.path(), "tok-1").expect("set");
        let cfg = Config::load_from(dir.path()).expect("load");
        assert_eq!(cfg.access_token.as_deref(), Some("tok-1"));

        set_access_token(dir.path(), "").expect("clear");
        let cfg = Config::load_from(dir.path()).expect("load");
        assert!(cfg.access_token.is_none(), "expected token cleared");
    }

    #[test]
    fn effective_cli_token_wins_over_file() {
        let dir = TempDir::new().expect("tmp");
        let cfg = Config {
            hackmd_api_endpoint_url: Some("https://file.test/v1".into()),
            access_token: Some("file-token".into()),
            extra: Default::default(),
        };
        cfg.save_to(dir.path()).expect("save");

        let eff = effective(
            Some(dir.path()),
            Some("https://cli.test/v1"),
            Some("cli-token"),
        )
        .expect("effective");
        assert_eq!(eff.endpoint, "https://cli.test/v1");
        assert_eq!(eff.token.as_deref(), Some("cli-token"));
    }

    #[test]
    fn effective_falls_back_to_default_endpoint() {
        let dir = TempDir::new().expect("tmp");
        // Don't write a config file at all — load_from will auto-create empty.
        let eff = effective(Some(dir.path()), None, None).expect("effective");
        assert_eq!(eff.endpoint, crate::client::DEFAULT_ENDPOINT);
        assert!(eff.token.is_none());
    }

    #[test]
    fn empty_file_is_treated_as_empty_config() {
        let dir = TempDir::new().expect("tmp");
        std::fs::write(config_file_path(dir.path()), "").expect("write empty");
        let cfg = Config::load_from(dir.path()).expect("load empty");
        assert!(cfg.access_token.is_none());
    }

    /// The Node CLI rewrites the whole config object on login/logout,
    /// preserving keys it doesn't know. Ours must do the same so the two
    /// CLIs can share `~/.hackmd/config.json` without clobbering each other.
    #[test]
    fn unknown_keys_survive_set_access_token() {
        let dir = TempDir::new().expect("tmp");
        std::fs::create_dir_all(dir.path()).expect("dir");
        std::fs::write(
            config_file_path(dir.path()),
            r#"{"accessToken":"old","hackmdAPIEndpointURL":"https://x.test/v1","futureKey":{"nested":true}}"#,
        )
        .expect("write");

        set_access_token(dir.path(), "new-token").expect("set");

        let raw = std::fs::read_to_string(config_file_path(dir.path())).expect("read");
        let v: serde_json::Value = serde_json::from_str(&raw).expect("json");
        assert_eq!(v["accessToken"], "new-token");
        assert_eq!(v["hackmdAPIEndpointURL"], "https://x.test/v1");
        assert_eq!(v["futureKey"]["nested"], true, "unknown key dropped: {raw}");
    }

    /// A config written by `@hackmd/hackmd-cli` reads cleanly, including
    /// its logout convention of `accessToken: ""` (treated as logged out).
    #[test]
    fn node_cli_written_config_interops() {
        let dir = TempDir::new().expect("tmp");
        std::fs::create_dir_all(dir.path()).expect("dir");
        std::fs::write(
            config_file_path(dir.path()),
            "{\n  \"accessToken\": \"\",\n  \"hackmdAPIEndpointURL\": \"https://ee.test/v1\"\n}",
        )
        .expect("write");

        let eff = effective(Some(dir.path()), None, None).expect("effective");
        assert_eq!(eff.endpoint, "https://ee.test/v1");
        assert!(
            eff.token.is_none(),
            "empty accessToken (Node CLI logout) must read as logged out"
        );
    }
}

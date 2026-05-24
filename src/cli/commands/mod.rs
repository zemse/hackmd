//! CLI command implementations.

pub mod auth;
pub mod export;
pub mod history;
pub mod notes;
pub mod team_notes;
pub mod teams;
pub mod tui;

use std::path::Path;

use crate::Client;
use crate::cli::config::{Effective, effective};
use crate::error::{Error, Result};

/// Resolve effective config + build an authenticated [`Client`].
///
/// Returns a friendly error when no token is configured anywhere.
pub fn build_client(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
) -> Result<(Client, Effective)> {
    let eff = effective(config_dir, cli_endpoint, cli_token)?;
    let token = eff.token.clone().ok_or_else(|| {
        Error::Config(
            "no access token configured — run `hackmd login` or set HMD_API_ACCESS_TOKEN".into(),
        )
    })?;
    let client = Client::with_endpoint(token, eff.endpoint.clone())?;
    Ok((client, eff))
}

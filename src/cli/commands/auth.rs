//! `login`, `logout`, `whoami`.
//!
//! Ports `_ref/hackmd-cli/src/commands/{login,logout,whoami}.ts`.

use std::path::Path;

use crate::Client;
use crate::cli::config::{self, effective};
use crate::cli::output::{OutputOpts, print_table};
use crate::error::{Error, Result};

/// Default columns for `whoami` (matches upstream column set).
const WHOAMI_COLUMNS: &[&str] = &["id", "email", "name", "userPath"];

/// `hackmd login` — prompt for a token, validate via `me()`, persist it.
pub async fn login(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
) -> Result<()> {
    let eff = effective(config_dir, cli_endpoint, cli_token)?;

    // Already logged in? Validate, short-circuit.
    if let Some(token) = eff.token.clone() {
        let client = Client::with_endpoint(token, eff.endpoint.clone())?;
        match client.me().await {
            Ok(user) => {
                println!("Already logged in as {} ({})", user.name, user.user_path);
                return Ok(());
            }
            Err(_) => {
                eprintln!("Stored credentials are invalid — please re-enter your token.");
            }
        }
    }

    let token = rpassword::prompt_password("Enter your HackMD access token: ")
        .map_err(|e| Error::Config(format!("could not read token: {e}")))?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(Error::Config("empty token".into()));
    }

    let client = Client::with_endpoint(token.clone(), eff.endpoint.clone())?;
    match client.me().await {
        Ok(_) => {
            config::set_access_token(&eff.config_dir, &token)?;
            println!("Login successfully");
            Ok(())
        }
        Err(e) => {
            eprintln!("Login failed");
            Err(e)
        }
    }
}

/// `hackmd logout` — clear the stored token (file stays in place).
pub fn logout(config_dir: Option<&Path>) -> Result<()> {
    let dir = config::resolve_config_dir(config_dir)?;
    config::set_access_token(&dir, "")?;
    println!("You've logged out successfully");
    Ok(())
}

/// `hackmd whoami` — render the authenticated user as a table.
pub async fn whoami(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    opts: &OutputOpts,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    let user = client.me().await?;
    print_table(&[user], WHOAMI_COLUMNS, opts)
}

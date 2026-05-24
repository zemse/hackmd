//! `hackmd tui` — entry point.
//!
//! When compiled with `--features tui`, hands off to [`crate::tui::run`].
//! Otherwise prints a one-liner telling the user to reinstall with the
//! feature enabled.

use std::path::Path;

use crate::error::Result;

#[cfg(feature = "tui")]
pub async fn run(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    crate::tui::run(client).await
}

#[cfg(not(feature = "tui"))]
pub async fn run(
    _config_dir: Option<&Path>,
    _cli_endpoint: Option<&str>,
    _cli_token: Option<&str>,
) -> Result<()> {
    println!(
        "this binary was built without the `tui` feature. \
         Reinstall with `cargo install hackmd --features tui` to enable."
    );
    Ok(())
}

//! `hackmd teams` — list teams the user belongs to.

use std::path::Path;

use crate::cli::output::{OutputOpts, print_table};
use crate::error::Result;

const TEAMS_COLUMNS: &[&str] = &["id", "name", "path", "ownerId"];

pub async fn run(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    opts: &OutputOpts,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    let teams = client.teams().await?;
    print_table(&teams, TEAMS_COLUMNS, opts)
}

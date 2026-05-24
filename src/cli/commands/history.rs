//! `hackmd history` — list the user's browse history.

use std::path::Path;

use crate::cli::output::{OutputOpts, print_table};
use crate::error::Result;

const HISTORY_COLUMNS: &[&str] = &["id", "title", "userPath", "teamPath"];

pub async fn run(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    opts: &OutputOpts,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    let history = client.history().await?;
    print_table(&history, HISTORY_COLUMNS, opts)
}

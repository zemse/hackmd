//! `hackmd team-folders {,create,update,delete,order}`.

use std::path::Path;

use crate::cli::commands::folders::{FOLDER_COLUMNS, parse_order};
use crate::cli::output::{OutputOpts, print_table};
use crate::error::{Error, Result};
use crate::types::{CreateTeamFolderBody, UpdateFolderOrderBody, UpdateTeamFolderBody};

pub async fn list(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    team_path: &str,
    opts: &OutputOpts,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    let folders = client.team_folders(team_path).await?;
    print_table(&folders, FOLDER_COLUMNS, opts)
}

pub async fn get(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    team_path: &str,
    folder_id: &str,
    opts: &OutputOpts,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    let folder = client.team_folder(team_path, folder_id).await?;
    print_table(&[folder], FOLDER_COLUMNS, opts)
}

pub async fn create(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    team_path: &str,
    body: CreateTeamFolderBody,
    opts: &OutputOpts,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    let folder = client.create_team_folder(team_path, body).await?;
    print_table(&[folder], FOLDER_COLUMNS, opts)
}

pub async fn update(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    team_path: &str,
    folder_id: &str,
    body: UpdateTeamFolderBody,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    client
        .update_team_folder(team_path, folder_id, body)
        .await?;
    Ok(())
}

pub async fn delete(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    team_path: &str,
    folder_id: &str,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    client.delete_team_folder(team_path, folder_id).await?;
    Ok(())
}

/// `hackmd team-folders order` — print the current order as JSON; with
/// `--order '<json>'`, replace it.
pub async fn order(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    team_path: &str,
    order: Option<String>,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    match order {
        None => {
            let current = client.team_folder_order(team_path).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&current).map_err(Error::Json)?
            );
        }
        Some(json) => {
            let order = parse_order(&json)?;
            client
                .update_team_folder_order(team_path, UpdateFolderOrderBody { order })
                .await?;
        }
    }
    Ok(())
}

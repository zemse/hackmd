//! `hackmd folders {,create,update,delete,order}`.

use std::path::Path;

use crate::cli::output::{OutputOpts, print_table};
use crate::error::{Error, Result};
use crate::types::{
    ApiFolderOrder, CreateUserFolderBody, UpdateFolderOrderBody, UpdateUserFolderBody,
};

pub(crate) const FOLDER_COLUMNS: &[&str] = &[
    "id",
    "name",
    "parentFolderId",
    "color",
    "description",
    "icon",
];

pub async fn list(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    opts: &OutputOpts,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    let folders = client.folders().await?;
    print_table(&folders, FOLDER_COLUMNS, opts)
}

pub async fn get(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    folder_id: &str,
    opts: &OutputOpts,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    let folder = client.folder(folder_id).await?;
    print_table(&[folder], FOLDER_COLUMNS, opts)
}

pub async fn create(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    body: CreateUserFolderBody,
    opts: &OutputOpts,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    let folder = client.create_folder(body).await?;
    print_table(&[folder], FOLDER_COLUMNS, opts)
}

pub async fn update(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    folder_id: &str,
    body: UpdateUserFolderBody,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    client.update_folder(folder_id, body).await?;
    Ok(())
}

pub async fn delete(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    folder_id: &str,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    client.delete_folder(folder_id).await?;
    Ok(())
}

/// `hackmd folders order` — print the current order as JSON; with
/// `--order '<json>'`, replace it.
pub async fn order(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    order: Option<String>,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    match order {
        None => {
            let current = client.folder_order().await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&current).map_err(Error::Json)?
            );
        }
        Some(json) => {
            let order = parse_order(&json)?;
            client
                .update_folder_order(UpdateFolderOrderBody { order })
                .await?;
        }
    }
    Ok(())
}

/// Parse the `--order` JSON argument. Shared with `team-folders order`.
pub(crate) fn parse_order(json: &str) -> Result<ApiFolderOrder> {
    serde_json::from_str(json).map_err(|e| {
        Error::Config(format!(
            "--order must be JSON like {{\"root\":[\"id\"]}}: {e}"
        ))
    })
}

//! `hackmd team-notes {list, create, update, delete}`.

use std::path::Path;

use crate::cli::commands::notes::{NOTES_CREATE_COLUMNS, NOTES_LIST_COLUMNS, resolve_content};
use crate::cli::output::{OutputOpts, print_table};
use crate::error::Result;
use crate::types::{CreateNoteOptions, UpdateNoteOptions};

pub async fn list(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    team_path: &str,
    opts: &OutputOpts,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    let notes = client.team_notes(team_path).await?;
    let rows = super::note_rows(&notes)?;
    print_table(&rows, NOTES_LIST_COLUMNS, opts)
}

pub async fn create(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    team_path: &str,
    mut payload: CreateNoteOptions,
    use_editor: bool,
    opts: &OutputOpts,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    payload.content = resolve_content(payload.content, use_editor)?;
    let note = client.create_team_note(team_path, payload).await?;
    print_table(&[note], NOTES_CREATE_COLUMNS, opts)
}

pub async fn update(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    team_path: &str,
    note_id: &str,
    opts: UpdateNoteOptions,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    client.update_team_note(team_path, note_id, opts).await?;
    Ok(())
}

pub async fn delete(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    team_path: &str,
    note_id: &str,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    client.delete_team_note(team_path, note_id).await?;
    Ok(())
}

//! `hackmd team-notes {list, create, update, delete}`.

use std::io::{IsTerminal, Read};
use std::path::Path;

use crate::cli::editor::open_in_editor;
use crate::cli::output::{OutputOpts, print_table};
use crate::error::{Error, Result};
use crate::types::{CommentPermissionType, CreateNoteOptions, NotePermissionRole};

const TEAM_NOTES_LIST_COLUMNS: &[&str] = &["id", "title", "userPath", "teamPath", "lastChangedAt"];
const TEAM_NOTES_CREATE_COLUMNS: &[&str] = &["id", "title", "userPath", "teamPath"];

pub async fn list(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    team_path: &str,
    opts: &OutputOpts,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    let notes = client.team_notes(team_path).await?;
    print_table(&notes, TEAM_NOTES_LIST_COLUMNS, opts)
}

#[allow(clippy::too_many_arguments)]
pub async fn create(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    team_path: &str,
    title: Option<String>,
    content: Option<String>,
    read_permission: Option<NotePermissionRole>,
    write_permission: Option<NotePermissionRole>,
    comment_permission: Option<CommentPermissionType>,
    use_editor: bool,
    opts: &OutputOpts,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;

    let final_content = if use_editor {
        Some(open_in_editor()?)
    } else if !std::io::stdin().is_terminal() {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf).map_err(Error::Io)?;
        if buf.is_empty() { content } else { Some(buf) }
    } else {
        content
    };

    let payload = CreateNoteOptions {
        title,
        content: final_content,
        read_permission,
        write_permission,
        comment_permission,
        ..Default::default()
    };
    let note = client.create_team_note(team_path, payload).await?;
    print_table(&[note], TEAM_NOTES_CREATE_COLUMNS, opts)
}

pub async fn update(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    team_path: &str,
    note_id: &str,
    content: Option<String>,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    client
        .update_team_note_content(team_path, note_id, content)
        .await?;
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

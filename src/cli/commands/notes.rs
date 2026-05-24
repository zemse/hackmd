//! `hackmd notes {list, get, create, update, delete}`.

use std::io::{IsTerminal, Read};
use std::path::Path;

use crate::CachedResponse;
use crate::cli::editor::open_in_editor;
use crate::cli::output::{OutputOpts, print_table};
use crate::error::{Error, Result};
use crate::types::{CommentPermissionType, CreateNoteOptions, NotePermissionRole};

const NOTES_LIST_COLUMNS: &[&str] = &["id", "title", "userPath", "teamPath", "lastChangedAt"];
const NOTES_GET_COLUMNS: &[&str] = &[
    "id",
    "title",
    "userPath",
    "teamPath",
    "readPermission",
    "writePermission",
];
const NOTES_CREATE_COLUMNS: &[&str] = &["id", "title", "userPath", "teamPath"];

pub async fn list(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    opts: &OutputOpts,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    let notes = client.notes().await?;
    print_table(&notes, NOTES_LIST_COLUMNS, opts)
}

pub async fn get(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    note_id: &str,
    opts: &OutputOpts,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    let resp = client.note(note_id, None).await?;
    let note = match resp {
        CachedResponse::Modified { body, .. } => body,
        CachedResponse::NotModified => {
            // Unreachable in practice — we never send If-None-Match here.
            return Err(Error::Config("unexpected 304 from server".into()));
        }
    };
    print_table(&[note], NOTES_GET_COLUMNS, opts)
}

#[allow(clippy::too_many_arguments)]
pub async fn create(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    title: Option<String>,
    content: Option<String>,
    read_permission: Option<NotePermissionRole>,
    write_permission: Option<NotePermissionRole>,
    comment_permission: Option<CommentPermissionType>,
    use_editor: bool,
    opts: &OutputOpts,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;

    // Content precedence: --editor > stdin (if piped) > --content.
    let final_content = if use_editor {
        Some(open_in_editor()?)
    } else if !std::io::stdin().is_terminal() {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(Error::Io)?;
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
    let note = client.create_note(payload).await?;
    print_table(&[note], NOTES_CREATE_COLUMNS, opts)
}

pub async fn update(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    note_id: &str,
    content: Option<String>,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    client.update_note_content(note_id, content).await?;
    Ok(())
}

pub async fn delete(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    note_id: &str,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    client.delete_note(note_id).await?;
    Ok(())
}

//! `hackmd notes {list, get, create, update, delete}`.

use std::io::{IsTerminal, Read};
use std::path::Path;

use crate::CachedResponse;
use crate::cli::editor::open_in_editor;
use crate::cli::output::{OutputOpts, print_table};
use crate::error::{Error, Result};
use crate::types::{CreateNoteOptions, UpdateNoteOptions};

/// Shared with `team_notes` — the list/create column sets are identical for
/// user and team notes, so there's one source of truth.
pub(crate) const NOTES_LIST_COLUMNS: &[&str] =
    &["id", "title", "owner", "visibility", "lastChangedAt"];
const NOTES_GET_COLUMNS: &[&str] = &[
    "id",
    "title",
    "userPath",
    "teamPath",
    "readPermission",
    "writePermission",
];
pub(crate) const NOTES_CREATE_COLUMNS: &[&str] = &["id", "title", "userPath", "teamPath"];

pub async fn list(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    opts: &OutputOpts,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    let notes = client.notes().await?;
    let rows = super::note_rows(&notes)?;
    print_table(&rows, NOTES_LIST_COLUMNS, opts)
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

pub async fn create(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    mut payload: CreateNoteOptions,
    use_editor: bool,
    opts: &OutputOpts,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    payload.content = resolve_content(payload.content, use_editor)?;
    let note = client.create_note(payload).await?;
    print_table(&[note], NOTES_CREATE_COLUMNS, opts)
}

/// Content precedence: `--editor` > stdin (if piped) > `--content`.
/// Shared with `team-notes create`.
pub(crate) fn resolve_content(content: Option<String>, use_editor: bool) -> Result<Option<String>> {
    if use_editor {
        return Ok(Some(open_in_editor()?));
    }
    if !std::io::stdin().is_terminal() {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(Error::Io)?;
        if !buf.is_empty() {
            return Ok(Some(buf));
        }
    }
    Ok(content)
}

pub async fn update(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    note_id: &str,
    opts: UpdateNoteOptions,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    client.update_note(note_id, opts).await?;
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

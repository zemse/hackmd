//! `hackmd export --note-id <id>` — dump raw note content to stdout.

use std::path::Path;

use crate::CachedResponse;
use crate::error::{Error, Result};

pub async fn run(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    note_id: &str,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    let resp = client.note(note_id, None).await?;
    match resp {
        CachedResponse::Modified { body, .. } => {
            println!("{}", body.content);
            Ok(())
        }
        // Unreachable: we passed `None` as etag, so the server can't legally
        // respond with 304. Surface a clear error rather than panic.
        CachedResponse::NotModified => Err(Error::Config(
            "unexpected 304 from server (no ETag was sent)".into(),
        )),
    }
}

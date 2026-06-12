//! `hackmd tui` — entry point.
//!
//! When compiled with `--features tui`, runs the full markdown TUI
//! (blocking event loop) via [`crate::tui::run_blocking`]. Otherwise
//! prints a one-liner telling the user to reinstall with the feature
//! enabled.

use std::path::{Path, PathBuf};

use crate::error::Result;

#[cfg(feature = "tui")]
pub async fn run(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    path: Option<PathBuf>,
) -> Result<()> {
    let eff = crate::cli::config::effective(config_dir, cli_endpoint, cli_token)?;
    let handle = tokio::runtime::Handle::current();
    let client = eff
        .token
        .as_deref()
        .and_then(|t| crate::Client::with_endpoint(t, eff.endpoint.clone()).ok());
    let cloud = crate::tui::cloud::CloudContext::new(handle, client);

    // With a path, open it locally (dir → browser, file → reader). With
    // none, come up on the cloud notes view (cwd stays the local fallback).
    let (source, start_cloud) = match path {
        None => (
            crate::tui::app::Source::Directory(
                std::env::current_dir().map_err(crate::error::Error::Io)?,
            ),
            true,
        ),
        Some(p) => {
            let meta = std::fs::metadata(&p)
                .map_err(|e| crate::error::Error::Config(format!("{}: {e}", p.display())))?;
            let p = p.canonicalize().unwrap_or(p);
            let source = if meta.is_dir() {
                crate::tui::app::Source::Directory(p)
            } else {
                crate::tui::app::Source::File(p)
            };
            (source, false)
        }
    };

    let opts = crate::tui::LaunchOpts {
        source,
        width: 0,
        line_numbers: false,
        style: "auto".to_string(),
        start_cloud,
        start_note: None,
    };
    // The event loop is sync and blocks until quit; `block_in_place` keeps
    // the (multi-thread) runtime healthy while this worker is occupied.
    tokio::task::block_in_place(|| crate::tui::run_blocking(opts, cloud))
        .map_err(|e| crate::error::Error::Config(e.to_string()))
}

#[cfg(not(feature = "tui"))]
pub async fn run(
    _config_dir: Option<&Path>,
    _cli_endpoint: Option<&str>,
    _cli_token: Option<&str>,
    _path: Option<PathBuf>,
) -> Result<()> {
    println!(
        "this binary was built without the `tui` feature. \
         Reinstall with `cargo install hackmd --features tui` to enable."
    );
    Ok(())
}

/// Footer appended to notes started with `hackmd new`.
const NEW_NOTE_FOOTER: &str =
    "*Written in the terminal with [hackmd TUI](https://github.com/zemse/hackmd)*";

/// `hackmd new [TITLE…]` — create a cloud note pre-filled with a title
/// heading and footer, then open it straight in the TUI split editor.
pub async fn new_note(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    title_words: Vec<String>,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    let joined = title_words.join(" ");
    let title = joined.trim();
    let title = if title.is_empty() { "New note" } else { title };
    // Blank body line between the heading and the footer rule — the editor
    // opens with the cursor there (see `App::open_created_note`).
    let content = format!("# {title}\n\n\n\n---\n\n{NEW_NOTE_FOOTER}\n");
    let note = client
        .create_note(crate::types::CreateNoteOptions {
            title: Some(title.to_string()),
            content: Some(content),
            ..Default::default()
        })
        .await?;

    #[cfg(feature = "tui")]
    {
        let handle = tokio::runtime::Handle::current();
        let cloud = crate::tui::cloud::CloudContext::with_client(client, handle);
        let opts = crate::tui::LaunchOpts {
            source: crate::tui::app::Source::Directory(
                std::env::current_dir().map_err(crate::error::Error::Io)?,
            ),
            width: 0,
            line_numbers: false,
            style: "auto".to_string(),
            start_cloud: false,
            start_note: Some(Box::new(note)),
        };
        tokio::task::block_in_place(|| crate::tui::run_blocking(opts, cloud))
            .map_err(|e| crate::error::Error::Config(e.to_string()))
    }
    #[cfg(not(feature = "tui"))]
    {
        println!("Created note {} — {}", note.id, note.title);
        println!(
            "this binary was built without the `tui` feature. \
             Reinstall with `cargo install hackmd --features tui` to edit it here."
        );
        Ok(())
    }
}

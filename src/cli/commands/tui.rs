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

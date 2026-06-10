//! `hackmd tui` — entry point.
//!
//! When compiled with `--features tui`, runs the full markdown TUI
//! (blocking event loop) via [`crate::tui::run_blocking`]. Otherwise
//! prints a one-liner telling the user to reinstall with the feature
//! enabled.

use std::path::Path;

use crate::error::Result;

#[cfg(feature = "tui")]
pub async fn run(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
) -> Result<()> {
    let _ = (config_dir, cli_endpoint, cli_token);
    let opts = crate::tui::LaunchOpts {
        source: crate::tui::app::Source::Directory(
            std::env::current_dir().map_err(crate::error::Error::Io)?,
        ),
        width: 0,
        line_numbers: false,
        style: "auto".to_string(),
    };
    // The event loop is sync and blocks until quit; `block_in_place` keeps
    // the (multi-thread) runtime healthy while this worker is occupied.
    tokio::task::block_in_place(|| crate::tui::run_blocking(opts))
        .map_err(|e| crate::error::Error::Config(e.to_string()))
}

#[cfg(not(feature = "tui"))]
pub async fn run(
    _config_dir: Option<&Path>,
    _cli_endpoint: Option<&str>,
    _cli_token: Option<&str>,
) -> Result<()> {
    println!(
        "this binary was built without the `tui` feature. \
         Reinstall with `cargo install hackmd --features tui` to enable."
    );
    Ok(())
}

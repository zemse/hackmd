//! Terminal markdown reader/editor, ported from md-tui v0.3.0
//! (zemse/md-tui @ d724187).
//!
//! Local-first: browse directories, read/edit markdown with a split
//! live-preview editor, mouse support, images, themes. Cloud (HackMD)
//! support is layered on top via [`crate::Client`].

pub mod app;
pub mod cloud;
pub mod events;
mod jsonl;
mod links;
mod markdown;
mod md_config;
mod palette;
mod read_state;
mod syntax;
pub mod theme;
pub mod ui;

use anyhow::Result;

/// Launch parameters for the `hackmd tui` subcommand.
pub struct LaunchOpts {
    /// Local fallback view (and the `Esc`/`H` target out of cloud mode).
    pub source: app::Source,
    /// Word-wrap width (0 = terminal width).
    pub width: u16,
    pub line_numbers: bool,
    /// Theme name: `dark` | `light` | `auto`.
    pub style: String,
    /// Open in the HackMD cloud browser when a token is configured;
    /// without one the TUI starts (and stays usable) on `source`.
    pub start_cloud: bool,
    /// Open straight into the split editor on this just-created note
    /// (the `hackmd new` flow). Takes precedence over `start_cloud`.
    pub start_note: Option<Box<crate::types::SingleNote>>,
}

/// Run the TUI event loop on the calling thread until the user quits.
///
/// Sync and blocking — the caller owns the terminal for the duration.
/// Cloud futures run on the runtime behind `cloud`; their results are
/// drained each tick.
pub fn run_blocking(opts: LaunchOpts, cloud: cloud::CloudContext) -> Result<()> {
    let cfg = md_config::load();
    let theme = theme::resolve(&opts.style, &cfg);

    let app_opts = app::Options {
        width: opts.width,
        line_numbers: opts.line_numbers,
        theme,
    };

    let mut app = app::App::with_cloud(opts.source, app_opts, cloud)?;
    if let Some(note) = opts.start_note {
        // `hackmd new` — straight into the editor on the fresh note.
        app.open_created_note(*note);
    } else if opts.start_cloud && app.cloud.is_connected() {
        // Open on the cloud browser; the local view stays one Esc/H away.
        let _ = app.navigate_to(app::EntryKind::CloudList, 0);
    }
    let mut term = ui::setup_terminal()?;
    // Probe for graphics support only after entering the alt screen, so any
    // unrecognized response bytes don't pollute the user's shell on exit.
    app.init_image_picker();

    let panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = ui::restore_raw();
        panic_hook(info);
    }));

    let res = events::run(&mut term, &mut app);
    app.read_state.flush();
    ui::restore_terminal(&mut term)?;
    res
}

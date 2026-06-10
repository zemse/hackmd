//! HackMD API client for Rust.
//!
//! Async client for the HackMD HTTP API (mirrors `@hackmd/api`).
//!
//! # Quick start
//!
//! ```no_run
//! # async fn run() -> hackmd::Result<()> {
//! let client = hackmd::Client::new(std::env::var("HMD_API_ACCESS_TOKEN").unwrap())?;
//! let me = client.me().await?;
//! println!("hello {}", me.name);
//!
//! // List the authenticated user's notes (may be empty).
//! let notes = client.notes().await?;
//! for n in &notes {
//!     println!("- {} ({})", n.title, n.id);
//! }
//! # Ok(()) }
//! ```
//!
//! Requires a tokio runtime.
//!
//! # Feature flags
//!
//! * `cli` (default) — builds the `hackmd` binary and the [`cli`] module.
//! * `tui` — builds the `md` binary and enables `hackmd tui` (implies
//!   `cli`): a full terminal markdown reader/browser/editor (the merged
//!   md-tui v0.3.0) with HackMD cloud mode behind the `H`/`gh` keybind —
//!   browse, read, edit, publish, push and download notes.
//!
//! With `default-features = false` you get a pure async SDK with no clap /
//! comfy-table / ratatui / etc. dependencies pulled into the binary.

pub mod api;
#[cfg(feature = "cli")]
pub mod cli;
pub mod client;
pub mod error;
#[cfg(feature = "tui")]
pub mod tui;
pub mod types;

pub use client::{CachedResponse, Client, ClientConfig, DEFAULT_ENDPOINT, RetryConfig};
pub use error::{Error, Result};
pub use types::{
    ApiFolder, ApiFolderOrder, CommentPermissionType, CreateNoteOptions, CreateTeamFolderBody,
    CreateUserFolderBody, FolderPath, Note, NotePermissionRole, NotePublishType, SimpleUserProfile,
    SingleNote, Team, TeamVisibilityType, UpdateFolderOrderBody, UpdateNoteOptions,
    UpdateTeamFolderBody, UpdateUserFolderBody, User,
};

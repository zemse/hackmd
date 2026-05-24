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
//! # Ok(()) }
//! ```
//!
//! Requires a tokio runtime.

pub mod api;
pub mod client;
pub mod error;
pub mod types;

pub use client::{CachedResponse, Client, ClientConfig, RetryConfig, DEFAULT_ENDPOINT};
pub use error::{Error, Result};
pub use types::{
    ApiFolder, ApiFolderOrder, CommentPermissionType, CreateNoteOptions, CreateTeamFolderBody,
    CreateUserFolderBody, FolderPath, Note, NotePermissionRole, NotePublishType,
    SimpleUserProfile, SingleNote, Team, TeamVisibilityType, UpdateFolderOrderBody,
    UpdateNoteOptions, UpdateTeamFolderBody, UpdateUserFolderBody, User,
};

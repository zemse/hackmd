//! HackMD HTTP API endpoint surface.
//!
//! Each submodule adds an `impl Client` block grouping the endpoints for a
//! given resource. End users invoke them as plain methods on
//! [`Client`](crate::Client) — the split is purely for code organization and
//! mirrors the HackMD API resource layout.

pub mod folders;
pub mod notes;
pub mod team_folders;
pub mod team_notes;
pub mod teams;
pub mod user;

use serde::Serialize;

/// PATCH body for the `update_*_content` shorthands — the wire shape
/// `{ "content": "…" }` (or `{}` when `None`). Shared by the user-note and
/// team-note endpoints, which take the identical body.
#[derive(Debug, Serialize)]
pub(crate) struct UpdateContentBody<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) content: Option<&'a str>,
}

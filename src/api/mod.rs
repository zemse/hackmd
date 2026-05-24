//! HackMD HTTP API endpoint surface.
//!
//! Each submodule adds an `impl Client` block grouping the endpoints for a
//! given resource. End users invoke them as plain methods on
//! [`Client`](crate::Client) — the split is purely for code organization and
//! mirrors the layout described in `PLAN.md` section 0.1.

pub mod folders;
pub mod notes;
pub mod team_folders;
pub mod team_notes;
pub mod teams;
pub mod user;

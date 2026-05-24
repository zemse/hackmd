//! Data types mirroring the HackMD HTTP API.
//!
//! All structs use `#[serde(rename_all = "camelCase")]` so they round-trip
//! against the JSON shapes documented in the upstream OpenAPI spec.

mod folder;
mod note;
mod user;

pub use folder::{
    ApiFolder, ApiFolderOrder, CreateTeamFolderBody, CreateUserFolderBody, FolderPath,
    UpdateFolderOrderBody, UpdateTeamFolderBody, UpdateUserFolderBody,
};
pub use note::{
    CommentPermissionType, CreateNoteOptions, Note, NotePermissionRole, NotePublishType,
    SingleNote, UpdateNoteOptions,
};
pub use user::{SimpleUserProfile, Team, TeamVisibilityType, User};

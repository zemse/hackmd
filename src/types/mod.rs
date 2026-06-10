//! Data types mirroring the HackMD HTTP API.
//!
//! All structs use `#[serde(rename_all = "camelCase")]` so they round-trip
//! against the JSON shapes documented in the upstream OpenAPI spec.

mod folder;
mod note;
mod user;

/// The live API is inconsistent about date encodings: some fields arrive as
/// ISO-8601 strings, others as epoch milliseconds — and it varies by
/// endpoint (e.g. `/me` returns teams with integer `createdAt` while the
/// note endpoints use strings). Accept both, normalizing millis to RFC3339.
pub(crate) fn de_date<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        S(String),
        Millis(i64),
    }
    Ok(match Raw::deserialize(d)? {
        Raw::S(s) => s,
        Raw::Millis(n) => millis_to_rfc3339(n),
    })
}

/// `Option` variant of [`de_date`] for nullable date fields.
pub(crate) fn de_opt_date<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        S(String),
        Millis(i64),
    }
    Ok(Option::<Raw>::deserialize(d)?.map(|v| match v {
        Raw::S(s) => s,
        Raw::Millis(n) => millis_to_rfc3339(n),
    }))
}

fn millis_to_rfc3339(n: i64) -> String {
    chrono::DateTime::from_timestamp_millis(n)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_else(|| n.to_string())
}

pub use folder::{
    ApiFolder, ApiFolderOrder, CreateTeamFolderBody, CreateUserFolderBody, FolderPath,
    UpdateFolderOrderBody, UpdateTeamFolderBody, UpdateUserFolderBody,
};
pub use note::{
    CommentPermissionType, CreateNoteOptions, Note, NotePermissionRole, NotePublishType,
    SingleNote, UpdateNoteOptions,
};
pub use user::{SimpleUserProfile, Team, TeamVisibilityType, User};

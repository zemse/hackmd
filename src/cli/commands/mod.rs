//! CLI command implementations.

pub mod auth;
pub mod export;
pub mod history;
pub mod notes;
pub mod team_notes;
pub mod teams;
pub mod tui;

use std::path::Path;

use crate::Client;
use crate::cli::config::{Effective, effective};
use crate::error::{Error, Result};
use crate::types::{Note, NotePermissionRole};

/// Human-readable access summary for note list views: `published` when the
/// note has a public page, otherwise who can read it — `anyone with link`
/// (guest), `signed in`, or `only team` / `only me` (owner).
fn visibility(note: &Note) -> &'static str {
    if note.published_at.is_some() {
        return "published";
    }
    match note.read_permission {
        NotePermissionRole::Guest => "anyone with link",
        NotePermissionRole::SignedIn => "signed in",
        NotePermissionRole::Owner => {
            if note.team_path.is_some() {
                "only team"
            } else {
                "only me"
            }
        }
    }
}

/// Serialize notes for list output, adding derived columns:
/// - `visibility` — see [`visibility`]
/// - `owner` — the workspace path (`@<path>` URL segment). `userPath` and
///   `teamPath` are mutually exclusive on the live API, so this is whichever
///   one is set.
pub(crate) fn note_rows(notes: &[Note]) -> Result<Vec<serde_json::Value>> {
    notes
        .iter()
        .map(|n| {
            let mut v = serde_json::to_value(n).map_err(Error::Json)?;
            if let Some(obj) = v.as_object_mut() {
                obj.insert("visibility".into(), visibility(n).into());
                let owner = n.user_path.as_deref().or(n.team_path.as_deref());
                obj.insert("owner".into(), owner.into());
            }
            Ok(v)
        })
        .collect()
}

/// Resolve effective config + build an authenticated [`Client`].
///
/// Returns a friendly error when no token is configured anywhere.
pub fn build_client(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
) -> Result<(Client, Effective)> {
    let eff = effective(config_dir, cli_endpoint, cli_token)?;
    let token = eff.token.clone().ok_or_else(|| {
        Error::Config(
            "no access token configured — run `hackmd login` or set HMD_API_ACCESS_TOKEN".into(),
        )
    })?;
    let client = Client::with_endpoint(token, eff.endpoint.clone())?;
    Ok((client, eff))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(published_at: Option<&str>, read: &str, team_path: Option<&str>) -> Note {
        serde_json::from_value(serde_json::json!({
            "id": "n1",
            "title": "t",
            "tags": [],
            "lastChangedAt": "2024-01-01T00:00:00.000Z",
            "createdAt": "2024-01-01T00:00:00.000Z",
            "lastChangeUser": null,
            "publishType": "view",
            "publishedAt": published_at,
            "userPath": "zemse",
            "teamPath": team_path,
            "permalink": null,
            "shortId": "s1",
            "publishLink": "https://hackmd.io/@zemse/n1",
            "readPermission": read,
            "writePermission": "owner",
        }))
        .expect("note fixture")
    }

    #[test]
    fn visibility_published_wins_over_read_permission() {
        let n = note(Some("2024-01-02T00:00:00.000Z"), "owner", None);
        assert_eq!(visibility(&n), "published");
    }

    #[test]
    fn visibility_maps_read_permission() {
        assert_eq!(visibility(&note(None, "guest", None)), "anyone with link");
        assert_eq!(visibility(&note(None, "signed_in", None)), "signed in");
        assert_eq!(visibility(&note(None, "owner", None)), "only me");
        assert_eq!(visibility(&note(None, "owner", Some("demo"))), "only team");
    }

    #[test]
    fn note_rows_injects_visibility_column() {
        let rows = note_rows(&[note(None, "guest", None)]).expect("rows");
        assert_eq!(rows[0]["visibility"], "anyone with link");
        assert_eq!(rows[0]["id"], "n1");
    }

    #[test]
    fn note_rows_owner_prefers_user_path_then_team_path() {
        let rows = note_rows(&[note(None, "owner", None)]).expect("rows");
        assert_eq!(rows[0]["owner"], "zemse");

        let mut team = note(None, "owner", Some("demo"));
        team.user_path = None;
        let rows = note_rows(&[team]).expect("rows");
        assert_eq!(rows[0]["owner"], "demo");
    }
}

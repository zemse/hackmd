//! Note types, enums, and create/update payloads.

use serde::{Deserialize, Serialize};

use super::folder::FolderPath;
use super::user::SimpleUserProfile;

/// How a published note is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotePublishType {
    Edit,
    View,
    Slide,
    Book,
}

/// Who may comment on a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommentPermissionType {
    #[serde(rename = "disabled")]
    Disabled,
    #[serde(rename = "forbidden")]
    Forbidden,
    #[serde(rename = "owners")]
    Owners,
    #[serde(rename = "signed_in_users")]
    SignedInUsers,
    #[serde(rename = "everyone")]
    Everyone,
}

/// Who may read/write a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotePermissionRole {
    #[serde(rename = "owner")]
    Owner,
    #[serde(rename = "signed_in")]
    SignedIn,
    #[serde(rename = "guest")]
    Guest,
}

/// A note as returned in list endpoints (no `content`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: String,
    pub title: String,
    pub tags: Vec<String>,
    // Date fields accept both ISO strings and epoch millis — the live API
    // mixes the two encodings across endpoints.
    #[serde(deserialize_with = "super::de_date")]
    pub last_changed_at: String,
    #[serde(deserialize_with = "super::de_date")]
    pub created_at: String,
    pub last_change_user: Option<SimpleUserProfile>,
    pub publish_type: NotePublishType,
    #[serde(default, deserialize_with = "super::de_opt_date")]
    pub published_at: Option<String>,
    pub user_path: Option<String>,
    pub team_path: Option<String>,
    pub permalink: Option<String>,
    pub short_id: String,
    pub publish_link: String,
    pub read_permission: NotePermissionRole,
    pub write_permission: NotePermissionRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_paths: Option<Vec<FolderPath>>,
}

impl Note {
    /// Human-readable access summary: `published` when the note has a
    /// public page (listed on the owner's profile, search-indexable),
    /// otherwise who can read it — `anyone with link` (guest, unlisted),
    /// `signed in`, or `only team` / `only me` (owner).
    pub fn visibility(&self) -> &'static str {
        if self.published_at.is_some() {
            return "published";
        }
        match self.read_permission {
            NotePermissionRole::Guest => "anyone with link",
            NotePermissionRole::SignedIn => "signed in",
            NotePermissionRole::Owner => {
                if self.team_path.is_some() {
                    "only team"
                } else {
                    "only me"
                }
            }
        }
    }
}

/// A note fetched individually — same as [`Note`] but includes the raw
/// markdown body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SingleNote {
    pub id: String,
    pub title: String,
    pub tags: Vec<String>,
    #[serde(deserialize_with = "super::de_date")]
    pub last_changed_at: String,
    #[serde(deserialize_with = "super::de_date")]
    pub created_at: String,
    pub last_change_user: Option<SimpleUserProfile>,
    pub publish_type: NotePublishType,
    #[serde(default, deserialize_with = "super::de_opt_date")]
    pub published_at: Option<String>,
    pub user_path: Option<String>,
    pub team_path: Option<String>,
    pub permalink: Option<String>,
    pub short_id: String,
    pub publish_link: String,
    pub read_permission: NotePermissionRole,
    pub write_permission: NotePermissionRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_paths: Option<Vec<FolderPath>>,
    pub content: String,
}

/// Body for `POST /notes` and `POST /teams/{teamPath}/notes`.
///
/// Every field is optional; absent fields are omitted from the serialized
/// JSON body so callers don't accidentally send `null`s.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNoteOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_permission: Option<NotePermissionRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write_permission: Option<NotePermissionRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment_permission: Option<CommentPermissionType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permalink: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_folder_id: Option<String>,
}

/// Body for `PATCH /notes/{id}` and `PATCH /teams/{teamPath}/notes/{id}`.
///
/// Mirrors `UpdateNoteOptions` upstream — a strict subset of [`SingleNote`]
/// fields plus `parentFolderId`. Absent fields are omitted so the PATCH only
/// touches the keys the caller actually set.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateNoteOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_permission: Option<NotePermissionRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write_permission: Option<NotePermissionRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permalink: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_folder_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_publish_type_round_trip() {
        for (v, s) in [
            (NotePublishType::Edit, "\"edit\""),
            (NotePublishType::View, "\"view\""),
            (NotePublishType::Slide, "\"slide\""),
            (NotePublishType::Book, "\"book\""),
        ] {
            assert_eq!(serde_json::to_string(&v).expect("ser"), s);
            let back: NotePublishType = serde_json::from_str(s).expect("de");
            assert_eq!(back, v);
        }
    }

    #[test]
    fn comment_permission_type_round_trip() {
        for (v, s) in [
            (CommentPermissionType::Disabled, "\"disabled\""),
            (CommentPermissionType::Forbidden, "\"forbidden\""),
            (CommentPermissionType::Owners, "\"owners\""),
            (CommentPermissionType::SignedInUsers, "\"signed_in_users\""),
            (CommentPermissionType::Everyone, "\"everyone\""),
        ] {
            assert_eq!(serde_json::to_string(&v).expect("ser"), s);
            let back: CommentPermissionType = serde_json::from_str(s).expect("de");
            assert_eq!(back, v);
        }
    }

    #[test]
    fn note_permission_role_round_trip() {
        for (v, s) in [
            (NotePermissionRole::Owner, "\"owner\""),
            (NotePermissionRole::SignedIn, "\"signed_in\""),
            (NotePermissionRole::Guest, "\"guest\""),
        ] {
            assert_eq!(serde_json::to_string(&v).expect("ser"), s);
            let back: NotePermissionRole = serde_json::from_str(s).expect("de");
            assert_eq!(back, v);
        }
    }

    #[test]
    fn note_round_trip_minimal() {
        // Minimal shape mirroring upstream test fixtures.
        let raw = r#"{
            "id": "n1",
            "title": "Hello",
            "tags": ["a", "b"],
            "lastChangedAt": "2024-01-01T00:00:00.000Z",
            "createdAt": "2024-01-01T00:00:00.000Z",
            "lastChangeUser": null,
            "publishType": "view",
            "publishedAt": null,
            "userPath": "alice",
            "teamPath": null,
            "permalink": null,
            "shortId": "abc",
            "publishLink": "https://hackmd.io/abc",
            "readPermission": "owner",
            "writePermission": "signed_in"
        }"#;
        let n: Note = serde_json::from_str(raw).expect("parse note");
        assert_eq!(n.id, "n1");
        assert_eq!(n.tags, vec!["a", "b"]);
        assert!(n.last_change_user.is_none());
        assert_eq!(n.publish_type, NotePublishType::View);
        assert_eq!(n.read_permission, NotePermissionRole::Owner);
        assert_eq!(n.write_permission, NotePermissionRole::SignedIn);
        assert!(n.folder_paths.is_none());

        let json = serde_json::to_value(&n).expect("to_value");
        assert_eq!(json["shortId"], "abc");
        assert_eq!(json["readPermission"], "owner");
        assert_eq!(json["writePermission"], "signed_in");
        // folder_paths absent because of skip_serializing_if.
        assert!(json.get("folderPaths").is_none());
    }

    // Regression: live API note endpoints can return epoch-millis numbers
    // for date fields (encoding varies by endpoint).
    #[test]
    fn note_accepts_epoch_millis_dates() {
        let raw = r#"{
            "id": "n1",
            "title": "Hello",
            "tags": [],
            "lastChangedAt": 1633693316914,
            "createdAt": 1633693316914,
            "lastChangeUser": null,
            "publishType": "view",
            "publishedAt": 1633693316914,
            "userPath": null,
            "teamPath": null,
            "permalink": null,
            "shortId": "abc",
            "publishLink": "",
            "readPermission": "owner",
            "writePermission": "owner"
        }"#;
        let n: Note = serde_json::from_str(raw).expect("parse note with millis dates");
        assert_eq!(n.last_changed_at, "2021-10-08T11:41:56.914Z");
        assert_eq!(n.created_at, "2021-10-08T11:41:56.914Z");
        assert_eq!(n.published_at.as_deref(), Some("2021-10-08T11:41:56.914Z"));
    }

    #[test]
    fn single_note_round_trip() {
        let raw = r##"{
            "id": "n1",
            "title": "Hello",
            "tags": [],
            "lastChangedAt": "2024-01-01T00:00:00.000Z",
            "createdAt": "2024-01-01T00:00:00.000Z",
            "lastChangeUser": null,
            "publishType": "edit",
            "publishedAt": null,
            "userPath": null,
            "teamPath": null,
            "permalink": null,
            "shortId": "abc",
            "publishLink": "",
            "readPermission": "guest",
            "writePermission": "owner",
            "content": "# hi"
        }"##;
        let n: SingleNote = serde_json::from_str(raw).expect("parse single note");
        assert_eq!(n.content, "# hi");
        assert_eq!(n.publish_type, NotePublishType::Edit);
        assert_eq!(n.read_permission, NotePermissionRole::Guest);

        let json = serde_json::to_value(&n).expect("to_value");
        assert_eq!(json["content"], "# hi");
    }

    #[test]
    fn create_note_options_omits_none_fields() {
        let opts = CreateNoteOptions {
            title: Some("Hi".into()),
            content: Some("body".into()),
            ..Default::default()
        };
        let json = serde_json::to_value(&opts).expect("to_value");
        assert_eq!(json["title"], "Hi");
        assert_eq!(json["content"], "body");
        // None fields are NOT serialized as null — they're omitted entirely.
        assert!(json.get("tags").is_none());
        assert!(json.get("description").is_none());
        assert!(json.get("readPermission").is_none());
        assert!(json.get("commentPermission").is_none());
    }

    #[test]
    fn create_note_options_full() {
        let opts = CreateNoteOptions {
            title: Some("T".into()),
            content: Some("C".into()),
            description: Some("D".into()),
            tags: Some(vec!["x".into()]),
            read_permission: Some(NotePermissionRole::Owner),
            write_permission: Some(NotePermissionRole::SignedIn),
            comment_permission: Some(CommentPermissionType::SignedInUsers),
            permalink: Some("perma".into()),
            parent_folder_id: Some("pf".into()),
        };
        let json = serde_json::to_value(&opts).expect("to_value");
        assert_eq!(json["readPermission"], "owner");
        assert_eq!(json["writePermission"], "signed_in");
        assert_eq!(json["commentPermission"], "signed_in_users");
        assert_eq!(json["parentFolderId"], "pf");
    }

    #[test]
    fn update_note_options_partial() {
        let opts = UpdateNoteOptions {
            content: Some("new".into()),
            ..Default::default()
        };
        let json = serde_json::to_value(&opts).expect("to_value");
        assert_eq!(json["content"], "new");
        assert!(json.get("title").is_none());
        assert!(json.get("tags").is_none());
    }

    #[test]
    fn update_note_options_title_tags_metadata() {
        // Mirrors upstream etag.spec.ts "should support updating note title and tags metadata"
        let opts = UpdateNoteOptions {
            title: Some("Updated Metadata Title".into()),
            tags: Some(vec!["api".into(), "metadata".into()]),
            ..Default::default()
        };
        let json = serde_json::to_value(&opts).expect("to_value");
        assert_eq!(
            json,
            serde_json::json!({
                "title": "Updated Metadata Title",
                "tags": ["api", "metadata"]
            })
        );
    }
}

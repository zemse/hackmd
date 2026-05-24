//! Folder types.
//!
//! Mirrors `_ref/api-client/nodejs/src/type.ts:124-176`.
//!
//! ## `Option<Option<T>>` on update bodies
//!
//! The folder PATCH endpoints need to distinguish three caller intents per
//! field:
//!
//! * absent in the request body → "don't change this field"
//! * explicit `null` in the request body → "clear this field"
//! * a concrete value → "set to this"
//!
//! Rust's `Option<T>` collapses the first two. The standard fix is the
//! `Option<Option<T>>` pattern: outer `None` ⇒ skip serializing; outer
//! `Some(None)` ⇒ serialize as `null`; outer `Some(Some(v))` ⇒ serialize as
//! the value. Serde needs a helper to deserialize the `null` case correctly,
//! so we use [`serde_with::rust::double_option`].

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Breadcrumb segment returned on notes' `folderPaths` field (OpenAPI
/// `FolderPath`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderPath {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub parent_id: Option<String>,
    pub client_id: String,
}

/// A user or team folder entity.
///
/// Note: `created_at` / `updated_at` are millisecond unix epochs as `i64`,
/// not strings — this is one of the few wire shapes where HackMD does serve
/// numbers instead of ISO strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiFolder {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub parent_folder_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Folder ordering map. Each key is either a parent folder id or the literal
/// `"root"` (for the root-level ordering); the value is the ordered list of
/// child folder ids under that parent.
pub type ApiFolderOrder = HashMap<String, Vec<String>>;

/// POST `/folders` request body — every field optional, absent fields are
/// omitted from the wire.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateUserFolderBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_folder_id: Option<String>,
}

/// PATCH `/folders/{id}` request body. Uses [`Option<Option<T>>`] on the
/// fields that may be cleared, so callers can distinguish "don't change"
/// (outer `None`) from "set to null" (outer `Some(None)`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUserFolderBody {
    /// Folder name. Upstream type is `string | undefined` (no `null`), so a
    /// plain `Option<String>` is sufficient here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(
        default,
        with = "serde_with::rust::double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<Option<String>>,
    #[serde(
        default,
        with = "serde_with::rust::double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub icon: Option<Option<String>>,
    #[serde(
        default,
        with = "serde_with::rust::double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub color: Option<Option<String>>,
    #[serde(
        default,
        with = "serde_with::rust::double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_folder_id: Option<Option<String>>,
}

/// Team-scope folder create body. Upstream type alias: identical shape to
/// [`CreateUserFolderBody`]; we mirror that with a type alias to keep the
/// surface small.
pub type CreateTeamFolderBody = CreateUserFolderBody;

/// Team-scope folder update body. Upstream type alias: identical shape to
/// [`UpdateUserFolderBody`]; we mirror that with a type alias to keep the
/// surface small.
pub type UpdateTeamFolderBody = UpdateUserFolderBody;

/// PUT `/folders/folder-order` request body — wraps an [`ApiFolderOrder`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateFolderOrderBody {
    pub order: ApiFolderOrder,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_path_round_trip() {
        let raw = r##"{
            "id": "f1",
            "name": "Research",
            "icon": null,
            "color": null,
            "parentId": null,
            "clientId": "cid-1"
        }"##;
        let fp: FolderPath = serde_json::from_str(raw).expect("parse FolderPath");
        assert_eq!(fp.id, "f1");
        assert_eq!(fp.name, "Research");
        assert!(fp.parent_id.is_none());
        assert_eq!(fp.client_id, "cid-1");

        let json = serde_json::to_value(&fp).expect("to_value");
        assert_eq!(json["parentId"], serde_json::Value::Null);
        assert_eq!(json["clientId"], "cid-1");
    }

    #[test]
    fn api_folder_round_trip() {
        // Mirrors the fixture shape used in upstream tests/api.spec.ts:117-139.
        let raw = r##"{
            "id": "folder-1",
            "name": "Research",
            "description": "notes about things",
            "icon": "book",
            "color": "#ff8800",
            "parentFolderId": null,
            "createdAt": 1700000000000,
            "updatedAt": 1700000001000
        }"##;
        let f: ApiFolder = serde_json::from_str(raw).expect("parse ApiFolder");
        assert_eq!(f.id, "folder-1");
        assert_eq!(f.created_at, 1700000000000);
        assert_eq!(f.updated_at, 1700000001000);
        assert_eq!(f.description.as_deref(), Some("notes about things"));
        assert_eq!(f.icon.as_deref(), Some("book"));
        assert_eq!(f.color.as_deref(), Some("#ff8800"));
        assert!(f.parent_folder_id.is_none());

        let json = serde_json::to_value(&f).expect("to_value");
        assert_eq!(json["createdAt"], 1700000000000_i64);
        assert_eq!(json["updatedAt"], 1700000001000_i64);
        assert_eq!(json["parentFolderId"], serde_json::Value::Null);
    }

    #[test]
    fn api_folder_round_trip_nulls() {
        // Verifies null description/icon/color (the common "freshly created" shape)
        // also deserializes cleanly.
        let raw = r##"{
            "id": "folder-2",
            "name": "Empty",
            "description": null,
            "icon": null,
            "color": null,
            "parentFolderId": null,
            "createdAt": 1700000000000,
            "updatedAt": 1700000000000
        }"##;
        let f: ApiFolder = serde_json::from_str(raw).expect("parse ApiFolder");
        assert!(f.description.is_none());
        assert!(f.icon.is_none());
        assert!(f.color.is_none());
        assert!(f.parent_folder_id.is_none());
    }

    #[test]
    fn create_user_folder_body_skips_absent_fields() {
        let body = CreateUserFolderBody {
            name: Some("New".into()),
            ..Default::default()
        };
        let json = serde_json::to_value(&body).expect("to_value");
        assert_eq!(json, serde_json::json!({ "name": "New" }));
    }

    #[test]
    fn update_user_folder_body_only_name() {
        let body = UpdateUserFolderBody {
            name: Some("Renamed".into()),
            ..Default::default()
        };
        let json = serde_json::to_value(&body).expect("to_value");
        // All other fields are absent — verify the wire payload has only `name`.
        assert_eq!(json, serde_json::json!({ "name": "Renamed" }));
    }

    #[test]
    fn update_user_folder_body_explicit_null_clears_field() {
        let body = UpdateUserFolderBody {
            description: Some(None),
            ..Default::default()
        };
        let json = serde_json::to_value(&body).expect("to_value");
        // Critical: `description: null` MUST appear on the wire to instruct the
        // server to clear the field.
        assert_eq!(json, serde_json::json!({ "description": null }));
    }

    #[test]
    fn update_user_folder_body_value_round_trip() {
        let body = UpdateUserFolderBody {
            color: Some(Some("#000000".into())),
            ..Default::default()
        };
        let json = serde_json::to_value(&body).expect("to_value");
        assert_eq!(json, serde_json::json!({ "color": "#000000" }));

        // And the reverse — a JSON value with `color: "#000000"` parses back to
        // `Some(Some("#000000"))`.
        let parsed: UpdateUserFolderBody = serde_json::from_value(json).expect("parse");
        assert_eq!(parsed.color, Some(Some("#000000".into())));
        assert!(parsed.name.is_none());
        assert!(parsed.description.is_none());
    }

    #[test]
    fn update_user_folder_body_all_absent_is_empty_object() {
        let body = UpdateUserFolderBody::default();
        let json = serde_json::to_value(&body).expect("to_value");
        assert_eq!(json, serde_json::json!({}));
    }

    #[test]
    fn update_user_folder_body_deserializes_null_as_some_none() {
        // Verify the double_option deserializer treats an explicit `null` as
        // `Some(None)` (i.e. preserves the distinction).
        let raw = r##"{ "description": null }"##;
        let body: UpdateUserFolderBody = serde_json::from_str(raw).expect("parse");
        assert_eq!(body.description, Some(None));
        // And an absent field stays absent (outer None).
        assert!(body.color.is_none());
    }

    #[test]
    fn api_folder_order_round_trip() {
        let mut order: ApiFolderOrder = HashMap::new();
        order.insert("root".into(), vec!["folder-1".into(), "folder-2".into()]);
        order.insert("folder-1".into(), vec!["folder-3".into()]);
        let json = serde_json::to_value(&order).expect("to_value");
        assert_eq!(json["root"], serde_json::json!(["folder-1", "folder-2"]));
        assert_eq!(json["folder-1"], serde_json::json!(["folder-3"]));

        let parsed: ApiFolderOrder = serde_json::from_value(json).expect("parse");
        assert_eq!(parsed.get("root").map(Vec::len), Some(2));
        assert_eq!(parsed.get("folder-1").map(Vec::len), Some(1));
    }

    #[test]
    fn update_folder_order_body_round_trip() {
        let mut order: ApiFolderOrder = HashMap::new();
        order.insert("root".into(), vec!["a".into(), "b".into()]);
        let body = UpdateFolderOrderBody { order };
        let json = serde_json::to_value(&body).expect("to_value");
        assert_eq!(json, serde_json::json!({ "order": { "root": ["a", "b"] } }));

        let parsed: UpdateFolderOrderBody = serde_json::from_value(json).expect("parse");
        assert_eq!(
            parsed.order.get("root"),
            Some(&vec!["a".into(), "b".into()])
        );
    }
}

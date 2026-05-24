//! Folder types.
//!
//! Only the minimum surface needed by [`super::Note::folder_paths`] lives here
//! for M1. The full folder API (create/update bodies, folder-order) ships in
//! Track B (milestone M3).

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_path_round_trip() {
        let raw = r#"{
            "id": "f1",
            "name": "Research",
            "icon": null,
            "color": null,
            "parentId": null,
            "clientId": "cid-1"
        }"#;
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
        // Mirrors the fixture used in upstream tests/api.spec.ts:103
        let raw = r#"{
            "id": "folder-1",
            "name": "Research",
            "description": null,
            "icon": null,
            "color": null,
            "parentFolderId": null,
            "createdAt": 1700000000,
            "updatedAt": 1700000001
        }"#;
        let f: ApiFolder = serde_json::from_str(raw).expect("parse ApiFolder");
        assert_eq!(f.id, "folder-1");
        assert_eq!(f.created_at, 1700000000);
        assert_eq!(f.updated_at, 1700000001);
        assert!(f.parent_folder_id.is_none());

        let json = serde_json::to_value(&f).expect("to_value");
        assert_eq!(json["createdAt"], 1700000000);
        assert_eq!(json["parentFolderId"], serde_json::Value::Null);
    }
}

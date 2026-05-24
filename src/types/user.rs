//! User, team, and simple-profile types.

use serde::{Deserialize, Serialize};

/// Visibility of a HackMD team workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamVisibilityType {
    Public,
    Private,
}

/// A HackMD team the authenticated user belongs to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    pub id: String,
    pub owner_id: String,
    pub name: String,
    pub logo: String,
    pub path: String,
    pub description: String,
    pub hard_breaks: bool,
    pub visibility: TeamVisibilityType,
    // Upstream types this as `Date` but the wire value is a JSON string; the JS
    // SDK never actually parses it, so we keep it as-is.
    pub created_at: String,
}

/// The authenticated user.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub email: Option<String>,
    pub name: String,
    pub user_path: String,
    pub photo: String,
    pub teams: Vec<Team>,
}

/// Compact user profile shape attached to notes' `lastChangeUser` field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimpleUserProfile {
    pub name: String,
    pub user_path: String,
    pub photo: String,
    pub biography: Option<String>,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_visibility_round_trip() {
        let v = TeamVisibilityType::Public;
        let s = serde_json::to_string(&v).expect("serialize");
        assert_eq!(s, "\"public\"");
        let back: TeamVisibilityType = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(back, TeamVisibilityType::Public);

        let v = TeamVisibilityType::Private;
        let s = serde_json::to_string(&v).expect("serialize");
        assert_eq!(s, "\"private\"");
    }

    #[test]
    fn team_round_trip() {
        let raw = r#"{
            "id": "team-1",
            "ownerId": "user-1",
            "name": "Demo Team",
            "logo": "logo.png",
            "path": "demo-team",
            "description": "A team",
            "hardBreaks": true,
            "visibility": "public",
            "createdAt": "2024-01-01T00:00:00.000Z"
        }"#;
        let team: Team = serde_json::from_str(raw).expect("parse team");
        assert_eq!(team.id, "team-1");
        assert_eq!(team.owner_id, "user-1");
        assert_eq!(team.path, "demo-team");
        assert!(team.hard_breaks);
        assert_eq!(team.visibility, TeamVisibilityType::Public);

        let json = serde_json::to_value(&team).expect("to_value");
        assert_eq!(json["ownerId"], "user-1");
        assert_eq!(json["hardBreaks"], true);
        assert_eq!(json["visibility"], "public");
    }

    #[test]
    fn user_round_trip_with_null_email() {
        let raw = r#"{
            "id": "u1",
            "email": null,
            "name": "Alice",
            "userPath": "alice",
            "photo": "p.png",
            "teams": []
        }"#;
        let user: User = serde_json::from_str(raw).expect("parse user");
        assert_eq!(user.id, "u1");
        assert!(user.email.is_none());
        assert_eq!(user.name, "Alice");
        assert_eq!(user.user_path, "alice");
        assert!(user.teams.is_empty());

        let json = serde_json::to_value(&user).expect("to_value");
        assert_eq!(json["userPath"], "alice");
        // Option<None> serializes as null (no skip_serializing_if on this struct).
        assert_eq!(json["email"], serde_json::Value::Null);
    }

    #[test]
    fn simple_user_profile_round_trip() {
        let raw = r#"{
            "name": "Bob",
            "userPath": "bob",
            "photo": "b.png",
            "biography": null,
            "createdAt": "2024-02-02T00:00:00.000Z"
        }"#;
        let p: SimpleUserProfile = serde_json::from_str(raw).expect("parse profile");
        assert_eq!(p.name, "Bob");
        assert_eq!(p.user_path, "bob");
        assert!(p.biography.is_none());

        let json = serde_json::to_value(&p).expect("to_value");
        assert_eq!(json["createdAt"], "2024-02-02T00:00:00.000Z");
    }
}

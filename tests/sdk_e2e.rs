//! Optional live-API end-to-end tests.
//!
//! These tests hit a real HackMD endpoint. They are NOT `#[ignore]`'d —
//! instead they self-skip at runtime when `HACKMD_ACCESS_TOKEN` is absent
//! from the environment. This lets a single `cargo test` invocation behave
//! sensibly both in CI (where the token isn't set) and on a developer's
//! machine (where it usually is, via `.env`).
//!
//! Env vars:
//!
//! * `HACKMD_ACCESS_TOKEN` — required. Skip with a clear message if missing.
//! * `HACKMD_API_ENDPOINT` — optional. Defaults to [`hackmd::DEFAULT_ENDPOINT`].
//! * `HACKMD_E2E_MUTATIONS=1` — opt-in to write tests (create/update/delete).
//! * `HACKMD_E2E_FOLDERS=1`   — opt-in to folder tests (not all hosts support).
//!
//! No `.env` file is ever committed — see `.gitignore`.

use hackmd::{Client, CreateNoteOptions};

/// Resolve a (token, endpoint) pair, or return `None` if the test should skip.
/// Callers print their own skip message and `return` to bail.
fn env_setup() -> Option<(String, String)> {
    let _ = dotenvy::dotenv();
    let token = std::env::var("HACKMD_ACCESS_TOKEN").ok()?;
    if token.is_empty() {
        return None;
    }
    let endpoint = std::env::var("HACKMD_API_ENDPOINT")
        .unwrap_or_else(|_| hackmd::DEFAULT_ENDPOINT.to_string());
    Some((token, endpoint))
}

fn mutations_enabled() -> bool {
    std::env::var("HACKMD_E2E_MUTATIONS").as_deref() == Ok("1")
}

fn folders_enabled() -> bool {
    std::env::var("HACKMD_E2E_FOLDERS").as_deref() == Ok("1")
}

#[tokio::test]
async fn e2e_me_returns_user_with_non_empty_id() {
    let (token, endpoint) = match env_setup() {
        Some(v) => v,
        None => {
            eprintln!("e2e: HACKMD_ACCESS_TOKEN not set, skipping");
            return;
        }
    };
    let client = Client::with_endpoint(token, endpoint).expect("client");
    let me = client.me().await.expect("me");
    assert!(!me.id.is_empty(), "expected a non-empty user id");
}

#[tokio::test]
async fn e2e_notes_returns_ok() {
    let (token, endpoint) = match env_setup() {
        Some(v) => v,
        None => {
            eprintln!("e2e: HACKMD_ACCESS_TOKEN not set, skipping");
            return;
        }
    };
    let client = Client::with_endpoint(token, endpoint).expect("client");
    // May be empty — we only assert the call succeeds.
    let _notes = client.notes().await.expect("notes");
}

#[tokio::test]
async fn e2e_teams_returns_ok() {
    let (token, endpoint) = match env_setup() {
        Some(v) => v,
        None => {
            eprintln!("e2e: HACKMD_ACCESS_TOKEN not set, skipping");
            return;
        }
    };
    let client = Client::with_endpoint(token, endpoint).expect("client");
    let _teams = client.teams().await.expect("teams");
}

#[tokio::test]
async fn e2e_history_returns_ok() {
    let (token, endpoint) = match env_setup() {
        Some(v) => v,
        None => {
            eprintln!("e2e: HACKMD_ACCESS_TOKEN not set, skipping");
            return;
        }
    };
    let client = Client::with_endpoint(token, endpoint).expect("client");
    let _history = client.history().await.expect("history");
}

/// Mutation gate: create → update → fetch → verify → delete.
#[tokio::test]
async fn e2e_create_update_fetch_delete_note_cycle() {
    let (token, endpoint) = match env_setup() {
        Some(v) => v,
        None => {
            eprintln!("e2e: HACKMD_ACCESS_TOKEN not set, skipping");
            return;
        }
    };
    if !mutations_enabled() {
        eprintln!("e2e: HACKMD_E2E_MUTATIONS != 1, skipping mutation test");
        return;
    }
    let client = Client::with_endpoint(token, endpoint).expect("client");

    let created = client
        .create_note(CreateNoteOptions {
            title: Some("hackmd-rs e2e".into()),
            content: Some("# initial".into()),
            ..Default::default()
        })
        .await
        .expect("create_note");
    assert!(!created.id.is_empty());

    let updated_content = "# updated body";
    let _updated = client
        .update_note_content(&created.id, Some(updated_content.into()))
        .await
        .expect("update_note_content");

    // Re-fetch and verify the body was updated.
    let fetched = client.note(&created.id, None).await.expect("note");
    match fetched {
        hackmd::CachedResponse::Modified { body, .. } => {
            assert_eq!(body.content, updated_content);
        }
        hackmd::CachedResponse::NotModified => {
            panic!("expected Modified on fresh fetch");
        }
    }

    client
        .delete_note(&created.id)
        .await
        .expect("delete_note");
}

#[tokio::test]
async fn e2e_folders_returns_ok() {
    let (token, endpoint) = match env_setup() {
        Some(v) => v,
        None => {
            eprintln!("e2e: HACKMD_ACCESS_TOKEN not set, skipping");
            return;
        }
    };
    if !folders_enabled() {
        eprintln!("e2e: HACKMD_E2E_FOLDERS != 1, skipping folder test");
        return;
    }
    let client = Client::with_endpoint(token, endpoint).expect("client");
    let _folders = client.folders().await.expect("folders");
}

#[tokio::test]
async fn e2e_folder_create_get_delete_cycle() {
    let (token, endpoint) = match env_setup() {
        Some(v) => v,
        None => {
            eprintln!("e2e: HACKMD_ACCESS_TOKEN not set, skipping");
            return;
        }
    };
    if !folders_enabled() {
        eprintln!("e2e: HACKMD_E2E_FOLDERS != 1, skipping folder mutation test");
        return;
    }
    if !mutations_enabled() {
        eprintln!("e2e: HACKMD_E2E_MUTATIONS != 1, skipping folder mutation test");
        return;
    }
    let client = Client::with_endpoint(token, endpoint).expect("client");

    let created = client
        .create_folder(hackmd::CreateUserFolderBody {
            name: Some("hackmd-rs e2e folder".into()),
            ..Default::default()
        })
        .await
        .expect("create_folder");
    assert!(!created.id.is_empty());

    let fetched = client.folder(&created.id).await.expect("folder");
    assert_eq!(fetched.id, created.id);

    client
        .delete_folder(&created.id)
        .await
        .expect("delete_folder");
}

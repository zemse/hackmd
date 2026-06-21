//! Async bridge between the sync TUI event loop and the HackMD API.
//!
//! Every cloud operation is spawned onto a tokio runtime [`Handle`]; the
//! result comes back over an unbounded mpsc channel as a [`CloudMsg`] and is
//! drained once per event-loop tick (`App::drain_cloud_msgs`, ≤4×/sec). The
//! TUI thread never blocks on the network. This is the same spawn + mpsc +
//! `try_recv` pattern the pre-merge minimal TUI shipped with.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use tokio::runtime::Handle;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::Client;
use crate::client::CachedResponse;
use crate::types::{
    CreateNoteOptions, Note, NotePermissionRole, SingleNote, Team, UpdateNoteOptions,
};

/// Connection to the HackMD API from inside the TUI. Holds the runtime
/// handle tasks are spawned on and the channel their results return over.
///
/// Both `client` and `handle` are optional so the TUI stays fully usable
/// with no token (or, in tests, with no runtime at all): spawn helpers
/// simply return `false` and the caller reports "not logged in".
pub struct CloudContext {
    handle: Option<Handle>,
    pub client: Option<Client>,
    tx: UnboundedSender<CloudMsg>,
    rx: UnboundedReceiver<CloudMsg>,
}

/// A finished cloud operation, delivered back to the sync loop. Responses
/// only — requests are made via the `spawn_*` helpers. Errors cross the
/// channel as display strings; the event loop puts them on the statusline.
#[derive(Debug)]
pub enum CloudMsg {
    /// `GET /notes` + `GET /teams` + per-team notes, fetched in one task.
    Lists(Result<CloudLists, String>),
    /// A single note fetch finished. `id` lets the receiver drop responses
    /// that arrive after the user navigated elsewhere.
    Note {
        id: String,
        intent: FetchIntent,
        result: Result<FetchedNote, String>,
    },
    /// A content PATCH finished. The live API returns `202 Accepted` with
    /// an empty body, so success carries the content the server accepted
    /// (i.e. what we sent) for cache/dirty bookkeeping.
    Saved {
        id: String,
        /// `Some(path)` when this save reflects the linked local file at `path`,
        /// so its sync base should advance to the saved content on success.
        /// `None` for cloud-only edits (editor/checkbox toggles on a
        /// `CloudNote`) — those must never touch a linked file's base, or
        /// they'd revert it on the next sync. Path-keyed so two files linked to
        /// the same note id keep independent bases.
        base_file: Option<PathBuf>,
        result: Result<String, String>,
    },
    /// A `POST /notes` (or team variant) finished.
    Created {
        intent: CreateIntent,
        result: Result<Box<SingleNote>, String>,
    },
    /// A `DELETE` finished. Title is carried through for the status message.
    Deleted {
        id: String,
        title: String,
        result: Result<(), String>,
    },
    /// A read-permission PATCH (publish/unpublish) finished. Like `Saved`,
    /// the response body is empty — success carries the permission now in
    /// effect.
    PermissionSet {
        id: String,
        result: Result<NotePermissionRole, String>,
    },
}

/// Why a note fetch was issued — tells the receiver what to do with the body.
#[derive(Debug, Clone)]
pub enum FetchIntent {
    /// Open the note in the Reader at `scroll` once it arrives.
    OpenReader { scroll: u16 },
    /// Write the note's content to a local file.
    DownloadTo(PathBuf),
    /// Conditional GET against a cached copy; 304 means "still current".
    Revalidate { etag: String },
    /// Bidirectional sync: the fetched upstream content is three-way merged
    /// against the linked local file at `path` and its cached base.
    SyncLocal { path: PathBuf },
}

/// What to do with a freshly created note.
#[derive(Debug, Clone)]
pub enum CreateIntent {
    /// Plain `n`ew note — open it in the reader.
    Blank,
    /// Created by pushing a local file's content up.
    PushedFrom(PathBuf),
}

/// Body of a note fetch: fresh content + ETag, or a 304.
#[derive(Debug)]
pub enum FetchedNote {
    Fresh {
        note: Box<SingleNote>,
        etag: Option<String>,
    },
    NotModified,
}

/// Everything the cloud browser lists: own notes plus each team's notes.
#[derive(Debug)]
pub struct CloudLists {
    pub notes: Vec<Note>,
    pub teams: Vec<TeamNotes>,
}

#[derive(Debug)]
pub struct TeamNotes {
    pub team: Team,
    pub notes: Vec<Note>,
}

/// A note body cached after a fetch, with the ETag for revalidation.
#[derive(Debug, Clone)]
pub struct CachedNote {
    pub note: SingleNote,
    pub etag: Option<String>,
}

impl CloudContext {
    /// Resolve credentials from the standard CLI config chain
    /// (`~/.hackmd/config.json` + env vars) and build a context on `handle`.
    /// A missing token is NOT an error — the context comes up disconnected
    /// and the TUI reports "not logged in" if cloud mode is requested.
    pub fn init(handle: Handle) -> Self {
        let client = crate::cli::config::effective(None, None, None)
            .ok()
            .and_then(|eff| {
                let token = eff.token?;
                Client::with_endpoint(token, eff.endpoint).ok()
            });
        Self::new(handle, client)
    }

    /// Build from an already-configured client (the `hackmd tui` path).
    pub fn with_client(client: Client, handle: Handle) -> Self {
        Self::new(handle, Some(client))
    }

    /// Build on `handle` with an optional client.
    pub fn new(handle: Handle, client: Option<Client>) -> Self {
        Self::build(Some(handle), client)
    }

    /// No runtime, no client. For tests and for code paths that must build
    /// an `App` without any tokio runtime alive.
    pub fn disconnected() -> Self {
        Self::build(None, None)
    }

    fn build(handle: Option<Handle>, client: Option<Client>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            handle,
            client,
            tx,
            rx,
        }
    }

    /// True when cloud operations can actually be spawned.
    pub fn is_connected(&self) -> bool {
        self.handle.is_some() && self.client.is_some()
    }

    /// Non-blocking receive of the next finished operation, if any.
    pub fn try_recv(&mut self) -> Option<CloudMsg> {
        self.rx.try_recv().ok()
    }

    /// Spawn `task(client, tx)` on the runtime. Returns `false` (and spawns
    /// nothing) when disconnected, so callers can show a login hint instead.
    fn spawn_with<F, Fut>(&self, task: F) -> bool
    where
        F: FnOnce(Client, UnboundedSender<CloudMsg>) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let (Some(handle), Some(client)) = (self.handle.as_ref(), self.client.clone()) else {
            return false;
        };
        handle.spawn(task(client, self.tx.clone()));
        true
    }

    /// Fetch own notes + teams + per-team notes in one task → `Lists`.
    pub fn spawn_fetch_lists(&self) -> bool {
        self.spawn_with(|client, tx| async move {
            let result = fetch_lists(&client).await;
            let _ = tx.send(CloudMsg::Lists(result));
        })
    }

    /// Fetch one note (ETag-aware when the intent is `Revalidate`) → `Note`.
    pub fn spawn_fetch_note(&self, id: String, intent: FetchIntent) -> bool {
        self.spawn_with(move |client, tx| async move {
            let etag = match &intent {
                FetchIntent::Revalidate { etag } => Some(etag.clone()),
                _ => None,
            };
            let result = match client.note(&id, etag.as_deref()).await {
                Ok(CachedResponse::Modified { body, etag }) => Ok(FetchedNote::Fresh {
                    note: Box::new(body),
                    etag,
                }),
                Ok(CachedResponse::NotModified) => Ok(FetchedNote::NotModified),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(CloudMsg::Note { id, intent, result });
        })
    }

    /// PATCH a note's content (team variant when `team_path` is set) → `Saved`.
    /// `base_file` is echoed back so the handler knows which linked file's sync
    /// base (if any) to advance on success.
    pub fn spawn_save(
        &self,
        id: String,
        team_path: Option<String>,
        content: String,
        base_file: Option<PathBuf>,
    ) -> bool {
        self.spawn_with(move |client, tx| async move {
            let res = match &team_path {
                Some(tp) => {
                    client
                        .update_team_note_content(tp, &id, Some(content.clone()))
                        .await
                }
                None => client.update_note_content(&id, Some(content.clone())).await,
            };
            let result = res.map(|_| content).map_err(|e| e.to_string());
            let _ = tx.send(CloudMsg::Saved {
                id,
                base_file,
                result,
            });
        })
    }

    /// POST a new note (team variant when `team_path` is set) → `Created`.
    pub fn spawn_create(
        &self,
        team_path: Option<String>,
        opts: CreateNoteOptions,
        intent: CreateIntent,
    ) -> bool {
        self.spawn_with(move |client, tx| async move {
            let res = match &team_path {
                Some(tp) => client.create_team_note(tp, opts).await,
                None => client.create_note(opts).await,
            };
            let result = res.map(Box::new).map_err(|e| e.to_string());
            let _ = tx.send(CloudMsg::Created { intent, result });
        })
    }

    /// DELETE a note (team variant when `team_path` is set) → `Deleted`.
    pub fn spawn_delete(&self, id: String, title: String, team_path: Option<String>) -> bool {
        self.spawn_with(move |client, tx| async move {
            let res = match &team_path {
                Some(tp) => client.delete_team_note(tp, &id).await,
                None => client.delete_note(&id).await,
            };
            let result = res.map_err(|e| e.to_string());
            let _ = tx.send(CloudMsg::Deleted { id, title, result });
        })
    }

    /// PATCH `readPermission` — `guest` publishes, `owner` unpublishes
    /// (the v1 API has no dedicated publish endpoint) → `PermissionSet`.
    pub fn spawn_set_read_permission(
        &self,
        id: String,
        team_path: Option<String>,
        perm: NotePermissionRole,
    ) -> bool {
        self.spawn_with(move |client, tx| async move {
            let opts = UpdateNoteOptions {
                read_permission: Some(perm),
                ..Default::default()
            };
            let res = match &team_path {
                Some(tp) => client.update_team_note(tp, &id, opts).await,
                None => client.update_note(&id, opts).await,
            };
            let result = res.map(|_| perm).map_err(|e| e.to_string());
            let _ = tx.send(CloudMsg::PermissionSet { id, result });
        })
    }
}

async fn fetch_lists(client: &Client) -> Result<CloudLists, String> {
    let notes = client.notes().await.map_err(|e| e.to_string())?;
    let teams = client.teams().await.map_err(|e| e.to_string())?;
    let mut team_lists = Vec::with_capacity(teams.len());
    for team in teams {
        let notes = client
            .team_notes(&team.path)
            .await
            .map_err(|e| e.to_string())?;
        team_lists.push(TeamNotes { team, notes });
    }
    Ok(CloudLists {
        notes,
        teams: team_lists,
    })
}

/// Cloud-side state hanging off `App`: the context, list/note caches, and
/// in-flight bookkeeping. All mutation happens on the TUI thread.
pub struct CloudState {
    pub ctx: CloudContext,
    /// Browser content. `None` until the first `Lists` response lands.
    pub lists: Option<CloudLists>,
    /// Fetched note bodies, keyed by note id, with ETags for revalidation.
    pub note_cache: HashMap<String, CachedNote>,
    /// Note ids with a content PATCH in flight — guards double-saves.
    pub saving: HashSet<String>,
    /// Count of spawned-but-unanswered operations; statusline shows a
    /// syncing indicator while non-zero.
    pub pending: u32,
    /// `(note id, scroll)` of a navigation waiting on its fetch. History is
    /// pushed only when the response completes the move, so a failed fetch
    /// leaves history clean; responses for any other id are stale and the
    /// nav stays armed.
    pub pending_nav: Option<(String, u16)>,
}

impl CloudState {
    pub fn new(ctx: CloudContext) -> Self {
        Self {
            ctx,
            lists: None,
            note_cache: HashMap::new(),
            saving: HashSet::new(),
            pending: 0,
            pending_nav: None,
        }
    }

    /// True when cloud operations can be issued.
    pub fn is_connected(&self) -> bool {
        self.ctx.is_connected()
    }

    /// Request the full lists refresh. Returns `false` when disconnected.
    pub fn request_lists(&mut self) -> bool {
        self.track(self.ctx.spawn_fetch_lists())
    }

    /// Request a note fetch. Returns `false` when disconnected.
    pub fn request_note(&mut self, id: String, intent: FetchIntent) -> bool {
        let spawned = self.ctx.spawn_fetch_note(id, intent);
        self.track(spawned)
    }

    /// Request a content save; tracks the id in `saving`. Returns `false`
    /// when disconnected or when a save for this id is already in flight.
    /// `base_file` flows through to the `Saved` handler (see
    /// [`CloudMsg::Saved`]).
    pub fn request_save(
        &mut self,
        id: String,
        team_path: Option<String>,
        content: String,
        base_file: Option<PathBuf>,
    ) -> bool {
        if self.saving.contains(&id) {
            return false;
        }
        let spawned = self
            .ctx
            .spawn_save(id.clone(), team_path, content, base_file);
        if spawned {
            self.saving.insert(id);
        }
        self.track(spawned)
    }

    /// Request note creation. Returns `false` when disconnected.
    pub fn request_create(
        &mut self,
        team_path: Option<String>,
        opts: CreateNoteOptions,
        intent: CreateIntent,
    ) -> bool {
        let spawned = self.ctx.spawn_create(team_path, opts, intent);
        self.track(spawned)
    }

    /// Request note deletion. Returns `false` when disconnected.
    pub fn request_delete(&mut self, id: String, title: String, team_path: Option<String>) -> bool {
        let spawned = self.ctx.spawn_delete(id, title, team_path);
        self.track(spawned)
    }

    /// Request a publish/unpublish permission change. Returns `false` when
    /// disconnected.
    pub fn request_set_read_permission(
        &mut self,
        id: String,
        team_path: Option<String>,
        perm: NotePermissionRole,
    ) -> bool {
        let spawned = self.ctx.spawn_set_read_permission(id, team_path, perm);
        self.track(spawned)
    }

    fn track(&mut self, spawned: bool) -> bool {
        if spawned {
            self.pending += 1;
        }
        spawned
    }

    /// Bookkeeping shared by every response: drop the pending count.
    pub fn note_response_received(&mut self) {
        self.pending = self.pending.saturating_sub(1);
    }
}

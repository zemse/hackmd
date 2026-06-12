//! Clap-based CLI surface mirroring `@hackmd/hackmd-cli`.
//!
//! Module entry-point: [`Cli::parse`] (via clap) yields a [`Cli`] value
//! that [`dispatch`] consumes to drive command handlers under
//! [`commands`].
//!
//! ## Compatibility with the original `hackmd-cli`
//!
//! Command lines written for [`@hackmd/hackmd-cli`] work unchanged:
//! every camelCase flag (`--noteId`, `--teamPath`, `--readPermission`,
//! …) is accepted as a hidden alias of its kebab-case form, bare
//! `notes` / `team-notes` list (with `--noteId` fetching a single
//! note), and the `folders` / `team-folders` groups mirror the
//! upstream surface. Kebab-case flags and the `list`/`get`
//! subcommands are this crate's extensions.
//!
//! [`@hackmd/hackmd-cli`]: https://github.com/hackmdio/hackmd-cli

pub mod commands;
pub mod config;
pub mod editor;
pub mod output;

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::error::Result;
use crate::types::{CommentPermissionType, NotePermissionRole};

use self::output::OutputOpts;

/// Top-level CLI. Mirrors the `hackmd-cli` upstream surface.
#[derive(Debug, Parser)]
#[command(
    name = "hackmd",
    version,
    disable_version_flag = true,
    about = "HackMD CLI — manage notes, teams, and folders from the terminal",
    long_about = None
)]
pub struct Cli {
    /// Print version.
    ///
    /// Hand-rolled so `-v` works alongside clap's usual `-V`
    /// (`hackmd-cli` accepts `-v`).
    #[arg(short = 'v', short_alias = 'V', long = "version", action = clap::ArgAction::Version)]
    pub version: Option<bool>,

    /// Override the config directory (defaults to `~/.hackmd`).
    #[arg(long = "config-dir", env = config::ENV_CONFIG_DIR, global = true)]
    pub config_dir: Option<PathBuf>,

    /// Override the API endpoint URL.
    #[arg(long = "endpoint", env = config::ENV_ENDPOINT_URL, global = true)]
    pub endpoint: Option<String>,

    /// Override the access token.
    #[arg(long = "token", env = config::ENV_ACCESS_TOKEN, global = true, hide_env_values = true)]
    pub token: Option<String>,

    /// File or directory to open in the TUI (e.g. `hackmd .`). With no
    /// path and no subcommand, opens the TUI on the HackMD cloud notes.
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Top-level subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Login to HackMD (prompts for an access token).
    Login,
    /// Clear the stored access token.
    Logout,
    /// Print the authenticated user.
    Whoami(WhoamiArgs),
    /// List the user's browse history.
    History(HistoryArgs),
    /// Export a note's raw markdown content to stdout.
    Export(ExportArgs),
    /// List teams.
    Teams(TeamsArgs),
    /// Manage notes (bare = list, or list/get/create/update/delete).
    Notes(NotesArgs),
    /// Manage team notes (bare = list, or list/create/update/delete).
    #[command(name = "team-notes")]
    TeamNotes(TeamNotesArgs),
    /// Manage folders (bare = list, or create/update/delete/order).
    Folders(FoldersArgs),
    /// Manage team folders (bare = list, or create/update/delete/order).
    #[command(name = "team-folders")]
    TeamFolders(TeamFoldersArgs),
    /// Create a new note and open it in the TUI editor.
    #[command(visible_alias = "create")]
    New(NewArgs),
    /// Print version.
    Version,
    /// Launch the TUI (requires the `tui` feature at build time).
    Tui,
}

// ─── Top-level command argument structs ────────────────────────────────────

#[derive(Debug, Args)]
pub struct WhoamiArgs {
    #[command(flatten)]
    pub output: OutputOpts,
}

#[derive(Debug, Args)]
pub struct HistoryArgs {
    #[command(flatten)]
    pub output: OutputOpts,
}

#[derive(Debug, Args)]
pub struct ExportArgs {
    /// HackMD note id.
    #[arg(long = "note-id", alias = "noteId")]
    pub note_id: String,
}

#[derive(Debug, Args)]
pub struct TeamsArgs {
    #[command(flatten)]
    pub output: OutputOpts,
}

#[derive(Debug, Args)]
pub struct NewArgs {
    /// Note title — quotes optional, words are joined with spaces.
    #[arg(value_name = "TITLE")]
    pub title: Vec<String>,
}

// ─── `notes` ────────────────────────────────────────────────────────────────

#[derive(Debug, Args)]
pub struct NotesArgs {
    /// HackMD note id — with no subcommand, fetch this note instead of
    /// listing (the original `hackmd-cli notes --noteId=…` form).
    #[arg(long = "note-id", alias = "noteId")]
    pub note_id: Option<String>,

    #[command(flatten)]
    pub output: OutputOpts,

    #[command(subcommand)]
    pub action: Option<NotesCmd>,
}

#[derive(Debug, Subcommand)]
pub enum NotesCmd {
    /// List the user's notes.
    List(NotesListArgs),
    /// Fetch a single note by id.
    Get(NotesGetArgs),
    /// Create a new note.
    Create(NotesCreateArgs),
    /// Update an existing note's content and metadata.
    Update(NotesUpdateArgs),
    /// Delete a note.
    Delete(NotesDeleteArgs),
}

#[derive(Debug, Args)]
pub struct NotesListArgs {
    #[command(flatten)]
    pub output: OutputOpts,
}

#[derive(Debug, Args)]
pub struct NotesGetArgs {
    #[arg(long = "note-id", alias = "noteId")]
    pub note_id: String,
    #[command(flatten)]
    pub output: OutputOpts,
}

#[derive(Debug, Args)]
pub struct NotesCreateArgs {
    /// Note title.
    #[arg(long = "title")]
    pub title: Option<String>,
    /// Note content (raw markdown).
    #[arg(long = "content")]
    pub content: Option<String>,
    /// Comma-separated tags (e.g. `tag1,tag2`).
    #[arg(long = "tags")]
    pub tags: Option<String>,
    /// `owner`, `signed_in`, or `guest`.
    #[arg(long = "read-permission", alias = "readPermission", value_enum)]
    pub read_permission: Option<PermArg>,
    /// `owner`, `signed_in`, or `guest`.
    #[arg(long = "write-permission", alias = "writePermission", value_enum)]
    pub write_permission: Option<PermArg>,
    /// `disabled`, `forbidden`, `owners`, `signed_in_users`, or `everyone`.
    #[arg(long = "comment-permission", alias = "commentPermission", value_enum)]
    pub comment_permission: Option<CommentArg>,
    /// Parent folder id.
    #[arg(long = "parent-folder-id", alias = "parentFolderId")]
    pub parent_folder_id: Option<String>,
    /// Open `$EDITOR` to author the content interactively.
    #[arg(short = 'e', long = "editor")]
    pub editor: bool,
    #[command(flatten)]
    pub output: OutputOpts,
}

#[derive(Debug, Args)]
pub struct NotesUpdateArgs {
    #[arg(long = "note-id", alias = "noteId")]
    pub note_id: String,
    /// Replacement markdown content.
    #[arg(long = "content")]
    pub content: Option<String>,
    /// Comma-separated tags (e.g. `tag1,tag2`).
    #[arg(long = "tags")]
    pub tags: Option<String>,
    /// `owner`, `signed_in`, or `guest`.
    #[arg(long = "read-permission", alias = "readPermission", value_enum)]
    pub read_permission: Option<PermArg>,
    /// `owner`, `signed_in`, or `guest`.
    #[arg(long = "write-permission", alias = "writePermission", value_enum)]
    pub write_permission: Option<PermArg>,
    /// Note permalink.
    #[arg(long = "permalink")]
    pub permalink: Option<String>,
    /// Parent folder id.
    #[arg(long = "parent-folder-id", alias = "parentFolderId")]
    pub parent_folder_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct NotesDeleteArgs {
    #[arg(long = "note-id", alias = "noteId")]
    pub note_id: String,
}

// ─── `team-notes` ───────────────────────────────────────────────────────────

#[derive(Debug, Args)]
pub struct TeamNotesArgs {
    /// HackMD team path. Required for every team-notes action.
    #[arg(long = "team-path", alias = "teamPath", global = true)]
    pub team_path: Option<String>,

    #[command(flatten)]
    pub output: OutputOpts,

    #[command(subcommand)]
    pub action: Option<TeamNotesCmd>,
}

#[derive(Debug, Subcommand)]
pub enum TeamNotesCmd {
    /// List notes belonging to the given team.
    List(TeamNotesListArgs),
    /// Create a note in the given team.
    Create(TeamNotesCreateArgs),
    /// Update a team note's content and metadata.
    Update(TeamNotesUpdateArgs),
    /// Delete a team note.
    Delete(TeamNotesDeleteArgs),
}

#[derive(Debug, Args)]
pub struct TeamNotesListArgs {
    #[command(flatten)]
    pub output: OutputOpts,
}

#[derive(Debug, Args)]
pub struct TeamNotesCreateArgs {
    #[arg(long = "title")]
    pub title: Option<String>,
    #[arg(long = "content")]
    pub content: Option<String>,
    /// Comma-separated tags (e.g. `tag1,tag2`).
    #[arg(long = "tags")]
    pub tags: Option<String>,
    #[arg(long = "read-permission", alias = "readPermission", value_enum)]
    pub read_permission: Option<PermArg>,
    #[arg(long = "write-permission", alias = "writePermission", value_enum)]
    pub write_permission: Option<PermArg>,
    #[arg(long = "comment-permission", alias = "commentPermission", value_enum)]
    pub comment_permission: Option<CommentArg>,
    /// Parent folder id.
    #[arg(long = "parent-folder-id", alias = "parentFolderId")]
    pub parent_folder_id: Option<String>,
    #[arg(short = 'e', long = "editor")]
    pub editor: bool,
    #[command(flatten)]
    pub output: OutputOpts,
}

#[derive(Debug, Args)]
pub struct TeamNotesUpdateArgs {
    #[arg(long = "note-id", alias = "noteId")]
    pub note_id: String,
    #[arg(long = "content")]
    pub content: Option<String>,
    /// Comma-separated tags (e.g. `tag1,tag2`).
    #[arg(long = "tags")]
    pub tags: Option<String>,
    #[arg(long = "read-permission", alias = "readPermission", value_enum)]
    pub read_permission: Option<PermArg>,
    #[arg(long = "write-permission", alias = "writePermission", value_enum)]
    pub write_permission: Option<PermArg>,
    /// Note permalink.
    #[arg(long = "permalink")]
    pub permalink: Option<String>,
    /// Parent folder id.
    #[arg(long = "parent-folder-id", alias = "parentFolderId")]
    pub parent_folder_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct TeamNotesDeleteArgs {
    #[arg(long = "note-id", alias = "noteId")]
    pub note_id: String,
}

// ─── `folders` / `team-folders` ────────────────────────────────────────────

#[derive(Debug, Args)]
pub struct FoldersArgs {
    /// HackMD folder id — with no subcommand, fetch this folder instead
    /// of listing.
    #[arg(long = "folder-id", alias = "folderId")]
    pub folder_id: Option<String>,

    #[command(flatten)]
    pub output: OutputOpts,

    #[command(subcommand)]
    pub action: Option<FoldersCmd>,
}

#[derive(Debug, Subcommand)]
pub enum FoldersCmd {
    /// Create a folder.
    Create(FoldersCreateArgs),
    /// Update a folder.
    Update(FoldersUpdateArgs),
    /// Delete a folder.
    Delete(FoldersDeleteArgs),
    /// Get or update the folder order.
    Order(FoldersOrderArgs),
}

#[derive(Debug, Args)]
pub struct FoldersCreateArgs {
    /// Folder name.
    #[arg(long = "name")]
    pub name: Option<String>,
    /// Folder description.
    #[arg(long = "description")]
    pub description: Option<String>,
    /// Folder icon.
    #[arg(long = "icon")]
    pub icon: Option<String>,
    /// Folder color.
    #[arg(long = "color")]
    pub color: Option<String>,
    /// Parent folder id.
    #[arg(long = "parent-folder-id", alias = "parentFolderId")]
    pub parent_folder_id: Option<String>,
    #[command(flatten)]
    pub output: OutputOpts,
}

#[derive(Debug, Args)]
pub struct FoldersUpdateArgs {
    #[arg(long = "folder-id", alias = "folderId")]
    pub folder_id: String,
    /// Folder name.
    #[arg(long = "name")]
    pub name: Option<String>,
    /// Folder description.
    #[arg(long = "description")]
    pub description: Option<String>,
    /// Folder icon.
    #[arg(long = "icon")]
    pub icon: Option<String>,
    /// Folder color.
    #[arg(long = "color")]
    pub color: Option<String>,
    /// Parent folder id.
    #[arg(long = "parent-folder-id", alias = "parentFolderId")]
    pub parent_folder_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct FoldersDeleteArgs {
    #[arg(long = "folder-id", alias = "folderId")]
    pub folder_id: String,
}

#[derive(Debug, Args)]
pub struct FoldersOrderArgs {
    /// Folder order JSON, e.g. `{"root":["folder-id"]}`. Omit to print
    /// the current order.
    #[arg(long = "order")]
    pub order: Option<String>,
}

#[derive(Debug, Args)]
pub struct TeamFoldersArgs {
    /// HackMD team path. Required for every team-folders action.
    #[arg(long = "team-path", alias = "teamPath", global = true)]
    pub team_path: Option<String>,

    /// HackMD folder id — with no subcommand, fetch this folder instead
    /// of listing.
    #[arg(long = "folder-id", alias = "folderId")]
    pub folder_id: Option<String>,

    #[command(flatten)]
    pub output: OutputOpts,

    #[command(subcommand)]
    pub action: Option<FoldersCmd>,
}

// ─── Enum adapters for clap ────────────────────────────────────────────────

/// CLI-friendly mirror of [`NotePermissionRole`]. Kept separate from the
/// SDK enum so we can use `clap::ValueEnum` without polluting the public
/// SDK surface.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum PermArg {
    Owner,
    SignedIn,
    Guest,
}

impl From<PermArg> for NotePermissionRole {
    fn from(v: PermArg) -> Self {
        match v {
            PermArg::Owner => NotePermissionRole::Owner,
            PermArg::SignedIn => NotePermissionRole::SignedIn,
            PermArg::Guest => NotePermissionRole::Guest,
        }
    }
}

/// CLI mirror of [`CommentPermissionType`].
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum CommentArg {
    Disabled,
    Forbidden,
    Owners,
    SignedInUsers,
    Everyone,
}

impl From<CommentArg> for CommentPermissionType {
    fn from(v: CommentArg) -> Self {
        match v {
            CommentArg::Disabled => CommentPermissionType::Disabled,
            CommentArg::Forbidden => CommentPermissionType::Forbidden,
            CommentArg::Owners => CommentPermissionType::Owners,
            CommentArg::SignedInUsers => CommentPermissionType::SignedInUsers,
            CommentArg::Everyone => CommentPermissionType::Everyone,
        }
    }
}

// ─── Dispatch ──────────────────────────────────────────────────────────────

/// `--tags tag1,tag2` → `vec!["tag1", "tag2"]`.
fn parse_tags(tags: Option<String>) -> Option<Vec<String>> {
    tags.map(|s| s.split(',').map(|t| t.trim().to_string()).collect())
}

fn create_note_options(
    title: Option<String>,
    content: Option<String>,
    tags: Option<String>,
    read_permission: Option<PermArg>,
    write_permission: Option<PermArg>,
    comment_permission: Option<CommentArg>,
    parent_folder_id: Option<String>,
) -> crate::types::CreateNoteOptions {
    crate::types::CreateNoteOptions {
        title,
        content,
        tags: parse_tags(tags),
        read_permission: read_permission.map(Into::into),
        write_permission: write_permission.map(Into::into),
        comment_permission: comment_permission.map(Into::into),
        parent_folder_id,
        ..Default::default()
    }
}

fn update_note_options(
    content: Option<String>,
    tags: Option<String>,
    read_permission: Option<PermArg>,
    write_permission: Option<PermArg>,
    permalink: Option<String>,
    parent_folder_id: Option<String>,
) -> crate::types::UpdateNoteOptions {
    crate::types::UpdateNoteOptions {
        content,
        title: None,
        tags: parse_tags(tags),
        read_permission: read_permission.map(Into::into),
        write_permission: write_permission.map(Into::into),
        permalink,
        parent_folder_id,
    }
}

fn create_folder_body(a: &FoldersCreateArgs) -> crate::types::CreateUserFolderBody {
    crate::types::CreateUserFolderBody {
        name: a.name.clone(),
        description: a.description.clone(),
        icon: a.icon.clone(),
        color: a.color.clone(),
        parent_folder_id: a.parent_folder_id.clone(),
    }
}

/// CLI flags only express "set to this value" — the outer `Some(None)`
/// ("clear this field") form of the PATCH body isn't reachable from
/// flags, matching the original CLI.
fn update_folder_body(a: &FoldersUpdateArgs) -> crate::types::UpdateUserFolderBody {
    crate::types::UpdateUserFolderBody {
        name: a.name.clone(),
        description: a.description.clone().map(Some),
        icon: a.icon.clone().map(Some),
        color: a.color.clone().map(Some),
        parent_folder_id: a.parent_folder_id.clone().map(Some),
    }
}

fn require_team_path(team_path: Option<String>, group: &str) -> Result<String> {
    team_path.ok_or_else(|| {
        crate::error::Error::Config(format!("--team-path is required for `{group}` commands"))
    })
}

/// Run the parsed CLI value.
pub async fn dispatch(cli: Cli) -> Result<()> {
    let config_dir = cli.config_dir.as_deref();
    let endpoint = cli.endpoint.as_deref();
    let token = cli.token.as_deref();

    let Some(command) = cli.command else {
        // Bare `hackmd` → TUI on cloud notes; `hackmd <path>` → TUI on the
        // local file/dir.
        return commands::tui::run(config_dir, endpoint, token, cli.path).await;
    };

    match command {
        Command::Login => commands::auth::login(config_dir, endpoint, token).await,
        Command::Logout => commands::auth::logout(config_dir),
        Command::Whoami(args) => {
            commands::auth::whoami(config_dir, endpoint, token, &args.output).await
        }
        Command::History(args) => {
            commands::history::run(config_dir, endpoint, token, &args.output).await
        }
        Command::Export(args) => {
            commands::export::run(config_dir, endpoint, token, &args.note_id).await
        }
        Command::Teams(args) => {
            commands::teams::run(config_dir, endpoint, token, &args.output).await
        }
        Command::Notes(n) => match n.action {
            // Original-CLI form: bare `notes` lists, `notes --noteId` gets.
            None => match n.note_id {
                Some(id) => commands::notes::get(config_dir, endpoint, token, &id, &n.output).await,
                None => commands::notes::list(config_dir, endpoint, token, &n.output).await,
            },
            Some(NotesCmd::List(a)) => {
                commands::notes::list(config_dir, endpoint, token, &a.output).await
            }
            Some(NotesCmd::Get(a)) => {
                commands::notes::get(config_dir, endpoint, token, &a.note_id, &a.output).await
            }
            Some(NotesCmd::Create(a)) => {
                let opts = create_note_options(
                    a.title,
                    a.content,
                    a.tags,
                    a.read_permission,
                    a.write_permission,
                    a.comment_permission,
                    a.parent_folder_id,
                );
                commands::notes::create(config_dir, endpoint, token, opts, a.editor, &a.output)
                    .await
            }
            Some(NotesCmd::Update(a)) => {
                let opts = update_note_options(
                    a.content,
                    a.tags,
                    a.read_permission,
                    a.write_permission,
                    a.permalink,
                    a.parent_folder_id,
                );
                commands::notes::update(config_dir, endpoint, token, &a.note_id, opts).await
            }
            Some(NotesCmd::Delete(a)) => {
                commands::notes::delete(config_dir, endpoint, token, &a.note_id).await
            }
        },
        Command::TeamNotes(t) => {
            let team_path = require_team_path(t.team_path.clone(), "team-notes")?;
            match t.action {
                // Original-CLI form: bare `team-notes --teamPath` lists.
                None => {
                    commands::team_notes::list(config_dir, endpoint, token, &team_path, &t.output)
                        .await
                }
                Some(TeamNotesCmd::List(a)) => {
                    commands::team_notes::list(config_dir, endpoint, token, &team_path, &a.output)
                        .await
                }
                Some(TeamNotesCmd::Create(a)) => {
                    let opts = create_note_options(
                        a.title,
                        a.content,
                        a.tags,
                        a.read_permission,
                        a.write_permission,
                        a.comment_permission,
                        a.parent_folder_id,
                    );
                    commands::team_notes::create(
                        config_dir, endpoint, token, &team_path, opts, a.editor, &a.output,
                    )
                    .await
                }
                Some(TeamNotesCmd::Update(a)) => {
                    let opts = update_note_options(
                        a.content,
                        a.tags,
                        a.read_permission,
                        a.write_permission,
                        a.permalink,
                        a.parent_folder_id,
                    );
                    commands::team_notes::update(
                        config_dir, endpoint, token, &team_path, &a.note_id, opts,
                    )
                    .await
                }
                Some(TeamNotesCmd::Delete(a)) => {
                    commands::team_notes::delete(
                        config_dir, endpoint, token, &team_path, &a.note_id,
                    )
                    .await
                }
            }
        }
        Command::Folders(f) => match f.action {
            None => match f.folder_id {
                Some(id) => {
                    commands::folders::get(config_dir, endpoint, token, &id, &f.output).await
                }
                None => commands::folders::list(config_dir, endpoint, token, &f.output).await,
            },
            Some(FoldersCmd::Create(a)) => {
                let body = create_folder_body(&a);
                commands::folders::create(config_dir, endpoint, token, body, &a.output).await
            }
            Some(FoldersCmd::Update(a)) => {
                let body = update_folder_body(&a);
                commands::folders::update(config_dir, endpoint, token, &a.folder_id, body).await
            }
            Some(FoldersCmd::Delete(a)) => {
                commands::folders::delete(config_dir, endpoint, token, &a.folder_id).await
            }
            Some(FoldersCmd::Order(a)) => {
                commands::folders::order(config_dir, endpoint, token, a.order).await
            }
        },
        Command::TeamFolders(f) => {
            let team_path = require_team_path(f.team_path.clone(), "team-folders")?;
            match f.action {
                None => match f.folder_id {
                    Some(id) => {
                        commands::team_folders::get(
                            config_dir, endpoint, token, &team_path, &id, &f.output,
                        )
                        .await
                    }
                    None => {
                        commands::team_folders::list(
                            config_dir, endpoint, token, &team_path, &f.output,
                        )
                        .await
                    }
                },
                Some(FoldersCmd::Create(a)) => {
                    let body = create_folder_body(&a);
                    commands::team_folders::create(
                        config_dir, endpoint, token, &team_path, body, &a.output,
                    )
                    .await
                }
                Some(FoldersCmd::Update(a)) => {
                    let body = update_folder_body(&a);
                    commands::team_folders::update(
                        config_dir,
                        endpoint,
                        token,
                        &team_path,
                        &a.folder_id,
                        body,
                    )
                    .await
                }
                Some(FoldersCmd::Delete(a)) => {
                    commands::team_folders::delete(
                        config_dir,
                        endpoint,
                        token,
                        &team_path,
                        &a.folder_id,
                    )
                    .await
                }
                Some(FoldersCmd::Order(a)) => {
                    commands::team_folders::order(config_dir, endpoint, token, &team_path, a.order)
                        .await
                }
            }
        }
        Command::New(a) => commands::tui::new_note(config_dir, endpoint, token, a.title).await,
        Command::Version => {
            println!("hackmd {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Tui => commands::tui::run(config_dir, endpoint, token, cli.path).await,
    }
}

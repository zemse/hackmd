//! Clap-based CLI surface mirroring `@hackmd/hackmd-cli`.
//!
//! Module entry-point: [`Cli::parse`] (via clap) yields a [`Cli`] value
//! that [`dispatch`] consumes to drive command handlers under
//! [`commands`].

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
    about = "HackMD CLI — manage notes, teams, and folders from the terminal",
    long_about = None
)]
pub struct Cli {
    /// Override the config directory (defaults to `~/.hackmd`).
    #[arg(long = "config-dir", env = config::ENV_CONFIG_DIR, global = true)]
    pub config_dir: Option<PathBuf>,

    /// Override the API endpoint URL.
    #[arg(long = "endpoint", env = config::ENV_ENDPOINT_URL, global = true)]
    pub endpoint: Option<String>,

    /// Override the access token.
    #[arg(long = "token", env = config::ENV_ACCESS_TOKEN, global = true, hide_env_values = true)]
    pub token: Option<String>,

    #[command(subcommand)]
    pub command: Command,
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
    /// Manage notes (list, get, create, update, delete).
    Notes(NotesArgs),
    /// Manage team notes (list, create, update, delete).
    #[command(name = "team-notes")]
    TeamNotes(TeamNotesArgs),
    /// Launch the TUI (placeholder — coming in v0.0.3).
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
    #[arg(long = "note-id")]
    pub note_id: String,
}

#[derive(Debug, Args)]
pub struct TeamsArgs {
    #[command(flatten)]
    pub output: OutputOpts,
}

// ─── `notes` ────────────────────────────────────────────────────────────────

#[derive(Debug, Args)]
pub struct NotesArgs {
    #[command(subcommand)]
    pub action: NotesCmd,
}

#[derive(Debug, Subcommand)]
pub enum NotesCmd {
    /// List the user's notes.
    List(NotesListArgs),
    /// Fetch a single note by id.
    Get(NotesGetArgs),
    /// Create a new note.
    Create(NotesCreateArgs),
    /// Update an existing note's content.
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
    #[arg(long = "note-id")]
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
    /// `owner`, `signed_in`, or `guest`.
    #[arg(long = "read-permission", value_enum)]
    pub read_permission: Option<PermArg>,
    /// `owner`, `signed_in`, or `guest`.
    #[arg(long = "write-permission", value_enum)]
    pub write_permission: Option<PermArg>,
    /// `disabled`, `forbidden`, `owners`, `signed_in_users`, or `everyone`.
    #[arg(long = "comment-permission", value_enum)]
    pub comment_permission: Option<CommentArg>,
    /// Open `$EDITOR` to author the content interactively.
    #[arg(short = 'e', long = "editor")]
    pub editor: bool,
    #[command(flatten)]
    pub output: OutputOpts,
}

#[derive(Debug, Args)]
pub struct NotesUpdateArgs {
    #[arg(long = "note-id")]
    pub note_id: String,
    /// Replacement markdown content.
    #[arg(long = "content")]
    pub content: Option<String>,
}

#[derive(Debug, Args)]
pub struct NotesDeleteArgs {
    #[arg(long = "note-id")]
    pub note_id: String,
}

// ─── `team-notes` ───────────────────────────────────────────────────────────

#[derive(Debug, Args)]
pub struct TeamNotesArgs {
    /// HackMD team path. Required for every team-notes subcommand.
    #[arg(long = "team-path", global = true)]
    pub team_path: Option<String>,

    #[command(subcommand)]
    pub action: TeamNotesCmd,
}

#[derive(Debug, Subcommand)]
pub enum TeamNotesCmd {
    /// List notes belonging to the given team.
    List(TeamNotesListArgs),
    /// Create a note in the given team.
    Create(TeamNotesCreateArgs),
    /// Update a team note's content.
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
    #[arg(long = "read-permission", value_enum)]
    pub read_permission: Option<PermArg>,
    #[arg(long = "write-permission", value_enum)]
    pub write_permission: Option<PermArg>,
    #[arg(long = "comment-permission", value_enum)]
    pub comment_permission: Option<CommentArg>,
    #[arg(short = 'e', long = "editor")]
    pub editor: bool,
    #[command(flatten)]
    pub output: OutputOpts,
}

#[derive(Debug, Args)]
pub struct TeamNotesUpdateArgs {
    #[arg(long = "note-id")]
    pub note_id: String,
    #[arg(long = "content")]
    pub content: Option<String>,
}

#[derive(Debug, Args)]
pub struct TeamNotesDeleteArgs {
    #[arg(long = "note-id")]
    pub note_id: String,
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

/// Run the parsed CLI value.
pub async fn dispatch(cli: Cli) -> Result<()> {
    let config_dir = cli.config_dir.as_deref();
    let endpoint = cli.endpoint.as_deref();
    let token = cli.token.as_deref();

    match cli.command {
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
            NotesCmd::List(a) => commands::notes::list(config_dir, endpoint, token, &a.output).await,
            NotesCmd::Get(a) => {
                commands::notes::get(config_dir, endpoint, token, &a.note_id, &a.output).await
            }
            NotesCmd::Create(a) => {
                commands::notes::create(
                    config_dir,
                    endpoint,
                    token,
                    a.title,
                    a.content,
                    a.read_permission.map(Into::into),
                    a.write_permission.map(Into::into),
                    a.comment_permission.map(Into::into),
                    a.editor,
                    &a.output,
                )
                .await
            }
            NotesCmd::Update(a) => {
                commands::notes::update(config_dir, endpoint, token, &a.note_id, a.content).await
            }
            NotesCmd::Delete(a) => {
                commands::notes::delete(config_dir, endpoint, token, &a.note_id).await
            }
        },
        Command::TeamNotes(t) => {
            let team_path = t.team_path.clone().ok_or_else(|| {
                crate::error::Error::Config(
                    "--team-path is required for `team-notes` subcommands".into(),
                )
            })?;
            match t.action {
                TeamNotesCmd::List(a) => {
                    commands::team_notes::list(config_dir, endpoint, token, &team_path, &a.output)
                        .await
                }
                TeamNotesCmd::Create(a) => {
                    commands::team_notes::create(
                        config_dir,
                        endpoint,
                        token,
                        &team_path,
                        a.title,
                        a.content,
                        a.read_permission.map(Into::into),
                        a.write_permission.map(Into::into),
                        a.comment_permission.map(Into::into),
                        a.editor,
                        &a.output,
                    )
                    .await
                }
                TeamNotesCmd::Update(a) => {
                    commands::team_notes::update(
                        config_dir,
                        endpoint,
                        token,
                        &team_path,
                        &a.note_id,
                        a.content,
                    )
                    .await
                }
                TeamNotesCmd::Delete(a) => {
                    commands::team_notes::delete(
                        config_dir,
                        endpoint,
                        token,
                        &team_path,
                        &a.note_id,
                    )
                    .await
                }
            }
        }
        Command::Tui => commands::tui::run(),
    }
}

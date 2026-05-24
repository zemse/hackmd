//! Terminal UI for browsing and editing HackMD notes.
//!
//! Gated behind the `tui` Cargo feature. Two-pane layout: note list on the
//! left, selected-note content preview on the right. Keybindings:
//!
//! | key             | action                                                       |
//! |-----------------|--------------------------------------------------------------|
//! | `j` / `Down`    | move selection down                                          |
//! | `k` / `Up`      | move selection up                                            |
//! | `Enter` / `o`   | load the selected note's full content into the right pane    |
//! | `r`             | refresh the note list                                        |
//! | `e`             | open the selected note in `$EDITOR`, save back via the SDK   |
//! | `q` / `Esc`     | quit                                                         |
//!
//! Only user notes are surfaced — team-notes and folders are not yet exposed
//! by the TUI (v0.0.2 scope; see `PLAN.md` C.11 / section 6).

pub mod app;
mod ui;

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;

use crate::Client;
use crate::error::{Error, Result};

pub use app::{Action, App, BackgroundMsg, EditorOutcome, handle_key};

/// RAII guard that always restores the terminal on drop (even on panic).
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().map_err(Error::Io)?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen).map_err(Error::Io)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Best-effort restore — never panic on the way out.
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

/// Entry point for `hackmd tui`. Owns the terminal, drives the event loop,
/// and tears the terminal back down on exit.
pub async fn run(client: Client) -> Result<()> {
    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).map_err(Error::Io)?;

    let mut app = App::new();
    let (tx, mut rx) = mpsc::unbounded_channel::<BackgroundMsg>();

    // Kick off the initial notes fetch.
    spawn_refresh(client.clone(), tx.clone());
    app.set_status("loading notes…");

    loop {
        terminal.draw(|f| ui::draw(f, &app)).map_err(Error::Io)?;

        // Drain any background messages without blocking the event loop.
        while let Ok(msg) = rx.try_recv() {
            app.apply_background(msg);
        }

        // Poll for input with a short timeout so background messages above
        // can land between keystrokes.
        if event::poll(Duration::from_millis(100)).map_err(Error::Io)? {
            match event::read().map_err(Error::Io)? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if let Some(action) = handle_key(&app, key) {
                        match dispatch_action(&mut app, action, &client, &tx).await {
                            ControlFlow::Continue => {}
                            ControlFlow::Quit => break,
                        }
                    }
                }
                Event::Resize(_, _) => { /* ratatui re-measures on next draw */ }
                _ => {}
            }
        }
    }

    Ok(())
}

enum ControlFlow {
    Continue,
    Quit,
}

async fn dispatch_action(
    app: &mut App,
    action: Action,
    client: &Client,
    tx: &mpsc::UnboundedSender<BackgroundMsg>,
) -> ControlFlow {
    match action {
        Action::Quit => ControlFlow::Quit,
        Action::Up => {
            app.move_up();
            ControlFlow::Continue
        }
        Action::Down => {
            app.move_down();
            ControlFlow::Continue
        }
        Action::Open => {
            if let Some(id) = app.selected_id() {
                app.set_status(&format!("loading note {id}…"));
                spawn_fetch_note(client.clone(), tx.clone(), id);
            } else {
                app.set_status("no note selected");
            }
            ControlFlow::Continue
        }
        Action::Refresh => {
            app.set_status("refreshing…");
            spawn_refresh(client.clone(), tx.clone());
            ControlFlow::Continue
        }
        Action::Edit => {
            if let Some(id) = app.selected_id() {
                // $EDITOR takes over the screen — drop and recreate raw-mode
                // around the editor invocation so the user sees their editor
                // cleanly.
                let outcome = run_editor_for(id.clone(), app.selected_content());
                match outcome {
                    EditorOutcome::Saved { id, content } => {
                        app.set_status(&format!("saving {id}…"));
                        spawn_save(client.clone(), tx.clone(), id, content);
                    }
                    EditorOutcome::Cancelled => {
                        app.set_status("edit cancelled (no changes)");
                    }
                    EditorOutcome::Failed(msg) => {
                        app.set_status(&format!("editor failed: {msg}"));
                    }
                }
            } else {
                app.set_status("no note selected");
            }
            ControlFlow::Continue
        }
    }
}

fn spawn_refresh(client: Client, tx: mpsc::UnboundedSender<BackgroundMsg>) {
    tokio::spawn(async move {
        let res = client.notes().await;
        let _ = tx.send(BackgroundMsg::Notes(res));
    });
}

fn spawn_fetch_note(client: Client, tx: mpsc::UnboundedSender<BackgroundMsg>, id: String) {
    tokio::spawn(async move {
        let res = client.note(&id, None).await;
        let _ = tx.send(BackgroundMsg::Note { id, result: res });
    });
}

fn spawn_save(
    client: Client,
    tx: mpsc::UnboundedSender<BackgroundMsg>,
    id: String,
    content: String,
) {
    tokio::spawn(async move {
        let res = client.update_note_content(&id, Some(content)).await;
        let _ = tx.send(BackgroundMsg::Saved { id, result: res });
    });
}

/// Drops the alternate screen, runs `$EDITOR` synchronously, then re-enters.
///
/// Uses the existing `cli::editor::open_in_editor` flow (an empty tempfile)
/// and prepends the current note's content so the user edits the real body.
fn run_editor_for(id: String, current: Option<String>) -> EditorOutcome {
    use std::io::Write;
    use std::process::Command;

    // Temporarily leave the alternate screen so the editor has the real
    // terminal. We re-enter on return.
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen);

    let outcome = (|| -> std::result::Result<EditorOutcome, String> {
        let tmp = tempfile::Builder::new()
            .suffix(".md")
            .tempfile()
            .map_err(|e| e.to_string())?;
        let path = tmp.path().to_path_buf();

        if let Some(c) = current.as_deref() {
            let mut f = std::fs::File::create(&path).map_err(|e| e.to_string())?;
            f.write_all(c.as_bytes()).map_err(|e| e.to_string())?;
        }

        let editor = resolve_editor();
        let mut parts = editor.split_whitespace();
        let bin = parts
            .next()
            .ok_or_else(|| "no editor configured".to_string())?;
        let extra: Vec<&str> = parts.collect();
        let status = Command::new(bin)
            .args(&extra)
            .arg(&path)
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Ok(EditorOutcome::Failed(format!(
                "editor exited with {status}"
            )));
        }

        let new_content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        if current.as_deref() == Some(new_content.as_str()) {
            Ok(EditorOutcome::Cancelled)
        } else {
            Ok(EditorOutcome::Saved {
                id,
                content: new_content,
            })
        }
    })()
    .unwrap_or_else(EditorOutcome::Failed);

    // Re-enter the alternate screen for the TUI loop.
    let _ = enable_raw_mode();
    let _ = execute!(io::stdout(), EnterAlternateScreen);

    outcome
}

fn resolve_editor() -> String {
    if let Ok(v) = std::env::var("VISUAL")
        && !v.is_empty()
    {
        return v;
    }
    if let Ok(e) = std::env::var("EDITOR")
        && !e.is_empty()
    {
        return e;
    }
    if cfg!(windows) {
        "notepad".to_string()
    } else {
        "vim".to_string()
    }
}


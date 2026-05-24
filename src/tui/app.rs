//! TUI application state + pure key-dispatch logic.
//!
//! Kept terminal-free on purpose: the event loop in [`super::run`] is the
//! only thing that touches stdin/stdout. Everything here is plain data, so
//! it's straightforward to unit-test.

use crossterm::event::{KeyCode, KeyEvent};

use crate::CachedResponse;
use crate::error::Result;
use crate::types::{Note, SingleNote};

/// User intents derived from key events. The event loop converts these to
/// side effects (network calls, editor spawns, exit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
    Up,
    Down,
    Open,
    Refresh,
    Edit,
}

/// Result reported back to the main loop from a `tokio::spawn`ed task.
#[derive(Debug)]
pub enum BackgroundMsg {
    /// Outcome of `client.notes()`.
    Notes(Result<Vec<Note>>),
    /// Outcome of `client.note(id, None)`.
    Note {
        id: String,
        result: Result<CachedResponse<SingleNote>>,
    },
    /// Outcome of `client.update_note_content(id, content)`.
    Saved {
        id: String,
        result: Result<SingleNote>,
    },
}

/// What happened when the user opened `$EDITOR`.
#[derive(Debug)]
pub enum EditorOutcome {
    /// Editor exited with new content (different from what we started with).
    Saved { id: String, content: String },
    /// Editor exited but content is unchanged.
    Cancelled,
    /// Editor failed to launch / exited non-zero / IO error.
    Failed(String),
}

/// In-memory TUI state.
#[derive(Debug, Default)]
pub struct App {
    pub notes: Vec<Note>,
    pub selected: usize,
    pub current_note: Option<SingleNote>,
    pub status: String,
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    /// Id of the currently-selected note in the list, if any.
    pub fn selected_id(&self) -> Option<String> {
        self.notes.get(self.selected).map(|n| n.id.clone())
    }

    /// Content of the currently-loaded note (right pane), if any.
    pub fn selected_content(&self) -> Option<String> {
        self.current_note.as_ref().map(|n| n.content.clone())
    }

    /// Move list selection up by one (saturating at the top).
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move list selection down by one (saturating at the bottom).
    pub fn move_down(&mut self) {
        if !self.notes.is_empty() && self.selected + 1 < self.notes.len() {
            self.selected += 1;
        }
    }

    /// Overwrite the bottom status line.
    pub fn set_status(&mut self, s: &str) {
        self.status = s.to_string();
    }

    /// Apply a message from a background task.
    pub fn apply_background(&mut self, msg: BackgroundMsg) {
        match msg {
            BackgroundMsg::Notes(Ok(list)) => {
                self.notes = list;
                if self.selected >= self.notes.len() {
                    self.selected = self.notes.len().saturating_sub(1);
                }
                self.status = format!("loaded {} note(s)", self.notes.len());
            }
            BackgroundMsg::Notes(Err(e)) => {
                self.status = format!("notes error: {e}");
            }
            BackgroundMsg::Note { id, result } => match result {
                Ok(CachedResponse::Modified { body, .. }) => {
                    self.status = format!("loaded {id}");
                    self.current_note = Some(body);
                }
                Ok(CachedResponse::NotModified) => {
                    self.status = format!("{id}: not modified");
                }
                Err(e) => {
                    self.status = format!("note error: {e}");
                }
            },
            BackgroundMsg::Saved { id, result } => match result {
                Ok(note) => {
                    self.status = format!("saved {id}");
                    self.current_note = Some(note);
                }
                Err(e) => {
                    self.status = format!("save error: {e}");
                }
            },
        }
    }
}

/// Map a key event to an [`Action`], or `None` if it should be ignored.
///
/// Kept as a free function (not a method) so tests don't need an [`App`] to
/// exercise the dispatch table.
pub fn handle_key(_app: &App, key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => Some(Action::Quit),
        KeyCode::Char('j') | KeyCode::Down => Some(Action::Down),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::Up),
        KeyCode::Char('o') | KeyCode::Enter => Some(Action::Open),
        KeyCode::Char('r') => Some(Action::Refresh),
        KeyCode::Char('e') => Some(Action::Edit),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn handle_key_maps_navigation() {
        let app = App::new();
        assert_eq!(
            handle_key(&app, key(KeyCode::Char('j'))),
            Some(Action::Down)
        );
        assert_eq!(handle_key(&app, key(KeyCode::Down)), Some(Action::Down));
        assert_eq!(handle_key(&app, key(KeyCode::Char('k'))), Some(Action::Up));
        assert_eq!(handle_key(&app, key(KeyCode::Up)), Some(Action::Up));
    }

    #[test]
    fn handle_key_maps_open_refresh_edit_quit() {
        let app = App::new();
        assert_eq!(handle_key(&app, key(KeyCode::Enter)), Some(Action::Open));
        assert_eq!(
            handle_key(&app, key(KeyCode::Char('o'))),
            Some(Action::Open)
        );
        assert_eq!(
            handle_key(&app, key(KeyCode::Char('r'))),
            Some(Action::Refresh)
        );
        assert_eq!(
            handle_key(&app, key(KeyCode::Char('e'))),
            Some(Action::Edit)
        );
        assert_eq!(
            handle_key(&app, key(KeyCode::Char('q'))),
            Some(Action::Quit)
        );
        assert_eq!(handle_key(&app, key(KeyCode::Esc)), Some(Action::Quit));
    }

    #[test]
    fn handle_key_ignores_other_keys() {
        let app = App::new();
        assert_eq!(handle_key(&app, key(KeyCode::Char('x'))), None);
        assert_eq!(handle_key(&app, key(KeyCode::Tab)), None);
    }

    #[test]
    fn move_up_saturates_at_zero() {
        let mut app = App::new();
        app.move_up();
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn move_down_saturates_at_last() {
        let mut app = App::new();
        app.notes = vec![dummy_note("a"), dummy_note("b")];
        app.move_down();
        assert_eq!(app.selected, 1);
        app.move_down();
        assert_eq!(app.selected, 1, "should saturate at last index");
    }

    #[test]
    fn selected_id_returns_none_when_empty() {
        let app = App::new();
        assert!(app.selected_id().is_none());
    }

    fn dummy_note(id: &str) -> Note {
        Note {
            id: id.into(),
            title: "t".into(),
            tags: vec![],
            last_changed_at: "".into(),
            created_at: "".into(),
            last_change_user: None,
            publish_type: crate::types::NotePublishType::Edit,
            published_at: None,
            user_path: None,
            team_path: None,
            permalink: None,
            short_id: "".into(),
            publish_link: "".into(),
            read_permission: crate::types::NotePermissionRole::Owner,
            write_permission: crate::types::NotePermissionRole::Owner,
            folder_paths: None,
        }
    }
}

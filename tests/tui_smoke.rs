//! Smoke tests for the TUI's pure-logic surface. No terminal is allocated —
//! we exercise [`hackmd::tui::handle_key`] and [`hackmd::tui::App`] mutators
//! directly.

#![cfg(feature = "tui")]

use hackmd::tui::{Action, App, handle_key};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn dispatch_basic_keys() {
    let app = App::new();
    assert_eq!(
        handle_key(&app, key(KeyCode::Char('q'))),
        Some(Action::Quit)
    );
    assert_eq!(handle_key(&app, key(KeyCode::Esc)), Some(Action::Quit));
    assert_eq!(
        handle_key(&app, key(KeyCode::Char('j'))),
        Some(Action::Down)
    );
    assert_eq!(handle_key(&app, key(KeyCode::Char('k'))), Some(Action::Up));
    assert_eq!(handle_key(&app, key(KeyCode::Enter)), Some(Action::Open));
    assert_eq!(
        handle_key(&app, key(KeyCode::Char('r'))),
        Some(Action::Refresh)
    );
    assert_eq!(
        handle_key(&app, key(KeyCode::Char('e'))),
        Some(Action::Edit)
    );
}

#[test]
fn dispatch_ignores_unbound_keys() {
    let app = App::new();
    assert!(handle_key(&app, key(KeyCode::Char('z'))).is_none());
    assert!(handle_key(&app, key(KeyCode::F(1))).is_none());
}

#[test]
fn empty_app_has_no_selection() {
    let app = App::new();
    assert!(app.selected_id().is_none());
    assert_eq!(app.selected, 0);
}

#[test]
fn status_set_is_visible() {
    let mut app = App::new();
    app.set_status("hello");
    assert_eq!(app.status, "hello");
}

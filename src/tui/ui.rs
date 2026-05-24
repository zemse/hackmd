//! Rendering for the TUI. Pure ratatui — no terminal I/O happens here, the
//! frame is supplied by [`Terminal::draw`](ratatui::Terminal::draw).

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use super::app::App;

const HELP_LINE: &str = "j/k navigate · enter/o open · e edit · r refresh · q quit";

pub fn draw(f: &mut Frame, app: &App) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(f.area());

    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(outer[0]);

    draw_list(f, app, main[0]);
    draw_content(f, app, main[1]);
    draw_status(f, app, outer[1]);
}

fn draw_list(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let items: Vec<ListItem> = app
        .notes
        .iter()
        .map(|n| {
            let title = if n.title.is_empty() {
                "(untitled)".to_string()
            } else {
                n.title.clone()
            };
            ListItem::new(Line::raw(title))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Notes (user)"))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");

    let mut state = ListState::default();
    if !app.notes.is_empty() {
        state.select(Some(app.selected));
    }
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_content(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let (title, body) = match &app.current_note {
        Some(n) => {
            let header = if n.title.is_empty() {
                "(untitled)".to_string()
            } else {
                n.title.clone()
            };
            (header, n.content.clone())
        }
        None => (
            "Preview".to_string(),
            "Select a note and press Enter to load its content.".to_string(),
        ),
    };
    let p = Paragraph::new(body)
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(p, area);
}

fn draw_status(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let text = if app.status.is_empty() {
        HELP_LINE.to_string()
    } else {
        format!("{}  ·  {}", app.status, HELP_LINE)
    };
    let p = Paragraph::new(Line::raw(text));
    f.render_widget(p, area);
}

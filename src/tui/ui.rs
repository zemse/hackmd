use std::io::{self, Stdout};

use anyhow::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};

use crate::tui::app::{
    self, App, BrowserEntryKind, DiffRowKind, EditMode, Focus, ReaderOrigin, View,
};
use crate::tui::links::LinkTarget;

pub type Term = Terminal<CrosstermBackend<Stdout>>;

pub fn setup_terminal() -> Result<Term> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let term = Terminal::new(backend)?;
    Ok(term)
}

pub fn restore_terminal(term: &mut Term) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        term.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    term.show_cursor()?;
    Ok(())
}

/// Restore raw/alt screen state for use in a panic hook (best-effort).
pub fn restore_raw() -> Result<()> {
    let _ = disable_raw_mode();
    let mut out = io::stdout();
    let _ = execute!(out, LeaveAlternateScreen, DisableMouseCapture);
    Ok(())
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // body (full screen except for one row)
            Constraint::Length(1), // statusline
        ])
        .split(area);

    let body = chunks[0];
    let status = chunks[1];
    app.viewport = body;
    app.statusline_area = status;

    // Reserve 1 column on the right for the reader scrollbar so layout stays
    // stable whether or not content overflows. Browser ignores this width.
    // In split-edit mode the preview pane is narrower than the body, so
    // re-target the wrap width to whichever pane will actually display the
    // rendered lines — otherwise the preview overflows on the right.
    let render_width = match &app.view {
        View::Reader(r) if matches!(r.edit.as_ref().map(|e| e.mode), Some(EditMode::Split)) => {
            if body.width >= 100 {
                // Horizontal split: preview is the right half minus separator.
                body.width.saturating_sub(body.width / 2 + 1)
            } else {
                // Vertical stack: preview spans full body width.
                body.width
            }
        }
        _ => body.width.saturating_sub(1),
    };
    app.ensure_rendered(render_width);

    if app.git_lens.is_some() && matches!(app.view, View::Reader(_)) {
        draw_git_lens(f, app, body);
    } else {
        match &app.view {
            View::Reader(r) => {
                let split_edit = r
                    .edit
                    .as_ref()
                    .map(|e| e.mode == EditMode::Split)
                    .unwrap_or(false);
                if split_edit {
                    draw_edit_split(f, app, body);
                } else {
                    draw_reader(f, app, body);
                }
            }
            View::Browser(_) => draw_browser(f, app, body),
        }
        if matches!(app.view, View::Reader(_)) {
            // Image rendering is read-only; in split-edit mode we still
            // show images in the preview pane area but the layout below
            // wires its own rect, so the global overlay is suppressed.
            let in_split = matches!(&app.view, View::Reader(r) if r.edit.as_ref().map(|e| e.mode == EditMode::Split).unwrap_or(false));
            if !in_split {
                draw_images_overlay(f, app, body);
            }
        }
    }

    draw_statusline(f, app, status);

    if app.search.is_some() {
        draw_search(f, app, area);
    }

    if app.help_open {
        draw_help(f, area);
    }
}

/// Render `p` shortened against the launch root (or `~/`) when one is a prefix.
fn display_path(p: &std::path::Path, root: &std::path::Path) -> String {
    if let Ok(rel) = p.strip_prefix(root) {
        let s = rel.display().to_string();
        if s.is_empty() {
            return ".".to_string();
        }
        return s;
    }
    if let Some(home) = dirs::home_dir() {
        if let Ok(rel) = p.strip_prefix(&home) {
            return format!("~/{}", rel.display());
        }
    }
    p.display().to_string()
}

fn draw_reader(f: &mut Frame, app: &mut App, area: Rect) {
    let View::Reader(r) = &app.view else {
        return;
    };
    let Some(rendered) = &r.rendered else {
        return;
    };
    let theme = &app.opts.theme;

    let total = rendered.lines.len();
    let scroll = (r.scroll as usize).min(total.saturating_sub(1));
    let visible_h = area.height as usize;

    // Mark the file read once its last line is on screen. Documents shorter
    // than the viewport satisfy this on open (nothing to scroll = read); longer
    // ones require scrolling to the end. `mark_read` is idempotent, so calling
    // it every frame while parked at the bottom is cheap. Not in edit mode —
    // that's a separate draw path.
    let mark_read_path = if r.edit.is_none() && r.scroll as usize + visible_h >= total {
        match &r.origin {
            ReaderOrigin::File(p) => Some(p.clone()),
            ReaderOrigin::Stdin => None,
        }
    } else {
        None
    };

    let line_num_w = if app.opts.line_numbers {
        format!("{}", total).len() as u16 + 1
    } else {
        0
    };
    let scrollbar_w: u16 = 1;
    let line_num_area = Rect {
        x: area.x,
        y: area.y,
        width: line_num_w,
        height: area.height,
    };
    let body_area = Rect {
        x: area.x + line_num_w,
        y: area.y,
        width: area
            .width
            .saturating_sub(line_num_w)
            .saturating_sub(scrollbar_w),
        height: area.height,
    };
    let scrollbar_area = Rect {
        x: area.x + area.width.saturating_sub(scrollbar_w),
        y: area.y,
        width: scrollbar_w,
        height: area.height,
    };

    let mut display_lines: Vec<Line> = Vec::with_capacity(visible_h);
    let mut nums: Vec<Line> = Vec::with_capacity(visible_h);
    // Suppress the keyboard-focus highlight while the user's been driving
    // with the mouse — two simultaneous "cursors" was the user's complaint.
    let show_focus = !app.mouse_recent;
    for i in 0..visible_h {
        let idx = scroll + i;
        if idx >= total {
            break;
        }
        let mut line = rendered.lines[idx].clone();
        if show_focus {
            match r.focus {
                Some(Focus::Link(fi)) => {
                    if let Some(link) = rendered.link_map.links.get(fi) {
                        if link.line == idx {
                            highlight_focused(&mut line, link, theme);
                        }
                    }
                }
                Some(Focus::Checkbox(ci)) => {
                    if let Some(cb) = rendered.checkbox_map.items.get(ci) {
                        if cb.line == idx {
                            highlight_checkbox_hover(&mut line, cb.col_start, cb.col_end);
                        }
                    }
                }
                None => {}
            }
        }
        if let Some(hi) = r.hover_link {
            if let Some(link) = rendered.link_map.links.get(hi) {
                if link.line == idx {
                    highlight_focused(&mut line, link, theme);
                }
            }
        }
        if let Some(ci) = r.hover_checkbox {
            if let Some(cb) = rendered.checkbox_map.items.get(ci) {
                if cb.line == idx {
                    highlight_checkbox_hover(&mut line, cb.col_start, cb.col_end);
                }
            }
        }
        if let Some(bi) = r.hover_jsonl {
            if let Some(btn) = r.jsonl_overlay.as_ref().and_then(|o| o.buttons.get(bi)) {
                if btn.line == idx {
                    highlight_checkbox_hover(&mut line, btn.col_start, btn.col_end);
                }
            }
        }
        if let Some(s) = r.doc_search.as_ref() {
            for (mi, m) in s.matches.iter().enumerate() {
                if m.line == idx {
                    let is_current = !s.editing && mi == s.current;
                    highlight_doc_match(&mut line, m.col_start, m.col_end, is_current, theme);
                }
            }
        }
        display_lines.push(line);
        if app.opts.line_numbers {
            nums.push(Line::from(Span::styled(
                format!(
                    "{:>width$} ",
                    idx + 1,
                    width = (line_num_w as usize).saturating_sub(1)
                ),
                Style::default().fg(theme.muted),
            )));
        }
    }

    if app.opts.line_numbers {
        f.render_widget(Paragraph::new(nums), line_num_area);
    }
    f.render_widget(Paragraph::new(display_lines), body_area);
    draw_scrollbar(f, scrollbar_area, scroll, total, visible_h, theme);

    // Drag-select overlay: paint reverse-video over the selection range.
    // Drawn before the cursor so the cursor halo wins on top.
    if let Some(sel) = app.selection.filter(|s| s.is_active()) {
        let ((s_line, s_col), (e_line, e_col)) = sel.normalized();
        let buf = f.buffer_mut();
        for li in s_line..=e_line {
            let cy_view = li as i32 - r.scroll as i32;
            if cy_view < 0 || cy_view as u16 >= body_area.height {
                continue;
            }
            let row = body_area.y + cy_view as u16;
            let from = if li == s_line { s_col as usize } else { 0 };
            let to = if li == e_line {
                e_col as usize
            } else {
                body_area.width as usize
            };
            for c in from..to {
                let x = body_area.x + c as u16;
                if x >= body_area.x + body_area.width {
                    break;
                }
                let cell = &mut buf[(x, row)];
                cell.set_style(cell.style().add_modifier(Modifier::REVERSED));
            }
        }
    }

    // Edit mode: paint the source cursor as a reverse-video cell on top of
    // the body. Doing this *after* rendering the paragraph keeps the cursor
    // visible regardless of the underlying span's foreground/background.
    if let Some((cx, cy)) = rendered.cursor_xy {
        let cy_view = cy as i32 - r.scroll as i32;
        if cy_view >= 0 && (cy_view as u16) < body_area.height {
            let row = body_area.y + cy_view as u16;
            let col = body_area.x + cx;
            if col < body_area.x + body_area.width {
                let buf = f.buffer_mut();
                let cell = &mut buf[(col, row)];
                // Carry the existing cell's char so we don't overwrite the
                // glyph the cursor is "on" — we just invert it.
                cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
            }
        }
    }

    // Last line is visible — record this file as read. Done after all `r`
    // borrows above are out of scope so the disjoint `&mut app.read_state`
    // borrow is clean.
    if let Some(path) = mark_read_path {
        app.read_state.mark_read(&path);
    }
}

/// Render split-screen edit mode: raw editor on the left (or top on
/// narrow terminals), rendered preview on the right (or bottom). Both
/// panes get a labeled header row; the focused pane (always the raw
/// editor for now since that's where the cursor lives) is highlighted.
fn draw_edit_split(f: &mut Frame, app: &mut App, area: Rect) {
    let View::Reader(r) = &app.view else {
        return;
    };
    let theme = &app.opts.theme;
    let Some(rendered) = r.rendered.as_ref() else {
        return;
    };

    // Layout choice: side-by-side at >= 100 cols, vertical stack below.
    let horizontal = area.width >= 100;
    let (raw_area, preview_area, _split_dir) = if horizontal {
        let half = area.width / 2;
        let raw = Rect {
            x: area.x,
            y: area.y,
            width: half,
            height: area.height,
        };
        let prev = Rect {
            x: area.x + half + 1,
            y: area.y,
            width: area.width.saturating_sub(half + 1),
            height: area.height,
        };
        // Draw the vertical separator column.
        let sep = Rect {
            x: area.x + half,
            y: area.y,
            width: 1,
            height: area.height,
        };
        let sep_style = Style::default().fg(theme.muted);
        let sep_lines: Vec<Line> = (0..sep.height)
            .map(|_| Line::from(Span::styled("│", sep_style)))
            .collect();
        f.render_widget(Paragraph::new(sep_lines), sep);
        (raw, prev, "h")
    } else {
        let half = area.height / 2;
        let raw = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: half,
        };
        let prev = Rect {
            x: area.x,
            y: area.y + half + 1,
            width: area.width,
            height: area.height.saturating_sub(half + 1),
        };
        let sep = Rect {
            x: area.x,
            y: area.y + half,
            width: area.width,
            height: 1,
        };
        let sep_style = Style::default().fg(theme.muted);
        let bar = "─".repeat(sep.width as usize);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(bar, sep_style))),
            sep,
        );
        (raw, prev, "v")
    };

    app.edit_raw_area = raw_area;
    app.edit_preview_area = preview_area;

    // ---- Raw pane ----
    let raw_rows = app::render_raw_pane(&r.raw, raw_area.width as usize);
    let cursor = r.edit.as_ref().map(|e| e.cursor).unwrap_or(0);
    let cur_row_idx = app::raw_row_for_cursor(&raw_rows, cursor);
    let cur_col = if let Some(row) = raw_rows.get(cur_row_idx) {
        app::raw_col_for_cursor(&r.raw, row, cursor)
    } else {
        0
    };

    // Auto-scroll raw pane so cursor is visible — but only when the cursor
    // *moved* since the last frame (typing, arrow keys, click). Wheel
    // scrolling doesn't touch the cursor, so without this gate every frame
    // would snap the scroll back and the wheel would feel inert.
    let visible_h_raw = raw_area.height as usize;
    let mut raw_scroll = r.scroll as usize;
    let cursor_changed = r
        .edit
        .as_ref()
        .map(|e| e.last_drawn_cursor != Some(cursor))
        .unwrap_or(false);
    if cursor_changed {
        if cur_row_idx < raw_scroll {
            raw_scroll = cur_row_idx;
        }
        if visible_h_raw > 0 && cur_row_idx >= raw_scroll + visible_h_raw {
            raw_scroll = cur_row_idx + 1 - visible_h_raw;
        }
    }
    let max_raw_scroll = raw_rows.len().saturating_sub(visible_h_raw);
    if raw_scroll > max_raw_scroll {
        raw_scroll = max_raw_scroll;
    }

    let mut raw_lines: Vec<Line> = Vec::with_capacity(visible_h_raw);
    for i in 0..visible_h_raw {
        let idx = raw_scroll + i;
        if idx >= raw_rows.len() {
            break;
        }
        let row = &raw_rows[idx];
        // Kind is computed per source line so wrapped continuation rows
        // keep the same styling as their head row.
        let style = match row.kind {
            app::RawRowKind::Heading => Style::default()
                .fg(theme.heading[0])
                .add_modifier(Modifier::BOLD),
            app::RawRowKind::Quote => Style::default().fg(theme.quote),
            app::RawRowKind::Normal => Style::default(),
        };
        raw_lines.push(Line::from(Span::styled(row.text.clone(), style)));
    }
    f.render_widget(Paragraph::new(raw_lines), raw_area);

    // Cursor: reverse-video cell at (cur_col, cur_row_idx - raw_scroll).
    let cy_view = cur_row_idx as i32 - raw_scroll as i32;
    if cy_view >= 0 && (cy_view as u16) < raw_area.height {
        let row = raw_area.y + cy_view as u16;
        let col = raw_area.x + cur_col;
        if col < raw_area.x + raw_area.width {
            let buf = f.buffer_mut();
            let cell = &mut buf[(col, row)];
            cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
        }
    }

    // ---- Preview pane ----
    // Sync preview to follow cursor block — but, like the raw pane, only
    // when the cursor moved. Otherwise wheel-driven scrolls (which sync
    // preview via the events layer) would be immediately undone by this
    // snap-back on the next frame.
    let preview_target = app::preview_row_for_source(rendered, cursor);
    let visible_h_prev = preview_area.height as usize;
    let mut prev_scroll = r.preview_scroll as usize;
    if cursor_changed {
        if preview_target < prev_scroll {
            prev_scroll = preview_target;
        }
        if visible_h_prev > 0 && preview_target >= prev_scroll + visible_h_prev {
            prev_scroll = preview_target + 1 - visible_h_prev;
        }
    }
    let max_prev_scroll = rendered.lines.len().saturating_sub(visible_h_prev);
    if prev_scroll > max_prev_scroll {
        prev_scroll = max_prev_scroll;
    }

    let mut prev_lines: Vec<Line> = Vec::with_capacity(visible_h_prev);
    for i in 0..visible_h_prev {
        let idx = prev_scroll + i;
        if idx >= rendered.lines.len() {
            break;
        }
        prev_lines.push(rendered.lines[idx].clone());
    }
    f.render_widget(Paragraph::new(prev_lines), preview_area);

    // Highlight the row in the preview that corresponds to the cursor's
    // block, as a subtle reverse-video bar at the start of the row. Helps
    // the user see where their cursor is in the rendered output.
    let cy_prev = preview_target as i32 - prev_scroll as i32;
    if cy_prev >= 0 && (cy_prev as u16) < preview_area.height {
        let row = preview_area.y + cy_prev as u16;
        let buf = f.buffer_mut();
        if preview_area.width > 0 {
            let cell = &mut buf[(preview_area.x, row)];
            cell.set_char('▎');
            cell.set_style(Style::default().fg(theme.heading[0]));
        }
    }

    // Persist scroll positions back to the reader so wheel handlers can
    // build on them. Record the cursor we just drew so the next frame
    // only re-follows when it actually moved.
    if let View::Reader(r) = &mut app.view {
        r.scroll = raw_scroll as u16;
        r.preview_scroll = prev_scroll as u16;
        if let Some(e) = r.edit.as_mut() {
            e.last_drawn_cursor = Some(cursor);
        }
    }
}

/// Render the git lens overlay: each diff row is one display line, with
/// added rows on a green background, removed on red, hunk headers muted.
/// The viewport scrolls via `git_lens.scroll`.
fn draw_git_lens(f: &mut Frame, app: &App, area: Rect) {
    use ratatui::style::Color;
    let Some(g) = app.git_lens.as_ref() else {
        return;
    };
    let theme = &app.opts.theme;
    let added_bg = Color::Rgb(0x2d, 0x4f, 0x2d);
    let removed_bg = Color::Rgb(0x5a, 0x2d, 0x2d);
    let visible_h = area.height as usize;
    let scroll = (g.scroll as usize).min(g.rows.len().saturating_sub(1));

    let mut display_lines: Vec<Line> = Vec::with_capacity(visible_h);
    for i in 0..visible_h {
        let idx = scroll + i;
        if idx >= g.rows.len() {
            break;
        }
        let row = &g.rows[idx];
        let style = match row.kind {
            DiffRowKind::Added => Style::default().bg(added_bg),
            DiffRowKind::Removed => Style::default().bg(removed_bg),
            DiffRowKind::Hunk => Style::default()
                .fg(theme.heading[0])
                .add_modifier(Modifier::BOLD),
            DiffRowKind::Header => Style::default().fg(theme.muted),
            DiffRowKind::Info => Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::BOLD),
            DiffRowKind::Context => Style::default(),
        };
        // Pad rows to full width so the bg color extends the whole line.
        let visible_w = unicode_width::UnicodeWidthStr::width(row.text.as_str());
        let pad = (area.width as usize).saturating_sub(visible_w);
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled(row.text.clone(), style));
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), style));
        }
        display_lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(display_lines), area);
}

/// Render any visible images in the document on top of the body, using the
/// terminal's detected graphics protocol. No-op if the picker isn't available
/// or if the image fails to decode.
fn draw_images_overlay(f: &mut Frame, app: &mut App, body: Rect) {
    use ratatui_image::StatefulImage;

    if app.image_picker.is_none() {
        return;
    }
    let (images, scroll) = match &app.view {
        View::Reader(r) => match &r.rendered {
            Some(rd) => (rd.images.clone(), r.scroll as i32),
            None => return,
        },
        _ => return,
    };

    for img in &images {
        let rel_y = img.line as i32 - scroll;
        if rel_y < 0 || rel_y as u16 >= body.height {
            continue;
        }
        let path = match std::fs::canonicalize(&img.source) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !app.image_protocols.contains_key(&path) {
            let dyn_img = match image::ImageReader::open(&path) {
                Ok(rdr) => match rdr.decode() {
                    Ok(d) => d,
                    Err(_) => continue,
                },
                Err(_) => continue,
            };
            let proto = match app.image_picker.as_ref() {
                Some(p) => p.new_resize_protocol(dyn_img),
                None => continue,
            };
            app.image_protocols.insert(path.clone(), proto);
        }
        let proto = match app.image_protocols.get_mut(&path) {
            Some(p) => p,
            None => continue,
        };
        let max_h = body.height.saturating_sub(rel_y as u16);
        let h = 12u16.min(max_h);
        if h == 0 {
            continue;
        }
        let area = Rect {
            x: body.x,
            y: body.y + rel_y as u16,
            width: body.width.saturating_sub(1),
            height: h,
        };
        f.render_stateful_widget(StatefulImage::default(), area, proto);
    }
}

/// Vertical scrollbar with a thumb sized proportionally to the visible viewport
/// (`thumb_h ≈ visible_h * track_h / total`). When content fits entirely the
/// thumb fills the track.
fn draw_scrollbar(
    f: &mut Frame,
    area: Rect,
    scroll: usize,
    total: usize,
    visible_h: usize,
    theme: &crate::tui::theme::Theme,
) {
    let track_h = area.height as usize;
    if track_h == 0 || area.width == 0 {
        return;
    }

    // Paint the cell BACKGROUND instead of relying on a `█`/`│` glyph: many
    // terminals add line-spacing padding between rows that no character can
    // cover, so a glyph-based bar appears as separate ticks. The cell bg fills
    // the entire cell (padding included), so a space with `.bg()` produces a
    // visually continuous column.
    let track_style = Style::default().bg(theme.muted);
    let thumb_style = Style::default().bg(theme.heading[0]);

    let (thumb_top, thumb_h) = if total <= visible_h || total == 0 {
        (0, track_h)
    } else {
        let h = ((track_h * visible_h) / total).max(1).min(track_h);
        let max_scroll = total - visible_h;
        let span = track_h - h;
        let top = if max_scroll == 0 {
            0
        } else {
            (scroll * span + max_scroll / 2) / max_scroll
        };
        (top.min(span), h)
    };

    let lines: Vec<Line> = (0..track_h)
        .map(|i| {
            let in_thumb = i >= thumb_top && i < thumb_top + thumb_h;
            let style = if in_thumb { thumb_style } else { track_style };
            Line::from(Span::styled(" ", style))
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

fn highlight_focused(
    line: &mut Line<'_>,
    link: &crate::tui::links::LinkSpan,
    theme: &crate::tui::theme::Theme,
) {
    let mut col = 0usize;
    for span in &mut line.spans {
        let w = unicode_width::UnicodeWidthStr::width(span.content.as_ref());
        let span_start = col;
        let span_end = col + w;
        if span_start >= link.col_start && span_end <= link.col_end {
            span.style = span
                .style
                .fg(theme.link_focused)
                .add_modifier(Modifier::REVERSED);
        }
        col = span_end;
    }
}

/// Paint a match span. Non-current matches get the code-block background
/// (subtle); the current match gets the link-focus color reversed.
fn highlight_doc_match(
    line: &mut Line<'_>,
    col_start: usize,
    col_end: usize,
    is_current: bool,
    theme: &crate::tui::theme::Theme,
) {
    let mut col = 0usize;
    for span in &mut line.spans {
        let w = unicode_width::UnicodeWidthStr::width(span.content.as_ref());
        let span_start = col;
        let span_end = col + w;
        if span_start >= col_start && span_end <= col_end {
            span.style = if is_current {
                span.style
                    .fg(theme.link_focused)
                    .add_modifier(Modifier::REVERSED)
                    .add_modifier(Modifier::BOLD)
            } else {
                span.style.add_modifier(Modifier::REVERSED)
            };
        }
        col = span_end;
    }
}

/// Paint reverse-video over spans that fall within `[col_start, col_end)` to
/// signal a hovered checkbox marker.
fn highlight_checkbox_hover(line: &mut Line<'_>, col_start: usize, col_end: usize) {
    let mut col = 0usize;
    for span in &mut line.spans {
        let w = unicode_width::UnicodeWidthStr::width(span.content.as_ref());
        let span_start = col;
        let span_end = col + w;
        if span_start >= col_start && span_end <= col_end {
            span.style = span.style.add_modifier(Modifier::REVERSED);
        }
        col = span_end;
    }
}

fn draw_browser(f: &mut Frame, app: &App, area: Rect) {
    let View::Browser(b) = &app.view else {
        return;
    };
    let theme = &app.opts.theme;
    let title = format!(" {} ", b.dir.display());
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(theme.muted));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let badge_style = Style::default()
        .fg(theme.heading[3])
        .add_modifier(Modifier::BOLD);
    let items: Vec<ListItem> = b
        .entries
        .iter()
        .map(|e| {
            let name = Span::styled(e.display.clone(), browser_entry_style(e.kind, theme));
            let unread = match e.kind {
                BrowserEntryKind::Markdown => app.read_state.is_unread(&e.path),
                BrowserEntryKind::Dir => app.read_state.dir_has_unread(&e.path),
                BrowserEntryKind::ParentDir => false,
            };
            if unread {
                ListItem::new(Line::from(vec![
                    name,
                    Span::styled(" [unread]", badge_style),
                ]))
            } else {
                ListItem::new(name)
            }
        })
        .collect();
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(theme.status_bg)
                .fg(theme.status_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    let mut state = ListState::default()
        .with_offset(b.scroll as usize)
        .with_selected(Some(b.selected));
    f.render_stateful_widget(list, inner, &mut state);
}

fn browser_entry_style(kind: BrowserEntryKind, theme: &crate::tui::theme::Theme) -> Style {
    match kind {
        BrowserEntryKind::ParentDir => Style::default().fg(theme.muted),
        BrowserEntryKind::Dir => Style::default()
            .fg(theme.heading[0])
            .add_modifier(Modifier::BOLD),
        BrowserEntryKind::Markdown => Style::default(),
    }
}

fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    let Some(s) = &app.search else {
        return;
    };
    let theme = &app.opts.theme;

    let w = area.width.saturating_sub(8).max(20).min(100);
    let h = area.height.saturating_sub(4).max(8);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    f.render_widget(Clear, popup);

    let title = format!(" Search [{}] ", s.results.len());
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(theme.heading[0]));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(1)])
        .split(inner);

    let prompt = Line::from(vec![
        Span::styled(
            "▸ ",
            Style::default()
                .fg(theme.heading[0])
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(s.query.clone()),
        Span::styled("█", Style::default().fg(theme.heading[0])),
        Span::styled(
            format!("   {}", short_root(&app.root)),
            Style::default().fg(theme.muted),
        ),
    ]);
    f.render_widget(Paragraph::new(prompt), layout[0]);

    let items: Vec<ListItem> = s
        .results
        .iter()
        .map(|r| {
            let style = if r.is_dir {
                Style::default()
                    .fg(theme.heading[0])
                    .add_modifier(Modifier::BOLD)
            } else if app::is_markdown_file(&r.path) {
                Style::default()
            } else {
                Style::default().fg(theme.muted)
            };
            let display = if r.is_dir {
                format!("{}/", r.display)
            } else {
                r.display.clone()
            };
            ListItem::new(Span::styled(display, style))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(theme.status_bg)
                .fg(theme.status_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    let mut state = ListState::default().with_selected(Some(s.selected));
    f.render_stateful_widget(list, layout[1], &mut state);
}

fn short_root(p: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rel) = p.strip_prefix(&home) {
            return format!("~/{}", rel.display());
        }
    }
    p.display().to_string()
}

/// Single-row context-aware statusline. Anatomy from left to right:
///   ` ‹ Back  path/file.md   <middle>   25% `
/// where `<middle>` resolves in priority order to the doc-search prompt, the
/// hover-URL, the keyboard-focus URL, an explicit status message, or a hint
/// snippet of the most useful current shortcuts.
///
/// Edge case (per user request): when the hovered link sits on the bottom-most
/// body row (visually adjacent to this statusline), the URL gets pulled to the
/// opposite half of the row from the mouse column so it doesn't crowd the
/// pointer.
fn draw_statusline(f: &mut Frame, app: &mut App, area: Rect) {
    use unicode_width::UnicodeWidthStr;

    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = &app.opts.theme;

    let bg = Style::default().bg(theme.status_bg).fg(theme.status_fg);
    let path_style = Style::default().add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(theme.muted);

    // Path (or browser dir) — same display logic as before.
    let path = match &app.view {
        View::Reader(r) => match &r.origin {
            crate::tui::app::ReaderOrigin::File(p) => display_path(p, &app.root),
            crate::tui::app::ReaderOrigin::Stdin => "<stdin>".to_string(),
        },
        View::Browser(b) => format!("{}/", display_path(&b.dir, &app.root)),
    };

    let scroll_pos = match &app.view {
        View::Reader(r) => {
            let total = r.rendered.as_ref().map(|x| x.lines.len()).unwrap_or(0);
            let h = app.viewport.height as usize;
            if total == 0 || total <= h {
                "All".to_string()
            } else {
                let max_scroll = total - h;
                let pct = ((r.scroll as usize) * 100 / max_scroll).min(100);
                format!("{}%", pct)
            }
        }
        View::Browser(b) => format!("{}/{}", b.selected + 1, b.entries.len().max(1)),
    };

    // Resolve middle content + classify the mode (search / hover / hint).
    let middle = compute_middle(app);

    // Right span: scroll %.
    let right = Span::styled(format!(" {} ", scroll_pos), bg);
    let right_w = UnicodeWidthStr::width(right.content.as_ref());

    // Left span: edit-mode badge (if editing) OR optional back button +
    // path. The edit badge takes precedence over Back so the user always
    // knows they're in a mutating mode.
    let mut back_span: Option<Span> = None;
    app.back_button_hit = None;
    app.statusline_url_hit = None;
    let edit_badge: Option<Span> = if let View::Reader(r) = &app.view {
        r.edit.as_ref().map(|e| {
            let label = if e.dirty { " EDIT* " } else { " EDIT " };
            Span::styled(
                label.to_string(),
                Style::default()
                    .bg(theme.heading[0])
                    .fg(theme.status_fg)
                    .add_modifier(Modifier::BOLD),
            )
        })
    } else {
        None
    };
    let lens_badge: Option<Span> = if app.git_lens.is_some() {
        Some(Span::styled(
            " GIT LENS ".to_string(),
            Style::default()
                .bg(theme.heading[1])
                .fg(theme.status_fg)
                .add_modifier(Modifier::BOLD),
        ))
    } else {
        None
    };
    let edit_badge = edit_badge.or(lens_badge);
    if edit_badge.is_none() && !app.history.is_empty() {
        let label = " ‹ Back ";
        let start_x = area.x;
        let end_x = area.x + label.chars().count() as u16;
        back_span = Some(Span::styled(
            label.to_string(),
            bg.add_modifier(Modifier::BOLD),
        ));
        app.back_button_hit = Some((start_x, end_x));
    }
    let left_badge = edit_badge.or(back_span);

    // Detect whether we should apply the bottom-row hover edge case: the
    // hovered link sits on the last body row, and the user's mouse is over it.
    let edge_swap = compute_edge_swap(app, &middle);

    // Actually render. Two layouts:
    //   - default: [back] [path]   middle   right
    //   - edge_swap to right: [back] [path]              [middle][right]
    //   - edge_swap to left:  [middle][gap][path]        [right]
    let total_w = area.width as usize;

    let mut line_spans: Vec<Span<'static>> = Vec::new();
    let mid_text = middle.text();
    // When the middle is a URL, record the screen column range it occupies so
    // a click on it can copy the (untruncated) target. `record_url_hit` reads
    // the accumulated span width to learn where the next span will start.
    let is_url = matches!(middle, Mid::Url { .. });
    // Returns the hit region for the URL span about to be pushed: it starts at
    // `area.x + <accumulated width>` and spans `width` columns. Returns None
    // when the middle isn't a URL.
    let url_hit = |spans: &[Span<'static>], width: usize| -> Option<(u16, u16, String)> {
        if is_url {
            let start_x = area.x + span_width(spans) as u16;
            Some((start_x, start_x + width as u16, mid_text.clone()))
        } else {
            None
        }
    };
    let mut url_hit_region: Option<(u16, u16, String)> = None;
    match edge_swap {
        EdgeSwap::Right => {
            // URL pinned to the right of the row (just before scroll%).
            push_left(&mut line_spans, &left_badge, &path, path_style);
            let mid_styled = Span::styled(format!(" {} ", mid_text), middle.style(theme));
            let mid_w = UnicodeWidthStr::width(mid_styled.content.as_ref());
            let used = span_width(&line_spans) + mid_w + right_w;
            line_spans.push(Span::raw(" ".repeat(total_w.saturating_sub(used))));
            url_hit_region = url_hit(&line_spans, mid_w);
            line_spans.push(mid_styled);
            line_spans.push(right);
        }
        EdgeSwap::Left => {
            // URL takes the left of the row, suppressing the path.
            if let Some(b) = left_badge.clone() {
                line_spans.push(b);
                line_spans.push(Span::raw(" "));
            }
            let mid_styled = Span::styled(format!(" {} ", mid_text), middle.style(theme));
            let mid_w = UnicodeWidthStr::width(mid_styled.content.as_ref());
            url_hit_region = url_hit(&line_spans, mid_w);
            line_spans.push(mid_styled);
            let used = span_width(&line_spans) + right_w;
            line_spans.push(Span::raw(" ".repeat(total_w.saturating_sub(used))));
            line_spans.push(right);
        }
        EdgeSwap::None => {
            // Default layout: [back] [path]   <middle>   <right>
            push_left(&mut line_spans, &left_badge, &path, path_style);
            if !mid_text.is_empty() {
                let pad_left = 2usize;
                let used_left = span_width(&line_spans) + pad_left;
                let max_mid = total_w
                    .saturating_sub(used_left)
                    .saturating_sub(right_w + 2);
                let truncated = truncate_mid(&mid_text, max_mid);
                let mid_w = UnicodeWidthStr::width(truncated.as_str());
                line_spans.push(Span::raw(" ".repeat(pad_left)));
                url_hit_region = url_hit(&line_spans, mid_w);
                line_spans.push(Span::styled(truncated, middle.style(theme)));
            }
            let used = span_width(&line_spans) + right_w;
            line_spans.push(Span::raw(" ".repeat(total_w.saturating_sub(used))));
            line_spans.push(right);
        }
    }
    app.statusline_url_hit = url_hit_region;

    let _ = muted;
    f.render_widget(Paragraph::new(Line::from(line_spans)), area);
}

#[derive(Clone, Debug)]
enum Mid {
    Hint(String),
    /// Static status message (e.g. "Copied: ...", "File reloaded").
    Status(String),
    /// `/query_` while typing, or `/query  [n/m]` after commit.
    Search(String),
    /// Hovered or focused link target.
    Url {
        text: String,
        on_last_row: bool,
    },
}

#[derive(Clone, Copy, Debug)]
enum EdgeSwap {
    None,
    Left,
    Right,
}

impl Mid {
    fn text(&self) -> String {
        match self {
            Mid::Hint(s) | Mid::Status(s) | Mid::Search(s) => s.clone(),
            Mid::Url { text, .. } => text.clone(),
        }
    }
    fn style(&self, theme: &crate::tui::theme::Theme) -> Style {
        match self {
            Mid::Url { .. } => Style::default()
                .fg(theme.link)
                .add_modifier(Modifier::UNDERLINED),
            Mid::Search(_) => Style::default().fg(theme.heading[0]),
            Mid::Status(_) => Style::default()
                .fg(theme.heading[0])
                .add_modifier(Modifier::BOLD),
            Mid::Hint(_) => Style::default().fg(theme.muted),
        }
    }
}

fn compute_middle(app: &App) -> Mid {
    // Edit mode confirm-discard prompt overrides everything (including the
    // status line — we want the user to act on the prompt before being
    // distracted by anything else).
    if let View::Reader(r) = &app.view {
        if let Some(e) = r.edit.as_ref() {
            if e.discard_pending {
                return Mid::Status("Press Esc again to discard, any other key to cancel".into());
            }
        }
    }
    // An active mouse hover over a link shows its target immediately — even
    // over a sticky status message ("Opened …", "Copied …", "File reloaded").
    // Hovering is a live gesture and the user wants feedback on what they're
    // pointing at; moving the mouse away restores the status. Suppressed while
    // editing or typing a search, which own the middle. Keyboard focus stays a
    // lower-priority fallback below the status (see the link block further on).
    if let View::Reader(r) = &app.view {
        let busy = r.edit.is_some() || r.doc_search.as_ref().map(|s| s.editing).unwrap_or(false);
        if !busy {
            if let (Some(rendered), Some(hi)) = (r.rendered.as_ref(), r.hover_link) {
                if let Some(link) = rendered.link_map.links.get(hi) {
                    let visible_h = app.viewport.height as usize;
                    let scroll = r.scroll as usize;
                    let last_row_idx = scroll + visible_h.saturating_sub(1);
                    let on_last_row = link.line == last_row_idx;
                    return Mid::Url {
                        text: describe_target(&link.target),
                        on_last_row,
                    };
                }
            }
        }
    }
    if !app.status.is_empty() {
        return Mid::Status(app.status.clone());
    }
    // Cloud operations in flight — keep the user informed without blocking.
    if app.cloud.pending > 0 {
        return Mid::Status("⟳ syncing…".into());
    }
    if app.git_lens.is_some() {
        return Mid::Hint("git lens (vs HEAD)  j/k scroll  Ctrl-G or Esc to dismiss".into());
    }
    if let View::Reader(r) = &app.view {
        // Edit-mode hint replaces the normal viewer hint when active.
        if r.edit.is_some() {
            return Mid::Hint(
                "type to edit  Ctrl-S save  Alt-←/→ word  Ctrl-Z undo  Esc Esc discard".into(),
            );
        }
        if let Some(s) = r.doc_search.as_ref() {
            let txt = if s.editing {
                format!("/{}_", s.query)
            } else if s.matches.is_empty() {
                format!("no match: /{}", s.query)
            } else {
                format!("/{}  [{}/{}]", s.query, s.current + 1, s.matches.len())
            };
            return Mid::Search(txt);
        }
        if let Some(rendered) = r.rendered.as_ref() {
            // Active hover is handled earlier (it outranks a sticky status); here
            // we fall back to the keyboard-focused link when nothing's hovered.
            let pick = r.hover_link.or_else(|| match r.focus {
                Some(Focus::Link(i)) => Some(i),
                _ => None,
            });
            if let Some(hi) = pick {
                if let Some(link) = rendered.link_map.links.get(hi) {
                    let visible_h = app.viewport.height as usize;
                    let scroll = r.scroll as usize;
                    let last_row_idx = scroll + visible_h.saturating_sub(1);
                    let on_last_row = link.line == last_row_idx;
                    return Mid::Url {
                        text: describe_target(&link.target),
                        on_last_row,
                    };
                }
            }
        }
    }
    Mid::Hint(default_hint(app))
}

fn default_hint(app: &App) -> String {
    match &app.view {
        View::Reader(_) => "j/k  d/u  /search  Tab:link  o:open  e:edit  ?:help  q:quit".into(),
        View::Browser(_) => "j/k  Enter:open  /search  T:fuzzy  ?:help  q:quit".into(),
    }
}

fn compute_edge_swap(app: &App, middle: &Mid) -> EdgeSwap {
    match middle {
        Mid::Url {
            on_last_row: true, ..
        } => {
            let half = app.viewport.width / 2;
            // Mouse on the left half → push URL to the right (away from cursor).
            // Mouse on the right half → URL on the left (the user-defined default).
            if app.last_mouse_col < app.viewport.x + half {
                EdgeSwap::Right
            } else {
                EdgeSwap::Left
            }
        }
        _ => EdgeSwap::None,
    }
}

fn push_left(
    out: &mut Vec<Span<'static>>,
    back: &Option<Span<'static>>,
    path: &str,
    path_style: Style,
) {
    if let Some(b) = back.clone() {
        out.push(b);
        out.push(Span::raw(" "));
    } else {
        out.push(Span::raw(" "));
    }
    out.push(Span::styled(path.to_string(), path_style));
}

fn span_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

fn truncate_mid(s: &str, max: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    if max == 0 {
        return String::new();
    }
    let total: usize = s.chars().map(|c| c.width().unwrap_or(0)).sum();
    if total <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

fn describe_target(t: &LinkTarget) -> String {
    match t {
        LinkTarget::Url(u) => u.clone(),
        LinkTarget::LocalFile(p) => p.display().to_string(),
        LinkTarget::Anchor(a) => format!("#{}", a),
        LinkTarget::FileAnchor(p, a) => format!("{}#{}", p.display(), a),
    }
}

fn draw_help(f: &mut Frame, area: Rect) {
    let w = 60.min(area.width.saturating_sub(4));
    let h = 30.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    f.render_widget(Clear, popup);
    let body = vec![
        Line::from("md keybindings"),
        Line::from(""),
        Line::from("  j / k / ↓ ↑      scroll one line"),
        Line::from("  d / u            half page down / up"),
        Line::from("  Ctrl-d / Ctrl-u  half page (vim)"),
        Line::from("  Ctrl-f / Ctrl-b  full page (vim)"),
        Line::from("  PgDn / Space     page down"),
        Line::from("  gg               top of buffer"),
        Line::from("  G                bottom of buffer (NG → line N)"),
        Line::from("  H / M / L        focus visible top / middle / bottom"),
        Line::from("  zz               center current focus"),
        Line::from("  <count><motion>  e.g. 5j, 10G, 3Ctrl-d"),
        Line::from(""),
        Line::from("  Tab / S-Tab      cycle focus across links + checkboxes"),
        Line::from("  Enter / →        follow link or toggle checkbox"),
        Line::from("  Esc / ←          back  (Esc at root quits)"),
        Line::from("  /                in-doc text search (Reader) / file search (Browser)"),
        Line::from("  n / N            next / prev match"),
        Line::from("  T                fuzzy file search"),
        Line::from("  h / b            history back"),
        Line::from("  l / f            history forward"),
        Line::from("  e                edit mode (split: raw + preview, scroll-synced)"),
        Line::from("                   Ctrl-S save  Ctrl-Z undo  Esc Esc discard"),
        Line::from("                   Alt-←/→ word jump  Alt-Bksp/Del word delete"),
        Line::from("  Ctrl-G            git lens (diff vs HEAD; staged + unstaged)"),
        Line::from("  o                open focused link in browser"),
        Line::from("  m                toggle mouse capture (drag-to-select)"),
        Line::from("  q / Ctrl-C       quit"),
        Line::from("  ?                toggle this help"),
        Line::from(""),
        Line::from("  Mouse hides the keyboard focus halo. Any key restores it."),
    ];
    let block = Block::default().borders(Borders::ALL).title(" Help ");
    let para = Paragraph::new(body).block(block);
    f.render_widget(para, popup);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{App, Options, Source, View};
    use crate::tui::theme::Theme;

    fn app_with_link() -> App {
        let mut p = std::env::temp_dir();
        p.push(format!("md-tui-ui-test-{}.md", std::process::id()));
        std::fs::write(
            &p,
            "a paragraph with a [site](https://example.com/x) here\n",
        )
        .unwrap();
        let opts = Options {
            width: 80,
            line_numbers: false,
            theme: Theme::dark(),
        };
        let mut app = App::new(Source::File(p), opts).unwrap();
        app.ensure_rendered(80);
        // compute_middle reads app.viewport.height to decide last-row edge case.
        app.viewport = Rect::new(0, 0, 80, 24);
        // Sanity: the rendered doc must contain exactly one link to hover.
        let View::Reader(r) = &app.view else {
            panic!("expected reader");
        };
        assert_eq!(r.rendered.as_ref().unwrap().link_map.links.len(), 1);
        app
    }

    // An active hover over a link must show the URL preview even when a sticky
    // status message ("Opened …", "Copied …") is present — the live gesture
    // wins. This is the regression the fix addresses.
    #[test]
    fn hover_url_outranks_sticky_status() {
        let mut app = app_with_link();
        app.status = "Copied: something".into();
        if let View::Reader(r) = &mut app.view {
            r.hover_link = Some(0);
        }
        match compute_middle(&app) {
            Mid::Url { text, .. } => assert_eq!(text, "https://example.com/x"),
            other => panic!("expected Url preview while hovering, got {:?}", other),
        }
    }

    // With nothing hovered, the sticky status still shows (hover doesn't
    // suppress it spuriously).
    #[test]
    fn status_shows_when_not_hovering() {
        let mut app = app_with_link();
        app.status = "Copied: something".into();
        if let View::Reader(r) = &mut app.view {
            r.hover_link = None;
        }
        match compute_middle(&app) {
            Mid::Status(s) => assert_eq!(s, "Copied: something"),
            other => panic!("expected status, got {:?}", other),
        }
    }
}

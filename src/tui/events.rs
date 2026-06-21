use std::io::stdout;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;

use crate::tui::app::{
    self, App, BrowserEntry, BrowserEntryKind, EntryKind, Focus, SearchResult, View,
};
use crate::tui::ui;

/// Two-key vim chord (g, z) timeout.
const CHORD_TIMEOUT_MS: u128 = 700;

/// Take the buffered numeric prefix (or `1` if none), clamped to a sane
/// minimum so motion arms always make progress.
fn consume_count(prefix: &mut Option<u32>) -> i32 {
    prefix.take().unwrap_or(1).max(1) as i32
}

pub fn run(term: &mut ui::Term, app: &mut App) -> Result<()> {
    while !app.should_quit {
        // Detect external edits to the open file. One stat syscall per tick
        // (≤4/sec when idle, served from the kernel inode cache) — far
        // cheaper than a notification thread, and zero new dependencies.
        app.poll_external_change();
        // Same idea for the browser: one stat of the listed directory per tick
        // picks up files added/removed/renamed in it without a watcher thread.
        app.poll_browser_change();
        // Apply any cloud operations that finished since the last tick.
        app.drain_cloud_msgs();
        // Bidirectional HackMD sync for a linked, open local file: fires on
        // open and then every SYNC_INTERVAL to pull upstream edits.
        app.maybe_sync();
        term.draw(|f| ui::draw(f, app))?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        match event::read()? {
            Event::Key(k) if k.kind == KeyEventKind::Press => handle_key(app, k)?,
            Event::Mouse(m) => handle_mouse(app, m)?,
            Event::Resize(_, _) => {
                if let View::Reader(r) = &mut app.view {
                    r.rendered = None;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    // Any key press hides the mouse-driven cursor suppression so the keyboard
    // focus highlight reappears.
    app.mouse_recent = false;
    // Transient feedback ("Copied …", "Saved …", "File reloaded") lives until
    // the next input: any key returns the statusline to the contextual hints.
    // Handlers that produce a new message set it after this clear.
    app.status.clear();

    // Ctrl+C is a hard exit no matter what overlay is on screen.
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.should_quit = true;
        return Ok(());
    }
    if app.help_open {
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => app.help_open = false,
            _ => {}
        }
        return Ok(());
    }
    // The conflict resolver is a full-screen modal — it captures all keys.
    if app.conflict.is_some() {
        return handle_conflict_key(app, key);
    }
    // Modal prompt (new note / push / download / delete confirm) captures
    // all keys while open.
    if app.prompt.is_some() {
        return handle_prompt_key(app, key);
    }
    // Ctrl-G toggles the git lens overlay (works in both view and edit
    // modes — but `toggle_git_lens` itself refuses while editing).
    if key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.toggle_git_lens();
        return Ok(());
    }
    // Git lens captures j/k/PgUp/PgDn for scrolling within the diff, plus
    // Esc/q to dismiss. Anything else falls through to the normal handler
    // so the user can e.g. press `?` for help while the lens is open.
    if app.git_lens.is_some() {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                app.git_lens = None;
                return Ok(());
            }
            KeyCode::Char('j') | KeyCode::Down => {
                app.git_lens_scroll(1);
                return Ok(());
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.git_lens_scroll(-1);
                return Ok(());
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                let h = app.viewport.height as i32;
                app.git_lens_scroll(h.max(1));
                return Ok(());
            }
            KeyCode::PageUp => {
                let h = app.viewport.height as i32;
                app.git_lens_scroll(-h.max(1));
                return Ok(());
            }
            _ => {}
        }
    }
    // TOC overlay: j/k move the selection, Enter jumps, Esc/q/t dismiss.
    if app.toc.is_some() {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('t') => {
                app.toc = None;
                return Ok(());
            }
            KeyCode::Char('j') | KeyCode::Down => {
                app.toc_move(1);
                return Ok(());
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.toc_move(-1);
                return Ok(());
            }
            KeyCode::PageDown => {
                app.toc_move(10);
                return Ok(());
            }
            KeyCode::PageUp => {
                app.toc_move(-10);
                return Ok(());
            }
            KeyCode::Enter => {
                let line = match (&app.view, app.toc) {
                    (View::Reader(r), Some(t)) => r
                        .rendered
                        .as_ref()
                        .and_then(|rd| rd.headings.get(t.selected))
                        .map(|h| h.line),
                    _ => None,
                };
                app.toc = None;
                if let Some(line) = line {
                    scroll_to(app, line.min(u16::MAX as usize) as u16);
                }
                return Ok(());
            }
            _ => {}
        }
        return Ok(());
    }
    if app.search.is_some() {
        return handle_search_key(app, key);
    }
    // Browser type-ahead find (lf-style): once `f` arms it, characters extend
    // the query and the selection live-jumps. Captures all keys until dismissed.
    if let View::Browser(b) = &app.view {
        if b.find.is_some() {
            return handle_browser_find_key(app, key);
        }
    }
    if let View::Reader(r) = &app.view {
        if r.doc_search.as_ref().map(|s| s.editing).unwrap_or(false) {
            return handle_doc_search_key(app, key);
        }
        // Edit mode: keys go to the in-house editor instead of the viewer.
        if r.edit.is_some() {
            return handle_edit_key(app, key);
        }
    }

    // Time-out stale chord state before interpreting the next key.
    let now = std::time::Instant::now();
    if let Some(t) = app.pending_g {
        if now.duration_since(t).as_millis() > CHORD_TIMEOUT_MS {
            app.pending_g = None;
        }
    }
    if let Some(t) = app.pending_z {
        if now.duration_since(t).as_millis() > CHORD_TIMEOUT_MS {
            app.pending_z = None;
        }
    }
    if let Some((_, t)) = app.pending_bracket {
        if now.duration_since(t).as_millis() > CHORD_TIMEOUT_MS {
            app.pending_bracket = None;
        }
    }

    // Resolve `gg` / `gh` / `zz` chord completions before falling into the
    // main match.
    if app.pending_g.is_some() {
        app.pending_g = None;
        if let KeyCode::Char('g') = key.code {
            scroll_to(app, 0);
            app.count_prefix = None;
            return Ok(());
        }
        // `gh` toggles local ↔ HackMD from anywhere (including the Reader,
        // where plain `H` keeps its vim viewport meaning).
        if let KeyCode::Char('h') = key.code {
            app.toggle_cloud_mode();
            app.count_prefix = None;
            return Ok(());
        }
        // Anything else: cancel the chord and fall through to normal handling.
    }
    if app.pending_z.is_some() {
        app.pending_z = None;
        if let KeyCode::Char('z') = key.code {
            center_focus_or_top(app);
            app.count_prefix = None;
            return Ok(());
        }
    }
    // `]]` / `[[` — jump to the next / previous heading (vim section motion,
    // count-aware: `3]]` skips three headings forward).
    if let Some((bracket, _)) = app.pending_bracket {
        app.pending_bracket = None;
        if key.code == KeyCode::Char(bracket) {
            let n = consume_count(&mut app.count_prefix);
            app.jump_heading(if bracket == ']' { n } else { -n });
            return Ok(());
        }
    }

    // Numeric prefix: digits are buffered into `count_prefix` until a motion
    // key consumes them. Plain `0` resets the count *only* if no count is
    // already being typed (matches vim — `0` on its own is "go to col 0",
    // which doesn't apply to a viewer).
    if let KeyCode::Char(c) = key.code {
        if !key.modifiers.contains(KeyModifiers::CONTROL)
            && c.is_ascii_digit()
            && (c != '0' || app.count_prefix.is_some())
        {
            let d = c.to_digit(10).unwrap();
            let cur = app.count_prefix.unwrap_or(0);
            // Cap at 99,999 to keep behavior sane on accidental key-mash.
            let next = (cur.saturating_mul(10)).saturating_add(d).min(99_999);
            app.count_prefix = Some(next);
            return Ok(());
        }
    }

    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Esc => {
            // Cancel any in-progress count or chord first.
            if app.count_prefix.is_some()
                || app.pending_g.is_some()
                || app.pending_z.is_some()
                || app.pending_bracket.is_some()
            {
                app.count_prefix = None;
                app.pending_g = None;
                app.pending_z = None;
                app.pending_bracket = None;
                return Ok(());
            }
            // Dismiss any committed in-doc search overlay.
            if let View::Reader(r) = &app.view {
                if r.doc_search.is_some() {
                    app.close_doc_search();
                    return Ok(());
                }
            }
            if !app.history.is_empty() {
                app.go_back()?;
            } else {
                app.should_quit = true;
            }
        }
        KeyCode::Char('?') => app.help_open = !app.help_open,
        KeyCode::Char('/') => match &app.view {
            View::Reader(_) => app.open_doc_search(),
            // The fuzzy file search indexes local files only; cloud notes
            // are browsed via their flat list.
            View::Browser(_) | View::Cloud(_) => app.open_search(),
        },
        KeyCode::Char('T') => app.open_search(),
        // `n` creates a note in the cloud browser and a file in the local
        // browser; in the Reader it's the doc-search "next match" motion.
        KeyCode::Char('n') => match &app.view {
            View::Cloud(_) => app.prompt_new_note(),
            View::Browser(_) => app.prompt_new_file(),
            _ => app.doc_search_step(true),
        },
        // `c` / F2 renames the selected entry in the local browser.
        KeyCode::Char('c') | KeyCode::F(2) if matches!(app.view, View::Browser(_)) => {
            app.prompt_rename();
        }
        KeyCode::Char('N') => app.doc_search_step(false),
        KeyCode::Char('m') => toggle_mouse(app),
        // `e` and vim's `i` both open the editor (which starts in insert
        // mode, so `i` lands exactly where a vim user expects).
        KeyCode::Char('e') | KeyCode::Char('i') => app.enter_edit(),
        // `A` opens the editor at the end of the document (append); `O` opens
        // it with a fresh blank line at the top. (`o` is taken by open-link.)
        KeyCode::Char('A') if matches!(app.view, View::Reader(_)) => app.enter_edit_append(),
        KeyCode::Char('O') if matches!(app.view, View::Reader(_)) => app.enter_edit_open_above(),

        // HackMD note actions — active wherever a cloud note is targeted
        // (cloud browser row or open cloud reader); no-ops elsewhere.
        KeyCode::Char('D') if app.cloud_target().is_some() => app.prompt_delete(),
        KeyCode::Char('P') if app.cloud_target().is_some() => app.publish_toggle(),
        KeyCode::Char('S') if app.cloud_target().is_some() => app.prompt_download(),
        // Copy the publish link of the open cloud note (reader only — the
        // browser rows don't carry the link).
        KeyCode::Char('y') if matches!(&app.view, View::Reader(r) if matches!(r.origin, crate::tui::app::ReaderOrigin::CloudNote { .. })) =>
        {
            copy_publish_link(app);
        }
        // Publish a local file to HackMD — creates a new note the first time
        // (stamping the file with a managed link block) and updates the linked
        // note on subsequent pushes.
        KeyCode::Char('U') => {
            if let Some(path) = local_push_target(app) {
                app.push_local(path);
            }
        }

        // Vim-style scrolling. Ctrl-modified arms come *before* unguarded
        // letter arms so the modifier path can match.
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let n = consume_count(&mut app.count_prefix);
            scroll_by_page(app, n, true);
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let n = consume_count(&mut app.count_prefix);
            scroll_by_page(app, -n, true);
        }
        KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let n = consume_count(&mut app.count_prefix);
            scroll_by_page(app, n, false);
        }
        KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let n = consume_count(&mut app.count_prefix);
            scroll_by_page(app, -n, false);
        }

        // History back, vim-jumplist style. (Ctrl-I, vim's forward, is
        // indistinguishable from Tab in most terminals, so forward stays
        // on `l` only.)
        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => app.go_back()?,
        KeyCode::Backspace => app.go_back()?,

        // `f` arms type-ahead find in the browser; in the Reader it pages like
        // less/more (mirrors Ctrl-F).
        KeyCode::Char('f') => {
            if let View::Browser(b) = &mut app.view {
                b.find = Some(String::new());
            } else {
                let n = consume_count(&mut app.count_prefix);
                scroll_by_page(app, n, false);
            }
        }
        KeyCode::Char('b') => {
            let n = consume_count(&mut app.count_prefix);
            scroll_by_page(app, -n, false);
        }

        KeyCode::Char('j') | KeyCode::Down => {
            let n = consume_count(&mut app.count_prefix);
            scroll_by(app, n);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            let n = consume_count(&mut app.count_prefix);
            scroll_by(app, -n);
        }
        // Lowercase shortcuts kept for parity with prior bindings.
        KeyCode::Char('d') => {
            let n = consume_count(&mut app.count_prefix);
            scroll_by_page(app, n, true);
        }
        KeyCode::Char('u') => {
            let n = consume_count(&mut app.count_prefix);
            scroll_by_page(app, -n, true);
        }
        KeyCode::PageDown | KeyCode::Char(' ') => {
            let n = consume_count(&mut app.count_prefix);
            scroll_by_page(app, n, false);
        }
        KeyCode::PageUp => {
            let n = consume_count(&mut app.count_prefix);
            scroll_by_page(app, -n, false);
        }

        // `g` is now the first key of `gg`. The count is preserved for the
        // chord completion to consume.
        KeyCode::Char('g') => {
            app.pending_g = Some(std::time::Instant::now());
        }
        KeyCode::Char('G') | KeyCode::End => {
            let n = consume_count(&mut app.count_prefix);
            if n > 1 {
                scroll_to(app, (n - 1).max(0) as u16);
            } else {
                scroll_to(app, u16::MAX);
            }
        }
        KeyCode::Home => scroll_to(app, 0),

        // `z` is the first key of `zz` (center on focus). Lowercase only —
        // uppercase Z is unbound.
        KeyCode::Char('z') => {
            app.pending_z = Some(std::time::Instant::now());
        }

        // `]` / `[` arm the `]]` / `[[` heading-jump chords. The count is
        // preserved for the chord completion to consume.
        KeyCode::Char(c @ (']' | '[')) => {
            app.pending_bracket = Some((c, std::time::Instant::now()));
        }

        // Table of contents (Reader only; no-op elsewhere).
        KeyCode::Char('t') => app.open_toc(),

        // `H` in browser views toggles local ↔ HackMD; in the Reader it keeps
        // its vim viewport-relative meaning (use `gh` there instead).
        KeyCode::Char('H') => match &app.view {
            View::Reader(_) => scroll_viewport_relative(app, ViewportTarget::Top),
            View::Browser(_) | View::Cloud(_) => app.toggle_cloud_mode(),
        },
        KeyCode::Char('M') => scroll_viewport_relative(app, ViewportTarget::Middle),
        KeyCode::Char('L') => scroll_viewport_relative(app, ViewportTarget::Bottom),

        // Cloud browser: Tab cycles workspace tabs. Elsewhere focus walks
        // links AND checkboxes.
        KeyCode::Tab => match &mut app.view {
            View::Cloud(c) => c.switch_tab(1),
            _ => focus_next(app),
        },
        KeyCode::BackTab => match &mut app.view {
            View::Cloud(c) => c.switch_tab(-1),
            _ => focus_prev(app),
        },
        KeyCode::Enter => activate(app)?,
        KeyCode::Char('o') => open_focused(app)?,

        // Cloud browser: refetch the note lists.
        KeyCode::Char('R') if matches!(app.view, View::Cloud(_)) => {
            app.refresh_cloud_lists();
        }

        // Browser only: mark the selected entry read. On a directory this marks
        // every text file under it read recursively, clearing its `[unread]`
        // badge (and those of its descendants). `../` is a deliberate no-op so
        // the whole parent subtree can't be cleared by accident.
        KeyCode::Char('r') => {
            let target = if let View::Browser(b) = &app.view {
                b.entries
                    .get(b.selected)
                    .filter(|e| !matches!(e.kind, BrowserEntryKind::ParentDir))
                    .map(|e| e.path.clone())
            } else {
                None
            };
            if let Some(path) = target {
                app.read_state.mark_read_recursive(&path);
                app.read_state.flush();
            }
        }

        // Browser navigation arrows.
        KeyCode::Right => enter_or_open(app)?,
        KeyCode::Left => {
            if matches!(app.view, View::Browser(_)) {
                app.go_back()?;
            }
        }
        // `h`/`l` only walk history when not in a count chord. (No conflict
        // with `gh` etc. since we resolved `pending_g` above already.)
        KeyCode::Char('h') => app.go_back()?,
        KeyCode::Char('l') => app.go_forward()?,

        _ => {}
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum ViewportTarget {
    Top,
    Middle,
    Bottom,
}

/// Vim H/M/L: place the focused element (or the visible top/middle/bottom
/// content line) without changing the underlying buffer position. Here, since
/// we have no separate "cursor line" beyond focus, H/M/L instead pick a focus
/// target visible on that part of the viewport.
fn scroll_viewport_relative(app: &mut App, target: ViewportTarget) {
    let h = app.viewport.height as usize;
    if let View::Reader(r) = &mut app.view {
        let Some(rendered) = r.rendered.as_ref() else {
            return;
        };
        let scroll = r.scroll as usize;
        let last = rendered.lines.len().saturating_sub(1);
        let want_line = match target {
            ViewportTarget::Top => scroll,
            ViewportTarget::Middle => (scroll + h / 2).min(last),
            ViewportTarget::Bottom => (scroll + h.saturating_sub(1)).min(last),
        };
        // Pick the focusable nearest to that line, if any.
        let mut targets = r.focus_targets();
        if targets.is_empty() {
            return;
        }
        targets.sort_by_key(|&(_, line, _)| (line as i64 - want_line as i64).abs());
        r.focus = Some(targets[0].0);
    }
}

/// Center the focus line in the viewport. Falls back to centering the top of
/// the buffer when nothing is focused.
fn center_focus_or_top(app: &mut App) {
    let h = app.viewport.height as usize;
    if let View::Reader(r) = &mut app.view {
        let line = r.focus_position().map(|(l, _)| l).unwrap_or(0);
        let total = r.rendered.as_ref().map(|x| x.lines.len()).unwrap_or(0);
        let max_scroll = total.saturating_sub(h);
        r.scroll = line.saturating_sub(h / 2).min(max_scroll) as u16;
    }
}

/// Browser type-ahead find handler. Active while `Browser::find.is_some()`.
/// Typed characters extend the query and the selection live-jumps to the first
/// anchored, smartcase match (forward from the top). `;`/`,` cycle to the
/// next/previous match of the current query; Backspace edits; Enter opens the
/// landed entry; Esc (or any other key) leaves find mode.
fn handle_browser_find_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::Browser(b) = &mut app.view else {
        return Ok(());
    };
    let Some(query) = b.find.clone() else {
        return Ok(());
    };
    match key.code {
        KeyCode::Esc => {
            b.find = None;
        }
        KeyCode::Enter => {
            b.find = None;
            activate(app)?;
        }
        KeyCode::Backspace => {
            let mut q = query;
            q.pop();
            if let Some(i) = b.find_from(&q, 0) {
                b.selected = i;
            }
            b.find = Some(q);
            keep_browser_selection_visible(app);
        }
        // Cycle to the next / previous match of the current query without
        // changing it (lf's find-next `;` / find-prev `,`).
        KeyCode::Char(';') if !query.is_empty() => {
            let start = (b.selected + 1) % b.entries.len().max(1);
            if let Some(i) = b.find_from(&query, start) {
                b.selected = i;
            }
            keep_browser_selection_visible(app);
        }
        KeyCode::Char(',') if !query.is_empty() => {
            if let Some(i) = b.find_back_from(&query, b.selected) {
                b.selected = i;
            }
            keep_browser_selection_visible(app);
        }
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            let mut q = query;
            q.push(c);
            // Live-jump to the first match. If nothing matches, keep the query
            // (so Backspace recovers) but leave the cursor where it was.
            if let Some(i) = b.find_from(&q, 0) {
                b.selected = i;
            }
            b.find = Some(q);
            keep_browser_selection_visible(app);
        }
        // Any other key (arrows, Tab, etc.) ends find mode and is ignored this
        // press; the user can repeat it to act normally.
        _ => {
            b.find = None;
        }
    }
    Ok(())
}

/// Clamp `Browser::scroll` so the selected row stays on screen after a find
/// jump. Mirrors the visibility math in `scroll_by`.
fn keep_browser_selection_visible(app: &mut App) {
    let h = app.viewport.height.saturating_sub(2) as usize;
    if let View::Browser(b) = &mut app.view {
        if b.selected < b.scroll as usize {
            b.scroll = b.selected as u16;
        } else if b.selected >= b.scroll as usize + h.max(1) {
            b.scroll = (b.selected + 1 - h.max(1)) as u16;
        }
    }
}

/// Edit-mode key handler. Active while `Reader::edit.is_some()`. Plain text
/// goes into the buffer (insert mode is the default); arrows/home/end/etc.
/// move the source cursor; Ctrl-S (or Ctrl-W backup) saves. The first Esc
/// opens the vim-style command line on the statusline (`:w`, `:wq`, `:q`,
/// `:preview`, …); a second Esc there discards changes and exits. Mouse
/// handling stays in `handle_mouse` and updates the cursor from click
/// position there.
fn handle_edit_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    // Ctrl-C is the hard escape hatch (handled above before we get here in
    // practice, but kept defensively in case dispatch shifts).
    if ctrl && matches!(key.code, KeyCode::Char('c')) {
        app.should_quit = true;
        return Ok(());
    }

    // `:preview` overlay — the unsaved buffer rendered like the reader.
    // A single Esc drops back to the editor; scroll keys page the preview.
    if matches!(&app.view, View::Reader(r) if r.edit.as_ref().map(|e| e.preview_full).unwrap_or(false))
    {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => close_edit_preview(app),
            KeyCode::Char('j') | KeyCode::Down => scroll_by(app, 1),
            KeyCode::Char('k') | KeyCode::Up => scroll_by(app, -1),
            KeyCode::PageDown | KeyCode::Char(' ') | KeyCode::Char('f') => {
                scroll_by_page(app, 1, false)
            }
            KeyCode::PageUp | KeyCode::Char('b') => scroll_by_page(app, -1, false),
            KeyCode::Char('d') => scroll_by_page(app, 1, true),
            KeyCode::Char('u') => scroll_by_page(app, -1, true),
            KeyCode::Char('g') => scroll_to(app, 0),
            KeyCode::Char('G') | KeyCode::End => scroll_to(app, u16::MAX),
            _ => {}
        }
        return Ok(());
    }

    // Command line open ("first Esc"): keys edit the command, Tab accepts
    // the grey suggestion, Enter executes, Esc is the non-vim escape hatch
    // (discard changes + exit the editor).
    if matches!(&app.view, View::Reader(r) if r.edit.as_ref().map(|e| e.command.is_some()).unwrap_or(false))
    {
        return handle_edit_command_key(app, key);
    }

    // An active drag-selection captures the next key: y copies, Backspace/
    // Delete deletes, Esc cancels. Any other key drops the selection and is
    // then handled normally (it does NOT replace the selected text).
    let sel_active = matches!(
        &app.view,
        View::Reader(r) if r
            .edit
            .as_ref()
            .and_then(|e| e.selection.as_ref())
            .map(|s| s.is_active())
            .unwrap_or(false)
    );
    if sel_active {
        match key.code {
            KeyCode::Char('y') if !ctrl && !alt => {
                if let Some(text) = app.edit_selection_text() {
                    copy_to_clipboard(&text);
                    app.status = format!("Copied {} chars", text.chars().count());
                    app.yank = Some(text);
                }
                app.edit_clear_selection();
                return Ok(());
            }
            // Cut: copy the selection to the clipboard/register, then delete
            // it. `x` mirrors the copy `y`; Ctrl-X is the GUI binding.
            KeyCode::Char('x') if !alt => {
                if let Some(text) = app.edit_selection_text() {
                    copy_to_clipboard(&text);
                    app.status = format!("Cut {} chars", text.chars().count());
                    app.yank = Some(text);
                }
                app.edit_delete_selection();
                return Ok(());
            }
            // Paste over the selection: overwrite the selected text with the
            // clipboard/register contents. `p` mirrors copy/cut; Ctrl-V is the
            // GUI binding.
            KeyCode::Char('p') if !ctrl && !alt => {
                paste_over_selection(app);
                return Ok(());
            }
            KeyCode::Char('v') if ctrl => {
                paste_over_selection(app);
                return Ok(());
            }
            KeyCode::Delete | KeyCode::Backspace => {
                app.edit_delete_selection();
                return Ok(());
            }
            // Pair completion over a selection: `*`/`_`/`` ` ``/`(`/`[`/`{`
            // wrap the selected text in that delimiter.
            KeyCode::Char(c) if !ctrl && !alt && pair_for(c).is_some() => {
                let (open, close) = pair_for(c).unwrap();
                app.edit_wrap_selection(open, close);
                return Ok(());
            }
            KeyCode::Esc => {
                app.edit_clear_selection();
                return Ok(());
            }
            _ => app.edit_clear_selection(),
        }
    }

    // Paste at the cursor (no selection). Ctrl-V is the universal GUI binding;
    // it works whether or not text is selected (the selection branch above
    // handles the paste-over case).
    if ctrl && matches!(key.code, KeyCode::Char('v')) {
        paste_over_selection(app);
        return Ok(());
    }

    // Word-level movement and deletion (Alt-arrow / Alt-Backspace / Alt-
    // Delete on macOS, Ctrl-arrow / Ctrl-Backspace on Linux/Windows).
    if alt || ctrl {
        match key.code {
            KeyCode::Left => {
                app.edit_move_word(-1);
                return Ok(());
            }
            KeyCode::Right => {
                app.edit_move_word(1);
                return Ok(());
            }
            KeyCode::Backspace => {
                app.edit_delete_word(false);
                return Ok(());
            }
            KeyCode::Delete => {
                app.edit_delete_word(true);
                return Ok(());
            }
            _ => {}
        }
    }
    // macOS terminals (Terminal.app, iTerm default profile) send Option-
    // arrow as the emacs sequences `Esc b` / `Esc f`, which crossterm
    // reports as Alt-b / Alt-f rather than an Alt-modified arrow. `Esc d`
    // (Alt-d) is the matching emacs forward word delete.
    if alt {
        match key.code {
            KeyCode::Char('b') => {
                app.edit_move_word(-1);
                return Ok(());
            }
            KeyCode::Char('f') => {
                app.edit_move_word(1);
                return Ok(());
            }
            KeyCode::Char('d') => {
                app.edit_delete_word(true);
                return Ok(());
            }
            _ => {}
        }
    }

    // Save: Ctrl-S primary, Ctrl-W backup (some terminals eat Ctrl-S as
    // XOFF / flow control).
    if ctrl && matches!(key.code, KeyCode::Char('s') | KeyCode::Char('w')) {
        app.save_edit()?;
        return Ok(());
    }

    // Undo / redo. Ctrl-Y is the more common second binding alongside the
    // canonical Ctrl-Z; Ctrl-Shift-Z would be cleaner but most terminals
    // can't disambiguate it from plain Ctrl-Z.
    if ctrl && matches!(key.code, KeyCode::Char('z')) {
        app.edit_undo();
        return Ok(());
    }
    if ctrl && matches!(key.code, KeyCode::Char('y') | KeyCode::Char('r')) {
        app.edit_redo();
        return Ok(());
    }

    match key.code {
        KeyCode::Esc => {
            // First Esc moves the cursor to the statusline command place
            // (vim-style). From there the user types a command (`:wq`,
            // `:q`, `:preview`, …) or presses Esc again to discard changes
            // and leave the editor.
            if let View::Reader(r) = &mut app.view {
                if let Some(e) = r.edit.as_mut() {
                    e.command = Some(String::new());
                }
            }
            // Fresh command line starts at the live entry, not mid-history.
            app.edit_cmd_nav = None;
            return Ok(());
        }
        KeyCode::Left => app.edit_move_horizontal(-1),
        KeyCode::Right => app.edit_move_horizontal(1),
        KeyCode::Up => app.edit_move_vertical(-1),
        KeyCode::Down => app.edit_move_vertical(1),
        KeyCode::Home => app.edit_move_line_edge(false),
        KeyCode::End => app.edit_move_line_edge(true),
        KeyCode::Backspace => app.edit_backspace(),
        KeyCode::Delete => app.edit_delete(),
        KeyCode::Enter => app.edit_newline(),
        KeyCode::Tab => app.edit_insert("  "),
        KeyCode::Char(c) if !ctrl && !alt => {
            // Bracket auto-close: `(`/`[`/`{` insert the matching closer and
            // sit the cursor between. Typing the closer when it's already
            // under the cursor steps over it instead of duplicating. Emphasis
            // (`*`/`_`/`` ` ``) is NOT auto-closed here — that would break
            // bullet/maths entry — but it does wrap a selection (handled
            // above).
            if let Some((open, close)) = bracket_open_pair(c) {
                app.edit_insert_pair(open, close);
            } else if is_bracket_closer(c) && app.edit_try_type_over(c) {
                // Stepped over an existing closer — nothing more to do.
            } else {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                app.edit_insert(s);
            }
        }
        _ => {}
    }
    Ok(())
}

/// Delimiter pair for pair-completion over a selection. Symmetric for
/// emphasis (`*`/`_`/`` ` ``), matched brackets otherwise.
fn pair_for(c: char) -> Option<(char, char)> {
    match c {
        '(' => Some(('(', ')')),
        '[' => Some(('[', ']')),
        '{' => Some(('{', '}')),
        '*' => Some(('*', '*')),
        '_' => Some(('_', '_')),
        '`' => Some(('`', '`')),
        _ => None,
    }
}

/// Opening brackets that auto-close on insert (no selection). Emphasis is
/// deliberately excluded so a lone `*`/`_` doesn't fight bullet/maths entry.
fn bracket_open_pair(c: char) -> Option<(char, char)> {
    match c {
        '(' => Some(('(', ')')),
        '[' => Some(('[', ']')),
        '{' => Some(('{', '}')),
        _ => None,
    }
}

/// The closing brackets matching [`bracket_open_pair`], for type-over.
fn is_bracket_closer(c: char) -> bool {
    matches!(c, ')' | ']' | '}')
}

/// Editor commands the command line accepts, in suggestion order. `:` is
/// implicit — the stored input never includes it.
const EDIT_COMMANDS: &[&str] = &["w", "wq", "x", "q", "q!", "preview", "e!"];

/// The grey inline completion for a partial command: the first command the
/// input is a strict prefix of. `None` for empty input, exact matches, and
/// no-match input.
pub(crate) fn complete_edit_command(input: &str) -> Option<&'static str> {
    if input.is_empty() {
        return None;
    }
    EDIT_COMMANDS
        .iter()
        .find(|c| c.starts_with(input) && c.len() > input.len())
        .copied()
}

/// Keystrokes while the editor command line is open.
fn handle_edit_command_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let dirty =
        matches!(&app.view, View::Reader(r) if r.edit.as_ref().map(|e| e.dirty).unwrap_or(false));
    match key.code {
        // Second Esc: the non-vim exit — discard changes, back to preview.
        KeyCode::Esc => {
            if dirty {
                app.exit_edit_discard();
            } else {
                app.exit_edit();
            }
        }
        KeyCode::Enter => {
            let input = match &mut app.view {
                View::Reader(r) => r.edit.as_mut().and_then(|e| e.command.take()),
                _ => None,
            };
            if let Some(input) = input {
                let cmd = input.trim().trim_start_matches(':').trim().to_string();
                // Record in history (skip blanks and consecutive duplicates).
                if !cmd.is_empty() && app.edit_cmd_history.last() != Some(&cmd) {
                    app.edit_cmd_history.push(cmd.clone());
                }
                app.edit_cmd_nav = None;
                exec_edit_command(app, &cmd)?;
            }
        }
        // ↑/↓ walk the command history (most-recent first), like a shell.
        KeyCode::Up => cmd_history_prev(app),
        KeyCode::Down => cmd_history_next(app),
        KeyCode::Tab => {
            if let View::Reader(r) = &mut app.view {
                if let Some(cmd) = r.edit.as_mut().and_then(|e| e.command.as_mut()) {
                    if let Some(full) = complete_edit_command(cmd) {
                        *cmd = full.to_string();
                    }
                }
            }
        }
        KeyCode::Backspace => {
            if let View::Reader(r) = &mut app.view {
                if let Some(e) = r.edit.as_mut() {
                    match e.command.as_mut().and_then(|c| c.pop()) {
                        Some(_) => {}
                        // Backspace on an empty command line aborts it,
                        // returning the cursor to the buffer.
                        None => e.command = None,
                    }
                }
            }
        }
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            // A leading ':' is implicit (the UI already shows it); typing
            // one anyway is tolerated and folded away.
            if let View::Reader(r) = &mut app.view {
                if let Some(cmd) = r.edit.as_mut().and_then(|e| e.command.as_mut()) {
                    if !(c == ':' && cmd.is_empty()) {
                        cmd.push(c);
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Execute a (already `:`-stripped) editor command.
fn exec_edit_command(app: &mut App, cmd: &str) -> Result<()> {
    let dirty =
        matches!(&app.view, View::Reader(r) if r.edit.as_ref().map(|e| e.dirty).unwrap_or(false));
    match cmd {
        // Empty Enter: back to insert mode.
        "" => {}
        "w" => app.save_edit()?,
        "wq" | "x" => {
            app.save_edit()?;
            app.exit_edit();
        }
        // Per the project's UX (a viewer-first app), `:q` leaves the editor
        // and drops unsaved changes — the statusline hint says so.
        "q" | "q!" => {
            if dirty {
                app.exit_edit_discard();
            } else {
                app.exit_edit();
            }
        }
        "preview" => open_edit_preview(app),
        // `:e!` — reload the buffer from disk / cloud cache, dropping
        // unsaved edits but staying in the editor.
        "e!" => edit_reload(app),
        other => {
            // `:s/old/new/[g]` (current line) and `:%s/old/new/[g]` (whole
            // buffer) — literal substitution (no regex).
            if looks_like_substitute(other) {
                edit_substitute(app, other);
            } else if let Ok(n) = other.parse::<usize>() {
                // `:N` — move the cursor to the start of line N (1-based,
                // clamped to the buffer like vim).
                edit_goto_line(app, n);
            } else {
                app.status = format!("Not an editor command: :{other}");
            }
        }
    }
    Ok(())
}

/// Replace the open command-line text with `val`. No-op if the command line
/// isn't open.
fn set_command_line(app: &mut App, val: String) {
    if let View::Reader(r) = &mut app.view {
        if let Some(e) = r.edit.as_mut() {
            if e.command.is_some() {
                e.command = Some(val);
            }
        }
    }
}

/// ↑ on the command line: step to an older history entry.
fn cmd_history_prev(app: &mut App) {
    if app.edit_cmd_history.is_empty() {
        return;
    }
    let idx = match app.edit_cmd_nav {
        None => app.edit_cmd_history.len() - 1,
        Some(i) => i.saturating_sub(1),
    };
    app.edit_cmd_nav = Some(idx);
    set_command_line(app, app.edit_cmd_history[idx].clone());
}

/// ↓ on the command line: step to a newer entry, or back to a blank live line
/// past the newest.
fn cmd_history_next(app: &mut App) {
    let Some(i) = app.edit_cmd_nav else {
        return;
    };
    if i + 1 < app.edit_cmd_history.len() {
        app.edit_cmd_nav = Some(i + 1);
        set_command_line(app, app.edit_cmd_history[i + 1].clone());
    } else {
        app.edit_cmd_nav = None;
        set_command_line(app, String::new());
    }
}

/// True when `cmd` looks like a substitute command: an optional `%`, then `s`,
/// then a non-alphanumeric delimiter (so `:s` collides with nothing real).
fn looks_like_substitute(cmd: &str) -> bool {
    let rest = cmd.strip_prefix('%').unwrap_or(cmd);
    let mut chars = rest.chars();
    chars.next() == Some('s')
        && matches!(chars.next(), Some(d) if !d.is_alphanumeric() && !d.is_whitespace())
}

/// Apply a literal `:s`/`:%s` substitution. The delimiter is whatever single
/// char follows `s` (vim-style), so `s#a#b#` works too. Flags: `g` (all
/// occurrences per line). Without `%`, only the cursor's line is touched.
fn edit_substitute(app: &mut App, cmd: &str) {
    let all_lines = cmd.starts_with('%');
    let rest = cmd.strip_prefix('%').unwrap_or(cmd);
    // Skip the leading `s`; the next char is the delimiter.
    let body = &rest[1..];
    let Some(delim) = body.chars().next() else {
        app.status = "Bad substitute (no delimiter)".into();
        return;
    };
    // pattern / replacement / flags — at most 3 fields; a missing closing
    // delimiter just means empty flags.
    let mut parts = body[delim.len_utf8()..].splitn(3, delim);
    let pattern = parts.next().unwrap_or("");
    let replacement = parts.next().unwrap_or("");
    let flags = parts.next().unwrap_or("");
    if pattern.is_empty() {
        app.status = "Bad substitute (empty pattern)".into();
        return;
    }
    let global = flags.contains('g');

    let View::Reader(r) = &mut app.view else {
        return;
    };
    if r.edit.is_none() {
        return;
    }
    let cursor = r.edit.as_ref().unwrap().cursor.min(r.raw.len());
    // Resolve the line range to operate on.
    let (from, to) = if all_lines {
        (0, r.raw.len())
    } else {
        let start = r.raw[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let end = r.raw[cursor..]
            .find('\n')
            .map(|i| cursor + i)
            .unwrap_or(r.raw.len());
        (start, end)
    };
    let region = &r.raw[from..to];
    let (replaced, count) = substitute_region(region, pattern, replacement, global);
    if count == 0 {
        app.status = format!("Pattern not found: {pattern}");
        return;
    }
    crate::tui::app::push_undo(r);
    r.raw.replace_range(from..to, &replaced);
    let e = r.edit.as_mut().unwrap();
    e.cursor = from.min(r.raw.len());
    e.dirty = true;
    e.command = None;
    r.rendered = None;
    app.status = format!("{count} substitution{}", if count == 1 { "" } else { "s" });
}

/// Literal substitution over `region`. With `global`, every occurrence is
/// replaced; otherwise only the first on each line. Returns the rewritten
/// text and the number of replacements made.
fn substitute_region(region: &str, pattern: &str, replacement: &str, global: bool) -> (String, usize) {
    if global {
        (
            region.replace(pattern, replacement),
            region.matches(pattern).count(),
        )
    } else {
        let mut out = String::with_capacity(region.len());
        let mut n = 0;
        for line in region.split_inclusive('\n') {
            if let Some(pos) = line.find(pattern) {
                out.push_str(&line[..pos]);
                out.push_str(replacement);
                out.push_str(&line[pos + pattern.len()..]);
                n += 1;
            } else {
                out.push_str(line);
            }
        }
        (out, n)
    }
}

/// `:N` — place the edit cursor at the start of source line `n` (1-based).
/// Out-of-range lands on the last line, `:0` on the first — vim's clamping.
fn edit_goto_line(app: &mut App, n: usize) {
    if let View::Reader(r) = &mut app.view {
        let Some(e) = r.edit.as_mut() else { return };
        let target = n.saturating_sub(1);
        let mut offset = 0usize;
        for (i, line) in r.raw.split_inclusive('\n').enumerate() {
            if i == target {
                break;
            }
            offset += line.len();
        }
        e.cursor = offset.min(r.raw.len());
        e.last_drawn_cursor = None;
        r.rendered = None;
    }
}

/// `:e!` — reload from the origin, discarding unsaved edits, but stay in
/// the editor. The undo history is dropped with the buffer it described.
fn edit_reload(app: &mut App) {
    let cached_raw = match &app.view {
        View::Reader(r) => match &r.origin {
            crate::tui::app::ReaderOrigin::CloudNote { id, .. } => {
                app.cloud.note_cache.get(id).map(|c| c.note.content.clone())
            }
            _ => None,
        },
        _ => None,
    };
    let reloaded = {
        let View::Reader(r) = &mut app.view else {
            return;
        };
        if r.edit.is_none() {
            return;
        }
        let fresh = match r.origin.clone() {
            crate::tui::app::ReaderOrigin::File(path) => std::fs::read_to_string(&path).ok(),
            crate::tui::app::ReaderOrigin::CloudNote { .. } => cached_raw,
            crate::tui::app::ReaderOrigin::Stdin => None,
        };
        match fresh {
            Some(fresh) => {
                r.raw = fresh;
                let e = r.edit.as_mut().unwrap();
                e.cursor = 0;
                e.dirty = false;
                e.undo.clear();
                e.redo.clear();
                e.last_drawn_cursor = None;
                r.rendered = None;
                true
            }
            None => false,
        }
    };
    app.status = if reloaded {
        "Buffer reloaded"
    } else {
        "Nothing to reload"
    }
    .into();
}

/// `:preview` — render the unsaved buffer full-screen like the reader.
fn open_edit_preview(app: &mut App) {
    if let View::Reader(r) = &mut app.view {
        if let Some(e) = r.edit.as_mut() {
            e.preview_full = true;
            e.command = None;
            r.scroll = 0;
            r.rendered = None;
        }
    }
}

/// Esc from the `:preview` overlay: back to the two-pane editor, with the
/// raw pane re-following the cursor (the overlay reused `scroll`).
fn close_edit_preview(app: &mut App) {
    if let View::Reader(r) = &mut app.view {
        if let Some(e) = r.edit.as_mut() {
            e.preview_full = false;
            e.last_drawn_cursor = None;
            r.scroll = 0;
            r.rendered = None;
        }
    }
}

/// Keystrokes while a modal prompt is open. Text prompts mirror the
/// doc-search input handling; the delete confirmation accepts only
/// `y`/Enter and treats everything else as a cancel.
/// Keystrokes while the full-screen conflict resolver is open. `j`/`k` (and
/// arrows) move between hunks; `l`/`u`/`b`/`n` (and `1`/`2`) pick a side for
/// the focused hunk and auto-advance; Enter applies the resolution; Esc
/// cancels (leaving both files untouched).
fn handle_conflict_key(app: &mut App, key: KeyEvent) -> Result<()> {
    use crate::tui::app::ConflictChoice;
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.should_quit = true;
        return Ok(());
    }
    let Some(st) = app.conflict.as_mut() else {
        return Ok(());
    };
    // After choosing a side, jump to the next still-unresolved hunk so the
    // user can rip through conflicts without manual navigation.
    let mut choose = |choice: ConflictChoice| {
        st.set_choice(choice);
        st.step(1);
    };
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => st.step(1),
        KeyCode::Char('k') | KeyCode::Up => st.step(-1),
        KeyCode::Char('l') | KeyCode::Char('1') => choose(ConflictChoice::Local),
        KeyCode::Char('u') | KeyCode::Char('2') => choose(ConflictChoice::Remote),
        KeyCode::Char('b') => choose(ConflictChoice::Both),
        KeyCode::Char('n') => choose(ConflictChoice::Neither),
        KeyCode::Enter => app.resolve_conflict(),
        KeyCode::Esc => {
            app.conflict = None;
            // Back off the periodic poll so we don't immediately re-detect the
            // same conflict; the user can edit/save to retrigger sooner.
            app.last_sync = app
                .last_sync
                .take()
                .map(|(p, _)| (p, std::time::Instant::now()));
            app.status = "Conflict resolution cancelled".into();
        }
        _ => {}
    }
    Ok(())
}

fn handle_prompt_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.should_quit = true;
        return Ok(());
    }
    let confirm = matches!(
        app.prompt.as_ref().map(|p| &p.kind),
        Some(crate::tui::app::PromptKind::ConfirmDelete { .. })
    );
    if confirm {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                if let Some(p) = app.prompt.take() {
                    app.commit_prompt(p);
                }
            }
            _ => {
                app.prompt = None;
                app.status = "Delete cancelled".into();
            }
        }
        return Ok(());
    }
    match key.code {
        KeyCode::Esc => app.prompt = None,
        KeyCode::Enter => {
            if let Some(p) = app.prompt.take() {
                app.commit_prompt(p);
            }
        }
        KeyCode::Backspace => {
            if let Some(p) = &mut app.prompt {
                p.input.pop();
            }
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(p) = &mut app.prompt {
                p.input.clear();
            }
        }
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            if let Some(p) = &mut app.prompt {
                p.input.push(c);
            }
        }
        _ => {}
    }
    Ok(())
}

/// `y` in a cloud reader: put the note's publish link on the clipboard.
fn copy_publish_link(app: &mut App) {
    let link = match &app.view {
        View::Reader(r) => match &r.origin {
            crate::tui::app::ReaderOrigin::CloudNote { publish_link, .. } => publish_link.clone(),
            _ => return,
        },
        _ => return,
    };
    if link.is_empty() {
        app.status = "No publish link yet".into();
        return;
    }
    copy_to_clipboard(&link);
    app.status = format!("Copied: {link}");
}

/// The local file `U` would push: the open file reader's path, or the
/// selected markdown file in the local browser.
fn local_push_target(app: &App) -> Option<std::path::PathBuf> {
    match &app.view {
        View::Reader(r) => match &r.origin {
            crate::tui::app::ReaderOrigin::File(p) => Some(p.clone()),
            _ => None,
        },
        View::Browser(b) => b
            .entries
            .get(b.selected)
            .filter(|e| e.kind == BrowserEntryKind::Markdown)
            .map(|e| e.path.clone()),
        View::Cloud(_) => None,
    }
}

/// Toggle mouse capture so the user can drag-select text natively. When
/// capture is on we get scroll/click/hover; when off the terminal handles
/// dragging.
fn toggle_mouse(app: &mut App) {
    let mut out = stdout();
    if app.mouse_enabled {
        let _ = execute!(out, DisableMouseCapture);
        app.mouse_enabled = false;
        app.status = "Mouse off — drag to select text (m to re-enable)".into();
    } else {
        let _ = execute!(out, EnableMouseCapture);
        app.mouse_enabled = true;
        app.status = "Mouse on".into();
    }
}

/// Handle keystrokes while the in-document search prompt is open.
fn handle_doc_search_key(app: &mut App, key: KeyEvent) -> Result<()> {
    // Ctrl+C always quits, even with overlays open.
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.should_quit = true;
        return Ok(());
    }
    match key.code {
        KeyCode::Esc => app.close_doc_search(),
        KeyCode::Enter => app.doc_search_commit(),
        KeyCode::Backspace => {
            if let View::Reader(r) = &mut app.view {
                if let Some(s) = &mut r.doc_search {
                    s.query.pop();
                }
            }
            app.doc_search_refresh();
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let View::Reader(r) = &mut app.view {
                if let Some(s) = &mut r.doc_search {
                    s.query.clear();
                }
            }
            app.doc_search_refresh();
        }
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            if let View::Reader(r) = &mut app.view {
                if let Some(s) = &mut r.doc_search {
                    s.query.push(c);
                }
            }
            app.doc_search_refresh();
        }
        _ => {}
    }
    Ok(())
}

fn handle_search_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.should_quit = true;
        return Ok(());
    }
    match key.code {
        KeyCode::Esc => app.close_search(),
        KeyCode::Enter => {
            let result = app
                .search
                .as_ref()
                .and_then(|s| s.results.get(s.selected).cloned());
            app.close_search();
            if let Some(r) = result {
                open_search_result(app, r)?;
            }
        }
        KeyCode::Down | KeyCode::Tab => {
            if let Some(s) = &mut app.search {
                s.move_selection(1);
            }
        }
        KeyCode::Up | KeyCode::BackTab => {
            if let Some(s) = &mut app.search {
                s.move_selection(-1);
            }
        }
        KeyCode::PageDown => {
            if let Some(s) = &mut app.search {
                s.move_selection(10);
            }
        }
        KeyCode::PageUp => {
            if let Some(s) = &mut app.search {
                s.move_selection(-10);
            }
        }
        KeyCode::Backspace => {
            if let Some(s) = &mut app.search {
                s.query.pop();
                s.refresh();
            }
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(s) = &mut app.search {
                s.query.clear();
                s.refresh();
            }
        }
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            if let Some(s) = &mut app.search {
                s.query.push(c);
                s.refresh();
            }
        }
        _ => {}
    }
    Ok(())
}

fn open_search_result(app: &mut App, r: SearchResult) -> Result<()> {
    if r.is_dir {
        app.navigate_to(EntryKind::Directory(r.path), 0)?;
    } else if app::is_markdown_file(&r.path) {
        app.navigate_to(EntryKind::File(r.path), 0)?;
    } else {
        let _ = open::that_detached(&r.path);
        app.status = format!("Opened externally: {}", r.path.display());
    }
    Ok(())
}

fn handle_mouse(app: &mut App, m: MouseEvent) -> Result<()> {
    // A modal prompt owns the screen; ignore the mouse until it resolves.
    if app.prompt.is_some() {
        return Ok(());
    }
    // TOC overlay: the wheel moves the selection; everything else is inert.
    if app.toc.is_some() {
        match m.kind {
            MouseEventKind::ScrollUp => app.toc_move(-1),
            MouseEventKind::ScrollDown => app.toc_move(1),
            _ => {}
        }
        return Ok(());
    }
    // Anything other than scroll/move counts as deliberate mouse interaction
    // and re-engages the mouse-cursor mode (hides the keyboard focus halo).
    match m.kind {
        MouseEventKind::Moved
        | MouseEventKind::Drag(_)
        | MouseEventKind::Down(_)
        | MouseEventKind::Up(_)
        | MouseEventKind::ScrollUp
        | MouseEventKind::ScrollDown => {
            app.mouse_recent = true;
            app.last_mouse_col = m.column;
            app.last_mouse_row = m.row;
        }
        _ => {}
    }
    // Like key presses, deliberate gestures (click / wheel) dismiss any
    // transient status message; a hover alone keeps it visible.
    if matches!(
        m.kind,
        MouseEventKind::Down(_) | MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
    ) {
        app.status.clear();
    }

    // An armed statusline-path drag owns the gesture until mouse-up, even if
    // the pointer wanders off the row mid-drag.
    if app.statusline_path_drag.is_some() {
        match m.kind {
            MouseEventKind::Drag(MouseButton::Left) => {
                if let (Some((s, e, _, _)), Some(d)) = (
                    app.statusline_path_hit.as_ref(),
                    app.statusline_path_drag.as_mut(),
                ) {
                    d.1 = m.column.clamp(*s, e.saturating_sub(1));
                }
                return Ok(());
            }
            MouseEventKind::Up(MouseButton::Left) => {
                finish_path_drag(app);
                return Ok(());
            }
            _ => {}
        }
    }

    // Statusline: clickable back button. (Was in the dedicated header row;
    // since we collapsed the layout it now sits on the statusline.)
    if m.row == app.statusline_area.y && matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
        if let Some((sx, ex)) = app.back_button_hit {
            if m.column >= sx && m.column < ex {
                if !app.history.is_empty() {
                    app.go_back()?;
                }
                return Ok(());
            }
        }
        // Click the URL shown in the statusline middle → copy it to clipboard.
        if let Some((sx, ex, url)) = app.statusline_url_hit.clone() {
            if m.column >= sx && m.column < ex {
                copy_to_clipboard(&url);
                app.status = format!("Copied: {}", url);
                return Ok(());
            }
        }
        // Mouse-down on the path arms a drag; mouse-up decides whether it was
        // a click (copy the full path) or a drag (copy the selected slice).
        if let Some((sx, ex)) = app.statusline_path_hit.as_ref().map(|h| (h.0, h.1)) {
            if m.column >= sx && m.column < ex {
                app.statusline_path_drag = Some((m.column, m.column));
                return Ok(());
            }
        }
    }
    // While the search overlay is up, the mouse wheel scrolls results.
    if app.search.is_some() {
        match m.kind {
            MouseEventKind::ScrollUp => {
                if let Some(s) = &mut app.search {
                    s.move_selection(-1);
                }
            }
            MouseEventKind::ScrollDown => {
                if let Some(s) = &mut app.search {
                    s.move_selection(1);
                }
            }
            _ => {}
        }
        return Ok(());
    }
    let area = app.viewport;
    // Split-screen edit mode owns the body: route by pane.
    if in_split_edit(app) {
        return handle_split_mouse(app, m);
    }
    if !point_in(area, m.column, m.row) {
        match m.kind {
            MouseEventKind::ScrollUp => wheel_scroll(app, -3),
            MouseEventKind::ScrollDown => wheel_scroll(app, 3),
            _ => {}
        }
        return Ok(());
    }
    match m.kind {
        MouseEventKind::ScrollUp => wheel_scroll(app, -3),
        MouseEventKind::ScrollDown => wheel_scroll(app, 3),
        MouseEventKind::Moved => {
            update_hover(app, m.column, m.row);
        }
        MouseEventKind::Down(MouseButton::Left) => {
            // Double-click → select & copy the word under the cursor.
            // (Pre-existing behaviour, fires on the second Down so the user
            // gets immediate feedback without waiting for Up.)
            let now = std::time::Instant::now();
            let double = match app.last_click {
                Some((t, c, r)) => {
                    now.duration_since(t) <= std::time::Duration::from_millis(450)
                        && c == m.column
                        && r == m.row
                }
                None => false,
            };
            app.last_click = Some((now, m.column, m.row));
            if double {
                select_word_at(app, m.column, m.row);
                app.pending_click = None;
                app.selection = None;
                return Ok(());
            }
            // Single Down: defer the click action until Up so a Drag can
            // claim the gesture as a selection. Set the selection anchor
            // to the down position; selection only "activates" once a Drag
            // arrives with a different position.
            app.pending_click = Some((m.column, m.row));
            if let Some((line_idx, col)) = body_pos(app, m.column, m.row) {
                app.selection = Some(crate::tui::app::Selection {
                    anchor_line: line_idx,
                    anchor_col: col,
                    focus_line: line_idx,
                    focus_col: col,
                    dragged: false,
                });
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some((line_idx, col)) = body_pos(app, m.column, m.row) {
                if let Some(s) = app.selection.as_mut() {
                    s.focus_line = line_idx;
                    s.focus_col = col;
                    if !s.dragged && (s.anchor_line != s.focus_line || s.anchor_col != s.focus_col)
                    {
                        s.dragged = true;
                        // Drag claimed the gesture; cancel the pending click
                        // so Up doesn't follow a link the user was trying to
                        // copy text from.
                        app.pending_click = None;
                    }
                }
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            // If the selection turned into a real drag, copy and exit.
            let copied = if let Some(s) = app.selection.take() {
                if s.is_active() {
                    if let Some(text) = extract_selection_text(app, &s) {
                        if !text.is_empty() {
                            copy_to_clipboard(&text);
                            app.status = format!("Copied {} chars", text.chars().count());
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };
            if copied {
                app.pending_click = None;
                return Ok(());
            }
            // No drag → fire the deferred click target (link / checkbox /
            // browser entry). This preserves the click-to-follow behaviour
            // that existed before drag-select was introduced.
            if let Some((c, r)) = app.pending_click.take() {
                click_at(app, c, r)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// True when the active reader is in split-screen edit mode.
fn in_split_edit(app: &App) -> bool {
    matches!(&app.view, View::Reader(r) if r.in_split_edit())
}

/// Mouse routing for split-screen edit. Wheel in either pane scrolls that
/// pane and syncs the other via block anchors (the bidirectional sync the
/// user picked). Click in the raw pane sets the source cursor; click in
/// the preview pane jumps the cursor to the corresponding source byte.
fn handle_split_mouse(app: &mut App, m: MouseEvent) -> Result<()> {
    let raw_area = app.edit_raw_area;
    let prev_area = app.edit_preview_area;
    let in_raw = point_in(raw_area, m.column, m.row);
    let in_prev = point_in(prev_area, m.column, m.row);
    match m.kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let dir = if matches!(m.kind, MouseEventKind::ScrollUp) {
                -1
            } else {
                1
            };
            let now = std::time::Instant::now();
            let dampened = compute_dampened_scroll(
                &mut app.last_scroll_at,
                &mut app.scroll_accum,
                dir * 3,
                now,
            );
            if dampened == 0 {
                return Ok(());
            }
            if in_prev {
                split_scroll_preview(app, dampened);
            } else {
                // Default + raw: scroll the raw pane (cursor side).
                split_scroll_raw(app, dampened);
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if in_raw {
                split_click_raw(app, m.column, m.row);
                // Arm a selection at the click position; it only activates
                // if a drag moves the focus off this anchor.
                if let View::Reader(r) = &mut app.view {
                    if let Some(e) = r.edit.as_mut() {
                        e.selection = Some(crate::tui::app::EditSelection {
                            anchor: e.cursor,
                            focus: e.cursor,
                            dragged: false,
                        });
                    }
                }
            } else if in_prev {
                app.edit_clear_selection();
                split_click_preview(app, m.column, m.row);
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            split_drag_raw(app, m.column, m.row);
        }
        MouseEventKind::Up(MouseButton::Left) => {
            // A plain click leaves no selection behind. A real drag keeps
            // it highlighted; the statusline offers y copy / Del delete /
            // Esc cancel (handled in `handle_edit_key`).
            if let View::Reader(r) = &mut app.view {
                if let Some(e) = r.edit.as_mut() {
                    if !e.selection.as_ref().map(|s| s.is_active()).unwrap_or(false) {
                        e.selection = None;
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Extend an armed raw-pane selection to the pointer position. The pointer
/// is clamped into the raw pane so a drag that wanders past an edge keeps
/// extending to the nearest cell. No-op when no selection was armed (e.g.
/// the drag started in the preview pane).
fn split_drag_raw(app: &mut App, col: u16, row: u16) {
    let area = app.edit_raw_area;
    if area.width == 0 || area.height == 0 {
        return;
    }
    let col = col.clamp(area.x, area.x + area.width - 1);
    let row = row.clamp(area.y, area.y + area.height - 1);
    let raw_w = area.width.max(1) as usize;
    let View::Reader(r) = &mut app.view else {
        return;
    };
    if r.edit.as_ref().and_then(|e| e.selection.as_ref()).is_none() {
        return;
    }
    let local_row = (row - area.y) as usize + r.scroll as usize;
    let local_col = (col - area.x) as usize;
    let rows = crate::tui::app::render_raw_pane(&r.raw, raw_w);
    let byte = crate::tui::app::raw_click_to_source(&rows, &r.raw, local_row, local_col);
    let e = r.edit.as_mut().unwrap();
    let sel = e.selection.as_mut().unwrap();
    sel.focus = byte;
    if !sel.dragged && sel.focus != sel.anchor {
        sel.dragged = true;
    }
    // The cursor follows the drag focus so the auto-scroll keeps the
    // selection edge visible.
    e.cursor = byte;
    e.command = None;
    r.rendered = None;
}

/// Scroll the raw pane by `delta` rows, clamping to the wrapped row count
/// and re-syncing the preview to follow the new top-of-raw block.
fn split_scroll_raw(app: &mut App, delta: i32) {
    let raw_w = app.edit_raw_area.width.max(1) as usize;
    let raw_h = app.edit_raw_area.height as usize;
    let prev_h = app.edit_preview_area.height as usize;
    let View::Reader(r) = &mut app.view else {
        return;
    };
    let rows = crate::tui::app::render_raw_pane(&r.raw, raw_w);
    let max = rows.len().saturating_sub(raw_h.max(1)) as i32;
    let new = (r.scroll as i32 + delta).clamp(0, max) as u16;
    r.scroll = new;
    // Sync preview: anchor on the source byte at top of raw pane.
    if let Some(top_row) = rows.get(new as usize) {
        let src = top_row.source_range.start;
        if let Some(rendered) = r.rendered.as_ref() {
            let prev_row = crate::tui::app::preview_row_for_source(rendered, src);
            let max_prev = rendered.lines.len().saturating_sub(prev_h.max(1)) as u16;
            r.preview_scroll = (prev_row as u16).min(max_prev);
        }
    }
}

/// Scroll the preview pane by `delta` rows; sync raw pane to follow.
fn split_scroll_preview(app: &mut App, delta: i32) {
    let raw_w = app.edit_raw_area.width.max(1) as usize;
    let raw_h = app.edit_raw_area.height as usize;
    let prev_h = app.edit_preview_area.height as usize;
    let View::Reader(r) = &mut app.view else {
        return;
    };
    let Some(rendered) = r.rendered.as_ref() else {
        return;
    };
    let max_prev = rendered.lines.len().saturating_sub(prev_h.max(1)) as i32;
    let new = (r.preview_scroll as i32 + delta).clamp(0, max_prev) as u16;
    r.preview_scroll = new;
    // Sync raw pane: source byte at top-of-preview → raw row.
    let src = crate::tui::app::source_for_preview_row(rendered, new as usize);
    let rows = crate::tui::app::render_raw_pane(&r.raw, raw_w);
    let raw_row = crate::tui::app::raw_row_for_cursor(&rows, src);
    let max_raw = rows.len().saturating_sub(raw_h.max(1)) as u16;
    r.scroll = (raw_row as u16).min(max_raw);
}

/// Set the source cursor from a click in the raw pane.
fn split_click_raw(app: &mut App, col: u16, row: u16) {
    let raw_w = app.edit_raw_area.width.max(1) as usize;
    let area = app.edit_raw_area;
    if !point_in(area, col, row) {
        return;
    }
    let local_row = (row - area.y) as usize
        + match &app.view {
            View::Reader(r) => r.scroll as usize,
            _ => 0,
        };
    let local_col = (col - area.x) as usize;
    let View::Reader(r) = &mut app.view else {
        return;
    };
    let rows = crate::tui::app::render_raw_pane(&r.raw, raw_w);
    let new_cursor = crate::tui::app::raw_click_to_source(&rows, &r.raw, local_row, local_col);
    if let Some(e) = r.edit.as_mut() {
        e.cursor = new_cursor;
        e.command = None;
        r.rendered = None;
    }
}

/// Click in the preview pane: jump source cursor to that block's start.
/// Useful for navigating to a section by clicking the rendered headline.
fn split_click_preview(app: &mut App, col: u16, row: u16) {
    let area = app.edit_preview_area;
    if !point_in(area, col, row) {
        return;
    }
    let View::Reader(r) = &mut app.view else {
        return;
    };
    let Some(rendered) = r.rendered.as_ref() else {
        return;
    };
    let local_row = (row - area.y) as usize + r.preview_scroll as usize;
    let _ = col;
    let new_cursor = crate::tui::app::source_for_preview_row(rendered, local_row);
    if let Some(e) = r.edit.as_mut() {
        e.cursor = new_cursor;
        e.command = None;
        r.rendered = None;
    }
}

/// Convert a screen (col, row) to a body-local (line_index, display_col).
/// Returns `None` if the point is outside the body or before the line-number
/// gutter. line_index is the index into `Rendered::lines` so it survives
/// scrolling mid-drag.
fn body_pos(app: &App, col: u16, row: u16) -> Option<(usize, u16)> {
    let body = app.viewport;
    if row < body.y || row >= body.y + body.height {
        return None;
    }
    if col < body.x {
        return None;
    }
    let line_num_w = if app.opts.line_numbers {
        match &app.view {
            View::Reader(r) => match &r.rendered {
                Some(rd) => (format!("{}", rd.lines.len()).len() + 1) as u16,
                None => 0,
            },
            _ => 0,
        }
    } else {
        0
    };
    if col < body.x + line_num_w {
        return None;
    }
    let scroll = match &app.view {
        View::Reader(r) => r.scroll as usize,
        View::Browser(b) => b.scroll as usize,
        View::Cloud(c) => c.tab().map(|t| t.scroll).unwrap_or(0) as usize,
    };
    let local_row = (row - body.y) as usize;
    let local_col = col - body.x - line_num_w;
    Some((scroll + local_row, local_col))
}

/// Extract the text covered by a selection, walking `Rendered::lines` and
/// slicing each line by display columns. Inserts `\n` between lines. Returns
/// `None` if the reader hasn't been rendered yet.
fn extract_selection_text(app: &App, sel: &crate::tui::app::Selection) -> Option<String> {
    use unicode_width::UnicodeWidthChar;
    let View::Reader(r) = &app.view else {
        return None;
    };
    let rd = r.rendered.as_ref()?;
    let ((s_line, s_col), (e_line, e_col)) = sel.normalized();
    let last = rd.lines.len().saturating_sub(1);
    let mut out = String::new();
    for li in s_line..=e_line.min(last) {
        let line = &rd.lines[li];
        let from = if li == s_line { s_col as usize } else { 0 };
        let to = if li == e_line {
            e_col as usize
        } else {
            usize::MAX
        };
        let mut col = 0usize;
        let mut wrote_any = false;
        for span in &line.spans {
            for ch in span.content.chars() {
                let w = ch.width().unwrap_or(0);
                let next = col + w;
                if col >= from && next <= to {
                    out.push(ch);
                    wrote_any = true;
                }
                col = next;
                if col >= to {
                    break;
                }
            }
            if col >= to {
                break;
            }
        }
        let _ = wrote_any;
        if li < e_line.min(last) {
            out.push('\n');
        }
    }
    Some(out)
}

/// Select the word under the click and push it to the system clipboard.
/// Uses `pbcopy` on macOS (always works locally) and OSC 52 elsewhere /
/// as a fallback so it still works over SSH. Updates the status bar.
fn select_word_at(app: &mut App, col: u16, row: u16) {
    let area = app.viewport;
    if !point_in(area, col, row) {
        return;
    }
    let View::Reader(r) = &app.view else {
        return;
    };
    let Some(rendered) = &r.rendered else {
        return;
    };

    let line_num_w = if app.opts.line_numbers {
        (format!("{}", rendered.lines.len()).len() + 1) as u16
    } else {
        0
    };
    let inner_x = area.x + line_num_w;
    if col < inner_x {
        return;
    }
    let local_col = (col - inner_x) as usize;
    let line_idx = r.scroll as usize + (row - area.y) as usize;
    let Some(line) = rendered.lines.get(line_idx) else {
        return;
    };

    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let Some(word) = word_at_col(&text, local_col) else {
        return;
    };

    copy_to_clipboard(&word);
    app.status = format!("Copied: {}", word);
}

/// Best-effort copy: native helper on macOS, OSC 52 otherwise (with tmux
/// passthrough wrapping when applicable). Both paths are silent on failure —
/// the status line already reports what we attempted to copy.
/// Resolve a finished statusline-path gesture: a plain click copies the full
/// (untruncated) path, a drag copies the column-selected slice of the
/// displayed text.
fn finish_path_drag(app: &mut App) {
    let Some((a, b)) = app.statusline_path_drag.take() else {
        return;
    };
    let Some((start, _, display, full)) = app.statusline_path_hit.clone() else {
        return;
    };
    let text = if a == b {
        full
    } else {
        let (lo, hi) = (a.min(b), a.max(b));
        col_slice(&display, (lo - start) as usize, (hi - start) as usize + 1)
    };
    if text.is_empty() {
        return;
    }
    copy_to_clipboard(&text);
    app.status = format!("Copied: {}", text);
}

/// The substring of `s` covering display columns `[from, to)`. A character is
/// included when any part of its width falls inside the range.
fn col_slice(s: &str, from: usize, to: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    let mut out = String::new();
    let mut col = 0usize;
    for ch in s.chars() {
        let w = ch.width().unwrap_or(0);
        if col + w > from && col < to {
            out.push(ch);
        }
        col += w;
        if col >= to {
            break;
        }
    }
    out
}

fn copy_to_clipboard(text: &str) {
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        use std::process::{Command, Stdio};
        if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
            return;
        }
    }
    osc52_copy(text);
}

/// Read the system clipboard, if we can. Only macOS (`pbpaste`) is wired up —
/// there's no portable, terminal-safe clipboard *read* (OSC 52 read is widely
/// disabled for security). Returns `None` everywhere else, so callers fall
/// back to the in-app yank register.
fn read_clipboard() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        if let Ok(out) = Command::new("pbpaste").output() {
            if out.status.success() {
                return String::from_utf8(out.stdout).ok();
            }
        }
    }
    None
}

/// Resolve the text to paste: the system clipboard if readable, else the
/// in-app yank register set by the last copy/cut.
fn clipboard_or_yank(app: &App) -> Option<String> {
    read_clipboard().or_else(|| app.yank.clone())
}

/// Paste the clipboard/register over the active editor selection, or at the
/// cursor when nothing is selected. Clears the selection either way and
/// reports the result on the statusline.
fn paste_over_selection(app: &mut App) {
    if let Some(text) = clipboard_or_yank(app) {
        app.edit_replace_selection(&text);
        app.status = format!("Pasted {} chars", text.chars().count());
    } else {
        app.edit_clear_selection();
        app.status = "Clipboard empty".into();
    }
}

fn osc52_copy(text: &str) {
    use base64::Engine;
    use std::io::Write;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let mut out = stdout();
    if std::env::var_os("TMUX").is_some() {
        // tmux DCS passthrough: tmux strips the wrapper and forwards the inner
        // OSC 52 to the outer terminal.
        let _ = write!(out, "\x1bPtmux;\x1b\x1b]52;c;{}\x07\x1b\\", encoded);
    } else {
        let _ = write!(out, "\x1b]52;c;{}\x1b\\", encoded);
    }
    let _ = out.flush();
}

#[cfg(test)]
mod substitute_tests {
    use super::{looks_like_substitute, substitute_region};

    #[test]
    fn recognises_substitute_commands() {
        assert!(looks_like_substitute("s/a/b/"));
        assert!(looks_like_substitute("%s/a/b/g"));
        assert!(looks_like_substitute("s#a#b#"));
        // Not substitutes.
        assert!(!looks_like_substitute("s")); // bare `s`
        assert!(!looks_like_substitute("set")); // word
        assert!(!looks_like_substitute("wq"));
        assert!(!looks_like_substitute("42"));
    }

    #[test]
    fn global_replaces_all_first_only_replaces_one_per_line() {
        let region = "foo foo\nfoo bar\n";
        let (g, gn) = substitute_region(region, "foo", "X", true);
        assert_eq!(g, "X X\nX bar\n");
        assert_eq!(gn, 3);
        let (f, fn_) = substitute_region(region, "foo", "X", false);
        assert_eq!(f, "X foo\nX bar\n");
        assert_eq!(fn_, 2);
    }

    #[test]
    fn no_match_reports_zero() {
        let (out, n) = substitute_region("hello\n", "zzz", "x", true);
        assert_eq!(out, "hello\n");
        assert_eq!(n, 0);
    }
}

#[cfg(test)]
mod scroll_damp_tests {
    use super::compute_dampened_scroll;
    use std::time::{Duration, Instant};

    #[test]
    fn first_event_emits_one_line() {
        // An isolated wheel tick (no recent previous event) should emit
        // exactly one line, even though the raw delta from crossterm is 3.
        let mut last = None;
        let mut accum = 0.0;
        let d = compute_dampened_scroll(&mut last, &mut accum, 3, Instant::now());
        assert_eq!(d, 1);
    }

    #[test]
    fn slow_scroll_emits_one_line_per_event() {
        let mut last = None;
        let mut accum = 0.0;
        let t0 = Instant::now();
        assert_eq!(compute_dampened_scroll(&mut last, &mut accum, 3, t0), 1);
        // 250ms later — past the burst threshold; deliberate ticks remain
        // one-line-at-a-time so reading flow isn't jumpy.
        let t1 = t0 + Duration::from_millis(250);
        assert_eq!(compute_dampened_scroll(&mut last, &mut accum, 3, t1), 1);
        // Negative direction also emits one line.
        let t2 = t1 + Duration::from_millis(250);
        assert_eq!(compute_dampened_scroll(&mut last, &mut accum, -3, t2), -1);
    }

    #[test]
    fn rapid_burst_is_halved_overall() {
        let mut last = None;
        let mut accum = 0.0;
        let t0 = Instant::now();
        // First event sets the clock; treat it as the start of the burst.
        let _ = compute_dampened_scroll(&mut last, &mut accum, 3, t0);
        // Six follow-up events 50ms apart — all in the burst zone (factor 0.5).
        let mut total = 0;
        for i in 1..=6 {
            let t = t0 + Duration::from_millis(50 * i);
            total += compute_dampened_scroll(&mut last, &mut accum, 3, t);
        }
        // Raw would have been 6 * 3 = 18 lines; dampened ≈ 9.
        assert_eq!(
            total, 9,
            "burst of 6 events at 3 lines should yield ~9 dampened"
        );
    }

    #[test]
    fn fractional_accumulator_is_preserved() {
        let mut last = None;
        let mut accum = 0.0;
        let t0 = Instant::now();
        let _ = compute_dampened_scroll(&mut last, &mut accum, 3, t0);
        // 50ms later: 3 * 0.5 = 1.5 lines → 1 line emitted, 0.5 carried.
        let t1 = t0 + Duration::from_millis(50);
        assert_eq!(compute_dampened_scroll(&mut last, &mut accum, 3, t1), 1);
        // 50ms later again: 1.5 + 0.5 carry = 2.0 lines → 2 emitted, 0 carried.
        let t2 = t1 + Duration::from_millis(50);
        assert_eq!(compute_dampened_scroll(&mut last, &mut accum, 3, t2), 2);
    }

    #[test]
    fn long_pause_resets_accumulator() {
        let mut accum = 0.7;
        let now = Instant::now();
        // Pretend the last event was way back; elapsed > 500ms.
        let mut last = Some(now - Duration::from_secs(2));
        let _ = compute_dampened_scroll(&mut last, &mut accum, 3, now);
        // Accum should have been zeroed before this event's contribution.
        // After a 2-second gap, factor=1.0, so 3 lines emitted, accum back to 0.
        assert!(
            accum.abs() < 1e-4,
            "accum should reset on long pause, got {}",
            accum
        );
    }

    #[test]
    fn direction_reversal_resets_carry() {
        let mut last = None;
        let mut accum = 0.0;
        let t0 = Instant::now();
        let _ = compute_dampened_scroll(&mut last, &mut accum, 3, t0);
        // Build positive carry.
        let t1 = t0 + Duration::from_millis(50);
        let _ = compute_dampened_scroll(&mut last, &mut accum, 3, t1);
        assert!(accum > 0.0);
        // Reverse direction — leftover positive carry must not cancel the
        // first up-scroll line.
        let t2 = t1 + Duration::from_millis(50);
        let d = compute_dampened_scroll(&mut last, &mut accum, -3, t2);
        assert!(
            d <= -1,
            "reversal should still emit a line in the new direction, got {}",
            d
        );
    }
}

#[cfg(test)]
mod word_tests {
    use super::word_at_col;

    #[test]
    fn picks_word_in_simple_line() {
        let line = "the quick brown fox";
        // 'q' lives at columns 4..5
        assert_eq!(word_at_col(line, 4).as_deref(), Some("quick"));
        assert_eq!(word_at_col(line, 6).as_deref(), Some("quick"));
    }

    #[test]
    fn returns_none_on_whitespace() {
        let line = "alpha   beta";
        assert_eq!(word_at_col(line, 6), None);
    }

    #[test]
    fn strips_trailing_punctuation() {
        let line = "hello, world!";
        assert_eq!(word_at_col(line, 0).as_deref(), Some("hello"));
        assert_eq!(word_at_col(line, 8).as_deref(), Some("world"));
    }

    #[test]
    fn keeps_internal_dashes_underscores_and_paths() {
        let line = "src/foo_bar-baz.rs";
        assert_eq!(word_at_col(line, 0).as_deref(), Some("src/foo_bar-baz.rs"));
    }
}

/// Map a display (line_idx, col) to a source byte offset. Uses
/// `row_source` (populated for raw-substituted edit-mode rows) to get an
/// exact answer; falls back to "start of containing block" for formatted
/// rows so the next render swaps that block to raw and the cursor lands
/// in a sensible place.
fn xy_to_source_offset(
    rendered: &crate::tui::markdown::Rendered,
    source: &str,
    line_idx: usize,
    col: usize,
) -> Option<usize> {
    use unicode_width::UnicodeWidthChar;
    if let Some(Some(range)) = rendered.row_source.get(line_idx) {
        let slice = source.get(range.clone())?;
        let mut taken = 0usize;
        for (i, ch) in slice.char_indices() {
            let w = ch.width().unwrap_or(0);
            if taken + w > col {
                return Some(range.start + i);
            }
            taken += w;
        }
        return Some(range.end);
    }
    // Formatted row: bytes don't map 1:1 with display columns, so jump
    // to the start of the block. The next render makes this block raw,
    // and a follow-up click can land precisely.
    let block = rendered
        .blocks
        .iter()
        .find(|b| line_idx >= b.display_start && line_idx < b.display_end)?;
    Some(block.source_range.start)
}

/// Walk left and right from `target_col` in `line` (using display widths) to
/// find the run of non-whitespace characters covering that column.
fn word_at_col(line: &str, target_col: usize) -> Option<String> {
    use unicode_width::UnicodeWidthChar;
    let mut col = 0usize;
    let mut hit_byte: Option<usize> = None;
    for (i, ch) in line.char_indices() {
        let w = ch.width().unwrap_or(0);
        if target_col >= col && target_col < col + w.max(1) {
            hit_byte = Some(i);
            break;
        }
        col += w;
    }
    let hit = hit_byte?;
    let bytes = line.as_bytes();
    if bytes
        .get(hit)
        .map(|b| (*b as char).is_whitespace())
        .unwrap_or(true)
    {
        return None;
    }
    let mut start = hit;
    while start > 0 {
        let prev = line[..start].chars().next_back()?;
        if prev.is_whitespace() {
            break;
        }
        start -= prev.len_utf8();
    }
    let mut end = hit;
    let mut iter = line[hit..].char_indices();
    iter.next(); // skip the hit char itself
    let len = line.len();
    let mut cursor = hit;
    for (_, ch) in line[hit..].char_indices() {
        if ch.is_whitespace() {
            break;
        }
        cursor += ch.len_utf8();
        end = cursor;
    }
    let _ = iter;
    let _ = len;
    if end <= start {
        return None;
    }
    let word = line[start..end].trim_matches(|c: char| {
        // Strip leading/trailing punctuation but keep internal characters.
        c.is_ascii_punctuation() && !matches!(c, '_' | '-' | '/' | '.' | '#')
    });
    if word.is_empty() {
        None
    } else {
        Some(word.to_string())
    }
}

fn point_in(rect: ratatui::layout::Rect, col: u16, row: u16) -> bool {
    col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
}

#[cfg(test)]
mod keybind_tests {
    use super::handle_key;
    use crate::tui::app::{App, Options, Source, View};
    use crate::tui::theme::Theme;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// A document with three headings separated by enough filler that every
    /// heading lands on a distinct scroll position in a 10-row viewport.
    fn app_with_headings(tag: &str) -> App {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "md-tui-events-test-{}-{}.md",
            tag,
            std::process::id()
        ));
        let mut src = String::from("# One\n\n");
        for i in 0..30 {
            src.push_str(&format!("filler {i}\n\n"));
        }
        src.push_str("## Two\n\n");
        for i in 0..30 {
            src.push_str(&format!("more {i}\n\n"));
        }
        src.push_str("## Three\n\n");
        for i in 0..30 {
            src.push_str(&format!("tail {i}\n\n"));
        }
        std::fs::write(&p, src).unwrap();
        let opts = Options {
            width: 80,
            line_numbers: false,
            theme: Theme::dark(),
        };
        let mut app = App::new(Source::File(p), opts).unwrap();
        app.ensure_rendered(80);
        app.viewport = ratatui::layout::Rect::new(0, 0, 80, 10);
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        handle_key(app, KeyEvent::new(code, KeyModifiers::NONE)).unwrap();
    }

    fn reader_scroll(app: &App) -> u16 {
        match &app.view {
            View::Reader(r) => r.scroll,
            _ => panic!("expected reader"),
        }
    }

    fn heading_lines(app: &App) -> Vec<usize> {
        match &app.view {
            View::Reader(r) => r
                .rendered
                .as_ref()
                .unwrap()
                .headings
                .iter()
                .map(|h| h.line)
                .collect(),
            _ => panic!("expected reader"),
        }
    }

    #[test]
    fn bracket_chords_jump_between_headings() {
        let mut app = app_with_headings("brackets");
        let lines = heading_lines(&app);
        assert_eq!(lines.len(), 3);
        assert!(lines[0] > 0, "first heading sits below the very top");
        // From the top, each `]]` lands on the next heading down.
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char(']'));
        assert_eq!(reader_scroll(&app), lines[0] as u16);
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char(']'));
        assert_eq!(reader_scroll(&app), lines[1] as u16);
        // `[[` goes back to the previous heading.
        press(&mut app, KeyCode::Char('['));
        press(&mut app, KeyCode::Char('['));
        assert_eq!(reader_scroll(&app), lines[0] as u16);
    }

    #[test]
    fn bracket_chord_honors_count_prefix() {
        let mut app = app_with_headings("count");
        let lines = heading_lines(&app);
        // `2]]` skips to the second heading strictly below the top.
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char(']'));
        let after: Vec<usize> = lines.iter().copied().filter(|&l| l > 0).collect();
        assert_eq!(reader_scroll(&app), after[1] as u16);
    }

    #[test]
    fn toc_opens_moves_and_jumps() {
        let mut app = app_with_headings("toc");
        let lines = heading_lines(&app);
        press(&mut app, KeyCode::Char('t'));
        let toc = app.toc.expect("t should open the TOC");
        assert_eq!(toc.selected, 0, "viewport at top selects the first heading");
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.toc.unwrap().selected, 2);
        press(&mut app, KeyCode::Enter);
        assert!(app.toc.is_none(), "Enter closes the TOC");
        assert_eq!(reader_scroll(&app), lines[2] as u16);
    }

    #[test]
    fn toc_dismisses_on_esc_without_jumping() {
        let mut app = app_with_headings("toc-esc");
        press(&mut app, KeyCode::Char('t'));
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Esc);
        assert!(app.toc.is_none());
        assert_eq!(reader_scroll(&app), 0, "Esc must not move the viewport");
    }

    #[test]
    fn plain_f_and_b_page_instead_of_history() {
        let mut app = app_with_headings("page");
        press(&mut app, KeyCode::Char('f'));
        // Full page = viewport height - 1 = 9 rows.
        assert_eq!(reader_scroll(&app), 9);
        press(&mut app, KeyCode::Char('b'));
        assert_eq!(reader_scroll(&app), 0);
        assert!(
            app.history.is_empty() && app.forward.is_empty(),
            "f/b must not touch history"
        );
    }

    #[test]
    fn ctrl_o_walks_history_back() {
        let mut app = app_with_headings("ctrl-o");
        press(&mut app, KeyCode::Char('f'));
        let key = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        handle_key(&mut app, key).unwrap();
        // With no history the call is a no-op — but it must not be treated
        // as plain `o` (open focused link) or crash.
        assert!(!app.should_quit);
    }

    fn edit_state_of(app: &App) -> Option<&crate::tui::app::EditState> {
        match &app.view {
            View::Reader(r) => r.edit.as_ref(),
            _ => None,
        }
    }

    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    #[test]
    fn i_enters_edit_mode_like_e() {
        let mut app = app_with_headings("i-edit");
        press(&mut app, KeyCode::Char('i'));
        assert!(edit_state_of(&app).is_some(), "i opens the editor");
    }

    #[test]
    fn esc_opens_command_line_and_esc_again_exits() {
        let mut app = app_with_headings("esc-cmd");
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Esc);
        let e = edit_state_of(&app).expect("still editing");
        assert_eq!(
            e.command.as_deref(),
            Some(""),
            "first Esc opens the command line"
        );
        press(&mut app, KeyCode::Esc);
        assert!(
            edit_state_of(&app).is_none(),
            "second Esc leaves the editor"
        );
    }

    #[test]
    fn wq_saves_and_exits() {
        let mut app = app_with_headings("wq");
        let path = match &app.view {
            View::Reader(r) => match &r.origin {
                crate::tui::app::ReaderOrigin::File(p) => p.clone(),
                _ => panic!("expected file origin"),
            },
            _ => panic!("expected reader"),
        };
        press(&mut app, KeyCode::Char('e'));
        type_str(&mut app, "XYZ");
        press(&mut app, KeyCode::Esc);
        type_str(&mut app, ":wq");
        press(&mut app, KeyCode::Enter);
        assert!(edit_state_of(&app).is_none(), ":wq exits the editor");
        let disk = std::fs::read_to_string(&path).unwrap();
        assert!(disk.starts_with("XYZ"), ":wq wrote the buffer to disk");
    }

    #[test]
    fn q_discards_changes_and_exits() {
        let mut app = app_with_headings("q-discard");
        press(&mut app, KeyCode::Char('e'));
        type_str(&mut app, "XYZ");
        press(&mut app, KeyCode::Esc);
        type_str(&mut app, "q");
        press(&mut app, KeyCode::Enter);
        assert!(edit_state_of(&app).is_none(), ":q exits the editor");
        let View::Reader(r) = &app.view else {
            panic!("expected reader");
        };
        assert!(!r.raw.starts_with("XYZ"), ":q dropped the unsaved edit");
    }

    #[test]
    fn preview_completes_with_tab_and_esc_returns() {
        let mut app = app_with_headings("preview-cmd");
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Esc);
        type_str(&mut app, "pre");
        press(&mut app, KeyCode::Tab);
        assert_eq!(
            edit_state_of(&app).unwrap().command.as_deref(),
            Some("preview"),
            "Tab accepts the grey completion"
        );
        press(&mut app, KeyCode::Enter);
        let e = edit_state_of(&app).expect("still editing under the overlay");
        assert!(e.preview_full, ":preview opens the overlay");
        assert!(e.command.is_none());
        press(&mut app, KeyCode::Esc);
        let e = edit_state_of(&app).expect("Esc returns to the editor");
        assert!(!e.preview_full);
    }

    #[test]
    fn unknown_command_reports_and_stays_editing() {
        let mut app = app_with_headings("bad-cmd");
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Esc);
        type_str(&mut app, "nope");
        press(&mut app, KeyCode::Enter);
        assert!(edit_state_of(&app).is_some(), "still editing");
        assert_eq!(app.status, "Not an editor command: :nope");
    }

    #[test]
    fn colon_number_jumps_cursor_to_line() {
        let mut app = app_with_headings("goto-line");
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Esc);
        type_str(&mut app, "5");
        press(&mut app, KeyCode::Enter);
        let View::Reader(r) = &app.view else {
            panic!("expected reader");
        };
        let expected: usize = r.raw.split_inclusive('\n').take(4).map(|l| l.len()).sum();
        let e = r.edit.as_ref().expect(":5 stays in the editor");
        assert_eq!(e.cursor, expected, "cursor lands at the start of line 5");
        assert!(e.command.is_none());
    }

    #[test]
    fn e_bang_reloads_from_disk_and_stays_editing() {
        let mut app = app_with_headings("e-bang");
        press(&mut app, KeyCode::Char('e'));
        type_str(&mut app, "XYZ");
        press(&mut app, KeyCode::Esc);
        type_str(&mut app, "e!");
        press(&mut app, KeyCode::Enter);
        let View::Reader(r) = &app.view else {
            panic!("expected reader");
        };
        let e = r.edit.as_ref().expect(":e! stays in the editor");
        assert!(!e.dirty, "reloaded buffer is clean");
        assert!(!r.raw.starts_with("XYZ"), "unsaved edit was dropped");
        assert_eq!(app.status, "Buffer reloaded");
    }

    #[test]
    fn complete_edit_command_prefixes() {
        use super::complete_edit_command;
        assert_eq!(complete_edit_command("pre"), Some("preview"));
        assert_eq!(complete_edit_command("w"), Some("wq"));
        assert_eq!(complete_edit_command(""), None);
        assert_eq!(complete_edit_command("preview"), None);
        assert_eq!(complete_edit_command("zz"), None);
    }

    #[test]
    fn col_slice_maps_display_columns_to_chars() {
        use super::col_slice;
        assert_eq!(col_slice("notes/file.md", 0, 5), "notes");
        assert_eq!(col_slice("notes/file.md", 6, 13), "file.md");
        // Wide chars: each kanji occupies two columns; a range touching any
        // part of the glyph includes it.
        assert_eq!(col_slice("漢字.md", 0, 2), "漢");
        assert_eq!(col_slice("漢字.md", 1, 3), "漢字");
        assert_eq!(col_slice("abc", 2, 10), "c");
    }

    #[test]
    fn any_key_clears_transient_status() {
        let mut app = app_with_headings("status-reset");
        app.status = "Saved /tmp/x.md".into();
        press(&mut app, KeyCode::Char('j'));
        assert!(
            app.status.is_empty(),
            "a scroll key must dismiss the sticky status message"
        );
    }

    /// A doc with two markdown links for focus-cycling tests.
    fn app_with_links(tag: &str) -> App {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "md-tui-events-test-{}-{}.md",
            tag,
            std::process::id()
        ));
        let mut src = String::from("[one](https://one.example)\n\n");
        for i in 0..30 {
            src.push_str(&format!("filler {i}\n\n"));
        }
        src.push_str("[two](https://two.example)\n");
        std::fs::write(&p, src).unwrap();
        let opts = Options {
            width: 80,
            line_numbers: false,
            theme: Theme::dark(),
        };
        let mut app = App::new(Source::File(p), opts).unwrap();
        app.ensure_rendered(80);
        app.viewport = ratatui::layout::Rect::new(0, 0, 80, 10);
        app
    }

    fn reader_focus(app: &App) -> Option<crate::tui::app::Focus> {
        match &app.view {
            View::Reader(r) => r.focus,
            _ => panic!("expected reader"),
        }
    }

    #[test]
    fn scrolling_drops_tab_focus() {
        let mut app = app_with_links("focus-scroll");
        press(&mut app, KeyCode::Tab);
        assert!(reader_focus(&app).is_some(), "Tab focuses the first link");
        press(&mut app, KeyCode::Down);
        assert!(
            reader_focus(&app).is_none(),
            "scrolling must end the Tab-cycling session"
        );
        // Tab again still works and `gg` (scroll_to) also clears it.
        press(&mut app, KeyCode::Tab);
        assert!(reader_focus(&app).is_some());
        press(&mut app, KeyCode::Char('g'));
        press(&mut app, KeyCode::Char('g'));
        assert!(reader_focus(&app).is_none());
    }
}

/// Mouse-wheel scroll with rate-aware dampening: deliberate single ticks
/// (>200ms apart) pass through at full strength; rapid bursts get scaled
/// down to 0.5× so trackpad momentum scrolls don't fly off the page. The
/// fractional accumulator on `App` carries sub-line credit across events
/// so the user sees smooth movement instead of stutter.
fn wheel_scroll(app: &mut App, raw_delta: i32) {
    let now = std::time::Instant::now();
    let dampened = compute_dampened_scroll(
        &mut app.last_scroll_at,
        &mut app.scroll_accum,
        raw_delta,
        now,
    );
    if dampened != 0 {
        scroll_viewport(app, dampened);
    }
}

/// Mouse-wheel scrolling moves the viewport like a page — in list views it
/// shifts the visible window without touching the selection (keyboard j/k
/// still steps the cursor). Mirrors how the reader scrolls.
fn scroll_viewport(app: &mut App, delta: i32) {
    match &mut app.view {
        View::Reader(_) => scroll_by(app, delta),
        View::Browser(b) => {
            let h = app.viewport.height.saturating_sub(2) as i32;
            let max = (b.entries.len() as i32 - h.max(1)).max(0);
            b.scroll = (b.scroll as i32 + delta).clamp(0, max) as u16;
        }
        View::Cloud(c) => {
            let chrome = 2 + c.show_tab_bar() as u16;
            let h = app.viewport.height.saturating_sub(chrome) as i32;
            if let Some(t) = c.tab_mut() {
                let max = (t.notes.len() as i32 - h.max(1)).max(0);
                t.scroll = (t.scroll as i32 + delta).clamp(0, max) as u16;
            }
        }
    }
}

/// Pure dampening logic, factored out for tests. The `now` parameter lets
/// callers feed deterministic timestamps. Updates `last_at` and `accum`
/// in place; returns the integer-line delta to apply to scroll position.
///
/// Behaviour:
/// - Deliberate / isolated wheel tick (>=200ms after the previous one) emits
///   exactly one line in the requested direction, regardless of the raw
///   wheel magnitude. Most terminals report a notch as a delta of 3, which
///   feels jumpy when the user is reading line-by-line.
/// - Rapid bursts (trackpad fling, momentum scroll) take a 0.5× factor and
///   accumulate fractional credit so the page still moves at a reasonable
///   speed, but doesn't fly off.
fn compute_dampened_scroll(
    last_at: &mut Option<std::time::Instant>,
    accum: &mut f32,
    requested: i32,
    now: std::time::Instant,
) -> i32 {
    if requested == 0 {
        return 0;
    }

    let elapsed_ms = last_at
        .map(|t| now.duration_since(t).as_millis() as u32)
        .unwrap_or(u32::MAX);

    // Long pause → drop fractional credit so a scroll started minutes ago
    // doesn't suddenly move an extra line on the next event.
    if elapsed_ms > 500 {
        *accum = 0.0;
    }
    // Reverse direction → reset, otherwise leftover credit in one direction
    // would silently absorb the first line of the opposite scroll.
    if (*accum > 0.0 && requested < 0) || (*accum < 0.0 && requested > 0) {
        *accum = 0.0;
    }

    *last_at = Some(now);

    // Slow / deliberate path: one line per event. Skip the accumulator so
    // a recent burst doesn't bleed extra lines into the next deliberate tick.
    if elapsed_ms >= 200 {
        *accum = 0.0;
        return if requested > 0 { 1 } else { -1 };
    }

    // Burst path: half-strength with fractional carry.
    let factor = 0.5_f32;
    *accum += (requested as f32) * factor;
    let lines = accum.trunc() as i32;
    *accum -= lines as f32;
    lines
}

fn scroll_by(app: &mut App, delta: i32) {
    match &mut app.view {
        View::Reader(r) => {
            let total = r.rendered.as_ref().map(|x| x.lines.len()).unwrap_or(0) as i32;
            let h = app.viewport.height as i32;
            let max = (total - h).max(0);
            let new = (r.scroll as i32 + delta).clamp(0, max) as u16;
            r.scroll = new;
            // Scrolling away ends a Tab-cycling session: drop the focused
            // link so the statusline returns to the contextual hints.
            r.focus = None;
        }
        View::Browser(b) => {
            let n = b.entries.len() as i32;
            if n == 0 {
                return;
            }
            // Wrap-around: pressing `k` at the top jumps to the last entry,
            // pressing `j` at the bottom jumps back to the first. rem_euclid
            // handles negative dividends correctly so a `-1` delta from
            // index 0 lands on `n-1`.
            let new = (b.selected as i32 + delta).rem_euclid(n) as usize;
            b.selected = new;
            // Keep selection visible. Account for the bordered title row.
            let h = app.viewport.height.saturating_sub(2) as usize;
            if b.selected < b.scroll as usize {
                b.scroll = b.selected as u16;
            } else if b.selected >= b.scroll as usize + h.max(1) {
                b.scroll = (b.selected + 1 - h.max(1)) as u16;
            }
        }
        View::Cloud(c) => {
            let h = app.viewport.height;
            c.move_selection(delta, h);
        }
    }
}

fn scroll_by_page(app: &mut App, dir: i32, half: bool) {
    // Paging is meaningless in the file browser — listings fit on one screen,
    // so a page jump would just wrap the selection around disorientingly. Row
    // movement there is `j`/`k` (and arrows) via `scroll_by`. Reader/Cloud page
    // normally.
    if matches!(app.view, View::Browser(_)) {
        return;
    }
    let h = app.viewport.height as i32;
    let amt = if half { (h / 2).max(1) } else { (h - 1).max(1) };
    scroll_by(app, dir * amt);
}

fn scroll_to(app: &mut App, line: u16) {
    if let View::Reader(r) = &mut app.view {
        let total = r.rendered.as_ref().map(|x| x.lines.len()).unwrap_or(0) as i32;
        let h = app.viewport.height as i32;
        let max = (total - h).max(0) as u16;
        r.scroll = line.min(max);
        r.focus = None;
    }
}

/// Right-arrow / Enter on a Browser: navigate into a directory (replacing
/// the current view, pushing it onto history) or open a file. No-op
/// outside Browser.
fn enter_or_open(app: &mut App) -> Result<()> {
    if let View::Cloud(c) = &app.view {
        if let Some(n) = c.selected_note() {
            let (id, title) = (n.id.clone(), n.title.clone());
            app.open_cloud_note(id, title);
        }
        return Ok(());
    }
    let entry = match &app.view {
        View::Browser(b) => b.entries.get(b.selected).cloned(),
        _ => return Ok(()),
    };
    if let Some(entry) = entry {
        activate_browser_entry(app, entry)?;
    }
    Ok(())
}

fn focus_next(app: &mut App) {
    walk_focus(app, 1);
}

fn focus_prev(app: &mut App) {
    walk_focus(app, -1);
}

/// Cycle focus by `delta` through the unified link+checkbox list.
fn walk_focus(app: &mut App, delta: i32) {
    if let View::Reader(r) = &mut app.view {
        let targets = r.focus_targets();
        let n = targets.len();
        if n == 0 {
            return;
        }
        let cur_idx = r
            .focus
            .and_then(|f| targets.iter().position(|(t, _, _)| *t == f));
        let new_idx: usize = match cur_idx {
            Some(i) => ((i as i32 + delta).rem_euclid(n as i32)) as usize,
            None if delta >= 0 => 0,
            None => n - 1,
        };
        let (focus, line, _col) = targets[new_idx];
        r.focus = Some(focus);
        center_on_line(app, line);
    }
}

fn center_on_line(app: &mut App, line: usize) {
    let h = app.viewport.height as usize;
    if let View::Reader(r) = &mut app.view {
        let scroll = r.scroll as usize;
        if line < scroll || line >= scroll + h.saturating_sub(1) {
            let new = line.saturating_sub(h / 2);
            r.scroll = new as u16;
        }
    }
}

fn activate(app: &mut App) -> Result<()> {
    let focus = match &app.view {
        View::Reader(r) => r.focus,
        _ => None,
    };
    match focus {
        Some(Focus::Link(fi)) => {
            if let View::Reader(r) = &app.view {
                if let Some(rendered) = &r.rendered {
                    if let Some(link) = rendered.link_map.links.get(fi) {
                        let target = link.target.clone();
                        app.follow(target)?;
                    }
                }
            }
            return Ok(());
        }
        Some(Focus::Checkbox(ci)) => {
            app.toggle_checkbox(ci)?;
            return Ok(());
        }
        None => {}
    }
    if let View::Browser(b) = &app.view {
        let entry = b.entries.get(b.selected).cloned();
        if let Some(e) = entry {
            activate_browser_entry(app, e)?;
        }
    }
    if let View::Cloud(c) = &app.view {
        if let Some(n) = c.selected_note() {
            let (id, title) = (n.id.clone(), n.title.clone());
            app.open_cloud_note(id, title);
        }
    }
    Ok(())
}

fn activate_browser_entry(app: &mut App, entry: BrowserEntry) -> Result<()> {
    match entry.kind {
        // Parent / dir: re-root the browser one level. Reader files: open.
        BrowserEntryKind::ParentDir | BrowserEntryKind::Dir => {
            app.navigate_to(EntryKind::Directory(entry.path), 0)?;
        }
        BrowserEntryKind::Markdown => {
            app.navigate_to(EntryKind::File(entry.path), 0)?;
        }
    }
    Ok(())
}

fn open_focused(app: &mut App) -> Result<()> {
    if let View::Reader(r) = &app.view {
        if let Some(Focus::Link(fi)) = r.focus {
            if let Some(rendered) = &r.rendered {
                if let Some(link) = rendered.link_map.links.get(fi) {
                    if let crate::tui::links::LinkTarget::Url(u) = &link.target {
                        let _ = open::that_detached(u);
                        app.status = format!("Opened {}", u);
                    }
                }
            }
            return Ok(());
        }
        // No focused link in a cloud reader: `o` opens the note's publish
        // link in the system browser instead.
        if let crate::tui::app::ReaderOrigin::CloudNote { publish_link, .. } = &r.origin {
            if publish_link.is_empty() {
                app.status = "No publish link yet".into();
            } else {
                let _ = open::that_detached(publish_link);
                app.status = format!("Opened {}", publish_link);
            }
        }
    }
    Ok(())
}

fn update_hover(app: &mut App, col: u16, row: u16) {
    let area = app.viewport;
    if let View::Reader(r) = &mut app.view {
        let Some(rendered) = &r.rendered else {
            return;
        };
        let line_num_w = if app.opts.line_numbers {
            (format!("{}", rendered.lines.len()).len() + 1) as u16
        } else {
            0
        };
        let inner_x = area.x + line_num_w;
        if col < inner_x {
            r.hover_link = None;
            r.hover_checkbox = None;
            r.hover_jsonl = None;
            return;
        }
        let local_col = (col - inner_x) as usize;
        let local_row = (row - area.y) as usize;
        let line_idx = r.scroll as usize + local_row;
        r.hover_link = rendered.link_map.at(line_idx, local_col);
        r.hover_checkbox = rendered.checkbox_map.at(line_idx, local_col);
        r.hover_jsonl = r
            .jsonl_overlay
            .as_ref()
            .and_then(|o| o.at(line_idx, local_col));
    }
}

fn click_at(app: &mut App, col: u16, row: u16) -> Result<()> {
    let area = app.viewport;
    // Cloud browser: a click on the tab bar switches workspaces; a click
    // on a note row selects + opens it.
    if let View::Cloud(_) = &app.view {
        let tab_bar = matches!(&app.view, View::Cloud(c) if c.show_tab_bar());
        // Row 0 is the bordered title; with multiple workspaces the next
        // row is the tab bar, then the list rows.
        let visual = (row - area.y) as usize;
        if tab_bar && visual == 1 {
            let hit = app
                .cloud_tab_hits
                .iter()
                .find(|&&(start, end, _)| col >= start && col <= end)
                .map(|&(_, _, i)| i);
            if let (Some(i), View::Cloud(c)) = (hit, &mut app.view) {
                c.active = i;
            }
            return Ok(());
        }
        let View::Cloud(c) = &mut app.view else {
            return Ok(());
        };
        let idx = visual.saturating_sub(1 + tab_bar as usize)
            + c.tab().map(|t| t.scroll).unwrap_or(0) as usize;
        let target = c
            .tab()
            .and_then(|t| t.notes.get(idx))
            .map(|n| (n.id.clone(), n.title.clone()));
        if let Some((id, title)) = target {
            if let Some(t) = c.tab_mut() {
                t.selected = idx;
            }
            app.open_cloud_note(id, title);
        }
        return Ok(());
    }
    let entry_to_open = match &mut app.view {
        View::Reader(r) => {
            let Some(rendered) = &r.rendered else {
                return Ok(());
            };
            let line_num_w = if app.opts.line_numbers {
                (format!("{}", rendered.lines.len()).len() + 1) as u16
            } else {
                0
            };
            let inner_x = area.x + line_num_w;
            if col < inner_x {
                return Ok(());
            }
            let local_col = (col - inner_x) as usize;
            let local_row = (row - area.y) as usize;
            let line_idx = r.scroll as usize + local_row;
            // Edit mode: click sets the source cursor and never follows links
            // / toggles checkboxes (so the user can edit link text without
            // having every click navigate away).
            if r.edit.is_some() {
                if let Some(offset) = xy_to_source_offset(rendered, &r.raw, line_idx, local_col) {
                    if let Some(e) = r.edit.as_mut() {
                        e.cursor = offset;
                        e.command = None;
                    }
                    r.rendered = None;
                }
                return Ok(());
            }
            // A click anywhere in the body ends a Tab-cycling session; the
            // link branch below re-focuses when the click lands on a link.
            r.focus = None;
            // JSON-line expand button beats every other hit-test on its row:
            // it sits in the left gutter so it never overlaps real content.
            if let Some(bi) = r
                .jsonl_overlay
                .as_ref()
                .and_then(|o| o.at(line_idx, local_col))
            {
                let src = r.jsonl_overlay.as_ref().unwrap().buttons[bi].source_line;
                match r.toggle_jsonl_line(src) {
                    Ok(_) => {
                        r.hover_jsonl = None;
                    }
                    Err(msg) => app.status = msg.to_string(),
                }
                return Ok(());
            }
            // Checkbox takes priority over link (the marker isn't part of any link).
            if let Some(ci) = rendered.checkbox_map.at(line_idx, local_col) {
                app.toggle_checkbox(ci)?;
                return Ok(());
            }
            // A link inside a table cell still follows the link; only clicks
            // that miss every link toggle table expansion (handled below).
            if let Some(li) = rendered.link_map.at(line_idx, local_col) {
                let target = rendered.link_map.links[li].target.clone();
                r.focus = Some(Focus::Link(li));
                app.follow(target)?;
                return Ok(());
            }
            // Table click-to-expand: border → whole table, header cell →
            // column, body cell → that cell.
            if let Some((id, hit)) = rendered.table_map.hit(line_idx, local_col) {
                r.toggle_table(id, hit);
                return Ok(());
            }
            None
        }
        View::Browser(b) => {
            let local_row = (row - area.y) as usize;
            // Row 0 is the bordered title; list rows start at 1.
            let visual = local_row.saturating_sub(1);
            let idx = visual + b.scroll as usize;
            if idx < b.entries.len() {
                b.selected = idx;
                Some(b.entries[idx].clone())
            } else {
                None
            }
        }
        // Handled by the early return above.
        View::Cloud(_) => None,
    };
    if let Some(entry) = entry_to_open {
        activate_browser_entry(app, entry)?;
    }
    Ok(())
}

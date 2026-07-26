use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use ratatui::layout::Rect;

use crate::tui::cloud::{CachedNote, CloudContext, CloudMsg, CloudState, FetchedNote};
use crate::tui::jsonl::{self, JsonlOverlay};
use crate::tui::links::LinkTarget;
use crate::tui::markdown::{self, Rendered};
use crate::tui::theme::Theme;
use ratatui_image::picker::Picker;
use std::collections::HashSet;

/// Statusline hint shown when a cloud action is attempted with no token.
pub const NO_TOKEN_HINT: &str = "No HackMD token — run `hackmd login` or set HMD_API_ACCESS_TOKEN";

#[derive(Clone, Debug)]
pub enum Source {
    File(PathBuf),
    Directory(PathBuf),
    Stdin(String),
}

#[derive(Clone, Debug)]
pub struct Options {
    pub width: u16,
    pub line_numbers: bool,
    pub theme: Theme,
}

pub struct App {
    pub view: View,
    /// Search root — set at launch from the file/dir argument; never moves.
    pub root: PathBuf,
    pub history: Vec<HistoryEntry>,
    pub forward: Vec<HistoryEntry>,
    pub opts: Options,
    pub help_open: bool,
    /// `Some` while the fuzzy search overlay is active.
    pub search: Option<Search>,
    pub should_quit: bool,
    pub status: String,
    pub viewport: Rect,
    /// In Marp presentation mode, the rect the current slide's body is drawn
    /// into (already offset for vertical centering). The image overlay uses
    /// this so images line up with the centered text. `None` when not
    /// presenting.
    pub slide_area: Option<Rect>,
    /// Marp slide background images to paint this frame: `(source, region)`.
    /// A `![bg left/right]` image reserves a column; the overlay fills it.
    /// Rebuilt each present-mode frame, cleared otherwise.
    pub slide_bg: Vec<(String, Rect)>,
    /// The row containing the statusline. Click handling on this row covers
    /// the back-button hit zone.
    pub statusline_area: Rect,
    /// Last column range occupied by the `[‹ Back]` button in the statusline,
    /// recorded by the renderer so click handling can hit-test it.
    pub back_button_hit: Option<(u16, u16)>,
    /// Column ranges of the cloud browser's workspace tab labels
    /// (`start_col, end_col, tab index`), recorded by the renderer so a
    /// click on the tab bar can switch workspaces.
    pub cloud_tab_hits: Vec<(u16, u16, usize)>,
    /// Column range and full target of the URL shown in the statusline middle,
    /// recorded by the renderer so a click on it can copy the (untruncated)
    /// URL to the clipboard.
    pub statusline_url_hit: Option<(u16, u16, String)>,
    /// Column range and publish link of the HackMD status badge shown on the
    /// statusline right (just left of the scroll indicator), recorded by the
    /// renderer so a click on it copies the note's publish link. `None` when
    /// the current doc isn't linked to HackMD or has no link to copy.
    pub statusline_badge_hit: Option<(u16, u16, String)>,
    /// The file path on the statusline left, recorded by the renderer:
    /// `(start_col, end_col, displayed text, full path to copy)`. A click on
    /// it copies the full path; a drag selects part of the displayed text.
    /// `None` when the current view has no copyable path (cloud note, stdin).
    pub statusline_path_hit: Option<(u16, u16, String, String)>,
    /// In-progress drag over the statusline path: `(anchor_col, focus_col)`.
    /// Armed on mouse-down over the path, resolved on mouse-up (click →
    /// full path, drag → the column-selected slice).
    pub statusline_path_drag: Option<(u16, u16)>,
    /// Pending vim count prefix (e.g. user typed `5` waiting for `j`). Reset
    /// after the motion key consumes it, or on Esc.
    pub count_prefix: Option<u32>,
    /// `Some(instant)` when the user pressed `g` and we're waiting for the
    /// second key of a `gg`/`ge`/`gh`-style chord. Times out after ~700ms so
    /// a stray `g` doesn't lock subsequent input.
    pub pending_g: Option<std::time::Instant>,
    /// `Some(instant)` waiting for the second key of a `zz` chord.
    pub pending_z: Option<std::time::Instant>,
    /// `Some((bracket, instant))` waiting for the second key of a `]]` or
    /// `[[` heading-jump chord. `bracket` is `']'` or `'['`.
    pub pending_bracket: Option<(char, std::time::Instant)>,
    /// Table-of-contents overlay (`t` in the Reader). `Some` while open.
    pub toc: Option<TocState>,
    /// True when the most recent input was a mouse event. While set we hide
    /// the keyboard focus highlight so the user isn't tracking two cursors.
    pub mouse_recent: bool,
    /// Last observed mouse column inside the body. Used by the statusline
    /// hover-URL edge case to decide which side of the row to render on when
    /// the hovered link sits on the bottom-most body row.
    pub last_mouse_col: u16,
    /// Last observed mouse row. Together with `last_mouse_col` lets the
    /// statusline detect "mouse is right above the statusline" cases.
    pub last_mouse_row: u16,
    /// Mouse capture state. When `false`, drag/click events fall through to
    /// the terminal so the user can select text natively.
    pub mouse_enabled: bool,
    /// In-app text selection state. Set on first Drag after Mouse Down,
    /// cleared on Up (after copying) or any view-mutating action.
    pub selection: Option<Selection>,
    /// Git lens overlay state. `Some` while the user has Ctrl-G toggled on.
    /// Holds the parsed diff vs HEAD (staged + unstaged combined) for the
    /// current file. `None` otherwise.
    pub git_lens: Option<GitLensState>,
    /// Most recent left-mouse-down (Instant + column + row), used to detect
    /// double-clicks for word selection.
    pub last_click: Option<(std::time::Instant, u16, u16)>,
    /// On left-mouse-down we stash a pending single-click target. It fires
    /// on Up only if no Drag arrived in between (so drag-select doesn't
    /// also navigate).
    pub pending_click: Option<(u16, u16)>,
    /// Anchor to scroll to once the freshly-opened document has been laid out.
    /// A cross-file `path#anchor` link navigates first, and the new reader has
    /// no render yet — the slug can only be resolved to a display line after
    /// the next `ensure_rendered`, so it waits here.
    pub pending_anchor: Option<String>,
    /// Sub-line accumulator for wheel-scroll dampening. Carries fractional
    /// lines across events so a halved scroll factor still produces smooth
    /// movement instead of "stuck" frames where nothing happens.
    pub scroll_accum: f32,
    /// Timestamp of the most recent wheel scroll event. `None` until the
    /// first wheel tick. Used to detect bursts.
    pub last_scroll_at: Option<std::time::Instant>,
    /// Detected terminal image protocol. `None` if the terminal can't render
    /// images (then we fall back to placeholder text).
    pub image_picker: Option<Picker>,
    /// Async image loader + decoded-protocol cache, keyed by source string
    /// (local path or remote URL). Handles remote download and SVG rasterizing.
    pub images: crate::tui::images::ImageStore,
    /// Raw-pane and preview-pane rects from the last frame in split-edit
    /// mode. Used by event routing (which pane received the click / wheel).
    /// Both default to `Rect::default()` outside split-edit mode.
    pub edit_raw_area: Rect,
    pub edit_preview_area: Rect,
    /// Screen cell of the raw-pane edit cursor as last drawn, used to anchor
    /// the heading-anchor autocomplete popup just below it. `None` when the
    /// cursor is off-screen or not in split-edit mode.
    pub edit_cursor_screen: Option<(u16, u16)>,
    /// Machine-local read/unread tracking for the file browser.
    pub read_state: crate::tui::read_state::ReadState,
    /// HackMD connection + caches. Disconnected (and inert) without a token.
    pub cloud: CloudState,
    /// Most recent local (file/dir/stdin) view, so the cloud toggle can
    /// land back where the user came from instead of resetting to the root.
    pub last_local: Option<EntryKind>,
    /// `Some` while a text/confirm prompt overlay is active (new note title,
    /// push title, download filename, delete confirmation).
    pub prompt: Option<Prompt>,
    /// Internal yank register — the last text copied or cut in the editor.
    /// Paste prefers the system clipboard (so external copies work) but falls
    /// back to this when the clipboard can't be read (no `pbpaste`, headless).
    pub yank: Option<String>,
    /// Throttle state for mirroring the dirty edit buffer to its crash-
    /// recovery file: `(when, content_hash)` of the last write, so an
    /// unchanged buffer or a too-recent write is skipped.
    pub recovery_throttle: Option<(std::time::Instant, u64)>,
    /// True while a local↔HackMD sync fetch is in flight, so we don't stack
    /// duplicate requests for the same file.
    pub pending_sync: bool,
    /// `(path, when)` of the last linked file we offered to sync. Records that
    /// the open file has already been handled (prompted or synced) so it isn't
    /// re-offered every tick. There is no background re-poll: an API quota as
    /// low as 400 calls/month makes periodic upstream polling too expensive, so
    /// a still-open file is fetched only when the user asks.
    pub last_sync: Option<(PathBuf, std::time::Instant)>,
    /// When the most recent HackMD `429 Too Many Requests` landed. Drives the
    /// live "rate limited (Ns ago)" statusline badge; survives the per-keypress
    /// `status.clear()` so the warning persists after the message is gone.
    pub last_rate_limit: Option<std::time::Instant>,
    /// Active conflict-resolution session (local vs upstream), shown as a
    /// full-screen resolver. `None` when there's no unresolved conflict.
    pub conflict: Option<ConflictState>,
    /// Editor `:`-command history (most recent last), navigated with ↑/↓ on
    /// the command line. Persists across edit sessions within a run.
    pub edit_cmd_history: Vec<String>,
    /// Transient cursor into `edit_cmd_history` while navigating it; `None`
    /// when on the live (just-typed) command line.
    pub edit_cmd_nav: Option<usize>,
    /// Dictionary-definition popover (double-click a word). `None` when closed.
    pub lookup: Option<LookupState>,
    /// `Some(message)` while a blocking error modal is shown (e.g. trying to
    /// open a file that isn't valid UTF-8). Dismissed by the next keypress.
    pub error: Option<String>,
    /// Cached uncommitted-file set for the repo the browser / reader is in.
    /// Drives the `[uncommitted]` browser badge and seeds the commit screen.
    pub git_status: crate::tui::git::GitStatus,
    /// `Some` while the git commit screen is open. `None` otherwise.
    pub commit: Option<CommitState>,
}

/// In-TUI dictionary-definition popover, opened by double-clicking a word in
/// the reader (which also copies it). Anchored to the word on screen and
/// dismissed on Esc, a scroll, or a click outside it.
pub struct LookupState {
    /// The word being looked up. The async `Defined` reply is matched against
    /// this so a stale result for a previous word is ignored.
    pub word: String,
    /// Word position for anchoring the popover: `(start_col, row, width)` in
    /// absolute terminal cells.
    pub anchor: (u16, u16, u16),
    /// Lookup progress / result.
    pub status: LookupStatus,
    /// Scroll offset into the definition text, for entries taller than the box.
    pub scroll: u16,
    /// Popup rect from the last frame, recorded by the renderer for mouse
    /// hit-testing (click-inside vs click-outside). Empty until first drawn.
    pub rect: Rect,
}

/// Progress of a dictionary lookup shown in the popover.
pub enum LookupStatus {
    /// Background lookup in flight.
    Loading,
    /// Definition text ready to display.
    Ready(String),
    /// The word has no dictionary entry (or lookup isn't supported here).
    NotFound,
}

/// A full-screen conflict-resolution session: the three-way merge found
/// regions local and upstream both changed, and the user picks a side per
/// hunk before the merged result is written back and pushed.
pub struct ConflictState {
    pub path: PathBuf,
    pub id: String,
    /// Link metadata, carried through so the resolved file is re-stamped.
    pub meta: crate::tui::hackmd_meta::HackmdMeta,
    /// The document in order: stable text interleaved with conflict hunks.
    pub items: Vec<ConflictItem>,
    /// Index of the focused conflict hunk among the conflict items only.
    pub selected: usize,
    /// Scroll offset into the rendered resolver view.
    pub scroll: u16,
}

/// One piece of the conflicted document.
pub enum ConflictItem {
    /// Agreed text, shown dimmed and uneditable.
    Stable(String),
    /// A region both sides changed; the user resolves it via `choice`.
    Conflict {
        local: String,
        remote: String,
        choice: ConflictChoice,
    },
}

/// Which side of a conflict hunk the user picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictChoice {
    /// Not yet decided — blocks finishing the resolution.
    Unresolved,
    Local,
    Remote,
    /// Keep both, local first.
    Both,
    /// Drop the hunk entirely.
    Neither,
}

impl ConflictState {
    /// Total number of conflict hunks.
    pub fn conflict_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i, ConflictItem::Conflict { .. }))
            .count()
    }

    /// Number of hunks still `Unresolved`.
    pub fn unresolved_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| {
                matches!(
                    i,
                    ConflictItem::Conflict {
                        choice: ConflictChoice::Unresolved,
                        ..
                    }
                )
            })
            .count()
    }

    /// Set the choice on the currently-selected conflict hunk.
    pub fn set_choice(&mut self, choice: ConflictChoice) {
        if let Some(ConflictItem::Conflict { choice: c, .. }) = self
            .items
            .iter_mut()
            .filter(|i| matches!(i, ConflictItem::Conflict { .. }))
            .nth(self.selected)
        {
            *c = choice;
        }
    }

    /// Move the selected-hunk cursor by `delta`, clamped.
    pub fn step(&mut self, delta: i32) {
        let n = self.conflict_count();
        if n == 0 {
            return;
        }
        let next = (self.selected as i32 + delta).clamp(0, n as i32 - 1) as usize;
        self.selected = next;
    }

    /// Assemble the resolved document from the current choices. Returns `None`
    /// if any hunk is still unresolved.
    pub fn assemble(&self) -> Option<String> {
        let mut out = String::new();
        for item in &self.items {
            match item {
                ConflictItem::Stable(s) => out.push_str(s),
                ConflictItem::Conflict {
                    local,
                    remote,
                    choice,
                } => match choice {
                    ConflictChoice::Unresolved => return None,
                    ConflictChoice::Local => out.push_str(local),
                    ConflictChoice::Remote => out.push_str(remote),
                    ConflictChoice::Both => {
                        out.push_str(local);
                        out.push_str(remote);
                    }
                    ConflictChoice::Neither => {}
                },
            }
        }
        Some(out)
    }
}

/// Modal one-line prompt. Input handling mirrors the doc-search prompt:
/// chars append, Backspace pops, Ctrl-U clears, Enter commits, Esc cancels.
/// `ConfirmDelete` is the exception — `y`/Enter confirms, anything cancels.
pub struct Prompt {
    /// Box title shown to the user (already padded/decorated).
    pub title: String,
    pub input: String,
    pub kind: PromptKind,
}

pub enum PromptKind {
    /// `n` in the cloud browser — create a note with the typed title.
    NewNoteTitle,
    /// `U` on a local file — push it up as a new cloud note with this title.
    PushTitle(PathBuf),
    /// `S` on a cloud note — write its content under `app.root` as this name.
    DownloadFilename { id: String },
    /// `D` on a cloud note — destructive, so it gets an explicit confirm.
    ConfirmDelete {
        id: String,
        title: String,
        team_path: Option<String>,
    },
    /// `n` in the local browser — create a file with the typed name under
    /// `dir`, then open it in the editor. A name ending in `/` creates a
    /// directory instead and browses into it.
    NewFile { dir: PathBuf },
    /// `r` / F2 in the local browser — rename the entry at `from` to the
    /// typed name (kept in the same parent directory).
    RenameFile { from: PathBuf },
    /// `m` in the local browser — move the entry at `from` to the typed
    /// destination, read relative to `dir` (the browsed directory).
    MoveEntry { from: PathBuf, dir: PathBuf },
    /// A dirty editor is about to be abandoned (second Esc / `:q` / Ctrl-C).
    /// Rather than silently dropping the buffer, ask first: `s` saves, `d`
    /// discards, Esc keeps editing. `after` is what to do once resolved.
    ConfirmDiscardEdit { after: AfterEdit },
    /// A file was opened that has unsaved edits mirrored to a crash-recovery
    /// file. Offer to restore them: `r`/Enter recovers into the editor, `d`
    /// discards the recovery file, Esc leaves it for next time. Carries the
    /// recovered buffer and cursor.
    RecoverEdit { content: String, cursor: usize },
    /// A HackMD-linked local file was just opened in the reader. Pulling the
    /// upstream copy costs an API call (quota is as low as 400/month on a Free
    /// workspace), so ask before spending one: `y`/Enter fetches, any other
    /// key keeps the local copy.
    ConfirmFetchUpdate { path: PathBuf },
    /// The editor was just left with the file still holding uncommitted
    /// changes. Offer to commit it there and then: Enter commits `file` (only)
    /// with the typed message, Esc skips (leaving it uncommitted). `root` is
    /// the enclosing repo's worktree root.
    CommitFile { root: PathBuf, file: PathBuf },
}

/// What to do once a dirty editor has been resolved through the
/// [`PromptKind::ConfirmDiscardEdit`] prompt.
#[derive(Clone, Copy)]
pub enum AfterEdit {
    /// Leave edit mode, back to the reader (second Esc / `:q`).
    Exit,
    /// Quit the whole app (Ctrl-C while editing).
    Quit,
}

/// The cloud note a context-sensitive action (`P`/`D`/`S`/`y`/`o`) applies
/// to: the selected browser row, or the open cloud reader.
pub struct CloudTarget {
    pub id: String,
    pub title: String,
    pub team_path: Option<String>,
    pub published: bool,
    /// Empty when targeting a browser row (rows don't carry the link); the
    /// reader origin always has it.
    pub publish_link: String,
}

/// Visibility/sync state of the HackMD note the open reader is linked to,
/// surfaced as a status-line badge. `None` for docs not on HackMD.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HackmdBadgeKind {
    /// A cloud sync is in flight for this note.
    Syncing,
    /// Readable by anyone with the link (`read_permission == Guest`).
    Public,
    /// Owner-only / signed-in-only — not publicly readable.
    Private,
    /// Linked to a note whose live permission we haven't fetched yet.
    Unknown,
}

pub struct HackmdBadge {
    pub kind: HackmdBadgeKind,
    /// Short label, e.g. "Public", "Private (only me)", "Syncing".
    pub label: String,
    /// Publish link to copy when the badge is clicked (may be empty).
    pub link: String,
}

pub enum View {
    Reader(Reader),
    Browser(Browser),
    /// HackMD note browser — entered via `H` / `gh`.
    Cloud(CloudBrowser),
}

#[derive(Clone)]
pub struct HistoryEntry {
    pub kind: EntryKind,
    pub scroll: u16,
    /// Browser cursor position. `None` for reader entries.
    pub selected: Option<usize>,
}

#[derive(Clone)]
pub enum EntryKind {
    File(PathBuf),
    Directory(PathBuf),
    Stdin(String),
    /// The HackMD note browser.
    CloudList,
    /// A single HackMD note. Title is carried so history navigation to an
    /// evicted note can still label its placeholder while it refetches.
    CloudNote {
        id: String,
        title: String,
    },
}

pub struct Reader {
    pub origin: ReaderOrigin,
    pub raw: String,
    pub rendered: Option<Rendered>,
    pub scroll: u16,
    /// In split-screen edit mode this is the preview-pane scroll (right /
    /// bottom side); the raw pane scroll lives on `Reader::scroll`.
    pub preview_scroll: u16,
    /// Unified keyboard cursor. Walks links AND checkboxes in document order
    /// via Tab / S-Tab. Suppressed visually while the mouse is recent.
    pub focus: Option<Focus>,
    pub hover_link: Option<usize>,
    pub hover_checkbox: Option<usize>,
    /// Index into `Rendered::headings` for the heading under the mouse, so the
    /// reader can advertise that clicking it copies a link to that section.
    /// `None` whenever a link or checkbox owns the same cell — those win.
    pub hover_heading: Option<usize>,
    pub doc_search: Option<DocSearch>,
    /// In-house edit mode. `Some` while the user is editing this buffer.
    pub edit: Option<EditState>,
    /// (mtime, size) snapshot of the source file at last read. Used by the
    /// event loop to detect external edits and reload. `None` for stdin or
    /// when the metadata wasn't available at load time.
    pub last_meta: Option<(std::time::SystemTime, u64)>,
    /// Set when the source isn't markdown (e.g. .json, .rs, .txt). The string
    /// is the syntect language token (empty = plain text). At render time we
    /// wrap `raw` in a fenced code block so the existing markdown pipeline
    /// gives us syntax highlighting for free. `raw` itself stays unwrapped so
    /// editing and saving operate on the actual file content.
    pub wrap_lang: Option<String>,
    /// JSON-line reader: source line indices the user has expanded into pretty-
    /// printed form. Empty for non-JSON files.
    pub jsonl_expanded: HashSet<usize>,
    /// JSON-line button hit boxes, rebuilt every render. `None` for non-JSON
    /// files or when no buttons are needed (no line overflows).
    pub jsonl_overlay: Option<JsonlOverlay>,
    /// Hover index into `jsonl_overlay.buttons` for cursor feedback.
    pub hover_jsonl: Option<usize>,
    /// Per-table click-to-expand state, keyed by the table's source byte
    /// offset. Clicking a table border expands the whole table; a header cell
    /// expands that column; a body cell expands just that cell. Threaded into
    /// the renderer so expanded parts show full, untruncated content.
    pub tables: crate::tui::links::TableExpansions,
    /// Present when the document is a Marp deck. Drives slide-at-a-time
    /// presentation mode; `None` for ordinary documents.
    pub marp: Option<MarpView>,
}

/// Presentation state for a Marp deck open in the reader.
pub struct MarpView {
    /// The parsed deck (slides + resolved directives).
    pub deck: crate::tui::marp::Deck,
    /// Index of the slide currently shown (`0`-based).
    pub slide: usize,
    /// `true` while showing slides one-at-a-time; `false` falls back to the
    /// ordinary scrolling reader over the full document.
    pub present: bool,
}

impl MarpView {
    /// The slide currently on screen, if any.
    pub fn current(&self) -> Option<&crate::tui::marp::Slide> {
        self.deck.slides.get(self.slide)
    }
}

/// Default-active edit-mode UI. `Split` is the new HackMD-style two-pane
/// editor (raw on one side, rendered preview on the other). `InPlace` is
/// the legacy block-toggle mode — kept compiled but inactive so we can
/// reactivate it later without rewriting from scratch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditMode {
    Split,
    #[allow(dead_code)]
    InPlace,
}

#[derive(Clone, Debug)]
pub struct EditState {
    /// Cursor position as a byte offset into `Reader::raw`. Always lands on
    /// a UTF-8 char boundary; helpers in the events layer enforce that.
    pub cursor: usize,
    /// True after any insert/delete since the last save or load.
    pub dirty: bool,
    /// Vim-style command line. `Some(input)` after the first Esc moves the
    /// cursor to the statusline; the user types a command (`:wq`, `:q`,
    /// `:preview`, …) there, Enter executes it, and a second Esc leaves the
    /// editor (asking save/discard first if dirty). Any buffer interaction
    /// (click, typing after it's dismissed) clears it back to insert mode.
    pub command: Option<String>,
    /// `:preview` overlay — render the unsaved buffer full-screen like the
    /// reader; a single Esc drops back to the editor.
    pub preview_full: bool,
    /// Undo stack: prior (raw, cursor) snapshots. Pushed before each
    /// mutating op; Ctrl-Z pops to revert.
    pub undo: Vec<EditSnapshot>,
    /// Redo stack: snapshots popped from undo after a Ctrl-Z. Cleared on
    /// any new mutation (since the future timeline diverged).
    pub redo: Vec<EditSnapshot>,
    /// Which UI flavor to render. New edits start `Split`.
    pub mode: EditMode,
    /// Cursor byte offset the last time we drew. The draw layer compares
    /// this with the current cursor to decide whether to scroll the raw
    /// pane to keep the cursor on-screen. `None` means "never drawn yet —
    /// follow on first frame so the cursor is initially visible". Wheel
    /// scrolling does not touch the cursor, so the follow logic stays put
    /// and the scroll sticks.
    pub last_drawn_cursor: Option<usize>,
    /// Drag-selection in the raw pane. Armed on mouse-down, activated by
    /// the first drag that moves off the anchor, and kept after mouse-up
    /// so the user can choose an action (`y` copy, Del delete) instead of
    /// the reader's copy-on-release behaviour.
    pub selection: Option<EditSelection>,
    /// Heading-anchor autocomplete popup. `Some` while the cursor sits inside
    /// a `[](…#…)` link destination after a `#`, offering the referenced
    /// document's headings. Up/Down move the selection, Tab/Enter (or a click)
    /// write the chosen anchor slug, Esc dismisses.
    pub anchor_complete: Option<AnchorComplete>,
}

/// Live state for the heading-anchor autocomplete popup (see
/// [`EditState::anchor_complete`]).
#[derive(Clone, Debug)]
pub struct AnchorComplete {
    /// Byte offset of the `#` that opened the popup, in `Reader::raw`. The
    /// anchor text being completed is `raw[hash + 1 .. cursor]`.
    pub hash: usize,
    /// Every heading of the referenced document (the current buffer when the
    /// path before `#` is empty, else the linked file), built once on open.
    pub candidates: Vec<crate::tui::links::DocHeading>,
    /// Indices into `candidates` that match the current query, in order.
    pub matches: Vec<usize>,
    /// Selected position within `matches`.
    pub selected: usize,
    /// First visible entry, windowed around `selected` at draw time. Stored so
    /// mouse clicks can map a screen row back to a match.
    pub scroll: usize,
    /// Screen rect of the popup as last drawn, for click hit-testing.
    pub rect: Rect,
}

/// A drag-selection in the raw editor pane, as byte offsets into
/// `Reader::raw`. Both ends always sit on UTF-8 char boundaries (they come
/// from the same click→byte mapping as the cursor).
#[derive(Clone, Debug)]
pub struct EditSelection {
    /// Selection start (a char-boundary byte offset). Together with `focus`
    /// this is the resolved `[start, end)` range: `focus` is EXCLUSIVE, so it
    /// sits at the far edge of the last selected char, not on its first byte.
    /// A mouse drag keeps this convention (see `origin`), so copy / link /
    /// delete all include the final char.
    pub anchor: usize,
    /// Selection end, EXCLUSIVE. May be before `anchor` (left-going drag).
    pub focus: usize,
    /// The byte the mouse first went down on (a char start). Held fixed for
    /// the whole drag so each Drag event can rebuild the exclusive range from
    /// (origin, pointer) without the anchor/focus it writes back drifting.
    /// Unused by keyboard/shift-click selections (they set it to `anchor`).
    pub origin: usize,
    /// True once a drag moved the focus off the anchor; a plain click
    /// never activates the selection.
    pub dragged: bool,
}

impl EditSelection {
    /// Ordered `(start, end)` byte range, end-exclusive.
    pub fn range(&self) -> (usize, usize) {
        (self.anchor.min(self.focus), self.anchor.max(self.focus))
    }

    /// True if the selection covers a non-empty range the user dragged out.
    pub fn is_active(&self) -> bool {
        self.dragged && self.anchor != self.focus
    }
}

#[derive(Clone, Debug)]
pub struct EditSnapshot {
    pub raw: String,
    pub cursor: usize,
}

/// Soft cap on undo depth. Picked to balance memory (each snapshot is one
/// String clone) against typical editing sessions.
pub const UNDO_LIMIT: usize = 200;

/// In-app drag-select state. Anchor and focus are stored in (line_index,
/// display_col) where line_index is the row in `Rendered::lines` (so
/// scrolling mid-drag doesn't tear the range). Anchor stays put once set;
/// focus moves with the mouse on each Drag event.
#[derive(Clone, Copy, Debug)]
pub struct Selection {
    pub anchor_line: usize,
    pub anchor_col: u16,
    pub focus_line: usize,
    pub focus_col: u16,
    /// True once the user has dragged at least one cell; pure clicks
    /// (Down + Up with no Drag) leave this false and don't trigger copy.
    pub dragged: bool,
}

impl Selection {
    /// Return the (start, end) pair in document order, regardless of
    /// drag direction.
    pub fn normalized(&self) -> ((usize, u16), (usize, u16)) {
        let a = (self.anchor_line, self.anchor_col);
        let b = (self.focus_line, self.focus_col);
        if a <= b { (a, b) } else { (b, a) }
    }
    /// True if the selection covers any non-empty range.
    pub fn is_active(&self) -> bool {
        self.dragged && (self.anchor_line != self.focus_line || self.anchor_col != self.focus_col)
    }
}

/// Git commit screen (`gc`): a full-screen modal listing the repo's
/// uncommitted files with a per-file include toggle and +/- line counts,
/// plus a commit-message input at the bottom. Files whose changes the user
/// wants to commit are checked; the rest are shown but excluded.
pub struct CommitState {
    /// Worktree root the commit runs against.
    pub root: PathBuf,
    /// Every uncommitted file in the repo, sorted by path.
    pub files: Vec<CommitFile>,
    /// Cursor row within `files` (which entry `space` toggles). The list
    /// scrolls to keep this row in view; there's no separate scroll offset.
    pub selected: usize,
    /// The commit message being typed.
    pub message: String,
    /// Which pane has keyboard focus (the file list or the message box).
    pub focus: CommitFocus,
}

/// One row in the commit screen's file list.
pub struct CommitFile {
    /// Absolute path (what `git` is invoked with).
    pub path: PathBuf,
    /// Path relative to `root`, for display.
    pub rel: String,
    /// Added / removed line counts vs HEAD (untracked: whole file added).
    pub added: usize,
    pub removed: usize,
    /// Whether this file will be committed.
    pub include: bool,
}

/// Focus target within the commit screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitFocus {
    List,
    Message,
}

/// Which uncommitted paths to pre-check when the commit screen opens, derived
/// from the selection that triggered it.
enum CommitScope {
    /// A single file (open in the reader, or selected in the browser).
    File(PathBuf),
    /// Every uncommitted file under this directory.
    Dir(PathBuf),
    /// No preselection (nothing checked initially).
    None,
}

impl CommitScope {
    /// Whether an uncommitted file falls in this scope.
    fn contains(&self, path: &Path) -> bool {
        match self {
            CommitScope::File(f) => path == f,
            CommitScope::Dir(d) => path.starts_with(d),
            CommitScope::None => false,
        }
    }
}

impl CommitState {
    /// Number of files currently checked for inclusion.
    pub fn included_count(&self) -> usize {
        self.files.iter().filter(|f| f.include).count()
    }

    /// Absolute paths of every checked file.
    pub fn included_paths(&self) -> Vec<PathBuf> {
        self.files
            .iter()
            .filter(|f| f.include)
            .map(|f| f.path.clone())
            .collect()
    }

    /// Move the list cursor by `delta`, clamped to the list bounds.
    pub fn step(&mut self, delta: i32) {
        if self.files.is_empty() {
            return;
        }
        let last = self.files.len() as i32 - 1;
        let next = (self.selected as i32 + delta).clamp(0, last);
        self.selected = next as usize;
    }
}

/// Pre-parsed `git diff HEAD -- <file>` content. Each entry is a single
/// display row tagged with how it should be styled. We deliberately keep
/// the parser tiny and tolerant — the goal is a quick visual lens, not a
/// fully-correct diff parser.
#[derive(Clone, Debug)]
pub struct GitLensState {
    pub rows: Vec<DiffRow>,
    /// Scroll offset within the diff overlay.
    pub scroll: u16,
}

#[derive(Clone, Debug)]
pub struct DiffRow {
    pub kind: DiffRowKind,
    pub text: String,
}

/// Table-of-contents overlay (`t` in the Reader): a jump list over
/// `Rendered::headings`. Selection moves with j/k; Enter scrolls the
/// reader to the selected heading.
#[derive(Clone, Copy, Debug, Default)]
pub struct TocState {
    /// Index into `Rendered::headings`.
    pub selected: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffRowKind {
    /// Context line (unchanged) — rendered plain.
    Context,
    /// Added line — green background.
    Added,
    /// Removed line — red background.
    Removed,
    /// Hunk header (`@@ -a,b +c,d @@`) — muted fg.
    Hunk,
    /// File header (`diff --git`, `index`, `---`, `+++`) — muted, dim.
    Header,
    /// Synthetic informational row (e.g. "no changes") — muted bold.
    Info,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Link(usize),
    Checkbox(usize),
}

#[derive(Clone, Debug)]
pub struct DocSearch {
    pub query: String,
    pub matches: Vec<DocMatch>,
    pub current: usize,
    /// True while the user is still typing the query (prompt is open).
    pub editing: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct DocMatch {
    pub line: usize,
    pub col_start: usize,
    pub col_end: usize,
}

#[derive(Clone)]
pub enum ReaderOrigin {
    File(PathBuf),
    Stdin,
    /// A note living on hackmd.io. Mirrors the metadata the cloud actions
    /// (publish toggle, copy link, save) need without a cache lookup.
    CloudNote {
        id: String,
        title: String,
        team_path: Option<String>,
        publish_link: String,
        read_permission: crate::types::NotePermissionRole,
        /// ETag of the fetched revision, for conditional revalidation.
        etag: Option<String>,
    },
}

/// The cloud browser: a flat list of "My notes" + per-team sections.
/// Folders are deliberately not modeled (v1 keeps the list flat).
pub struct CloudBrowser {
    /// One tab per workspace: "My notes" first, then each team.
    pub tabs: Vec<CloudTab>,
    /// Index into `tabs` of the workspace being shown.
    pub active: usize,
}

/// One workspace (personal or a team) shown as a tab in the cloud browser.
pub struct CloudTab {
    /// Tab label: "My notes" or the team name.
    pub label: String,
    pub notes: Vec<CloudNoteRow>,
    pub selected: usize,
    pub scroll: u16,
}

pub struct CloudNoteRow {
    pub id: String,
    pub title: String,
    /// `Some` when the note belongs to a team (drives the team API variants).
    pub team_path: Option<String>,
    /// True when `readPermission == guest` — drives the `P` publish toggle.
    pub published: bool,
    /// Access summary badge — see [`crate::types::Note::visibility`].
    pub visibility: &'static str,
}

impl CloudBrowser {
    /// Group the cached lists into workspace tabs. With no lists yet (still
    /// fetching, or not logged in) the browser comes up with no tabs and the
    /// draw layer shows an explanatory line instead.
    pub fn from_lists(lists: Option<&crate::tui::cloud::CloudLists>) -> Self {
        let mut tabs = Vec::new();
        if let Some(l) = lists {
            let note_row = |n: &crate::types::Note, team_path: Option<&str>| CloudNoteRow {
                id: n.id.clone(),
                title: n.title.clone(),
                team_path: team_path.map(str::to_string),
                published: matches!(n.read_permission, crate::types::NotePermissionRole::Guest),
                visibility: n.visibility(),
            };
            tabs.push(CloudTab {
                label: "My notes".to_string(),
                notes: l.notes.iter().map(|n| note_row(n, None)).collect(),
                selected: 0,
                scroll: 0,
            });
            for t in &l.teams {
                tabs.push(CloudTab {
                    label: t.team.name.clone(),
                    notes: t
                        .notes
                        .iter()
                        .map(|n| note_row(n, Some(&t.team.path)))
                        .collect(),
                    selected: 0,
                    scroll: 0,
                });
            }
        }
        Self { tabs, active: 0 }
    }

    /// The active workspace tab. `None` only before the lists have loaded.
    pub fn tab(&self) -> Option<&CloudTab> {
        self.tabs.get(self.active)
    }

    pub fn tab_mut(&mut self) -> Option<&mut CloudTab> {
        self.tabs.get_mut(self.active)
    }

    /// More than one workspace → the draw layer shows the tab bar.
    pub fn show_tab_bar(&self) -> bool {
        self.tabs.len() > 1
    }

    /// Cycle the active tab by `delta`, wrapping at either end.
    pub fn switch_tab(&mut self, delta: i32) {
        let n = self.tabs.len();
        if n > 1 {
            self.active = (self.active as i32 + delta).rem_euclid(n as i32) as usize;
        }
    }

    /// The currently selected note row of the active tab.
    pub fn selected_note(&self) -> Option<&CloudNoteRow> {
        let t = self.tab()?;
        t.notes.get(t.selected)
    }

    /// Step the selection by `delta` within the active tab, wrapping at
    /// either end like the local browser. Keeps the selection visible
    /// within `viewport_h` (minus the border rows and the tab bar).
    pub fn move_selection(&mut self, delta: i32, viewport_h: u16) {
        let chrome = 2 + self.show_tab_bar() as u16;
        let Some(t) = self.tab_mut() else {
            return;
        };
        if t.notes.is_empty() {
            return;
        }
        t.selected = (t.selected as i32 + delta).rem_euclid(t.notes.len() as i32) as usize;
        let h = viewport_h.saturating_sub(chrome) as usize;
        if t.selected < t.scroll as usize {
            t.scroll = t.selected as u16;
        } else if t.selected >= t.scroll as usize + h.max(1) {
            t.scroll = (t.selected + 1 - h.max(1)) as u16;
        }
    }
}

pub struct Browser {
    pub dir: PathBuf,
    pub entries: Vec<BrowserEntry>,
    pub selected: usize,
    pub scroll: u16,
    /// Fingerprint of `dir` (mtime, size) at the last rebuild. A directory's
    /// own stat changes when an entry is added, removed, or renamed in it, so
    /// comparing this each tick tells us when the listing is stale — without a
    /// file-watcher thread. `None` when the dir is gone or unstatable.
    pub last_meta: Option<(std::time::SystemTime, u64)>,
    /// Active type-ahead find buffer (lf-style). `Some(query)` while find mode
    /// is on: typed characters extend the query and the selection live-jumps to
    /// the first anchored, smartcase match. `None` when not finding.
    pub find: Option<String>,
    /// When `true` (toggled with `A`), the listing shows *everything* — every
    /// file regardless of type, plus hidden and gitignored entries. The default
    /// (`false`) shows only browsable text files and the directories that
    /// contain them.
    pub show_all: bool,
    /// When `true` (toggled with `s`), entries are ordered by last-modified
    /// time, newest first, instead of the default name sort. `../` stays
    /// pinned at the top either way.
    pub sort_by_modified: bool,
    /// `(path, mtime)` of every sub-directory eligible for listing at the last
    /// poll, *before* the "does it hold anything openable" filter. Whether a
    /// sub-directory is shown depends on its contents, and adding a file inside
    /// one doesn't touch the browsed directory's own mtime, so `last_meta`
    /// alone can't see such a change. Refreshed by [`Browser::rebuild`].
    pub child_dirs: Vec<(PathBuf, Option<std::time::SystemTime>)>,
    /// Set while Shift is held down (kitty keyboard protocol only): each row
    /// then shows a dimmed 1-based jump number and typing that number moves
    /// the cursor straight to it, so a distant entry is reachable without a
    /// run of `j`/`k`. The `String` buffers the digits typed so far this
    /// hold, so multi-digit targets (e.g. `16`) accumulate and re-jump live.
    /// `None` when Shift isn't held (labels hidden); legacy terminals that
    /// can't report a lone modifier keypress leave it `None` and the feature
    /// is simply absent.
    pub jump_labels: Option<String>,
}

#[derive(Clone)]
pub struct BrowserEntry {
    pub path: PathBuf,
    pub display: String,
    pub kind: BrowserEntryKind,
    /// Last-modified time of the entry, shown right-aligned in the listing and
    /// used as the sort key when `Browser::sort_by_modified` is on. `None` when
    /// unstatable, or for the synthetic `../` row.
    pub modified: Option<std::time::SystemTime>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserEntryKind {
    ParentDir,
    Dir,
    Markdown,
}

pub struct Search {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub selected: usize,
    /// Pre-built index of paths under the search root. Filtered by `query`
    /// each time the query changes.
    paths: Vec<IndexedPath>,
}

#[derive(Clone)]
struct IndexedPath {
    path: PathBuf,
    display: String,
    display_lower: String,
    is_dir: bool,
}

#[derive(Clone)]
pub struct SearchResult {
    pub path: PathBuf,
    pub display: String,
    pub score: i32,
    pub is_dir: bool,
}

impl App {
    /// Build an app with no cloud connection (tests, plain local usage).
    pub fn new(source: Source, opts: Options) -> Result<Self> {
        Self::with_cloud(source, opts, CloudContext::disconnected())
    }

    /// Build an app wired to a HackMD connection. Both binaries route
    /// through this — `md` via `CloudContext::init`, `hackmd tui` via
    /// `CloudContext::with_client`.
    pub fn with_cloud(source: Source, opts: Options, cloud: CloudContext) -> Result<Self> {
        let root = derive_root(&source);
        let read_state = crate::tui::read_state::ReadState::load(&root);
        let view = match source {
            Source::File(p) => View::Reader(Reader::from_file(&p)?),
            Source::Directory(d) => View::Browser(Browser::scan(&d)?),
            Source::Stdin(text) => View::Reader(Reader::from_string(text)),
        };
        let mut app = Self {
            view,
            root,
            history: Vec::new(),
            forward: Vec::new(),
            opts,
            help_open: false,
            search: None,
            should_quit: false,
            status: String::new(),
            viewport: Rect::new(0, 0, 0, 0),
            slide_area: None,
            slide_bg: Vec::new(),
            statusline_area: Rect::new(0, 0, 0, 0),
            back_button_hit: None,
            cloud_tab_hits: Vec::new(),
            statusline_url_hit: None,
            statusline_badge_hit: None,
            statusline_path_hit: None,
            statusline_path_drag: None,
            count_prefix: None,
            pending_g: None,
            pending_z: None,
            pending_bracket: None,
            toc: None,
            mouse_recent: false,
            last_mouse_col: 0,
            last_mouse_row: 0,
            mouse_enabled: true,
            selection: None,
            pending_click: None,
            pending_anchor: None,
            git_lens: None,
            last_click: None,
            scroll_accum: 0.0,
            last_scroll_at: None,
            image_picker: None,
            images: crate::tui::images::ImageStore::new(),
            edit_raw_area: Rect::default(),
            edit_preview_area: Rect::default(),
            edit_cursor_screen: None,
            read_state,
            cloud: CloudState::new(cloud),
            last_local: None,
            prompt: None,
            yank: None,
            recovery_throttle: None,
            pending_sync: false,
            last_sync: None,
            last_rate_limit: None,
            conflict: None,
            edit_cmd_history: Vec::new(),
            edit_cmd_nav: None,
            lookup: None,
            error: None,
            git_status: crate::tui::git::GitStatus::default(),
            commit: None,
        };
        app.offer_recovery_for_current();
        Ok(app)
    }

    /// Drain every finished cloud operation, applying each to app state.
    /// Called once per event-loop tick beside the file watchers.
    pub fn drain_cloud_msgs(&mut self) {
        loop {
            let Some(msg) = self.cloud.ctx.try_recv() else {
                break;
            };
            self.apply_cloud_msg(msg);
            // A rate-limited op surfaces via the error Display, which carries
            // the "HTTP 429" marker (see `Error::RateLimit`). Stamp the time so
            // the statusline can show a live "rate limited (Ns ago)" badge that
            // outlives the next keypress's `status.clear()`. Checked per message
            // so a 429 isn't masked by a later success in the same drain.
            if self.status.contains("HTTP 429") {
                self.last_rate_limit = Some(std::time::Instant::now());
            }
        }
    }

    /// Apply one finished cloud operation. Pure sync state transition —
    /// no terminal, no network — so it unit-tests without a runtime.
    pub fn apply_cloud_msg(&mut self, msg: CloudMsg) {
        // A dictionary lookup isn't a tracked cloud op (it never bumped the
        // in-flight counter), so it must not decrement it either.
        if !matches!(msg, CloudMsg::Defined { .. }) {
            self.cloud.note_response_received();
        }
        match msg {
            CloudMsg::Lists(Ok(lists)) => {
                self.cloud.lists = Some(lists);
                // If the user is looking at the cloud browser, rebuild it in
                // place, keeping the active tab and the cursor on the same
                // note when they survive.
                if let View::Cloud(c) = &mut self.view {
                    let active_label = c.tab().map(|t| t.label.clone());
                    let sel_id = c.selected_note().map(|n| n.id.clone());
                    let old_scroll = c.tab().map(|t| t.scroll).unwrap_or(0);
                    let mut fresh = CloudBrowser::from_lists(self.cloud.lists.as_ref());
                    if let Some(label) = active_label
                        && let Some(i) = fresh.tabs.iter().position(|t| t.label == label)
                    {
                        fresh.active = i;
                    }
                    if let Some(id) = sel_id
                        && let Some(t) = fresh.tab_mut()
                        && let Some(i) = t.notes.iter().position(|n| n.id == id)
                    {
                        t.selected = i;
                        t.scroll = old_scroll;
                    }
                    *c = fresh;
                }
            }
            CloudMsg::Lists(Err(e)) => self.status = format!("HackMD lists: {e}"),
            CloudMsg::Note { id, intent, result } => match result {
                Ok(FetchedNote::Fresh { note, etag }) => {
                    let note = *note;
                    self.cloud.note_cache.insert(
                        id.clone(),
                        CachedNote {
                            note: note.clone(),
                            etag,
                        },
                    );
                    // A sync fetch three-way merges the upstream content into
                    // the linked local file and is done — it owns its own
                    // local/reader updates.
                    if let crate::tui::cloud::FetchIntent::SyncLocal { path } = &intent {
                        let remote = note.content.clone();
                        self.apply_sync(path.clone(), remote);
                        return;
                    }
                    // A download fetch writes the file and is done — it never
                    // navigates or touches the open reader.
                    if let crate::tui::cloud::FetchIntent::DownloadTo(path) = &intent {
                        self.status = match std::fs::write(path, &note.content) {
                            Ok(()) => format!("Downloaded to {}", path.display()),
                            Err(e) => format!("write {}: {e}", path.display()),
                        };
                        return;
                    }
                    // A navigation armed for this id completes now: history is
                    // pushed only here, so a failed/stale fetch never lands a
                    // phantom entry.
                    if self
                        .cloud
                        .pending_nav
                        .as_ref()
                        .is_some_and(|(nid, _)| *nid == id)
                    {
                        let (_, scroll) = self.cloud.pending_nav.take().expect("checked");
                        let title = note.title.clone();
                        if let Err(e) = self.navigate_to(EntryKind::CloudNote { id, title }, scroll)
                        {
                            self.status = format!("HackMD open: {e}");
                        }
                        return;
                    }
                    // No nav armed — if the note is open right now (history
                    // placeholder or remote-change revalidation), refresh it
                    // in place. Never clobber an in-flight edit.
                    if let View::Reader(r) = &mut self.view
                        && let ReaderOrigin::CloudNote {
                            id: cur_id,
                            title,
                            team_path,
                            publish_link,
                            read_permission,
                            etag: cur_etag,
                        } = &mut r.origin
                        && *cur_id == id
                        && r.edit.is_none()
                    {
                        *title = note.title.clone();
                        *team_path = note.team_path.clone();
                        *publish_link = note.publish_link.clone();
                        *read_permission = note.read_permission;
                        *cur_etag = self.cloud.note_cache.get(&id).and_then(|c| c.etag.clone());
                        if r.raw != note.content {
                            r.raw = note.content;
                            r.rendered = None;
                            r.focus = None;
                            r.hover_link = None;
                            r.hover_checkbox = None;
                            self.status = "Note updated remotely".into();
                        }
                    }
                }
                Ok(FetchedNote::NotModified) => {
                    if matches!(intent, crate::tui::cloud::FetchIntent::SyncLocal { .. }) {
                        self.pending_sync = false;
                    }
                }
                Err(e) => {
                    // A sync fetch that failed just clears its in-flight flag
                    // and retries on the next interval; no navigation to undo.
                    if matches!(intent, crate::tui::cloud::FetchIntent::SyncLocal { .. }) {
                        self.pending_sync = false;
                        self.status = format!("Sync failed: {e}");
                        return;
                    }
                    // A failed fetch abandons any navigation waiting on it —
                    // history was never pushed, so the user simply stays put.
                    if self
                        .cloud
                        .pending_nav
                        .as_ref()
                        .is_some_and(|(nid, _)| *nid == id)
                    {
                        self.cloud.pending_nav = None;
                    }
                    self.status = format!("HackMD note: {e}");
                }
            },
            CloudMsg::Saved {
                id,
                base_file,
                result,
            } => {
                self.cloud.saving.remove(&id);
                match result {
                    Ok(content) => {
                        // The PATCH answers 202 with an empty body; `content`
                        // is what the server accepted. Update the cached copy
                        // and drop its ETag so the next open revalidates.
                        if let Some(cached) = self.cloud.note_cache.get_mut(&id) {
                            cached.note.content = content.clone();
                            cached.etag = None;
                        }
                        // Advance the sync base only once the server has
                        // accepted the content — never optimistically. Until
                        // this lands the base stays at the prior agreed state,
                        // so an interim re-sync against the not-yet-updated
                        // server still prefers our local content rather than
                        // reverting it. Only for saves that carry a linked
                        // file's content (`base_file` is `Some`): a cloud-only
                        // edit of a note that also has a local file must NOT
                        // move the base, or the next sync would push the stale
                        // local file back and revert the cloud edit.
                        if let Some(file) = &base_file {
                            let _ = crate::tui::sync::write_base(&self.root, &id, file, &content);
                        }
                        // Pessimistic dirty: cleared only here, and only when
                        // the buffer still matches what the server accepted —
                        // keystrokes typed after Ctrl-S keep the marker.
                        if let View::Reader(r) = &mut self.view
                            && let ReaderOrigin::CloudNote {
                                id: cur_id,
                                etag: cur_etag,
                                ..
                            } = &mut r.origin
                            && *cur_id == id
                        {
                            *cur_etag = None;
                            if let Some(e) = r.edit.as_mut()
                                && r.raw == content
                            {
                                e.dirty = false;
                            }
                        }
                        self.status = "Saved to HackMD".into();
                    }
                    Err(e) => self.status = format!("HackMD save failed: {e}"),
                }
            }
            CloudMsg::Created { intent, result } => match result {
                Ok(note) => {
                    let id = note.id.clone();
                    let title = note.title.clone();
                    self.cloud.note_cache.insert(
                        id.clone(),
                        CachedNote {
                            note: *note,
                            etag: None,
                        },
                    );
                    // The lists are stale now — refetch in the background so
                    // the browser shows the new note.
                    self.cloud.request_lists();
                    match intent {
                        crate::tui::cloud::CreateIntent::Blank => {
                            self.status = format!("Created \"{title}\"");
                            if let Err(e) = self.navigate_to(EntryKind::CloudNote { id, title }, 0)
                            {
                                self.status = format!("HackMD open: {e}");
                            }
                        }
                        crate::tui::cloud::CreateIntent::PushedFrom(path) => {
                            let name = path
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("file")
                                .to_string();
                            // Stamp the local file with the new note's id and
                            // links so a later `U` updates it instead of
                            // creating a duplicate.
                            let cached = self.cloud.note_cache.get(&id).map(|c| &c.note);
                            let meta = crate::tui::hackmd_meta::HackmdMeta {
                                id: id.clone(),
                                team_path: cached.and_then(|n| n.team_path.clone()),
                                url: crate::tui::hackmd_meta::HackmdMeta::editor_url(&id),
                                publish_link: cached
                                    .map(|n| n.publish_link.clone())
                                    .unwrap_or_default(),
                            };
                            // Rewrite the local file to exactly the pushed
                            // content (which now carries the footer) plus the
                            // link block at the bottom, so local and upstream
                            // match from the start and the file gains the
                            // footer too. Falls back to a plain re-stamp if the
                            // cloud copy isn't cached for some reason.
                            let pushed = self
                                .cloud
                                .note_cache
                                .get(&id)
                                .map(|c| c.note.content.clone());
                            match pushed {
                                Some(body) => {
                                    self.write_synced_local(&path, &meta, &body);
                                    let _ =
                                        crate::tui::sync::write_base(&self.root, &id, &path, &body);
                                }
                                None => self.stamp_local_file(&path, &meta),
                            }
                            // Copy the note's link so the user can paste it
                            // right after publishing. Prefer the publish link;
                            // fall back to the editor URL when not yet known.
                            let link = if meta.publish_link.is_empty() {
                                meta.url.clone()
                            } else {
                                meta.publish_link.clone()
                            };
                            self.status = if link.is_empty() {
                                format!("Pushed {name} → \"{title}\" (linked)")
                            } else {
                                crate::tui::events::copy_to_clipboard(&link);
                                format!("Pushed {name} → \"{title}\" (linked) · link copied")
                            };
                        }
                    }
                }
                Err(e) => self.status = format!("HackMD create failed: {e}"),
            },
            CloudMsg::Deleted { id, title, result } => match result {
                Ok(()) => {
                    self.cloud.note_cache.remove(&id);
                    // Drop the entry from the cached lists so the browser
                    // reflects the delete without a full refetch.
                    if let Some(lists) = self.cloud.lists.as_mut() {
                        lists.notes.retain(|n| n.id != id);
                        for t in &mut lists.teams {
                            t.notes.retain(|n| n.id != id);
                        }
                    }
                    // Don't leave the user staring at a deleted note — drop
                    // them back into the cloud browser. `load` (not
                    // `navigate_to`) so the dead note isn't pushed to history.
                    if matches!(
                        &self.view,
                        View::Reader(r) if matches!(&r.origin, ReaderOrigin::CloudNote { id: rid, .. } if *rid == id)
                    ) {
                        let _ = self.load(EntryKind::CloudList, 0, None);
                    }
                    self.status = format!("Deleted \"{title}\"");
                }
                Err(e) => self.status = format!("HackMD delete failed: {e}"),
            },
            CloudMsg::PermissionSet { id, result } => match result {
                Ok(perm) => {
                    // Empty PATCH response — `perm` is what's now in effect.
                    // The publish link never changes (it always exists), so
                    // pick it up from whichever state already knows it.
                    let mut link = String::new();
                    if let Some(lists) = self.cloud.lists.as_mut() {
                        for n in lists
                            .notes
                            .iter_mut()
                            .chain(lists.teams.iter_mut().flat_map(|t| t.notes.iter_mut()))
                        {
                            if n.id == id {
                                n.read_permission = perm;
                                link = n.publish_link.clone();
                            }
                        }
                    }
                    // Keep an open reader's origin in sync so `P`/`y`/`o`
                    // immediately reflect the new state.
                    if let View::Reader(r) = &mut self.view
                        && let ReaderOrigin::CloudNote {
                            id: cur_id,
                            publish_link,
                            read_permission,
                            ..
                        } = &mut r.origin
                        && *cur_id == id
                    {
                        *read_permission = perm;
                        if link.is_empty() {
                            link = publish_link.clone();
                        }
                    }
                    if let Some(cached) = self.cloud.note_cache.get_mut(&id) {
                        cached.note.read_permission = perm;
                        cached.etag = None;
                        if link.is_empty() {
                            link = cached.note.publish_link.clone();
                        }
                    }
                    let published = matches!(perm, crate::types::NotePermissionRole::Guest);
                    self.status = if published {
                        if link.is_empty() {
                            "Published".into()
                        } else {
                            format!("Published: {link}")
                        }
                    } else {
                        "Unpublished".into()
                    };
                }
                Err(e) => self.status = format!("HackMD publish failed: {e}"),
            },
            CloudMsg::Defined { word, result } => {
                // Drop a stale reply: the popover may have closed or moved to a
                // different word while this lookup was in flight.
                if let Some(l) = self.lookup.as_mut()
                    && l.word == word
                {
                    l.status = match result {
                        Some(text) => LookupStatus::Ready(text),
                        None => LookupStatus::NotFound,
                    };
                    l.scroll = 0;
                }
            }
        }
    }

    /// Scroll the open definition popover by `delta` rows. The renderer clamps
    /// the upper bound against the entry's length, so over-scrolling just rests
    /// at the bottom.
    pub fn lookup_scroll(&mut self, delta: i32) {
        if let Some(l) = self.lookup.as_mut() {
            l.scroll = (l.scroll as i32 + delta).max(0) as u16;
        }
    }

    /// Probe the terminal for graphics-protocol support. Must be called after
    /// the alternate screen is active — the probe writes a query to stdout
    /// and reads the reply from stdin, and any unrecognized escape bytes
    /// would otherwise be left on the user's main screen.
    pub fn init_image_picker(&mut self) {
        self.image_picker = Picker::from_query_stdio().ok();
    }

    /// Fold any images that finished loading on their worker threads into ready
    /// protocols. Cheap no-op when nothing landed; called once per event tick.
    /// Skipped entirely when the terminal has no image protocol.
    pub fn drain_images(&mut self) {
        if let Some(picker) = self.image_picker.as_mut() {
            self.images.drain(picker);
        }
    }

    pub fn record_current(&self) -> HistoryEntry {
        match &self.view {
            View::Reader(r) => HistoryEntry {
                kind: match &r.origin {
                    ReaderOrigin::File(p) => EntryKind::File(p.clone()),
                    ReaderOrigin::Stdin => EntryKind::Stdin(r.raw.clone()),
                    ReaderOrigin::CloudNote { id, title, .. } => EntryKind::CloudNote {
                        id: id.clone(),
                        title: title.clone(),
                    },
                },
                scroll: r.scroll,
                selected: None,
            },
            View::Browser(b) => HistoryEntry {
                kind: EntryKind::Directory(b.dir.clone()),
                scroll: b.scroll,
                selected: Some(b.selected),
            },
            View::Cloud(c) => HistoryEntry {
                kind: EntryKind::CloudList,
                scroll: c.tab().map(|t| t.scroll).unwrap_or(0),
                selected: c.tab().map(|t| t.selected),
            },
        }
    }

    pub fn navigate_to(&mut self, kind: EntryKind, scroll: u16) -> Result<()> {
        self.forward.clear();
        let prev = self.record_current();
        self.history.push(prev);
        self.load(kind, scroll, None)
    }

    pub fn go_back(&mut self) -> Result<()> {
        if let Some(prev) = self.history.pop() {
            let cur = self.record_current();
            self.forward.push(cur);
            self.load(prev.kind, prev.scroll, prev.selected)?;
        }
        Ok(())
    }

    pub fn go_forward(&mut self) -> Result<()> {
        if let Some(next) = self.forward.pop() {
            let cur = self.record_current();
            self.history.push(cur);
            self.load(next.kind, next.scroll, next.selected)?;
        }
        Ok(())
    }

    fn load(&mut self, kind: EntryKind, scroll: u16, selected: Option<usize>) -> Result<()> {
        // Remember the latest local view so the cloud toggle can return to it.
        if matches!(
            kind,
            EntryKind::File(_) | EntryKind::Directory(_) | EntryKind::Stdin(_)
        ) {
            self.last_local = Some(kind.clone());
        }
        self.view = match kind {
            EntryKind::File(p) => {
                let mut r = Reader::from_file(&p)?;
                r.scroll = scroll;
                View::Reader(r)
            }
            EntryKind::Directory(d) => {
                let mut b = Browser::scan(&d)?;
                b.scroll = scroll;
                if let Some(sel) = selected {
                    let max = b.entries.len().saturating_sub(1);
                    b.selected = sel.min(max);
                }
                View::Browser(b)
            }
            EntryKind::Stdin(text) => {
                let mut r = Reader::from_string(text);
                r.scroll = scroll;
                View::Reader(r)
            }
            EntryKind::CloudList => {
                let mut c = CloudBrowser::from_lists(self.cloud.lists.as_ref());
                if let Some(t) = c.tab_mut() {
                    t.scroll = scroll;
                    if let Some(sel) = selected {
                        t.selected = sel.min(t.notes.len().saturating_sub(1));
                    }
                }
                if self.cloud.lists.is_none() {
                    self.cloud.request_lists();
                }
                View::Cloud(c)
            }
            EntryKind::CloudNote { id, title } => {
                if let Some(cached) = self.cloud.note_cache.get(&id) {
                    let mut r = Reader::from_cloud(&cached.note, cached.etag.clone());
                    r.scroll = scroll;
                    // Freshness check in the background: with an ETag a 304
                    // costs nothing; without one a full fetch refreshes the
                    // open reader in place (skipped while editing).
                    let intent = match cached.etag.clone() {
                        Some(etag) => crate::tui::cloud::FetchIntent::Revalidate { etag },
                        None => crate::tui::cloud::FetchIntent::OpenReader { scroll },
                    };
                    self.cloud.request_note(id, intent);
                    View::Reader(r)
                } else {
                    // History navigation to a note that's no longer cached:
                    // show a placeholder and refill it in place when the
                    // fetch lands (`apply_cloud_msg` matches on the open id).
                    self.cloud.request_note(
                        id.clone(),
                        crate::tui::cloud::FetchIntent::OpenReader { scroll },
                    );
                    View::Reader(Reader::cloud_placeholder(id, title))
                }
            }
        };
        self.offer_recovery_for_current();
        Ok(())
    }

    /// Open a HackMD note from the cloud browser. Cache hit navigates
    /// immediately; a miss arms `pending_nav` and stays put — history is
    /// pushed only when the fetch completes, so failures leave it clean.
    pub fn open_cloud_note(&mut self, id: String, title: String) {
        if self.cloud.note_cache.contains_key(&id) {
            if let Err(e) = self.navigate_to(EntryKind::CloudNote { id, title }, 0) {
                self.status = format!("HackMD open: {e}");
            }
            return;
        }
        if self.cloud.request_note(
            id.clone(),
            crate::tui::cloud::FetchIntent::OpenReader { scroll: 0 },
        ) {
            self.cloud.pending_nav = Some((id, 0));
        } else {
            self.status = NO_TOKEN_HINT.into();
        }
    }

    /// Open a just-created cloud note straight into the split editor (the
    /// `hackmd new` flow). The create response is seeded into the note
    /// cache so no fetch round-trip is needed; the cursor lands on the
    /// blank body line below the title heading.
    pub fn open_created_note(&mut self, note: crate::types::SingleNote) {
        let id = note.id.clone();
        let title = note.title.clone();
        self.cloud.note_cache.insert(
            id.clone(),
            crate::tui::cloud::CachedNote { note, etag: None },
        );
        if let Err(e) = self.navigate_to(EntryKind::CloudNote { id, title }, 0) {
            self.status = format!("HackMD open: {e}");
            return;
        }
        self.enter_edit();
        if let View::Reader(r) = &mut self.view
            && let Some(edit) = &mut r.edit
        {
            // First blank line after the heading (template shape from
            // `hackmd new`); byte 0 when the content has no blank line.
            edit.cursor = r.raw.find("\n\n").map_or(0, |i| i + 2).min(r.raw.len());
        }
    }

    /// `H` (browsers) / `gh` (anywhere): flip between the local file world
    /// and the HackMD cloud browser. Toggling out of cloud returns to the
    /// most recent local view.
    pub fn toggle_cloud_mode(&mut self) {
        let in_cloud = match &self.view {
            View::Cloud(_) => true,
            View::Reader(r) => matches!(r.origin, ReaderOrigin::CloudNote { .. }),
            View::Browser(_) => false,
        };
        let result = if in_cloud {
            let kind = self
                .last_local
                .clone()
                .unwrap_or_else(|| EntryKind::Directory(self.root.clone()));
            self.navigate_to(kind, 0)
        } else {
            if !self.cloud.is_connected() {
                self.status = NO_TOKEN_HINT.into();
                return;
            }
            self.navigate_to(EntryKind::CloudList, 0)
        };
        if let Err(e) = result {
            self.status = format!("{e}");
        }
    }

    /// `R` in the cloud browser: refetch the note lists.
    pub fn refresh_cloud_lists(&mut self) {
        if self.cloud.request_lists() {
            self.status = "Refreshing…".into();
        } else {
            self.status = NO_TOKEN_HINT.into();
        }
    }

    /// The cloud note the context-sensitive actions apply to: the selected
    /// browser row, or the open cloud reader. `None` in local contexts.
    pub fn cloud_target(&self) -> Option<CloudTarget> {
        match &self.view {
            View::Cloud(c) => c.selected_note().map(|n| CloudTarget {
                id: n.id.clone(),
                title: n.title.clone(),
                team_path: n.team_path.clone(),
                published: n.published,
                publish_link: String::new(),
            }),
            View::Reader(r) => match &r.origin {
                ReaderOrigin::CloudNote {
                    id,
                    title,
                    team_path,
                    publish_link,
                    read_permission,
                    ..
                } => Some(CloudTarget {
                    id: id.clone(),
                    title: title.clone(),
                    team_path: team_path.clone(),
                    published: matches!(read_permission, crate::types::NotePermissionRole::Guest),
                    publish_link: publish_link.clone(),
                }),
                _ => None,
            },
            View::Browser(_) => None,
        }
    }

    /// HackMD status badge for the doc open in the reader, or `None` when the
    /// doc isn't linked to HackMD. Covers both cloud notes (live permission
    /// from the origin) and local files carrying a `<!-- hackmd … -->` block
    /// (permission looked up in the note cache when available). A cloud sync
    /// in flight takes precedence and shows as `Syncing`.
    pub fn hackmd_badge(&self) -> Option<HackmdBadge> {
        let View::Reader(r) = &self.view else {
            return None;
        };
        // Resolve (live read permission, is-team, publish link) for the doc.
        let (perm, team, link): (Option<crate::types::NotePermissionRole>, bool, String) =
            match &r.origin {
                ReaderOrigin::CloudNote {
                    read_permission,
                    team_path,
                    publish_link,
                    ..
                } => (
                    Some(*read_permission),
                    team_path.is_some(),
                    publish_link.clone(),
                ),
                ReaderOrigin::File(_) => {
                    let meta = crate::tui::hackmd_meta::parse(&r.raw)?;
                    let cached = self.cloud.note_cache.get(&meta.id).map(|c| &c.note);
                    let team = meta.team_path.is_some()
                        || cached.map(|n| n.team_path.is_some()).unwrap_or(false);
                    (cached.map(|n| n.read_permission), team, meta.publish_link)
                }
                ReaderOrigin::Stdin => return None,
            };
        if self.cloud.pending > 0 {
            return Some(HackmdBadge {
                kind: HackmdBadgeKind::Syncing,
                label: "Syncing".into(),
                link,
            });
        }
        let (kind, label) = match perm {
            None => (HackmdBadgeKind::Unknown, "On HackMD".to_string()),
            Some(crate::types::NotePermissionRole::Guest) => {
                (HackmdBadgeKind::Public, "Public".to_string())
            }
            Some(crate::types::NotePermissionRole::SignedIn) => {
                (HackmdBadgeKind::Private, "Private (signed-in)".to_string())
            }
            Some(crate::types::NotePermissionRole::Owner) => (
                HackmdBadgeKind::Private,
                if team {
                    "Private (team)".to_string()
                } else {
                    "Private (only me)".to_string()
                },
            ),
        };
        Some(HackmdBadge { kind, label, link })
    }

    /// `n` in the cloud browser: prompt for a new note's title.
    pub fn prompt_new_note(&mut self) {
        if !self.cloud.is_connected() {
            self.status = NO_TOKEN_HINT.into();
            return;
        }
        self.prompt = Some(Prompt {
            title: " New HackMD note — title ".into(),
            input: String::new(),
            kind: PromptKind::NewNoteTitle,
        });
    }

    /// `U` on a local file: publish it to HackMD. If the file already carries
    /// a managed `<!-- hackmd … -->` block (it was published before), update
    /// the linked note in place; otherwise fall through to the title prompt
    /// that creates a fresh note.
    pub fn push_local(&mut self, path: PathBuf) {
        if !self.cloud.is_connected() {
            self.status = NO_TOKEN_HINT.into();
            return;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                self.status = format!("read {}: {e}", path.display());
                return;
            }
        };
        match crate::tui::hackmd_meta::parse(&content) {
            Some(mut meta) => {
                // A team note whose `team:` line was hand-deleted parses as a
                // personal note, so a push would route to the personal endpoint
                // (and fail or mis-create). Recover the team path from the
                // cache when we have it.
                if meta.team_path.is_none()
                    && let Some(c) = self.cloud.note_cache.get(&meta.id)
                    && c.note.team_path.is_some()
                {
                    meta.team_path = c.note.team_path.clone();
                }
                // Already linked → PATCH the existing note with the document
                // body (our managed block is local-only, so strip it before
                // pushing).
                let clean = crate::tui::hackmd_meta::strip(&content);
                if self.cloud.saving.contains(&meta.id) {
                    self.status = "Update already in flight…".into();
                    return;
                }
                if self.cloud.request_save(
                    meta.id.clone(),
                    meta.team_path.clone(),
                    clean,
                    Some(path.clone()),
                ) {
                    // Optimistically refresh the `synced:` stamp; the base
                    // advances when the `Saved` confirmation lands.
                    self.stamp_local_file(&path, &meta);
                    self.status = format!("⟳ updating \"{}\"…", meta.id);
                } else {
                    self.status = NO_TOKEN_HINT.into();
                }
            }
            None => {
                // A managed block is present but didn't parse (e.g. the `id:`
                // line was hand-deleted): treating this as a first publish
                // would silently create a duplicate note. Warn and bail so the
                // user can fix the block instead.
                if crate::tui::hackmd_meta::has_block(&content) {
                    self.status =
                        "Corrupted hackmd block (no id) — fix or remove it before publishing"
                            .into();
                    return;
                }
                // First publish: infer the title from the document's first H1
                // (`# Title`). Only fall back to the title prompt when the file
                // has no H1 to take it from.
                let clean = crate::tui::hackmd_meta::strip(&content);
                match first_h1(&clean) {
                    Some(title) => self.create_pushed_note(path, title),
                    None => self.prompt_push(path),
                }
            }
        }
    }

    /// Create a new HackMD note from a local file's content (block stripped),
    /// linking the file on success via [`CreateIntent::PushedFrom`].
    pub fn create_pushed_note(&mut self, path: PathBuf, title: String) {
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                self.status = format!("read {}: {e}", path.display());
                return;
            }
        };
        // First publish gets the attribution footer (once), mirroring
        // `hackmd new`. After this it's plain content the user can edit.
        let clean = crate::tui::hackmd_meta::strip(&content);
        let body = crate::tui::hackmd_meta::ensure_footer(&clean);
        // Default a freshly-published note to "anyone with link" rather than
        // HackMD's owner-only default: guest read so the link works without a
        // login, everyone may comment, but write stays owner-only.
        let opts = crate::types::CreateNoteOptions {
            title: Some(title),
            content: Some(body),
            read_permission: Some(crate::types::NotePermissionRole::Guest),
            write_permission: Some(crate::types::NotePermissionRole::Owner),
            comment_permission: Some(crate::types::CommentPermissionType::Everyone),
            ..Default::default()
        };
        if !self.cloud.request_create(
            None,
            opts,
            crate::tui::cloud::CreateIntent::PushedFrom(path),
        ) {
            self.status = NO_TOKEN_HINT.into();
        }
    }

    /// Write (or refresh) the managed HackMD block at the top of `path`. Best-
    /// effort: a write failure only sets the statusline, since the cloud side
    /// already succeeded.
    pub fn stamp_local_file(&mut self, path: &Path, meta: &crate::tui::hackmd_meta::HackmdMeta) {
        let Ok(content) = std::fs::read_to_string(path) else {
            return;
        };
        let synced = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let stamped = crate::tui::hackmd_meta::upsert(&content, meta, &synced);
        if stamped == content {
            return;
        }
        if let Err(e) = std::fs::write(path, &stamped) {
            self.status = format!("stamp {}: {e}", path.display());
            return;
        }
        // If this file is the one open in the reader, keep the buffer and
        // fingerprint in sync so the watcher doesn't flag our own write.
        if let View::Reader(r) = &mut self.view {
            if matches!(&r.origin, ReaderOrigin::File(p) if p == path) {
                r.raw = stamped;
                r.rendered = None;
                r.last_meta = file_meta(path);
            }
        }
    }

    /// Background-driven check, called every event-loop tick. When a
    /// HackMD-linked local file is freshly opened in the reader (and not being
    /// edited), offer a one-shot prompt to pull upstream edits. There is
    /// deliberately NO periodic re-poll: at 400 API calls/month on a Free
    /// workspace, a 15s background poll drained a whole day's quota in ~3
    /// minutes, so the fetch happens only if the user confirms. The file read +
    /// parse runs once per newly-seen file, not every tick.
    pub fn maybe_sync(&mut self) {
        if self.pending_sync
            || self.conflict.is_some()
            || self.prompt.is_some()
            || !self.cloud.is_connected()
        {
            return;
        }
        // Only a plain (non-edit) reader over a real file can sync.
        let path = match &self.view {
            View::Reader(r) if r.edit.is_none() => match &r.origin {
                ReaderOrigin::File(p) => p.clone(),
                _ => return,
            },
            _ => return,
        };
        // Act once per newly-seen file. `last_sync` marks the path already
        // handled; with no periodic re-poll it never becomes "due" again while
        // the same file stays open.
        if matches!(&self.last_sync, Some((p, _)) if *p == path) {
            return;
        }
        self.last_sync = Some((path.clone(), std::time::Instant::now()));
        // Read + parse to see whether it's actually a HackMD-linked note. Only
        // a linked file has anything to fetch; an unlinked file is marked
        // handled above and silently skipped (no prompt, no call).
        let Ok(content) = std::fs::read_to_string(&path) else {
            return;
        };
        if crate::tui::hackmd_meta::parse(&content).is_none() {
            return;
        }
        self.prompt = Some(Prompt {
            title: "Fetch latest from HackMD?".into(),
            input: String::new(),
            kind: PromptKind::ConfirmFetchUpdate { path },
        });
    }

    /// Kick off a three-way sync for the linked file at `path`: fetch upstream,
    /// then merge when it lands (see [`Self::apply_sync`]). No-op for an
    /// unlinked file or when disconnected. Records the path as handled so the
    /// open-file check doesn't re-offer it.
    pub fn sync_local_file(&mut self, path: PathBuf) {
        if self.pending_sync || !self.cloud.is_connected() {
            return;
        }
        // Record the attempt up front so an unlinked (or unreadable) file backs
        // off for the interval instead of being re-read every tick.
        self.last_sync = Some((path.clone(), std::time::Instant::now()));
        let Ok(content) = std::fs::read_to_string(&path) else {
            return;
        };
        let Some(meta) = crate::tui::hackmd_meta::parse(&content) else {
            // Not linked yet — nothing to sync.
            return;
        };
        if self
            .cloud
            .request_note(meta.id, crate::tui::cloud::FetchIntent::SyncLocal { path })
        {
            self.pending_sync = true;
        }
    }

    /// Apply a landed sync fetch: three-way merge the upstream `remote` content
    /// against the linked local file and its cached base. A clean merge writes
    /// both sides and updates the base; a conflict opens the resolver. Dropped
    /// (retried later) if the file is mid-edit and dirty, so in-progress
    /// keystrokes are never clobbered.
    pub fn apply_sync(&mut self, path: PathBuf, remote: String) {
        self.pending_sync = false;
        // Never overwrite an unsaved edit buffer — defer to the next trigger.
        let dirty_edit = matches!(
            &self.view,
            View::Reader(r) if matches!(&r.origin, ReaderOrigin::File(p) if *p == path)
                && r.edit.as_ref().map(|e| e.dirty).unwrap_or(false)
        );
        if dirty_edit {
            return;
        }
        let Ok(local_raw) = std::fs::read_to_string(&path) else {
            return;
        };
        let Some(meta) = crate::tui::hackmd_meta::parse(&local_raw) else {
            return;
        };
        let id = meta.id.clone();
        // Normalise line endings before the merge: HackMD may return `\r\n`
        // while the local file is `\n`, and `merge3` compares exact strings —
        // a mismatch would surface every line as a spurious whole-file
        // conflict on the first sync.
        // `normalize_for_merge` reconciles both CRLF *and* the trailing-newline
        // mismatch between HackMD content (no trailing `\n`) and `strip` (which
        // forces one): without it an otherwise-identical first sync differs by
        // that lone newline and, with no cached base, blows up into a spurious
        // whole-file conflict.
        let local_clean =
            crate::tui::sync::normalize_for_merge(&crate::tui::hackmd_meta::strip(&local_raw));
        let remote = crate::tui::sync::normalize_for_merge(&remote);
        // Missing base → treat as empty so nothing is silently dropped: equal
        // sides still merge clean, differing sides surface as a conflict.
        let base = crate::tui::sync::normalize_for_merge(
            &crate::tui::sync::read_base(&self.root, &id, &path).unwrap_or_default(),
        );
        match crate::tui::sync::merge3(&base, &local_clean, &remote) {
            crate::tui::sync::MergeOutcome::Clean(merged) => {
                self.write_synced_local(&path, &meta, &merged);
                if merged != remote {
                    // Push the merged result; the base advances when the
                    // `Saved` confirmation lands (not optimistically), so a
                    // re-sync before the server updates can't revert us. If the
                    // push is skipped (a save for this id already in flight),
                    // the local file holds the merge but the server doesn't yet
                    // — the next sync retries against the unchanged server, so
                    // nothing is lost; just don't claim we synced.
                    if self.cloud.request_save(
                        id.clone(),
                        meta.team_path.clone(),
                        merged.clone(),
                        Some(path.clone()),
                    ) {
                        self.status = "Synced with HackMD".into();
                    } else {
                        self.status = "Merged locally — upstream push deferred".into();
                    }
                } else {
                    // Already equal to upstream — server == merged is the new
                    // agreed base, with no push to wait on.
                    let _ = crate::tui::sync::write_base(&self.root, &id, &path, &merged);
                    self.status = "Synced with HackMD".into();
                }
                if let Some(c) = self.cloud.note_cache.get_mut(&id) {
                    c.note.content = merged;
                    c.etag = None;
                }
            }
            crate::tui::sync::MergeOutcome::Conflict { segments } => {
                self.open_conflict(path, id, meta, segments);
            }
        }
    }

    /// Write the synced `body` (already block-free) to the local file with a
    /// refreshed managed block, updating the open reader buffer if it's the
    /// same file and not being edited.
    fn write_synced_local(
        &mut self,
        path: &Path,
        meta: &crate::tui::hackmd_meta::HackmdMeta,
        body: &str,
    ) {
        let synced = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let stamped = crate::tui::hackmd_meta::upsert(body, meta, &synced);
        if let Err(e) = std::fs::write(path, &stamped) {
            self.status = format!("write {}: {e}", path.display());
            return;
        }
        if let View::Reader(r) = &mut self.view {
            if matches!(&r.origin, ReaderOrigin::File(p) if p == path) {
                // A *dirty* edit is never overwritten (the caller already bails
                // in that case); a clean edit buffer adopts the merged content
                // with the cursor clamped back into range.
                let dirty = r.edit.as_ref().map(|e| e.dirty).unwrap_or(false);
                if !dirty && r.raw != stamped {
                    r.raw = stamped;
                    r.rendered = None;
                    r.focus = None;
                    r.hover_link = None;
                    r.hover_checkbox = None;
                    if let Some(e) = r.edit.as_mut() {
                        e.cursor = floor_char_boundary(&r.raw, e.cursor.min(r.raw.len()));
                    }
                }
                r.last_meta = file_meta(path);
            }
        }
    }

    /// Build a [`ConflictState`] from merge segments and switch to the resolver.
    fn open_conflict(
        &mut self,
        path: PathBuf,
        id: String,
        meta: crate::tui::hackmd_meta::HackmdMeta,
        segments: Vec<crate::tui::sync::Segment>,
    ) {
        let items = segments
            .into_iter()
            .map(|s| match s {
                crate::tui::sync::Segment::Stable(t) => ConflictItem::Stable(t),
                crate::tui::sync::Segment::Conflict { local, remote } => ConflictItem::Conflict {
                    local,
                    remote,
                    choice: ConflictChoice::Unresolved,
                },
            })
            .collect();
        let count = {
            let st = ConflictState {
                path,
                id,
                meta,
                items,
                selected: 0,
                scroll: 0,
            };
            let c = st.conflict_count();
            self.conflict = Some(st);
            c
        };
        self.status = format!("Sync conflict — {count} hunk(s) to resolve");
    }

    /// Finish the active conflict resolution: assemble the chosen content,
    /// write it locally and push it upstream, update the base, and close the
    /// resolver. No-op while any hunk is still unresolved.
    pub fn resolve_conflict(&mut self) {
        let Some(st) = self.conflict.as_ref() else {
            return;
        };
        let Some(merged) = st.assemble() else {
            self.status = format!("{} hunk(s) still unresolved", st.unresolved_count());
            return;
        };
        let path = st.path.clone();
        let id = st.id.clone();
        let meta = st.meta.clone();
        self.conflict = None;
        self.write_synced_local(&path, &meta, &merged);
        // Push the resolution; the base advances on the `Saved` confirmation,
        // not here, so the periodic re-sync (which still sees the pre-push
        // server) keeps our resolved local content rather than reverting it. A
        // skipped push (save already in flight) leaves the resolved content on
        // disk for the next sync to retry — don't claim it reached the server.
        let pushed = self.cloud.request_save(
            id.clone(),
            meta.team_path.clone(),
            merged.clone(),
            Some(path.clone()),
        );
        if let Some(c) = self.cloud.note_cache.get_mut(&id) {
            c.note.content = merged;
            c.etag = None;
        }
        self.status = if pushed {
            "Conflict resolved — syncing…".into()
        } else {
            "Conflict resolved locally — upstream push deferred".into()
        };
    }

    /// `U` on a local file: prompt for the pushed note's title (defaults to
    /// the file stem).
    pub fn prompt_push(&mut self, path: PathBuf) {
        if !self.cloud.is_connected() {
            self.status = NO_TOKEN_HINT.into();
            return;
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file")
            .to_string();
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("untitled")
            .to_string();
        self.prompt = Some(Prompt {
            title: format!(" Push {name} to HackMD — title "),
            input: stem,
            kind: PromptKind::PushTitle(path),
        });
    }

    /// `n` in the local browser: prompt for a new file's name. The file is
    /// created under the current directory and opened in the editor. A name
    /// ending in `/` creates a directory and browses into it instead.
    pub fn prompt_new_file(&mut self) {
        let View::Browser(b) = &self.view else {
            return;
        };
        let dir = b.dir.clone();
        self.prompt = Some(Prompt {
            title: " New file (end the name with / for a folder) ".into(),
            input: String::new(),
            kind: PromptKind::NewFile { dir },
        });
    }

    /// `m` in the local browser: prompt for where to move the selected entry.
    /// The destination is read relative to the browsed directory; a trailing
    /// `/` (or a name that already is a directory) means "into there, keep the
    /// name", anything else is the full new path, so one keystroke can move
    /// and rename at once.
    pub fn prompt_move(&mut self) {
        let View::Browser(b) = &self.view else {
            return;
        };
        let dir = b.dir.clone();
        let Some(entry) = b
            .entries
            .get(b.selected)
            .filter(|e| !matches!(e.kind, BrowserEntryKind::ParentDir))
        else {
            return;
        };
        let from = entry.path.clone();
        let name = from
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        self.prompt = Some(Prompt {
            title: format!(" Move {name} to "),
            input: String::new(),
            kind: PromptKind::MoveEntry { from, dir },
        });
    }

    /// For an open new-file or move prompt, the grey ghost that completes the
    /// segment currently being typed to an existing sub-directory, so Tab
    /// nests inside it. Returns the suffix to append (including the trailing
    /// `/`), or `None` when nothing existing extends what's typed. Only
    /// directories are offered, since the point is to reach a folder that
    /// already exists.
    pub fn path_completion(&self) -> Option<String> {
        let (base, input) = match &self.prompt {
            Some(Prompt {
                kind: PromptKind::NewFile { dir } | PromptKind::MoveEntry { dir, .. },
                input,
                ..
            }) => (dir, input.as_str()),
            _ => return None,
        };
        if input.is_empty() || input.starts_with('/') || input.contains('\\') {
            return None;
        }
        // Complete only the final segment; everything before the last `/` must
        // already resolve to a real directory for there to be anything to list.
        let (parent, seg) = match input.rsplit_once('/') {
            Some((p, s)) => (p, s),
            None => ("", input),
        };
        let search = if parent.is_empty() {
            base.clone()
        } else {
            base.join(parent)
        };
        let mut names: Vec<String> = std::fs::read_dir(&search)
            .ok()?
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.') && n.len() > seg.len() && n.starts_with(seg))
            .collect();
        names.sort();
        names
            .into_iter()
            .next()
            .map(|n| format!("{}/", &n[seg.len()..]))
    }

    /// `r` / F2 in the local browser: prompt to rename the selected entry.
    /// Pre-fills the current name so the user edits rather than retypes.
    pub fn prompt_rename(&mut self) {
        let View::Browser(b) = &self.view else {
            return;
        };
        let Some(entry) = b
            .entries
            .get(b.selected)
            .filter(|e| !matches!(e.kind, BrowserEntryKind::ParentDir))
        else {
            return;
        };
        let from = entry.path.clone();
        let name = from
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        self.prompt = Some(Prompt {
            title: format!(" Rename {name} — new name "),
            input: name,
            kind: PromptKind::RenameFile { from },
        });
    }

    /// HackMD sync bases to carry across a pending rename/move of `path`, as
    /// `(note id, current cache path, path relative to `path`)`.
    ///
    /// [`crate::tui::sync::base_path`] hashes the file's *canonical* path, so
    /// the old cache name can only be computed while the file is still where
    /// it is: this must run before the rename, and its result is handed to
    /// [`Self::after_move`] afterwards. A directory contributes one entry per
    /// linked markdown file beneath it.
    fn linked_bases(&self, path: &Path) -> Vec<(String, PathBuf, PathBuf)> {
        let linked = |file: &Path| -> Option<(String, PathBuf)> {
            let content = std::fs::read_to_string(file).ok()?;
            let meta = crate::tui::hackmd_meta::parse(&content)?;
            let base = crate::tui::sync::base_path(&self.root, &meta.id, file);
            Some((meta.id, base))
        };
        let is_markdown = |p: &Path| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
        };
        if path.is_dir() {
            // Hidden and gitignored files count: a linked note that happens to
            // be ignored still has a base worth keeping.
            ignore::WalkBuilder::new(path)
                .hidden(false)
                .git_ignore(false)
                .git_exclude(false)
                .git_global(false)
                .require_git(false)
                .build()
                .flatten()
                .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
                .filter(|e| is_markdown(e.path()))
                .filter_map(|e| {
                    let rel = e.path().strip_prefix(path).ok()?.to_path_buf();
                    let (id, base) = linked(e.path())?;
                    Some((id, base, rel))
                })
                .collect()
        } else {
            linked(path)
                .into_iter()
                .map(|(id, base)| (id, base, PathBuf::new()))
                .collect()
        }
    }

    /// Bookkeeping that has to follow an entry when it is renamed or moved:
    /// the read-state key (else the file reappears as `[unread]`), the HackMD
    /// sync base snapshots collected by [`Self::linked_bases`] (else the next
    /// sync has no common ancestor and explodes into a whole-file conflict),
    /// and any history entry still pointing at the old path.
    fn after_move(&mut self, from: &Path, to: &Path, bases: Vec<(String, PathBuf, PathBuf)>) {
        self.read_state.move_path(from, to);
        for (id, old_base, rel) in bases {
            let file = if rel.as_os_str().is_empty() {
                to.to_path_buf()
            } else {
                to.join(&rel)
            };
            let _ = crate::tui::sync::rehome_base(&self.root, &id, &old_base, &file);
        }
        let remap = |p: &Path| -> Option<PathBuf> {
            if p == from {
                Some(to.to_path_buf())
            } else {
                p.strip_prefix(from).ok().map(|rest| to.join(rest))
            }
        };
        for e in self.history.iter_mut().chain(self.forward.iter_mut()) {
            match &mut e.kind {
                EntryKind::File(p) | EntryKind::Directory(p) => {
                    if let Some(next) = remap(p) {
                        *p = next;
                    }
                }
                _ => {}
            }
        }
    }

    /// `S` on a cloud note: prompt for the local filename (defaults to the
    /// slugified title). The file lands under `app.root`.
    pub fn prompt_download(&mut self) {
        let Some(t) = self.cloud_target() else {
            return;
        };
        self.prompt = Some(Prompt {
            title: format!(" Download to {}/ ", self.root.display()),
            input: format!("{}.md", slugify(&t.title)),
            kind: PromptKind::DownloadFilename { id: t.id },
        });
    }

    /// `D` on a cloud note: arm the delete confirmation.
    pub fn prompt_delete(&mut self) {
        let Some(t) = self.cloud_target() else {
            return;
        };
        if !self.cloud.is_connected() {
            self.status = NO_TOKEN_HINT.into();
            return;
        }
        self.prompt = Some(Prompt {
            title: format!(" Delete \"{}\" from HackMD? ", t.title),
            input: String::new(),
            kind: PromptKind::ConfirmDelete {
                id: t.id,
                title: t.title,
                team_path: t.team_path,
            },
        });
    }

    /// `P` on a cloud note: flip `readPermission` between `guest`
    /// (published) and `owner` (private). The v1 API has no publish
    /// endpoint; this is how publish works.
    pub fn publish_toggle(&mut self) {
        let Some(t) = self.cloud_target() else {
            return;
        };
        let perm = if t.published {
            crate::types::NotePermissionRole::Owner
        } else {
            crate::types::NotePermissionRole::Guest
        };
        if self
            .cloud
            .request_set_read_permission(t.id, t.team_path, perm)
        {
            self.status = if t.published {
                "⟳ unpublishing…".into()
            } else {
                "⟳ publishing…".into()
            };
        } else {
            self.status = NO_TOKEN_HINT.into();
        }
    }

    /// Execute a committed prompt.
    pub fn commit_prompt(&mut self, p: Prompt) {
        match p.kind {
            PromptKind::NewNoteTitle => {
                let title = p.input.trim().to_string();
                if title.is_empty() {
                    self.status = "Cancelled — empty title".into();
                    return;
                }
                let opts = crate::types::CreateNoteOptions {
                    title: Some(title.clone()),
                    content: Some(format!("# {title}\n")),
                    ..Default::default()
                };
                if !self
                    .cloud
                    .request_create(None, opts, crate::tui::cloud::CreateIntent::Blank)
                {
                    self.status = NO_TOKEN_HINT.into();
                }
            }
            PromptKind::PushTitle(path) => {
                let title = p.input.trim().to_string();
                if title.is_empty() {
                    self.status = "Cancelled — empty title".into();
                    return;
                }
                self.create_pushed_note(path, title);
            }
            PromptKind::DownloadFilename { id } => {
                let name = p.input.trim();
                if name.is_empty() {
                    self.status = "Cancelled — empty filename".into();
                    return;
                }
                let path = self.root.join(name);
                if path.exists() {
                    self.status = format!("Refusing to overwrite {}", path.display());
                    return;
                }
                if let Some(cached) = self.cloud.note_cache.get(&id) {
                    match std::fs::write(&path, &cached.note.content) {
                        Ok(()) => self.status = format!("Downloaded to {}", path.display()),
                        Err(e) => self.status = format!("write {}: {e}", path.display()),
                    }
                } else if !self
                    .cloud
                    .request_note(id, crate::tui::cloud::FetchIntent::DownloadTo(path))
                {
                    self.status = NO_TOKEN_HINT.into();
                }
            }
            PromptKind::ConfirmDelete {
                id,
                title,
                team_path,
            } => {
                if !self.cloud.request_delete(id, title, team_path) {
                    self.status = NO_TOKEN_HINT.into();
                }
            }
            PromptKind::NewFile { dir } => {
                let name = p.input.trim();
                if name.is_empty() {
                    self.status = "Cancelled — empty name".into();
                    return;
                }
                // `foo/bar.md` is allowed and creates `foo/` on the way; a
                // trailing `/` creates a directory instead of a file. A
                // backslash, a leading slash, or any `..` is refused so the
                // entry can't escape the browsed directory.
                if name.contains('\\') {
                    self.status = "Use / to nest, not \\".into();
                    return;
                }
                if name.starts_with('/') {
                    self.status = "Name can't be an absolute path".into();
                    return;
                }
                let make_dir = name.ends_with('/');
                let name = name.trim_end_matches('/');
                if name.is_empty() {
                    self.status = "Cancelled — empty name".into();
                    return;
                }
                let rel = std::path::Path::new(name);
                if rel.components().any(|c| {
                    matches!(
                        c,
                        std::path::Component::ParentDir
                            | std::path::Component::RootDir
                            | std::path::Component::Prefix(_)
                    )
                }) {
                    self.status = "Name can't contain `..`".into();
                    return;
                }
                let path = dir.join(rel);
                if path.exists() {
                    self.status = format!("Already exists: {}", path.display());
                    return;
                }
                if make_dir {
                    if let Err(e) = std::fs::create_dir_all(&path) {
                        self.status = format!("create {}: {e}", path.display());
                        return;
                    }
                    // Browse into the new folder, mirroring how creating a file
                    // drops you into the editor: `n` always lands you inside
                    // whatever it just made.
                    if let Err(e) = self.navigate_to(EntryKind::Directory(path.clone()), 0) {
                        self.status = format!("open {}: {e}", path.display());
                        return;
                    }
                    self.status = format!("New folder {}", path.display());
                    return;
                }
                // Create any intermediate directories the name introduces.
                if let Some(parent) = path.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        self.status = format!("create {}: {e}", parent.display());
                        return;
                    }
                }
                // Create the (empty) file, then open it in the editor so the
                // user can start writing immediately.
                if let Err(e) = std::fs::write(&path, "") {
                    self.status = format!("create {}: {e}", path.display());
                    return;
                }
                if let Err(e) = self.navigate_to(EntryKind::File(path.clone()), 0) {
                    self.status = format!("open {}: {e}", path.display());
                    return;
                }
                self.enter_edit();
                self.status = format!("New file {}", path.display());
            }
            // Resolved directly in `handle_prompt_key` (s/d/Esc / r/d), never
            // via the generic Enter→commit path.
            PromptKind::ConfirmDiscardEdit { .. } => {}
            PromptKind::RecoverEdit { .. } => {}
            PromptKind::ConfirmFetchUpdate { path } => {
                // User accepted the one API call — pull upstream and merge.
                self.status = "⟳ fetching from HackMD…".into();
                self.sync_local_file(path);
            }
            PromptKind::CommitFile { root, file } => {
                let message = p.input.trim();
                if message.is_empty() {
                    self.status = "Commit skipped (empty message)".into();
                    return;
                }
                match crate::tui::git::commit(&root, std::slice::from_ref(&file), message) {
                    Ok(summary) => {
                        self.refresh_git_status();
                        self.status = summary;
                    }
                    Err(e) => self.status = e,
                }
            }
            PromptKind::RenameFile { from } => {
                let name = p.input.trim();
                if name.is_empty() {
                    self.status = "Cancelled — empty name".into();
                    return;
                }
                if name.contains('/') || name.contains('\\') {
                    self.status = "Name can't contain a path separator".into();
                    return;
                }
                let Some(parent) = from.parent() else {
                    self.status = "Can't rename: no parent directory".into();
                    return;
                };
                let to = parent.join(name);
                if to == from {
                    self.status = "Name unchanged".into();
                    return;
                }
                if to.exists() {
                    self.status = format!("Already exists: {}", to.display());
                    return;
                }
                // Collected before the rename: the sync cache name hashes the
                // file's canonical path, which stops resolving once it moves.
                let bases = self.linked_bases(&from);
                if let Err(e) = std::fs::rename(&from, &to) {
                    self.status = format!("rename: {e}");
                    return;
                }
                self.after_move(&from, &to, bases);
                // Rebuild the listing and re-select the renamed entry so the
                // cursor follows it rather than snapping to the top.
                if let View::Browser(b) = &mut self.view {
                    let _ = b.rebuild();
                    if let Some(i) = b.entries.iter().position(|e| e.path == to) {
                        b.selected = i;
                    }
                }
                self.status = format!("Renamed to {name}");
            }
            PromptKind::MoveEntry { from, dir } => {
                let dest = p.input.trim();
                if dest.is_empty() {
                    self.status = "Cancelled — empty destination".into();
                    return;
                }
                if dest.contains('\\') {
                    self.status = "Use / to nest, not \\".into();
                    return;
                }
                let Some(name) = from.file_name() else {
                    self.status = "Can't move this entry".into();
                    return;
                };
                // Relative destinations resolve against the browsed directory;
                // `../` is allowed because the browser itself isn't fenced to
                // the search root.
                let rel = std::path::Path::new(dest);
                let mut to = if rel.is_absolute() {
                    rel.to_path_buf()
                } else {
                    dir.join(rel)
                };
                // `mv` semantics: an explicit trailing `/`, or a destination
                // that already is a directory, means "into there, keep the
                // name". Anything else is the full new path, so a move can
                // rename in the same keystroke.
                if dest.ends_with('/') || to.is_dir() {
                    to = to.join(name);
                }
                let to = normalize_path(&to);
                let from_norm = normalize_path(&from);
                if to == from_norm {
                    self.status = "Already there".into();
                    return;
                }
                if from.is_dir() && to.starts_with(&from_norm) {
                    self.status = "Can't move a folder into itself".into();
                    return;
                }
                if to.exists() {
                    self.status = format!("Already exists: {}", to.display());
                    return;
                }
                if let Some(parent) = to.parent()
                    && let Err(e) = std::fs::create_dir_all(parent)
                {
                    self.status = format!("create {}: {e}", parent.display());
                    return;
                }
                // Captured before the move, for the same reason as in rename.
                let bases = self.linked_bases(&from);
                if let Err(e) = std::fs::rename(&from, &to) {
                    // A cross-filesystem move fails here rather than silently
                    // degrading to copy-then-delete, which is not atomic and
                    // would need recursive handling for directories.
                    self.status = format!("move: {e}");
                    return;
                }
                self.after_move(&from, &to, bases);
                // The entry usually left this directory, so only re-select it
                // when it's still listed here; otherwise the rebuild keeps the
                // cursor at a sane neighbouring row.
                if let View::Browser(b) = &mut self.view {
                    let _ = b.rebuild();
                    if let Some(i) = b.entries.iter().position(|e| e.path == to) {
                        b.selected = i;
                    }
                }
                self.status = format!("Moved to {}", crate::tui::ui::display_path(&to, &self.root));
            }
        }
    }

    pub fn open_search(&mut self) {
        self.search = Some(Search::build(&self.root));
    }

    pub fn close_search(&mut self) {
        self.search = None;
    }

    /// Resolve and follow a link target. Returns `Ok(true)` if the action was
    /// handled internally (navigation), `Ok(false)` if it was external.
    pub fn follow(&mut self, target: LinkTarget) -> Result<bool> {
        match target {
            LinkTarget::Url(url) => {
                let _ = open::that_detached(&url);
                self.status = format!("Opened {}", url);
                Ok(false)
            }
            LinkTarget::Anchor(slug) => {
                self.scroll_to_anchor(&slug);
                Ok(true)
            }
            LinkTarget::LocalFile(p) => {
                let resolved = match resolve_local_path(&p).or_else(|| vault_lookup(&self.root, &p))
                {
                    Some(r) => r,
                    None => {
                        self.status = format!("Not found: {}", p.display());
                        return Ok(false);
                    }
                };
                if resolved.is_dir() {
                    self.navigate_to(EntryKind::Directory(resolved), 0)?;
                    Ok(true)
                } else if is_text_file(&resolved) {
                    self.navigate_to(EntryKind::File(resolved), 0)?;
                    Ok(true)
                } else {
                    let _ = open::that_detached(&resolved);
                    self.status = format!("Opened externally: {}", resolved.display());
                    Ok(false)
                }
            }
            LinkTarget::FileAnchor(p, slug) => {
                let resolved = match resolve_local_path(&p).or_else(|| vault_lookup(&self.root, &p))
                {
                    Some(r) => r,
                    None => {
                        self.status = format!("Not found: {}", p.display());
                        return Ok(false);
                    }
                };
                if !is_markdown_file(&resolved) {
                    let _ = open::that_detached(&resolved);
                    self.status = format!("Opened externally: {}", resolved.display());
                    return Ok(false);
                }
                self.navigate_to(EntryKind::File(resolved), 0)?;
                self.scroll_to_anchor(&slug);
                Ok(true)
            }
        }
    }

    /// The link a heading in the current document should be shared as:
    /// `/abs/path/to/file.md#slug` for a file, the publish URL plus `#slug`
    /// for a published cloud note, and a bare `#slug` when there's nothing to
    /// hang the anchor off (stdin, an unpublished note).
    pub fn heading_link(&self, anchor: &str) -> String {
        let View::Reader(r) = &self.view else {
            return format!("#{anchor}");
        };
        match &r.origin {
            ReaderOrigin::File(p) => {
                let abs = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
                format!("{}#{}", abs.display(), anchor)
            }
            ReaderOrigin::CloudNote { publish_link, .. } if !publish_link.is_empty() => {
                format!("{publish_link}#{anchor}")
            }
            ReaderOrigin::CloudNote { .. } | ReaderOrigin::Stdin => format!("#{anchor}"),
        }
    }

    /// Scroll the reader so `slug`'s heading sits at the top. A document that
    /// hasn't been laid out yet (the usual case right after navigating to
    /// another file) has no line for the slug, so the jump is parked in
    /// `pending_anchor` and replayed by [`App::ensure_rendered`].
    fn scroll_to_anchor(&mut self, slug: &str) {
        let viewport_h = self.viewport.height.max(1) as usize;
        let View::Reader(r) = &mut self.view else {
            return;
        };
        let Some(rendered) = &r.rendered else {
            self.pending_anchor = Some(slug.to_string());
            return;
        };
        match rendered.link_map.anchors.get(slug) {
            Some(&line) => {
                // Clamp like `scroll_by` does: an anchor in the last screenful
                // scrolls only as far as keeps the page full.
                let page_max = rendered.lines.len().saturating_sub(viewport_h) as u16;
                r.scroll = (line as u16).min(page_max);
                self.status = format!("→ #{}", slug);
            }
            None => self.status = format!("Anchor not found: #{}", slug),
        }
    }

    /// Cheap external-change check, called every event-loop tick. Stats the
    /// open file; if (mtime, size) differs from the recorded fingerprint, the
    /// content is re-read and the cached render is dropped. Returns `true`
    /// when the on-screen content actually changed (mtime touched but byte-
    /// identical content does not count). No-op for stdin or non-Reader views.
    pub fn poll_external_change(&mut self) -> bool {
        let View::Reader(r) = &mut self.view else {
            return false;
        };
        // Don't clobber an in-flight edit. The user can resolve any conflict
        // explicitly by saving (overwrites disk) or discarding via Esc-Esc
        // (reloads from disk).
        if r.edit.is_some() {
            return false;
        }
        let path = match &r.origin {
            ReaderOrigin::File(p) => p.clone(),
            // Stdin has no source to watch; cloud freshness is handled by
            // ETag revalidation on open, not a per-tick poll.
            ReaderOrigin::Stdin | ReaderOrigin::CloudNote { .. } => return false,
        };
        let Some(new_meta) = file_meta(&path) else {
            return false;
        };
        if r.last_meta.as_ref() == Some(&new_meta) {
            return false;
        }
        // Fingerprint moved — re-read and decide whether content actually
        // changed. A transient read failure (editor mid-rename, etc.) is
        // ignored; we'll retry on the next tick.
        let Ok(new_raw) = std::fs::read_to_string(&path) else {
            return false;
        };
        r.last_meta = Some(new_meta);
        if new_raw == r.raw {
            return false;
        }
        r.raw = new_raw;
        r.rendered = None;
        r.hover_link = None;
        r.hover_checkbox = None;
        r.hover_jsonl = None;
        r.focus = None;
        if let Some(ds) = &mut r.doc_search {
            ds.matches.clear();
            ds.current = 0;
        }
        self.status = "File reloaded".into();
        true
    }

    /// Cheap external-change check for the file browser, called every event-loop
    /// tick. Stats the listed directory; if its (mtime, size) fingerprint moved
    /// — an entry was added, removed, or renamed — the listing is rebuilt in
    /// place (preserving the highlighted path when it survives). Returns `true`
    /// when a rebuild happened. No-op outside the Browser view.
    ///
    /// The listed sub-directories are stat'd too, because whether one is shown
    /// at all depends on its contents (see `dir_has_listable`): dropping the
    /// first markdown file into an otherwise unlistable folder doesn't touch
    /// the browsed directory's own mtime, so without this the folder would
    /// stay hidden until something else moved.
    ///
    /// Unread badges don't need this — `draw_browser` recomputes them from disk
    /// every frame, so they're already live; only the entry list itself goes
    /// stale.
    pub fn poll_browser_change(&mut self) -> bool {
        let View::Browser(b) = &mut self.view else {
            return false;
        };
        let new_meta = file_meta(&b.dir);
        let new_children = child_dir_meta(&b.dir, b.show_all);
        if b.last_meta == new_meta && b.child_dirs == new_children {
            return false;
        }
        // rebuild() refreshes last_meta, so a transient stat failure just retries
        // next tick rather than looping.
        b.rebuild().is_ok()
    }

    /// Flip the `[ ]`/`[x]` task marker at `idx` and persist to the source file.
    /// No-op for stdin sources. Drops the cached render so the next draw
    /// reflects the new state.
    pub fn toggle_checkbox(&mut self, idx: usize) -> Result<()> {
        let View::Reader(r) = &mut self.view else {
            return Ok(());
        };
        let Some(rendered) = r.rendered.as_ref() else {
            return Ok(());
        };
        let Some(cb) = rendered.checkbox_map.items.get(idx) else {
            return Ok(());
        };
        let offset = cb.source_offset;
        let was_checked = cb.checked;
        if offset + 3 > r.raw.len() {
            return Ok(());
        }
        let replacement = if was_checked { "[ ]" } else { "[x]" };
        let mut new_raw = String::with_capacity(r.raw.len());
        new_raw.push_str(&r.raw[..offset]);
        new_raw.push_str(replacement);
        new_raw.push_str(&r.raw[offset + 3..]);
        r.raw = new_raw;
        match r.origin.clone() {
            ReaderOrigin::File(path) => {
                std::fs::write(&path, &r.raw)
                    .map_err(|e| anyhow!("write {}: {}", path.display(), e))?;
                // Refresh fingerprint so the watcher doesn't see our own write
                // as an external change and trigger a redundant reload.
                r.last_meta = file_meta(&path);
                self.status = if was_checked {
                    "Unchecked".into()
                } else {
                    "Checked".into()
                };
            }
            ReaderOrigin::CloudNote { id, team_path, .. } => {
                // Optimistic: the buffer already flipped; PATCH in the
                // background. On failure the error hits the statusline and
                // the local flip stays (the next revalidation reconciles).
                // Cloud-only edit → never advance a linked file's base.
                if self.cloud.request_save(id, team_path, r.raw.clone(), None) {
                    self.status = if was_checked {
                        "Unchecked (syncing…)".into()
                    } else {
                        "Checked (syncing…)".into()
                    };
                } else if self.cloud.is_connected() {
                    self.status = "Save already in flight — toggle kept locally".into();
                } else {
                    self.status = NO_TOKEN_HINT.into();
                }
            }
            ReaderOrigin::Stdin => {
                self.status = "Toggled (in-memory; stdin not persisted)".into();
            }
        }
        r.rendered = None;
        r.hover_checkbox = None;
        r.hover_link = None;
        Ok(())
    }

    /// Open the in-document text search prompt. No-op outside Reader.
    pub fn open_doc_search(&mut self) {
        if let View::Reader(r) = &mut self.view {
            r.doc_search = Some(DocSearch {
                query: String::new(),
                matches: Vec::new(),
                current: 0,
                editing: true,
            });
            self.status.clear();
        }
    }

    pub fn close_doc_search(&mut self) {
        if let View::Reader(r) = &mut self.view {
            r.doc_search = None;
        }
    }

    /// Recompute matches from the rendered document for the current query.
    pub fn doc_search_refresh(&mut self) {
        let View::Reader(r) = &mut self.view else {
            return;
        };
        let Some(rendered) = r.rendered.as_ref() else {
            return;
        };
        let Some(s) = r.doc_search.as_mut() else {
            return;
        };
        s.matches = find_doc_matches(&rendered.lines, &s.query);
        if s.matches.is_empty() {
            s.current = 0;
        } else if s.current >= s.matches.len() {
            s.current = 0;
        }
    }

    /// Confirm the current query (close prompt, jump to first match).
    pub fn doc_search_commit(&mut self) {
        let View::Reader(r) = &mut self.view else {
            return;
        };
        let Some(s) = r.doc_search.as_mut() else {
            return;
        };
        s.editing = false;
        if s.matches.is_empty() {
            self.status = "No matches".into();
            return;
        }
        s.current = 0;
        self.center_on_doc_match();
    }

    /// Step to the next/previous match (after commit).
    pub fn doc_search_step(&mut self, forward: bool) {
        let View::Reader(r) = &mut self.view else {
            return;
        };
        let Some(s) = r.doc_search.as_mut() else {
            return;
        };
        if s.matches.is_empty() {
            return;
        }
        let n = s.matches.len();
        s.current = if forward {
            (s.current + 1) % n
        } else {
            (s.current + n - 1) % n
        };
        self.center_on_doc_match();
    }

    fn center_on_doc_match(&mut self) {
        let h = self.viewport.height as usize;
        let View::Reader(r) = &mut self.view else {
            return;
        };
        let Some(s) = r.doc_search.as_ref() else {
            return;
        };
        let Some(m) = s.matches.get(s.current) else {
            return;
        };
        let new = m.line.saturating_sub(h / 2);
        let total = r.rendered.as_ref().map(|x| x.lines.len()).unwrap_or(0);
        let max_scroll = total.saturating_sub(h);
        r.scroll = new.min(max_scroll) as u16;
    }

    /// Toggle the git lens overlay. On the way in, runs `git diff HEAD --`
    /// for the current reader file (combined staged + unstaged); on the way
    /// out, just drops the cached diff. No-op for stdin or non-Reader views.
    /// Disabled in edit mode (the diff would race with un-saved buffer).
    pub fn toggle_git_lens(&mut self) {
        if let View::Reader(r) = &self.view {
            if r.edit.is_some() {
                self.status = "Git lens unavailable while editing".into();
                return;
            }
        }
        if self.git_lens.is_some() {
            self.git_lens = None;
            self.status.clear();
            return;
        }
        let path = match &self.view {
            View::Reader(r) => match &r.origin {
                ReaderOrigin::File(p) => p.clone(),
                ReaderOrigin::Stdin | ReaderOrigin::CloudNote { .. } => {
                    self.status = "Git lens needs a local file".into();
                    return;
                }
            },
            _ => return,
        };
        match run_git_diff(&path) {
            Ok(diff) => {
                let rows = parse_unified_diff(&diff);
                let clean = rows
                    .iter()
                    .all(|r| matches!(r.kind, DiffRowKind::Header | DiffRowKind::Info));
                let rows = if clean {
                    vec![DiffRow {
                        kind: DiffRowKind::Info,
                        text: "✓ No uncommitted changes".to_string(),
                    }]
                } else {
                    rows
                };
                let _ = clean;
                self.git_lens = Some(GitLensState { rows, scroll: 0 });
            }
            Err(e) => {
                self.status = format!("git diff: {}", e);
            }
        }
    }

    /// The filesystem path the git status/commit context is anchored to: the
    /// open file in the Reader, or the current directory in the Browser. `None`
    /// for stdin, cloud notes, or the cloud browser.
    pub fn git_anchor(&self) -> Option<PathBuf> {
        match &self.view {
            View::Reader(r) => match &r.origin {
                ReaderOrigin::File(p) => Some(p.clone()),
                ReaderOrigin::Stdin | ReaderOrigin::CloudNote { .. } => None,
            },
            View::Browser(b) => Some(b.dir.clone()),
            View::Cloud(_) => None,
        }
    }

    /// Refresh the cached git status against the current view's anchor. Called
    /// eagerly on navigation, save and commit.
    pub fn refresh_git_status(&mut self) {
        let anchor = self.git_anchor();
        self.git_status.refresh(anchor.as_deref());
    }

    /// TTL-gated git-status refresh for the tick loop. `changed` forces a
    /// rebuild (something on disk moved); otherwise it only rebuilds when the
    /// cache has gone stale.
    pub fn poll_git_status(&mut self, changed: bool) {
        let anchor = self.git_anchor();
        self.git_status.poll(anchor.as_deref(), changed);
    }

    /// Open the git commit screen (`gc`). Lists every uncommitted file in the
    /// repo, pre-checking the ones in the current selection's scope: the open
    /// file (Reader), the selected file, or every uncommitted file under the
    /// selected directory (Browser). Refuses to open outside a repo or when
    /// there's nothing to commit.
    pub fn open_commit(&mut self) {
        // Only from a local file view or the file browser.
        let anchor = match self.git_anchor() {
            Some(a) => a,
            None => {
                self.status = "Commit needs a local file or folder".into();
                return;
            }
        };
        self.git_status.refresh(Some(&anchor));
        let Some(root) = self.git_status.root.clone() else {
            self.status = "Not a git repository".into();
            return;
        };
        let uncommitted = self.git_status.sorted_files();
        if uncommitted.is_empty() {
            self.status = "Nothing to commit — working tree clean".into();
            return;
        }

        // Which paths should start checked, based on the current selection.
        let scope: CommitScope = match &self.view {
            View::Reader(r) => match &r.origin {
                ReaderOrigin::File(p) => CommitScope::File(p.clone()),
                _ => CommitScope::None,
            },
            View::Browser(b) => match b.entries.get(b.selected) {
                Some(e) if matches!(e.kind, BrowserEntryKind::Markdown) => {
                    CommitScope::File(e.path.clone())
                }
                Some(e) if matches!(e.kind, BrowserEntryKind::Dir) => {
                    CommitScope::Dir(e.path.clone())
                }
                // `../` selected or empty dir: scope to the current directory.
                _ => CommitScope::Dir(b.dir.clone()),
            },
            View::Cloud(_) => CommitScope::None,
        };

        let stats = crate::tui::git::numstat(&root, &uncommitted);
        let files: Vec<CommitFile> = uncommitted
            .iter()
            .map(|p| {
                let rel = p
                    .strip_prefix(&root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .into_owned();
                let (added, removed) = stats.get(p).copied().unwrap_or((0, 0));
                CommitFile {
                    include: scope.contains(p),
                    path: p.clone(),
                    rel,
                    added,
                    removed,
                }
            })
            .collect();

        // Land the cursor on the first checked file so the eye starts on the
        // scoped selection rather than the top of the repo-wide list.
        let selected = files.iter().position(|f| f.include).unwrap_or(0);
        self.commit = Some(CommitState {
            root,
            files,
            selected,
            message: String::new(),
            focus: CommitFocus::Message,
        });
    }

    /// Execute the commit from the open commit screen. Requires a non-empty
    /// message and at least one checked file; surfaces the outcome (short git
    /// summary, or the error) on the statusline and closes the screen on
    /// success.
    pub fn do_commit(&mut self) {
        let Some(st) = self.commit.as_ref() else {
            return;
        };
        let paths = st.included_paths();
        if paths.is_empty() {
            self.status = "Select at least one file to commit (space)".into();
            return;
        }
        let message = st.message.trim().to_string();
        if message.is_empty() {
            self.status = "Enter a commit message".into();
            return;
        }
        let root = st.root.clone();
        match crate::tui::git::commit(&root, &paths, &message) {
            Ok(summary) => {
                self.commit = None;
                self.refresh_git_status();
                self.status = summary;
            }
            Err(e) => {
                self.status = e;
            }
        }
    }

    /// Open the table-of-contents overlay (`t` in the Reader). Pre-selects
    /// the heading the viewport currently sits in so Enter is a no-op-ish
    /// "stay here" and j/k move relative to the reading position.
    pub fn open_toc(&mut self) {
        let View::Reader(r) = &self.view else {
            return;
        };
        if r.edit.is_some() {
            return;
        }
        let Some(rendered) = r.rendered.as_ref() else {
            return;
        };
        if rendered.headings.is_empty() {
            self.status = "No headings in this document".into();
            return;
        }
        let cur = r.scroll as usize;
        // Last heading at or above the current scroll position; the first
        // heading when the viewport is above all of them.
        let selected = rendered
            .headings
            .iter()
            .rposition(|h| h.line <= cur)
            .unwrap_or(0);
        self.toc = Some(TocState { selected });
    }

    /// Move the TOC selection by `delta` (clamped to the heading list).
    pub fn toc_move(&mut self, delta: i32) {
        let total = match &self.view {
            View::Reader(r) => r.rendered.as_ref().map(|rd| rd.headings.len()).unwrap_or(0),
            _ => 0,
        };
        if total == 0 {
            return;
        }
        if let Some(t) = self.toc.as_mut() {
            t.selected = (t.selected as i32 + delta).clamp(0, total as i32 - 1) as usize;
        }
    }

    /// Jump `n` headings forward (positive) or back (negative) from the
    /// current scroll position — the `]]` / `[[` motions. Clamps at the
    /// first/last heading; no-op when the document has none.
    pub fn jump_heading(&mut self, n: i32) {
        let h = self.viewport.height as usize;
        let View::Reader(r) = &mut self.view else {
            return;
        };
        let Some(rendered) = r.rendered.as_ref() else {
            return;
        };
        if rendered.headings.is_empty() || n == 0 {
            return;
        }
        let cur = r.scroll as usize;
        let lines: Vec<usize> = rendered.headings.iter().map(|x| x.line).collect();
        let target = if n > 0 {
            let after: Vec<usize> = lines.iter().copied().filter(|&l| l > cur).collect();
            match after.get(n as usize - 1) {
                Some(&l) => l,
                None => match after.last() {
                    Some(&l) => l,
                    None => return,
                },
            }
        } else {
            let before: Vec<usize> = lines.iter().copied().filter(|&l| l < cur).collect();
            let k = (-n) as usize;
            if before.is_empty() {
                return;
            }
            before[before.len().saturating_sub(k)]
        };
        let total = rendered.lines.len();
        let max_scroll = total.saturating_sub(h);
        r.scroll = target.min(max_scroll) as u16;
    }

    /// Scroll the git lens overlay by `delta` rows (clamped). No-op if the
    /// overlay isn't open.
    pub fn git_lens_scroll(&mut self, delta: i32) {
        let h = self.viewport.height as i32;
        if let Some(g) = self.git_lens.as_mut() {
            let total = g.rows.len() as i32;
            let max = (total - h).max(0);
            let new = (g.scroll as i32 + delta).clamp(0, max) as u16;
            g.scroll = new;
        }
    }

    /// Enter in-house edit mode (insert by default). Cursor starts at byte 0;
    /// nothing dirty; command line closed. No-op for stdin (we'd have nothing
    /// to write to).
    pub fn enter_edit(&mut self) {
        if let View::Reader(r) = &mut self.view {
            if matches!(r.origin, ReaderOrigin::Stdin) {
                self.status = "Cannot edit: source is stdin".into();
                return;
            }
            r.edit = Some(EditState {
                cursor: 0,
                dirty: false,
                command: None,
                preview_full: false,
                undo: Vec::new(),
                redo: Vec::new(),
                mode: EditMode::Split,
                last_drawn_cursor: None,
                selection: None,
                anchor_complete: None,
            });
            r.rendered = None;
            r.scroll = 0;
            r.preview_scroll = 0;
            self.status.clear();
        }
    }

    /// `A` in the reader: enter the editor with the cursor at the end of the
    /// buffer (append to the document).
    pub fn enter_edit_append(&mut self) {
        self.enter_edit();
        if let View::Reader(r) = &mut self.view {
            if let Some(e) = r.edit.as_mut() {
                e.cursor = floor_char_boundary(&r.raw, r.raw.len());
                r.rendered = None;
            }
        }
    }

    /// `O` in the reader: enter the editor with a fresh blank line opened at
    /// the very top, cursor on it (open-above).
    pub fn enter_edit_open_above(&mut self) {
        self.enter_edit();
        if !matches!(&self.view, View::Reader(r) if r.edit.is_some()) {
            return; // stdin / no editor
        }
        self.edit_insert("\n");
        if let View::Reader(r) = &mut self.view {
            if let Some(e) = r.edit.as_mut() {
                e.cursor = 0;
                r.rendered = None;
            }
        }
    }

    /// True while the open reader is in edit mode (buffer dirty or not).
    pub fn is_editing(&self) -> bool {
        matches!(&self.view, View::Reader(r) if r.edit.is_some())
    }

    /// True while the open reader has an unsaved (dirty) edit buffer.
    pub fn editing_dirty(&self) -> bool {
        matches!(&self.view, View::Reader(r) if r.edit.as_ref().map(|e| e.dirty).unwrap_or(false))
    }

    /// Mirror the dirty edit buffer to its crash-recovery file so an unsaved
    /// document survives Ctrl-C / a crash / a kill. Throttled to avoid a disk
    /// write per keystroke; `force` (used right before quitting) bypasses the
    /// throttle so the very last edits are captured. No-op when not editing a
    /// dirty on-disk file.
    pub fn autosave_recovery(&mut self, force: bool) {
        let now = std::time::Instant::now();
        let hash = {
            if !self.editing_dirty() {
                return;
            }
            let View::Reader(r) = &self.view else {
                return;
            };
            let ReaderOrigin::File(path) = &r.origin else {
                return;
            };
            let hash = crate::tui::recovery::content_hash(&r.raw);
            let due = force
                || match self.recovery_throttle {
                    Some((t, h)) => {
                        h != hash && now.duration_since(t) >= std::time::Duration::from_millis(700)
                    }
                    None => true,
                };
            if !due {
                return;
            }
            let cursor = r.edit.as_ref().map(|e| e.cursor).unwrap_or(0);
            crate::tui::recovery::save(path, &r.raw, cursor);
            hash
        };
        self.recovery_throttle = Some((now, hash));
    }

    /// Discard a pending recovery the user declined at the recover prompt.
    pub fn discard_recovery_for_current(&mut self) {
        self.clear_recovery();
    }

    /// Drop the recovery mirror for the open file (after a save or an explicit
    /// discard) and reset the autosave throttle.
    fn clear_recovery(&mut self) {
        if let View::Reader(r) = &self.view {
            if let ReaderOrigin::File(p) = &r.origin {
                crate::tui::recovery::clear(p);
            }
        }
        self.recovery_throttle = None;
    }

    /// If the open reader is an on-disk file with a pending recovery (a
    /// mirrored buffer that differs from the file), raise the recover/discard
    /// prompt. Called whenever a file is opened.
    fn offer_recovery_for_current(&mut self) {
        if self.prompt.is_some() {
            return;
        }
        let (path, disk) = match &self.view {
            View::Reader(r) => match &r.origin {
                ReaderOrigin::File(p) => (p.clone(), r.raw.clone()),
                _ => return,
            },
            _ => return,
        };
        if let Some(rec) = crate::tui::recovery::pending(&path, &disk) {
            self.prompt = Some(Prompt {
                title: " Recover unsaved edits? ".into(),
                input: String::new(),
                kind: PromptKind::RecoverEdit {
                    content: rec.content,
                    cursor: rec.cursor,
                },
            });
        }
    }

    /// Apply a chosen recovery: replace the buffer with the recovered text and
    /// drop into the editor at the saved cursor, marked dirty so it's clearly
    /// unsaved (and re-mirrored to the recovery file).
    pub fn apply_recovery(&mut self, content: String, cursor: usize) {
        self.enter_edit();
        if let View::Reader(r) = &mut self.view {
            if let Some(e) = r.edit.as_mut() {
                r.raw = content;
                e.cursor = floor_char_boundary(&r.raw, cursor.min(r.raw.len()));
                e.dirty = true;
                r.rendered = None;
                self.status = "Recovered unsaved edits — save to keep them".into();
            }
        }
        self.recovery_throttle = None;
    }

    /// Begin leaving the editor. A clean buffer (or no edit at all) performs
    /// `after` straight away; a dirty buffer raises the save/discard/cancel
    /// prompt so a pending edit is never thrown away without asking.
    pub fn request_leave_edit(&mut self, after: AfterEdit) {
        if self.editing_dirty() {
            self.prompt = Some(Prompt {
                title: " Unsaved changes ".into(),
                input: String::new(),
                kind: PromptKind::ConfirmDiscardEdit { after },
            });
            return;
        }
        self.apply_after_edit(after, false);
    }

    /// Resolve a [`PromptKind::ConfirmDiscardEdit`] prompt. `save` writes the
    /// buffer first (keeping the editor open if the write fails so nothing is
    /// lost); otherwise the edit is discarded.
    pub fn resolve_discard_edit(&mut self, after: AfterEdit, save: bool) -> Result<()> {
        if save {
            self.save_edit()?;
            self.apply_after_edit(after, false);
        } else {
            self.apply_after_edit(after, true);
        }
        Ok(())
    }

    /// Carry out the post-resolution action: leave edit mode (reloading from
    /// the origin when `discard`) or quit the app.
    fn apply_after_edit(&mut self, after: AfterEdit, discard: bool) {
        match after {
            AfterEdit::Exit => {
                if discard {
                    self.exit_edit_discard();
                } else {
                    self.exit_edit();
                }
            }
            AfterEdit::Quit => {
                if discard {
                    // Quitting and throwing the edit away: drop the recovery
                    // mirror and mark the buffer clean so the event loop's
                    // final autosave doesn't write it back out.
                    self.clear_recovery();
                    if let View::Reader(r) = &mut self.view {
                        if let Some(e) = r.edit.as_mut() {
                            e.dirty = false;
                        }
                    }
                }
                self.should_quit = true;
            }
        }
    }

    /// Discard buffer changes and exit edit mode. Reloads the file from disk
    /// (or the cloud cache) to drop any unsaved edits, then returns the
    /// reader to view mode.
    pub fn exit_edit_discard(&mut self) {
        let cached_raw = match &self.view {
            View::Reader(r) => match &r.origin {
                ReaderOrigin::CloudNote { id, .. } => self
                    .cloud
                    .note_cache
                    .get(id)
                    .map(|c| c.note.content.clone()),
                _ => None,
            },
            _ => None,
        };
        if let View::Reader(r) = &mut self.view {
            if r.edit.is_none() {
                return;
            }
            match r.origin.clone() {
                ReaderOrigin::File(path) => {
                    if let Ok(disk) = std::fs::read_to_string(&path) {
                        r.raw = disk;
                        r.last_meta = file_meta(&path);
                    }
                    // Explicit discard → drop the crash-recovery mirror.
                    crate::tui::recovery::clear(&path);
                }
                ReaderOrigin::CloudNote { .. } => {
                    if let Some(raw) = cached_raw {
                        r.raw = raw;
                    }
                }
                ReaderOrigin::Stdin => {}
            }
            r.edit = None;
            r.rendered = None;
            self.recovery_throttle = None;
            self.status = "Edit discarded".into();
        }
    }

    /// Exit edit mode without modifying the buffer. Used after a successful
    /// save and on Esc from a clean buffer (no edits to discard).
    pub fn exit_edit(&mut self) {
        if let View::Reader(r) = &mut self.view {
            r.edit = None;
            r.rendered = None;
        }
        self.maybe_prompt_commit();
    }

    /// After leaving the editor with the file's on-disk state preserved (a
    /// clean exit or a save-then-exit, never a discard), offer to commit it
    /// right away if it now carries uncommitted changes. This turns editing a
    /// file and committing it into one flow, so a finished edit (or a freshly
    /// created file) can be landed without a separate trip to the commit
    /// screen. A one-line message popup: Enter commits just this file, Esc
    /// skips and leaves it uncommitted. No-op outside a git repo, for
    /// non-file readers, when the file is already committed, or when another
    /// overlay is already up.
    fn maybe_prompt_commit(&mut self) {
        if self.prompt.is_some() || self.commit.is_some() {
            return;
        }
        let path = match &self.view {
            View::Reader(r) => match &r.origin {
                ReaderOrigin::File(p) => p.clone(),
                _ => return,
            },
            _ => return,
        };
        // The buffer was just written (or was already on disk); rebuild the
        // status so a file that only now became dirty is seen as uncommitted.
        self.refresh_git_status();
        let Some(root) = self.git_status.root.clone() else {
            return;
        };
        if !self.git_status.is_uncommitted(&path) {
            return;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        self.prompt = Some(Prompt {
            title: format!(" Commit {name} (Esc to skip) "),
            input: String::new(),
            kind: PromptKind::CommitFile { root, file: path },
        });
    }

    /// Keep the editor buffer ending in a single trailing newline while a
    /// document is open for editing. This is the POSIX text-file convention
    /// (git and most editors enforce a final newline on save), and the raw
    /// editor leans on it directly: a trailing `\n` produces the phantom empty
    /// row in `render_raw_pane`, which is what lets the cursor step down off
    /// the last content line. The event loop calls this after every input
    /// event, so a Backspace, Delete, or `:s` that strips the newline is
    /// corrected on the spot — the user can't leave the buffer without one.
    /// No-op outside edit mode and for an empty buffer; never strips content
    /// and never touches the cursor (appending at the end keeps every byte
    /// offset valid).
    pub fn ensure_edit_trailing_newline(&mut self) {
        if let View::Reader(r) = &mut self.view {
            if r.edit.is_some() && !r.raw.is_empty() && !r.raw.ends_with('\n') {
                r.raw.push('\n');
                r.rendered = None;
            }
        }
    }

    /// Insert `text` at the current edit cursor and advance the cursor past
    /// it. Marks dirty. No-op outside edit mode.
    pub fn edit_insert(&mut self, text: &str) {
        let View::Reader(r) = &mut self.view else {
            return;
        };
        if r.edit.is_none() {
            return;
        }
        push_undo(r);
        let e = r.edit.as_mut().unwrap();
        let pos = e.cursor.min(r.raw.len());
        // Snap to the nearest char boundary <= pos so we don't mid-byte split.
        let pos = floor_char_boundary(&r.raw, pos);
        r.raw.insert_str(pos, text);
        let e = r.edit.as_mut().unwrap();
        e.cursor = pos + text.len();
        e.dirty = true;
        e.command = None;
        r.rendered = None;
    }

    /// Recompute the heading-anchor autocomplete popup against the current
    /// buffer/cursor. Called after every ordinary edit keystroke. When
    /// `allow_open` is set (the user just typed `#`) a matching context opens
    /// a fresh popup; otherwise it only refreshes/closes one already open, so
    /// merely moving the cursor past an existing `#` never pops the list up.
    pub fn edit_anchor_sync(&mut self, allow_open: bool) {
        // Phase 1: read-only detection, producing owned data so the immutable
        // borrow of `self.view` is dropped before we mutate below.
        let prepared = {
            let View::Reader(r) = &self.view else {
                return;
            };
            let Some(e) = r.edit.as_ref() else {
                return;
            };
            let cursor = floor_char_boundary(&r.raw, e.cursor.min(r.raw.len()));
            match anchor_context(&r.raw, cursor) {
                None => None,
                Some((hash, path)) => {
                    let same = e.anchor_complete.as_ref().map(|a| a.hash) == Some(hash);
                    if !same && !allow_open {
                        None
                    } else {
                        // The path can't change without moving the `#`, so an
                        // already-open popup on this same `#` keeps its list.
                        let candidates = if same {
                            e.anchor_complete.as_ref().unwrap().candidates.clone()
                        } else {
                            build_anchor_candidates(r, &self.root, path)
                        };
                        // Preserve the highlighted slug across a re-filter.
                        let prev_slug = e.anchor_complete.as_ref().and_then(|a| {
                            a.matches
                                .get(a.selected)
                                .and_then(|&i| a.candidates.get(i))
                                .map(|c| c.slug.clone())
                        });
                        let query = r.raw[hash + 1..cursor].to_ascii_lowercase();
                        Some((hash, candidates, prev_slug, query))
                    }
                }
            }
        };
        let Some((hash, candidates, prev_slug, query)) = prepared else {
            self.edit_anchor_dismiss();
            return;
        };
        let matches: Vec<usize> = candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                query.is_empty()
                    || c.slug.contains(&query)
                    || c.text.to_ascii_lowercase().contains(&query)
            })
            .map(|(i, _)| i)
            .collect();
        if matches.is_empty() {
            self.edit_anchor_dismiss();
            return;
        }
        let selected = prev_slug
            .and_then(|s| matches.iter().position(|&i| candidates[i].slug == s))
            .unwrap_or(0);
        if let View::Reader(r) = &mut self.view {
            if let Some(e) = r.edit.as_mut() {
                e.anchor_complete = Some(AnchorComplete {
                    hash,
                    candidates,
                    matches,
                    selected,
                    scroll: 0,
                    rect: Rect::default(),
                });
            }
        }
    }

    /// Move the anchor-autocomplete selection by `delta` rows, wrapping around.
    pub fn edit_anchor_move(&mut self, delta: i32) {
        if let View::Reader(r) = &mut self.view {
            if let Some(ac) = r.edit.as_mut().and_then(|e| e.anchor_complete.as_mut()) {
                let n = ac.matches.len() as i32;
                if n > 0 {
                    ac.selected = (ac.selected as i32 + delta).rem_euclid(n) as usize;
                }
            }
        }
    }

    /// Close the anchor-autocomplete popup without inserting anything.
    pub fn edit_anchor_dismiss(&mut self) {
        if let View::Reader(r) = &mut self.view {
            if let Some(e) = r.edit.as_mut() {
                e.anchor_complete = None;
            }
        }
    }

    /// Accept the highlighted anchor: replace the typed query (`raw[hash + 1 ..
    /// cursor]`) with the chosen slug, leaving the `#` in place, and close the
    /// popup. No-op if the stored range no longer makes sense.
    pub fn edit_anchor_accept(&mut self) {
        let View::Reader(r) = &mut self.view else {
            return;
        };
        let Some(e) = r.edit.as_ref() else {
            return;
        };
        let Some(ac) = e.anchor_complete.as_ref() else {
            return;
        };
        let Some(slug) = ac
            .matches
            .get(ac.selected)
            .and_then(|&i| ac.candidates.get(i))
            .map(|c| c.slug.clone())
        else {
            return;
        };
        let start = ac.hash + 1;
        let cursor = floor_char_boundary(&r.raw, e.cursor.min(r.raw.len()));
        if start > cursor || cursor > r.raw.len() {
            // Stale range (buffer moved under the popup) — just close it.
            r.edit.as_mut().unwrap().anchor_complete = None;
            return;
        }
        push_undo(r);
        r.raw.replace_range(start..cursor, &slug);
        let e = r.edit.as_mut().unwrap();
        e.cursor = start + slug.len();
        e.dirty = true;
        e.command = None;
        e.anchor_complete = None;
        r.rendered = None;
    }

    /// Enter in the editor. Inside a markdown list this auto-continues the
    /// list: a bullet, numbered, or checkbox line spawns the next marker on
    /// the new line, and in a numbered list the items below shift up so the
    /// sequence stays continuous. Pressing Enter on an *empty* list item (just
    /// the marker) instead terminates the list, clearing it and leaving a blank
    /// line — the standard GitHub/Obsidian behaviour. Outside a list it's a
    /// plain newline.
    pub fn edit_newline(&mut self) {
        let View::Reader(r) = &self.view else {
            return;
        };
        let Some(e) = r.edit.as_ref() else { return };
        let cursor = floor_char_boundary(&r.raw, e.cursor.min(r.raw.len()));
        let line_start = r.raw[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_end = r.raw[cursor..]
            .find('\n')
            .map(|i| cursor + i)
            .unwrap_or(r.raw.len());
        let line = r.raw[line_start..line_end].to_string();
        let cursor_at_end = cursor == line_end;
        match list_continuation(&line, cursor_at_end) {
            Some(ListContinue::Marker(marker)) => {
                self.edit_insert(&format!("\n{marker}"));
                self.edit_renumber_below(&marker);
            }
            Some(ListContinue::Empty) => {
                // Terminate the list: blank the marker-only line, cursor at
                // its start. No new line is added — pressing Enter again then
                // produces a normal newline.
                let View::Reader(r) = &mut self.view else {
                    return;
                };
                push_undo(r);
                r.raw.replace_range(line_start..line_end, "");
                let e = r.edit.as_mut().unwrap();
                e.cursor = line_start;
                e.dirty = true;
                e.command = None;
                r.rendered = None;
            }
            None => self.edit_insert("\n"),
        }
    }

    /// After Enter continued a numbered list, bump the numbers of the items
    /// below so `1. 2. 3.` becomes `1. 2. 3. 4.` instead of `1. 2. 2. 3.`.
    /// `marker` is the just-inserted `{indent}{n}{sep} ` prefix; anything else
    /// (bullets, checkboxes, alphabetic markers) is a no-op.
    ///
    /// Runs after `edit_insert`, which already pushed the undo snapshot, so a
    /// single Enter still undoes as one step.
    fn edit_renumber_below(&mut self, marker: &str) {
        let View::Reader(r) = &mut self.view else {
            return;
        };
        let Some(e) = r.edit.as_ref() else { return };
        let indent_len = marker
            .find(|c: char| c != ' ' && c != '\t')
            .unwrap_or(marker.len());
        let (indent, rest) = marker.split_at(indent_len);
        let Some((num, sep, _)) = ordered_marker(rest) else {
            return;
        };
        // The inserted item ends at the next newline; the list below starts
        // after it. The cursor sits before that, so it needs no adjusting.
        let cursor = e.cursor.min(r.raw.len());
        let Some(nl) = r.raw[cursor..].find('\n') else {
            return;
        };
        let Some(next) = num.checked_add(1) else {
            return;
        };
        let edits = renumber_edits(&r.raw, cursor + nl + 1, indent, sep, next);
        if edits.is_empty() {
            return;
        }
        // Apply back-to-front so the earlier offsets stay valid.
        for (start, end, text) in edits.into_iter().rev() {
            r.raw.replace_range(start..end, &text);
        }
        r.rendered = None;
    }

    /// Move the current source line up (`delta < 0`) or down (`delta > 0`),
    /// swapping it with its neighbour. Operates on logical lines (the text
    /// between `\n`s, as if the pane were infinitely wide), so the whole line
    /// travels as one unit regardless of how it visually wraps — handy for
    /// reordering list items. The cursor rides along with the moved line,
    /// keeping its byte column. No-op at the buffer edge (the first line can't
    /// go up, the last can't go down) or outside edit mode.
    pub fn edit_move_line(&mut self, delta: i32) {
        let View::Reader(r) = &mut self.view else {
            return;
        };
        let Some(e) = r.edit.as_ref() else { return };
        let len = r.raw.len();
        let c = floor_char_boundary(&r.raw, e.cursor.min(len));
        // Current line: [ls, le), excluding its trailing newline (if any).
        let ls = r.raw[..c].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let le = r.raw[c..].find('\n').map(|i| c + i).unwrap_or(len);

        let (region_start, region_end, new_region, new_cursor) = if delta < 0 {
            // Move up: swap with the previous line.
            if ls == 0 {
                return; // already the first line
            }
            let pe = ls - 1; // the '\n' that ends the previous line
            let ps = r.raw[..pe].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let prev = &r.raw[ps..pe];
            let cur = &r.raw[ls..le];
            (ps, le, format!("{cur}\n{prev}"), ps + (c - ls))
        } else {
            // Move down: swap with the next line.
            if le == len {
                return; // already the last line
            }
            let ns = le + 1;
            let ne = r.raw[ns..].find('\n').map(|i| ns + i).unwrap_or(len);
            let cur = &r.raw[ls..le];
            let next = &r.raw[ns..ne];
            (
                ls,
                ne,
                format!("{next}\n{cur}"),
                ls + next.len() + 1 + (c - ls),
            )
        };

        push_undo(r);
        r.raw.replace_range(region_start..region_end, &new_region);
        let e = r.edit.as_mut().unwrap();
        e.cursor = new_cursor;
        e.dirty = true;
        e.command = None;
        e.selection = None;
        r.rendered = None;
    }

    /// Delete `n` chars to the left of the cursor (Backspace). No-op if
    /// the cursor is at byte 0.
    pub fn edit_backspace(&mut self) {
        let View::Reader(r) = &mut self.view else {
            return;
        };
        let Some(e) = r.edit.as_ref() else { return };
        if e.cursor == 0 {
            return;
        }
        push_undo(r);
        let e = r.edit.as_mut().unwrap();
        let end = floor_char_boundary(&r.raw, e.cursor);
        let prev = prev_char_boundary(&r.raw, end);
        r.raw.replace_range(prev..end, "");
        let e = r.edit.as_mut().unwrap();
        e.cursor = prev;
        e.dirty = true;
        e.command = None;
        r.rendered = None;
    }

    /// Delete one char to the right of the cursor (Delete key).
    pub fn edit_delete(&mut self) {
        let View::Reader(r) = &mut self.view else {
            return;
        };
        if r.edit.is_none() {
            return;
        }
        let e = r.edit.as_ref().unwrap();
        let pos = floor_char_boundary(&r.raw, e.cursor);
        if pos >= r.raw.len() {
            return;
        }
        push_undo(r);
        let next = next_char_boundary(&r.raw, pos);
        r.raw.replace_range(pos..next, "");
        let e = r.edit.as_mut().unwrap();
        e.dirty = true;
        e.command = None;
        r.rendered = None;
    }

    /// Undo the last edit. Pops a snapshot off the undo stack, pushes the
    /// current state to redo, and restores raw + cursor.
    pub fn edit_undo(&mut self) {
        let View::Reader(r) = &mut self.view else {
            return;
        };
        if r.edit.is_none() {
            return;
        }
        let e = r.edit.as_mut().unwrap();
        let Some(snap) = e.undo.pop() else { return };
        // Save current as redo entry.
        let cur_cursor = e.cursor;
        let cur_raw = std::mem::take(&mut r.raw);
        // Restore.
        r.raw = snap.raw;
        let e = r.edit.as_mut().unwrap();
        e.redo.push(EditSnapshot {
            raw: cur_raw,
            cursor: cur_cursor,
        });
        e.cursor = snap.cursor.min(r.raw.len());
        e.dirty = true; // Even after undo, the buffer differs from disk usually.
        e.command = None;
        r.rendered = None;
    }

    /// Redo a previously undone edit.
    pub fn edit_redo(&mut self) {
        let View::Reader(r) = &mut self.view else {
            return;
        };
        if r.edit.is_none() {
            return;
        }
        let e = r.edit.as_mut().unwrap();
        let Some(snap) = e.redo.pop() else { return };
        let cur_cursor = e.cursor;
        let cur_raw = std::mem::take(&mut r.raw);
        r.raw = snap.raw;
        let e = r.edit.as_mut().unwrap();
        e.undo.push(EditSnapshot {
            raw: cur_raw,
            cursor: cur_cursor,
        });
        if e.undo.len() > UNDO_LIMIT {
            e.undo.remove(0);
        }
        e.cursor = snap.cursor.min(r.raw.len());
        e.dirty = true;
        e.command = None;
        r.rendered = None;
    }

    /// Move cursor by one word left/right. "Word" = run of non-whitespace,
    /// matching macOS-native Alt-arrow semantics: skips through any
    /// whitespace adjacent to the cursor, then through the next non-
    /// whitespace run, landing on the far edge.
    pub fn edit_move_word(&mut self, delta: i32) {
        let View::Reader(r) = &mut self.view else {
            return;
        };
        let Some(e) = r.edit.as_mut() else { return };
        e.command = None;
        let pos = floor_char_boundary(&r.raw, e.cursor);
        let new = if delta < 0 {
            prev_word_boundary(&r.raw, pos)
        } else {
            next_word_boundary(&r.raw, pos)
        };
        if new != e.cursor {
            e.cursor = new;
            r.rendered = None;
        }
    }

    /// Delete from the cursor to the next/previous word boundary.
    /// `forward = true` deletes rightward (Alt-Delete), `false` deletes
    /// leftward (Alt-Backspace). One undo snapshot per call.
    pub fn edit_delete_word(&mut self, forward: bool) {
        let View::Reader(r) = &mut self.view else {
            return;
        };
        if r.edit.is_none() {
            return;
        }
        let cur = r.edit.as_ref().unwrap().cursor;
        let pos = floor_char_boundary(&r.raw, cur);
        let (from, to) = if forward {
            let to = next_word_boundary(&r.raw, pos);
            (pos, to)
        } else {
            let from = prev_word_boundary(&r.raw, pos);
            (from, pos)
        };
        if from == to {
            return;
        }
        push_undo(r);
        r.raw.replace_range(from..to, "");
        let e = r.edit.as_mut().unwrap();
        e.cursor = from;
        e.dirty = true;
        e.command = None;
        r.rendered = None;
    }

    /// The current source line (the one the cursor sits on), including its
    /// trailing newline. Backs whole-line copy/cut when nothing is selected,
    /// mirroring the common editor behaviour where Ctrl+C/Ctrl+X with no
    /// selection acts on the line.
    pub fn edit_current_line_text(&self) -> Option<String> {
        let View::Reader(r) = &self.view else {
            return None;
        };
        let e = r.edit.as_ref()?;
        let (start, end) = line_bounds_with_newline(&r.raw, e.cursor);
        r.raw.get(start..end).map(str::to_string)
    }

    /// Remove the current source line (with its trailing newline) and return
    /// it. One undo snapshot; the cursor lands at the start of what is now the
    /// line that followed.
    pub fn edit_cut_current_line(&mut self) -> Option<String> {
        let View::Reader(r) = &mut self.view else {
            return None;
        };
        let cursor = r.edit.as_ref()?.cursor;
        let (start, end) = line_bounds_with_newline(&r.raw, cursor);
        if start == end {
            return None;
        }
        push_undo(r);
        let removed = r.raw[start..end].to_string();
        r.raw.replace_range(start..end, "");
        let e = r.edit.as_mut().unwrap();
        e.cursor = floor_char_boundary(&r.raw, start.min(r.raw.len()));
        e.dirty = true;
        e.command = None;
        r.rendered = None;
        Some(removed)
    }

    /// Text covered by the active editor drag-selection, if any.
    pub fn edit_selection_text(&self) -> Option<String> {
        let View::Reader(r) = &self.view else {
            return None;
        };
        let sel = r.edit.as_ref()?.selection.as_ref()?;
        if !sel.is_active() {
            return None;
        }
        let (from, to) = sel.range();
        r.raw.get(from..to.min(r.raw.len())).map(str::to_string)
    }

    /// Delete the active editor drag-selection. One undo snapshot; the
    /// cursor lands where the selection started.
    pub fn edit_delete_selection(&mut self) {
        let View::Reader(r) = &mut self.view else {
            return;
        };
        let Some((from, to)) = r
            .edit
            .as_ref()
            .and_then(|e| e.selection.as_ref())
            .filter(|s| s.is_active())
            .map(|s| s.range())
        else {
            return;
        };
        let from = floor_char_boundary(&r.raw, from.min(r.raw.len()));
        let to = floor_char_boundary(&r.raw, to.min(r.raw.len()));
        if from >= to {
            r.edit.as_mut().unwrap().selection = None;
            return;
        }
        push_undo(r);
        r.raw.replace_range(from..to, "");
        let e = r.edit.as_mut().unwrap();
        e.cursor = from;
        e.dirty = true;
        e.command = None;
        e.selection = None;
        r.rendered = None;
    }

    /// Drop the editor drag-selection without touching the buffer.
    pub fn edit_clear_selection(&mut self) {
        if let View::Reader(r) = &mut self.view {
            if let Some(e) = r.edit.as_mut() {
                e.selection = None;
            }
        }
    }

    /// Run a cursor motion while growing the keyboard selection (Shift+arrow
    /// family, macOS-style). Anchors at the pre-move cursor when no selection
    /// is active, then drags the focus to wherever the motion lands. Drops the
    /// selection if the motion didn't move (focus collapses onto the anchor).
    fn edit_extend(&mut self, motion: impl FnOnce(&mut Self)) {
        let before = match &self.view {
            View::Reader(r) => match r.edit.as_ref() {
                Some(e) => e.cursor,
                None => return,
            },
            _ => return,
        };
        if let View::Reader(r) = &mut self.view {
            if let Some(e) = r.edit.as_mut() {
                // Keep an existing active selection's anchor so repeated
                // Shift+arrow keeps extending from the same origin.
                let anchor = e
                    .selection
                    .as_ref()
                    .filter(|s| s.dragged)
                    .map(|s| s.anchor)
                    .unwrap_or(before);
                e.selection = Some(EditSelection {
                    anchor,
                    focus: before,
                    origin: anchor,
                    dragged: true,
                });
            }
        }
        motion(self);
        if let View::Reader(r) = &mut self.view {
            if let Some(e) = r.edit.as_mut() {
                let cur = e.cursor;
                if let Some(s) = e.selection.as_mut() {
                    s.focus = cur;
                    if s.anchor == s.focus {
                        e.selection = None;
                    }
                }
            }
        }
    }

    /// Extend the keyboard selection by one char left/right (Shift+arrow).
    pub fn edit_extend_horizontal(&mut self, delta: i32) {
        self.edit_extend(|s| s.edit_move_horizontal(delta));
    }

    /// Extend the keyboard selection by one word left/right (Shift+Option/
    /// Ctrl+arrow), matching the word motion of `edit_move_word`.
    pub fn edit_extend_word(&mut self, delta: i32) {
        self.edit_extend(|s| s.edit_move_word(delta));
    }

    /// Extend the keyboard selection up/down one display row (Shift+arrow).
    pub fn edit_extend_vertical(&mut self, delta: i32) {
        self.edit_extend(|s| s.edit_move_vertical(delta));
    }

    /// Extend the keyboard selection to the start/end of the line (Shift+Home/
    /// End, or Shift+Cmd+arrow).
    pub fn edit_extend_line_edge(&mut self, eol: bool) {
        self.edit_extend(|s| s.edit_move_line_edge(eol));
    }

    /// Set the editor selection to span inclusively between two click points
    /// (byte offsets at char starts) for shift-click range select. The char
    /// at each end is fully covered, regardless of click order. Lands the
    /// cursor at the right edge of the selection.
    pub fn edit_select_inclusive(&mut self, a: usize, b: usize) {
        let View::Reader(r) = &mut self.view else {
            return;
        };
        if r.edit.is_none() {
            return;
        }
        let a = floor_char_boundary(&r.raw, a.min(r.raw.len()));
        let b = floor_char_boundary(&r.raw, b.min(r.raw.len()));
        let lo = a.min(b);
        // Cover the char under the rightmost click by reaching its far edge.
        let hi = next_char_boundary(&r.raw, a.max(b));
        let e = r.edit.as_mut().unwrap();
        e.selection = if lo >= hi {
            None
        } else {
            Some(EditSelection {
                anchor: lo,
                focus: hi,
                origin: lo,
                dragged: true,
            })
        };
        e.cursor = hi;
        e.command = None;
        r.rendered = None;
    }

    /// Replace the active editor drag-selection with `text` (paste-over), or
    /// insert `text` at the cursor when no selection is active. One undo
    /// snapshot; the cursor lands just past the inserted text. No-op outside
    /// edit mode.
    pub fn edit_replace_selection(&mut self, text: &str) {
        let View::Reader(r) = &mut self.view else {
            return;
        };
        if r.edit.is_none() {
            return;
        }
        // An active drag-selection defines the range to overwrite; otherwise
        // we paste at the cursor (a zero-width range).
        let range = r
            .edit
            .as_ref()
            .and_then(|e| e.selection.as_ref())
            .filter(|s| s.is_active())
            .map(|s| s.range());
        let (from, to) = match range {
            Some((a, b)) => (
                floor_char_boundary(&r.raw, a.min(r.raw.len())),
                floor_char_boundary(&r.raw, b.min(r.raw.len())),
            ),
            None => {
                let c =
                    floor_char_boundary(&r.raw, r.edit.as_ref().unwrap().cursor.min(r.raw.len()));
                (c, c)
            }
        };
        push_undo(r);
        r.raw.replace_range(from..to, text);
        let e = r.edit.as_mut().unwrap();
        e.cursor = from + text.len();
        e.dirty = true;
        e.command = None;
        e.selection = None;
        r.rendered = None;
    }

    /// Wrap the active drag-selection in `open`…`close` (pair completion over
    /// a selection, e.g. select a word and press `*` → `*word*`). Returns
    /// `true` when a selection was wrapped. One undo step; the cursor lands
    /// just past the closing delimiter and the selection is cleared.
    pub fn edit_wrap_selection(&mut self, open: char, close: char) -> bool {
        let View::Reader(r) = &mut self.view else {
            return false;
        };
        let range = r
            .edit
            .as_ref()
            .and_then(|e| e.selection.as_ref())
            .filter(|s| s.is_active())
            .map(|s| s.range());
        let Some((from, to)) = range else {
            return false;
        };
        let from = floor_char_boundary(&r.raw, from.min(r.raw.len()));
        let to = floor_char_boundary(&r.raw, to.min(r.raw.len()));
        if from >= to {
            return false;
        }
        push_undo(r);
        // Insert the closer first so `from` stays valid for the opener.
        r.raw.insert(to, close);
        r.raw.insert(from, open);
        let e = r.edit.as_mut().unwrap();
        e.cursor = to + open.len_utf8() + close.len_utf8();
        e.dirty = true;
        e.command = None;
        e.selection = None;
        r.rendered = None;
        true
    }

    /// Wrap the active selection as a markdown link `[selected](url)`, using
    /// the selected text as the link label and `url` as the target. Returns
    /// `true` when a selection was linked; a no-op (`false`) when nothing is
    /// selected. One undo step; the cursor lands just past the closing paren
    /// and the selection is cleared.
    pub fn edit_link_selection(&mut self, url: &str) -> bool {
        let View::Reader(r) = &mut self.view else {
            return false;
        };
        let range = r
            .edit
            .as_ref()
            .and_then(|e| e.selection.as_ref())
            .filter(|s| s.is_active())
            .map(|s| s.range());
        let Some((from, to)) = range else {
            return false;
        };
        let from = floor_char_boundary(&r.raw, from.min(r.raw.len()));
        let to = floor_char_boundary(&r.raw, to.min(r.raw.len()));
        if from >= to {
            return false;
        }
        push_undo(r);
        // Insert the `](url)` tail first so `from` stays valid for the `[`.
        let tail = format!("]({url})");
        r.raw.insert_str(to, &tail);
        r.raw.insert(from, '[');
        let e = r.edit.as_mut().unwrap();
        e.cursor = to + '['.len_utf8() + tail.len();
        e.dirty = true;
        e.command = None;
        e.selection = None;
        r.rendered = None;
        true
    }

    /// Insert an `open``close` pair at the cursor and place the cursor between
    /// them (bracket auto-close). One undo step.
    /// Whether an opening bracket should auto-insert its closer: only when the
    /// cursor is at the end of the buffer or the next character is whitespace
    /// (space / tab / newline). Typing `(` directly left of other text inserts
    /// a lone `(` so wrapping existing text doesn't strand a `)` mid-word.
    pub fn edit_autoclose_ok(&self) -> bool {
        let View::Reader(r) = &self.view else {
            return true;
        };
        let Some(e) = r.edit.as_ref() else {
            return true;
        };
        let pos = e.cursor.min(r.raw.len());
        match r.raw[pos..].chars().next() {
            None => true,
            Some(c) => c.is_whitespace(),
        }
    }

    pub fn edit_insert_pair(&mut self, open: char, close: char) {
        let View::Reader(r) = &mut self.view else {
            return;
        };
        if r.edit.is_none() {
            return;
        }
        push_undo(r);
        let e = r.edit.as_mut().unwrap();
        let pos = floor_char_boundary(&r.raw, e.cursor.min(r.raw.len()));
        r.raw.insert(pos, close);
        r.raw.insert(pos, open);
        let e = r.edit.as_mut().unwrap();
        e.cursor = pos + open.len_utf8();
        e.dirty = true;
        e.command = None;
        r.rendered = None;
    }

    /// If the char immediately after the cursor is `close`, step over it
    /// instead of inserting (so typing the closing bracket of an auto-closed
    /// pair just moves past it). Returns `true` when it stepped over.
    pub fn edit_try_type_over(&mut self, close: char) -> bool {
        let View::Reader(r) = &mut self.view else {
            return false;
        };
        let Some(e) = r.edit.as_ref() else {
            return false;
        };
        let pos = floor_char_boundary(&r.raw, e.cursor.min(r.raw.len()));
        if r.raw[pos..].chars().next() == Some(close) {
            let e = r.edit.as_mut().unwrap();
            e.cursor = pos + close.len_utf8();
            r.rendered = None;
            return true;
        }
        false
    }

    /// Move cursor by one char left/right (`delta` ±1). Re-renders so the
    /// block-level toggle can swap blocks if the cursor crossed a boundary.
    pub fn edit_move_horizontal(&mut self, delta: i32) {
        let View::Reader(r) = &mut self.view else {
            return;
        };
        let Some(e) = r.edit.as_mut() else { return };
        e.command = None;
        let pos = floor_char_boundary(&r.raw, e.cursor);
        let new = if delta < 0 {
            prev_char_boundary(&r.raw, pos)
        } else {
            next_char_boundary(&r.raw, pos)
        };
        if new != e.cursor {
            e.cursor = new;
            r.rendered = None;
        }
    }

    /// Move the cursor up/down one *display* row in whichever pane owns the
    /// cursor (raw pane in split mode; the cursor's block in legacy in-place
    /// mode). Falls back to source-line stepping if the target row is out
    /// of range.
    pub fn edit_move_vertical(&mut self, delta: i32) {
        // Split-mode: walk the raw-pane wrap.
        let split_mode = matches!(
            &self.view,
            View::Reader(r) if r.edit.as_ref().map(|e| e.mode == EditMode::Split).unwrap_or(false)
        );
        if split_mode {
            let raw_w = self.edit_raw_area.width.max(1) as usize;
            let View::Reader(r) = &mut self.view else {
                return;
            };
            let Some(e) = r.edit.as_mut() else { return };
            e.command = None;
            let cursor = e.cursor;
            let rows = render_raw_pane(&r.raw, raw_w);
            let cur_idx = raw_row_for_cursor(&rows, cursor);
            let target_idx = (cur_idx as i32 + delta).max(0) as usize;
            let target_idx = target_idx.min(rows.len().saturating_sub(1));
            let cur_col = rows
                .get(cur_idx)
                .map(|row| raw_col_for_cursor(&r.raw, row, cursor))
                .unwrap_or(0) as usize;
            let new = raw_click_to_source(&rows, &r.raw, target_idx, cur_col);
            if new != e.cursor {
                e.cursor = new;
                r.rendered = None;
            }
            return;
        }

        // Legacy InPlace mode: step display rows of the active raw block,
        // falling back to source-line stepping when crossing block bounds.
        let View::Reader(r) = &mut self.view else {
            return;
        };
        let Some(e) = r.edit.as_mut() else { return };
        e.command = None;
        if let Some(rendered) = r.rendered.as_ref() {
            if let Some((cur_col, cur_row)) = rendered.cursor_xy {
                let target_row = (cur_row as i32 + delta).max(0) as usize;
                if let Some(Some(range)) = rendered.row_source.get(target_row) {
                    let new = source_offset_at_col(&r.raw, range, cur_col as usize);
                    if new != e.cursor {
                        e.cursor = new;
                        r.rendered = None;
                    }
                    return;
                }
            }
        }
        let (line_idx, col) = source_line_col(&r.raw, e.cursor);
        let target_line = (line_idx as i32 + delta).max(0) as usize;
        let new = source_offset_for(&r.raw, target_line, col);
        if new != e.cursor {
            e.cursor = new;
            r.rendered = None;
        }
    }

    /// Move cursor to start (`eol=false`) or end (`eol=true`) of the current
    /// display row in the raw pane (split mode) or current source line
    /// (legacy in-place mode).
    pub fn edit_move_line_edge(&mut self, eol: bool) {
        let split_mode = matches!(
            &self.view,
            View::Reader(r) if r.edit.as_ref().map(|e| e.mode == EditMode::Split).unwrap_or(false)
        );
        if split_mode {
            let raw_w = self.edit_raw_area.width.max(1) as usize;
            let View::Reader(r) = &mut self.view else {
                return;
            };
            let Some(e) = r.edit.as_mut() else { return };
            e.command = None;
            let rows = render_raw_pane(&r.raw, raw_w);
            let cur_idx = raw_row_for_cursor(&rows, e.cursor);
            let new = if let Some(row) = rows.get(cur_idx) {
                if eol {
                    row.source_range.end
                } else {
                    row.source_range.start
                }
            } else {
                e.cursor
            };
            if new != e.cursor {
                e.cursor = new;
                r.rendered = None;
            }
            return;
        }

        let View::Reader(r) = &mut self.view else {
            return;
        };
        let Some(e) = r.edit.as_mut() else { return };
        e.command = None;
        let (line_idx, _col) = source_line_col(&r.raw, e.cursor);
        let new = if eol {
            source_line_end(&r.raw, line_idx)
        } else {
            source_line_start(&r.raw, line_idx)
        };
        if new != e.cursor {
            e.cursor = new;
            r.rendered = None;
        }
    }

    /// Persist the current buffer to disk. Refreshes the on-disk fingerprint
    /// so the external-change watcher doesn't see our own write as a phantom
    /// edit on the next tick.
    pub fn save_edit(&mut self) -> Result<()> {
        let View::Reader(r) = &mut self.view else {
            return Ok(());
        };
        if r.edit.is_none() {
            return Ok(());
        }
        // Persist with the POSIX trailing newline. The edit loop already keeps
        // the live buffer normalized; this also covers non-interactive save
        // paths so what lands on disk / HackMD always ends in one `\n`.
        if !r.raw.is_empty() && !r.raw.ends_with('\n') {
            r.raw.push('\n');
            r.rendered = None;
        }
        // Set when a saved local file is linked, so we can kick off a sync
        // after the `view`/`r` borrow is released.
        let mut sync_after: Option<PathBuf> = None;
        match r.origin.clone() {
            ReaderOrigin::File(path) => {
                std::fs::write(&path, &r.raw)
                    .map_err(|e| anyhow!("write {}: {}", path.display(), e))?;
                r.last_meta = file_meta(&path);
                if let Some(e) = r.edit.as_mut() {
                    e.dirty = false;
                    e.command = None;
                }
                // Saved to disk → the crash-recovery mirror is obsolete.
                crate::tui::recovery::clear(&path);
                self.recovery_throttle = None;
                self.status = format!("Saved {}", path.display());
                // Linked file → merge with upstream right after saving.
                if crate::tui::hackmd_meta::parse(&r.raw).is_some() {
                    sync_after = Some(path);
                }
            }
            ReaderOrigin::CloudNote { id, team_path, .. } => {
                // Pessimistic: `dirty` stays set until `Saved{Ok}` lands, so
                // a failed PATCH can never silently lose the marker.
                if self.cloud.saving.contains(&id) {
                    self.status = "Save already in flight…".into();
                    return Ok(());
                }
                // Cloud-only edit → never advance a linked file's base.
                if self.cloud.request_save(id, team_path, r.raw.clone(), None) {
                    if let Some(e) = r.edit.as_mut() {
                        e.command = None;
                    }
                    self.status = "⟳ saving to HackMD…".into();
                } else {
                    self.status = NO_TOKEN_HINT.into();
                }
            }
            ReaderOrigin::Stdin => {}
        }
        // Trigger an immediate sync now the borrow on `self.view` is gone. A
        // fresh trigger (clear `last_sync`) so it fires this cycle regardless
        // of the background interval.
        if let Some(path) = sync_after {
            self.last_sync = None;
            self.sync_local_file(path);
        }
        // A save changes the working tree — refresh so the `[uncommitted]`
        // badge is accurate the moment the user returns to the browser.
        self.refresh_git_status();
        Ok(())
    }

    /// Re-render reader if width or edit-mode cursor changed since last
    /// render. Edit mode bypasses the cached render whenever the cursor
    /// has moved (in-place mode) or whenever the buffer differs from
    /// the last render's source (split mode rebuilds the preview each
    /// frame; the renderer is fast enough at TUI file sizes).
    pub fn ensure_rendered(&mut self, width: u16) {
        let theme = self.opts.theme.clone();
        let user_width = self.opts.width;
        let target_w = if user_width == 0 {
            width
        } else {
            user_width.min(width)
        };
        // Viewport height for the post-render scroll clamp below. Captured
        // before the `view` borrow so it can be read while `r` is held.
        let viewport_h = self.viewport.height as usize;
        if let View::Reader(r) = &mut self.view {
            // A Marp slide with a left/right background wraps its text into the
            // narrower content column, so the cache key is that effective width
            // (comparing against `target_w` would re-render every frame).
            let target_w = if r.marp_present() {
                let (l, rr) = r
                    .marp
                    .as_ref()
                    .and_then(|m| m.current())
                    .map(|s| s.split_widths(target_w))
                    .unwrap_or((0, 0));
                target_w.saturating_sub(l + rr).max(1)
            } else {
                target_w
            };
            let needs = match &r.rendered {
                Some(rd) => rd.width != target_w || r.edit.is_some(),
                None => true,
            };
            if needs {
                let base_dir = match &r.origin {
                    ReaderOrigin::File(p) => p.parent().map(|p| p.to_path_buf()),
                    ReaderOrigin::Stdin | ReaderOrigin::CloudNote { .. } => None,
                };
                // In split-screen edit mode the preview pane shows the fully
                // formatted markdown — no in-place block toggle. The cursor
                // lives in the raw pane only. In legacy InPlace mode (kept
                // for future) we still pass the cursor so the cursor's
                // block renders raw.
                let edit_ctx = r.edit.as_ref().and_then(|e| match e.mode {
                    EditMode::Split => None,
                    EditMode::InPlace => Some(markdown::EditCtx { cursor: e.cursor }),
                });
                // For JSON-line files we bypass the plain `render_source` and
                // emit a transformed code block in which expanded source lines
                // explode into multiple content lines. The returned mapping
                // lets the post-render pass paint expand/collapse buttons on
                // the right rows.
                let (source, jsonl_map) = if r.marp_present() {
                    // Presentation mode renders only the current slide's Markdown
                    // (comments/directives already stripped by the parser).
                    let body = r
                        .marp
                        .as_ref()
                        .and_then(|m| m.current())
                        .map(|s| s.body.clone())
                        .unwrap_or_default();
                    (std::borrow::Cow::Owned(body), None)
                } else if r.is_jsonl_view() {
                    let (s, m) = r.jsonl_render_source();
                    (std::borrow::Cow::Owned(s), Some(m))
                } else {
                    (r.render_source(), None)
                };
                let mut rendered = markdown::render_with_edit(
                    source.as_ref(),
                    base_dir.as_deref(),
                    target_w,
                    &theme,
                    edit_ctx,
                    &r.tables,
                );
                // Inject the gutter buttons + record their hit boxes. The
                // Pre block sits inside `rendered.blocks` — pick the first
                // (and only) entry produced by our synthetic fenced wrap.
                if let Some(map) = jsonl_map {
                    let pre_start = rendered
                        .blocks
                        .first()
                        .map(|b| b.display_start)
                        .unwrap_or(0);
                    let raw_lines: Vec<&str> = r.raw.split('\n').collect();
                    let overlay = crate::tui::jsonl::inject_buttons(
                        &mut rendered.lines,
                        &raw_lines,
                        &map,
                        &r.jsonl_expanded,
                        pre_start,
                        target_w as usize,
                        theme.accent,
                    );
                    r.jsonl_overlay = if overlay.buttons.is_empty() {
                        None
                    } else {
                        Some(overlay)
                    };
                } else {
                    r.jsonl_overlay = None;
                }
                r.rendered = Some(rendered);
                if let Some(rd) = &r.rendered {
                    let max_scroll = rd.lines.len().saturating_sub(1) as u16;
                    if r.preview_scroll > max_scroll {
                        r.preview_scroll = max_scroll;
                    }
                    // In split mode `r.scroll` is the raw-pane scroll; raw
                    // wrap is computed at draw time so we can't clamp here.
                    let in_split = r
                        .edit
                        .as_ref()
                        .map(|e| e.mode == EditMode::Split)
                        .unwrap_or(false);
                    // Clamp the view-mode scroll so it never leaves the
                    // viewport blank. The last useful scroll keeps a full
                    // screen of content visible (`lines - height`), matching
                    // `scroll_by`'s `max`. Without subtracting the height,
                    // exiting edit mode after deleting most of a long buffer
                    // would strand `scroll` near the old end and show an empty
                    // screen with only the final line at the top.
                    if !in_split {
                        let page_max = rd.lines.len().saturating_sub(viewport_h.max(1)) as u16;
                        if r.scroll > page_max {
                            r.scroll = page_max;
                        }
                    }
                }
            }
        }
        // A cross-file `path#anchor` jump parked its slug while the new
        // document was still unrendered; now that it has display lines, land
        // on the heading.
        if matches!(&self.view, View::Reader(r) if r.rendered.is_some())
            && let Some(slug) = self.pending_anchor.take()
        {
            self.scroll_to_anchor(&slug);
        }
    }
}

/// The text of the document's first level-1 ATX heading (`# Title`), if any.
/// Used to title a note inferred from a local file. Skips a leading YAML
/// front-matter block (`---` … `---`) so a `title:` key there doesn't shadow
/// the heading scan. `## Sub` and deeper are ignored — only a true H1 counts.
fn first_h1(content: &str) -> Option<String> {
    let mut lines = content.lines().peekable();
    // Skip YAML front matter if the very first line is `---`.
    if lines.peek().map(|l| l.trim_end()) == Some("---") {
        lines.next();
        for l in lines.by_ref() {
            if l.trim_end() == "---" {
                break;
            }
        }
    }
    for line in lines {
        let t = line.trim_start();
        // H1 is `#` followed by whitespace then text; `##`+ is not an H1.
        if let Some(rest) = t.strip_prefix('#') {
            if rest.starts_with(char::is_whitespace) {
                let title = rest.trim().trim_end_matches('#').trim();
                if !title.is_empty() {
                    return Some(title.to_string());
                }
            }
        }
    }
    None
}

/// Filesystem-friendly slug of a note title for the download default:
/// lowercased, alphanumerics kept, separator runs collapsed to one `-`.
fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for c in s.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_alphanumeric() {
            out.push(c);
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "note".to_string()
    } else {
        out
    }
}

fn derive_root(source: &Source) -> PathBuf {
    let base = match source {
        Source::File(p) => p
            .parent()
            .map(|x| x.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))),
        Source::Directory(d) => d.clone(),
        Source::Stdin(_) => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    std::fs::canonicalize(&base).unwrap_or(base)
}

pub fn is_markdown_file(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "md" | "markdown" | "mdown" | "mkd"
            )
        })
        .unwrap_or(false)
}

/// Files the browser will list and open: markdown (`.md` and friends), plain
/// `.txt`, or an extension-less file whose bytes look like UTF-8 text. The
/// link-click path uses the same predicate to decide whether to open inside
/// the TUI or hand off to `open::that_detached`.
pub fn is_text_file(p: &Path) -> bool {
    if is_markdown_file(p) {
        return true;
    }
    match p.extension().and_then(|e| e.to_str()) {
        Some(ext) => ext.eq_ignore_ascii_case("txt"),
        // No extension: include it only if it actually reads as text, so we
        // surface notes/READMEs without dragging in binaries.
        None => file_looks_like_text(p),
    }
}

/// Heuristic for "this extension-less file is text": read a bounded prefix and
/// accept it when there's no NUL byte and the bytes decode as UTF-8. A
/// multi-byte char truncated by the read boundary is tolerated. Cheap enough to
/// call while listing a directory.
fn file_looks_like_text(p: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(p) else {
        return false;
    };
    let mut buf = [0u8; 65536];
    let n = match f.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return false,
    };
    let sample = &buf[..n];
    if sample.contains(&0) {
        return false;
    }
    match std::str::from_utf8(sample) {
        Ok(_) => true,
        // `error_len() == None` means the only problem is an incomplete
        // multi-byte sequence at the very end — an artifact of the prefix cut,
        // not actual binary data.
        Err(e) => e.error_len().is_none() && e.valid_up_to() > 0,
    }
}

/// Whether `path`'s full contents decode as UTF-8. Used to refuse opening a
/// file we can't render (the reader needs a `String`). Read errors return
/// `true` so the normal open path surfaces the I/O error instead.
pub fn file_is_valid_utf8(path: &Path) -> bool {
    match std::fs::read(path) {
        Ok(bytes) => std::str::from_utf8(&bytes).is_ok(),
        Err(_) => true,
    }
}

/// Does `dir` hold at least one browsable text file at any depth? Directories
/// with nothing we can open are hidden from the default listing. Honours the
/// same gitignore/hidden rules as the listing and short-circuits on the first
/// hit.
/// `(path, mtime)` for every sub-directory of `dir` that survives the same
/// hidden/gitignore filtering as the listing itself, sorted by path so the
/// result compares equal across calls. Deliberately *not* filtered by
/// `dir_has_listable`: this is what detects a folder whose contents just made
/// it (un)listable. Dot and ignored directories are skipped, which also keeps
/// `.git`'s constant churn from forcing a rebuild on every tick.
fn child_dir_meta(dir: &Path, show_all: bool) -> Vec<(PathBuf, Option<std::time::SystemTime>)> {
    let mut out: Vec<_> = ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(!show_all)
        .git_ignore(!show_all)
        .git_exclude(!show_all)
        .git_global(!show_all)
        .require_git(false)
        .build()
        .flatten()
        .filter(|e| e.path() != dir)
        .filter(|e| e.file_type().is_some_and(|t| t.is_dir()))
        .map(|e| {
            let mtime = e.metadata().ok().and_then(|m| m.modified().ok());
            (e.path().to_path_buf(), mtime)
        })
        .collect();
    out.sort();
    out
}

fn dir_has_listable(dir: &Path) -> bool {
    let walker = ignore::WalkBuilder::new(dir)
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .require_git(false)
        .build();
    let mut empty = true;
    for result in walker {
        let Ok(entry) = result else { continue };
        if entry.path() == dir {
            continue;
        }
        empty = false;
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) && is_text_file(entry.path()) {
            return true;
        }
    }
    // An empty directory is listed even though it holds nothing to read: it's
    // a place to put things, and hiding it makes a folder just created (here
    // with `n`, or outside the app with `mkdir`) look like it failed.
    empty
}

/// Map a path to a syntect language token. Returns an empty string for
/// recognized text formats without a dedicated highlighter (plain text /
/// data formats), which makes syntect fall back to plain text. The empty
/// string is also the fallback for unknown extensions on text files we
/// chose to display anyway.
pub fn lang_token_for_path(p: &Path) -> &'static str {
    if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
        if let Some(tok) = lang_token_for_ext(&ext.to_ascii_lowercase()) {
            return tok;
        }
    }
    let name = p
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.to_ascii_lowercase());
    match name.as_deref() {
        Some("dockerfile") => "dockerfile",
        Some("makefile") | Some("gnumakefile") => "make",
        _ => "",
    }
}

fn lang_token_for_ext(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "txt" | "text" | "log" => "",
        "json" => "json",
        "jsonl" | "ndjson" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "ini" | "conf" | "cfg" | "properties" | "env" => "ini",
        "xml" | "svg" | "plist" => "xml",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" | "sass" => "scss",
        "less" => "less",
        "js" | "mjs" | "cjs" => "js",
        "jsx" => "jsx",
        "ts" => "ts",
        "tsx" => "tsx",
        "py" | "pyw" => "python",
        "rb" => "ruby",
        "php" => "php",
        "pl" | "pm" => "perl",
        "lua" => "lua",
        "go" => "go",
        "rs" => "rust",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "cs" => "c#",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "dart" => "dart",
        "scala" => "scala",
        "clj" | "cljs" | "cljc" => "clojure",
        "ex" | "exs" => "elixir",
        "erl" | "hrl" => "erlang",
        "hs" => "haskell",
        "ml" | "mli" => "ocaml",
        "sh" | "bash" | "zsh" | "fish" => "bash",
        "ps1" | "psm1" => "powershell",
        "bat" | "cmd" => "batch",
        "sql" => "sql",
        "graphql" | "gql" => "",
        "csv" | "tsv" => "",
        "diff" | "patch" => "diff",
        "dockerfile" => "dockerfile",
        "mk" => "make",
        "nix" => "",
        "vim" => "vim",
        "r" => "r",
        "tex" | "ltx" => "latex",
        "rst" => "",
        _ => return None,
    })
}

/// Resolve a local link's path: try as-is, then with a `.md` extension as a
/// fallback. Returns `None` if neither variant exists.
fn resolve_local_path(p: &Path) -> Option<PathBuf> {
    if p.exists() {
        return Some(canonicalize_or(p.to_path_buf()));
    }
    if p.extension().is_none() {
        let with_md = p.with_extension("md");
        if with_md.exists() {
            return Some(canonicalize_or(with_md));
        }
    }
    None
}

fn canonicalize_or(p: PathBuf) -> PathBuf {
    std::fs::canonicalize(&p).unwrap_or(p)
}

/// Flat one-level listing of `dir`: dirs-first, then files, both sorted
/// case-insensitively. By default only browsable text files (and directories
/// that contain them) are listed, and .gitignored/hidden entries are skipped.
/// With `show_all`, every file and directory is listed, including hidden and
/// ignored ones.
fn push_children(dir: &Path, out: &mut Vec<BrowserEntry>, show_all: bool) {
    type Row = (String, PathBuf, Option<std::time::SystemTime>);
    let mut dirs: Vec<Row> = Vec::new();
    let mut files: Vec<Row> = Vec::new();
    let walker = ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(!show_all)
        .git_ignore(!show_all)
        .git_exclude(!show_all)
        .git_global(!show_all)
        .require_git(false)
        .build();
    for result in walker {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.path() == dir {
            continue;
        }
        let name = match entry.file_name().to_str() {
            Some(n) => n.to_string(),
            None => continue,
        };
        let path = entry.path().to_path_buf();
        let ft = match entry.file_type() {
            Some(f) => f,
            None => continue,
        };
        let modified = entry.metadata().ok().and_then(|m| m.modified().ok());
        if ft.is_dir() {
            if show_all || dir_has_listable(&path) {
                dirs.push((name, path, modified));
            }
        } else if ft.is_file() && (show_all || is_text_file(&path)) {
            files.push((name, path, modified));
        }
    }
    let by_name = |a: &Row, b: &Row| a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase());
    dirs.sort_by(by_name);
    files.sort_by(by_name);
    for (name, path, modified) in dirs {
        out.push(BrowserEntry {
            path,
            display: format!("{}/", name),
            kind: BrowserEntryKind::Dir,
            modified,
        });
    }
    for (name, path, modified) in files {
        out.push(BrowserEntry {
            path,
            display: name,
            kind: BrowserEntryKind::Markdown,
            modified,
        });
    }
}

/// Case-insensitive substring search across the rendered lines, mapped to
/// (line, col_start, col_end) in display-width coordinates so the highlight
/// aligns with what the user sees.
pub fn find_doc_matches(lines: &[ratatui::text::Line<'static>], query: &str) -> Vec<DocMatch> {
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    let q = query.to_ascii_lowercase();
    for (line_idx, line) in lines.iter().enumerate() {
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let text_lower = text.to_ascii_lowercase();
        let mut from = 0;
        while let Some(rel) = text_lower[from..].find(&q) {
            let abs = from + rel;
            let col_start = unicode_width::UnicodeWidthStr::width(&text[..abs]);
            let end_byte = abs + q.len();
            let col_end = unicode_width::UnicodeWidthStr::width(&text[..end_byte]);
            out.push(DocMatch {
                line: line_idx,
                col_start,
                col_end,
            });
            from = end_byte.max(abs + 1);
        }
    }
    out
}

/// Wiki-link fallback: walk `root` looking for a markdown file whose basename
/// (with or without `.md`) matches the file component of `target`. Returns the
/// first hit. Bounded depth and skips dotfiles to avoid pathological scans.
fn vault_lookup(root: &Path, target: &Path) -> Option<PathBuf> {
    let needle = target.file_name()?.to_str()?.to_string();
    let needle_md = if needle.contains('.') {
        needle.clone()
    } else {
        format!("{}.md", needle)
    };
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .max_depth(8)
        .into_iter()
        .filter_entry(|e| {
            e.file_name()
                .to_str()
                .map(|s| !(s.starts_with('.') && s != "." && s != ".."))
                .unwrap_or(true)
        })
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let name = match entry.file_name().to_str() {
            Some(n) => n,
            None => continue,
        };
        if name == needle || name == needle_md {
            return Some(canonicalize_or(entry.path().to_path_buf()));
        }
    }
    None
}

impl Reader {
    /// True while the two-pane editor owns the body: edit mode in `Split`
    /// flavor and not hidden behind the `:preview` overlay (which renders
    /// the unsaved buffer like the plain reader).
    pub fn in_split_edit(&self) -> bool {
        self.edit
            .as_ref()
            .map(|e| e.mode == EditMode::Split && !e.preview_full)
            .unwrap_or(false)
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        // Capture metadata BEFORE reading content: if a writer races us between
        // these two syscalls, our recorded mtime is older than the file's
        // actual mtime and the next watcher tick will reload. The other order
        // would silently swallow the concurrent edit.
        let last_meta = file_meta(path);
        let raw =
            std::fs::read_to_string(path).map_err(|e| anyhow!("read {}: {}", path.display(), e))?;
        let wrap_lang = if is_markdown_file(path) {
            None
        } else {
            Some(lang_token_for_path(path).to_string())
        };
        // A Marp deck opens straight into presentation mode; `p`/Esc drops back
        // to the ordinary scrolling reader. Only markdown files are decks.
        let marp = if wrap_lang.is_none() && crate::tui::marp::detect(&raw) {
            Some(MarpView {
                deck: crate::tui::marp::parse(&raw),
                slide: 0,
                present: true,
            })
        } else {
            None
        };
        Ok(Self {
            origin: ReaderOrigin::File(path.to_path_buf()),
            raw,
            rendered: None,
            scroll: 0,
            preview_scroll: 0,
            focus: None,
            hover_link: None,
            hover_checkbox: None,
            hover_heading: None,
            doc_search: None,
            edit: None,
            last_meta,
            wrap_lang,
            jsonl_expanded: HashSet::new(),
            jsonl_overlay: None,
            hover_jsonl: None,
            tables: crate::tui::links::TableExpansions::new(),
            marp,
        })
    }

    /// All focusable spans (links + checkboxes) sorted by (line, col_start).
    /// Returns `(Focus, line, col_start)` triples so callers can scroll to or
    /// highlight the focused element without re-deriving the position.
    pub fn focus_targets(&self) -> Vec<(Focus, usize, usize)> {
        let mut out: Vec<(Focus, usize, usize)> = Vec::new();
        let Some(rd) = self.rendered.as_ref() else {
            return out;
        };
        for (i, l) in rd.link_map.links.iter().enumerate() {
            out.push((Focus::Link(i), l.line, l.col_start));
        }
        for (i, c) in rd.checkbox_map.items.iter().enumerate() {
            out.push((Focus::Checkbox(i), c.line, c.col_start));
        }
        out.sort_by_key(|&(_, line, col)| (line, col));
        out
    }

    /// Resolve the current focus (if any) to its (line, col_start) position.
    pub fn focus_position(&self) -> Option<(usize, usize)> {
        let rd = self.rendered.as_ref()?;
        match self.focus? {
            Focus::Link(i) => rd.link_map.links.get(i).map(|l| (l.line, l.col_start)),
            Focus::Checkbox(i) => rd.checkbox_map.items.get(i).map(|c| (c.line, c.col_start)),
        }
    }

    /// Toggle the click-to-expand state of part of the table identified by
    /// source byte offset `id`. Forces a re-render so the new state takes
    /// effect; drops the entry entirely once nothing in the table is expanded.
    pub fn toggle_table(&mut self, id: u64, hit: crate::tui::links::TableHit) {
        use crate::tui::links::TableHit;
        let st = self.tables.entry(id).or_default();
        match hit {
            TableHit::All => st.all = !st.all,
            TableHit::Column(c) => {
                if !st.cols.remove(&c) {
                    st.cols.insert(c);
                }
            }
            TableHit::Cell(r, c) => {
                if !st.cells.remove(&(r, c)) {
                    st.cells.insert((r, c));
                }
            }
        }
        if st.is_empty() {
            self.tables.remove(&id);
        }
        self.rendered = None;
    }

    /// True while showing a Marp deck one slide at a time.
    pub fn marp_present(&self) -> bool {
        self.marp.as_ref().map(|m| m.present).unwrap_or(false)
    }

    /// Step the current slide by `delta`, clamping at the ends (no wrap — a
    /// deck has a first and a last slide). Re-renders and resets the intra-
    /// slide scroll. No-op outside presentation mode.
    pub fn slide_by(&mut self, delta: i32) {
        let Some(m) = self.marp.as_mut() else {
            return;
        };
        let last = m.deck.len().saturating_sub(1);
        let next = (m.slide as i32 + delta).clamp(0, last as i32) as usize;
        if next != m.slide {
            m.slide = next;
            self.scroll = 0;
            self.rendered = None;
        }
    }

    /// Jump to slide `idx` (clamped). Used for Home/End (first/last).
    pub fn slide_goto(&mut self, idx: usize) {
        let Some(m) = self.marp.as_mut() else {
            return;
        };
        let idx = idx.min(m.deck.len().saturating_sub(1));
        if idx != m.slide {
            m.slide = idx;
            self.scroll = 0;
            self.rendered = None;
        }
    }

    /// Toggle between slide-at-a-time presentation and the ordinary scrolling
    /// reader over the whole document. No-op for non-Marp documents.
    pub fn toggle_present(&mut self) {
        if let Some(m) = self.marp.as_mut() {
            m.present = !m.present;
            self.scroll = 0;
            self.rendered = None;
        }
    }

    /// Leave presentation mode (if in it). Returns whether it was active — lets
    /// the Esc handler consume the key only when it actually exited a deck.
    pub fn exit_present(&mut self) -> bool {
        match self.marp.as_mut() {
            Some(m) if m.present => {
                m.present = false;
                self.scroll = 0;
                self.rendered = None;
                true
            }
            _ => false,
        }
    }

    /// Build a reader over a fetched HackMD note. Sits next to `from_file`;
    /// the origin carries the metadata cloud actions need.
    pub fn from_cloud(note: &crate::types::SingleNote, etag: Option<String>) -> Self {
        let mut r = Self::from_string(note.content.clone());
        r.origin = ReaderOrigin::CloudNote {
            id: note.id.clone(),
            title: note.title.clone(),
            team_path: note.team_path.clone(),
            publish_link: note.publish_link.clone(),
            read_permission: note.read_permission,
            etag,
        };
        r
    }

    /// Placeholder shown while a history navigation refetches an uncached
    /// note; `apply_cloud_msg` swaps the real content in when it lands.
    pub fn cloud_placeholder(id: String, title: String) -> Self {
        let mut r = Self::from_string(format!("# {title}\n\n*Fetching from hackmd.io…*\n"));
        r.origin = ReaderOrigin::CloudNote {
            id,
            title,
            team_path: None,
            publish_link: String::new(),
            read_permission: crate::types::NotePermissionRole::Owner,
            etag: None,
        };
        r
    }

    pub fn from_string(raw: String) -> Self {
        Self {
            origin: ReaderOrigin::Stdin,
            raw,
            rendered: None,
            scroll: 0,
            preview_scroll: 0,
            focus: None,
            hover_link: None,
            hover_checkbox: None,
            hover_heading: None,
            doc_search: None,
            edit: None,
            last_meta: None,
            wrap_lang: None,
            jsonl_expanded: HashSet::new(),
            jsonl_overlay: None,
            hover_jsonl: None,
            tables: crate::tui::links::TableExpansions::new(),
            marp: None,
        }
    }

    /// True for `.json` / `.jsonl` / `.ndjson` files. Drives per-line expand
    /// affordance.
    pub fn is_jsonl_view(&self) -> bool {
        self.wrap_lang.as_deref() == Some("json")
    }

    /// For a JSON-line file, build the transformed source the markdown
    /// renderer should consume. Returns `(source, code_line_to_source_line)`
    /// where each entry in the vec maps a content line index (within the
    /// emitted fenced block, top-to-bottom) back to its origin line index in
    /// `self.raw`. Expanded source lines explode into multiple content lines
    /// — all sharing the same origin index, so a click on any of their
    /// rendered rows targets the same source line.
    fn jsonl_render_source(&self) -> (String, Vec<usize>) {
        let lang = self.wrap_lang.as_deref().unwrap_or("");
        let mut out = String::with_capacity(self.raw.len() + lang.len() + 16);
        let mut map: Vec<usize> = Vec::new();
        out.push_str("```");
        out.push_str(lang);
        out.push('\n');
        for (idx, line) in self.raw.split('\n').enumerate() {
            // `split('\n')` keeps a trailing empty element when raw ends in
            // `\n` — emit that empty line too so positions stay aligned.
            if self.jsonl_expanded.contains(&idx) {
                if let Some(pretty) = jsonl::prettify(line) {
                    for pl in &pretty {
                        out.push_str(pl);
                        out.push('\n');
                        map.push(idx);
                    }
                    continue;
                }
            }
            out.push_str(line);
            out.push('\n');
            map.push(idx);
        }
        out.push_str("```\n");
        (out, map)
    }

    /// Toggle the expanded state of source `line`. If collapsing, the entry
    /// is removed; if expanding, the line is parsed first and the toggle is
    /// rejected (returning `Err`) when the line isn't valid JSON.
    pub fn toggle_jsonl_line(&mut self, line: usize) -> std::result::Result<bool, &'static str> {
        if self.jsonl_expanded.remove(&line) {
            self.rendered = None;
            return Ok(false);
        }
        let raw_line = self.raw.split('\n').nth(line).unwrap_or("");
        if jsonl::prettify(raw_line).is_none() {
            return Err("Line is not valid JSON");
        }
        self.jsonl_expanded.insert(line);
        self.rendered = None;
        Ok(true)
    }

    /// Text the markdown renderer should parse. For markdown files this is
    /// the file content verbatim. For other text files we wrap in a fenced
    /// code block so syntect highlights it through the existing pipeline.
    pub fn render_source(&self) -> std::borrow::Cow<'_, str> {
        match &self.wrap_lang {
            None => std::borrow::Cow::Borrowed(&self.raw),
            Some(lang) => {
                let mut s = String::with_capacity(self.raw.len() + lang.len() + 10);
                s.push_str("```");
                s.push_str(lang);
                s.push('\n');
                s.push_str(&self.raw);
                if !self.raw.ends_with('\n') {
                    s.push('\n');
                }
                s.push_str("```\n");
                std::borrow::Cow::Owned(s)
            }
        }
    }
}

/// Run `git diff HEAD -- <path>` (which combines staged + unstaged changes
/// against the last commit) and return the raw output. Errors surface as
/// strings so the statusline can show them.
fn run_git_diff(path: &Path) -> std::result::Result<String, String> {
    use std::process::Command;
    // Run from the file's directory so `git` finds the right repo.
    let dir = path.parent().unwrap_or(Path::new("."));
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("--no-pager")
        .arg("diff")
        .arg("--no-color")
        .arg("HEAD")
        .arg("--")
        .arg(path)
        .output()
        .map_err(|e| format!("spawn: {}", e))?;
    if !output.status.success() {
        // `git diff` returns 0 with no output when there are no changes.
        // A non-zero exit means a real failure (not in a repo, etc.).
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if err.is_empty() {
            format!("exit {}", output.status)
        } else {
            err
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parse a unified diff into typed rows. The parser is deliberately small:
/// line-prefix dispatch, no detection of binary diffs / mode changes /
/// rename headers beyond bucketing them as `Header`. That's plenty for a
/// quick visual lens.
fn parse_unified_diff(diff: &str) -> Vec<DiffRow> {
    let mut out = Vec::with_capacity(diff.lines().count());
    for line in diff.lines() {
        let kind = if line.starts_with("@@") {
            DiffRowKind::Hunk
        } else if line.starts_with("+++")
            || line.starts_with("---")
            || line.starts_with("diff ")
            || line.starts_with("index ")
            || line.starts_with("new file")
            || line.starts_with("deleted file")
            || line.starts_with("rename ")
            || line.starts_with("similarity ")
        {
            DiffRowKind::Header
        } else if line.starts_with('+') {
            DiffRowKind::Added
        } else if line.starts_with('-') {
            DiffRowKind::Removed
        } else {
            DiffRowKind::Context
        };
        out.push(DiffRow {
            kind,
            text: line.to_string(),
        });
    }
    out
}

/// Outcome of inspecting the current editor line for list auto-continuation.
enum ListContinue {
    /// The line is a non-empty list item; Enter should insert a newline plus
    /// this already-rendered marker string (indent + marker + trailing space).
    Marker(String),
    /// The line is an *empty* list item (marker only); Enter should terminate
    /// the list by clearing the line instead of adding another marker.
    Empty,
}

/// If `s` begins with a markdown checkbox (`[ ]`, `[x]`, `[X]`) followed by at
/// least one space, return the byte length consumed (the brackets plus the
/// trailing spaces). `None` otherwise.
fn checkbox_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.len() >= 3 && b[0] == b'[' && b[2] == b']' && matches!(b[1], b' ' | b'x' | b'X') {
        let after = &s[3..];
        let sp = after.len() - after.trim_start_matches(' ').len();
        if sp >= 1 {
            return Some(3 + sp);
        }
    }
    None
}

/// The next letter in an alphabetic ordered list (`a`→`b`, `A`→`B`). At `z`/`Z`
/// it stays put rather than overflowing past the alphabet.
fn next_alpha(c: char) -> char {
    match c {
        'z' | 'Z' => c,
        _ if c.is_ascii_alphabetic() => (c as u8 + 1) as char,
        _ => c,
    }
}

/// Split a leading ordered-list marker (`12.`, `3)`) off `s`, returning its
/// number, its separator, and the byte length of the digits. `None` when `s`
/// doesn't start with digits followed by `.`/`)`, or when the number is too
/// large for a `u64`.
fn ordered_marker(s: &str) -> Option<(u64, char, usize)> {
    let digits = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if digits == 0 {
        return None;
    }
    let sep = s[digits..]
        .chars()
        .next()
        .filter(|c| *c == '.' || *c == ')')?;
    Some((s[..digits].parse().ok()?, sep, digits))
}

/// Byte edits that renumber the ordered-list items following a freshly
/// inserted one, so the sequence stays continuous. `from` is the offset of the
/// line after the inserted item; `indent`/`sep` describe the list being edited
/// and `expected` is the number the first following item should take.
///
/// Only a run that was already consecutive gets shifted: each item must carry
/// the number it would have had before the insertion (`expected - 1`), so a
/// deliberately odd list (`1.` `1.` `1.`, or `1.` `5.` `9.`) is left alone.
/// More deeply indented lines (nested lists, continuation paragraphs) are
/// skipped without consuming a number. The scan stops at anything that ends
/// the list: a shallower or differently indented line, a non-numbered line, or
/// a second consecutive blank line.
///
/// Returns `(start, end, replacement)` triples over the digits of each item,
/// in ascending order.
fn renumber_edits(
    raw: &str,
    from: usize,
    indent: &str,
    sep: char,
    mut expected: u64,
) -> Vec<(usize, usize, String)> {
    let mut edits = Vec::new();
    let mut start = from;
    let mut blank = false;
    while start < raw.len() {
        let end = raw[start..]
            .find('\n')
            .map(|i| start + i)
            .unwrap_or(raw.len());
        let line = &raw[start..end];
        let line_start = start;
        start = end + 1;
        if line.trim().is_empty() {
            // One blank line still sits inside a loose list; two end it.
            if blank {
                break;
            }
            blank = true;
            continue;
        }
        blank = false;
        let ind_len = line
            .find(|c: char| c != ' ' && c != '\t')
            .unwrap_or(line.len());
        let (ind, rest) = line.split_at(ind_len);
        if ind.len() > indent.len() && ind.starts_with(indent) {
            continue; // Nested item or continuation paragraph, not ours.
        }
        if ind != indent {
            break;
        }
        let Some((num, s, digits)) = ordered_marker(rest) else {
            break;
        };
        // Require a space after the marker, same as `list_continuation`.
        if s != sep || !rest[digits + 1..].starts_with(' ') || num.checked_add(1) != Some(expected)
        {
            break;
        }
        let num_start = line_start + ind_len;
        edits.push((num_start, num_start + digits, expected.to_string()));
        expected += 1;
    }
    edits
}

/// Inspect a full source `line` (no trailing newline) for a markdown list
/// marker, deciding how Enter should behave. `cursor_at_end` is whether the
/// edit cursor sits at the line's end — an empty item only terminates the
/// list when the cursor is there (otherwise Enter just splits the line).
///
/// Recognises unordered bullets (`-`/`*`/`+`), task checkboxes
/// (`- [ ]`/`- [x]`), numbered lists (`1.`/`1)`), and single-letter alphabetic
/// lists (`a.`/`A)`). The continued marker normalises to one trailing space;
/// numbered/alpha markers advance by one; new checkboxes start unchecked.
fn list_continuation(line: &str, cursor_at_end: bool) -> Option<ListContinue> {
    let indent_len = line
        .find(|c: char| c != ' ' && c != '\t')
        .unwrap_or(line.len());
    let (indent, rest) = line.split_at(indent_len);
    let count_spaces = |s: &str| s.len() - s.trim_start_matches(' ').len();
    let empty_or = |content: &str, marker: String| {
        if content.trim().is_empty() && cursor_at_end {
            ListContinue::Empty
        } else {
            ListContinue::Marker(marker)
        }
    };

    let first = rest.chars().next()?;

    // Unordered bullets and task checkboxes.
    if matches!(first, '-' | '*' | '+') {
        let after = &rest[1..];
        let sp = count_spaces(after);
        if sp == 0 {
            return None; // e.g. "-5" or "*bold*" — not a list item.
        }
        let after_sp = &after[sp..];
        if let Some(cb) = checkbox_len(after_sp) {
            let content = &after_sp[cb..];
            return Some(empty_or(content, format!("{indent}{first} [ ] ")));
        }
        return Some(empty_or(after_sp, format!("{indent}{first} ")));
    }

    // Numbered lists: one or more digits, then `.` or `)`, then a space.
    if rest.starts_with(|c: char| c.is_ascii_digit()) {
        let (num, sep, digits) = ordered_marker(rest)?;
        let after_sep = &rest[digits + 1..];
        let sp = count_spaces(after_sep);
        if sp == 0 {
            return None;
        }
        let content = &after_sep[sp..];
        let next = num.saturating_add(1);
        return Some(empty_or(content, format!("{indent}{next}{sep} ")));
    }

    // Alphabetic ordered lists: a single letter, then `.` or `)`, then a space.
    if first.is_ascii_alphabetic() {
        let after = &rest[first.len_utf8()..];
        let sep = after.chars().next().filter(|c| *c == '.' || *c == ')')?;
        let after_sep = &after[1..];
        let sp = count_spaces(after_sep);
        if sp == 0 {
            return None;
        }
        let content = &after_sep[sp..];
        let next = next_alpha(first);
        return Some(empty_or(content, format!("{indent}{next}{sep} ")));
    }

    None
}

/// Snapshot the current (raw, cursor) into the reader's undo stack.
/// Clears the redo stack since a new mutation diverges the timeline.
/// Caps the undo stack at `UNDO_LIMIT` entries (FIFO eviction).
pub(crate) fn push_undo(r: &mut Reader) {
    let Some(e) = r.edit.as_mut() else { return };
    e.undo.push(EditSnapshot {
        raw: r.raw.clone(),
        cursor: e.cursor,
    });
    if e.undo.len() > UNDO_LIMIT {
        e.undo.remove(0);
    }
    e.redo.clear();
}

/// Byte range of the source line containing `pos`: from just after the
/// previous newline (or the start) to just after the next newline (or the
/// end), so the trailing `\n` is included when there is one.
fn line_bounds_with_newline(s: &str, pos: usize) -> (usize, usize) {
    let pos = pos.min(s.len());
    let start = s[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = s[start..]
        .find('\n')
        .map(|i| start + i + 1)
        .unwrap_or(s.len());
    (start, end)
}

/// If the edit cursor sits inside a markdown link destination `](…)` with a
/// `#` before it, return `(hash_offset, path)` where `hash_offset` is the byte
/// offset of that `#` in `raw` and `path` is the destination text between the
/// opening `(` and the `#` (the referenced file, empty for the current doc).
/// The query being typed is `raw[hash_offset + 1 .. cursor]`. Returns `None`
/// when the cursor isn't in such a destination.
///
/// `cursor` must be a char boundary. Detection is delimiter-based on ASCII
/// bytes, so it never splits a multi-byte char.
fn anchor_context(raw: &str, cursor: usize) -> Option<(usize, &str)> {
    let bytes = raw.as_bytes();
    // Walk back to the enclosing '('. A ')' or newline first means we're not
    // inside an open destination.
    let mut i = cursor;
    let open = loop {
        if i == 0 {
            return None;
        }
        i -= 1;
        match bytes[i] {
            b')' | b'\n' => return None,
            b'(' => break i,
            _ => {}
        }
    };
    // The '(' must be immediately preceded by ']' — otherwise it's a bare
    // parenthesis, not a link destination.
    if open == 0 || bytes[open - 1] != b']' {
        return None;
    }
    // Anchor starts at the first '#' inside the destination-so-far.
    let dest = &raw[open + 1..cursor];
    let hash_rel = dest.find('#')?;
    let hash = open + 1 + hash_rel;
    let path = &raw[open + 1..hash];
    let query = &raw[hash + 1..cursor];
    // A space (link title) or stray '#' in the query means this isn't a plain
    // path#anchor destination we should complete.
    if path.bytes().any(|b| b == b' ' || b == b'\t')
        || query.bytes().any(|b| b == b' ' || b == b'\t' || b == b'#')
    {
        return None;
    }
    Some((hash, path))
}

/// Build the anchor candidates for a link destination `path`. Empty `path`
/// means the current buffer; otherwise the path is resolved (relative to the
/// current file, then the vault root) and that markdown file's headings are
/// read from disk. Returns an empty list for non-markdown or missing targets.
fn build_anchor_candidates(
    r: &Reader,
    root: &Path,
    path: &str,
) -> Vec<crate::tui::links::DocHeading> {
    use crate::tui::links::{self, LinkTarget};
    if path.is_empty() {
        return links::extract_headings(&r.raw);
    }
    let base_dir = match &r.origin {
        ReaderOrigin::File(p) => p.parent().map(|p| p.to_path_buf()),
        _ => None,
    };
    let file = match links::resolve(path, base_dir.as_deref()) {
        LinkTarget::LocalFile(p) => resolve_local_path(&p).or_else(|| vault_lookup(root, &p)),
        _ => None,
    };
    let Some(file) = file else {
        return Vec::new();
    };
    if !is_markdown_file(&file) {
        return Vec::new();
    }
    match std::fs::read_to_string(&file) {
        Ok(content) => links::extract_headings(&content),
        Err(_) => Vec::new(),
    }
}

/// Snap `pos` down to the nearest UTF-8 char boundary <= pos.
fn floor_char_boundary(s: &str, pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    let mut p = pos;
    while p > 0 && !s.is_char_boundary(p) {
        p -= 1;
    }
    p
}

/// Byte offset of the next char boundary strictly after `pos`. Returns
/// `s.len()` if `pos` is already at end.
pub(crate) fn next_char_boundary(s: &str, pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    let mut p = pos + 1;
    while p < s.len() && !s.is_char_boundary(p) {
        p += 1;
    }
    p
}

/// Byte offset of the previous char boundary strictly before `pos`. Returns
/// 0 if `pos` is already at the start.
fn prev_char_boundary(s: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let mut p = pos - 1;
    while p > 0 && !s.is_char_boundary(p) {
        p -= 1;
    }
    p
}

/// Convert a byte offset to (line index, char column within line). Lines
/// are split on `\n`; CR is ignored. Column is in chars, not display width.
fn source_line_col(s: &str, pos: usize) -> (usize, usize) {
    let pos = pos.min(s.len());
    let head = &s[..pos];
    let line = head.bytes().filter(|&b| b == b'\n').count();
    let col_bytes = head.rfind('\n').map(|i| pos - i - 1).unwrap_or(pos);
    let col = s[pos - col_bytes..pos].chars().count();
    (line, col)
}

/// Byte offset of the first char of `line`. Out-of-range lines return
/// `s.len()`.
fn source_line_start(s: &str, line: usize) -> usize {
    if line == 0 {
        return 0;
    }
    let mut count = 0;
    for (i, b) in s.bytes().enumerate() {
        if b == b'\n' {
            count += 1;
            if count == line {
                return i + 1;
            }
        }
    }
    s.len()
}

/// Byte offset of the last char of `line` (i.e. the position just before
/// the trailing `\n`, or `s.len()` for the last line).
fn source_line_end(s: &str, line: usize) -> usize {
    let start = source_line_start(s, line);
    s[start..].find('\n').map(|i| start + i).unwrap_or(s.len())
}

// ---------------------------------------------------------------------------
// Split-screen edit mode: raw-pane rendering and pane-to-pane scroll sync.
// ---------------------------------------------------------------------------

/// Visual kind of a raw-pane source line. Computed once per source line
/// and copied onto every wrapped row, so continuation rows keep the same
/// styling as the first row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawRowKind {
    Normal,
    Heading,
    Quote,
}

/// One wrapped row of the raw pane. Pure plain text plus the source byte
/// range it covers, so click→cursor and cursor→display-row mappings are
/// trivial in either direction.
#[derive(Clone, Debug)]
pub struct RawRow {
    pub text: String,
    pub source_range: std::ops::Range<usize>,
    pub kind: RawRowKind,
    /// 1-based source line number, set only on the *head* row of each source
    /// line. Wrapped continuation rows carry `None` so the gutter shows a
    /// number once per logical line, like every other editor.
    pub line_no: Option<usize>,
}

/// Rows of blank breathing room added above the first and below the last
/// line of the raw editor pane. It lives in the scrollable page (not as a
/// fixed viewport overlay) and doubles as a scroll margin (scrolloff), so the
/// document top shows a gap, the bottom never feels like writing into a wall,
/// and the top gap lines up with the preview pane's leading blank.
pub const EDIT_PAD: usize = 1;

/// Total scrollable height of the raw pane for `n` wrapped rows, counting the
/// top and bottom pad. The raw-pane scroll offset (`Reader::scroll` in split
/// edit) is an index into this padded space.
pub fn raw_scroll_span(n: usize) -> usize {
    n + 2 * EDIT_PAD
}

/// Wrap `raw` to `width` columns, producing one `RawRow` per display row.
/// Splits on `\n` first (preserving the implicit empty row at the end of
/// the buffer so the cursor can land just after a trailing newline), then
/// soft-wraps each source line via the same word-aware wrapper used by
/// the markdown renderer.
pub fn render_raw_pane(raw: &str, width: usize) -> Vec<RawRow> {
    let mut rows: Vec<RawRow> = Vec::new();
    let inner_width = width.max(1);
    let mut byte = 0usize;
    // Iterate `\n`-delimited source lines; `split('\n')` yields the trailing
    // empty if `raw` ends with `\n`, which gives us the empty row at EOF.
    let mut lines: Vec<&str> = raw.split('\n').collect();
    if raw.is_empty() {
        lines = vec![""];
    }
    for (i, line) in lines.iter().enumerate() {
        let line_start = byte;
        let line_len = line.len();
        let stripped = line.strip_suffix('\r').unwrap_or(line);
        let trimmed = stripped.trim_start();
        let kind = if trimmed.starts_with('#') {
            RawRowKind::Heading
        } else if trimmed.starts_with('>') {
            RawRowKind::Quote
        } else {
            RawRowKind::Normal
        };
        let chunks = markdown::wrap_to_width_pub(stripped, inner_width);
        let chunks: Vec<(std::ops::Range<usize>, String)> = if chunks.is_empty() {
            vec![(0..0, String::new())]
        } else {
            chunks
        };
        for (ci, (chunk_range, chunk_text)) in chunks.into_iter().enumerate() {
            let src_start = line_start + chunk_range.start;
            let src_end = line_start + chunk_range.end;
            rows.push(RawRow {
                text: chunk_text,
                source_range: src_start..src_end,
                kind,
                // Number only the first display row of each source line.
                line_no: (ci == 0).then_some(i + 1),
            });
        }
        // Advance past `\n` between lines (but not after the last entry,
        // which terminates the iteration cleanly).
        if i + 1 < lines.len() {
            byte = line_start + line_len + 1;
        } else {
            byte = line_start + line_len;
        }
    }
    rows
}

/// Find the raw-pane row index containing `cursor` (or the last row when
/// the cursor sits at EOF).
pub fn raw_row_for_cursor(rows: &[RawRow], cursor: usize) -> usize {
    if rows.is_empty() {
        return 0;
    }
    for (i, row) in rows.iter().enumerate() {
        // A cursor on the boundary between two rows belongs to the *next*
        // row when there's a wrap-break (no `\n`); but the first row that
        // spans `cursor` works for both wrapped and `\n`-delimited cases
        // because the previous row's `source_range.end` equals the next
        // row's `source_range.start`.
        if cursor >= row.source_range.start && cursor <= row.source_range.end {
            // Prefer the *first* row containing the position when at a
            // boundary, except at end of the row when the next row starts
            // at the same position (wrap break) — then bump to the next.
            let at_end = cursor == row.source_range.end;
            let next_starts_here = rows
                .get(i + 1)
                .map(|nr| nr.source_range.start == cursor)
                .unwrap_or(false);
            if at_end && next_starts_here {
                return i + 1;
            }
            return i;
        }
    }
    rows.len() - 1
}

/// Display column of `cursor` within its row. Walks the row's source slice
/// by char width.
pub fn raw_col_for_cursor(raw: &str, row: &RawRow, cursor: usize) -> u16 {
    use unicode_width::UnicodeWidthChar;
    let start = row.source_range.start;
    let end = row.source_range.end.min(cursor);
    if cursor < start {
        return 0;
    }
    let slice = match raw.get(start..end) {
        Some(s) => s,
        None => return 0,
    };
    slice.chars().map(|c| c.width().unwrap_or(0)).sum::<usize>() as u16
}

/// Map a (row, col) click in the raw pane to a source byte offset.
pub fn raw_click_to_source(rows: &[RawRow], raw: &str, row: usize, col: usize) -> usize {
    use unicode_width::UnicodeWidthChar;
    let Some(r) = rows.get(row) else {
        return rows.last().map(|x| x.source_range.end).unwrap_or(0);
    };
    let slice = raw.get(r.source_range.clone()).unwrap_or("");
    let mut taken = 0usize;
    for (i, ch) in slice.char_indices() {
        let w = ch.width().unwrap_or(0);
        if taken + w > col {
            return r.source_range.start + i;
        }
        taken += w;
    }
    r.source_range.end
}

/// Map a source byte offset to a preview-pane row index. Uses
/// `Rendered::blocks` for block-level alignment, then linearly interpolates
/// inside the block by source position.
pub fn preview_row_for_source(rendered: &Rendered, cursor: usize) -> usize {
    // Find the block whose source range contains the cursor.
    for b in &rendered.blocks {
        if cursor >= b.source_range.start && cursor < b.source_range.end {
            let span = b
                .source_range
                .end
                .saturating_sub(b.source_range.start)
                .max(1);
            let off = cursor.saturating_sub(b.source_range.start);
            let h = b.display_end.saturating_sub(b.display_start);
            let row_in_block = (off * h) / span;
            return b.display_start + row_in_block;
        }
    }
    // Fall back: nearest block at or before the cursor.
    let mut best: usize = 0;
    for b in &rendered.blocks {
        if b.source_range.start <= cursor {
            best = b.display_end.saturating_sub(1);
        } else {
            break;
        }
    }
    best
}

/// Reverse of `preview_row_for_source`: given a preview row, find a
/// representative source byte offset for the block containing that row.
pub fn source_for_preview_row(rendered: &Rendered, row: usize) -> usize {
    for b in &rendered.blocks {
        if row >= b.display_start && row < b.display_end {
            let span = b.source_range.end.saturating_sub(b.source_range.start);
            let h = b.display_end.saturating_sub(b.display_start).max(1);
            let off_in_block = row - b.display_start;
            return b.source_range.start + (off_in_block * span) / h;
        }
    }
    0
}

/// Byte offset of the next word boundary after `pos`. Skips through any
/// whitespace, then through the next non-whitespace run, landing at the
/// edge or `s.len()`. Matches macOS-native Alt-Right semantics.
fn next_word_boundary(s: &str, pos: usize) -> usize {
    let pos = pos.min(s.len());
    let mut i = pos;
    let len = s.len();
    while i < len {
        let ch = s[i..].chars().next().unwrap();
        if !ch.is_whitespace() {
            break;
        }
        i += ch.len_utf8();
    }
    while i < len {
        let ch = s[i..].chars().next().unwrap();
        if ch.is_whitespace() {
            break;
        }
        i += ch.len_utf8();
    }
    i
}

/// Byte offset of the previous word boundary before `pos`. Symmetric
/// counterpart to `next_word_boundary`.
fn prev_word_boundary(s: &str, pos: usize) -> usize {
    let mut i = pos.min(s.len());
    while i > 0 {
        let prev = s[..i].chars().next_back().unwrap();
        if !prev.is_whitespace() {
            break;
        }
        i -= prev.len_utf8();
    }
    while i > 0 {
        let prev = s[..i].chars().next_back().unwrap();
        if prev.is_whitespace() {
            break;
        }
        i -= prev.len_utf8();
    }
    i
}

/// Walk source[range] by display-column width and return the byte offset
/// where the cursor should land for `col` columns. Used by edit-mode
/// vertical movement so a soft-wrapped paragraph steps display-row by
/// display-row, not source-line by source-line.
fn source_offset_at_col(s: &str, range: &std::ops::Range<usize>, col: usize) -> usize {
    use unicode_width::UnicodeWidthChar;
    let Some(slice) = s.get(range.clone()) else {
        return range.start;
    };
    let mut taken = 0usize;
    for (i, ch) in slice.char_indices() {
        let w = ch.width().unwrap_or(0);
        if taken + w > col {
            return range.start + i;
        }
        taken += w;
    }
    range.end
}

/// Byte offset of `col` chars into `line`. Clamps if the line is shorter.
fn source_offset_for(s: &str, line: usize, col: usize) -> usize {
    let start = source_line_start(s, line);
    let end = source_line_end(s, line);
    let line_str = &s[start..end];
    let mut taken = 0usize;
    let mut last = start;
    for (i, _ch) in line_str.char_indices() {
        if taken == col {
            return start + i;
        }
        taken += 1;
        last = start
            + i
            + line_str[i..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(1);
    }
    last.min(end)
}

/// Cheap stat read; returns `None` if the file is gone or unstatable. Called
/// every event-loop tick — must not allocate or do anything beyond a single
/// `metadata` syscall (kernel serves this from the inode cache).
/// Resolve `.` and `..` lexically, without touching the filesystem.
///
/// `std::fs::canonicalize` can't be used for a move destination (it requires
/// the path to exist) and would also resolve symlinks, which would make a move
/// land somewhere other than where the user typed. A purely textual clean-up
/// is what's needed here: it keeps `starts_with` checks and status messages
/// honest for inputs like `../notes/x.md`. Leading `..` that would climb past
/// the start of a relative path are preserved.
pub fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                // Only pop a real name; `..` after a root or another `..` has
                // nothing to cancel out and has to stay.
                let pops = out
                    .components()
                    .next_back()
                    .is_some_and(|last| matches!(last, Component::Normal(_)));
                if pops {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

pub fn file_meta(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let md = std::fs::metadata(path).ok()?;
    Some((md.modified().ok()?, md.len()))
}

impl Browser {
    /// Build a flat one-level listing of `dir`. The first entry is `..` when
    /// `dir` has a parent (so users who launched directly into a sub-tree can
    /// still walk up); below that, dirs come before markdown files, both
    /// sorted case-insensitively. .gitignored entries are skipped.
    pub fn scan(dir: &Path) -> Result<Self> {
        let mut b = Self {
            dir: dir.to_path_buf(),
            entries: Vec::new(),
            selected: 0,
            scroll: 0,
            last_meta: None,
            find: None,
            show_all: false,
            sort_by_modified: false,
            child_dirs: Vec::new(),
            jump_labels: None,
        };
        b.rebuild()?;
        Ok(b)
    }

    /// Rebuild `entries` from `dir`. Preserves the highlighted path across
    /// rebuilds when possible.
    pub fn rebuild(&mut self) -> Result<()> {
        let prev_selected_path = self.entries.get(self.selected).map(|e| e.path.clone());
        let mut entries = Vec::new();
        if let Some(parent) = self.dir.parent() {
            if parent != self.dir {
                entries.push(BrowserEntry {
                    path: parent.to_path_buf(),
                    display: "../".to_string(),
                    kind: BrowserEntryKind::ParentDir,
                    modified: None,
                });
            }
        }
        push_children(&self.dir, &mut entries, self.show_all);
        if self.sort_by_modified {
            // Reorder by mtime, newest first, but keep a leading `../` pinned at
            // the top. `None` mtimes sort last; ties fall back to name order.
            let start = usize::from(matches!(
                entries.first().map(|e| e.kind),
                Some(BrowserEntryKind::ParentDir)
            ));
            entries[start..].sort_by(|a, b| {
                b.modified.cmp(&a.modified).then_with(|| {
                    a.display
                        .to_ascii_lowercase()
                        .cmp(&b.display.to_ascii_lowercase())
                })
            });
        }
        self.entries = entries;
        // Skip past `../` on a fresh listing so the cursor lands on the first
        // real entry — `Esc`/`h`/`Backspace` already covers "go up", and
        // landing on `../` makes Enter feel redundant. History-restored
        // selections set `selected` explicitly afterwards in `App::load`,
        // so this default doesn't fight that path.
        let first_real = self
            .entries
            .iter()
            .position(|e| !matches!(e.kind, BrowserEntryKind::ParentDir))
            .unwrap_or(0);
        self.selected = match prev_selected_path {
            Some(p) => self
                .entries
                .iter()
                .position(|e| e.path == p)
                .unwrap_or(first_real),
            None => first_real,
        };
        // Record the directory fingerprint this listing reflects, so the
        // tick-driven poll only rebuilds when the dir actually changes.
        self.last_meta = file_meta(&self.dir);
        self.child_dirs = child_dir_meta(&self.dir, self.show_all);
        Ok(())
    }

    /// Toggle "show everything" mode (`A`) and re-list. Preserves the
    /// highlighted entry across the rebuild when it survives the filter change.
    pub fn toggle_show_all(&mut self) {
        self.show_all = !self.show_all;
        let _ = self.rebuild();
    }

    /// Toggle name-order vs. recency-order (`s`) and re-list. Preserves the
    /// highlighted entry across the reorder when it survives.
    pub fn toggle_sort_by_modified(&mut self) {
        self.sort_by_modified = !self.sort_by_modified;
        let _ = self.rebuild();
    }

    #[allow(dead_code)]
    pub fn selected_entry(&self) -> Option<&BrowserEntry> {
        self.entries.get(self.selected)
    }

    /// Does `entry`'s display name start with `query`? Anchored prefix match,
    /// smartcase: case-insensitive unless `query` itself contains an uppercase
    /// letter (then the comparison is case-sensitive). The trailing `/` on
    /// directory rows is part of `display`, which is fine — users type the bare
    /// name and the prefix still matches.
    fn find_matches(entry: &BrowserEntry, query: &str) -> bool {
        if query.is_empty() {
            return false;
        }
        let smart = query.chars().any(|c| c.is_uppercase());
        if smart {
            entry.display.starts_with(query)
        } else {
            entry
                .display
                .to_lowercase()
                .starts_with(&query.to_lowercase())
        }
    }

    /// Index of the first entry matching `query`, searching forward from
    /// `start` and wrapping. `None` when nothing matches.
    pub fn find_from(&self, query: &str, start: usize) -> Option<usize> {
        let n = self.entries.len();
        if n == 0 {
            return None;
        }
        (0..n)
            .map(|off| (start + off) % n)
            .find(|&i| Self::find_matches(&self.entries[i], query))
    }

    /// Like [`find_from`](Self::find_from) but searching backward from `start`
    /// (wrapping). Used by find-prev (`,`).
    pub fn find_back_from(&self, query: &str, start: usize) -> Option<usize> {
        let n = self.entries.len();
        if n == 0 {
            return None;
        }
        (1..=n)
            .map(|off| (start + n - (off % n)) % n)
            .find(|&i| Self::find_matches(&self.entries[i], query))
    }
}

impl Search {
    /// Build the index by walking `root` (depth-capped, gitignore-aware).
    pub fn build(root: &Path) -> Self {
        let mut paths = Vec::new();
        let walker = ignore::WalkBuilder::new(root)
            .max_depth(Some(8))
            .hidden(true)
            .git_ignore(true)
            .git_exclude(true)
            .git_global(true)
            .require_git(false)
            .build();
        for result in walker {
            let entry = match result {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry.path() == root {
                continue;
            }
            let display = entry
                .path()
                .strip_prefix(root)
                .unwrap_or(entry.path())
                .display()
                .to_string();
            let display_lower = display.to_ascii_lowercase();
            let is_dir = entry.file_type().map(|f| f.is_dir()).unwrap_or(false);
            paths.push(IndexedPath {
                path: entry.path().to_path_buf(),
                display,
                display_lower,
                is_dir,
            });
        }
        let mut s = Self {
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            paths,
        };
        s.refresh();
        s
    }

    pub fn refresh(&mut self) {
        self.results.clear();
        let q = self.query.to_ascii_lowercase();
        if q.is_empty() {
            for ip in &self.paths {
                self.results.push(SearchResult {
                    path: ip.path.clone(),
                    display: ip.display.clone(),
                    score: 0,
                    is_dir: ip.is_dir,
                });
            }
            self.results.sort_by(|a, b| {
                b.is_dir.cmp(&a.is_dir).then(
                    a.display
                        .to_ascii_lowercase()
                        .cmp(&b.display.to_ascii_lowercase()),
                )
            });
        } else {
            for ip in &self.paths {
                if let Some(score) = score_substring(&ip.display_lower, &q) {
                    self.results.push(SearchResult {
                        path: ip.path.clone(),
                        display: ip.display.clone(),
                        score,
                        is_dir: ip.is_dir,
                    });
                }
            }
            self.results.sort_by(|a, b| {
                b.score.cmp(&a.score).then(
                    a.display
                        .to_ascii_lowercase()
                        .cmp(&b.display.to_ascii_lowercase()),
                )
            });
        }
        if self.selected >= self.results.len() {
            self.selected = 0;
        }
    }

    pub fn move_selection(&mut self, delta: i32) {
        let n = self.results.len() as i32;
        if n == 0 {
            return;
        }
        let new = ((self.selected as i32 + delta) % n + n) % n;
        self.selected = new as usize;
    }
}

/// Substring score: higher when the match starts earlier, at a word boundary,
/// or in the basename. Returns `None` if `pattern` is not a substring.
fn score_substring(text: &str, pattern: &str) -> Option<i32> {
    let idx = text.find(pattern)?;
    let mut score = 1000 - idx as i32;
    if idx == 0 {
        score += 500;
    } else if let Some(prev) = text.as_bytes().get(idx - 1) {
        if matches!(*prev as char, '/' | '_' | '-' | '.' | ' ') {
            score += 250;
        }
    }
    if let Some(slash) = text.rfind('/') {
        if idx > slash {
            score += 100;
        }
    } else {
        score += 100;
    }
    score -= text.len() as i32 / 4;
    Some(score)
}

#[cfg(test)]
mod conflict_state_tests {
    use super::{ConflictChoice, ConflictItem, ConflictState};
    use crate::tui::hackmd_meta::HackmdMeta;
    use std::path::PathBuf;

    fn state(items: Vec<ConflictItem>) -> ConflictState {
        ConflictState {
            path: PathBuf::from("/tmp/x.md"),
            id: "id1".into(),
            meta: HackmdMeta {
                id: "id1".into(),
                team_path: None,
                url: String::new(),
                publish_link: String::new(),
            },
            items,
            selected: 0,
            scroll: 0,
        }
    }

    fn conflict() -> ConflictItem {
        ConflictItem::Conflict {
            local: "LOCAL\n".into(),
            remote: "REMOTE\n".into(),
            choice: ConflictChoice::Unresolved,
        }
    }

    #[test]
    fn assemble_blocks_until_all_resolved() {
        let mut st = state(vec![
            ConflictItem::Stable("top\n".into()),
            conflict(),
            ConflictItem::Stable("bottom\n".into()),
        ]);
        assert_eq!(st.conflict_count(), 1);
        assert_eq!(st.unresolved_count(), 1);
        // Unresolved → no output.
        assert!(st.assemble().is_none());
        st.set_choice(ConflictChoice::Local);
        assert_eq!(st.unresolved_count(), 0);
        assert_eq!(st.assemble().as_deref(), Some("top\nLOCAL\nbottom\n"));
    }

    #[test]
    fn each_choice_assembles_its_side() {
        let cases = [
            (ConflictChoice::Local, "LOCAL\n"),
            (ConflictChoice::Remote, "REMOTE\n"),
            (ConflictChoice::Both, "LOCAL\nREMOTE\n"),
            (ConflictChoice::Neither, ""),
        ];
        for (choice, expected) in cases {
            let mut st = state(vec![conflict()]);
            st.set_choice(choice);
            assert_eq!(st.assemble().as_deref(), Some(expected));
        }
    }

    #[test]
    fn set_choice_and_step_target_the_selected_hunk() {
        let mut st = state(vec![
            conflict(),
            ConflictItem::Stable("mid\n".into()),
            conflict(),
        ]);
        assert_eq!(st.conflict_count(), 2);
        // Resolve first, advance, resolve second.
        st.set_choice(ConflictChoice::Local);
        st.step(1);
        assert_eq!(st.selected, 1);
        st.set_choice(ConflictChoice::Remote);
        assert_eq!(st.unresolved_count(), 0);
        assert_eq!(st.assemble().as_deref(), Some("LOCAL\nmid\nREMOTE\n"));
        // Step clamps at the ends.
        st.step(5);
        assert_eq!(st.selected, 1);
        st.step(-5);
        assert_eq!(st.selected, 0);
    }
}

#[cfg(test)]
mod list_continuation_tests {
    use super::{ListContinue, list_continuation};

    fn marker(line: &str) -> Option<String> {
        match list_continuation(line, true) {
            Some(ListContinue::Marker(m)) => Some(m),
            _ => None,
        }
    }

    #[test]
    fn bullets_continue_with_same_char_and_indent() {
        assert_eq!(marker("- item").as_deref(), Some("- "));
        assert_eq!(marker("* item").as_deref(), Some("* "));
        assert_eq!(marker("+ item").as_deref(), Some("+ "));
        assert_eq!(marker("  - nested").as_deref(), Some("  - "));
    }

    #[test]
    fn checkboxes_continue_unchecked() {
        assert_eq!(marker("- [ ] todo").as_deref(), Some("- [ ] "));
        // A checked box still spawns a fresh *unchecked* one.
        assert_eq!(marker("- [x] done").as_deref(), Some("- [ ] "));
        assert_eq!(marker("  - [X] DONE").as_deref(), Some("  - [ ] "));
    }

    #[test]
    fn numbered_lists_increment() {
        assert_eq!(marker("1. first").as_deref(), Some("2. "));
        assert_eq!(marker("9. ninth").as_deref(), Some("10. "));
        assert_eq!(marker("3) paren").as_deref(), Some("4) "));
        assert_eq!(marker("  2. nested").as_deref(), Some("  3. "));
    }

    #[test]
    fn alpha_lists_advance() {
        assert_eq!(marker("a. apple").as_deref(), Some("b. "));
        assert_eq!(marker("A) Apple").as_deref(), Some("B) "));
    }

    #[test]
    fn empty_item_terminates_when_cursor_at_end() {
        assert!(matches!(
            list_continuation("- ", true),
            Some(ListContinue::Empty)
        ));
        assert!(matches!(
            list_continuation("1. ", true),
            Some(ListContinue::Empty)
        ));
        assert!(matches!(
            list_continuation("- [ ] ", true),
            Some(ListContinue::Empty)
        ));
        // Mid-line cursor: an empty marker still continues rather than clears.
        assert!(matches!(
            list_continuation("- ", false),
            Some(ListContinue::Marker(_))
        ));
    }

    #[test]
    fn non_list_lines_are_plain_newlines() {
        assert!(list_continuation("just prose", true).is_none());
        assert!(list_continuation("-no space", true).is_none());
        assert!(list_continuation("*emphasis*", true).is_none());
        assert!(list_continuation("", true).is_none());
        // "e.g." has no space after the dot, so it isn't a marker.
        assert!(list_continuation("e.g whatever", true).is_none());
    }
}

#[cfg(test)]
mod renumber_tests {
    use super::renumber_edits;

    /// Renumber the items below `marker_line` (0-based) in `raw`, mimicking
    /// what `edit_renumber_below` does after Enter inserted that line.
    fn renumber(raw: &str, marker_line: usize) -> String {
        let mut offset = 0;
        for _ in 0..=marker_line {
            offset += raw[offset..].find('\n').unwrap() + 1;
        }
        let line = raw.lines().nth(marker_line).unwrap();
        let ind = line.find(|c: char| c != ' ').unwrap_or(0);
        let (num, sep, _) = super::ordered_marker(&line[ind..]).unwrap();
        let edits = renumber_edits(raw, offset, &line[..ind], sep, num + 1);
        let mut out = raw.to_string();
        for (start, end, text) in edits.into_iter().rev() {
            out.replace_range(start..end, &text);
        }
        out
    }

    #[test]
    fn following_items_shift_up_by_one() {
        let raw = "1. a\n2. \n2. b\n3. c\n4. d\n";
        assert_eq!(renumber(raw, 1), "1. a\n2. \n3. b\n4. c\n5. d\n");
    }

    #[test]
    fn multi_digit_rollover_rewrites_widening_markers() {
        let raw = "8. a\n9. \n9. b\n10. c\n";
        assert_eq!(renumber(raw, 1), "8. a\n9. \n10. b\n11. c\n");
    }

    #[test]
    fn stops_at_the_end_of_the_list() {
        // Prose after the list is untouched.
        let raw = "1. a\n2. \n2. b\nplain text\n3. later\n";
        assert_eq!(renumber(raw, 1), "1. a\n2. \n3. b\nplain text\n3. later\n");
        // So is a bullet list.
        let raw = "1. a\n2. \n2. b\n- bullet\n";
        assert_eq!(renumber(raw, 1), "1. a\n2. \n3. b\n- bullet\n");
        // And a different separator.
        let raw = "1. a\n2. \n2) b\n";
        assert_eq!(renumber(raw, 1), raw);
    }

    #[test]
    fn deliberately_odd_numbering_is_left_alone() {
        // An all-`1.` list is a legitimate markdown style.
        let raw = "1. a\n2. \n1. b\n1. c\n";
        assert_eq!(renumber(raw, 1), raw);
        // So is a list whose numbers were never consecutive.
        let raw = "1. a\n2. \n5. b\n9. c\n";
        assert_eq!(renumber(raw, 1), raw);
        // A run that goes off-sequence stops there, keeping the tail intact.
        let raw = "1. a\n2. \n2. b\n7. c\n4. d\n";
        assert_eq!(renumber(raw, 1), "1. a\n2. \n3. b\n7. c\n4. d\n");
    }

    #[test]
    fn nested_items_and_continuations_are_skipped() {
        let raw = "1. a\n2. \n   - sub\n   more text\n2. b\n";
        assert_eq!(
            renumber(raw, 1),
            "1. a\n2. \n   - sub\n   more text\n3. b\n"
        );
        // Nested numbering renumbers within its own indent level.
        let raw = "1. top\n   1. a\n   2. \n   2. b\n2. next\n";
        assert_eq!(
            renumber(raw, 2),
            "1. top\n   1. a\n   2. \n   3. b\n2. next\n"
        );
    }

    #[test]
    fn one_blank_line_stays_inside_a_loose_list_two_end_it() {
        let raw = "1. a\n2. \n\n2. b\n";
        assert_eq!(renumber(raw, 1), "1. a\n2. \n\n3. b\n");
        let raw = "1. a\n2. \n\n\n2. b\n";
        assert_eq!(renumber(raw, 1), raw);
    }

    #[test]
    fn last_line_without_a_trailing_newline_is_renumbered() {
        let raw = "1. a\n2. \n2. b";
        assert_eq!(renumber(raw, 1), "1. a\n2. \n3. b");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::theme::Theme;

    /// Per-test temp dir under the system temp root. Cleared on entry so a
    /// previous failure doesn't leave stale state behind.
    fn fresh_temp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("md-tui-test-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn opts() -> Options {
        Options {
            width: 80,
            line_numbers: false,
            theme: Theme::dark(),
        }
    }

    #[test]
    fn raw_rows_number_source_lines_once_across_wraps() {
        // Line 1 wraps into several display rows; lines 2 and 3 are short.
        // The gutter numbers each *source* line once, on its head row, and
        // leaves wrapped continuation rows unnumbered.
        let raw = "aaaa bbbb cccc dddd eeee ffff\nshort\nlast";
        let rows = render_raw_pane(raw, 10);

        // Head rows carry sequential 1-based line numbers.
        let numbered: Vec<usize> = rows.iter().filter_map(|r| r.line_no).collect();
        assert_eq!(numbered, vec![1, 2, 3]);

        // The first source line wrapped, so at least one continuation row with
        // no number exists before line 2 appears.
        let first_two = rows.first().and_then(|r| r.line_no);
        assert_eq!(first_two, Some(1));
        assert!(
            rows.iter().any(|r| r.line_no.is_none()),
            "expected an unnumbered wrapped continuation row"
        );
    }

    #[test]
    fn heading_link_is_absolute_path_plus_anchor() {
        let dir = fresh_temp("heading-link");
        let note = dir.join("note.md");
        std::fs::write(&note, "# Top\n\ntext\n\n## The Sub Section!\n\nmore\n").unwrap();

        let mut app = App::new(Source::File(note.clone()), opts()).unwrap();
        app.viewport = Rect::new(0, 0, 80, 20);
        app.ensure_rendered(80);

        let canon = std::fs::canonicalize(&note).unwrap();
        assert_eq!(
            app.heading_link("the-sub-section"),
            format!("{}#the-sub-section", canon.display())
        );

        // The link a click produces is the one the reader can follow back.
        let View::Reader(r) = &app.view else {
            panic!("expected Reader view");
        };
        let rd = r.rendered.as_ref().unwrap();
        let hi = rd
            .headings
            .iter()
            .position(|h| h.text == "The Sub Section!");
        let hi = hi.expect("sub heading missing from outline");
        assert_eq!(rd.headings[hi].anchor, "the-sub-section");
        // Clicking anywhere on the heading's glyphs hits it; clicking past
        // the end of the text does not.
        let line = rd.headings[hi].line;
        assert_eq!(rd.heading_at(line, 0), Some(hi));
        assert_eq!(rd.heading_at(line, 79), None);
    }

    #[test]
    fn cross_file_anchor_link_lands_on_the_heading() {
        let dir = fresh_temp("cross-file-anchor");
        let target = dir.join("target.md");
        // Enough filler that the anchor is well below the first screen.
        let mut body = String::from("# Target Top\n\n");
        for i in 0..40 {
            body.push_str(&format!("filler line {i}\n\n"));
        }
        body.push_str("## Deep Section\n\npayload\n\n");
        // Tail filler so the anchor isn't inside the last screenful, where the
        // scroll clamp would legitimately stop short of putting it on row 0.
        for i in 0..40 {
            body.push_str(&format!("tail line {i}\n\n"));
        }
        std::fs::write(&target, &body).unwrap();

        let from = dir.join("from.md");
        std::fs::write(&from, "# From\n").unwrap();

        let mut app = App::new(Source::File(from), opts()).unwrap();
        app.viewport = Rect::new(0, 0, 80, 20);
        app.ensure_rendered(80);

        // The freshly-loaded target has no render yet, so the jump has to
        // survive until after layout.
        app.follow(LinkTarget::FileAnchor(target, "deep-section".into()))
            .unwrap();
        app.ensure_rendered(80);

        let View::Reader(r) = &app.view else {
            panic!("expected Reader view");
        };
        let rd = r.rendered.as_ref().unwrap();
        let want = *rd.link_map.anchors.get("deep-section").unwrap();
        assert!(want > 0, "anchor should not be the first line");
        assert_eq!(r.scroll as usize, want);
        assert!(app.pending_anchor.is_none());
    }

    #[test]
    fn non_markdown_files_load_with_syntax_highlight_wrapper() {
        let dir = fresh_temp("non-md-wrap");
        let json = dir.join("data.jsonl");
        let body = "{\"a\":1}\n{\"a\":2}\n";
        std::fs::write(&json, body).unwrap();

        let mut app = App::new(Source::File(json.clone()), opts()).unwrap();
        // Raw stays exactly what's on disk — saving must not write the
        // synthetic code fence back to the file.
        let View::Reader(r) = &app.view else {
            panic!("expected Reader view");
        };
        assert_eq!(r.raw, body);
        assert_eq!(r.wrap_lang.as_deref(), Some("json"));
        let rs = r.render_source();
        assert!(rs.starts_with("```json\n"), "render source: {:?}", rs);
        assert!(rs.ends_with("```\n"), "render source: {:?}", rs);

        // Render actually produces lines (i.e. pulldown-cmark accepted the
        // wrapped buffer and syntect highlighted the body).
        app.ensure_rendered(80);
        let View::Reader(r) = &app.view else {
            panic!("reader view");
        };
        let rendered = r.rendered.as_ref().expect("rendered");
        assert!(
            !rendered.lines.is_empty(),
            "expected rendered lines for json file"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn jsonl_long_lines_get_expand_button() {
        let dir = fresh_temp("jsonl-button");
        let json = dir.join("data.jsonl");
        // Two lines: one short (fits in 80 cols), one long enough to overflow.
        let short = r#"{"a":1}"#;
        let long = format!("{{\"big\":\"{}\"}}", "x".repeat(120));
        std::fs::write(&json, format!("{}\n{}\n", short, long)).unwrap();

        let mut app = App::new(Source::File(json.clone()), opts()).unwrap();
        app.ensure_rendered(80);
        let View::Reader(r) = &app.view else {
            panic!("reader");
        };
        let overlay = r.jsonl_overlay.as_ref().expect("overlay present");
        assert_eq!(overlay.buttons.len(), 1);
        assert_eq!(overlay.buttons[0].source_line, 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn jsonl_toggle_expand_grows_rendered_rows() {
        let dir = fresh_temp("jsonl-toggle");
        let json = dir.join("data.jsonl");
        let long = format!(r#"{{"big":"{}"}}"#, "x".repeat(120));
        std::fs::write(&json, format!("{}\n", long)).unwrap();

        let mut app = App::new(Source::File(json.clone()), opts()).unwrap();
        app.ensure_rendered(80);
        let initial_rows = {
            let View::Reader(r) = &app.view else { panic!() };
            r.rendered.as_ref().unwrap().lines.len()
        };

        // Expand line 0.
        if let View::Reader(r) = &mut app.view {
            r.toggle_jsonl_line(0).expect("valid JSON");
        }
        app.ensure_rendered(80);
        let View::Reader(r) = &app.view else { panic!() };
        let expanded_rows = r.rendered.as_ref().unwrap().lines.len();
        assert!(
            expanded_rows > initial_rows,
            "expanded ({}) should exceed initial ({})",
            expanded_rows,
            initial_rows
        );
        // Every row that backs the source line should have a clickable button
        // (head row + continuation guides).
        let overlay = r.jsonl_overlay.as_ref().expect("overlay");
        assert!(overlay.buttons.len() >= 3);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn jsonl_toggle_rejects_invalid_json() {
        let dir = fresh_temp("jsonl-invalid");
        let json = dir.join("data.jsonl");
        // Long enough to get a button but not valid JSON.
        std::fs::write(&json, format!("{}\n", "x".repeat(120))).unwrap();
        let mut app = App::new(Source::File(json.clone()), opts()).unwrap();
        if let View::Reader(r) = &mut app.view {
            assert!(r.toggle_jsonl_line(0).is_err());
            assert!(r.jsonl_expanded.is_empty());
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn markdown_files_are_not_wrapped() {
        let dir = fresh_temp("md-no-wrap");
        let md = dir.join("doc.md");
        std::fs::write(&md, "# hi\n").unwrap();
        let app = App::new(Source::File(md.clone()), opts()).unwrap();
        let View::Reader(r) = &app.view else {
            panic!("reader");
        };
        assert!(r.wrap_lang.is_none());
        assert_eq!(r.render_source().as_ref(), "# hi\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn browser_lists_only_text_md_txt_and_dirs_holding_them() {
        let dir = fresh_temp("browser-filter");
        // A subdir that holds a markdown file (should show) and one that holds
        // nothing openable (should be hidden).
        std::fs::create_dir_all(dir.join("withmd")).unwrap();
        std::fs::write(dir.join("withmd").join("inner.md"), "# inner").unwrap();
        std::fs::create_dir_all(dir.join("empty")).unwrap();
        std::fs::create_dir_all(dir.join("binonly")).unwrap();
        std::fs::write(dir.join("binonly").join("blob.bin"), &[0u8, 1, 2][..]).unwrap();

        std::fs::write(dir.join("a.md"), "# a").unwrap();
        std::fs::write(dir.join("b.markdown"), "# b").unwrap();
        std::fs::write(dir.join("note.txt"), "plain text").unwrap();
        std::fs::write(dir.join("NOTES"), "extension-less but text").unwrap();
        // Not txt/md/no-ext → filtered out even though it's text.
        std::fs::write(dir.join("data.json"), "{}").unwrap();
        std::fs::write(dir.join("Cargo.toml"), "ignored").unwrap();
        // No extension but binary → filtered out.
        std::fs::write(dir.join("blob"), &[0u8, 1, 2][..]).unwrap();
        std::fs::write(dir.join("pic.bin"), &[0u8, 1, 2][..]).unwrap();
        std::fs::write(dir.join(".hidden.md"), "hidden").unwrap();

        let b = Browser::scan(&dir).unwrap();
        let names: Vec<&str> = b.entries.iter().map(|e| e.display.as_str()).collect();

        // An empty directory is listed (it's a place to put things); one that
        // holds only unopenable files is not.
        for want in [
            "withmd/",
            "empty/",
            "a.md",
            "b.markdown",
            "note.txt",
            "NOTES",
        ] {
            assert!(names.contains(&want), "missing {want}, got {names:?}");
        }
        assert!(
            !names.contains(&"binonly/"),
            "binonly/ shown, got {names:?}"
        );
        // Non-(txt|md|no-ext) files, binaries, and hidden files are excluded.
        for unwanted in ["data.json", "Cargo.toml", "blob", "pic.bin"] {
            assert!(
                !names.contains(&unwanted),
                "{unwanted} should be filtered out, got {names:?}"
            );
        }
        assert!(names.iter().all(|n| !n.contains(".hidden")));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A new-file name with `/` in it creates the intermediate directories and
    /// drops the file inside, then opens it in the editor.
    #[test]
    fn new_file_creates_intermediate_dirs() {
        let dir = fresh_temp("new-file-nested");
        let mut app = App::new(Source::Directory(dir.clone()), opts()).unwrap();
        let base = match &app.view {
            View::Browser(b) => b.dir.clone(),
            _ => panic!("expected browser"),
        };
        app.prompt_new_file();
        let mut p = app.prompt.take().unwrap();
        p.input = "sub/deep/note.md".into();
        app.commit_prompt(p);

        let created = base.join("sub").join("deep").join("note.md");
        assert!(created.is_file(), "expected {created:?} to be created");
        assert!(matches!(&app.view, View::Reader(r) if r.edit.is_some()));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A new-file name ending in `/` makes a directory (nesting as needed) and
    /// browses into it instead of opening the editor.
    #[test]
    fn new_file_trailing_slash_creates_dir() {
        let dir = fresh_temp("new-dir-trailing-slash");
        let mut app = App::new(Source::Directory(dir.clone()), opts()).unwrap();
        let base = match &app.view {
            View::Browser(b) => b.dir.clone(),
            _ => panic!("expected browser"),
        };
        app.prompt_new_file();
        let mut p = app.prompt.take().unwrap();
        p.input = "notes/drafts/".into();
        app.commit_prompt(p);

        let created = base.join("notes").join("drafts");
        assert!(created.is_dir(), "expected {created:?} to be a directory");
        // Landed inside the new folder, not in the editor.
        match &app.view {
            View::Browser(b) => assert_eq!(b.dir, created),
            _ => panic!("expected to browse into the new folder"),
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A freshly created (so empty) directory is listed. It holds nothing to
    /// read, but hiding it would make creating one look like a no-op.
    #[test]
    fn browser_lists_empty_directories() {
        let dir = fresh_temp("browser-empty-dir");
        std::fs::create_dir_all(dir.join("empty")).unwrap();
        std::fs::write(dir.join("a.md"), "# a").unwrap();
        let app = App::new(Source::Directory(dir.clone()), opts()).unwrap();
        match &app.view {
            View::Browser(b) => assert!(
                b.entries.iter().any(|e| e.display == "empty/"),
                "empty dir missing from listing"
            ),
            _ => panic!("expected browser"),
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `m` moves the selected entry: into a directory when the destination
    /// names one, and renaming on the way when it names a file.
    #[test]
    fn move_entry_into_directory_and_with_rename() {
        let dir = fresh_temp("move-entry");
        std::fs::create_dir_all(dir.join("archive")).unwrap();
        std::fs::write(dir.join("note.md"), "# note").unwrap();
        std::fs::write(dir.join("other.md"), "# other").unwrap();
        let mut app = App::new(Source::Directory(dir.clone()), opts()).unwrap();
        let base = match &app.view {
            View::Browser(b) => b.dir.clone(),
            _ => panic!("expected browser"),
        };

        let select = |app: &mut App, name: &str| match &mut app.view {
            View::Browser(b) => {
                b.selected = b.entries.iter().position(|e| e.display == name).unwrap();
            }
            _ => panic!("expected browser"),
        };

        // Into an existing directory: the file keeps its name.
        select(&mut app, "note.md");
        app.prompt_move();
        let mut p = app.prompt.take().unwrap();
        p.input = "archive".into();
        app.commit_prompt(p);
        assert!(base.join("archive").join("note.md").is_file());
        assert!(!base.join("note.md").exists());

        // A destination naming a file moves and renames in one go, creating
        // the intermediate directory.
        select(&mut app, "other.md");
        app.prompt_move();
        let mut p = app.prompt.take().unwrap();
        p.input = "sub/renamed.md".into();
        app.commit_prompt(p);
        assert!(base.join("sub").join("renamed.md").is_file());
        assert!(!base.join("other.md").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A move onto an existing path is refused rather than clobbering it, and
    /// a directory can't be moved inside itself.
    #[test]
    fn move_entry_refuses_clobber_and_self_nesting() {
        let dir = fresh_temp("move-entry-refuse");
        std::fs::create_dir_all(dir.join("box")).unwrap();
        std::fs::write(dir.join("box").join("taken.md"), "# taken").unwrap();
        std::fs::write(dir.join("taken.md"), "# mine").unwrap();
        let mut app = App::new(Source::Directory(dir.clone()), opts()).unwrap();
        let base = match &app.view {
            View::Browser(b) => b.dir.clone(),
            _ => panic!("expected browser"),
        };

        let select = |app: &mut App, name: &str| match &mut app.view {
            View::Browser(b) => {
                b.selected = b.entries.iter().position(|e| e.display == name).unwrap();
            }
            _ => panic!("expected browser"),
        };

        select(&mut app, "taken.md");
        app.prompt_move();
        let mut p = app.prompt.take().unwrap();
        p.input = "box/".into();
        app.commit_prompt(p);
        assert!(base.join("taken.md").is_file(), "source must survive");
        assert_eq!(
            std::fs::read_to_string(base.join("box").join("taken.md")).unwrap(),
            "# taken",
            "destination must not be clobbered"
        );

        select(&mut app, "box/");
        app.prompt_move();
        let mut p = app.prompt.take().unwrap();
        p.input = "box/inner".into();
        app.commit_prompt(p);
        assert!(base.join("box").is_dir());
        assert!(!base.join("box").join("inner").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A rename or move carries the sync base snapshot across, so the next
    /// sync of a HackMD-linked file still has its common ancestor. Without
    /// this the path-keyed cache orphans and a clean pull turns into a
    /// whole-file conflict.
    #[test]
    fn move_carries_the_sync_base_cache() {
        let dir = fresh_temp("move-sync-base");
        std::fs::create_dir_all(dir.join("archive")).unwrap();
        let body =
            "# linked\n\n<!-- hackmd-sync\nid: note123\nurl: https://hackmd.io/note123\n-->\n";
        std::fs::write(dir.join("linked.md"), body).unwrap();
        let mut app = App::new(Source::Directory(dir.clone()), opts()).unwrap();
        let base = match &app.view {
            View::Browser(b) => b.dir.clone(),
            _ => panic!("expected browser"),
        };
        crate::tui::sync::write_base(&app.root, "note123", &base.join("linked.md"), body).unwrap();

        match &mut app.view {
            View::Browser(b) => {
                b.selected = b
                    .entries
                    .iter()
                    .position(|e| e.display == "linked.md")
                    .unwrap();
            }
            _ => panic!("expected browser"),
        }
        app.prompt_move();
        let mut p = app.prompt.take().unwrap();
        p.input = "archive/".into();
        app.commit_prompt(p);

        let moved = base.join("archive").join("linked.md");
        assert!(moved.is_file());
        assert_eq!(
            crate::tui::sync::read_base(&app.root, "note123", &moved).as_deref(),
            Some(body),
            "the base snapshot should follow the file"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `..` in a new-file name is refused so the file can't escape the tree.
    #[test]
    fn new_file_rejects_parent_traversal() {
        let dir = fresh_temp("new-file-escape");
        let mut app = App::new(Source::Directory(dir.clone()), opts()).unwrap();
        app.prompt_new_file();
        let mut p = app.prompt.take().unwrap();
        p.input = "../escape.md".into();
        app.commit_prompt(p);

        assert!(!dir.parent().unwrap().join("escape.md").exists());
        // Creation refused → still in the browser.
        assert!(matches!(app.view, View::Browser(_)));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The new-file prompt ghost-completes the typed segment to an existing
    /// sub-directory, and offers nothing when there's no such directory.
    #[test]
    fn new_file_completion_suggests_existing_dirs() {
        let dir = fresh_temp("new-file-complete");
        std::fs::create_dir_all(dir.join("guides")).unwrap();
        std::fs::create_dir_all(dir.join("assets").join("img")).unwrap();
        let mut app = App::new(Source::Directory(dir.clone()), opts()).unwrap();
        app.prompt_new_file();

        // Prefix of an existing top-level dir.
        app.prompt.as_mut().unwrap().input = "gui".into();
        assert_eq!(app.path_completion(), Some("des/".into()));

        // Complete a segment nested inside an existing dir.
        app.prompt.as_mut().unwrap().input = "assets/i".into();
        assert_eq!(app.path_completion(), Some("mg/".into()));

        // A plain file name with no matching dir gets no ghost.
        app.prompt.as_mut().unwrap().input = "README".into();
        assert_eq!(app.path_completion(), None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn browser_show_all_reveals_everything_including_hidden() {
        let dir = fresh_temp("browser-show-all");
        std::fs::create_dir_all(dir.join("empty")).unwrap();
        std::fs::write(dir.join("a.md"), "# a").unwrap();
        std::fs::write(dir.join("data.json"), "{}").unwrap();
        std::fs::write(dir.join("blob"), &[0u8, 1, 2][..]).unwrap();
        std::fs::write(dir.join(".hidden.md"), "hidden").unwrap();

        let mut b = Browser::scan(&dir).unwrap();
        b.toggle_show_all();
        let names: Vec<&str> = b.entries.iter().map(|e| e.display.as_str()).collect();

        for want in ["empty/", "a.md", "data.json", "blob", ".hidden.md"] {
            assert!(
                names.contains(&want),
                "show-all missing {want}, got {names:?}"
            );
        }
        assert!(b.show_all);

        // Toggling back restores the filtered listing.
        b.toggle_show_all();
        let names: Vec<&str> = b.entries.iter().map(|e| e.display.as_str()).collect();
        assert!(!names.contains(&"blob"), "blob still shown, got {names:?}");
        assert!(!b.show_all);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sort_by_modified_orders_newest_first() {
        use std::time::{Duration, SystemTime};
        let dir = fresh_temp("browser-sort-mtime");
        // Stamp distinct mtimes so recency order is deterministic regardless of
        // the order the filesystem reports them in.
        for (name, age_secs) in [("old.md", 3000u64), ("mid.md", 1200), ("new.md", 60)] {
            let p = dir.join(name);
            std::fs::write(&p, "x").unwrap();
            let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
            f.set_modified(SystemTime::now() - Duration::from_secs(age_secs))
                .unwrap();
        }
        let md_names = |b: &Browser| -> Vec<String> {
            b.entries
                .iter()
                .filter(|e| e.kind == BrowserEntryKind::Markdown)
                .map(|e| e.display.clone())
                .collect()
        };

        let mut b = Browser::scan(&dir).unwrap();
        // Default: case-insensitive name order.
        assert_eq!(md_names(&b), vec!["mid.md", "new.md", "old.md"]);

        // Toggle → newest first.
        b.toggle_sort_by_modified();
        assert!(b.sort_by_modified);
        assert_eq!(md_names(&b), vec!["new.md", "mid.md", "old.md"]);

        // Toggle back → name order again.
        b.toggle_sort_by_modified();
        assert_eq!(md_names(&b), vec!["mid.md", "new.md", "old.md"]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_utf8_files_are_detected() {
        let dir = fresh_temp("utf8-check");
        let bad = dir.join("bad.txt");
        std::fs::write(&bad, &[0xff, 0xfe, 0x00][..]).unwrap();
        let good = dir.join("good.txt");
        std::fs::write(&good, "héllo").unwrap();

        assert!(!file_is_valid_utf8(&bad));
        assert!(file_is_valid_utf8(&good));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn checkbox_toggle_writes_back_to_file() {
        let dir = fresh_temp("checkbox-toggle");
        let path = dir.join("tasks.md");
        std::fs::write(&path, "- [ ] alpha\n- [x] beta\n").unwrap();

        let mut app = App::new(Source::File(path.clone()), opts()).unwrap();
        app.ensure_rendered(80);

        // Flip the first marker (currently unchecked → checked).
        app.toggle_checkbox(0).unwrap();
        let after_first = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after_first, "- [x] alpha\n- [x] beta\n");

        // Render must regenerate; toggle the second marker (checked → unchecked).
        app.ensure_rendered(80);
        app.toggle_checkbox(1).unwrap();
        let after_second = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after_second, "- [x] alpha\n- [ ] beta\n");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn poll_external_change_reloads_when_file_edited() {
        let dir = fresh_temp("watch-reload");
        let path = dir.join("doc.md");
        std::fs::write(&path, "# original\n").unwrap();

        let mut app = App::new(Source::File(path.clone()), opts()).unwrap();
        app.ensure_rendered(80);
        // Steady-state tick should be a no-op even after rendering.
        assert!(!app.poll_external_change());

        // Some filesystems have second-resolution mtime — bump it so the
        // fingerprint definitely shifts. Belt-and-suspenders: the file
        // length also changes.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&path, "# updated content\n").unwrap();

        assert!(app.poll_external_change(), "expected reload after edit");
        match &app.view {
            View::Reader(r) => assert_eq!(r.raw, "# updated content\n"),
            _ => panic!("expected reader view"),
        }
        // A second poll with no further edits should not reload again.
        assert!(!app.poll_external_change());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn poll_external_change_ignores_byte_identical_touches() {
        let dir = fresh_temp("watch-touch");
        let path = dir.join("doc.md");
        std::fs::write(&path, "# same\n").unwrap();

        let mut app = App::new(Source::File(path.clone()), opts()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        // Rewrite identical content — mtime moves but content doesn't.
        std::fs::write(&path, "# same\n").unwrap();

        assert!(
            !app.poll_external_change(),
            "no-op rewrite must not signal a reload"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn poll_browser_change_rebuilds_when_dir_contents_change() {
        let dir = fresh_temp("browser-watch");
        std::fs::write(dir.join("a.md"), "# a").unwrap();

        let mut app = App::new(Source::Directory(dir.clone()), opts()).unwrap();
        // Steady-state tick is a no-op.
        assert!(!app.poll_browser_change());
        let count_before = match &app.view {
            View::Browser(b) => b.entries.len(),
            _ => panic!("expected browser view"),
        };

        // Drop a new file into the listed directory — the dir's own stat moves.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(dir.join("b.md"), "# b").unwrap();

        assert!(
            app.poll_browser_change(),
            "expected a rebuild after a file was added"
        );
        match &app.view {
            View::Browser(b) => {
                assert_eq!(b.entries.len(), count_before + 1);
                assert!(b.entries.iter().any(|e| e.display == "b.md"));
            }
            _ => panic!("expected browser view"),
        }
        // No further changes → no further rebuild.
        assert!(!app.poll_browser_change());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A change one level down is picked up too. Dropping the first markdown
    /// file into a folder that held nothing openable makes that folder listable
    /// without touching the browsed directory's own mtime, so the listing would
    /// otherwise stay stale until something else changed.
    #[test]
    fn poll_browser_change_notices_a_subdir_becoming_listable() {
        let dir = fresh_temp("browser-watch-nested");
        std::fs::write(dir.join("a.md"), "# a").unwrap();
        std::fs::create_dir_all(dir.join("binonly")).unwrap();
        std::fs::write(dir.join("binonly").join("blob.bin"), &[0u8, 1, 2][..]).unwrap();

        let mut app = App::new(Source::Directory(dir.clone()), opts()).unwrap();
        assert!(!app.poll_browser_change());
        match &app.view {
            View::Browser(b) => assert!(
                !b.entries.iter().any(|e| e.display == "binonly/"),
                "a dir holding only binaries starts hidden"
            ),
            _ => panic!("expected browser view"),
        }

        // Sleep past the coarsest mtime granularity we might be sitting on.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(dir.join("binonly").join("now.md"), "# now").unwrap();

        assert!(
            app.poll_browser_change(),
            "expected a rebuild after a subdir gained a markdown file"
        );
        match &app.view {
            View::Browser(b) => assert!(
                b.entries.iter().any(|e| e.display == "binonly/"),
                "the subdir should now be listed"
            ),
            _ => panic!("expected browser view"),
        }
        assert!(!app.poll_browser_change());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn checkbox_toggle_does_not_self_trigger_reload() {
        let dir = fresh_temp("watch-self-write");
        let path = dir.join("t.md");
        std::fs::write(&path, "- [ ] task\n").unwrap();

        let mut app = App::new(Source::File(path.clone()), opts()).unwrap();
        app.ensure_rendered(80);
        app.toggle_checkbox(0).unwrap();
        // Our own write must refresh the fingerprint so the watcher tick
        // immediately afterwards does not see a phantom external change.
        assert!(!app.poll_external_change());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn checkbox_toggle_is_idempotent_round_trip() {
        let dir = fresh_temp("checkbox-roundtrip");
        let path = dir.join("t.md");
        std::fs::write(&path, "- [ ] task\n").unwrap();

        let mut app = App::new(Source::File(path.clone()), opts()).unwrap();
        app.ensure_rendered(80);
        app.toggle_checkbox(0).unwrap();
        app.ensure_rendered(80);
        app.toggle_checkbox(0).unwrap();

        let final_content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(final_content, "- [ ] task\n");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn go_back_restores_browser_selected_index() {
        let dir = fresh_temp("history-selected");
        std::fs::write(dir.join("a.md"), "# A").unwrap();
        std::fs::write(dir.join("b.md"), "# B").unwrap();
        std::fs::write(dir.join("c.md"), "# C").unwrap();

        let mut app = App::new(Source::Directory(dir.clone()), opts()).unwrap();

        // Pick a non-default entry.
        let target_idx = match &mut app.view {
            View::Browser(b) => {
                let idx = b
                    .entries
                    .iter()
                    .position(|e| e.display == "b.md")
                    .expect("b.md must be listed");
                b.selected = idx;
                idx
            }
            _ => panic!("expected browser at startup"),
        };

        // Open the file, then come back.
        let target_path = match &app.view {
            View::Browser(b) => b.entries[b.selected].path.clone(),
            _ => unreachable!(),
        };
        app.navigate_to(EntryKind::File(target_path), 0).unwrap();
        app.go_back().unwrap();

        match &app.view {
            View::Browser(b) => assert_eq!(b.selected, target_idx),
            _ => panic!("expected to land back in browser"),
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reader_scroll_preserved_across_back_forward() {
        let dir = fresh_temp("history-scroll");
        let path = dir.join("a.md");
        std::fs::write(&path, "# A\n\nbody\n").unwrap();
        let other = dir.join("b.md");
        std::fs::write(&other, "# B").unwrap();

        let mut app = App::new(Source::File(path.clone()), opts()).unwrap();
        if let View::Reader(r) = &mut app.view {
            r.scroll = 1;
        }
        app.navigate_to(EntryKind::File(other), 0).unwrap();
        app.go_back().unwrap();
        match &app.view {
            View::Reader(r) => assert_eq!(r.scroll, 1, "scroll should be restored"),
            _ => panic!(),
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn browser_navigate_into_subdir_and_back() {
        let dir = fresh_temp("flat-nav-back");
        std::fs::create_dir_all(dir.join("inner")).unwrap();
        std::fs::write(dir.join("inner/note.md"), "# note").unwrap();

        let mut app = App::new(Source::Directory(dir.clone()), opts()).unwrap();
        app.navigate_to(EntryKind::Directory(dir.join("inner")), 0)
            .unwrap();
        match &app.view {
            View::Browser(b) => assert_eq!(b.dir, dir.join("inner")),
            _ => panic!("expected browser at inner/"),
        }
        app.go_back().unwrap();
        match &app.view {
            View::Browser(b) => assert_eq!(b.dir, dir),
            _ => panic!("expected browser at root after back"),
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn browser_lists_one_level_only() {
        let dir = fresh_temp("flat-listing");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("top.md"), "# top").unwrap();
        std::fs::write(dir.join("sub/inner.md"), "# inner").unwrap();

        // Top-level browser shows `sub/` and `top.md` but never recurses into
        // `sub/`, so `inner.md` must not appear.
        let b = Browser::scan(&dir).unwrap();
        assert!(b.entries.iter().any(|e| e.display == "sub/"));
        assert!(b.entries.iter().any(|e| e.display == "top.md"));
        assert!(!b.entries.iter().any(|e| e.display == "inner.md"));

        // Scanning the sub-directory directly is the only way to see its
        // contents — that's what activating `sub/` does in the event handler.
        let sub = Browser::scan(&dir.join("sub")).unwrap();
        assert!(sub.entries.iter().any(|e| e.display == "inner.md"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn go_back_walks_up_nested_browser_history() {
        let dir = fresh_temp("nested-browser-back");
        std::fs::create_dir_all(dir.join("a/b/c")).unwrap();
        let a = dir.join("a");
        let b = a.join("b");
        let c = b.join("c");

        let mut app = App::new(Source::Directory(dir.clone()), opts()).unwrap();
        app.navigate_to(EntryKind::Directory(a.clone()), 0).unwrap();
        app.navigate_to(EntryKind::Directory(b.clone()), 0).unwrap();
        app.navigate_to(EntryKind::Directory(c.clone()), 0).unwrap();

        // Pop once: should land in `b`, not quit / collapse history.
        app.go_back().unwrap();
        match &app.view {
            View::Browser(br) => assert_eq!(br.dir, b),
            _ => panic!("expected browser at b"),
        }

        // Pop again: `a`.
        app.go_back().unwrap();
        match &app.view {
            View::Browser(br) => assert_eq!(br.dir, a),
            _ => panic!("expected browser at a"),
        }

        // History still has the original root, so we're not "stuck".
        assert!(!app.history.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn edit_mode_insert_marks_dirty_and_advances_cursor() {
        let dir = fresh_temp("edit-insert");
        let path = dir.join("doc.md");
        std::fs::write(&path, "hello\n").unwrap();

        let mut app = App::new(Source::File(path.clone()), opts()).unwrap();
        app.ensure_rendered(80);
        app.enter_edit();
        app.edit_insert("X");

        let View::Reader(r) = &app.view else {
            panic!("expected reader")
        };
        assert_eq!(r.raw, "Xhello\n");
        let e = r.edit.as_ref().unwrap();
        assert!(e.dirty);
        assert_eq!(e.cursor, 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn edit_newline_in_a_numbered_list_renumbers_the_items_below() {
        let dir = fresh_temp("edit-newline-renumber");
        let path = dir.join("doc.md");
        std::fs::write(&path, "1. one\n2. two\n3. three\n").unwrap();

        let mut app = App::new(Source::File(path.clone()), opts()).unwrap();
        app.ensure_rendered(80);
        app.enter_edit();
        // Cursor at the end of "1. one".
        match &mut app.view {
            View::Reader(r) => r.edit.as_mut().unwrap().cursor = "1. one".len(),
            _ => panic!("expected reader"),
        }
        app.edit_newline();

        let View::Reader(r) = &app.view else {
            panic!("expected reader")
        };
        assert_eq!(r.raw, "1. one\n2. \n3. two\n4. three\n");
        // Cursor sits just after the inserted marker, untouched by renumbering.
        assert_eq!(r.edit.as_ref().unwrap().cursor, "1. one\n2. ".len());

        // One Enter undoes as a single step, renumbering included.
        app.edit_undo();
        let View::Reader(r) = &app.view else { panic!() };
        assert_eq!(r.raw, "1. one\n2. two\n3. three\n");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn edit_save_persists_to_disk_and_clears_dirty() {
        let dir = fresh_temp("edit-save");
        let path = dir.join("doc.md");
        std::fs::write(&path, "first\n").unwrap();

        let mut app = App::new(Source::File(path.clone()), opts()).unwrap();
        app.ensure_rendered(80);
        app.enter_edit();
        app.edit_insert("X");
        app.save_edit().unwrap();

        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert_eq!(on_disk, "Xfirst\n");
        let View::Reader(r) = &app.view else { panic!() };
        assert!(!r.edit.as_ref().unwrap().dirty);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn edit_discard_reloads_from_disk_and_exits_edit_mode() {
        let dir = fresh_temp("edit-discard");
        let path = dir.join("doc.md");
        std::fs::write(&path, "original\n").unwrap();

        let mut app = App::new(Source::File(path.clone()), opts()).unwrap();
        app.ensure_rendered(80);
        app.enter_edit();
        app.edit_insert("XYZ");
        // Sanity: buffer was mutated.
        match &app.view {
            View::Reader(r) => assert_eq!(r.raw, "XYZoriginal\n"),
            _ => panic!(),
        }
        app.exit_edit_discard();

        match &app.view {
            View::Reader(r) => {
                assert!(r.edit.is_none(), "should be back in view mode");
                assert_eq!(r.raw, "original\n", "buffer should match disk");
            }
            _ => panic!(),
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn autoclose_only_before_space_or_eol() {
        let dir = fresh_temp("edit-autoclose");
        let path = dir.join("a.md");
        std::fs::write(&path, "foo\n").unwrap();
        let mut app = App::new(Source::File(path.clone()), opts()).unwrap();
        app.ensure_rendered(80);
        app.enter_edit();

        // Cursor at start (before `f`): next char is text → no auto-close.
        assert!(!app.edit_autoclose_ok());

        // Cursor at end of buffer → auto-close ok.
        if let View::Reader(r) = &mut app.view {
            r.edit.as_mut().unwrap().cursor = r.raw.len();
        }
        assert!(app.edit_autoclose_ok());

        // Cursor right before the newline → next char is whitespace → ok.
        if let View::Reader(r) = &mut app.view {
            r.edit.as_mut().unwrap().cursor = 3; // "foo|\n"
        }
        assert!(app.edit_autoclose_ok());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cut_current_line_removes_line_and_returns_it() {
        let dir = fresh_temp("edit-cutline");
        let path = dir.join("c.md");
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        let mut app = App::new(Source::File(path.clone()), opts()).unwrap();
        app.ensure_rendered(80);
        app.enter_edit();
        // Put the cursor on the "two" line.
        if let View::Reader(r) = &mut app.view {
            r.edit.as_mut().unwrap().cursor = 5; // inside "two"
        }
        let cut = app.edit_cut_current_line();
        assert_eq!(cut.as_deref(), Some("two\n"));
        let View::Reader(r) = &app.view else { panic!() };
        assert_eq!(r.raw, "one\nthree\n");
        assert!(r.edit.as_ref().unwrap().dirty);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn recovery_round_trips_and_prompts_on_reopen() {
        use crate::tui::recovery;
        let dir = fresh_temp("edit-recovery");
        let path = dir.join("r.md");
        std::fs::write(&path, "saved\n").unwrap();

        // Edit without saving, then autosave to the recovery mirror.
        {
            let mut app = App::new(Source::File(path.clone()), opts()).unwrap();
            app.ensure_rendered(80);
            app.enter_edit();
            app.edit_insert("UNSAVED ");
            app.autosave_recovery(true);
        }
        // The mirror differs from disk → a fresh open offers recovery.
        assert!(recovery::pending(&path, "saved\n").is_some());
        let app = App::new(Source::File(path.clone()), opts()).unwrap();
        assert!(
            matches!(
                app.prompt.as_ref().map(|p| &p.kind),
                Some(PromptKind::RecoverEdit { .. })
            ),
            "reopen should raise the recover prompt"
        );

        // Saving clears the mirror.
        {
            let mut app = App::new(Source::File(path.clone()), opts()).unwrap();
            app.prompt = None; // dismiss recover prompt
            app.enter_edit();
            app.edit_insert("X");
            app.autosave_recovery(true);
            app.save_edit().unwrap();
        }
        assert!(recovery::load(&path).is_none(), "save clears recovery");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn undo_restores_prior_buffer_and_cursor() {
        let dir = fresh_temp("edit-undo");
        let path = dir.join("u.md");
        std::fs::write(&path, "abc\n").unwrap();
        let mut app = App::new(Source::File(path.clone()), opts()).unwrap();
        app.ensure_rendered(80);
        app.enter_edit();
        // Insert two distinct edits; undo once → first edit remains; undo
        // twice → buffer back to original.
        app.edit_insert("X");
        app.edit_insert("Y");
        match &app.view {
            View::Reader(r) => assert_eq!(r.raw, "XYabc\n"),
            _ => panic!(),
        };
        app.edit_undo();
        match &app.view {
            View::Reader(r) => assert_eq!(r.raw, "Xabc\n"),
            _ => panic!(),
        };
        app.edit_undo();
        match &app.view {
            View::Reader(r) => assert_eq!(r.raw, "abc\n"),
            _ => panic!(),
        };
        // Redo once → first edit reapplied.
        app.edit_redo();
        match &app.view {
            View::Reader(r) => assert_eq!(r.raw, "Xabc\n"),
            _ => panic!(),
        };
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn move_line_swaps_with_neighbour_and_keeps_cursor() {
        let dir = fresh_temp("edit-move-line");
        let path = dir.join("m.md");
        std::fs::write(&path, "alpha\nbravo\ncharlie\n").unwrap();
        let mut app = App::new(Source::File(path.clone()), opts()).unwrap();
        app.ensure_rendered(80);
        app.enter_edit();

        // Put the cursor inside "bravo" (col 2 → the 'a'), then move it down.
        let set_cursor = |app: &mut App, pos: usize| match &mut app.view {
            View::Reader(r) => r.edit.as_mut().unwrap().cursor = pos,
            _ => panic!(),
        };
        set_cursor(&mut app, 8); // "alpha\n" = 6, "br" = 2 → byte 8
        app.edit_move_line(1);
        match &app.view {
            View::Reader(r) => {
                assert_eq!(r.raw, "alpha\ncharlie\nbravo\n");
                // Cursor rode along: still on the 'a' of "bravo".
                assert_eq!(&r.raw[r.edit.as_ref().unwrap().cursor..][..1], "a");
            }
            _ => panic!(),
        }

        // Move it back up to the original order.
        app.edit_move_line(-1);
        match &app.view {
            View::Reader(r) => assert_eq!(r.raw, "alpha\nbravo\ncharlie\n"),
            _ => panic!(),
        }

        // Edge: first line can't go up, last line can't go down (no-op).
        set_cursor(&mut app, 0);
        app.edit_move_line(-1);
        match &app.view {
            View::Reader(r) => assert_eq!(r.raw, "alpha\nbravo\ncharlie\n"),
            _ => panic!(),
        }
        // Last logical line is "charlie" (the trailing "" after the final \n
        // is the real last line, so charlie moving down swaps with it).
        set_cursor(&mut app, 14); // inside "charlie"
        app.edit_move_line(1);
        match &app.view {
            View::Reader(r) => assert_eq!(r.raw, "alpha\nbravo\n\ncharlie"),
            _ => panic!(),
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn redo_stack_clears_on_new_mutation() {
        let dir = fresh_temp("edit-redo-clear");
        let path = dir.join("r.md");
        std::fs::write(&path, "a\n").unwrap();
        let mut app = App::new(Source::File(path.clone()), opts()).unwrap();
        app.ensure_rendered(80);
        app.enter_edit();
        app.edit_insert("X");
        app.edit_undo(); // redo stack now has the X-insert
        app.edit_insert("Y"); // diverging mutation — should drop the redo
        match &app.view {
            View::Reader(r) => {
                assert_eq!(r.raw, "Ya\n");
                let e = r.edit.as_ref().unwrap();
                assert!(
                    e.redo.is_empty(),
                    "redo should be cleared after diverging edit"
                );
            }
            _ => panic!(),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_unified_diff_classifies_row_kinds() {
        let diff = "\
diff --git a/x.md b/x.md\n\
index abc..def 100644\n\
--- a/x.md\n\
+++ b/x.md\n\
@@ -1,3 +1,4 @@\n\
 context line\n\
-removed\n\
+added\n\
+another added\n\
 trailer\n";
        let rows = parse_unified_diff(diff);
        let kinds: Vec<DiffRowKind> = rows.iter().map(|r| r.kind).collect();
        assert_eq!(
            kinds,
            vec![
                DiffRowKind::Header,
                DiffRowKind::Header,
                DiffRowKind::Header,
                DiffRowKind::Header,
                DiffRowKind::Hunk,
                DiffRowKind::Context,
                DiffRowKind::Removed,
                DiffRowKind::Added,
                DiffRowKind::Added,
                DiffRowKind::Context,
            ],
        );
    }

    #[test]
    fn long_paragraph_wraps_in_edit_mode_with_cursor_on_correct_row() {
        let dir = fresh_temp("edit-wrap");
        let path = dir.join("long.md");
        // Single source line, much wider than the render width (40), with
        // the cursor near the end. Without wrap, the cursor would be off
        // screen.
        let body = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon";
        std::fs::write(&path, body).unwrap();

        let mut o = opts();
        o.width = 40;
        let mut app = App::new(Source::File(path.clone()), o).unwrap();
        app.viewport = Rect::new(0, 0, 40, 24);
        app.ensure_rendered(40);
        app.enter_edit();
        // This test exercises the legacy in-place toggle (cursor_xy is only
        // populated in that mode); split mode renders preview without a
        // cursor in the markdown grid.
        if let View::Reader(r) = &mut app.view {
            r.edit.as_mut().unwrap().mode = EditMode::InPlace;
            r.edit.as_mut().unwrap().cursor = body.len() - 5;
        }
        app.ensure_rendered(40);

        let View::Reader(r) = &app.view else { panic!() };
        let rd = r.rendered.as_ref().unwrap();
        let xy = rd.cursor_xy.expect("cursor should have a display position");
        // Cursor must land on a row > 0 (the line wrapped) AND its column
        // must be inside the body width.
        assert!(xy.1 > 0, "expected cursor on wrapped row, got {:?}", xy);
        assert!(
            xy.0 < 40,
            "cursor col should fit within render width, got {:?}",
            xy
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn block_level_raw_toggle_substitutes_only_cursor_block() {
        let dir = fresh_temp("edit-toggle");
        let path = dir.join("doc.md");
        std::fs::write(&path, "# Heading\n\nSecond paragraph.\n").unwrap();

        let mut app = App::new(Source::File(path.clone()), opts()).unwrap();
        app.ensure_rendered(80);
        app.enter_edit();
        // Block-level toggle is the legacy in-place mode.
        if let View::Reader(r) = &mut app.view {
            r.edit.as_mut().unwrap().mode = EditMode::InPlace;
        }
        // Cursor at byte 0 → in the heading block.
        app.ensure_rendered(80);

        let View::Reader(r) = &app.view else { panic!() };
        let rd = r.rendered.as_ref().unwrap();
        // The heading line should be visible with its raw `#` marker since
        // the cursor is in that block.
        let any_raw_heading = rd.lines.iter().any(|l| {
            let s: String = l.spans.iter().map(|sp| sp.content.as_ref()).collect();
            s.contains("# Heading")
        });
        assert!(
            any_raw_heading,
            "expected raw heading line in rendered output"
        );

        // The other paragraph should remain formatted (no `#` markers).
        let any_raw_para_marker = rd.lines.iter().any(|l| {
            let s: String = l.spans.iter().map(|sp| sp.content.as_ref()).collect();
            s.contains("# Second")
        });
        assert!(
            !any_raw_para_marker,
            "second paragraph should stay formatted"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fresh_browser_skips_parent_dir_default_selection() {
        let dir = fresh_temp("browser-default-skip");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.md"), "# a").unwrap();
        std::fs::write(dir.join("b.md"), "# b").unwrap();

        let b = Browser::scan(&dir).unwrap();
        // First entry is `../`; cursor should not start on it.
        assert!(matches!(b.entries[0].kind, BrowserEntryKind::ParentDir));
        assert!(
            b.selected > 0,
            "expected to skip ../, got selected={}",
            b.selected
        );
        assert!(!matches!(
            b.entries[b.selected].kind,
            BrowserEntryKind::ParentDir
        ));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn root_directory_with_no_parent_starts_at_zero() {
        // Filesystem root has no parent → `../` not added → first entry
        // is a real one and the cursor lands on it (index 0).
        let dir = fresh_temp("browser-no-parent");
        std::fs::write(dir.join("a.md"), "# a").unwrap();

        // Force the no-parent case by stripping the parent reference. Easier:
        // just verify rebuild's fallback in the absence of `..`.
        let mut b = Browser {
            dir: dir.clone(),
            entries: vec![BrowserEntry {
                path: dir.join("a.md"),
                display: "a.md".to_string(),
                kind: BrowserEntryKind::Markdown,
                modified: None,
            }],
            selected: 0,
            scroll: 0,
            last_meta: None,
            find: None,
            show_all: false,
            sort_by_modified: false,
            child_dirs: Vec::new(),
            jump_labels: None,
        };
        // Re-scan (rebuild discovers `..` if the dir has a parent — fine, just
        // assert the fallback never out-of-bounds).
        b.rebuild().unwrap();
        assert!(b.selected < b.entries.len());

        std::fs::remove_dir_all(&dir).ok();
    }

    fn browser_with(names: &[&str]) -> Browser {
        Browser {
            dir: PathBuf::from("/tmp"),
            entries: names
                .iter()
                .map(|n| BrowserEntry {
                    path: PathBuf::from("/tmp").join(n),
                    display: n.to_string(),
                    kind: BrowserEntryKind::Markdown,
                    modified: None,
                })
                .collect(),
            selected: 0,
            scroll: 0,
            last_meta: None,
            find: None,
            show_all: false,
            sort_by_modified: false,
            child_dirs: Vec::new(),
            jump_labels: None,
        }
    }

    #[test]
    fn find_is_anchored_prefix_and_smartcase() {
        let b = browser_with(&["alpha.md", "beta.md", "Beacon.md", "bonus.md"]);
        // Anchored: "be" matches "beta.md" (prefix), not "bonus.md".
        assert_eq!(b.find_from("be", 0), Some(1));
        // Lowercase query is case-insensitive (smartcase off): "bea" finds
        // "Beacon.md".
        assert_eq!(b.find_from("bea", 0), Some(2));
        // Uppercase in query turns on case sensitivity: "Be" matches only
        // "Beacon.md", skipping lowercase "beta.md".
        assert_eq!(b.find_from("Be", 0), Some(2));
        // A single char lands on the first item starting with it.
        assert_eq!(b.find_from("b", 0), Some(1));
        // No match → None.
        assert_eq!(b.find_from("z", 0), None);
    }

    #[test]
    fn find_next_and_prev_cycle_with_wraparound() {
        let b = browser_with(&["bat.md", "bee.md", "bun.md", "cat.md"]);
        // Forward from just past index 0 finds the next "b" (index 1), then 2,
        // then wraps back to 0.
        assert_eq!(b.find_from("b", 1), Some(1));
        assert_eq!(b.find_from("b", 3 % 4), Some(0)); // wrap past "cat.md"
        // Backward from index 0 wraps to the last "b" match (index 2).
        assert_eq!(b.find_back_from("b", 0), Some(2));
        assert_eq!(b.find_back_from("b", 2), Some(1));
    }

    #[test]
    fn focus_targets_interleaves_links_and_checkboxes_in_document_order() {
        let dir = fresh_temp("focus-targets");
        let path = dir.join("doc.md");
        // Two links, two checkboxes, intentionally interleaved so plain
        // sequential indexing wouldn't yield document order.
        let src = "\
- [ ] task one with [link a](https://a)\n\
\n\
[link b](https://b)\n\
\n\
- [x] task two\n";
        std::fs::write(&path, src).unwrap();

        let mut app = App::new(Source::File(path.clone()), opts()).unwrap();
        app.ensure_rendered(80);
        let View::Reader(r) = &app.view else {
            panic!("expected reader")
        };

        let targets = r.focus_targets();
        // Expect 4 items: cb0, link0 (same line as cb0), link1, cb1.
        assert_eq!(targets.len(), 4, "got {:?}", targets);
        let kinds: Vec<&'static str> = targets
            .iter()
            .map(|(f, _, _)| match f {
                Focus::Link(_) => "link",
                Focus::Checkbox(_) => "cb",
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["cb", "link", "link", "cb"],
            "ordering: {:?}",
            targets
        );

        // The lines must be monotonically non-decreasing.
        for w in targets.windows(2) {
            assert!(w[0].1 <= w[1].1, "lines not in order: {:?}", targets);
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn is_markdown_file_recognises_extensions() {
        assert!(is_markdown_file(Path::new("foo.md")));
        assert!(is_markdown_file(Path::new("foo.MD")));
        assert!(is_markdown_file(Path::new("foo.markdown")));
        assert!(is_markdown_file(Path::new("foo.mdown")));
        assert!(!is_markdown_file(Path::new("foo.txt")));
        assert!(!is_markdown_file(Path::new("foo")));
    }
}

#[cfg(test)]
mod cloud_msg_tests {
    use super::*;
    use crate::tui::cloud::{CloudLists, CloudMsg, FetchIntent, FetchedNote};
    use crate::types::{NotePermissionRole, NotePublishType, SingleNote};

    fn opts() -> Options {
        Options {
            width: 80,
            line_numbers: false,
            theme: crate::tui::theme::Theme::dark(),
        }
    }

    /// App over a stdin reader with a disconnected cloud — no disk, no
    /// runtime. `apply_cloud_msg` is a pure state transition on top.
    fn test_app() -> App {
        App::new(Source::Stdin("local text".into()), opts()).expect("app")
    }

    /// A *connected* app: a live tokio runtime plus a client pointed at an
    /// unroutable endpoint (no request is actually made in these tests). The
    /// returned `Runtime` must outlive the app — dropping it staleness the
    /// handle the cloud holds.
    fn connected_app() -> (tokio::runtime::Runtime, App) {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let client = crate::Client::with_endpoint("tok", "http://127.0.0.1:1/").expect("client");
        let cloud = CloudContext::new(rt.handle().clone(), Some(client));
        let app = App::with_cloud(Source::Stdin("x".into()), opts(), cloud).expect("app");
        (rt, app)
    }

    fn open_file_reader(app: &mut App, path: &std::path::Path) {
        app.view = View::Reader(Reader::from_file(path).expect("open reader"));
        // Fresh file → not yet handled by the open-file sync check.
        app.last_sync = None;
    }

    /// A fresh git repo (canonical root) with one committed `note.md`. The
    /// returned `TempDir` keeps it alive for the test.
    fn commit_repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
        fn git(dir: &std::path::Path, args: &[&str]) {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .expect("spawn git");
            assert!(
                out.status.success(),
                "git {:?}: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        git(&root, &["init", "-q"]);
        git(&root, &["config", "user.email", "t@example.com"]);
        git(&root, &["config", "user.name", "Test"]);
        git(&root, &["config", "commit.gpgsign", "false"]);
        git(&root, &["config", "core.hooksPath", "/dev/null"]);
        let path = root.join("note.md");
        std::fs::write(&path, "one\n").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "baseline"]);
        (dir, root, path)
    }

    /// Leaving the editor with the file now differing from HEAD raises a
    /// one-line commit popup scoped to that file; entering a message commits
    /// just that file and clears its uncommitted state.
    #[test]
    fn exit_edit_offers_commit_for_uncommitted_file() {
        let (_repo, _root, path) = commit_repo();
        let mut app = test_app();
        open_file_reader(&mut app, &path);

        edit_with(&mut app, "one\ntwo\n", 0);
        app.save_edit().unwrap();
        app.exit_edit();

        // Editor closed, commit popup up, scoped to this file.
        assert!(matches!(&app.view, View::Reader(r) if r.edit.is_none()));
        let prompt = app.prompt.as_ref().expect("expected commit prompt");
        assert!(
            matches!(&prompt.kind, PromptKind::CommitFile { file, .. } if file == &path),
            "prompt not scoped to the edited file"
        );

        // Type a message and commit.
        let mut p = app.prompt.take().unwrap();
        p.input = "add two".into();
        app.commit_prompt(p);

        app.refresh_git_status();
        assert!(!app.git_status.is_uncommitted(&path));
    }

    /// An empty message skips the commit and leaves the file uncommitted.
    #[test]
    fn exit_edit_commit_skipped_on_empty_message() {
        let (_repo, _root, path) = commit_repo();
        let mut app = test_app();
        open_file_reader(&mut app, &path);
        edit_with(&mut app, "one\ntwo\n", 0);
        app.save_edit().unwrap();
        app.exit_edit();

        let p = app.prompt.take().expect("expected commit prompt");
        app.commit_prompt(p); // empty input
        app.refresh_git_status();
        assert!(app.git_status.is_uncommitted(&path));
    }

    /// Leaving a clean, already-committed file raises no commit popup.
    #[test]
    fn exit_edit_no_commit_prompt_when_clean() {
        let (_repo, _root, path) = commit_repo();
        let mut app = test_app();
        open_file_reader(&mut app, &path);
        app.enter_edit(); // no changes
        app.exit_edit();
        assert!(app.prompt.is_none());
    }

    const LINKED_DOC: &str =
        "<!-- hackmd-sync\nid: AbCdEf123\nurl: https://hackmd.io/AbCdEf123\n-->\n# hi\n";

    const DECK: &str = "---\nmarp: true\npaginate: true\n---\n<!-- _class: lead -->\n# Title\n\n---\n\n## Second\n\n- a\n- b\n";

    /// Opening a Marp file auto-enters presentation mode and renders only the
    /// current slide; slide navigation moves the deck and re-renders.
    #[test]
    fn marp_file_presents_and_navigates() {
        let mut app = test_app();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deck.md");
        std::fs::write(&path, DECK).unwrap();
        open_file_reader(&mut app, &path);

        // Auto-entered presentation on slide 0.
        let View::Reader(r) = &app.view else {
            panic!("reader")
        };
        assert!(r.marp_present(), "a Marp deck opens in presentation mode");
        assert_eq!(r.marp.as_ref().unwrap().deck.len(), 2);

        // First slide renders the title, not the second slide's content.
        app.ensure_rendered(80);
        let View::Reader(r) = &app.view else {
            panic!("reader")
        };
        let text: String = r
            .rendered
            .as_ref()
            .unwrap()
            .lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Title"), "slide 0 shows the title");
        assert!(!text.contains("Second"), "slide 0 must not show slide 1");

        // Advance one slide and re-render.
        if let View::Reader(r) = &mut app.view {
            r.slide_by(1);
        }
        app.ensure_rendered(80);
        let View::Reader(r) = &app.view else {
            panic!("reader")
        };
        assert_eq!(r.marp.as_ref().unwrap().slide, 1);
        let text: String = r
            .rendered
            .as_ref()
            .unwrap()
            .lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Second"), "slide 1 shows its heading");

        // Can't advance past the last slide (no wrap).
        if let View::Reader(r) = &mut app.view {
            r.slide_by(1);
            assert_eq!(r.marp.as_ref().unwrap().slide, 1);
        }

        // Toggling off presentation renders the whole document again.
        if let View::Reader(r) = &mut app.view {
            r.toggle_present();
            assert!(!r.marp_present());
        }
        app.ensure_rendered(80);
        let View::Reader(r) = &app.view else {
            panic!("reader")
        };
        let text: String = r
            .rendered
            .as_ref()
            .unwrap()
            .lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("Title") && text.contains("Second"),
            "scroll mode shows the full deck"
        );
    }

    /// A plain markdown file (no `marp: true`) is never treated as a deck.
    #[test]
    fn plain_markdown_is_not_a_deck() {
        let mut app = test_app();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Just a note\n\n---\n\nmore text\n").unwrap();
        open_file_reader(&mut app, &path);
        let View::Reader(r) = &app.view else {
            panic!("reader")
        };
        assert!(r.marp.is_none());
        assert!(!r.marp_present());
    }

    /// Opening a HackMD-linked local file offers a confirm prompt and does NOT
    /// spend an API call until the user accepts — this is the fix for the 15s
    /// background poll that drained a Free workspace's 400/month quota.
    #[test]
    fn maybe_sync_prompts_before_fetching_linked_file() {
        let (_rt, mut app) = connected_app();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, LINKED_DOC).unwrap();
        open_file_reader(&mut app, &path);

        app.maybe_sync();

        assert!(
            matches!(
                app.prompt.as_ref().map(|p| &p.kind),
                Some(PromptKind::ConfirmFetchUpdate { .. })
            ),
            "a linked file should offer a fetch confirmation on open"
        );
        assert!(
            !app.pending_sync,
            "no fetch may be in flight before the user confirms"
        );
    }

    /// The prompt fires exactly once per open — the tick loop must not re-offer
    /// it (nor re-poll) every 250ms.
    #[test]
    fn maybe_sync_does_not_re_prompt_same_file() {
        let (_rt, mut app) = connected_app();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, LINKED_DOC).unwrap();
        open_file_reader(&mut app, &path);

        app.maybe_sync();
        // Dismiss (user chose "keep local").
        app.prompt = None;
        app.maybe_sync();

        assert!(
            app.prompt.is_none(),
            "a dismissed fetch prompt must not immediately reappear"
        );
    }

    /// Accepting the prompt spends the call: `sync_local_file` runs and marks a
    /// fetch in flight.
    #[test]
    fn confirming_fetch_prompt_kicks_off_sync() {
        let (_rt, mut app) = connected_app();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, LINKED_DOC).unwrap();
        open_file_reader(&mut app, &path);
        app.maybe_sync();
        let prompt = app.prompt.take().expect("prompt present");

        app.commit_prompt(prompt);

        assert!(
            app.pending_sync,
            "confirming should start the upstream fetch"
        );
    }

    /// A plain local file with no HackMD block is silently skipped: no prompt,
    /// no call, but marked handled so it isn't re-read every tick.
    #[test]
    fn maybe_sync_skips_unlinked_file() {
        let (_rt, mut app) = connected_app();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.md");
        std::fs::write(&path, "# just a local note\n").unwrap();
        open_file_reader(&mut app, &path);

        app.maybe_sync();

        assert!(app.prompt.is_none(), "unlinked file must not prompt");
        assert!(!app.pending_sync, "unlinked file must not fetch");
        assert!(
            matches!(&app.last_sync, Some((p, _)) if *p == path),
            "unlinked file should still be marked handled"
        );
    }

    fn note(id: &str, title: &str, content: &str) -> SingleNote {
        SingleNote {
            id: id.to_string(),
            title: title.to_string(),
            tags: Vec::new(),
            last_changed_at: "2024-01-01T00:00:00.000Z".into(),
            created_at: "2024-01-01T00:00:00.000Z".into(),
            last_change_user: None,
            publish_type: NotePublishType::View,
            published_at: None,
            user_path: None,
            team_path: None,
            permalink: None,
            short_id: "s".into(),
            publish_link: format!("https://hackmd.io/{id}"),
            read_permission: NotePermissionRole::Owner,
            write_permission: NotePermissionRole::Owner,
            folder_paths: None,
            content: content.to_string(),
        }
    }

    fn fresh(id: &str, title: &str, content: &str) -> CloudMsg {
        CloudMsg::Note {
            id: id.to_string(),
            intent: FetchIntent::OpenReader { scroll: 0 },
            result: Ok(FetchedNote::Fresh {
                note: Box::new(note(id, title, content)),
                etag: Some("W/\"v1\"".into()),
            }),
        }
    }

    fn edit_state(dirty: bool) -> EditState {
        EditState {
            cursor: 0,
            dirty,
            command: None,
            preview_full: false,
            undo: Vec::new(),
            redo: Vec::new(),
            mode: EditMode::Split,
            last_drawn_cursor: None,
            selection: None,
            anchor_complete: None,
        }
    }

    fn list_note(id: &str, title: &str) -> crate::types::Note {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "title": title,
            "tags": [],
            "lastChangedAt": "2024-01-01T00:00:00.000Z",
            "createdAt": "2024-01-01T00:00:00.000Z",
            "lastChangeUser": null,
            "publishType": "view",
            "publishedAt": null,
            "userPath": null,
            "teamPath": null,
            "permalink": null,
            "shortId": "s",
            "publishLink": format!("https://hackmd.io/{id}"),
            "readPermission": "owner",
            "writePermission": "owner",
        }))
        .expect("note fixture")
    }

    fn team(path: &str, name: &str) -> crate::types::Team {
        serde_json::from_value(serde_json::json!({
            "id": "team-1",
            "ownerId": "u1",
            "name": name,
            "logo": "logo.png",
            "path": path,
            "description": "",
            "hardBreaks": false,
            "visibility": "private",
            "createdAt": "2024-01-01T00:00:00.000Z",
        }))
        .expect("team fixture")
    }

    fn two_workspace_lists() -> CloudLists {
        CloudLists {
            notes: vec![list_note("n1", "Mine"), list_note("n2", "Mine too")],
            teams: vec![crate::tui::cloud::TeamNotes {
                team: team("demo", "Demo Team"),
                notes: vec![list_note("t1", "Team note")],
            }],
        }
    }

    #[test]
    fn cloud_browser_builds_one_tab_per_workspace() {
        let lists = two_workspace_lists();
        let c = CloudBrowser::from_lists(Some(&lists));
        assert_eq!(c.tabs.len(), 2);
        assert!(c.show_tab_bar());
        assert_eq!(c.tabs[0].label, "My notes");
        assert_eq!(c.tabs[1].label, "Demo Team");
        assert_eq!(c.tabs[0].notes.len(), 2);
        assert!(c.tabs[0].notes[0].team_path.is_none());
        assert_eq!(c.tabs[1].notes[0].team_path.as_deref(), Some("demo"));

        // Personal-only account: single tab, no tab bar.
        let solo = CloudBrowser::from_lists(Some(&CloudLists {
            notes: vec![list_note("n1", "Mine")],
            teams: Vec::new(),
        }));
        assert!(!solo.show_tab_bar());
    }

    #[test]
    fn switch_tab_wraps_and_keeps_per_tab_selection() {
        let lists = two_workspace_lists();
        let mut c = CloudBrowser::from_lists(Some(&lists));
        c.move_selection(1, 24);
        assert_eq!(c.tab().unwrap().selected, 1);
        c.switch_tab(1);
        assert_eq!(c.active, 1);
        assert_eq!(c.tab().unwrap().selected, 0, "fresh tab starts at 0");
        c.switch_tab(1);
        assert_eq!(c.active, 0, "wraps past the end");
        assert_eq!(c.tab().unwrap().selected, 1, "selection survived the trip");
        c.switch_tab(-1);
        assert_eq!(c.active, 1, "wraps backwards");
    }

    #[test]
    fn list_refresh_preserves_active_tab_and_selection() {
        let mut app = test_app();
        app.apply_cloud_msg(CloudMsg::Lists(Ok(two_workspace_lists())));
        let _ = app.navigate_to(EntryKind::CloudList, 0);

        if let View::Cloud(c) = &mut app.view {
            c.switch_tab(1);
            c.move_selection(0, 24); // no-op move, stays on t1
        }
        app.apply_cloud_msg(CloudMsg::Lists(Ok(two_workspace_lists())));
        let View::Cloud(c) = &app.view else {
            panic!("expected cloud view");
        };
        assert_eq!(c.active, 1, "active tab survives refresh");
        assert_eq!(c.selected_note().map(|n| n.id.as_str()), Some("t1"));
    }

    #[test]
    fn lists_populate_and_pending_counter_zeroes() {
        let mut app = test_app();
        app.cloud.pending = 2;
        app.apply_cloud_msg(CloudMsg::Lists(Ok(CloudLists {
            notes: Vec::new(),
            teams: Vec::new(),
        })));
        assert_eq!(app.cloud.pending, 1);
        assert!(app.cloud.lists.is_some());

        app.apply_cloud_msg(CloudMsg::Lists(Err("boom".into())));
        assert_eq!(app.cloud.pending, 0);
        assert!(app.status.contains("boom"));

        // A surplus response must saturate, not underflow.
        app.apply_cloud_msg(CloudMsg::Lists(Err("again".into())));
        assert_eq!(app.cloud.pending, 0);
    }

    #[test]
    fn pending_nav_completes_and_pushes_history() {
        let mut app = test_app();
        app.cloud.pending_nav = Some(("n1".into(), 5));
        let h0 = app.history.len();

        app.apply_cloud_msg(CloudMsg::Note {
            id: "n1".into(),
            intent: FetchIntent::OpenReader { scroll: 5 },
            result: Ok(FetchedNote::Fresh {
                note: Box::new(note("n1", "T", "# body")),
                etag: None,
            }),
        });

        assert!(app.cloud.pending_nav.is_none());
        assert_eq!(app.history.len(), h0 + 1, "completion pushes history");
        let View::Reader(r) = &app.view else {
            panic!("expected reader view");
        };
        assert_eq!(r.raw, "# body");
        assert_eq!(r.scroll, 5);
        assert!(matches!(&r.origin, ReaderOrigin::CloudNote { id, .. } if id == "n1"));
    }

    #[test]
    fn open_created_note_enters_editor_with_cursor_on_body() {
        let mut app = test_app();
        let content = "# Hello\n\n\n\n---\n\n*footer*\n";
        let h0 = app.history.len();

        app.open_created_note(note("n9", "Hello", content));

        assert!(app.cloud.note_cache.contains_key("n9"), "cache seeded");
        assert_eq!(app.history.len(), h0 + 1, "local view stays one Esc away");
        let View::Reader(r) = &app.view else {
            panic!("expected reader view");
        };
        assert!(matches!(&r.origin, ReaderOrigin::CloudNote { id, .. } if id == "n9"));
        assert_eq!(r.raw, content);
        let edit = r.edit.as_ref().expect("editor open");
        assert!(!edit.dirty);
        // Cursor on the blank body line between the heading and the rule.
        assert_eq!(edit.cursor, content.find("\n\n").expect("blank line") + 2);
    }

    #[test]
    fn stale_note_response_is_dropped() {
        let mut app = test_app();
        app.cloud.pending_nav = Some(("wanted".into(), 0));
        let h0 = app.history.len();

        app.apply_cloud_msg(fresh("other", "Other", "content"));

        // The nav stays armed, the view and history are untouched, but the
        // response body still lands in the cache.
        assert_eq!(
            app.cloud.pending_nav.as_ref().map(|(id, _)| id.as_str()),
            Some("wanted")
        );
        assert_eq!(app.history.len(), h0);
        assert!(matches!(&app.view, View::Reader(r) if matches!(r.origin, ReaderOrigin::Stdin)));
        assert!(app.cloud.note_cache.contains_key("other"));
    }

    #[test]
    fn failed_fetch_clears_pending_nav_and_keeps_history_clean() {
        let mut app = test_app();
        app.cloud.pending_nav = Some(("n1".into(), 0));
        let h0 = app.history.len();

        app.apply_cloud_msg(CloudMsg::Note {
            id: "n1".into(),
            intent: FetchIntent::OpenReader { scroll: 0 },
            result: Err("404".into()),
        });

        assert!(app.cloud.pending_nav.is_none());
        assert_eq!(app.history.len(), h0);
        assert!(app.status.contains("404"));
    }

    #[test]
    fn edit_selection_copy_and_delete() {
        // "hello world" — select bytes 6..11 ("world").
        let mut app = test_app();
        let n = note("n1", "T", "hello world");
        let mut r = Reader::from_cloud(&n, None);
        let mut e = edit_state(false);
        e.selection = Some(EditSelection {
            anchor: 6,
            focus: 11,
            origin: 6,
            dragged: true,
        });
        r.edit = Some(e);
        app.view = View::Reader(r);

        assert_eq!(app.edit_selection_text().as_deref(), Some("world"));

        app.edit_delete_selection();
        let View::Reader(r) = &app.view else {
            panic!("expected reader");
        };
        assert_eq!(r.raw, "hello ");
        let e = r.edit.as_ref().expect("editing");
        assert_eq!(e.cursor, 6);
        assert!(e.dirty);
        assert!(e.selection.is_none(), "selection cleared after delete");
        assert_eq!(e.undo.len(), 1, "one undo snapshot for the delete");

        // A merely armed (never dragged) selection is inert: no text, and
        // delete is a no-op.
        let mut app = test_app();
        let mut r = Reader::from_cloud(&n, None);
        let mut e = edit_state(false);
        e.selection = Some(EditSelection {
            anchor: 3,
            focus: 3,
            origin: 3,
            dragged: false,
        });
        r.edit = Some(e);
        app.view = View::Reader(r);
        assert_eq!(app.edit_selection_text(), None);
        app.edit_delete_selection();
        let View::Reader(r) = &app.view else {
            panic!("expected reader");
        };
        assert_eq!(r.raw, "hello world");
        assert!(!r.edit.as_ref().expect("editing").dirty);
    }

    #[test]
    fn edit_link_selection_wraps_in_markdown_link() {
        // "see hackmd here" — select "hackmd" (bytes 4..10).
        let mut app = test_app();
        let n = note("n1", "T", "see hackmd here");
        let mut r = Reader::from_cloud(&n, None);
        let mut e = edit_state(false);
        e.selection = Some(EditSelection {
            anchor: 4,
            focus: 10,
            origin: 4,
            dragged: true,
        });
        r.edit = Some(e);
        app.view = View::Reader(r);

        assert!(app.edit_link_selection("https://hackmd.io"));
        let View::Reader(r) = &app.view else {
            panic!("expected reader");
        };
        assert_eq!(r.raw, "see [hackmd](https://hackmd.io) here");
        let e = r.edit.as_ref().expect("editing");
        // Cursor sits just past the closing paren.
        assert_eq!(e.cursor, "see [hackmd](https://hackmd.io)".len());
        assert!(e.selection.is_none());
        assert_eq!(e.undo.len(), 1);

        // No active selection → no-op.
        let mut app = test_app();
        let mut r = Reader::from_cloud(&n, None);
        r.edit = Some(edit_state(false));
        app.view = View::Reader(r);
        assert!(!app.edit_link_selection("https://x.io"));
    }

    #[test]
    fn edit_select_inclusive_covers_both_end_chars() {
        // "hello world": click char 'e' (byte 1), shift-click 'r' (byte 8).
        let mut app = test_app();
        let n = note("n1", "T", "hello world");
        let mut r = Reader::from_cloud(&n, None);
        r.edit = Some(edit_state(false));
        app.view = View::Reader(r);

        app.edit_select_inclusive(1, 8);
        // Inclusive of both 'e' and 'r' → bytes 1..9 == "ello wor".
        assert_eq!(app.edit_selection_text().as_deref(), Some("ello wor"));

        // Order-independent: same span regardless of click order.
        app.edit_select_inclusive(8, 1);
        assert_eq!(app.edit_selection_text().as_deref(), Some("ello wor"));
    }

    #[test]
    fn edit_extend_grows_and_collapses_selection() {
        // "hello world", cursor at 0; Shift+word-right then back.
        let mut app = test_app();
        let n = note("n1", "T", "hello world");
        let mut r = Reader::from_cloud(&n, None);
        let mut e = edit_state(false);
        e.cursor = 0;
        r.edit = Some(e);
        app.view = View::Reader(r);

        app.edit_extend_word(1); // select "hello"
        assert_eq!(app.edit_selection_text().as_deref(), Some("hello"));

        app.edit_extend_word(1); // extend to "hello world"
        assert_eq!(app.edit_selection_text().as_deref(), Some("hello world"));

        // Collapsing back onto the anchor drops the selection.
        app.edit_extend_word(-1);
        app.edit_extend_word(-1);
        assert!(app.edit_selection_text().is_none());
    }

    #[test]
    fn saved_err_keeps_dirty_marker() {
        let mut app = test_app();
        let n = note("n1", "T", "body");
        let mut r = Reader::from_cloud(&n, None);
        r.edit = Some(edit_state(true));
        app.view = View::Reader(r);
        app.cloud.saving.insert("n1".into());

        app.apply_cloud_msg(CloudMsg::Saved {
            id: "n1".into(),
            base_file: None,
            result: Err("500".into()),
        });

        assert!(app.cloud.saving.is_empty(), "in-flight guard released");
        let View::Reader(r) = &app.view else {
            panic!("expected reader");
        };
        assert!(
            r.edit.as_ref().expect("still editing").dirty,
            "failed PATCH must never clear dirty"
        );
        assert!(app.status.contains("save failed"));
    }

    #[test]
    fn saved_advances_base_only_for_linked_file_saves() {
        // Regression: a cloud-only edit of a note that also has a linked local
        // file must NOT move the sync base, or the next sync would push the
        // stale local file back and revert the cloud edit.
        let dir = std::env::temp_dir().join(format!("hackmd-base-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp root");

        let mut app = test_app();
        app.root = dir.clone();
        let file = dir.join("note.md");

        // Cloud-only save (base_file: None) leaves the base absent.
        app.apply_cloud_msg(CloudMsg::Saved {
            id: "n1".into(),
            base_file: None,
            result: Ok("cloud edit".to_string()),
        });
        assert!(
            crate::tui::sync::read_base(&dir, "n1", &file).is_none(),
            "cloud-only save must not write the base"
        );

        // A linked-file save (base_file: Some) advances that file's base.
        app.apply_cloud_msg(CloudMsg::Saved {
            id: "n1".into(),
            base_file: Some(file.clone()),
            result: Ok("local content".to_string()),
        });
        assert_eq!(
            crate::tui::sync::read_base(&dir, "n1", &file).as_deref(),
            Some("local content"),
            "linked-file save must advance the base"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saved_ok_clears_dirty_only_when_buffer_matches() {
        let mut app = test_app();
        let n = note("n1", "T", "body");
        let mut r = Reader::from_cloud(&n, None);
        r.edit = Some(edit_state(true));
        app.view = View::Reader(r);

        app.apply_cloud_msg(CloudMsg::Saved {
            id: "n1".into(),
            base_file: None,
            result: Ok("body".to_string()),
        });
        let View::Reader(r) = &app.view else {
            panic!("expected reader");
        };
        assert!(!r.edit.as_ref().expect("editing").dirty);

        // Type more, then a (stale) save confirmation for the older content
        // arrives — dirty must survive.
        let mut app = test_app();
        let mut r = Reader::from_cloud(&n, None);
        r.raw = "body plus unsent keystrokes".into();
        r.edit = Some(edit_state(true));
        app.view = View::Reader(r);
        app.apply_cloud_msg(CloudMsg::Saved {
            id: "n1".into(),
            base_file: None,
            result: Ok("body".to_string()),
        });
        let View::Reader(r) = &app.view else {
            panic!("expected reader");
        };
        assert!(r.edit.as_ref().expect("editing").dirty);
    }

    #[test]
    fn open_reader_refreshes_in_place_without_nav() {
        let mut app = test_app();
        let n = note("n1", "T", "old content");
        app.view = View::Reader(Reader::from_cloud(&n, None));

        app.apply_cloud_msg(fresh("n1", "T", "new content"));

        let View::Reader(r) = &app.view else {
            panic!("expected reader");
        };
        assert_eq!(r.raw, "new content");
        assert_eq!(app.status, "Note updated remotely");
    }

    #[test]
    fn deleted_note_drops_reader_back_to_cloud_browser() {
        let mut app = test_app();
        let n = note("n1", "T", "body");
        app.view = View::Reader(Reader::from_cloud(&n, None));
        app.cloud.note_cache.insert(
            "n1".into(),
            CachedNote {
                note: n,
                etag: None,
            },
        );

        app.apply_cloud_msg(CloudMsg::Deleted {
            id: "n1".into(),
            title: "T".into(),
            result: Ok(()),
        });

        assert!(!app.cloud.note_cache.contains_key("n1"));
        assert!(matches!(app.view, View::Cloud(_)));
        assert!(app.status.contains("Deleted"));
    }

    #[test]
    fn slugify_makes_safe_filenames() {
        assert_eq!(slugify("Meeting Notes 2024"), "meeting-notes-2024");
        assert_eq!(slugify("  --weird__ / title!  "), "weird-title");
        assert_eq!(slugify("???"), "note");
    }

    #[test]
    fn first_h1_extracts_title_or_none() {
        assert_eq!(
            first_h1("# Hello World\n\nbody").as_deref(),
            Some("Hello World")
        );
        // Leading prose then an H1 still counts.
        assert_eq!(
            first_h1("intro line\n# Real Title\n").as_deref(),
            Some("Real Title")
        );
        // `##` and deeper are not H1s.
        assert_eq!(first_h1("## Sub\n### Deep\n").as_deref(), None);
        // Closed ATX (`# Title #`) trims the trailing hashes.
        assert_eq!(first_h1("# Title #\n").as_deref(), Some("Title"));
        // YAML front matter is skipped so its `title:` doesn't shadow the H1.
        assert_eq!(
            first_h1("---\ntitle: Front\n---\n# Body Heading\n").as_deref(),
            Some("Body Heading")
        );
        // No heading at all.
        assert_eq!(first_h1("just text\nmore text\n"), None);
        // `#nospace` is not a heading.
        assert_eq!(first_h1("#nospace\n"), None);
    }

    // ---- Heading-anchor autocomplete inside `[](…#…)` ----

    /// A file-backed app dropped into edit mode over `raw`, cursor at byte
    /// `cursor`. Stdin readers can't edit, so anchor-complete tests need a real
    /// file origin; the returned `TempDir` keeps it alive for the test.
    fn edit_app(raw: &str, cursor: usize) -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "placeholder\n").unwrap();
        let mut app = test_app();
        open_file_reader(&mut app, &path);
        edit_with(&mut app, raw, cursor);
        (dir, app)
    }

    /// Drop an already-file-backed `app` into edit mode over `raw`.
    fn edit_with(app: &mut App, raw: &str, cursor: usize) {
        app.enter_edit();
        if let View::Reader(r) = &mut app.view {
            r.raw = raw.to_string();
            let e = r.edit.as_mut().unwrap();
            e.cursor = cursor;
        }
    }

    fn popup_slugs(app: &App) -> Vec<String> {
        let View::Reader(r) = &app.view else {
            return Vec::new();
        };
        let Some(ac) = r.edit.as_ref().and_then(|e| e.anchor_complete.as_ref()) else {
            return Vec::new();
        };
        ac.matches
            .iter()
            .map(|&i| ac.candidates[i].slug.clone())
            .collect()
    }

    /// `anchor_context` only fires inside an open `](…)` destination that has a
    /// `#` before the cursor.
    #[test]
    fn anchor_context_detects_link_destinations() {
        // Current-doc anchor, empty path.
        let raw = "[link](#se";
        assert_eq!(
            anchor_context(raw, raw.len()),
            Some((raw.find('#').unwrap(), ""))
        );
        // Path + anchor.
        let raw = "[x](other.md#in";
        assert_eq!(
            anchor_context(raw, raw.len()),
            Some((raw.find('#').unwrap(), "other.md"))
        );
        // A markdown heading is not a link destination.
        assert_eq!(anchor_context("# Title", 7), None);
        // No `#` yet → nothing to complete.
        assert_eq!(anchor_context("[x](other.md", 12), None);
        // Bare parens (no preceding `]`) are not a destination.
        assert_eq!(anchor_context("(#foo", 5), None);
        // A closed destination before the cursor doesn't count.
        assert_eq!(anchor_context("[x](#a) more", 12), None);
        // A space after `#` (link title) disqualifies it.
        assert_eq!(anchor_context("[x](#a b", 8), None);
    }

    /// Typing `#` inside a link destination opens the popup listing the current
    /// document's headings.
    #[test]
    fn anchor_complete_lists_current_doc_headings() {
        let raw = "# Hello World\n\n## Sub Section\n\n[link](#)\n";
        let hash = raw.find("(#").unwrap() + 1;
        let (_dir, mut app) = edit_app(raw, hash + 1);
        app.edit_anchor_sync(true);
        assert_eq!(popup_slugs(&app), vec!["hello-world", "sub-section"]);
    }

    /// The query after `#` filters the list.
    #[test]
    fn anchor_complete_filters_by_query() {
        let raw = "# Hello World\n\n## Sub Section\n\n[link](#sub)\n";
        let hash = raw.find("(#").unwrap() + 1;
        // Cursor just past "sub".
        let (_dir, mut app) = edit_app(raw, hash + 1 + 3);
        app.edit_anchor_sync(true);
        assert_eq!(popup_slugs(&app), vec!["sub-section"]);
    }

    /// Accepting writes the selected slug in place of the typed query and closes
    /// the popup.
    #[test]
    fn anchor_complete_accept_writes_slug() {
        let raw = "# Hello World\n\n## Sub Section\n\n[link](#su)\n";
        let hash = raw.find("(#").unwrap() + 1;
        let (_dir, mut app) = edit_app(raw, hash + 1 + 2);
        app.edit_anchor_sync(true);
        app.edit_anchor_accept();
        let View::Reader(r) = &app.view else {
            panic!("reader")
        };
        assert!(r.raw.contains("[link](#sub-section)"), "raw = {:?}", r.raw);
        assert!(r.edit.as_ref().unwrap().anchor_complete.is_none());
        assert!(r.edit.as_ref().unwrap().dirty);
    }

    /// Up/Down wrap around the match list.
    #[test]
    fn anchor_complete_move_wraps() {
        let raw = "# One\n\n## Two\n\n[link](#)\n";
        let hash = raw.find("(#").unwrap() + 1;
        let (_dir, mut app) = edit_app(raw, hash + 1);
        app.edit_anchor_sync(true);
        app.edit_anchor_move(-1); // wrap from 0 to last
        let View::Reader(r) = &app.view else {
            panic!("reader")
        };
        assert_eq!(
            r.edit
                .as_ref()
                .unwrap()
                .anchor_complete
                .as_ref()
                .unwrap()
                .selected,
            1
        );
    }

    /// A link to another file completes against that file's headings on disk.
    #[test]
    fn anchor_complete_lists_referenced_file_headings() {
        let mut app = test_app();
        let dir = tempfile::tempdir().unwrap();
        let other = dir.path().join("other.md");
        std::fs::write(&other, "# Far Away\n\n## Nested Bit\n").unwrap();
        let here = dir.path().join("here.md");
        std::fs::write(&here, "start\n").unwrap();
        open_file_reader(&mut app, &here);

        let raw = "See [x](other.md#)\n";
        let hash = raw.find("(other.md#").unwrap() + "(other.md".len();
        edit_with(&mut app, raw, hash + 1);
        app.edit_anchor_sync(true);
        assert_eq!(popup_slugs(&app), vec!["far-away", "nested-bit"]);
    }

    /// Only a typed `#` opens the popup; merely moving the cursor past an
    /// existing `#` (allow_open = false) leaves it closed.
    #[test]
    fn anchor_complete_does_not_open_on_mere_motion() {
        let raw = "# Hello\n\n[link](#)\n";
        let hash = raw.find("(#").unwrap() + 1;
        let (_dir, mut app) = edit_app(raw, hash + 1);
        app.edit_anchor_sync(false);
        assert!(popup_slugs(&app).is_empty());
    }
}

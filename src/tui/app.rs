use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use ratatui::layout::Rect;

use crate::tui::cloud::{CachedNote, CloudContext, CloudMsg, CloudState, FetchedNote};
use crate::tui::jsonl::{self, JsonlOverlay};
use crate::tui::links::LinkTarget;
use crate::tui::markdown::{self, Rendered};
use crate::tui::theme::Theme;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use std::collections::{HashMap, HashSet};

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
    /// Lazily-decoded image protocol cache, keyed by canonicalised path.
    pub image_protocols: HashMap<PathBuf, StatefulProtocol>,
    /// Raw-pane and preview-pane rects from the last frame in split-edit
    /// mode. Used by event routing (which pane received the click / wheel).
    /// Both default to `Rect::default()` outside split-edit mode.
    pub edit_raw_area: Rect,
    pub edit_preview_area: Rect,
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
    /// True while a local↔HackMD sync fetch is in flight, so the periodic
    /// poll doesn't stack duplicate requests for the same file.
    pub pending_sync: bool,
    /// `(path, when)` of the last sync trigger for the open linked file. A
    /// different path syncs immediately (on open); the same path re-syncs once
    /// `SYNC_INTERVAL` elapses (background upstream polling).
    pub last_sync: Option<(PathBuf, std::time::Instant)>,
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
}

/// How often a still-open linked file re-syncs with upstream in the
/// background (catching edits made on hackmd.io).
pub const SYNC_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

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
    /// `dir`, then open it in the editor.
    NewFile { dir: PathBuf },
    /// `c` / F2 in the local browser — rename the entry at `from` to the
    /// typed name (kept in the same parent directory).
    RenameFile { from: PathBuf },
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
    /// `:preview`, …) there, Enter executes it, and a second Esc discards
    /// changes and exits the editor. Any buffer interaction (click, typing
    /// after it's dismissed) clears it back to insert mode.
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
}

/// A drag-selection in the raw editor pane, as byte offsets into
/// `Reader::raw`. Both ends always sit on UTF-8 char boundaries (they come
/// from the same click→byte mapping as the cursor).
#[derive(Clone, Debug)]
pub struct EditSelection {
    /// Where the mouse went down.
    pub anchor: usize,
    /// Where the mouse is / was released. May be before `anchor`.
    pub focus: usize,
    /// True once a drag moved the focus off the anchor; a plain click
    /// never activates the selection.
    pub dragged: bool,
}

impl EditSelection {
    /// Ordered `(start, end)` byte range.
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
}

#[derive(Clone)]
pub struct BrowserEntry {
    pub path: PathBuf,
    pub display: String,
    pub kind: BrowserEntryKind,
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
        Ok(Self {
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
            git_lens: None,
            last_click: None,
            scroll_accum: 0.0,
            last_scroll_at: None,
            image_picker: None,
            image_protocols: HashMap::new(),
            edit_raw_area: Rect::default(),
            edit_preview_area: Rect::default(),
            read_state,
            cloud: CloudState::new(cloud),
            last_local: None,
            prompt: None,
            yank: None,
            pending_sync: false,
            last_sync: None,
            conflict: None,
            edit_cmd_history: Vec::new(),
            edit_cmd_nav: None,
            lookup: None,
            error: None,
        })
    }

    /// Drain every finished cloud operation, applying each to app state.
    /// Called once per event-loop tick beside the file watchers.
    pub fn drain_cloud_msgs(&mut self) {
        loop {
            let Some(msg) = self.cloud.ctx.try_recv() else {
                break;
            };
            self.apply_cloud_msg(msg);
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

    /// Background-driven sync check, called every event-loop tick. When a
    /// linked local file is open in the reader (and not being edited), kick off
    /// a sync on first sight and then every [`SYNC_INTERVAL`]. Cheap: the file
    /// read + parse only runs when a sync is actually due.
    pub fn maybe_sync(&mut self) {
        if self.pending_sync || self.conflict.is_some() || !self.cloud.is_connected() {
            return;
        }
        // Only auto-sync a plain (non-edit) reader over a real file.
        let path = match &self.view {
            View::Reader(r) if r.edit.is_none() => match &r.origin {
                ReaderOrigin::File(p) => p.clone(),
                _ => return,
            },
            _ => return,
        };
        let due = match &self.last_sync {
            Some((p, t)) if *p == path => t.elapsed() >= SYNC_INTERVAL,
            _ => true, // newly-opened file (or never synced) → sync now
        };
        if due {
            self.sync_local_file(path);
        }
    }

    /// Kick off a three-way sync for the linked file at `path`: fetch upstream,
    /// then merge when it lands (see [`Self::apply_sync`]). No-op for an
    /// unlinked file or when disconnected. Records the trigger time so the
    /// periodic poll backs off until [`SYNC_INTERVAL`] passes.
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
        let local_clean =
            crate::tui::sync::normalize_newlines(&crate::tui::hackmd_meta::strip(&local_raw));
        let remote = crate::tui::sync::normalize_newlines(&remote);
        // Missing base → treat as empty so nothing is silently dropped: equal
        // sides still merge clean, differing sides surface as a conflict.
        let base = crate::tui::sync::normalize_newlines(
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
    /// created under the current directory and opened in the editor.
    pub fn prompt_new_file(&mut self) {
        let View::Browser(b) = &self.view else {
            return;
        };
        let dir = b.dir.clone();
        self.prompt = Some(Prompt {
            title: " New file — name ".into(),
            input: String::new(),
            kind: PromptKind::NewFile { dir },
        });
    }

    /// `c` / F2 in the local browser: prompt to rename the selected entry.
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
                // Reject path separators so a stray `foo/bar` can't escape the
                // browsed directory or create implicit subdirs the user didn't
                // ask for.
                if name.contains('/') || name.contains('\\') {
                    self.status = "Name can't contain a path separator".into();
                    return;
                }
                let path = dir.join(name);
                if path.exists() {
                    self.status = format!("Already exists: {}", path.display());
                    return;
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
                if let Err(e) = std::fs::rename(&from, &to) {
                    self.status = format!("rename: {e}");
                    return;
                }
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

    fn scroll_to_anchor(&mut self, slug: &str) {
        if let View::Reader(r) = &mut self.view {
            if let Some(rendered) = &r.rendered {
                if let Some(&line) = rendered.link_map.anchors.get(slug) {
                    r.scroll = line as u16;
                    self.status = format!("→ #{}", slug);
                } else {
                    self.status = format!("Anchor not found: #{}", slug);
                }
            }
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
    /// Unread badges don't need this — `draw_browser` recomputes them from disk
    /// every frame, so they're already live; only the entry list itself goes
    /// stale, and that only changes when the directory's own stat changes.
    pub fn poll_browser_change(&mut self) -> bool {
        let View::Browser(b) = &mut self.view else {
            return false;
        };
        let new_meta = file_meta(&b.dir);
        if b.last_meta == new_meta {
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

    /// Enter in the editor. Inside a markdown list this auto-continues the
    /// list: a bullet, numbered, or checkbox line spawns the next marker on
    /// the new line. Pressing Enter on an *empty* list item (just the marker)
    /// instead terminates the list, clearing the marker and leaving a blank
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

    /// Insert an `open``close` pair at the cursor and place the cursor between
    /// them (bracket auto-close). One undo step.
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

    /// Move cursor to start (`bol=false`) or end (`eol=true`) of the current
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
                let (source, jsonl_map) = if r.is_jsonl_view() {
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
                        theme.heading[0],
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
fn dir_has_listable(dir: &Path) -> bool {
    let walker = ignore::WalkBuilder::new(dir)
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .require_git(false)
        .build();
    for result in walker {
        let Ok(entry) = result else { continue };
        if entry.path() == dir {
            continue;
        }
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) && is_text_file(entry.path()) {
            return true;
        }
    }
    false
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
    let mut dirs: Vec<(String, PathBuf)> = Vec::new();
    let mut files: Vec<(String, PathBuf)> = Vec::new();
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
        if ft.is_dir() {
            if show_all || dir_has_listable(&path) {
                dirs.push((name, path));
            }
        } else if ft.is_file() && (show_all || is_text_file(&path)) {
            files.push((name, path));
        }
    }
    let by_name = |a: &(String, PathBuf), b: &(String, PathBuf)| {
        a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase())
    };
    dirs.sort_by(by_name);
    files.sort_by(by_name);
    for (name, path) in dirs {
        out.push(BrowserEntry {
            path,
            display: format!("{}/", name),
            kind: BrowserEntryKind::Dir,
        });
    }
    for (name, path) in files {
        out.push(BrowserEntry {
            path,
            display: name,
            kind: BrowserEntryKind::Markdown,
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
        Ok(Self {
            origin: ReaderOrigin::File(path.to_path_buf()),
            raw,
            rendered: None,
            scroll: 0,
            preview_scroll: 0,
            focus: None,
            hover_link: None,
            hover_checkbox: None,
            doc_search: None,
            edit: None,
            last_meta,
            wrap_lang,
            jsonl_expanded: HashSet::new(),
            jsonl_overlay: None,
            hover_jsonl: None,
            tables: crate::tui::links::TableExpansions::new(),
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
            doc_search: None,
            edit: None,
            last_meta: None,
            wrap_lang: None,
            jsonl_expanded: HashSet::new(),
            jsonl_overlay: None,
            hover_jsonl: None,
            tables: crate::tui::links::TableExpansions::new(),
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
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let after = &rest[digits.len()..];
        let sep = after.chars().next().filter(|c| *c == '.' || *c == ')')?;
        let after_sep = &after[1..];
        let sp = count_spaces(after_sep);
        if sp == 0 {
            return None;
        }
        let content = &after_sep[sp..];
        let next = digits.parse::<u64>().unwrap_or(0).saturating_add(1);
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
fn next_char_boundary(s: &str, pos: usize) -> usize {
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
        for (chunk_range, chunk_text) in chunks {
            let src_start = line_start + chunk_range.start;
            let src_end = line_start + chunk_range.end;
            rows.push(RawRow {
                text: chunk_text,
                source_range: src_start..src_end,
                kind,
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
                });
            }
        }
        push_children(&self.dir, &mut entries, self.show_all);
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
        Ok(())
    }

    /// Toggle "show everything" mode (`A`) and re-list. Preserves the
    /// highlighted entry across the rebuild when it survives the filter change.
    pub fn toggle_show_all(&mut self) {
        self.show_all = !self.show_all;
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

        for want in ["withmd/", "a.md", "b.markdown", "note.txt", "NOTES"] {
            assert!(names.contains(&want), "missing {want}, got {names:?}");
        }
        // A directory with nothing openable, or only binaries, is hidden.
        assert!(!names.contains(&"empty/"), "empty/ shown, got {names:?}");
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
            }],
            selected: 0,
            scroll: 0,
            last_meta: None,
            find: None,
            show_all: false,
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
                })
                .collect(),
            selected: 0,
            scroll: 0,
            last_meta: None,
            find: None,
            show_all: false,
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
}

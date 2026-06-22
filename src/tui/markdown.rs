//! Markdown → ratatui rendering.
//!
//! Walks `pulldown-cmark` events into a token list, then runs a layout pass
//! that produces wrapped `Line<'static>`s plus a parallel `LinkMap` recording
//! click targets in (line, col_start, col_end) space.

use std::path::{Path, PathBuf};

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::tui::links::{
    self, CheckboxMap, CheckboxSpan, ImageRef, LinkMap, LinkSpan, LinkTarget, TableExpand,
    TableExpansions, TableMap, TableRegion,
};
use crate::tui::syntax;
use crate::tui::theme::Theme;

#[derive(Clone, Debug)]
pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    pub link_map: LinkMap,
    pub checkbox_map: CheckboxMap,
    /// Click-to-expand hit-test geometry for every table in the document.
    pub table_map: TableMap,
    pub images: Vec<ImageRef>,
    pub width: u16,
    /// One entry per block in document order. Lets edit mode locate the
    /// block containing a given source byte offset.
    pub blocks: Vec<BlockInfo>,
    /// Document outline: every heading in order, with its display line.
    /// Drives the TOC popup (`t`) and `]]` / `[[` heading jumps.
    pub headings: Vec<HeadingInfo>,
    /// Cursor display position when rendered in edit mode. `None` outside
    /// edit mode.
    pub cursor_xy: Option<(u16, u16)>,
    /// Per-display-row source byte range. `Some(range)` for rows in raw-
    /// substituted blocks (the cursor's block in edit mode); `None` for
    /// formatted rows where bytes don't map 1:1 with display columns. Used
    /// by edit mode to convert mouse clicks and Up/Down keys into source
    /// byte offsets in display-row space (which respects soft-wrap).
    pub row_source: Vec<Option<std::ops::Range<usize>>>,
}

/// One heading in the document outline.
#[derive(Clone, Debug)]
pub struct HeadingInfo {
    /// 1–6 (`#` … `######`).
    pub level: u8,
    /// Plain heading text with inline markup stripped.
    pub text: String,
    /// Display line index into `Rendered::lines`.
    pub line: usize,
}

/// Source-byte range + display-line range for one block. `display_start`/
/// `display_end` are line indices into `Rendered::lines` (half-open). Edit
/// mode uses these to map (cursor_offset → block) and to scroll a freshly
/// raw-substituted block back into view.
#[derive(Clone, Debug)]
pub struct BlockInfo {
    pub source_range: std::ops::Range<usize>,
    pub display_start: usize,
    pub display_end: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EditCtx {
    /// Source byte offset of the cursor.
    pub cursor: usize,
}

/// Render markdown to terminal lines. Convenience wrapper around
/// `render_with_edit`; pass `None` for the edit context when not editing.
#[cfg(test)]
pub fn render(source: &str, base_dir: Option<&Path>, width: u16, theme: &Theme) -> Rendered {
    render_with_edit(
        source,
        base_dir,
        width,
        theme,
        None,
        &TableExpansions::new(),
    )
}

pub fn render_with_edit(
    source: &str,
    base_dir: Option<&Path>,
    width: u16,
    theme: &Theme,
    edit: Option<EditCtx>,
    tables: &TableExpansions,
) -> Rendered {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_SMART_PUNCTUATION);
    opts.insert(Options::ENABLE_WIKILINKS);

    let parser = Parser::new_ext(source, opts).into_offset_iter();
    let mut b = Builder::new(
        theme.clone(),
        width as usize,
        base_dir.map(|p| p.to_path_buf()),
        source.to_string(),
        edit,
        tables.clone(),
    );
    for (ev, range) in parser {
        b.event(ev, range);
    }
    b.finish()
}

#[derive(Clone, Debug, Default)]
struct Run {
    text: String,
    style: Style,
    /// Index into `links` if this run is part of a hyperlink.
    link: Option<usize>,
    /// Index into `checkboxes` if this run is the `[ ]`/`[x]` glyph of a task item.
    checkbox: Option<usize>,
    /// Index into `images` if this run is an image placeholder. Recorded so
    /// the layout pass can map (image idx → output line) for later rendering.
    image: Option<usize>,
    /// Source byte range of the *innermost* inline element this run is part
    /// of (Strong/Emphasis/Strikethrough/Link/Image/Code). Edit mode uses
    /// this to find the smallest enclosing element to swap for raw source.
    /// `None` for plain paragraph/heading text not wrapped in inline syntax.
    inline_range: Option<std::ops::Range<usize>>,
    /// When `Some(byte_offset)`, the cursor sits at this byte position within
    /// `text`. Set by the edit-mode substitution pass on the synthetic raw
    /// run; the layout pass watches for it and records the resulting display
    /// (col, line) in `Rendered::cursor_xy`.
    cursor_at: Option<usize>,
}

#[derive(Clone, Debug)]
struct BlockEntry {
    block: Block,
    /// Source byte range. `0..0` for synthetic Blank blocks emitted by the
    /// builder to insert vertical spacing — those have no analogue in source.
    source_range: std::ops::Range<usize>,
}

#[derive(Clone, Debug)]
enum Block {
    /// Flowing paragraph — word-wrapped.
    Paragraph {
        runs: Vec<Run>,
        prefix: Vec<Run>,
        hanging: Vec<Run>,
    },
    /// Heading — word-wrapped, anchor recorded. `level` is 1–6 and `text`
    /// the plain (markup-stripped) heading text; both feed the document
    /// outline (`Rendered::headings`).
    Heading {
        runs: Vec<Run>,
        anchor: String,
        level: u8,
        text: String,
    },
    /// Pre-formatted block — rendered line-by-line, not wrapped. `flat` is
    /// true for the synthetic raw block produced by edit-mode substitution:
    /// it skips the 2-col left pad and code background so the on-screen
    /// content lines up with the view-mode rendering of the same source.
    /// `line_sources[i]` is the source byte range for `lines[i]` when this
    /// is an edit-mode raw substitution; empty for real code fences.
    Pre {
        lines: Vec<Vec<Run>>,
        prefix: Vec<Run>,
        flat: bool,
        line_sources: Vec<Option<std::ops::Range<usize>>>,
    },
    /// Table — column-aligned with box-drawing borders.
    Table {
        alignments: Vec<Alignment>,
        header: Vec<Vec<Run>>,
        rows: Vec<Vec<Vec<Run>>>,
    },
    /// Horizontal rule.
    Rule,
    /// Empty line.
    Blank,
}

struct Builder {
    theme: Theme,
    width: usize,
    base_dir: Option<PathBuf>,
    source: String,
    edit: Option<EditCtx>,

    blocks: Vec<BlockEntry>,
    /// Pending links awaiting layout — index in this vec is referenced by `Run.link`.
    links: Vec<PendingLink>,
    /// Pending task-list checkboxes awaiting layout — index referenced by `Run.checkbox`.
    checkboxes: Vec<PendingCheckbox>,
    /// Image embed paths awaiting layout. Index is referenced by `Run.image`.
    images: Vec<PathBuf>,

    // assembly state for current block
    cur_runs: Vec<Run>,
    cur_prefix: Vec<Run>,
    cur_hanging: Vec<Run>,
    style_stack: Vec<Style>,

    // structural state
    list_stack: Vec<ListFrame>,
    quote_depth: usize,
    in_heading: Option<HeadingLevel>,
    heading_buf: String,

    // code block state
    code_lang: Option<String>,
    code_content: String,
    in_code_block: bool,

    // link state
    open_link: Option<usize>,

    // Innermost-first stack of inline-element source ranges. Push on
    // Tag::Strong/Emphasis/Strikethrough/Link/Image; pop on End. The top of
    // stack tags every Run created while inside via `Run::inline_range`.
    inline_range_stack: Vec<std::ops::Range<usize>>,

    // table state
    table: Option<TableState>,
    /// Per-table click-to-expand state, keyed by source byte offset. Threaded
    /// into the layout pass so expanded cells/columns/tables render full.
    tables: TableExpansions,
}

struct TableState {
    alignments: Vec<Alignment>,
    header: Vec<Vec<Run>>,
    rows: Vec<Vec<Vec<Run>>>,
    current_row: Vec<Vec<Run>>,
    cell_start: usize,
}

struct PendingLink {
    target: LinkTarget,
}

struct PendingCheckbox {
    /// Byte offset in source where the `[` character begins.
    source_offset: usize,
    checked: bool,
}

#[derive(Clone, Debug)]
struct ListFrame {
    ordered: Option<u64>,
}

impl Builder {
    fn new(
        theme: Theme,
        width: usize,
        base_dir: Option<PathBuf>,
        source: String,
        edit: Option<EditCtx>,
        tables: TableExpansions,
    ) -> Self {
        let width = if width == 0 { 80 } else { width };
        Self {
            theme,
            width,
            base_dir,
            source,
            edit,
            blocks: Vec::new(),
            links: Vec::new(),
            checkboxes: Vec::new(),
            images: Vec::new(),
            cur_runs: Vec::new(),
            cur_prefix: Vec::new(),
            cur_hanging: Vec::new(),
            style_stack: vec![Style::default()],
            list_stack: Vec::new(),
            quote_depth: 0,
            in_heading: None,
            heading_buf: String::new(),
            code_lang: None,
            code_content: String::new(),
            in_code_block: false,
            open_link: None,
            inline_range_stack: Vec::new(),
            table: None,
            tables,
        }
    }

    fn cur_inline_range(&self) -> Option<std::ops::Range<usize>> {
        self.inline_range_stack.last().cloned()
    }

    fn cur_style(&self) -> Style {
        *self.style_stack.last().unwrap()
    }

    fn push_style(&mut self, mods: impl FnOnce(Style) -> Style) {
        let s = mods(self.cur_style());
        self.style_stack.push(s);
    }

    fn pop_style(&mut self) {
        if self.style_stack.len() > 1 {
            self.style_stack.pop();
        }
    }

    fn quote_prefix(&self) -> Vec<Run> {
        if self.quote_depth == 0 {
            return Vec::new();
        }
        let bar = "│ ".repeat(self.quote_depth);
        vec![Run {
            text: bar,
            style: Style::default().fg(self.theme.quote),
            link: None,
            checkbox: None,
            image: None,
            inline_range: None,
            cursor_at: None,
        }]
    }

    fn list_prefixes(&mut self) -> (Vec<Run>, Vec<Run>) {
        if self.list_stack.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let depth = self.list_stack.len();
        let indent = "  ".repeat(depth - 1);
        let frame = self.list_stack.last_mut().unwrap();
        let marker = match &mut frame.ordered {
            Some(n) => {
                let s = format!("{}. ", *n);
                *n += 1;
                s
            }
            None => "• ".to_string(),
        };
        let pad = " ".repeat(marker.chars().count());
        let style = Style::default().fg(self.theme.list_marker);
        let prefix = vec![
            Run {
                text: indent.clone(),
                style: Style::default(),
                link: None,
                checkbox: None,
                image: None,
                inline_range: None,
                cursor_at: None,
            },
            Run {
                text: marker,
                style,
                link: None,
                checkbox: None,
                image: None,
                inline_range: None,
                cursor_at: None,
            },
        ];
        let hanging = vec![Run {
            text: format!("{}{}", indent, pad),
            style: Style::default(),
            link: None,
            checkbox: None,
            image: None,
            inline_range: None,
            cursor_at: None,
        }];
        (prefix, hanging)
    }

    fn push_block(&mut self, block: Block, source_range: std::ops::Range<usize>) {
        self.blocks.push(BlockEntry {
            block,
            source_range,
        });
    }

    /// Spacer block emitted between content blocks. No source range — the
    /// renderer treats the cursor as never landing on these.
    fn push_blank(&mut self) {
        self.blocks.push(BlockEntry {
            block: Block::Blank,
            source_range: 0..0,
        });
    }

    fn finish_paragraph(&mut self, source_range: std::ops::Range<usize>) {
        if self.cur_runs.is_empty() && self.cur_prefix.is_empty() {
            return;
        }
        let mut prefix = self.quote_prefix();
        prefix.extend(self.cur_prefix.drain(..));
        let mut hanging = self.quote_prefix();
        hanging.extend(self.cur_hanging.drain(..));
        let runs = std::mem::take(&mut self.cur_runs);
        self.push_block(
            Block::Paragraph {
                runs,
                prefix,
                hanging,
            },
            source_range,
        );
    }

    fn push_text(&mut self, text: &str) {
        if self.in_code_block {
            self.code_content.push_str(text);
            return;
        }
        if self.in_heading.is_some() {
            self.heading_buf.push_str(text);
        }
        let style = self.cur_style();
        let inline_range = self.cur_inline_range();
        // Inside an explicit markdown/angle-bracket link the whole run is
        // already a click target — emit verbatim, don't autolink within it.
        if self.open_link.is_some() {
            self.cur_runs.push(Run {
                text: text.to_string(),
                style,
                link: self.open_link,
                checkbox: None,
                image: None,
                inline_range,
                cursor_at: None,
            });
            return;
        }
        // Bare URLs (`https://…`, `www.…`) in plain text aren't links to
        // pulldown-cmark, so split them into their own clickable runs.
        for seg in autolink_segments(text) {
            match seg {
                Segment::Text(t) => self.cur_runs.push(Run {
                    text: t.to_string(),
                    style,
                    link: None,
                    checkbox: None,
                    image: None,
                    inline_range: inline_range.clone(),
                    cursor_at: None,
                }),
                Segment::Url { display, href } => {
                    let idx = self.links.len();
                    self.links.push(PendingLink {
                        target: LinkTarget::Url(href),
                    });
                    let link_style = style
                        .fg(self.theme.link)
                        .add_modifier(self.theme.link_modifier);
                    self.cur_runs.push(Run {
                        text: display.to_string(),
                        style: link_style,
                        link: Some(idx),
                        checkbox: None,
                        image: None,
                        inline_range: inline_range.clone(),
                        cursor_at: None,
                    });
                }
            }
        }
    }

    fn event(&mut self, ev: Event<'_>, range: std::ops::Range<usize>) {
        match ev {
            Event::Start(tag) => self.start_tag(tag, range.clone()),
            Event::End(tag) => self.end_tag(tag, range.clone()),
            Event::Text(s) => self.push_text(&s),
            Event::Code(s) => {
                let style = self
                    .cur_style()
                    .fg(self.theme.code_fg)
                    .bg_opt(self.theme.code_bg);
                // Inline `code` is its own element — record the full source
                // range (including backticks) so edit mode can substitute
                // back to `\`code\`` raw.
                self.cur_runs.push(Run {
                    text: format!(" {} ", s),
                    style,
                    link: self.open_link,
                    checkbox: None,
                    image: None,
                    inline_range: Some(range.clone()),
                    cursor_at: None,
                });
                if self.in_heading.is_some() {
                    self.heading_buf.push_str(&s);
                }
            }
            Event::Html(_) | Event::InlineHtml(_) => {}
            Event::FootnoteReference(name) => {
                let style = self.cur_style().fg(self.theme.muted);
                self.cur_runs.push(Run {
                    text: format!("[^{}]", name),
                    style,
                    link: self.open_link,
                    checkbox: None,
                    image: None,
                    inline_range: None,
                    cursor_at: None,
                });
            }
            Event::SoftBreak => {
                self.cur_runs.push(Run {
                    text: " ".to_string(),
                    style: self.cur_style(),
                    link: self.open_link,
                    checkbox: None,
                    image: None,
                    inline_range: None,
                    cursor_at: None,
                });
            }
            Event::HardBreak => {
                self.cur_runs.push(Run {
                    text: "\n".to_string(),
                    style: self.cur_style(),
                    link: self.open_link,
                    checkbox: None,
                    image: None,
                    inline_range: None,
                    cursor_at: None,
                });
            }
            Event::Rule => self.push_block(Block::Rule, range.clone()),
            Event::TaskListMarker(checked) => {
                let glyph = if checked { "[x]" } else { "[ ]" };
                let style = Style::default()
                    .fg(self.theme.list_marker)
                    .add_modifier(Modifier::BOLD);
                let cb_idx = self.checkboxes.len();
                self.checkboxes.push(PendingCheckbox {
                    source_offset: range.start,
                    checked,
                });
                self.cur_runs.push(Run {
                    text: glyph.to_string(),
                    style,
                    link: None,
                    checkbox: Some(cb_idx),
                    image: None,
                    inline_range: None,
                    cursor_at: None,
                });
                self.cur_runs.push(Run {
                    text: " ".to_string(),
                    style: Style::default(),
                    link: None,
                    checkbox: None,
                    image: None,
                    inline_range: None,
                    cursor_at: None,
                });
            }
            _ => {}
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>, range: std::ops::Range<usize>) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                self.in_heading = Some(level);
                self.heading_buf.clear();
                let lvl = heading_idx(level);
                let style = Style::default()
                    .fg(self.theme.heading[lvl])
                    .add_modifier(self.theme.heading_modifier);
                self.style_stack.push(style);
            }
            Tag::BlockQuote(_) => {
                self.quote_depth += 1;
            }
            Tag::CodeBlock(kind) => {
                self.in_code_block = true;
                self.code_content.clear();
                self.code_lang = match kind {
                    CodeBlockKind::Fenced(s) => {
                        let s = s.into_string();
                        if s.is_empty() { None } else { Some(s) }
                    }
                    CodeBlockKind::Indented => None,
                };
            }
            Tag::List(start) => {
                self.list_stack.push(ListFrame { ordered: start });
            }
            Tag::Item => {
                let (prefix, hanging) = self.list_prefixes();
                self.cur_prefix = prefix;
                self.cur_hanging = hanging;
            }
            Tag::Emphasis => {
                let m = self.theme.emphasis;
                self.push_style(|s| s.add_modifier(m));
                self.inline_range_stack.push(range.clone());
            }
            Tag::Strong => {
                let m = self.theme.strong;
                self.push_style(|s| s.add_modifier(m));
                self.inline_range_stack.push(range.clone());
            }
            Tag::Strikethrough => {
                let m = self.theme.strikethrough;
                self.push_style(|s| s.add_modifier(m));
                self.inline_range_stack.push(range.clone());
            }
            Tag::Link { dest_url, .. } => {
                let target = links::resolve(&dest_url, self.base_dir.as_deref());
                let idx = self.links.len();
                self.links.push(PendingLink { target });
                self.open_link = Some(idx);
                let style = Style::default()
                    .fg(self.theme.link)
                    .add_modifier(self.theme.link_modifier);
                self.style_stack.push(style);
                self.inline_range_stack.push(range.clone());
            }
            Tag::Image {
                dest_url, title, ..
            } => {
                let style = Style::default().fg(self.theme.muted);
                let label = if title.is_empty() {
                    format!("[image: {}]", dest_url)
                } else {
                    format!("[image: {} ({})]", title, dest_url)
                };
                // Resolve image path against the markdown file's base dir.
                let resolved: PathBuf = match self.base_dir.as_ref() {
                    Some(b) => b.join(dest_url.as_ref()),
                    None => PathBuf::from(dest_url.as_ref()),
                };
                let img_idx = self.images.len();
                self.images.push(resolved);
                self.cur_runs.push(Run {
                    text: label,
                    style,
                    link: None,
                    checkbox: None,
                    image: Some(img_idx),
                    inline_range: None,
                    cursor_at: None,
                });
            }
            Tag::Table(aligns) => {
                self.table = Some(TableState {
                    alignments: aligns,
                    header: Vec::new(),
                    rows: Vec::new(),
                    current_row: Vec::new(),
                    cell_start: 0,
                });
            }
            Tag::TableHead | Tag::TableRow => {}
            Tag::TableCell => {
                if let Some(t) = &mut self.table {
                    t.cell_start = self.cur_runs.len();
                }
            }
            Tag::FootnoteDefinition(name) => {
                let style = Style::default()
                    .fg(self.theme.muted)
                    .add_modifier(Modifier::BOLD);
                self.cur_runs.push(Run {
                    text: format!("[^{}]: ", name),
                    style,
                    link: None,
                    checkbox: None,
                    image: None,
                    inline_range: None,
                    cursor_at: None,
                });
            }
            _ => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd, range: std::ops::Range<usize>) {
        match tag {
            TagEnd::Paragraph => {
                self.finish_paragraph(range);
                self.push_blank();
            }
            TagEnd::Heading(level) => {
                self.in_heading = None;
                let anchor = links::slugify(&self.heading_buf);
                let text = self.heading_buf.trim().to_string();
                self.style_stack.pop();
                let runs = std::mem::take(&mut self.cur_runs);
                self.push_blank();
                self.push_block(
                    Block::Heading {
                        runs,
                        anchor,
                        level: heading_idx(level) as u8 + 1,
                        text,
                    },
                    range,
                );
                self.push_blank();
                self.cur_prefix.clear();
                self.cur_hanging.clear();
            }
            TagEnd::BlockQuote(_) => {
                if self.quote_depth > 0 {
                    self.quote_depth -= 1;
                }
                if self.quote_depth == 0 {
                    self.push_blank();
                }
            }
            TagEnd::CodeBlock => {
                self.in_code_block = false;
                let mut highlighted: Vec<Vec<Span<'static>>> = Vec::new();
                let lang = self.code_lang.take();
                syntax::highlight(
                    &self.code_content,
                    lang.as_deref(),
                    self.theme.syntect_theme,
                    &mut highlighted,
                );
                let lines: Vec<Vec<Run>> = highlighted
                    .into_iter()
                    .map(|spans| {
                        spans
                            .into_iter()
                            .map(|sp| Run {
                                text: sp.content.into_owned(),
                                style: sp.style,
                                link: None,
                                checkbox: None,
                                image: None,
                                inline_range: None,
                                cursor_at: None,
                            })
                            .collect()
                    })
                    .collect();
                let prefix = self.quote_prefix();
                self.code_content.clear();
                self.push_block(
                    Block::Pre {
                        lines,
                        prefix,
                        flat: false,
                        line_sources: Vec::new(),
                    },
                    range,
                );
                self.push_blank();
            }
            TagEnd::List(_) => {
                self.list_stack.pop();
                if self.list_stack.is_empty() {
                    self.push_blank();
                }
            }
            TagEnd::Item => {
                self.finish_paragraph(range);
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.pop_style();
                self.inline_range_stack.pop();
            }
            TagEnd::Link => {
                self.style_stack.pop();
                self.open_link = None;
                self.inline_range_stack.pop();
            }
            TagEnd::Table => {
                if let Some(t) = self.table.take() {
                    self.push_block(
                        Block::Table {
                            alignments: t.alignments,
                            header: t.header,
                            rows: t.rows,
                        },
                        range,
                    );
                    self.push_blank();
                }
            }
            TagEnd::TableHead => {
                if let Some(t) = &mut self.table {
                    t.header = std::mem::take(&mut t.current_row);
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = &mut self.table {
                    t.rows.push(std::mem::take(&mut t.current_row));
                }
            }
            TagEnd::TableCell => {
                if let Some(t) = &mut self.table {
                    let cell: Vec<Run> = self.cur_runs.drain(t.cell_start..).collect();
                    t.current_row.push(cell);
                }
            }
            _ => {}
        }
    }

    fn finish(mut self) -> Rendered {
        // Best-effort flush of any in-flight paragraph runs. The synthetic
        // range covers from the run start (if known) to current source end —
        // edit mode tolerates `0..source.len()` as a fallback since the
        // cursor will only land in committed blocks via the `blocks` list.
        let pseudo = 0..self.source.len();
        self.finish_paragraph(pseudo);

        // Edit-mode live-preview toggle:
        // - For Paragraph blocks where the cursor falls inside an inline
        //   element (Strong/Emphasis/Strike/Link/Image/Code), replace just
        //   that element's runs with a single raw Run carrying cursor_at.
        // - For everything else (heading, code fence, list item, table,
        //   block-quote, AND paragraphs whose cursor sits in plain text
        //   between inline elements), fall back to whole-block raw via
        //   make_raw_block so the cursor is always rendered on a raw line.
        if let Some(ctx) = self.edit {
            let cursor = ctx.cursor;
            for entry in &mut self.blocks {
                if entry.source_range.is_empty() {
                    continue;
                }
                if !(entry.source_range.start <= cursor && cursor <= entry.source_range.end) {
                    continue;
                }
                let did_inline = match &mut entry.block {
                    Block::Paragraph { runs, .. } => {
                        substitute_inline_at_cursor(runs, &self.source, cursor, &self.theme)
                    }
                    _ => false,
                };
                if !did_inline {
                    // Pre-wrap to the configured width so long source
                    // lines (e.g. an unbroken paragraph) display as
                    // multiple wrapped rows with the cursor on the right
                    // wrapped row instead of running off the right edge.
                    entry.block = make_raw_block(
                        &self.source,
                        &entry.source_range,
                        cursor,
                        self.width,
                        &self.theme,
                    );
                }
            }
        }
        layout(
            &self.theme,
            self.width,
            self.source,
            self.blocks,
            self.links,
            self.checkboxes,
            self.images,
            self.edit,
            &self.tables,
        )
    }
}

/// Public re-export of `wrap_to_width` for the raw-pane renderer.
pub fn wrap_to_width_pub(s: &str, max_w: usize) -> Vec<(std::ops::Range<usize>, String)> {
    wrap_to_width(s, max_w)
}

/// Break `s` into chunks each ≤ `max_w` display columns. Splits on whitespace
/// when possible; for a single word longer than `max_w`, splits mid-word at
/// char boundaries. Each chunk records its byte range within `s` so the
/// edit-mode cursor can land on the right wrapped row.
fn wrap_to_width(s: &str, max_w: usize) -> Vec<(std::ops::Range<usize>, String)> {
    use unicode_width::UnicodeWidthChar;
    let mut out = Vec::new();
    if s.is_empty() {
        return out;
    }
    if max_w == 0 {
        out.push((0..s.len(), s.to_string()));
        return out;
    }
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut line_start = 0usize;
    let mut col = 0usize;
    // Byte offset and char index just after the most recent whitespace, so a
    // wrap can both cut at that byte boundary and rewind the scan to it.
    let mut last_break: Option<usize> = None;
    let mut last_break_ci: Option<usize> = None;
    let mut ci = 0usize;
    while ci < chars.len() {
        let (idx, ch) = chars[ci];
        let w = ch.width().unwrap_or(0);
        if col + w > max_w && col > 0 {
            // Wrap. Prefer the most recent whitespace boundary; if there
            // wasn't one inside the current line, hard-break at `idx`.
            let whitespace_break = last_break.filter(|&b| b > line_start);
            let break_at = whitespace_break.unwrap_or(idx);
            let text = s[line_start..break_at]
                .trim_end_matches(|c: char| c.is_whitespace())
                .to_string();
            out.push((line_start..break_at, text));
            line_start = break_at;
            col = 0;
            // When we cut at an earlier whitespace boundary, the scan head
            // `ci` is past it — rewind to the break so the new line's width
            // is recounted from `break_at`. A hard break is at the current
            // char, so leave `ci` to re-evaluate it on the fresh line.
            if let Some(bci) = last_break_ci.filter(|_| whitespace_break.is_some()) {
                ci = bci;
            }
            last_break = None;
            last_break_ci = None;
            continue;
        }
        if ch.is_whitespace() {
            last_break = Some(idx + ch.len_utf8());
            last_break_ci = Some(ci + 1);
        }
        col += w;
        ci += 1;
    }
    if line_start < s.len() {
        out.push((line_start..s.len(), s[line_start..].to_string()));
    }
    out
}

/// Try to find an inline element in `runs` whose range contains `cursor`,
/// and substitute its runs with a single raw Run carrying cursor_at.
/// Returns true if a substitution was made.
fn substitute_inline_at_cursor(
    runs: &mut Vec<Run>,
    source: &str,
    cursor: usize,
    theme: &Theme,
) -> bool {
    // Find the *smallest* (innermost) inline element containing cursor.
    // Each run records its innermost inline_range so the smallest range
    // touching the cursor wins.
    let mut best: Option<std::ops::Range<usize>> = None;
    for r in runs.iter() {
        if let Some(rng) = &r.inline_range {
            if rng.start <= cursor && cursor <= rng.end {
                let take = match &best {
                    None => true,
                    Some(b) => (rng.end - rng.start) < (b.end - b.start),
                };
                if take {
                    best = Some(rng.clone());
                }
            }
        }
    }
    let elem = match best {
        Some(r) => r,
        None => return false,
    };

    // Find the contiguous slice of runs whose inline_range matches `elem`.
    // (Innermost-only tracking means matching ranges are contiguous within
    // a paragraph.)
    let first = runs
        .iter()
        .position(|r| r.inline_range.as_ref() == Some(&elem));
    let first = match first {
        Some(i) => i,
        None => return false,
    };
    let mut last = first;
    while last + 1 < runs.len() && runs[last + 1].inline_range.as_ref() == Some(&elem) {
        last += 1;
    }

    let raw = source.get(elem.clone()).unwrap_or("").to_string();
    let cursor_at = cursor.saturating_sub(elem.start).min(raw.len());
    let style = Style::default().fg(theme.muted).bg_opt(theme.code_bg);
    let synthetic = Run {
        text: raw,
        style,
        link: None,
        checkbox: None,
        image: None,
        inline_range: Some(elem),
        cursor_at: Some(cursor_at),
    };
    runs.splice(first..=last, std::iter::once(synthetic));
    true
}

/// Take a source byte range and produce a `Pre`-like block whose content is
/// the raw source slice. Used by edit mode to replace the formatted display
/// of the block under the cursor with its underlying markdown text. We use
/// the muted text color (no code background) so it visually distinguishes
/// from real code blocks.
fn make_raw_block(
    source: &str,
    range: &std::ops::Range<usize>,
    cursor: usize,
    width: usize,
    theme: &Theme,
) -> Block {
    let slice = source.get(range.clone()).unwrap_or("");
    let style = Style::default().fg(theme.muted);
    let cursor_in_block = cursor.saturating_sub(range.start);
    // Flat raw blocks render with no left pad / chrome (see Block::Pre
    // layout), so wrap to the full configured width — same as the view-mode
    // paragraph wrap, which is what the user expects (no visual reflow).
    let inner_width = width.max(1);

    let mut lines: Vec<Vec<Run>> = Vec::new();
    let mut line_sources: Vec<Option<std::ops::Range<usize>>> = Vec::new();
    let mut byte_idx = 0usize;
    for line in slice.split('\n') {
        let line_len = line.len();
        let stripped = line.strip_suffix('\r').unwrap_or(line);
        // Wrap this single source line to `inner_width` display columns.
        // Each wrapped chunk records its byte range within `stripped` so
        // cursor_at can be placed on the right chunk.
        let chunks = wrap_to_width(stripped, inner_width);
        let chunks: Vec<(std::ops::Range<usize>, String)> = if chunks.is_empty() {
            vec![(0..0, String::new())]
        } else {
            chunks
        };
        for (chunk_range, chunk_text) in chunks {
            // chunk byte offsets are relative to `stripped`, which equals
            // the source line minus an optional trailing `\r`. cursor_in_block
            // counts from the start of the source range, so add byte_idx.
            let chunk_block_start = byte_idx + chunk_range.start;
            let chunk_block_end = byte_idx + chunk_range.end;
            let cursor_at =
                if cursor_in_block >= chunk_block_start && cursor_in_block <= chunk_block_end {
                    Some(cursor_in_block - chunk_block_start)
                } else {
                    None
                };
            if chunk_text.is_empty() && cursor_at.is_none() {
                lines.push(Vec::new());
            } else {
                lines.push(vec![Run {
                    text: chunk_text,
                    style,
                    link: None,
                    checkbox: None,
                    image: None,
                    inline_range: None,
                    cursor_at,
                }]);
            }
            // Source byte range covered by this display row, in `source`
            // coordinates. Edit mode uses this to map clicks/cursor moves
            // between display columns and source bytes.
            let src_start = range.start + chunk_block_start;
            let src_end = range.start + chunk_block_end;
            line_sources.push(Some(src_start..src_end));
        }
        byte_idx += line_len + 1; // consumed `\n`
    }
    // Render with no left-pad / quote-bar prefix so the raw text aligns to
    // column 0 — that lets the cursor display position math match the
    // source-line column directly.
    Block::Pre {
        lines,
        prefix: Vec::new(),
        flat: true,
        line_sources,
    }
}

/// One piece of a plain-text run after bare-URL scanning: either inert text
/// or a detected hyperlink (`display` = the text shown, `href` = where it
/// points; they differ only for `www.` matches, which gain an `https://`).
enum Segment<'a> {
    Text(&'a str),
    Url { display: &'a str, href: String },
}

/// Split `text` into inert spans and bare URL spans. Recognises `http://`,
/// `https://`, and `www.` starts at a non-alphanumeric boundary, mirroring the
/// common subset of GFM autolinking. Trailing sentence punctuation and
/// unbalanced closing parens are excluded from the link.
fn autolink_segments(text: &str) -> Vec<Segment<'_>> {
    let mut segs = Vec::new();
    let mut plain_start = 0;
    let mut i = 0;
    while i < text.len() {
        // A URL may only start at the document start or after a non-
        // alphanumeric char, so `foohttp://x` isn't matched mid-word.
        let boundary_ok = i == 0
            || !text[..i]
                .chars()
                .next_back()
                .map(|c| c.is_alphanumeric())
                .unwrap_or(false);
        if boundary_ok {
            if let Some((display, href)) = match_url_at(text, i) {
                if plain_start < i {
                    segs.push(Segment::Text(&text[plain_start..i]));
                }
                segs.push(Segment::Url { display, href });
                i += display.len();
                plain_start = i;
                continue;
            }
        }
        i += text[i..].chars().next().map(char::len_utf8).unwrap_or(1);
    }
    if plain_start < text.len() {
        segs.push(Segment::Text(&text[plain_start..]));
    }
    segs
}

/// If a bare URL begins exactly at byte `i`, return its `(display, href)`.
fn match_url_at(text: &str, i: usize) -> Option<(&str, String)> {
    let rest = &text[i..];
    let (prefix_len, is_www) = if let Some(r) = rest.strip_prefix("https://") {
        (rest.len() - r.len(), false)
    } else if let Some(r) = rest.strip_prefix("http://") {
        (rest.len() - r.len(), false)
    } else if rest.starts_with("www.") {
        (0, true)
    } else {
        return None;
    };
    // Consume URL characters: everything up to whitespace or a delimiter that
    // can't appear in a URL.
    let mut end = i + prefix_len;
    for (off, ch) in rest[prefix_len..].char_indices() {
        if ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '`' | '|' | '\\') {
            break;
        }
        end = i + prefix_len + off + ch.len_utf8();
    }
    let url = trim_url_end(&text[i..end]);
    // Reject a scheme/host with no real content (e.g. a lone `https://`).
    if is_www {
        if url.len() <= "www.".len() {
            return None;
        }
        Some((url, format!("https://{url}")))
    } else {
        if url.ends_with("://") {
            return None;
        }
        Some((url, url.to_string()))
    }
}

/// Strip trailing characters that read as sentence punctuation rather than
/// part of the URL: `.`, `,`, `!`, `?`, etc., plus a closing paren/bracket
/// that isn't balanced by an opener inside the URL.
fn trim_url_end(mut url: &str) -> &str {
    loop {
        let last = match url.chars().next_back() {
            Some(c) => c,
            None => break,
        };
        if matches!(
            last,
            '.' | ',' | ';' | ':' | '!' | '?' | '*' | '_' | '~' | '\'' | '"'
        ) {
            url = &url[..url.len() - last.len_utf8()];
            continue;
        }
        if matches!(last, ')' | ']' | '}') {
            let (open, close) = match last {
                ')' => ('(', ')'),
                ']' => ('[', ']'),
                _ => ('{', '}'),
            };
            if url.matches(close).count() > url.matches(open).count() {
                url = &url[..url.len() - last.len_utf8()];
                continue;
            }
        }
        break;
    }
    url
}

fn heading_idx(l: HeadingLevel) -> usize {
    match l {
        HeadingLevel::H1 => 0,
        HeadingLevel::H2 => 1,
        HeadingLevel::H3 => 2,
        HeadingLevel::H4 => 3,
        HeadingLevel::H5 => 4,
        HeadingLevel::H6 => 5,
    }
}

// ---------------------------------------------------------------------------
// Layout pass: blocks → wrapped lines + LinkMap
// ---------------------------------------------------------------------------

fn layout(
    theme: &Theme,
    width: usize,
    _source: String,
    blocks: Vec<BlockEntry>,
    links: Vec<PendingLink>,
    checkboxes: Vec<PendingCheckbox>,
    images: Vec<PathBuf>,
    _edit: Option<EditCtx>,
    tables: &TableExpansions,
) -> Rendered {
    let mut out_lines: Vec<Line<'static>> = Vec::new();
    let mut row_source: Vec<Option<std::ops::Range<usize>>> = Vec::new();
    let mut out_links: Vec<LinkSpan> = Vec::new();
    let mut out_checkboxes: Vec<CheckboxSpan> = Vec::new();
    let mut out_tables: Vec<TableRegion> = Vec::new();
    let mut image_lines: Vec<Option<usize>> = (0..images.len()).map(|_| None).collect();
    let mut anchors = std::collections::HashMap::new();
    let mut block_infos: Vec<BlockInfo> = Vec::new();
    let mut headings: Vec<HeadingInfo> = Vec::new();
    let mut cursor_xy: Option<(u16, u16)> = None;

    // For each link index, track the open span being built across runs.
    let mut open_spans: Vec<Option<OpenSpan>> = (0..links.len()).map(|_| None).collect();

    for entry in blocks {
        let block_start_line = out_lines.len();
        let block_source_range = entry.source_range.clone();
        let block = entry.block;
        match block {
            Block::Blank => {
                if out_lines
                    .last()
                    .map(|l| l.spans.is_empty())
                    .unwrap_or(false)
                {
                    continue;
                }
                out_lines.push(Line::from(""));
            }
            Block::Rule => {
                let bar = "─".repeat(width.max(1));
                out_lines.push(Line::from(Span::styled(
                    bar,
                    Style::default().fg(theme.rule),
                )));
            }
            Block::Heading {
                runs,
                anchor,
                level,
                text,
            } => {
                let start_line = out_lines.len();
                anchors.insert(anchor, start_line);
                headings.push(HeadingInfo {
                    level,
                    text,
                    line: start_line,
                });
                let prefix = Vec::new();
                wrap_runs(
                    &runs,
                    &prefix,
                    &prefix,
                    width,
                    &mut out_lines,
                    &mut out_links,
                    &links,
                    &mut open_spans,
                    &mut out_checkboxes,
                    &checkboxes,
                    &mut image_lines,
                    &mut cursor_xy,
                );
            }
            Block::Paragraph {
                runs,
                prefix,
                hanging,
            } => {
                wrap_runs(
                    &runs,
                    &prefix,
                    &hanging,
                    width,
                    &mut out_lines,
                    &mut out_links,
                    &links,
                    &mut open_spans,
                    &mut out_checkboxes,
                    &checkboxes,
                    &mut image_lines,
                    &mut cursor_xy,
                );
            }
            Block::Table {
                alignments,
                header,
                rows,
            } => {
                let id = block_source_range.start as u64;
                layout_table(
                    theme,
                    &alignments,
                    &header,
                    &rows,
                    width,
                    &mut out_lines,
                    &mut out_links,
                    &links,
                    id,
                    tables.get(&id),
                    &mut out_tables,
                );
            }
            Block::Pre {
                lines,
                prefix,
                flat,
                line_sources,
            } => {
                // Flat = edit-mode raw substitution: skip the 2-col left pad
                // and code background so the rendered output lines up
                // visually with view-mode formatting of the same source.
                let pad_left = if flat { "" } else { "  " };
                let prefix_width =
                    prefix.iter().map(|r| r.text.width()).sum::<usize>() + pad_left.width();
                let bg = if flat { None } else { theme.code_bg };
                for (i, line_runs) in lines.into_iter().enumerate() {
                    let line_y = out_lines.len() as u16;
                    let mut spans: Vec<Span<'static>> = Vec::new();
                    for r in &prefix {
                        spans.push(Span::styled(r.text.clone(), r.style));
                    }
                    if !pad_left.is_empty() {
                        spans.push(Span::styled(
                            pad_left.to_string(),
                            Style::default().bg_opt(bg),
                        ));
                    }
                    let mut content_w = 0usize;
                    let mut col_offset = prefix_width as u16;
                    for r in line_runs {
                        // Cursor lookup: if this run carries cursor_at, the
                        // display column is prefix + content-so-far + width
                        // of the run prefix up to cursor_at bytes.
                        if let Some(c) = r.cursor_at {
                            let pre = r.text.get(..c.min(r.text.len())).unwrap_or("");
                            let cursor_col = col_offset + pre.width() as u16;
                            cursor_xy = Some((cursor_col, line_y));
                        }
                        let s = r.style.bg_opt(bg);
                        let w = r.text.width();
                        content_w += w;
                        col_offset += w as u16;
                        spans.push(Span::styled(r.text, s));
                    }
                    let pad_right = width.saturating_sub(prefix_width + content_w);
                    if pad_right > 0 {
                        spans.push(Span::styled(
                            " ".repeat(pad_right),
                            Style::default().bg_opt(bg),
                        ));
                    }
                    out_lines.push(Line::from(spans));
                    // Edit-mode raw rows expose source-byte mapping so clicks
                    // and Up/Down keys respect soft-wrap; everything else is
                    // formatted and stays None.
                    let src = if flat {
                        line_sources.get(i).cloned().unwrap_or(None)
                    } else {
                        None
                    };
                    row_source.push(src);
                }
            }
        }
        // Pad row_source so it stays parallel to out_lines for blocks that
        // don't fill it themselves.
        while row_source.len() < out_lines.len() {
            row_source.push(None);
        }
        let block_end_line = out_lines.len();
        if !block_source_range.is_empty() {
            block_infos.push(BlockInfo {
                source_range: block_source_range,
                display_start: block_start_line,
                display_end: block_end_line,
            });
        }
    }

    // The cursor display position is recorded directly by the layout pass
    // when emitting a Run with `cursor_at` set. Edit mode produces such a
    // Run (either via inline-element substitution or block-level raw fall-
    // back); view mode never does.
    let link_map = LinkMap {
        links: out_links,
        anchors,
    };
    let checkbox_map = CheckboxMap {
        items: out_checkboxes,
    };
    let images_out: Vec<ImageRef> = images
        .into_iter()
        .zip(image_lines.iter())
        .filter_map(|(source, line)| line.map(|l| ImageRef { line: l, source }))
        .collect();
    Rendered {
        lines: out_lines,
        link_map,
        checkbox_map,
        table_map: TableMap {
            regions: out_tables,
        },
        images: images_out,
        width: width as u16,
        blocks: block_infos,
        headings,
        cursor_xy,
        row_source,
    }
}

#[derive(Clone, Debug)]
struct OpenSpan {
    line: usize,
    col_start: usize,
    col_cur: usize,
}

/// Wrap `runs` to `width`, prepending `prefix` to the first physical line and
/// `hanging` to subsequent wrapped continuations. Pushes each physical line
/// to `out_lines`. Splits link spans across wrap boundaries by emitting one
/// `LinkSpan` per physical line that the link covers.
fn wrap_runs(
    runs: &[Run],
    prefix: &[Run],
    hanging: &[Run],
    width: usize,
    out_lines: &mut Vec<Line<'static>>,
    out_links: &mut Vec<LinkSpan>,
    links: &[PendingLink],
    open_spans: &mut [Option<OpenSpan>],
    out_checkboxes: &mut Vec<CheckboxSpan>,
    checkboxes: &[PendingCheckbox],
    image_lines: &mut [Option<usize>],
    cursor_xy: &mut Option<(u16, u16)>,
) {
    let prefix_width: usize = prefix.iter().map(|r| r.text.width()).sum();
    let hanging_width: usize = hanging.iter().map(|r| r.text.width()).sum();
    let inner_width = width.saturating_sub(prefix_width).max(1);
    let cont_inner_width = width.saturating_sub(hanging_width).max(1);

    let mut cur_spans: Vec<Span<'static>> = prefix
        .iter()
        .map(|r| Span::styled(r.text.clone(), r.style))
        .collect();
    let mut cur_col = prefix_width;
    let mut cur_inner = 0usize;
    let mut at_line_start = true;
    let mut active_inner_width = inner_width;

    let mut current_link: Option<usize> = None;

    let break_line = |out_lines: &mut Vec<Line<'static>>,
                      cur_spans: &mut Vec<Span<'static>>,
                      cur_col: &mut usize,
                      cur_inner: &mut usize,
                      active_inner_width: &mut usize,
                      at_line_start: &mut bool,
                      out_links: &mut Vec<LinkSpan>,
                      open_spans: &mut [Option<OpenSpan>],
                      current_link: &mut Option<usize>,
                      links: &[PendingLink],
                      hanging: &[Run],
                      cont_inner_width: usize| {
        // Close any open link span on this line.
        if let Some(li) = *current_link {
            if let Some(open) = open_spans[li].take() {
                let line = out_lines.len();
                if open.line == line && open.col_cur > open.col_start {
                    out_links.push(LinkSpan {
                        line: open.line,
                        col_start: open.col_start,
                        col_end: open.col_cur,
                        target: links[li].target.clone(),
                    });
                }
            }
        }
        out_lines.push(Line::from(std::mem::take(cur_spans)));
        // Start a new line with hanging prefix.
        for r in hanging {
            cur_spans.push(Span::styled(r.text.clone(), r.style));
        }
        *cur_col = hanging.iter().map(|r| r.text.width()).sum();
        *cur_inner = 0;
        *active_inner_width = cont_inner_width;
        *at_line_start = true;
        // Reopen link span at new line if a link is still in progress.
        if let Some(li) = *current_link {
            open_spans[li] = Some(OpenSpan {
                line: out_lines.len(),
                col_start: *cur_col,
                col_cur: *cur_col,
            });
        }
    };

    for run in runs {
        // Snapshot (line, col) at the start of this run so we can locate
        // cursor_at after emit. We assume the substituted inline element
        // (the one with cursor_at) doesn't internally wrap — true for
        // `**bold**`, `[text](url)`, `` `code` ``, and other no-space spans.
        // If it does wrap, the cursor lands at the run-start position
        // (still inside the substituted text, just possibly at a row above
        // its true location). Acceptable for an MVP.
        let cursor_run_start = run
            .cursor_at
            .map(|c| (out_lines.len() as u16, cur_col as u16, c));

        // Capture checkbox start position before emit; close after.
        let cb_start = run.checkbox.map(|ci| (ci, out_lines.len(), cur_col));
        // Pin the image's output line at the moment its placeholder is emitted.
        if let Some(ii) = run.image {
            if let Some(slot) = image_lines.get_mut(ii) {
                if slot.is_none() {
                    *slot = Some(out_lines.len());
                }
            }
        }

        // Switch link tracking if needed.
        if run.link != current_link {
            // Close old.
            if let Some(li) = current_link {
                if let Some(open) = open_spans[li].take() {
                    let line = out_lines.len();
                    if open.line == line && open.col_cur > open.col_start {
                        out_links.push(LinkSpan {
                            line: open.line,
                            col_start: open.col_start,
                            col_end: open.col_cur,
                            target: links[li].target.clone(),
                        });
                    }
                }
            }
            current_link = run.link;
            if let Some(li) = current_link {
                open_spans[li] = Some(OpenSpan {
                    line: out_lines.len(),
                    col_start: cur_col,
                    col_cur: cur_col,
                });
            }
        }

        // Handle hard breaks embedded in text.
        let mut text = run.text.as_str();
        while !text.is_empty() {
            // Hard break.
            if let Some(idx) = text.find('\n') {
                let head = &text[..idx];
                emit_segment(
                    head,
                    run,
                    &mut cur_spans,
                    &mut cur_col,
                    &mut cur_inner,
                    &mut at_line_start,
                    active_inner_width,
                    out_lines,
                    hanging,
                    cont_inner_width,
                    out_links,
                    open_spans,
                    &mut current_link,
                    links,
                    &mut active_inner_width,
                );
                // Force break.
                break_line(
                    out_lines,
                    &mut cur_spans,
                    &mut cur_col,
                    &mut cur_inner,
                    &mut active_inner_width,
                    &mut at_line_start,
                    out_links,
                    open_spans,
                    &mut current_link,
                    links,
                    hanging,
                    cont_inner_width,
                );
                text = &text[idx + 1..];
            } else {
                emit_segment(
                    text,
                    run,
                    &mut cur_spans,
                    &mut cur_col,
                    &mut cur_inner,
                    &mut at_line_start,
                    active_inner_width,
                    out_lines,
                    hanging,
                    cont_inner_width,
                    out_links,
                    open_spans,
                    &mut current_link,
                    links,
                    &mut active_inner_width,
                );
                text = "";
            }
        }
        // Record the checkbox span now that the run has been emitted. If a
        // line break occurred during emission, the span lives on the new line.
        if let Some((ci, start_line, start_col)) = cb_start {
            let (line, col_start) = if out_lines.len() == start_line {
                (start_line, start_col)
            } else {
                let w = run.text.width();
                (out_lines.len(), cur_col.saturating_sub(w))
            };
            if cur_col > col_start {
                out_checkboxes.push(CheckboxSpan {
                    line,
                    col_start,
                    col_end: cur_col,
                    source_offset: checkboxes[ci].source_offset,
                    checked: checkboxes[ci].checked,
                });
            }
        }

        // If this run carried cursor_at, compute its display position now.
        if let Some((start_line, start_col, c)) = cursor_run_start {
            let pre = run.text.get(..c.min(run.text.len())).unwrap_or("");
            let cursor_col = start_col + pre.width() as u16;
            *cursor_xy = Some((cursor_col, start_line));
        }
    }

    // Close any open link.
    if let Some(li) = current_link {
        if let Some(open) = open_spans[li].take() {
            let line = out_lines.len();
            if open.line == line && open.col_cur > open.col_start {
                out_links.push(LinkSpan {
                    line: open.line,
                    col_start: open.col_start,
                    col_end: open.col_cur,
                    target: links[li].target.clone(),
                });
            }
        }
    }

    out_lines.push(Line::from(cur_spans));
}

fn emit_segment(
    text: &str,
    run: &Run,
    cur_spans: &mut Vec<Span<'static>>,
    cur_col: &mut usize,
    cur_inner: &mut usize,
    at_line_start: &mut bool,
    _initial_inner_width: usize,
    out_lines: &mut Vec<Line<'static>>,
    hanging: &[Run],
    cont_inner_width: usize,
    out_links: &mut Vec<LinkSpan>,
    open_spans: &mut [Option<OpenSpan>],
    current_link: &mut Option<usize>,
    links: &[PendingLink],
    active_inner_width: &mut usize,
) {
    // Word-wrap the text on whitespace boundaries.
    let mut remaining = text;
    while !remaining.is_empty() {
        // Take a chunk: either whitespace run or non-whitespace word.
        let is_space = remaining.chars().next().unwrap().is_whitespace();
        let end = remaining
            .char_indices()
            .find(|(_, c)| c.is_whitespace() != is_space)
            .map(|(i, _)| i)
            .unwrap_or(remaining.len());
        let (word, rest) = remaining.split_at(end);
        let w = word.width();

        if is_space {
            if *at_line_start {
                // Skip leading whitespace on a wrapped line.
                remaining = rest;
                continue;
            }
            if *cur_inner + w > *active_inner_width {
                // Drop trailing whitespace and break.
                break_line_inline(
                    out_lines,
                    cur_spans,
                    cur_col,
                    cur_inner,
                    at_line_start,
                    active_inner_width,
                    out_links,
                    open_spans,
                    current_link,
                    links,
                    hanging,
                    cont_inner_width,
                );
                remaining = rest;
                continue;
            }
            push_chunk(
                cur_spans,
                cur_col,
                cur_inner,
                run,
                word,
                current_link,
                open_spans,
            );
            *at_line_start = false;
            remaining = rest;
            continue;
        }

        // Non-whitespace word.
        if w > *active_inner_width {
            // Word longer than line: hard-split by chars.
            let mut chars_left = word;
            while !chars_left.is_empty() {
                let avail = active_inner_width.saturating_sub(*cur_inner);
                if avail == 0 && !*at_line_start {
                    break_line_inline(
                        out_lines,
                        cur_spans,
                        cur_col,
                        cur_inner,
                        at_line_start,
                        active_inner_width,
                        out_links,
                        open_spans,
                        current_link,
                        links,
                        hanging,
                        cont_inner_width,
                    );
                    continue;
                }
                let mut taken = 0usize;
                let mut taken_w = 0usize;
                for (i, c) in chars_left.char_indices() {
                    let cw = c.to_string().width();
                    if taken_w + cw > avail.max(1) {
                        break;
                    }
                    taken = i + c.len_utf8();
                    taken_w += cw;
                }
                if taken == 0 {
                    taken = chars_left.chars().next().unwrap().len_utf8();
                }
                let (head, tail) = chars_left.split_at(taken);
                push_chunk(
                    cur_spans,
                    cur_col,
                    cur_inner,
                    run,
                    head,
                    current_link,
                    open_spans,
                );
                *at_line_start = false;
                if !tail.is_empty() {
                    break_line_inline(
                        out_lines,
                        cur_spans,
                        cur_col,
                        cur_inner,
                        at_line_start,
                        active_inner_width,
                        out_links,
                        open_spans,
                        current_link,
                        links,
                        hanging,
                        cont_inner_width,
                    );
                }
                chars_left = tail;
            }
            remaining = rest;
            continue;
        }

        if *cur_inner + w > *active_inner_width && !*at_line_start {
            break_line_inline(
                out_lines,
                cur_spans,
                cur_col,
                cur_inner,
                at_line_start,
                active_inner_width,
                out_links,
                open_spans,
                current_link,
                links,
                hanging,
                cont_inner_width,
            );
        }
        push_chunk(
            cur_spans,
            cur_col,
            cur_inner,
            run,
            word,
            current_link,
            open_spans,
        );
        *at_line_start = false;
        remaining = rest;
    }
}

fn push_chunk(
    cur_spans: &mut Vec<Span<'static>>,
    cur_col: &mut usize,
    cur_inner: &mut usize,
    run: &Run,
    chunk: &str,
    current_link: &Option<usize>,
    open_spans: &mut [Option<OpenSpan>],
) {
    if chunk.is_empty() {
        return;
    }
    let w = chunk.width();
    cur_spans.push(Span::styled(chunk.to_string(), run.style));
    *cur_col += w;
    *cur_inner += w;
    if let Some(li) = current_link {
        if let Some(open) = &mut open_spans[*li] {
            open.col_cur = *cur_col;
        }
    }
}

fn break_line_inline(
    out_lines: &mut Vec<Line<'static>>,
    cur_spans: &mut Vec<Span<'static>>,
    cur_col: &mut usize,
    cur_inner: &mut usize,
    at_line_start: &mut bool,
    active_inner_width: &mut usize,
    out_links: &mut Vec<LinkSpan>,
    open_spans: &mut [Option<OpenSpan>],
    current_link: &mut Option<usize>,
    links: &[PendingLink],
    hanging: &[Run],
    cont_inner_width: usize,
) {
    if let Some(li) = *current_link {
        if let Some(open) = open_spans[li].take() {
            let line = out_lines.len();
            if open.line == line && open.col_cur > open.col_start {
                out_links.push(LinkSpan {
                    line: open.line,
                    col_start: open.col_start,
                    col_end: open.col_cur,
                    target: links[li].target.clone(),
                });
            }
        }
    }
    out_lines.push(Line::from(std::mem::take(cur_spans)));
    for r in hanging {
        cur_spans.push(Span::styled(r.text.clone(), r.style));
    }
    *cur_col = hanging.iter().map(|r| r.text.width()).sum();
    *cur_inner = 0;
    *active_inner_width = cont_inner_width;
    *at_line_start = true;
    if let Some(li) = *current_link {
        open_spans[li] = Some(OpenSpan {
            line: out_lines.len(),
            col_start: *cur_col,
            col_cur: *cur_col,
        });
    }
}

// ---------------------------------------------------------------------------
// Style helper: optional bg.
// ---------------------------------------------------------------------------

trait StyleExt {
    fn bg_opt(self, c: Option<Color>) -> Style;
}
impl StyleExt for Style {
    fn bg_opt(self, c: Option<Color>) -> Style {
        match c {
            Some(col) => self.bg(col),
            None => self,
        }
    }
}

// ---------------------------------------------------------------------------
// Table layout: column-aligned with box-drawing borders.
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn layout_table(
    theme: &Theme,
    alignments: &[Alignment],
    header: &[Vec<Run>],
    rows: &[Vec<Vec<Run>>],
    max_width: usize,
    out_lines: &mut Vec<Line<'static>>,
    out_links: &mut Vec<LinkSpan>,
    links: &[PendingLink],
    table_id: u64,
    expand: Option<&TableExpand>,
    out_tables: &mut Vec<TableRegion>,
) {
    let n_cols = header
        .len()
        .max(rows.iter().map(|r| r.len()).max().unwrap_or(0));
    if n_cols == 0 {
        return;
    }

    let cell_w = |runs: &[Run]| -> usize { runs.iter().map(|r| r.text.width()).sum() };

    // Natural (untruncated) width of every column.
    let mut natural = vec![0usize; n_cols];
    for (i, c) in header.iter().enumerate() {
        if i < n_cols {
            natural[i] = natural[i].max(cell_w(c));
        }
    }
    for row in rows {
        for (i, c) in row.iter().enumerate() {
            if i < n_cols {
                natural[i] = natural[i].max(cell_w(c));
            }
        }
    }

    let all = expand.map(|e| e.all).unwrap_or(false);
    let col_expanded = |c: usize| all || expand.map(|e| e.cols.contains(&c)).unwrap_or(false);
    let cell_expanded = |r: usize, c: usize| {
        all || col_expanded(c) || expand.map(|e| e.cells.contains(&(r, c))).unwrap_or(false)
    };

    // Constrain to terminal width: total frame = 1 + sum(w + 3). Expanded
    // columns keep their natural width; the rest absorb the shrink first so a
    // freshly-expanded date column reclaims room from prose columns.
    let frame_overhead = 1 + 3 * n_cols;
    let avail = max_width.saturating_sub(frame_overhead).max(n_cols);
    let mut col_widths = natural.clone();
    let total: usize = col_widths.iter().sum();
    if total > avail {
        let over = total - avail;
        let unprotected: Vec<usize> = (0..n_cols).filter(|&c| !col_expanded(c)).collect();
        let removed = shrink_columns(&mut col_widths, &unprotected, over);
        if removed < over {
            let protected: Vec<usize> = (0..n_cols).filter(|&c| col_expanded(c)).collect();
            shrink_columns(&mut col_widths, &protected, over - removed);
        }
    }
    for w in col_widths.iter_mut() {
        if *w == 0 {
            *w = 1;
        }
    }

    let border = Style::default().fg(theme.muted);

    // Border x-positions: a │ sits at x=0 and after each column's
    // (pad + content + pad). Column click area is the span between borders.
    let mut border_x: Vec<usize> = Vec::with_capacity(n_cols + 1);
    let mut x = 0usize;
    border_x.push(x);
    for w in &col_widths {
        x += w + 3;
        border_x.push(x);
    }
    let col_x: Vec<(usize, usize)> = (0..n_cols)
        .map(|c| (border_x[c] + 1, border_x[c + 1]))
        .collect();

    let line_start = out_lines.len();
    let mut border_lines: Vec<usize> = Vec::new();

    border_lines.push(out_lines.len());
    out_lines.push(border_line(&col_widths, '┌', '┬', '┐', border));

    let header_start = out_lines.len();
    emit_row(
        theme,
        header,
        &col_widths,
        alignments,
        true,
        &|c| col_expanded(c),
        out_lines,
        out_links,
        links,
        border,
    );
    let header_end = out_lines.len();

    border_lines.push(out_lines.len());
    out_lines.push(border_line(&col_widths, '├', '┼', '┤', border));

    let mut body_rows: Vec<(usize, usize)> = Vec::with_capacity(rows.len());
    for (ri, row) in rows.iter().enumerate() {
        let row_start = out_lines.len();
        emit_row(
            theme,
            row,
            &col_widths,
            alignments,
            false,
            &|c| cell_expanded(ri, c),
            out_lines,
            out_links,
            links,
            border,
        );
        body_rows.push((row_start, out_lines.len()));
    }

    border_lines.push(out_lines.len());
    out_lines.push(border_line(&col_widths, '└', '┴', '┘', border));

    out_tables.push(TableRegion {
        id: table_id,
        line_start,
        line_end: out_lines.len(),
        border_lines,
        header_start,
        header_end,
        col_x,
        border_x,
        body_rows,
    });
}

/// Reduce the combined width of `cols` by up to `amount`, taking a column from
/// the widest eligible column each step and never dropping below 1. Returns the
/// amount actually removed (less than `amount` only when every listed column is
/// already at width 1).
fn shrink_columns(widths: &mut [usize], cols: &[usize], amount: usize) -> usize {
    let mut removed = 0usize;
    while removed < amount {
        let mut best: Option<usize> = None;
        for &c in cols {
            if widths[c] > 1 && best.map(|b| widths[c] > widths[b]).unwrap_or(true) {
                best = Some(c);
            }
        }
        match best {
            Some(b) => {
                widths[b] -= 1;
                removed += 1;
            }
            None => break,
        }
    }
    removed
}

fn border_line(
    col_widths: &[usize],
    left: char,
    mid: char,
    right: char,
    style: Style,
) -> Line<'static> {
    let mut s = String::new();
    s.push(left);
    for (i, w) in col_widths.iter().enumerate() {
        for _ in 0..(w + 2) {
            s.push('─');
        }
        s.push(if i + 1 < col_widths.len() { mid } else { right });
    }
    Line::from(Span::styled(s, style))
}

/// Emit one logical table row, which may span several physical lines: an
/// expanded cell word-wraps to its column width while its siblings stay on the
/// first line. `expanded(col)` decides per-column whether to wrap (full
/// content) or truncate to a single line.
#[allow(clippy::too_many_arguments)]
fn emit_row(
    theme: &Theme,
    row: &[Vec<Run>],
    col_widths: &[usize],
    alignments: &[Alignment],
    is_header: bool,
    expanded: &dyn Fn(usize) -> bool,
    out_lines: &mut Vec<Line<'static>>,
    out_links: &mut Vec<LinkSpan>,
    links: &[PendingLink],
    border: Style,
) {
    let empty: Vec<Run> = Vec::new();

    // Per-column physical lines: truncated cells are a single line; expanded
    // cells wrap to many. Row height is the tallest column.
    let mut cell_lines: Vec<Vec<Vec<Run>>> = Vec::with_capacity(col_widths.len());
    let mut height = 1usize;
    for (i, w) in col_widths.iter().enumerate() {
        let cell = row.get(i).unwrap_or(&empty);
        let lines = if expanded(i) {
            wrap_runs_to_lines(cell, *w)
        } else {
            vec![truncate_runs(cell, *w)]
        };
        height = height.max(lines.len());
        cell_lines.push(lines);
    }

    for k in 0..height {
        let line = out_lines.len();
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut col = 0usize;

        spans.push(Span::styled("│".to_string(), border));
        col += 1;

        for (i, w) in col_widths.iter().enumerate() {
            let align = alignments.get(i).copied().unwrap_or(Alignment::None);
            let content: &[Run] = cell_lines[i].get(k).map(|v| v.as_slice()).unwrap_or(&[]);
            let cw: usize = content.iter().map(|r| r.text.width()).sum();
            let extra = w.saturating_sub(cw);
            let (lpad, rpad) = match align {
                Alignment::Right => (extra, 0),
                Alignment::Center => (extra / 2, extra - extra / 2),
                _ => (0, extra),
            };

            // leading inner pad
            spans.push(Span::raw(" ".to_string()));
            col += 1;
            if lpad > 0 {
                spans.push(Span::raw(" ".repeat(lpad)));
                col += lpad;
            }

            // cell content
            emit_runs_tracking_links(
                content, is_header, theme, &mut spans, &mut col, line, out_links, links,
            );

            if rpad > 0 {
                spans.push(Span::raw(" ".repeat(rpad)));
                col += rpad;
            }
            // trailing inner pad
            spans.push(Span::raw(" ".to_string()));
            col += 1;

            spans.push(Span::styled("│".to_string(), border));
            col += 1;
        }

        out_lines.push(Line::from(spans));
    }
}

/// Word-wrap a cell's runs to `width` display columns, one `Vec<Run>` per
/// physical line, preserving each run's style and link. Trailing whitespace is
/// trimmed so it doesn't disturb column alignment. Always returns at least one
/// (possibly empty) line.
fn wrap_runs_to_lines(runs: &[Run], width: usize) -> Vec<Vec<Run>> {
    // Concatenate into a single string plus a parallel run-boundary table so
    // wrap byte ranges can be sliced back into styled sub-runs.
    let mut s = String::new();
    let mut offsets: Vec<usize> = Vec::with_capacity(runs.len() + 1);
    for r in runs {
        offsets.push(s.len());
        s.push_str(&r.text);
    }
    offsets.push(s.len());

    if s.is_empty() {
        return vec![Vec::new()];
    }

    let mut out: Vec<Vec<Run>> = Vec::new();
    for (range, _text) in wrap_to_width(&s, width.max(1)) {
        let mut line = slice_runs(runs, &offsets, range.start, range.end);
        trim_trailing_ws(&mut line);
        out.push(line);
    }
    if out.is_empty() {
        out.push(Vec::new());
    }
    out
}

/// Slice the styled `runs` to the byte window `[start, end)` of their
/// concatenation (`offsets[i]` is run `i`'s start byte). Drops checkbox/image/
/// cursor metadata — wrapped table cells carry only text, style, and links.
fn slice_runs(runs: &[Run], offsets: &[usize], start: usize, end: usize) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    for (i, r) in runs.iter().enumerate() {
        let rs = offsets[i];
        let re = offsets[i + 1];
        let a = start.max(rs);
        let b = end.min(re);
        if a >= b {
            continue;
        }
        let text = r.text.get(a - rs..b - rs).unwrap_or("").to_string();
        if text.is_empty() {
            continue;
        }
        out.push(Run {
            text,
            style: r.style,
            link: r.link,
            checkbox: None,
            image: None,
            inline_range: None,
            cursor_at: None,
        });
    }
    out
}

/// Trim trailing whitespace from the last run(s) of a wrapped line.
fn trim_trailing_ws(line: &mut Vec<Run>) {
    while let Some(last) = line.last_mut() {
        let trimmed = last.text.trim_end_matches(char::is_whitespace).len();
        if trimmed == last.text.len() {
            break;
        }
        last.text.truncate(trimmed);
        if last.text.is_empty() {
            line.pop();
        } else {
            break;
        }
    }
}

fn truncate_runs(runs: &[Run], max: usize) -> Vec<Run> {
    let total: usize = runs.iter().map(|r| r.text.width()).sum();
    if total <= max {
        return runs.to_vec();
    }
    let mut out: Vec<Run> = Vec::new();
    let mut budget = max.saturating_sub(1); // reserve 1 for ellipsis
    for run in runs {
        let w = run.text.width();
        if w <= budget {
            out.push(run.clone());
            budget -= w;
        } else {
            // Take chars up to budget.
            let mut taken_w = 0usize;
            let mut taken_bytes = 0usize;
            for (i, ch) in run.text.char_indices() {
                let cw = ch.to_string().width();
                if taken_w + cw > budget {
                    break;
                }
                taken_w += cw;
                taken_bytes = i + ch.len_utf8();
            }
            if taken_bytes > 0 {
                out.push(Run {
                    text: run.text[..taken_bytes].to_string(),
                    style: run.style,
                    link: run.link,
                    checkbox: run.checkbox,
                    image: run.image,
                    inline_range: run.inline_range.clone(),
                    cursor_at: None,
                });
            }
            break;
        }
    }
    out.push(Run {
        text: "…".to_string(),
        style: Style::default(),
        link: None,
        checkbox: None,
        image: None,
        inline_range: None,
        cursor_at: None,
    });
    out
}

fn emit_runs_tracking_links(
    runs: &[Run],
    is_header: bool,
    theme: &Theme,
    spans: &mut Vec<Span<'static>>,
    col: &mut usize,
    line: usize,
    out_links: &mut Vec<LinkSpan>,
    links: &[PendingLink],
) {
    let mut current_link: Option<usize> = None;
    let mut open_start: usize = *col;
    for run in runs {
        if run.link != current_link {
            if let Some(li) = current_link {
                if open_start < *col {
                    out_links.push(LinkSpan {
                        line,
                        col_start: open_start,
                        col_end: *col,
                        target: links[li].target.clone(),
                    });
                }
            }
            current_link = run.link;
            open_start = *col;
        }
        let mut style = run.style;
        if is_header {
            style = style.add_modifier(Modifier::BOLD).fg(theme.heading[1]);
        }
        let w = run.text.width();
        spans.push(Span::styled(run.text.clone(), style));
        *col += w;
    }
    if let Some(li) = current_link {
        if open_start < *col {
            out_links.push(LinkSpan {
                line,
                col_start: open_start,
                col_end: *col,
                target: links[li].target.clone(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::links::{LinkTarget, TableHit};
    use crate::tui::theme::Theme;

    /// Every wrapped chunk must fit within `max_w` columns. Regression for a
    /// bug where wrapping at a whitespace boundary behind the scan head left
    /// `col` under-counted, so a long token (e.g. a markdown link URL)
    /// followed by more words collapsed onto one overwide, clipped row.
    #[test]
    fn wrap_never_exceeds_width() {
        use unicode_width::UnicodeWidthChar;
        let line = "- Contributed [PR 1433](https://github.com/ReamLabs/ream/pull/1433) to cover a lean spec update in Ream.";
        for w in 5..=80usize {
            for (range, text) in wrap_to_width(line, w) {
                let cols: usize = text.chars().map(|c| c.width().unwrap_or(0)).sum();
                assert!(
                    cols <= w,
                    "row {:?} is {} cols, exceeds width {}",
                    text,
                    cols,
                    w
                );
                // Ranges must stay within bounds and produce the right slice.
                assert!(range.end <= line.len());
            }
        }
    }

    /// Chunk byte ranges must tile the input gap-free and in order, so the
    /// editor's cursor/click mapping over wrapped rows stays correct.
    #[test]
    fn wrap_ranges_are_contiguous() {
        let line = "- Contributed [PR 1433](https://github.com/ReamLabs/ream/pull/1433) to cover a lean spec update in Ream.";
        let chunks = wrap_to_width(line, 62);
        assert_eq!(chunks.first().unwrap().0.start, 0);
        assert_eq!(chunks.last().unwrap().0.end, line.len());
        for pair in chunks.windows(2) {
            assert_eq!(pair[0].0.end, pair[1].0.start, "ranges must be contiguous");
        }
    }

    #[test]
    fn renders_paragraph_and_link() {
        let src = "hello [there](https://example.com) world";
        let r = render(src, None, 80, &Theme::dark());
        assert!(!r.lines.is_empty());
        assert_eq!(r.link_map.links.len(), 1);
        let link = &r.link_map.links[0];
        assert_eq!(
            link.target,
            LinkTarget::Url("https://example.com".to_string())
        );
        assert!(link.col_end > link.col_start);
    }

    #[test]
    fn autolinks_bare_url_in_text() {
        let src = "see https://example.com/foo for details";
        let r = render(src, None, 80, &Theme::dark());
        assert_eq!(r.link_map.links.len(), 1);
        let link = &r.link_map.links[0];
        assert_eq!(
            link.target,
            LinkTarget::Url("https://example.com/foo".to_string())
        );
        assert!(link.col_end > link.col_start);
    }

    #[test]
    fn autolink_trims_trailing_punctuation() {
        // Sentence period and the wrapping parens must stay out of the URL.
        let src = "visit https://example.com. (also https://a.test)";
        let r = render(src, None, 80, &Theme::dark());
        let targets: Vec<_> = r.link_map.links.iter().map(|l| &l.target).collect();
        assert_eq!(
            targets,
            vec![
                &LinkTarget::Url("https://example.com".to_string()),
                &LinkTarget::Url("https://a.test".to_string()),
            ]
        );
    }

    #[test]
    fn autolinks_www_with_https_scheme() {
        let src = "go to www.example.com now";
        let r = render(src, None, 80, &Theme::dark());
        assert_eq!(r.link_map.links.len(), 1);
        assert_eq!(
            r.link_map.links[0].target,
            LinkTarget::Url("https://www.example.com".to_string())
        );
    }

    #[test]
    fn does_not_double_link_markdown_or_code() {
        // Explicit link keeps its dest; URL inside inline code is not linked.
        let src = "[x](https://md.test) and `https://code.test`";
        let r = render(src, None, 80, &Theme::dark());
        assert_eq!(r.link_map.links.len(), 1);
        assert_eq!(
            r.link_map.links[0].target,
            LinkTarget::Url("https://md.test".to_string())
        );
    }

    #[test]
    fn records_heading_anchors() {
        let src = "# Hello World\n\nbody";
        let r = render(src, None, 80, &Theme::dark());
        assert_eq!(r.link_map.anchors.get("hello-world"), Some(&1));
    }

    #[test]
    fn records_document_outline() {
        let src = "# Top\n\nbody\n\n## Sub\n\nmore\n\n### Deep *em*\n";
        let r = render(src, None, 80, &Theme::dark());
        let got: Vec<(u8, &str)> = r
            .headings
            .iter()
            .map(|h| (h.level, h.text.as_str()))
            .collect();
        assert_eq!(got, vec![(1, "Top"), (2, "Sub"), (3, "Deep em")]);
        // Display lines are strictly increasing and agree with the anchors.
        assert!(r.headings.windows(2).all(|w| w[0].line < w[1].line));
        assert_eq!(r.link_map.anchors.get("top"), Some(&r.headings[0].line));
    }

    #[test]
    fn wraps_long_paragraph() {
        let src = "one two three four five six seven eight nine ten";
        let r = render(src, None, 12, &Theme::dark());
        // "one two three" (13) needs wrapping in ≤12-wide column.
        assert!(r.lines.len() > 2);
    }

    #[test]
    fn renders_aligned_table() {
        let src = "\
| A | Bee |\n\
| --- | --- |\n\
| 1 | one |\n\
| 22 | two |\n";
        let r = render(src, None, 80, &Theme::dark());
        // Find the header row; it should be a `│` framed row of fixed total width.
        let mut frame_lines: Vec<String> = Vec::new();
        for line in &r.lines {
            let s: String = line.spans.iter().map(|sp| sp.content.as_ref()).collect();
            if s.starts_with('│') || s.starts_with('┌') || s.starts_with('├') || s.starts_with('└')
            {
                frame_lines.push(s);
            }
        }
        assert!(
            frame_lines.len() >= 6,
            "expected ≥6 framed lines, got {}",
            frame_lines.len()
        );
        // All framed lines must share the same display width — that's the alignment guarantee.
        let widths: Vec<usize> = frame_lines.iter().map(|s| s.as_str().width()).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "table frame widths not equal: {:?}\nlines:\n{}",
            widths,
            frame_lines.join("\n"),
        );
    }

    /// Render `src` with a specific table expansion state applied to the only
    /// table in the document (id = the table block's source byte offset).
    fn render_table(src: &str, width: u16, mutate: impl FnOnce(&mut TableExpand)) -> Rendered {
        // First render to discover the table's id from its hit-test region.
        let probe = render(src, None, width, &Theme::dark());
        let id = probe.table_map.regions[0].id;
        let mut tables = TableExpansions::new();
        let mut st = TableExpand::default();
        mutate(&mut st);
        tables.insert(id, st);
        render_with_edit(src, None, width, &Theme::dark(), None, &tables)
    }

    #[test]
    fn table_truncates_overflowing_cell_by_default() {
        let src = "\
| Name | Note |\n\
| --- | --- |\n\
| a | this is a long note that overflows the column badly |\n";
        let r = render(src, None, 30, &Theme::dark());
        let body: String = r.lines.iter().map(|l| line_text_of(l)).collect();
        assert!(body.contains('…'), "expected an ellipsis from truncation");
        // One body row → exactly one physical body line.
        assert_eq!(r.table_map.regions[0].body_rows.len(), 1);
        let (s, e) = r.table_map.regions[0].body_rows[0];
        assert_eq!(e - s, 1, "unexpanded body row should be a single line");
    }

    fn line_text_of(l: &Line<'static>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn expanding_cell_wraps_it_across_multiple_lines() {
        let src = "\
| Name | Note |\n\
| --- | --- |\n\
| a | this is a long note that overflows the column badly |\n";
        let base = render(src, None, 30, &Theme::dark());
        let base_rows = base.lines.len();
        let r = render_table(src, 30, |st| {
            st.cells.insert((0, 1));
        });
        assert!(
            r.lines.len() > base_rows,
            "expanded cell should add lines ({} vs {})",
            r.lines.len(),
            base_rows
        );
        let full: String = r.lines.iter().map(line_text_of).collect();
        // The cell wraps across physical lines, so the words appear but split;
        // the tail word "badly" must survive and nothing is truncated.
        assert!(
            !full.contains('…'),
            "expanded cell must not truncate:\n{full}"
        );
        assert!(
            full.contains("badly"),
            "full cell text should be visible when expanded:\n{full}"
        );
        // The expanded body row now spans multiple physical lines.
        let (s, e) = r.table_map.regions[0].body_rows[0];
        assert!(e - s > 1, "expanded row should span multiple lines");
    }

    #[test]
    fn expanding_whole_table_shows_all_content() {
        let src = "\
| Name | Note |\n\
| --- | --- |\n\
| a | this is a long note that overflows the column badly |\n";
        let r = render_table(src, 30, |st| st.all = true);
        let full: String = r.lines.iter().map(line_text_of).collect();
        assert!(!full.contains('…'), "no truncation when fully expanded");
        assert!(full.contains("badly"));
    }

    #[test]
    fn expanding_column_reclaims_natural_width() {
        // A date column gets squeezed at narrow widths; expanding it should
        // restore enough room to show the full date.
        let src = "\
| When | Description |\n\
| --- | --- |\n\
| 2026-05-30 | some descriptive text that eats the available width here |\n";
        let narrow = render(src, None, 24, &Theme::dark());
        let narrow_txt: String = narrow.lines.iter().map(line_text_of).collect();
        assert!(
            !narrow_txt.contains("2026-05-30"),
            "date should be truncated at narrow width:\n{narrow_txt}"
        );
        let r = render_table(src, 24, |st| {
            st.cols.insert(0);
        });
        let txt: String = r.lines.iter().map(line_text_of).collect();
        assert!(
            txt.contains("2026-05-30"),
            "expanded date column should show the full date:\n{txt}"
        );
    }

    #[test]
    fn table_hit_test_classifies_clicks() {
        let src = "\
| Name | Note |\n\
| --- | --- |\n\
| a | bee |\n";
        let r = render(src, None, 80, &Theme::dark());
        let reg = &r.table_map.regions[0];
        // A vertical border column → whole-table toggle.
        let border_col = reg.border_x[0];
        assert_eq!(
            r.table_map.hit(reg.header_start, border_col),
            Some((reg.id, TableHit::All))
        );
        // A top horizontal border line → whole-table toggle.
        assert_eq!(
            r.table_map.hit(reg.line_start, reg.col_x[0].0),
            Some((reg.id, TableHit::All))
        );
        // Header cell content → column toggle.
        let (cs, _) = reg.col_x[1];
        assert_eq!(
            r.table_map.hit(reg.header_start, cs),
            Some((reg.id, TableHit::Column(1)))
        );
        // Body cell content → that cell.
        let (bs, _) = reg.body_rows[0];
        assert_eq!(
            r.table_map.hit(bs, reg.col_x[0].0),
            Some((reg.id, TableHit::Cell(0, 0)))
        );
    }

    #[test]
    fn link_span_split_across_wrapped_lines() {
        let src = "[long link text that spans multiple wrapped output lines](https://x)";
        let r = render(src, None, 16, &Theme::dark());
        assert!(r.link_map.links.len() >= 2);
        for l in &r.link_map.links {
            assert_eq!(l.target, LinkTarget::Url("https://x".to_string()));
        }
    }

    #[test]
    fn renders_unchecked_task_marker() {
        let src = "- [ ] task one\n";
        let r = render(src, None, 80, &Theme::dark());
        assert_eq!(r.checkbox_map.items.len(), 1);
        let cb = &r.checkbox_map.items[0];
        assert!(!cb.checked);
        assert_eq!(&src[cb.source_offset..cb.source_offset + 3], "[ ]");
    }

    #[test]
    fn renders_checked_task_marker() {
        let src = "- [x] done\n";
        let r = render(src, None, 80, &Theme::dark());
        assert_eq!(r.checkbox_map.items.len(), 1);
        let cb = &r.checkbox_map.items[0];
        assert!(cb.checked);
        assert_eq!(&src[cb.source_offset..cb.source_offset + 3], "[x]");
    }

    #[test]
    fn checkbox_lookup_by_line_col() {
        let src = "- [ ] task\n";
        let r = render(src, None, 80, &Theme::dark());
        let cb = r.checkbox_map.items[0].clone();
        assert_eq!(r.checkbox_map.at(cb.line, cb.col_start), Some(0));
        assert_eq!(r.checkbox_map.at(cb.line, cb.col_end - 1), Some(0));
        assert_eq!(r.checkbox_map.at(cb.line, cb.col_end), None);
    }

    #[test]
    fn multiple_checkboxes_indexed_in_order() {
        let src = "- [ ] one\n- [x] two\n- [ ] three\n";
        let r = render(src, None, 80, &Theme::dark());
        assert_eq!(r.checkbox_map.items.len(), 3);
        let states: Vec<bool> = r.checkbox_map.items.iter().map(|c| c.checked).collect();
        assert_eq!(states, vec![false, true, false]);
        for cb in &r.checkbox_map.items {
            assert_eq!(&src[cb.source_offset..cb.source_offset + 1], "[");
        }
    }
}

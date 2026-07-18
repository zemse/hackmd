use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    /// External URL — opened in the system browser.
    Url(String),
    /// Local markdown / text file — navigated to within the app.
    LocalFile(PathBuf),
    /// Anchor in the current document.
    Anchor(String),
    /// Anchor in another local file.
    FileAnchor(PathBuf, String),
}

#[derive(Clone, Debug)]
pub struct LinkSpan {
    pub line: usize,
    pub col_start: usize,
    pub col_end: usize,
    pub target: LinkTarget,
}

#[derive(Default, Clone, Debug)]
pub struct LinkMap {
    pub links: Vec<LinkSpan>,
    /// heading anchor id -> line index
    pub anchors: HashMap<String, usize>,
}

/// A `[ ]` / `[x]` task-list marker, located in both rendered output (line/col)
/// and the original source (byte offset of the opening `[`).
#[derive(Clone, Debug)]
pub struct CheckboxSpan {
    pub line: usize,
    pub col_start: usize,
    pub col_end: usize,
    pub source_offset: usize,
    pub checked: bool,
}

/// Reference to an image embed in the rendered document. Ties the line where
/// its placeholder lives to the resolved local path / URL.
#[derive(Clone, Debug)]
pub struct ImageRef {
    pub line: usize,
    pub source: PathBuf,
}

#[derive(Default, Clone, Debug)]
pub struct CheckboxMap {
    pub items: Vec<CheckboxSpan>,
}

// ---------------------------------------------------------------------------
// Tables: click-to-expand state + hit-test geometry
// ---------------------------------------------------------------------------

/// Which parts of one table are expanded (show full, untruncated content).
/// Keyed in `TableExpansions` by the table's source byte offset so it survives
/// re-renders. `all` overrides the granular sets.
#[derive(Default, Clone, Debug)]
pub struct TableExpand {
    /// Whole table expanded (clicking any border toggles this).
    pub all: bool,
    /// Columns expanded by clicking their header cell.
    pub cols: HashSet<usize>,
    /// Individual `(body_row, col)` cells expanded by clicking them.
    pub cells: HashSet<(usize, usize)>,
}

impl TableExpand {
    /// True once nothing is expanded — lets callers drop the entry entirely.
    pub fn is_empty(&self) -> bool {
        !self.all && self.cols.is_empty() && self.cells.is_empty()
    }
}

/// Per-table expansion state, keyed by table source byte offset.
pub type TableExpansions = HashMap<u64, TableExpand>;

/// What a click inside a table region resolves to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableHit {
    /// A border cell (any `│`/`─` glyph) — toggles the whole table.
    All,
    /// A header cell at this column index — toggles that column.
    Column(usize),
    /// A body cell at `(body_row, col)` — toggles just that cell.
    Cell(usize, usize),
}

/// Hit-test geometry for one rendered table. Produced by the layout pass and
/// consumed by click handling to decide whether a click expands the whole
/// table, a column, or a single cell.
#[derive(Clone, Debug)]
pub struct TableRegion {
    /// Stable id = source byte offset of the table block.
    pub id: u64,
    /// Display-line range of the whole table including borders (half-open).
    pub line_start: usize,
    pub line_end: usize,
    /// Display lines that are full-width horizontal borders.
    pub border_lines: Vec<usize>,
    /// Display-line range spanned by the header row group (half-open).
    pub header_start: usize,
    pub header_end: usize,
    /// `[start, end)` content+padding x-range of each column (between borders).
    pub col_x: Vec<(usize, usize)>,
    /// x positions occupied by the vertical `│` borders.
    pub border_x: Vec<usize>,
    /// `(line_start, line_end)` per body row, parallel to body row index.
    pub body_rows: Vec<(usize, usize)>,
}

#[derive(Default, Clone, Debug)]
pub struct TableMap {
    pub regions: Vec<TableRegion>,
}

impl TableMap {
    /// Resolve a click at display `(line, col)` to the table it lands in and
    /// the granularity it targets. A click on any border glyph expands the
    /// whole table; a click in a header cell expands that column; a click in a
    /// body cell expands that cell.
    pub fn hit(&self, line: usize, col: usize) -> Option<(u64, TableHit)> {
        for reg in &self.regions {
            if line < reg.line_start || line >= reg.line_end {
                continue;
            }
            // Any border glyph (horizontal border row or vertical bar column)
            // → whole-table toggle.
            if reg.border_lines.contains(&line) || reg.border_x.contains(&col) {
                return Some((reg.id, TableHit::All));
            }
            let column = reg.col_x.iter().position(|&(s, e)| col >= s && col < e);
            let Some(ci) = column else {
                // Inside the table outline but not in any column cell — treat
                // as a border-style whole-table toggle.
                return Some((reg.id, TableHit::All));
            };
            if line >= reg.header_start && line < reg.header_end {
                return Some((reg.id, TableHit::Column(ci)));
            }
            if let Some(ri) = reg
                .body_rows
                .iter()
                .position(|&(s, e)| line >= s && line < e)
            {
                return Some((reg.id, TableHit::Cell(ri, ci)));
            }
            return Some((reg.id, TableHit::All));
        }
        None
    }
}

impl CheckboxMap {
    pub fn at(&self, line: usize, col: usize) -> Option<usize> {
        self.items
            .iter()
            .position(|c| c.line == line && col >= c.col_start && col < c.col_end)
    }
}

#[allow(dead_code)]
impl LinkMap {
    /// Returns the index of the link containing (line, col), if any.
    pub fn at(&self, line: usize, col: usize) -> Option<usize> {
        self.links
            .iter()
            .position(|l| l.line == line && col >= l.col_start && col < l.col_end)
    }

    /// First link strictly after `(line, col)` — skips any link at exactly `col`.
    pub fn next_from(&self, line: usize, col: usize) -> Option<usize> {
        self.links
            .iter()
            .enumerate()
            .find(|(_, l)| l.line > line || (l.line == line && l.col_start > col))
            .map(|(i, _)| i)
    }

    pub fn prev_from(&self, line: usize, col: usize) -> Option<usize> {
        self.links
            .iter()
            .enumerate()
            .rev()
            .find(|(_, l)| l.line < line || (l.line == line && l.col_end <= col))
            .map(|(i, _)| i)
    }
}

/// Resolve a markdown link's `dest_url` into a typed [`LinkTarget`].
pub fn resolve(dest: &str, base_dir: Option<&Path>) -> LinkTarget {
    if dest.starts_with('#') {
        return LinkTarget::Anchor(slugify(&dest[1..]));
    }
    if let Some((scheme, _)) = dest.split_once("://") {
        if !scheme.is_empty() {
            return LinkTarget::Url(dest.to_string());
        }
    }
    if dest.starts_with("mailto:") || dest.starts_with("tel:") {
        return LinkTarget::Url(dest.to_string());
    }
    let (path_part, anchor) = match dest.split_once('#') {
        Some((p, a)) => (p, Some(a.to_string())),
        None => (dest, None),
    };
    let decoded = percent_decode(path_part);
    let path: PathBuf = match base_dir {
        Some(b) => b.join(&decoded),
        None => PathBuf::from(decoded),
    };
    match anchor {
        Some(a) => LinkTarget::FileAnchor(path, slugify(&a)),
        None => LinkTarget::LocalFile(path),
    }
}

/// Minimal URL percent-decoder for local link paths (handles `%20` and friends).
/// UTF-8 aware: bytes are decoded then re-validated; falls back to the original
/// string if the result isn't valid UTF-8.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// One heading pulled from a markdown document, for anchor autocomplete.
/// `text` is the plain heading text (inline markup stripped); `slug` is the
/// GitHub-style anchor it resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocHeading {
    pub level: u8,
    pub text: String,
    pub slug: String,
}

/// Extract every heading from `md` in document order. Uses the same parser and
/// options as the renderer so detection (ATX, setext, code-fence skipping) and
/// the resulting slugs match the anchors the reader actually jumps to. Blank
/// headings are dropped. Duplicate slugs are kept as-is (the renderer does not
/// de-duplicate them either).
pub fn extract_headings(md: &str) -> Vec<DocHeading> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_SMART_PUNCTUATION);
    opts.insert(Options::ENABLE_WIKILINKS);

    let mut out = Vec::new();
    // (level, accumulated text) while inside a heading.
    let mut cur: Option<(u8, String)> = None;
    for ev in Parser::new_ext(md, opts) {
        match ev {
            Event::Start(Tag::Heading { level, .. }) => {
                cur = Some((level as u8, String::new()));
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some((_, buf)) = cur.as_mut() {
                    buf.push_str(&t);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, buf)) = cur.take() {
                    let text = buf.trim().to_string();
                    if !text.is_empty() {
                        let slug = slugify(&text);
                        out.push(DocHeading { level, text, slug });
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Convert heading text to a GitHub-style anchor slug.
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            for c in ch.to_lowercase() {
                out.push(c);
            }
            prev_dash = false;
        } else if ch.is_whitespace() || ch == '-' || ch == '_' {
            if !prev_dash && !out.is_empty() {
                out.push('-');
                prev_dash = true;
            }
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod heading_tests {
    use super::{DocHeading, extract_headings};

    fn slugs(md: &str) -> Vec<String> {
        extract_headings(md).into_iter().map(|h| h.slug).collect()
    }

    #[test]
    fn extracts_atx_headings_with_slugs() {
        let hs = extract_headings("# Hello World\n\ntext\n\n## Sub-Section!\n");
        assert_eq!(
            hs,
            vec![
                DocHeading {
                    level: 1,
                    text: "Hello World".into(),
                    slug: "hello-world".into(),
                },
                DocHeading {
                    level: 2,
                    text: "Sub-Section!".into(),
                    slug: "sub-section".into(),
                },
            ]
        );
    }

    #[test]
    fn extracts_setext_headings() {
        assert_eq!(slugs("Title\n=====\n\nSub\n---\n"), vec!["title", "sub"]);
    }

    #[test]
    fn skips_headings_inside_code_fences() {
        let md = "# Real\n\n```\n# Not A Heading\n```\n\n## Also Real\n";
        assert_eq!(slugs(md), vec!["real", "also-real"]);
    }

    #[test]
    fn keeps_inline_code_in_heading_text() {
        let hs = extract_headings("# Use `cargo` now\n");
        assert_eq!(hs[0].text, "Use cargo now");
        assert_eq!(hs[0].slug, "use-cargo-now");
    }
}

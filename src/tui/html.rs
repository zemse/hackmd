//! Minimal HTML parsing for the markdown reader.
//!
//! Markdown documents carry a small, well-formed HTML subset that CommonMark
//! passes through untouched: `<table>` grids with `colspan`/`rowspan` (the one
//! way to write a merged-cell table in markdown), `<details>` folds, and inline
//! `<b>`/`<code>`/`<kbd>` style tags. This module tokenizes that subset and
//! shapes the table and inline pieces into plain data; the renderer turns that
//! into styled runs. Anything unrecognized degrades to its text content rather
//! than being dropped.

/// One HTML lexeme. Comments and doctypes are dropped by the tokenizer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    /// `<name attr="v">` or `<name/>`. `name` is lowercased.
    Open {
        name: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
    },
    /// `</name>`, lowercased.
    Close(String),
    /// Character data with entities already decoded.
    Text(String),
}

impl Token {
    /// True when this is an opening `name` tag.
    pub fn opens(&self, name: &str) -> bool {
        matches!(self, Token::Open { name: n, .. } if n == name)
    }

    /// True when this is a closing `name` tag.
    pub fn closes(&self, name: &str) -> bool {
        matches!(self, Token::Close(n) if n == name)
    }
}

/// Elements that never have a closing tag, so `<br>` and friends don't swallow
/// the rest of the document.
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Look up an attribute value, case-insensitively on the name.
pub fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// Split HTML into tags and text. Malformed input is treated as text — an
/// unterminated `<` stays a literal `<` rather than eating the remainder.
pub fn tokenize(src: &str) -> Vec<Token> {
    let b = src.as_bytes();
    let mut out: Vec<Token> = Vec::new();
    let mut text = String::new();
    let mut i = 0usize;

    while i < b.len() {
        if b[i] != b'<' {
            let start = i;
            while i < b.len() && b[i] != b'<' {
                i += 1;
            }
            text.push_str(&src[start..i]);
            continue;
        }
        // `<!-- … -->` and `<!doctype …>` carry nothing to render.
        if src[i..].starts_with("<!--") {
            match src[i + 4..].find("-->") {
                Some(end) => {
                    i = i + 4 + end + 3;
                    continue;
                }
                None => {
                    i = b.len();
                    continue;
                }
            }
        }
        if src[i..].starts_with("<!") || src[i..].starts_with("<?") {
            match src[i..].find('>') {
                Some(end) => {
                    i += end + 1;
                    continue;
                }
                None => {
                    i = b.len();
                    continue;
                }
            }
        }
        let closing = src[i..].starts_with("</");
        let name_start = if closing { i + 2 } else { i + 1 };
        if name_start >= b.len() || !b[name_start].is_ascii_alphabetic() {
            text.push('<');
            i += 1;
            continue;
        }
        let Some((tag, next)) = parse_tag(src, name_start, closing) else {
            text.push('<');
            i += 1;
            continue;
        };
        if !text.is_empty() {
            out.push(Token::Text(std::mem::take(&mut text)));
        }
        out.push(tag);
        i = next;
    }
    if !text.is_empty() {
        out.push(Token::Text(text));
    }
    out
}

/// Parse one tag whose name starts at `at`. Returns the token and the byte
/// offset just past the closing `>`, or `None` if the tag never closes.
fn parse_tag(src: &str, at: usize, closing: bool) -> Option<(Token, usize)> {
    let b = src.as_bytes();
    let mut i = at;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-' || b[i] == b':') {
        i += 1;
    }
    let name = src[at..i].to_ascii_lowercase();

    if closing {
        let end = src[i..].find('>')? + i;
        return Some((Token::Close(name), end + 1));
    }

    let mut attrs: Vec<(String, String)> = Vec::new();
    let mut self_closing = false;
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            return None;
        }
        if b[i] == b'>' {
            i += 1;
            break;
        }
        if b[i] == b'/' {
            self_closing = true;
            i += 1;
            continue;
        }
        let key_start = i;
        while i < b.len()
            && !b[i].is_ascii_whitespace()
            && b[i] != b'='
            && b[i] != b'>'
            && b[i] != b'/'
        {
            i += 1;
        }
        if i == key_start {
            // Not a name character and not a delimiter — skip it so a stray
            // byte can't spin the loop.
            i += 1;
            continue;
        }
        let key = src[key_start..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let quote = b[i];
                i += 1;
                let vs = i;
                while i < b.len() && b[i] != quote {
                    i += 1;
                }
                value = decode_entities(&src[vs..i.min(b.len())]);
                i = (i + 1).min(b.len());
            } else {
                let vs = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                    i += 1;
                }
                value = decode_entities(&src[vs..i]);
            }
        }
        attrs.push((key, value));
    }

    let self_closing = self_closing || VOID.contains(&name.as_str());
    Some((
        Token::Open {
            name,
            attrs,
            self_closing,
        },
        i,
    ))
}

/// Decode the named and numeric character references that show up in hand-
/// written markdown. Unknown references are left as-is.
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        if b[i] != b'&' {
            let start = i;
            while i < b.len() && b[i] != b'&' {
                i += 1;
            }
            out.push_str(&s[start..i]);
            continue;
        }
        let Some(semi) = s[i..].find(';').filter(|&n| n <= 12) else {
            out.push('&');
            i += 1;
            continue;
        };
        let body = &s[i + 1..i + semi];
        let decoded = if let Some(hex) = body.strip_prefix("#x").or(body.strip_prefix("#X")) {
            u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
        } else if let Some(dec) = body.strip_prefix('#') {
            dec.parse::<u32>().ok().and_then(char::from_u32)
        } else {
            named_entity(body)
        };
        match decoded {
            Some(c) => {
                out.push(c);
                i += semi + 1;
            }
            None => {
                out.push('&');
                i += 1;
            }
        }
    }
    out
}

fn named_entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "times" => '×',
        "divide" => '÷',
        "deg" => '°',
        "plusmn" => '±',
        "le" => '≤',
        "ge" => '≥',
        "ne" => '≠',
        "rarr" => '→',
        "larr" => '←',
        "uarr" => '↑',
        "darr" => '↓',
        "check" => '✓',
        "cross" => '✗',
        "bull" => '•',
        "middot" => '·',
        "laquo" => '«',
        "raquo" => '»',
        "lsquo" => '\u{2018}',
        "rsquo" => '\u{2019}',
        "ldquo" => '\u{201c}',
        "rdquo" => '\u{201d}',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "sect" => '§',
        "para" => '¶',
        "dagger" => '†',
        "infin" => '∞',
        _ => return None,
    })
}

/// Inline styling carried by a [`Fragment`]. Flags stack, so text inside
/// `<b><code>` carries both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Emphasis {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    /// `<mark>` — highlighted text.
    pub mark: bool,
    /// `<kbd>` — a key name, rendered like a keycap.
    pub kbd: bool,
    /// `<sub>` / `<sup>` — raised or lowered text.
    pub sub: bool,
    pub sup: bool,
}

/// One styled run of text pulled out of an HTML fragment. `text` may contain
/// `\n` (from `<br>` or a `<pre>` block); everything else is already
/// whitespace-collapsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fragment {
    pub text: String,
    pub emph: Emphasis,
    /// `href` of the enclosing `<a>`, if any.
    pub href: Option<String>,
}

/// Horizontal cell alignment from an `align=` attribute or a `text-align`
/// style. `None` means "inherit the column default".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// One `<td>`/`<th>`, or a synthetic continuation cell standing in for the
/// part of a `rowspan` that reaches into a later row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub frags: Vec<Fragment>,
    pub align: Option<Align>,
    /// Grid columns covered (>= 1).
    pub colspan: usize,
    pub header: bool,
}

/// A parsed `<table>`, already resolved into a rectangular grid: every row's
/// colspans sum to `cols`, and `rowspan` continuations are filled in as empty
/// cells so the renderer never has to look across rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    /// Header rows (`<thead>`, or a leading row made only of `<th>`).
    pub head: Vec<Vec<Cell>>,
    pub body: Vec<Vec<Cell>>,
    pub cols: usize,
}

/// Parse the `<table>` opening at `tokens[start]`. Returns the table and the
/// index just past its `</table>` (or the end of input for an unclosed table).
pub fn parse_table(tokens: &[Token], start: usize) -> (Table, usize) {
    let mut head: Vec<Vec<RawRow>> = Vec::new();
    let mut body: Vec<Vec<RawRow>> = Vec::new();
    // Rows land in `head` while inside `<thead>`, in `body` otherwise. A
    // table with no `<thead>` promotes a leading all-`<th>` row instead.
    let mut in_head = false;
    let mut i = start + 1;
    let mut section_head: Vec<RawRow> = Vec::new();
    let mut section_body: Vec<RawRow> = Vec::new();

    while i < tokens.len() {
        match &tokens[i] {
            Token::Close(n) if n == "table" => {
                i += 1;
                break;
            }
            Token::Open { name, .. } if name == "thead" => {
                in_head = true;
                i += 1;
            }
            Token::Close(n) if n == "thead" => {
                in_head = false;
                i += 1;
            }
            Token::Open { name, .. } if name == "tbody" || name == "tfoot" => {
                in_head = false;
                i += 1;
            }
            Token::Open { name, .. } if name == "tr" => {
                let (row, next) = parse_row(tokens, i);
                i = next;
                if in_head {
                    section_head.push(row);
                } else {
                    section_body.push(row);
                }
            }
            _ => i += 1,
        }
    }
    if !section_head.is_empty() {
        head.push(section_head);
    }
    if !section_body.is_empty() {
        body.push(section_body);
    }

    let mut head_rows: Vec<RawRow> = head.into_iter().flatten().collect();
    let mut body_rows: Vec<RawRow> = body.into_iter().flatten().collect();
    // No `<thead>`: a first row of nothing but `<th>` is the header, matching
    // how browsers render such a table.
    if head_rows.is_empty()
        && body_rows
            .first()
            .map(|r| !r.is_empty() && r.iter().all(|c| c.header))
            .unwrap_or(false)
    {
        head_rows.push(body_rows.remove(0));
    }

    let mut table = Table::default();
    let head_len = head_rows.len();
    let mut rows = head_rows;
    rows.append(&mut body_rows);
    let grid = resolve_spans(rows);
    table.cols = grid.iter().map(|r| row_width(r)).max().unwrap_or(0);
    for row in &grid {
        // Short rows (a `<tr>` with fewer cells than its siblings) get a
        // filler so every row lines up with the column grid.
        let w = row_width(row);
        if w < table.cols {
            let mut row = row.clone();
            row.push(Cell {
                frags: Vec::new(),
                align: None,
                colspan: table.cols - w,
                header: false,
            });
            push_row(&mut table, row, head_len);
        } else {
            push_row(&mut table, row.clone(), head_len);
        }
    }
    (table, i)
}

fn push_row(table: &mut Table, row: Vec<Cell>, head_len: usize) {
    if table.head.len() < head_len {
        table.head.push(row);
    } else {
        table.body.push(row);
    }
}

fn row_width(row: &[Cell]) -> usize {
    row.iter().map(|c| c.colspan).sum()
}

/// A cell as written, before `rowspan`s are expanded into the grid.
#[derive(Clone, Debug)]
struct RawCell {
    frags: Vec<Fragment>,
    align: Option<Align>,
    colspan: usize,
    rowspan: usize,
    header: bool,
}

type RawRow = Vec<RawCell>;

/// Parse the `<tr>` opening at `tokens[start]`; returns its cells and the
/// index just past `</tr>`.
fn parse_row(tokens: &[Token], start: usize) -> (RawRow, usize) {
    let mut row: RawRow = Vec::new();
    let mut i = start + 1;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Close(n) if n == "tr" => {
                i += 1;
                break;
            }
            // A missing `</tr>` shouldn't swallow the next row or the table.
            Token::Open { name, .. } if name == "tr" => break,
            Token::Close(n) if n == "table" || n == "thead" || n == "tbody" || n == "tfoot" => {
                break;
            }
            Token::Open { name, attrs, .. } if name == "td" || name == "th" => {
                let header = name == "th";
                let align = align_of(attrs);
                let colspan = span_attr(attrs, "colspan");
                let rowspan = span_attr(attrs, "rowspan");
                let (frags, next) = cell_fragments(tokens, i + 1);
                i = next;
                row.push(RawCell {
                    frags,
                    align,
                    colspan,
                    rowspan,
                    header,
                });
            }
            _ => i += 1,
        }
    }
    (row, i)
}

/// `colspan`/`rowspan` value, clamped to a sane range — a bogus `colspan=999`
/// shouldn't blow the grid up.
fn span_attr(attrs: &[(String, String)], name: &str) -> usize {
    attr(attrs, name)
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 64)
}

fn align_of(attrs: &[(String, String)]) -> Option<Align> {
    let raw = attr(attrs, "align").map(str::to_string).or_else(|| {
        let style = attr(attrs, "style")?;
        let at = style.to_ascii_lowercase().find("text-align")?;
        let value = style[at..].split_once(':')?.1;
        Some(
            value
                .trim_start()
                .trim_end_matches(';')
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_string(),
        )
    })?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "left" | "start" => Some(Align::Left),
        "center" | "centre" | "middle" => Some(Align::Center),
        "right" | "end" => Some(Align::Right),
        _ => None,
    }
}

/// Collect a cell's inline content, stopping at its own close tag or at the
/// next cell / row / table boundary (cells are routinely left unclosed).
fn cell_fragments(tokens: &[Token], start: usize) -> (Vec<Fragment>, usize) {
    let mut end = start;
    while end < tokens.len() {
        let stop = match &tokens[end] {
            Token::Close(n) => matches!(
                n.as_str(),
                "td" | "th" | "tr" | "thead" | "tbody" | "tfoot" | "table"
            ),
            Token::Open { name, .. } => matches!(name.as_str(), "td" | "th" | "tr"),
            _ => false,
        };
        if stop {
            break;
        }
        end += 1;
    }
    let frags = fragments(&tokens[start..end]);
    // Step past our own `</td>`; leave any other boundary for the caller.
    let next = match tokens.get(end) {
        Some(Token::Close(n)) if n == "td" || n == "th" => end + 1,
        _ => end,
    };
    (frags, next)
}

/// Expand `rowspan` into the grid: a cell that reaches into later rows leaves
/// an empty continuation cell of the same width in each of them.
fn resolve_spans(rows: Vec<RawRow>) -> Vec<Vec<Cell>> {
    // (grid column, rows still to fill, width) for each in-flight rowspan.
    let mut pending: Vec<(usize, usize, usize)> = Vec::new();
    let mut out: Vec<Vec<Cell>> = Vec::with_capacity(rows.len());

    for raw in rows {
        let mut row: Vec<Cell> = Vec::new();
        let mut col = 0usize;
        let mut cells = raw.into_iter();
        let mut next = cells.next();
        loop {
            // Continuations first: they own their column before any cell
            // written on this row does.
            if let Some(idx) = pending.iter().position(|&(c, _, _)| c == col) {
                let (_, left, width) = pending[idx];
                row.push(Cell {
                    frags: Vec::new(),
                    align: None,
                    colspan: width,
                    header: false,
                });
                col += width;
                if left <= 1 {
                    pending.remove(idx);
                } else {
                    pending[idx].1 = left - 1;
                }
                continue;
            }
            let Some(cell) = next.take() else { break };
            if cell.rowspan > 1 {
                pending.push((col, cell.rowspan - 1, cell.colspan));
            }
            col += cell.colspan;
            row.push(Cell {
                frags: cell.frags,
                align: cell.align,
                colspan: cell.colspan,
                header: cell.header,
            });
            next = cells.next();
        }
        // Trailing continuations past the last written cell.
        while let Some(idx) = pending.iter().position(|&(c, _, _)| c == col) {
            let (_, left, width) = pending[idx];
            row.push(Cell {
                frags: Vec::new(),
                align: None,
                colspan: width,
                header: false,
            });
            col += width;
            if left <= 1 {
                pending.remove(idx);
            } else {
                pending[idx].1 = left - 1;
            }
        }
        out.push(row);
    }
    out
}

/// Flatten inline HTML into styled fragments. Whitespace is collapsed the way
/// a browser would, except inside `<pre>`; `<br>` becomes a newline. Unknown
/// tags contribute nothing but their text.
pub fn fragments(tokens: &[Token]) -> Vec<Fragment> {
    let mut out: Vec<Fragment> = Vec::new();
    let mut emph = Emphasis::default();
    let mut stack: Vec<(String, Emphasis, Option<String>)> = Vec::new();
    let mut href: Option<String> = None;
    let mut pre = 0usize;
    // Set right after a `<br>` so the next run doesn't start with the
    // whitespace that followed it in the source.
    let mut at_line_start = false;

    for tok in tokens {
        match tok {
            Token::Text(t) => {
                let mut text = if pre > 0 { t.clone() } else { collapse_ws(t) };
                if at_line_start && pre == 0 {
                    text = text.trim_start().to_string();
                }
                if text.is_empty() {
                    continue;
                }
                at_line_start = text.ends_with('\n');
                push_frag(&mut out, text, emph, href.clone());
            }
            Token::Open {
                name,
                attrs,
                self_closing,
            } => {
                match name.as_str() {
                    "br" => {
                        // The space a source line leaves before `<br>` is
                        // invisible in HTML; keeping it would pad the cell.
                        trim_line_end(&mut out);
                        at_line_start = true;
                        push_frag(&mut out, "\n".to_string(), emph, href.clone());
                    }
                    "wbr" => {}
                    "img" => {
                        // No pixels here — show the alt text, or the file name.
                        let label = attr(attrs, "alt")
                            .filter(|a| !a.is_empty())
                            .map(str::to_string)
                            .or_else(|| attr(attrs, "src").map(str::to_string))
                            .unwrap_or_default();
                        if !label.is_empty() {
                            push_frag(&mut out, format!("[image: {label}]"), emph, href.clone());
                        }
                    }
                    _ => {}
                }
                if *self_closing {
                    continue;
                }
                stack.push((name.clone(), emph, href.clone()));
                match name.as_str() {
                    "b" | "strong" => emph.bold = true,
                    "i" | "em" | "cite" | "var" | "dfn" => emph.italic = true,
                    "u" | "ins" => emph.underline = true,
                    "s" | "del" | "strike" => emph.strike = true,
                    "code" | "samp" | "tt" => emph.code = true,
                    "mark" => emph.mark = true,
                    "kbd" => emph.kbd = true,
                    "sub" => emph.sub = true,
                    "sup" => emph.sup = true,
                    "pre" => pre += 1,
                    "a" => href = attr(attrs, "href").map(str::to_string).or(href),
                    _ => {}
                }
            }
            Token::Close(name) => {
                // Unwind to the matching open tag; a stray `</x>` with no open
                // is ignored.
                if let Some(at) = stack.iter().rposition(|(n, _, _)| n == name) {
                    if name == "pre" {
                        pre = pre.saturating_sub(1);
                    }
                    let (_, e, h) = stack[at].clone();
                    emph = e;
                    href = h;
                    stack.truncate(at);
                }
            }
        }
    }

    // Leading/trailing whitespace belongs to the markup, not the content.
    trim_fragments(&mut out);
    out
}

/// Drop trailing spaces from the text built so far, stopping at a newline.
fn trim_line_end(out: &mut Vec<Fragment>) {
    while let Some(last) = out.last_mut() {
        let trimmed = last.text.trim_end_matches([' ', '\t']).to_string();
        if trimmed == last.text {
            break;
        }
        last.text = trimmed;
        if last.text.is_empty() {
            out.pop();
        } else {
            break;
        }
    }
}

fn push_frag(out: &mut Vec<Fragment>, text: String, emph: Emphasis, href: Option<String>) {
    if let Some(last) = out.last_mut()
        && last.emph == emph
        && last.href == href
    {
        last.text.push_str(&text);
        return;
    }
    out.push(Fragment { text, emph, href });
}

/// Collapse every run of whitespace (newlines included — HTML source wraps
/// freely) into a single space.
fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            space = true;
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(ch);
    }
    if space {
        out.push(' ');
    }
    out
}

fn trim_fragments(frags: &mut Vec<Fragment>) {
    while let Some(first) = frags.first_mut() {
        let trimmed = first.text.trim_start().to_string();
        if trimmed.is_empty() {
            frags.remove(0);
        } else {
            first.text = trimmed;
            break;
        }
    }
    while let Some(last) = frags.last_mut() {
        let trimmed = last.text.trim_end().to_string();
        if trimmed.is_empty() {
            frags.pop();
        } else {
            last.text = trimmed;
            break;
        }
    }
}

/// Plain text of a fragment list.
#[cfg(test)]
pub fn fragments_text(frags: &[Fragment]) -> String {
    frags.iter().map(|f| f.text.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell_text(row: &[Cell], i: usize) -> String {
        fragments_text(&row[i].frags)
    }

    #[test]
    fn tokenizes_tags_attrs_and_text() {
        let toks = tokenize(r#"<a href="x.md" data-y>hi</a> tail"#);
        assert_eq!(
            toks,
            vec![
                Token::Open {
                    name: "a".into(),
                    attrs: vec![("href".into(), "x.md".into()), ("data-y".into(), "".into())],
                    self_closing: false,
                },
                Token::Text("hi".into()),
                Token::Close("a".into()),
                Token::Text(" tail".into()),
            ]
        );
    }

    #[test]
    fn drops_comments_and_keeps_stray_lt() {
        let toks = tokenize("a <!-- note --> b < c");
        assert_eq!(toks, vec![Token::Text("a  b < c".into())]);
    }

    #[test]
    fn void_tags_are_self_closing() {
        let toks = tokenize("<br><img src='a.png'/>");
        assert!(matches!(
            &toks[0],
            Token::Open {
                self_closing: true,
                ..
            }
        ));
        assert!(matches!(
            &toks[1],
            Token::Open {
                self_closing: true,
                ..
            }
        ));
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(
            decode_entities("a &amp; b &lt;c&gt; &#65;&#x42; &nope;"),
            "a & b <c> AB &nope;"
        );
    }

    #[test]
    fn fragments_carry_nested_styles() {
        let frags = fragments(&tokenize("plain <b>bold <code>c</code></b>"));
        assert_eq!(frags.len(), 3);
        assert_eq!(frags[0].text, "plain ");
        assert!(frags[1].emph.bold && !frags[1].emph.code);
        assert!(frags[2].emph.bold && frags[2].emph.code);
    }

    #[test]
    fn fragments_collapse_whitespace_and_break_on_br() {
        let frags = fragments(&tokenize("  one\n  two  <br>three "));
        assert_eq!(fragments_text(&frags), "one two\nthree");
    }

    #[test]
    fn fragments_keep_pre_whitespace() {
        let frags = fragments(&tokenize("<pre>a\n  b</pre>"));
        assert_eq!(fragments_text(&frags), "a\n  b");
    }

    #[test]
    fn fragments_flag_sub_and_sup() {
        let frags = fragments(&tokenize("x<sup>2</sup>H<sub>2</sub>O"));
        assert!(frags[1].emph.sup && !frags[1].emph.sub);
        assert!(frags[3].emph.sub);
    }

    #[test]
    fn fragments_record_links() {
        let frags = fragments(&tokenize(r#"see <a href="https://x">here</a>"#));
        assert_eq!(frags[1].href.as_deref(), Some("https://x"));
        assert_eq!(frags[0].href, None);
    }

    #[test]
    fn parses_simple_table_with_thead() {
        let toks = tokenize(
            "<table><thead><tr><th>a</th><th align=\"right\">b</th></tr></thead>\
             <tbody><tr><td>1</td><td>2</td></tr></tbody></table>",
        );
        let (t, next) = parse_table(&toks, 0);
        assert_eq!(next, toks.len());
        assert_eq!(t.cols, 2);
        assert_eq!(t.head.len(), 1);
        assert_eq!(t.body.len(), 1);
        assert_eq!(cell_text(&t.head[0], 1), "b");
        assert_eq!(t.head[0][1].align, Some(Align::Right));
        assert_eq!(cell_text(&t.body[0], 0), "1");
    }

    #[test]
    fn promotes_leading_th_row_without_thead() {
        let toks = tokenize("<table><tr><th>a</th></tr><tr><td>1</td></tr></table>");
        let (t, _) = parse_table(&toks, 0);
        assert_eq!(t.head.len(), 1);
        assert_eq!(t.body.len(), 1);
    }

    #[test]
    fn expands_colspan_and_rowspan_into_a_rectangular_grid() {
        let toks = tokenize(
            "<table><thead>\
             <tr><th rowspan=\"2\">prog</th><th>gpu</th><th colspan=\"2\">cpu</th></tr>\
             <tr><th>a</th><th>b</th><th>c</th></tr>\
             </thead><tbody><tr><td>p</td><td>1</td><td>2</td><td>3</td></tr></tbody></table>",
        );
        let (t, _) = parse_table(&toks, 0);
        assert_eq!(t.cols, 4);
        assert_eq!(
            t.head[0].iter().map(|c| c.colspan).collect::<Vec<_>>(),
            vec![1, 1, 2]
        );
        // Row 2 opens with the rowspan continuation, then its own three cells.
        assert_eq!(t.head[1].len(), 4);
        assert_eq!(cell_text(&t.head[1], 0), "");
        assert_eq!(cell_text(&t.head[1], 1), "a");
        assert_eq!(t.body[0].len(), 4);
    }

    #[test]
    fn pads_short_rows_to_the_grid_width() {
        let toks = tokenize("<table><tr><td>a</td><td>b</td></tr><tr><td>c</td></tr></table>");
        let (t, _) = parse_table(&toks, 0);
        assert_eq!(t.cols, 2);
        assert_eq!(t.body[1].iter().map(|c| c.colspan).sum::<usize>(), 2);
    }

    #[test]
    fn tolerates_unclosed_cells_and_rows() {
        let toks = tokenize("<table><tr><td>a<td>b<tr><td>c</table>");
        let (t, _) = parse_table(&toks, 0);
        assert_eq!(t.cols, 2);
        assert_eq!(t.body.len(), 2);
        assert_eq!(cell_text(&t.body[0], 1), "b");
        assert_eq!(cell_text(&t.body[1], 0), "c");
    }
}

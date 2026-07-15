//! Marp deck detection and parsing for the presentation reader.
//!
//! [Marp](https://marp.app) turns a Markdown file into a slide deck. A deck is
//! enabled by a `marp: true` line in the leading YAML front matter (or, as a
//! convenience, a leading `<!-- marp: true -->` HTML-comment directive). Slides
//! are separated by `---` page breaks, and presentation directives
//! (`header`, `footer`, `paginate`, `class`, …) live in the front matter and in
//! `<!-- ... -->` comments — global by default, or scoped to a single slide when
//! prefixed with `_` (e.g. `_class: lead`).
//!
//! We can't reproduce Marp's CSS themes in a terminal, so this module extracts
//! just what a text renderer can use: the per-slide Markdown (comments and
//! directives stripped out) plus the resolved header/footer/paginate/lead state.
//! The reader renders each slide with the normal Markdown pipeline and paints
//! the chrome around it (see `ui::draw_slide`).

/// A parsed Marp deck: an ordered list of slides.
#[derive(Clone, Debug)]
pub struct Deck {
    pub slides: Vec<Slide>,
}

impl Deck {
    pub fn len(&self) -> usize {
        self.slides.len()
    }

    /// Always `false` — `parse` guarantees at least one slide. Present only to
    /// satisfy the `len_without_is_empty` lint.
    pub fn is_empty(&self) -> bool {
        self.slides.is_empty()
    }
}

/// One slide's renderable content plus its resolved chrome.
#[derive(Clone, Debug)]
pub struct Slide {
    /// Slide Markdown with front matter, page breaks, HTML comments and
    /// directives already removed — ready for the Markdown renderer.
    pub body: String,
    /// Effective header text drawn at the top (global unless overridden).
    pub header: Option<String>,
    /// Effective footer text drawn at the bottom-left.
    pub footer: Option<String>,
    /// Whether to show the `n / total` page number.
    pub paginate: bool,
    /// The slide carries a `lead` class — Marp's built-in "center everything"
    /// layout, which we approximate by vertically + horizontally centering.
    pub lead: bool,
    /// Background images (`![bg ...](url)`) extracted from the body. A `left`/
    /// `right` background reserves a column of the slide for the image; the text
    /// flows in what remains.
    pub background: Vec<BgImage>,
}

/// A Marp background image and where it sits on the slide.
#[derive(Clone, Debug)]
pub struct BgImage {
    pub url: String,
    pub placement: BgPlacement,
}

/// Background placement. The fraction is the share of the slide width the image
/// column occupies (defaults to half when `left`/`right` carry no `:NN%`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BgPlacement {
    Full,
    Left(f32),
    Right(f32),
}

impl Slide {
    /// Cells to reserve on the left / right for background images at `total`
    /// slide width. Never crushes the text column below a readable minimum.
    pub fn split_widths(&self, total: u16) -> (u16, u16) {
        let mut left = 0u16;
        let mut right = 0u16;
        for bg in &self.background {
            match bg.placement {
                BgPlacement::Left(f) => left = left.max((total as f32 * f).round() as u16),
                BgPlacement::Right(f) => right = right.max((total as f32 * f).round() as u16),
                BgPlacement::Full => {}
            }
        }
        let budget = total.saturating_sub(16u16.min(total));
        while left + right > budget && (left > 0 || right > 0) {
            if left >= right {
                left = left.saturating_sub(1);
            } else {
                right = right.saturating_sub(1);
            }
        }
        (left, right)
    }
}

/// True when `raw` should be treated as a Marp deck.
pub fn detect(raw: &str) -> bool {
    // Front matter: `---` on the first line, then a `marp: true` directive
    // before the closing `---`/`...`.
    if let Some(first_end) = raw.find('\n')
        && raw[..first_end].trim_end() == "---"
    {
        let mut idx = first_end + 1;
        while idx <= raw.len() {
            let end = raw[idx..].find('\n').map(|p| idx + p).unwrap_or(raw.len());
            let line = raw[idx..end].trim();
            if line == "---" || line == "..." {
                break;
            }
            if is_marp_true(line) {
                return true;
            }
            if end >= raw.len() {
                break;
            }
            idx = end + 1;
        }
    }
    // Convenience: a leading `<!-- marp: true -->` comment, ahead of any real
    // content. Skip blank lines and other leading comments while scanning.
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(inner) = t.strip_prefix("<!--").and_then(|s| s.strip_suffix("-->")) {
            if inner.split(&['\n', ';'][..]).any(is_marp_true) {
                return true;
            }
            continue;
        }
        break;
    }
    false
}

/// Parse `raw` into a deck. Never fails: a malformed or empty document yields a
/// single (possibly empty) slide so the presenter always has something to show.
pub fn parse(raw: &str) -> Deck {
    let mut global = Directives::default();
    // Front-matter spot directives (`_class: lead`, …) apply to the first slide.
    let mut pending_local = Directives::default();
    let body = strip_front_matter(raw, &mut global, &mut pending_local);

    let mut slides = Vec::new();
    for chunk in split_slides(body) {
        let mut local = Directives::default();
        let clean = strip_comments(&chunk, &mut global, &mut local);
        if clean.trim().is_empty() {
            // A chunk that was only comments/directives (or blank) is not a real
            // slide — but its global directives above still carry forward.
            continue;
        }
        // Fold the pending front-matter spot directives into the first real
        // slide (in-slide `_` comments still win over them), then consume.
        let local = std::mem::take(&mut pending_local).merged(&local);
        let eff = global.merged(&local);
        // Pull `![bg ...]` background images out of the flow so they don't
        // render as inline `[image: …]` text; draw_slide positions them.
        let (body, background) = extract_backgrounds(&clean);
        slides.push(Slide {
            body: body.trim_matches('\n').to_string(),
            header: eff.header,
            footer: eff.footer,
            paginate: eff.paginate.unwrap_or(false),
            lead: eff
                .class
                .as_deref()
                .map(|c| c.split_whitespace().any(|w| w == "lead"))
                .unwrap_or(false),
            background,
        });
    }
    if slides.is_empty() {
        slides.push(Slide {
            body: String::new(),
            header: None,
            footer: None,
            paginate: false,
            lead: false,
            background: Vec::new(),
        });
    }
    Deck { slides }
}

/// `marp: true` (any surrounding whitespace).
fn is_marp_true(s: &str) -> bool {
    let s = s.trim();
    let Some(rest) = s.strip_prefix("marp") else {
        return false;
    };
    let Some(rest) = rest.trim_start().strip_prefix(':') else {
        return false;
    };
    rest.trim() == "true"
}

/// Global/local presentation directives we can honour in a terminal. Unknown
/// Marp directives (`theme`, `size`, `backgroundImage`, …) are recognised only
/// far enough to swallow them out of the rendered text.
#[derive(Clone, Debug, Default)]
struct Directives {
    header: Option<String>,
    footer: Option<String>,
    paginate: Option<bool>,
    class: Option<String>,
}

impl Directives {
    /// Overlay slide-local directives on top of the carried global state.
    fn merged(&self, local: &Directives) -> Directives {
        Directives {
            header: local.header.clone().or_else(|| self.header.clone()),
            footer: local.footer.clone().or_else(|| self.footer.clone()),
            paginate: local.paginate.or(self.paginate),
            class: local.class.clone().or_else(|| self.class.clone()),
        }
    }

    fn set(&mut self, key: &str, raw_val: &str) {
        let val = unquote(raw_val);
        match key {
            // An empty string clears the directive (Marp's reset convention).
            "header" => self.header = non_empty(val),
            "footer" => self.footer = non_empty(val),
            "class" => self.class = non_empty(val),
            "paginate" => self.paginate = Some(val.eq_ignore_ascii_case("true")),
            _ => {}
        }
    }
}

/// Keys this renderer understands. Others are still consumed from comments (so
/// they don't render as text) but have no display effect.
fn known_directive(key: &str) -> bool {
    matches!(
        key,
        "header"
            | "footer"
            | "paginate"
            | "class"
            | "theme"
            | "size"
            | "color"
            | "backgroundColor"
            | "backgroundImage"
            | "backgroundPosition"
            | "backgroundRepeat"
            | "backgroundSize"
            | "style"
            | "marp"
    )
}

fn non_empty(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}

/// Strip one pair of matching quotes, if present.
fn unquote(s: &str) -> String {
    let s = s.trim();
    let bytes = s.as_bytes();
    if s.len() >= 2
        && ((bytes[0] == b'"' && bytes[s.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[s.len() - 1] == b'\''))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Consume a leading `---` … `---`/`...` front-matter block, applying any
/// recognised directives to `global`, and return the remaining body. Returns
/// `raw` unchanged when there is no front matter.
fn strip_front_matter<'a>(
    raw: &'a str,
    global: &mut Directives,
    first_local: &mut Directives,
) -> &'a str {
    let first_end = raw.find('\n').unwrap_or(raw.len());
    if raw[..first_end].trim_end() != "---" {
        return raw;
    }
    let mut idx = first_end + 1;
    while idx <= raw.len() {
        let end = raw[idx..].find('\n').map(|p| idx + p).unwrap_or(raw.len());
        let line = raw[idx..end].trim();
        if line == "---" || line == "..." {
            let body_start = (end + 1).min(raw.len());
            return &raw[body_start..];
        }
        // A `_`-prefixed key in front matter is a Marp "spot" directive scoped
        // to the first slide; a plain key is global.
        apply_directive_line(line, global, first_local);
        if end >= raw.len() {
            break;
        }
        idx = end + 1;
    }
    // Unterminated front matter: nothing left to show as a body.
    ""
}

/// Split a body into slide chunks on `---` page breaks. Any line that is only
/// dashes (three or more, optionally space-separated) is a break — EXCEPT inside
/// a fenced code block, where a `---` line is code (this is what shattered decks
/// that show a `---` separator inside a ```` ```markdown ```` sample). This is
/// deliberately greedy otherwise: in a Marp deck a lone `---` is a slide
/// separator, so we don't try to disambiguate the rare setext-`##` underline it
/// collides with.
fn split_slides(body: &str) -> Vec<String> {
    let mut slides = Vec::new();
    let mut cur = String::new();
    // `Some((marker, len))` while inside a ``` / ~~~ fence of that width.
    let mut fence: Option<(char, usize)> = None;
    for line in body.lines() {
        match fence {
            Some((marker, len)) => {
                if fence_marker(line).is_some_and(|(m, n)| m == marker && n >= len) {
                    fence = None;
                }
            }
            None => {
                if let Some(open) = fence_marker(line) {
                    fence = Some(open);
                } else if is_page_break(line) {
                    slides.push(std::mem::take(&mut cur));
                    continue;
                }
            }
        }
        cur.push_str(line);
        cur.push('\n');
    }
    slides.push(cur);
    slides
}

/// A code-fence line (``` or ~~~, 3+), returning `(marker, run length)`. The
/// info string after an opening fence is ignored. `None` for non-fence lines.
fn fence_marker(line: &str) -> Option<(char, usize)> {
    let t = line.trim_start();
    for marker in ['`', '~'] {
        let n = t.chars().take_while(|&c| c == marker).count();
        if n >= 3 {
            return Some((marker, n));
        }
    }
    None
}

fn is_page_break(line: &str) -> bool {
    let t = line.trim();
    let dashes = t.chars().filter(|&c| c == '-').count();
    dashes >= 3 && t.chars().all(|c| c == '-' || c == ' ')
}

/// Pull Marp background images (`![bg ...](url)`) out of the slide body, leaving
/// the rest of the Markdown untouched. Returns `(clean_body, backgrounds)`.
fn extract_backgrounds(body: &str) -> (String, Vec<BgImage>) {
    let mut out = String::new();
    let mut bgs = Vec::new();
    let mut rest = body;
    while let Some(pos) = rest.find("![") {
        out.push_str(&rest[..pos]);
        let after = &rest[pos..];
        if let Some((alt, url, consumed)) = parse_image(after)
            && is_bg_alt(alt)
        {
            bgs.push(BgImage {
                url: url.to_string(),
                placement: parse_placement(alt),
            });
            rest = &after[consumed..];
            continue;
        }
        // Not a background image (or unparsable): keep `![` and move past it so
        // an ordinary inline image is preserved.
        out.push_str("![");
        rest = &after[2..];
    }
    out.push_str(rest);
    (out, bgs)
}

/// Parse a `![alt](url …)` starting at `s[0]`. Returns the alt text, the URL
/// (first token inside the parens; a trailing `"title"` is ignored) and the
/// number of bytes consumed. `None` if `s` isn't a complete image.
fn parse_image(s: &str) -> Option<(&str, &str, usize)> {
    let rest = s.strip_prefix("![")?;
    let close = rest.find(']')?;
    let alt = &rest[..close];
    let after = rest[close + 1..].strip_prefix('(')?;
    let end = after.find(')')?;
    let url = after[..end].split_whitespace().next().unwrap_or("");
    // "![" + alt + "]" + "(" + inside + ")"
    let consumed = 2 + close + 1 + 1 + end + 1;
    Some((alt, url, consumed))
}

/// A Marp background image is one whose alt text's first token is `bg`.
fn is_bg_alt(alt: &str) -> bool {
    alt.split_whitespace().next() == Some("bg")
}

/// Read placement from the alt keywords: `left`/`right` (optionally `:NN%`),
/// else a full-slide background. Size keywords (`fit`, `cover`, `80%`) are
/// ignored — the image is fit to its column.
fn parse_placement(alt: &str) -> BgPlacement {
    let mut placement = BgPlacement::Full;
    for tok in alt.split_whitespace().skip(1) {
        let (key, val) = tok.split_once(':').unwrap_or((tok, ""));
        match key {
            "left" => placement = BgPlacement::Left(parse_fraction(val)),
            "right" => placement = BgPlacement::Right(parse_fraction(val)),
            _ => {}
        }
    }
    placement
}

/// A `NN%` share as a 0..1 fraction, clamped to a sane band. Empty → half.
fn parse_fraction(val: &str) -> f32 {
    let v = val.trim().trim_end_matches('%');
    if v.is_empty() {
        return 0.5;
    }
    v.parse::<f32>()
        .map(|p| (p / 100.0).clamp(0.1, 0.9))
        .unwrap_or(0.5)
}

/// Remove every `<!-- ... -->` comment from `chunk`, routing directive lines
/// inside them to `global`/`local`. Returns the comment-free Markdown.
fn strip_comments(chunk: &str, global: &mut Directives, local: &mut Directives) -> String {
    let mut out = String::new();
    let mut rest = chunk;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 4..];
        match after.find("-->") {
            Some(end) => {
                parse_comment(&after[..end], global, local);
                rest = &after[end + 3..];
            }
            None => {
                // Unterminated comment: drop to end of chunk.
                parse_comment(after, global, local);
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Interpret the inside of a comment: recognised `key: value` lines become
/// directives, everything else is a presenter note and is discarded.
fn parse_comment(inner: &str, global: &mut Directives, local: &mut Directives) {
    for line in inner.lines() {
        apply_directive_line(line.trim(), global, local);
    }
}

/// Apply a single `key: value` directive line. A `_`-prefixed key is slide-local
/// (routed to `local`); anything else is global. Unrecognised keys are ignored.
fn apply_directive_line(line: &str, global: &mut Directives, local: &mut Directives) {
    let Some((k, v)) = line.split_once(':') else {
        return;
    };
    let k = k.trim();
    let (target, key): (&mut Directives, &str) = match k.strip_prefix('_') {
        Some(rest) => (local, rest),
        None => (global, k),
    };
    if known_directive(key) {
        target.set(key, v.trim());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_front_matter() {
        assert!(detect("---\nmarp: true\n---\n# Hi\n"));
        assert!(detect(
            "---\ntheme: gaia\nmarp: true\npaginate: true\n---\n# Hi"
        ));
        assert!(!detect("---\ntitle: Not a deck\n---\n# Hi"));
        assert!(!detect("# Just markdown\n\nno front matter"));
        assert!(!detect("---\nmarp: false\n---\n# Hi"));
    }

    #[test]
    fn detects_leading_comment() {
        assert!(detect("<!-- marp: true -->\n# Hi"));
        assert!(detect("\n\n<!-- marp: true -->\n# Hi"));
        // A marp comment that comes after real content does not enable.
        assert!(!detect("# Heading first\n\n<!-- marp: true -->"));
    }

    #[test]
    fn splits_on_page_breaks() {
        let deck = parse("---\nmarp: true\n---\n# One\n\n---\n\n# Two\n\n---\n\n# Three\n");
        assert_eq!(deck.len(), 3);
        assert_eq!(deck.slides[0].body, "# One");
        assert_eq!(deck.slides[1].body, "# Two");
        assert_eq!(deck.slides[2].body, "# Three");
    }

    #[test]
    fn strips_comments_and_notes() {
        let deck = parse("---\nmarp: true\n---\n# Slide\n\n<!-- a presenter note -->\nbody\n");
        assert_eq!(deck.len(), 1);
        assert!(!deck.slides[0].body.contains("presenter note"));
        assert!(deck.slides[0].body.contains("body"));
    }

    #[test]
    fn global_directives_carry_forward() {
        let deck =
            parse("---\nmarp: true\npaginate: true\nfooter: Corp\n---\n# One\n\n---\n\n# Two\n");
        assert!(deck.slides[0].paginate);
        assert!(deck.slides[1].paginate);
        assert_eq!(deck.slides[1].footer.as_deref(), Some("Corp"));
    }

    #[test]
    fn page_break_inside_code_fence_is_not_a_split() {
        // The canonical Marp starter deck: slide 2 contains a fenced markdown
        // sample whose `---` must NOT create extra slides.
        let src = "---\nmarp: true\n---\n# One\n\n---\n\n# Two\n\n```markdown\n# Slide 1\n\n---\n\n# Slide 2\n```\n";
        let deck = parse(src);
        assert_eq!(deck.len(), 2, "code-fence `---` must not split the deck");
        assert!(deck.slides[1].body.contains("```markdown"));
        assert!(deck.slides[1].body.contains("# Slide 2"));
    }

    #[test]
    fn front_matter_spot_directive_scopes_to_first_slide() {
        // `_class: lead` in the front matter centers only slide 1.
        let deck = parse("---\nmarp: true\n_class: lead\n---\n# Cover\n\n---\n\n# Body\n");
        assert!(deck.slides[0].lead, "front-matter `_class` centers slide 1");
        assert!(!deck.slides[1].lead, "and no other slide");
    }

    #[test]
    fn local_class_lead_is_scoped() {
        let deck = parse("---\nmarp: true\n---\n<!-- _class: lead -->\n# Title\n\n---\n\n# Body\n");
        assert!(deck.slides[0].lead);
        assert!(!deck.slides[1].lead);
    }

    #[test]
    fn local_paginate_overrides_global() {
        let deck = parse(
            "---\nmarp: true\npaginate: true\n---\n<!-- _paginate: false -->\n# Cover\n\n---\n\n# Content\n",
        );
        assert!(!deck.slides[0].paginate);
        assert!(deck.slides[1].paginate);
    }

    #[test]
    fn comment_only_chunk_is_not_a_slide() {
        let deck = parse("---\nmarp: true\n---\n# Real\n\n---\n\n<!-- just: notes -->\n");
        assert_eq!(deck.len(), 1);
        assert_eq!(deck.slides[0].body, "# Real");
    }

    #[test]
    fn empty_document_yields_one_slide() {
        let deck = parse("---\nmarp: true\n---\n");
        assert_eq!(deck.len(), 1);
    }

    #[test]
    fn background_image_is_extracted_with_placement() {
        let deck = parse(
            "---\nmarp: true\n---\n![bg left:40% 80%](https://marp.app/assets/marp.svg)\n\n# Marp\n\ntagline\n",
        );
        let s = &deck.slides[0];
        // The bg image is pulled out of the flow…
        assert!(
            !s.body.contains("marp.svg"),
            "bg image left in body: {:?}",
            s.body
        );
        assert!(s.body.contains("# Marp"));
        // …and recorded as a left 40% background.
        assert_eq!(s.background.len(), 1);
        assert_eq!(s.background[0].url, "https://marp.app/assets/marp.svg");
        assert_eq!(s.background[0].placement, BgPlacement::Left(0.4));
    }

    #[test]
    fn plain_bg_defaults_to_full_and_ordinary_images_stay() {
        let deck = parse("---\nmarp: true\n---\n![bg](bgpic.png)\n\n![logo](logo.png)\n");
        let s = &deck.slides[0];
        assert_eq!(s.background.len(), 1);
        assert_eq!(s.background[0].placement, BgPlacement::Full);
        // A non-`bg` image is not touched.
        assert!(s.body.contains("![logo](logo.png)"));
    }

    #[test]
    fn split_widths_reserve_the_right_columns() {
        let deck = parse("---\nmarp: true\n---\n![bg right:30%](x.png)\n\n# Hi\n");
        let (left, right) = deck.slides[0].split_widths(100);
        assert_eq!(left, 0);
        assert_eq!(right, 30);
    }

    #[test]
    fn footer_quotes_are_stripped() {
        let deck = parse("---\nmarp: true\nfooter: \"© 2026\"\n---\n# Hi\n");
        assert_eq!(deck.slides[0].footer.as_deref(), Some("© 2026"));
    }
}

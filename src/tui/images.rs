//! Asynchronous image loader for the reader / presentation image overlays.
//!
//! The reader draws images with the terminal graphics protocol (kitty / iTerm2 /
//! sixel) via `ratatui-image`. Building a protocol needs a decoded bitmap, and
//! obtaining one can block: a remote `https://…` asset has to be downloaded, and
//! an SVG has to be rasterized. Doing either on the render thread would stall the
//! UI, so each image is loaded on a background thread and the decoded bitmap is
//! sent back over a channel. The main loop folds finished bitmaps into ready
//! protocols once per tick (see `App::drain_images`).
//!
//! Sources are keyed by their string form — a local filesystem path or a remote
//! URL — so the same asset referenced twice loads once.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};

use image::DynamicImage;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;

/// A finished background load: the decoded bitmap, or `None` on any failure
/// (network error, unsupported format, corrupt data).
struct Loaded {
    key: String,
    image: Option<DynamicImage>,
}

/// Caches decoded image protocols and tracks in-flight / failed loads.
pub struct ImageStore {
    ready: HashMap<String, StatefulProtocol>,
    pending: HashSet<String>,
    failed: HashSet<String>,
    tx: Sender<Loaded>,
    rx: Receiver<Loaded>,
}

impl Default for ImageStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageStore {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            ready: HashMap::new(),
            pending: HashSet::new(),
            failed: HashSet::new(),
            tx,
            rx,
        }
    }

    /// Kick off a background load for `key` unless it's already ready, in
    /// flight, or known to have failed. Idempotent — safe to call every frame.
    pub fn request(&mut self, key: &str) {
        if self.ready.contains_key(key) || self.pending.contains(key) || self.failed.contains(key) {
            return;
        }
        self.pending.insert(key.to_string());
        let tx = self.tx.clone();
        let key = key.to_string();
        std::thread::spawn(move || {
            let image = load(&key);
            // The receiver only goes away on shutdown; ignore a send error.
            let _ = tx.send(Loaded { key, image });
        });
    }

    /// Fold every finished load into a ready protocol (or the failed set).
    /// Needs the terminal's `Picker`, which must stay on the main thread.
    pub fn drain(&mut self, picker: &mut Picker) {
        while let Ok(msg) = self.rx.try_recv() {
            self.pending.remove(&msg.key);
            match msg.image {
                Some(img) => {
                    self.ready.insert(msg.key, picker.new_resize_protocol(img));
                }
                None => {
                    self.failed.insert(msg.key);
                }
            }
        }
    }

    /// The ready protocol for `key`, if its load has completed.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut StatefulProtocol> {
        self.ready.get_mut(key)
    }
}

/// Load and decode one image source (blocking — runs on a worker thread).
fn load(key: &str) -> Option<DynamicImage> {
    let bytes = if is_remote(key) {
        download(key)?
    } else {
        std::fs::read(key).ok()?
    };
    if looks_like_svg(&bytes, key) {
        rasterize_svg(&bytes)
    } else {
        image::load_from_memory(&bytes).ok()
    }
}

fn is_remote(key: &str) -> bool {
    key.starts_with("http://") || key.starts_with("https://")
}

/// Download `url` into memory. Builds a throwaway current-thread runtime so we
/// don't depend on the cloud runtime being connected (images render offline of
/// HackMD too).
fn download(url: &str) -> Option<Vec<u8>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    rt.block_on(async {
        let resp = reqwest::get(url).await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.bytes().await.ok().map(|b| b.to_vec())
    })
}

/// SVG detection: by `.svg` extension, else a sniff of the leading bytes.
fn looks_like_svg(bytes: &[u8], key: &str) -> bool {
    if key
        .rsplit('.')
        .next()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
    {
        return true;
    }
    let head = &bytes[..bytes.len().min(512)];
    let text = String::from_utf8_lossy(head);
    text.contains("<svg")
}

/// Rasterize an SVG to an opaque bitmap. The canvas is filled white first, so
/// transparent regions become white (matching Marp's default white slides) and
/// the premultiplied-alpha output is straight RGBA for every pixel — avoiding
/// the black fringing a naive copy of premultiplied data would show.
fn rasterize_svg(bytes: &[u8]) -> Option<DynamicImage> {
    use resvg::{tiny_skia, usvg};

    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_data(bytes, &opt).ok()?;
    let size = tree.size().to_int_size();

    // Cap the raster so an SVG with a huge viewBox can't allocate wildly; the
    // terminal downsizes it to a dozen rows anyway.
    const MAX_DIM: u32 = 1024;
    let longest = size.width().max(size.height()).max(1);
    let scale = if longest > MAX_DIM {
        MAX_DIM as f32 / longest as f32
    } else {
        1.0
    };
    let w = ((size.width() as f32) * scale).round().max(1.0) as u32;
    let h = ((size.height() as f32) * scale).round().max(1.0) as u32;

    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    pixmap.fill(tiny_skia::Color::WHITE);
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let rgba = image::RgbaImage::from_raw(w, h, pixmap.take())?;
    Some(DynamicImage::ImageRgba8(rgba))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;

    #[test]
    fn remote_urls_are_detected() {
        assert!(is_remote("https://marp.app/assets/marp.svg"));
        assert!(is_remote("http://example.com/x.png"));
        assert!(!is_remote("/local/path.png"));
        assert!(!is_remote("assets/logo.svg"));
    }

    #[test]
    fn svg_is_detected_by_extension_or_content() {
        assert!(looks_like_svg(b"", "logo.SVG"));
        assert!(looks_like_svg(b"<?xml ...?>\n<svg xmlns=...>", "noext"));
        assert!(!looks_like_svg(b"\x89PNG\r\n", "photo.png"));
    }

    #[test]
    fn svg_rasterizes_to_expected_size() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="red"/></svg>"#;
        let img = rasterize_svg(svg).expect("valid SVG rasterizes");
        assert_eq!(img.dimensions(), (20, 10));
        // White-filled canvas → opaque output (no transparent fringing).
        assert_eq!(img.get_pixel(0, 0).0[3], 255);
    }

    #[test]
    fn garbage_svg_fails_cleanly() {
        assert!(rasterize_svg(b"not svg at all").is_none());
    }

    // Network-gated: the full remote path (download + SVG rasterize) against the
    // real Marp asset. Ignored by default so offline / CI runs stay green; run
    // with `cargo test --features tui -- --ignored end_to_end`.
    #[test]
    #[ignore]
    fn end_to_end_remote_svg_download() {
        let img = load("https://marp.app/assets/marp.svg").expect("download + rasterize");
        assert!(img.width() > 0 && img.height() > 0);
    }
}

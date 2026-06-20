//! Image rendering, caching, and link-region tracking.
//!
//! Cross-cuts every supported terminal image protocol (`image_protocol`):
//! Kitty graphics (`kitty_*`), iTerm2 inline (`iterm2_crop_cache`),
//! Sixel (`sixel_*`), and a Unicode halfblock fallback. Caches resized
//! PNGs (`native_image_cache`), tracks frame-to-frame visibility for
//! redraw skipping (`prev_visible_images`), and routes background
//! decode work through `image_render_tx` / `image_render_rx`. Also
//! holds the `LinkRegion` list and `link_url_map` used by the
//! post-render OSC 8 hyperlink injector.

use std::collections::{HashMap, HashSet};

use std::sync::mpsc;

use ratatui::style::Color;
use ratatui::text::Line;

pub use crate::config::ImageMode;

use crate::image_render::ImageProtocol;

/// An image visible on screen, for native protocol overlay rendering.
#[derive(PartialEq, Eq)]
pub struct VisibleImage {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    /// Total image height in cells (before viewport clipping).
    pub full_height: u16,
    /// Cells cropped from the top when the image is partially scrolled out.
    pub crop_top: u16,
    pub path: String,
}

/// Result from a background image render task.
pub struct ImageRenderResult {
    pub conv_id: String,
    pub timestamp_ms: i64,
    pub is_preview: bool,
    pub lines: Option<Vec<Line<'static>>>,
    pub image_path: Option<String>,
    /// Pre-encoded PNG for native_image_cache: (path, base64, pixel_w, pixel_h)
    pub pre_native_png: Option<(String, String, u32, u32)>,
    /// Pre-encoded full Sixel for sixel_cache: (path, sixel_string)
    pub pre_sixel: Option<(String, String)>,
}

/// A clickable link region detected in the rendered buffer.
pub struct LinkRegion {
    pub x: u16,
    pub y: u16,
    pub url: String,
    pub text: String,
    /// Display width in terminal columns (may differ from text.len() for Unicode).
    pub width: u16,
    /// Background color from the buffer cell, if non-default (e.g. highlight).
    pub bg: Option<Color>,
}

/// State for image rendering, caching, and link overlay tracking.
pub struct ImageState {
    /// Image display mode
    pub image_mode: ImageMode,
    /// Show link previews (title, description, thumbnail) for URLs
    pub show_link_previews: bool,
    /// Link regions detected in the last rendered frame
    pub link_regions: Vec<LinkRegion>,
    /// Maps display text to hidden URL for attachment links
    pub link_url_map: HashMap<String, String>,
    /// Detected terminal image protocol (Kitty, iTerm2, Sixel, or Halfblock)
    pub image_protocol: ImageProtocol,
    /// Cell pixel dimensions (width, height) for Sixel encoding
    pub cell_px: (u16, u16),
    /// Images visible on screen for native protocol overlay (cleared each frame)
    pub visible_images: Vec<VisibleImage>,
    /// Previous scroll offset for Sixel stale pixel detection
    pub sixel_prev_scroll: usize,
    /// Previous frame's visible images, for skipping redundant image redraws
    pub prev_visible_images: Vec<VisibleImage>,
    /// Cache of pre-resized PNGs for native protocol
    pub native_image_cache: HashMap<String, (String, u32, u32)>,
    /// Next Kitty image ID to assign
    pub next_kitty_image_id: u32,
    /// Map from image path to Kitty image ID
    pub kitty_image_ids: HashMap<String, u32>,
    /// Set of image IDs already transmitted to the terminal
    pub kitty_transmitted: HashSet<u32>,
    /// Images to transmit this frame
    pub kitty_pending_transmits: Vec<(u32, String, u16, u16)>,
    /// Cache of cropped image base64 for iTerm2
    pub iterm2_crop_cache: HashMap<(String, u16, u16), String>,
    /// Cache of full Sixel-encoded images
    pub sixel_cache: HashMap<String, String>,
    /// Background image render channel (sender)
    pub image_render_tx: mpsc::Sender<ImageRenderResult>,
    /// Background image render channel (receiver)
    pub image_render_rx: mpsc::Receiver<ImageRenderResult>,
    /// In-flight background renders: (conv_id, timestamp, is_preview)
    pub image_render_in_flight: HashSet<(String, i64, bool)>,
    /// Cached inputs of the last viewport scan in `ensure_active_images`:
    /// (conv_id, scroll_offset, message_count, image_mode, show_link_previews).
    /// When unchanged and no render just completed, the per-tick scan is
    /// skipped to avoid idle CPU churn (#492).
    pub scan_signature: Option<(String, usize, usize, ImageMode, bool)>,
}

impl ImageState {
    /// Create a new ImageState with the given render channels.
    pub fn new(
        image_render_tx: mpsc::Sender<ImageRenderResult>,
        image_render_rx: mpsc::Receiver<ImageRenderResult>,
    ) -> Self {
        use crate::image_render;
        Self {
            image_mode: ImageMode::Halfblock,
            show_link_previews: true,
            link_regions: Vec::new(),
            link_url_map: HashMap::new(),
            image_protocol: image_render::detect_protocol(),
            cell_px: image_render::detect_cell_pixel_size(),
            visible_images: Vec::new(),
            sixel_prev_scroll: 0,
            prev_visible_images: Vec::new(),
            native_image_cache: HashMap::new(),
            next_kitty_image_id: 1,
            kitty_image_ids: HashMap::new(),
            kitty_transmitted: HashSet::new(),
            kitty_pending_transmits: Vec::new(),
            iterm2_crop_cache: HashMap::new(),
            sixel_cache: HashMap::new(),
            image_render_tx,
            image_render_rx,
            image_render_in_flight: HashSet::new(),
            scan_signature: None,
        }
    }

    /// Bound the encode caches so they can't grow without limit (#492). Called
    /// once per frame. At these caps anything evicted is off-screen and
    /// regenerates lazily via the background render path, so a crude cap (rather
    /// than true LRU) is enough and avoids per-insert bookkeeping. Sixel strings
    /// are large (0.5-3MB each) so that cache gets a smaller entry budget.
    pub fn enforce_cache_caps(&mut self) {
        const NATIVE_CACHE_CAP: usize = 256;
        const ITERM2_CACHE_CAP: usize = 256;
        const SIXEL_CACHE_CAP: usize = 64;
        cap_map(&mut self.native_image_cache, NATIVE_CACHE_CAP);
        cap_map(&mut self.iterm2_crop_cache, ITERM2_CACHE_CAP);
        cap_map(&mut self.sixel_cache, SIXEL_CACHE_CAP);
    }
}

/// Drop arbitrary entries from `map` until it holds at most `cap`. A no-op when
/// already within budget. Eviction order is unspecified (HashMap iteration); at
/// the call sites' caps the working set is far smaller than `cap`, so only stale
/// off-screen entries are ever removed.
fn cap_map<K: Eq + std::hash::Hash + Clone, V>(map: &mut HashMap<K, V>, cap: usize) {
    if map.len() <= cap {
        return;
    }
    let excess = map.len() - cap;
    let victims: Vec<K> = map.keys().take(excess).cloned().collect();
    for k in victims {
        map.remove(&k);
    }
}

#[cfg(test)]
mod tests {
    use super::cap_map;
    use std::collections::HashMap;

    #[test]
    fn cap_map_is_noop_within_budget() {
        let mut m: HashMap<u32, u32> = (0..5).map(|i| (i, i)).collect();
        cap_map(&mut m, 10);
        assert_eq!(m.len(), 5);
    }

    #[test]
    fn cap_map_trims_to_cap() {
        let mut m: HashMap<u32, u32> = (0..100).map(|i| (i, i)).collect();
        cap_map(&mut m, 16);
        assert_eq!(m.len(), 16);
    }

    #[test]
    fn cap_map_exact_cap_unchanged() {
        let mut m: HashMap<u32, u32> = (0..8).map(|i| (i, i)).collect();
        cap_map(&mut m, 8);
        assert_eq!(m.len(), 8);
    }
}

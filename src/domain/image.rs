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

use crate::app::{ImageRenderResult, VisibleImage};
use crate::image_render::ImageProtocol;
use crate::ui::LinkRegion;

/// State for image rendering, caching, and link overlay tracking.
pub struct ImageState {
    /// Image display mode: "native", "halfblock", or "none"
    pub image_mode: String,
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
}

impl ImageState {
    /// Create a new ImageState with the given render channels.
    pub fn new(
        image_render_tx: mpsc::Sender<ImageRenderResult>,
        image_render_rx: mpsc::Receiver<ImageRenderResult>,
    ) -> Self {
        use crate::image_render;
        Self {
            image_mode: "halfblock".to_string(),
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
        }
    }
}

use std::io::Cursor;
use std::path::Path;

use image::GenericImageView;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

/// Terminal image display protocol.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImageProtocol {
    /// Kitty Graphics Protocol (Kitty, Ghostty)
    Kitty,
    /// iTerm2 Inline Images Protocol (iTerm2, WezTerm)
    Iterm2,
    /// Unicode halfblock fallback (universal)
    Halfblock,
}

/// Detect the best available image protocol by checking environment variables.
pub fn detect_protocol() -> ImageProtocol {
    if std::env::var("KITTY_WINDOW_ID").is_ok() {
        return ImageProtocol::Kitty;
    }
    if let Ok(term) = std::env::var("TERM_PROGRAM") {
        match term.as_str() {
            "ghostty" => return ImageProtocol::Kitty,
            "iTerm.app" | "WezTerm" => return ImageProtocol::Iterm2,
            _ => {}
        }
    }
    ImageProtocol::Halfblock
}

/// Pre-resize an image and encode as PNG for native terminal protocol rendering.
///
/// Returns `(base64_data, pixel_width, pixel_height)` sized to look good at the
/// given cell dimensions. Assumes ~8px per cell width and ~16px per cell height.
pub fn encode_native_png(path: &Path, cell_width: u32, cell_height: u32) -> Option<(String, u32, u32)> {
    let img = image::open(path).ok()?;
    let (orig_w, orig_h) = img.dimensions();
    if orig_w == 0 || orig_h == 0 {
        return None;
    }

    // Target pixel dimensions based on typical cell size
    let target_w = cell_width * 8;
    let target_h = cell_height * 16;

    let scale = f64::min(
        target_w as f64 / orig_w as f64,
        target_h as f64 / orig_h as f64,
    )
    .min(1.0);

    let new_w = ((orig_w as f64 * scale).round() as u32).max(1);
    let new_h = ((orig_h as f64 * scale).round() as u32).max(1);

    let resized = img.resize_exact(new_w, new_h, image::imageops::FilterType::Triangle);

    let mut buf = Cursor::new(Vec::new());
    resized
        .write_to(&mut buf, image::ImageFormat::Png)
        .ok()?;

    use base64::Engine;
    Some((base64::engine::general_purpose::STANDARD.encode(buf.into_inner()), new_w, new_h))
}

/// Render an image file as halfblock-character lines for display in a terminal.
///
/// Each terminal cell represents two vertical pixels using the upper-half-block
/// character (▀) with the top pixel as foreground and bottom pixel as background.
///
/// Returns `None` if the image cannot be loaded or decoded.
pub fn render_image(path: &Path, max_width: u32) -> Option<Vec<Line<'static>>> {
    let img = image::open(path).ok()?;

    let cap_width = max_width;
    let cap_height: u32 = 60; // 30 cell-rows × 2 pixels per row

    let (orig_w, orig_h) = img.dimensions();
    if orig_w == 0 || orig_h == 0 {
        return None;
    }

    // Calculate target size preserving aspect ratio
    let scale = f64::min(
        cap_width as f64 / orig_w as f64,
        cap_height as f64 / orig_h as f64,
    )
    .min(1.0); // never upscale

    let new_w = ((orig_w as f64 * scale).round() as u32).max(1);
    let new_h = ((orig_h as f64 * scale).round() as u32).max(1);

    let resized = img.resize_exact(new_w, new_h, image::imageops::FilterType::Triangle);
    let rgba = resized.to_rgba8();

    let (w, h) = rgba.dimensions();
    // Process pixel rows in pairs (top/bottom per cell row)
    let row_pairs = h.div_ceil(2);

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(row_pairs as usize);

    for row in 0..row_pairs {
        let y_top = row * 2;
        let y_bot = y_top + 1;

        let mut spans: Vec<Span<'static>> = Vec::with_capacity(w as usize + 1);
        // 2-space indent for visual separation
        spans.push(Span::raw("  "));

        for x in 0..w {
            let top_pixel = rgba.get_pixel(x, y_top);
            let fg = if top_pixel[3] < 128 {
                Color::Reset
            } else {
                Color::Rgb(top_pixel[0], top_pixel[1], top_pixel[2])
            };

            let bg = if y_bot < h {
                let bot_pixel = rgba.get_pixel(x, y_bot);
                if bot_pixel[3] < 128 {
                    Color::Reset
                } else {
                    Color::Rgb(bot_pixel[0], bot_pixel[1], bot_pixel[2])
                }
            } else {
                Color::Reset
            };

            spans.push(Span::styled(
                "▀",
                Style::default().fg(fg).bg(bg),
            ));
        }

        lines.push(Line::from(spans));
    }

    Some(lines)
}

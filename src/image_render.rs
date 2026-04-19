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
    /// Sixel (Windows Terminal, foot, mlterm, xterm)
    Sixel,
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
    if std::env::var("WT_SESSION").is_ok() {
        return ImageProtocol::Sixel;
    }
    ImageProtocol::Halfblock
}

/// Pre-resize an image and encode as PNG for native terminal protocol rendering.
///
/// Returns `(base64_data, pixel_width, pixel_height)` sized to look good at the
/// given cell dimensions. Assumes ~8px per cell width and ~16px per cell height.
pub fn encode_native_png(
    path: &Path,
    cell_width: u32,
    cell_height: u32,
) -> Option<(String, u32, u32)> {
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
    resized.write_to(&mut buf, image::ImageFormat::Png).ok()?;

    use base64::Engine;
    Some((
        base64::engine::general_purpose::STANDARD.encode(buf.into_inner()),
        new_w,
        new_h,
    ))
}

/// Detect the pixel dimensions of a single terminal cell.
///
/// Tries `crossterm::terminal::window_size()` (pixel dimensions ÷ cell grid).
/// Falls back to a platform-specific default: 10×20 for Windows Terminal
/// (typical Cascadia Code at default DPI), 8×16 elsewhere.
pub fn detect_cell_pixel_size() -> (u16, u16) {
    if let Ok(ws) = crossterm::terminal::window_size() {
        if ws.width > 0 && ws.height > 0 && ws.columns > 0 && ws.rows > 0 {
            return (ws.width / ws.columns, ws.height / ws.rows);
        }
    }
    if std::env::var("WT_SESSION").is_ok() {
        (10, 20) // Windows Terminal with Cascadia Code at typical DPI
    } else {
        (8, 16)
    }
}

/// Encode a pre-sized image as a Sixel DCS string (CPU-bound).
///
/// Decodes the base64 PNG, resizes to fill the target cell area, and
/// Sixel-encodes.  Designed to run on a blocking thread via `spawn_blocking`.
pub fn encode_sixel(
    b64_png: &str,
    width_cells: u16,
    height_cells: u16,
    cell_px: (u16, u16),
) -> Option<String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64_png)
        .ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }

    let target_w = (width_cells as u32 * cell_px.0 as u32).max(1);
    let target_h = (height_cells as u32 * cell_px.1 as u32).max(1);

    let scale = f64::min(target_w as f64 / w as f64, target_h as f64 / h as f64);
    let new_w = ((w as f64 * scale).round() as u32).max(1);
    let new_h = ((h as f64 * scale).round() as u32).max(1);

    let resized = img.resize_exact(new_w, new_h, image::imageops::FilterType::Triangle);
    let rgba = resized.to_rgba8().into_raw();

    icy_sixel::sixel_encode(
        &rgba,
        new_w as usize,
        new_h as usize,
        &icy_sixel::EncodeOptions::default(),
    )
    .ok()
}

/// Slice a cached full-size Sixel DCS string to extract only the visible bands.
///
/// This is an instant string operation - no image decoding or re-encoding.
/// Each Sixel band represents 6 pixel rows.  We compute which bands correspond
/// to the visible cell range and reconstruct the DCS with just those bands.
///
/// Returns `None` if the Sixel string cannot be parsed or the crop is empty.
pub fn slice_sixel_bands(
    full_sixel: &str,
    cell_px_h: u16,
    _full_height_cells: u16,
    crop_top_cells: u16,
    visible_height_cells: u16,
) -> Option<String> {
    // Find DCS body boundaries: ESC P [params] q [body] ESC \
    let body_start = full_sixel.find('q')? + 1;
    let body_end = full_sixel.rfind("\x1b\\")?;
    if body_start >= body_end {
        return None;
    }

    let dcs_header = &full_sixel[..body_start]; // includes "q"
    let body = &full_sixel[body_start..body_end];

    // Separate preamble (raster attrs + color definitions) from pixel data.
    let preamble_end = find_preamble_end(body);
    let preamble = &body[..preamble_end];
    let pixel_data = &body[preamble_end..];

    // Split pixel data into bands (separated by '-', the Graphics New Line).
    // Filter empty elements: icy_sixel adds a trailing '-' after the last
    // band, which creates a spurious empty element in the split.
    let bands: Vec<&str> = pixel_data.split('-').filter(|b| !b.is_empty()).collect();
    let total_bands = bands.len();
    if total_bands == 0 {
        return None;
    }

    // Calculate which bands correspond to the visible region.
    // Use FLOOR division for take_bands to prevent overflow into adjacent
    // cells. icy_sixel rounds up to a multiple of 6 pixel rows, so the
    // cached Sixel often has 1 extra band. Clipping it prevents artifacts.
    let skip_px = crop_top_cells as usize * cell_px_h as usize;
    let vis_px = visible_height_cells as usize * cell_px_h as usize;

    let skip_bands = skip_px / 6;
    let take_bands = vis_px / 6; // floor - never overflow cell area

    let start = skip_bands.min(total_bands);
    let end = (start + take_bands).min(total_bands);

    if start >= end {
        return None;
    }

    // Reconstruct the DCS, stripping raster attributes. The raster declares
    // image dimensions ("Pan;Pad;Ph;Pv) which WT uses for cursor positioning.
    // After slicing, these dimensions are wrong and can corrupt WT's state.
    // Raster attributes are optional per the Sixel spec - terminals determine
    // dimensions from the actual pixel data instead.
    let mut result = String::with_capacity(full_sixel.len());
    result.push_str(dcs_header);

    // Skip raster attributes, keep only color definitions
    if let Some(raster_content) = preamble.strip_prefix('"') {
        let raster_len = raster_content
            .find(|c: char| !c.is_ascii_digit() && c != ';')
            .unwrap_or(raster_content.len());
        // Everything after the raster digits is color definitions
        result.push_str(&raster_content[raster_len..]);
    } else {
        result.push_str(preamble);
    }

    for (i, band) in bands[start..end].iter().enumerate() {
        if i > 0 {
            result.push('-');
        }
        result.push_str(band);
    }
    result.push_str("\x1b\\");

    Some(result)
}

/// Find where the preamble (raster attributes + color definitions) ends
/// and actual pixel data begins in a Sixel body.
///
/// Raster attributes: `"Pan;Pad;Ph;Pv`
/// Color definitions: `#Pc;Pu;Px;Py;Pz` (semicolon after color number)
/// Color selection (pixel data): `#Pc` followed by pixel data chars (no semicolon)
fn find_preamble_end(body: &str) -> usize {
    let bytes = body.as_bytes();
    let mut i = 0;

    // Skip optional raster attributes: "digits;digits;...
    if i < bytes.len() && bytes[i] == b'"' {
        i += 1;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b';') {
            i += 1;
        }
    }

    // Skip color definitions: #Pc;Pu;Px;Py;Pz (has ; after color number)
    while i < bytes.len() && bytes[i] == b'#' {
        let mark = i;
        i += 1;
        // Read color number (Pc)
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b';' {
            // Color definition - skip through it
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b';') {
                i += 1;
            }
        } else {
            // Color selection (part of pixel data) - revert
            return mark;
        }
    }

    i
}

/// Crop and re-encode a cached full-size PNG for partial display.
///
/// Given the base64-encoded full image and its pixel height, returns a new
/// base64 PNG cropped to the visible vertical slice. Used by iTerm2 which
/// has no native source-crop parameter.
pub fn crop_png_vertical(
    b64_full: &str,
    px_h: u32,
    full_height_cells: u16,
    crop_top_cells: u16,
    visible_height_cells: u16,
) -> Option<String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64_full)
        .ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let (w, _) = img.dimensions();

    let y_px = if full_height_cells > 0 {
        crop_top_cells as u32 * px_h / full_height_cells as u32
    } else {
        0
    };
    let h_px = if full_height_cells > 0 {
        (visible_height_cells as u32 * px_h / full_height_cells as u32).max(1)
    } else {
        px_h
    };
    let h_px = h_px.min(px_h.saturating_sub(y_px));

    let cropped = img.crop_imm(0, y_px, w, h_px);

    let mut buf = Cursor::new(Vec::new());
    cropped.write_to(&mut buf, image::ImageFormat::Png).ok()?;
    Some(base64::engine::general_purpose::STANDARD.encode(buf.into_inner()))
}

/// 256 combining diacritics for encoding row/column values 0-255 per the Kitty spec.
/// Source: https://sw.kovidgoyal.net/kitty/_downloads/f0a0de9ec8d9ff4456206db8e0814937/rowcolumn-diacritics.txt
const DIACRITICS: [char; 256] = [
    '\u{0305}', '\u{030D}', '\u{030E}', '\u{0310}', '\u{0312}', // 0-4
    '\u{033D}', '\u{033E}', '\u{033F}', '\u{0346}', '\u{034A}', // 5-9
    '\u{034B}', '\u{034C}', '\u{0350}', '\u{0351}', '\u{0352}', // 10-14
    '\u{0357}', '\u{035B}', '\u{0363}', '\u{0364}', '\u{0365}', // 15-19
    '\u{0366}', '\u{0367}', '\u{0368}', '\u{0369}', '\u{036A}', // 20-24
    '\u{036B}', '\u{036C}', '\u{036D}', '\u{036E}', '\u{036F}', // 25-29
    '\u{0483}', '\u{0484}', '\u{0485}', '\u{0486}', '\u{0487}', // 30-34
    '\u{0592}', '\u{0593}', '\u{0594}', '\u{0595}', '\u{0597}', // 35-39
    '\u{0598}', '\u{0599}', '\u{059C}', '\u{059D}', '\u{059E}', // 40-44
    '\u{059F}', '\u{05A0}', '\u{05A1}', '\u{05A8}', '\u{05A9}', // 45-49
    '\u{05AB}', '\u{05AC}', '\u{05AF}', '\u{05C4}', '\u{0610}', // 50-54
    '\u{0611}', '\u{0612}', '\u{0613}', '\u{0614}', '\u{0615}', // 55-59
    '\u{0616}', '\u{0617}', '\u{0657}', '\u{0658}', '\u{0659}', // 60-64
    '\u{065A}', '\u{065B}', '\u{065D}', '\u{065E}', '\u{06D6}', // 65-69
    '\u{06D7}', '\u{06D8}', '\u{06D9}', '\u{06DA}', '\u{06DB}', // 70-74
    '\u{06DC}', '\u{06DF}', '\u{06E0}', '\u{06E1}', '\u{06E2}', // 75-79
    '\u{06E4}', '\u{06E7}', '\u{06E8}', '\u{06EB}', '\u{06EC}', // 80-84
    '\u{0730}', '\u{0732}', '\u{0733}', '\u{0735}', '\u{0736}', // 85-89
    '\u{073A}', '\u{073D}', '\u{073F}', '\u{0740}', '\u{0741}', // 90-94
    '\u{0743}', '\u{0745}', '\u{0747}', '\u{0749}', '\u{074A}', // 95-99
    '\u{07EB}', '\u{07EC}', '\u{07ED}', '\u{07EE}', '\u{07EF}', // 100-104
    '\u{07F0}', '\u{07F1}', '\u{07F3}', '\u{0816}', '\u{0817}', // 105-109
    '\u{0818}', '\u{0819}', '\u{081B}', '\u{081C}', '\u{081D}', // 110-114
    '\u{081E}', '\u{081F}', '\u{0820}', '\u{0821}', '\u{0822}', // 115-119
    '\u{0823}', '\u{0825}', '\u{0826}', '\u{0827}', '\u{0829}', // 120-124
    '\u{082A}', '\u{082B}', '\u{082C}', '\u{082D}', '\u{0951}', // 125-129
    '\u{0953}', '\u{0954}', '\u{0F82}', '\u{0F83}', '\u{0F86}', // 130-134
    '\u{0F87}', '\u{135D}', '\u{135E}', '\u{135F}', '\u{17DD}', // 135-139
    '\u{193A}', '\u{1A17}', '\u{1A75}', '\u{1A76}', '\u{1A77}', // 140-144
    '\u{1A78}', '\u{1A79}', '\u{1A7A}', '\u{1A7B}', '\u{1A7C}', // 145-149
    '\u{1B6B}', '\u{1B6D}', '\u{1B6E}', '\u{1B6F}', '\u{1B70}', // 150-154
    '\u{1B71}', '\u{1B72}', '\u{1B73}', '\u{1CD0}', '\u{1CD1}', // 155-159
    '\u{1CD2}', '\u{1CDA}', '\u{1CDB}', '\u{1CE0}', '\u{1DC0}', // 160-164
    '\u{1DC1}', '\u{1DC3}', '\u{1DC4}', '\u{1DC5}', '\u{1DC6}', // 165-169
    '\u{1DC7}', '\u{1DC8}', '\u{1DC9}', '\u{1DCB}', '\u{1DCC}', // 170-174
    '\u{1DD1}', '\u{1DD2}', '\u{1DD3}', '\u{1DD4}', '\u{1DD5}', // 175-179
    '\u{1DD6}', '\u{1DD7}', '\u{1DD8}', '\u{1DD9}', '\u{1DDA}', // 180-184
    '\u{1DDB}', '\u{1DDC}', '\u{1DDD}', '\u{1DDE}', '\u{1DDF}', // 185-189
    '\u{1DE0}', '\u{1DE1}', '\u{1DE2}', '\u{1DE3}', '\u{1DE4}', // 190-194
    '\u{1DE5}', '\u{1DE6}', '\u{1DFE}', '\u{20D0}', '\u{20D1}', // 195-199
    '\u{20D4}', '\u{20D5}', '\u{20D6}', '\u{20D7}', '\u{20DB}', // 200-204
    '\u{20DC}', '\u{20E1}', '\u{20E7}', '\u{20E9}', '\u{20F0}', // 205-209
    '\u{2CEF}', '\u{2CF0}', '\u{2CF1}', '\u{2DE0}', '\u{2DE1}', // 210-214
    '\u{2DE2}', '\u{2DE3}', '\u{2DE4}', '\u{2DE5}', '\u{2DE6}', // 215-219
    '\u{2DE7}', '\u{2DE8}', '\u{2DE9}', '\u{2DEA}', '\u{2DEB}', // 220-224
    '\u{2DEC}', '\u{2DED}', '\u{2DEE}', '\u{2DEF}', '\u{2DF0}', // 225-229
    '\u{2DF1}', '\u{2DF2}', '\u{2DF3}', '\u{2DF4}', '\u{2DF5}', // 230-234
    '\u{2DF6}', '\u{2DF7}', '\u{2DF8}', '\u{2DF9}', '\u{2DFA}', // 235-239
    '\u{2DFB}', '\u{2DFC}', '\u{2DFD}', '\u{2DFE}', '\u{2DFF}', // 240-244
    '\u{A66F}', '\u{A67C}', '\u{A67D}', '\u{A6F0}', '\u{A6F1}', // 245-249
    '\u{A8E0}', '\u{A8E1}', '\u{A8E2}', '\u{A8E3}', '\u{A8E4}', // 250-254
    '\u{A8E5}', // 255
];

/// Return the placeholder symbol for a specific (row, col) image cell.
/// Each cell is U+10EEEE + row diacritic + column diacritic.
pub fn placeholder_symbol(row: usize, col: usize) -> String {
    let row_d = DIACRITICS[row.min(255)];
    let col_d = DIACRITICS[col.min(255)];
    format!("\u{10EEEE}{row_d}{col_d}")
}

/// Return the foreground color encoding a Kitty image ID as RGB.
pub fn kitty_id_color(image_id: u32) -> Color {
    let r = ((image_id >> 16) & 0xFF) as u8;
    let g = ((image_id >> 8) & 0xFF) as u8;
    let b = (image_id & 0xFF) as u8;
    Color::Rgb(r, g, b)
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

            spans.push(Span::styled("▀", Style::default().fg(fg).bg(bg)));
        }

        lines.push(Line::from(spans));
    }

    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic Sixel DCS string with the given number of bands.
    /// Each band is a simple pixel row for testing.
    fn make_sixel(num_bands: usize) -> String {
        // DCS header + raster attrs + 2 color definitions + bands + ST
        let mut s = String::from("\x1bPq\"1;1;10;");
        s.push_str(&(num_bands * 6).to_string()); // vertical pixel count
        s.push_str("#0;2;100;0;0#1;2;0;100;0"); // two color definitions
        for i in 0..num_bands {
            if i > 0 {
                s.push('-');
            }
            // Each band: select color 0, write some pixel data
            s.push_str("#0???");
        }
        s.push_str("\x1b\\");
        s
    }

    #[test]
    fn slice_no_crop_exact_bands() {
        // cell_px_h=6, 10 cells = 60px = 10 bands. Sixel has exactly 10 bands.
        // Raster is stripped but all 10 bands are preserved.
        let sixel = make_sixel(10);
        let result = slice_sixel_bands(&sixel, 6, 10, 0, 10).unwrap();
        let body_start = result.find('q').unwrap() + 1;
        let body_end = result.rfind("\x1b\\").unwrap();
        let body = &result[body_start..body_end];
        let preamble_end = find_preamble_end(body);
        let pixel_data = &body[preamble_end..];
        assert_eq!(pixel_data.split('-').count(), 10);
        // Raster attributes are stripped
        assert!(!result.contains("\"1;1;"));
        // Color definitions preserved
        assert!(result.contains("#0;2;100;0;0"));
    }

    #[test]
    fn slice_crop_top() {
        // 10 bands, cell_px_h=6, so 1 band per cell.
        // Crop top 3 cells = skip 3 bands, take remaining 7.
        let sixel = make_sixel(10);
        let result = slice_sixel_bands(&sixel, 6, 10, 3, 7).unwrap();
        // Should have 7 bands
        let body_start = result.find('q').unwrap() + 1;
        let body_end = result.rfind("\x1b\\").unwrap();
        let body = &result[body_start..body_end];
        let preamble_end = find_preamble_end(body);
        let pixel_data = &body[preamble_end..];
        let band_count = pixel_data.split('-').count();
        assert_eq!(band_count, 7);
    }

    #[test]
    fn slice_crop_bottom() {
        // 10 bands, take only 4 from top.
        let sixel = make_sixel(10);
        let result = slice_sixel_bands(&sixel, 6, 10, 0, 4).unwrap();
        let body_start = result.find('q').unwrap() + 1;
        let body_end = result.rfind("\x1b\\").unwrap();
        let body = &result[body_start..body_end];
        let preamble_end = find_preamble_end(body);
        let pixel_data = &body[preamble_end..];
        let band_count = pixel_data.split('-').count();
        assert_eq!(band_count, 4);
    }

    #[test]
    fn slice_middle() {
        // 20 bands, cell_px_h=6 (1 band per cell). Skip 5, take 8.
        let sixel = make_sixel(20);
        let result = slice_sixel_bands(&sixel, 6, 20, 5, 8).unwrap();
        let body_start = result.find('q').unwrap() + 1;
        let body_end = result.rfind("\x1b\\").unwrap();
        let body = &result[body_start..body_end];
        let preamble_end = find_preamble_end(body);
        let pixel_data = &body[preamble_end..];
        let band_count = pixel_data.split('-').count();
        assert_eq!(band_count, 8);
    }

    #[test]
    fn slice_preserves_color_defs() {
        let sixel = make_sixel(5);
        let result = slice_sixel_bands(&sixel, 6, 5, 2, 3).unwrap();
        // Color definitions should be preserved
        assert!(result.contains("#0;2;100;0;0"));
        assert!(result.contains("#1;2;0;100;0"));
    }

    #[test]
    fn slice_empty_crop_returns_none() {
        let sixel = make_sixel(5);
        // Skip all bands
        let result = slice_sixel_bands(&sixel, 6, 5, 5, 0);
        assert!(result.is_none());
    }

    #[test]
    fn find_preamble_end_basic() {
        let body = "\"1;1;10;60#0;2;100;0;0#1;2;0;100;0#0???-#0???";
        let end = find_preamble_end(body);
        // Should stop at the #0 that's a color selection (no ; after digits)
        assert_eq!(&body[end..end + 2], "#0");
        // And the char after the digits should be '?' (pixel data)
        assert_eq!(body.as_bytes()[end + 2], b'?');
    }

    #[test]
    fn slice_with_larger_cell_px() {
        // cell_px_h=20, full_height=5 cells = 100px = 17 bands (ceil(100/6))
        // crop_top=2 cells = 40px = skip 6 bands (40/6=6.67, floor=6)
        // visible=3 cells = 60px = take 10 bands (floor(60/6)=10)
        let sixel = make_sixel(17);
        let result = slice_sixel_bands(&sixel, 20, 5, 2, 3).unwrap();
        let body_start = result.find('q').unwrap() + 1;
        let body_end = result.rfind("\x1b\\").unwrap();
        let body = &result[body_start..body_end];
        let preamble_end = find_preamble_end(body);
        let pixel_data = &body[preamble_end..];
        let band_count = pixel_data.split('-').count();
        assert_eq!(band_count, 10);
    }

    #[test]
    fn slice_clips_overflow_band() {
        // Simulate icy_sixel adding an extra band: 18 cells * 20px = 360px
        // = 60 bands needed, but Sixel has 61 bands (366px).
        // Floor division should clip to 60 bands, not 61.
        let sixel = make_sixel(61); // 61 bands like real icy_sixel output
        let result = slice_sixel_bands(&sixel, 20, 18, 0, 18).unwrap();
        let body_start = result.find('q').unwrap() + 1;
        let body_end = result.rfind("\x1b\\").unwrap();
        let body = &result[body_start..body_end];
        let preamble_end = find_preamble_end(body);
        let pixel_data = &body[preamble_end..];
        let band_count = pixel_data.split('-').count();
        // 18 cells * 20px = 360px. floor(360/6) = 60 bands, NOT 61.
        assert_eq!(band_count, 60);
    }

    #[test]
    fn slice_clips_full_image_overflow() {
        // Full image (no crop) should still clip the extra band.
        // 30 cells * 20px = 600px = 100 bands, but Sixel has 101.
        let sixel = make_sixel(101);
        let result = slice_sixel_bands(&sixel, 20, 30, 0, 30).unwrap();
        let body_start = result.find('q').unwrap() + 1;
        let body_end = result.rfind("\x1b\\").unwrap();
        let body = &result[body_start..body_end];
        let preamble_end = find_preamble_end(body);
        let pixel_data = &body[preamble_end..];
        let band_count = pixel_data.split('-').count();
        assert_eq!(band_count, 100);
    }
}

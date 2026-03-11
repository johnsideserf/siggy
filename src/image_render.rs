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
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64_full).ok()?;
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
    '\u{A8E5}',                                                    // 255
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

            spans.push(Span::styled(
                "▀",
                Style::default().fg(fg).bg(bg),
            ));
        }

        lines.push(Line::from(spans));
    }

    Some(lines)
}

//! Text for the vertical bar: rasterised, then rotated to read bottom-to-top.

use ab_glyph::{Font, FontArc, ScaleFont};
use iced::widget::image;
use iced::Color;

/// Truncate a string to fit within `max_chars`, adding ellipsis
pub fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}\u{2026}")
}

/// Rasterize text using `ab_glyph` with proper anti-aliasing, then rotate the
/// resulting bitmap -90 degrees so it reads bottom-to-top in a vertical bar.
pub fn render_rotated_text(
    font: &FontArc,
    text: &str,
    font_size: f32,
    color: Color,
) -> image::Handle {
    let (alpha_buf, buf_w, buf_h) = rasterise(font, text, font_size);
    let [r, g, b, _] = color.into_rgba8();
    let rgba = rotate_ccw(&alpha_buf, buf_w, buf_h, [r, g, b]);
    image::Handle::from_rgba(buf_h, buf_w, rgba)
}

/// Rasterize `text` set in `font` at `font_size` into a coverage buffer,
/// tightly sized to the text's advance width and line height.
///
/// Returns the coverage buffer along with its width and height.
#[allow(clippy::arithmetic_side_effects)]
fn rasterise(font: &FontArc, text: &str, font_size: f32) -> (Vec<u8>, u32, u32) {
    let scaled = font.as_scaled(font_size);

    let ascent = scaled.ascent();
    let descent = scaled.descent();
    let line_height = ascent - descent;

    let glyph_ids: Vec<ab_glyph::GlyphId> = text.chars().map(|ch| font.glyph_id(ch)).collect();
    let total_advance: f32 = glyph_ids.iter().map(|&g| scaled.h_advance(g)).sum();

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let buf_w = (total_advance.ceil() as u32).max(1);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let buf_h = (line_height.ceil() as u32).max(1);

    let mut alpha_buf = vec![0u8; buf_w as usize * buf_h as usize];
    let mut cursor_x: f32 = 0.0;

    for &glyph_id in &glyph_ids {
        let glyph = glyph_id.with_scale_and_position(font_size, ab_glyph::point(cursor_x, ascent));

        if let Some(outlined) = font.outline_glyph(glyph) {
            let bb = outlined.px_bounds();
            outlined.draw(|px, py, coverage| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
                let x = px.cast_signed() + (bb.min.x as i32);
                #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
                let y = py.cast_signed() + (bb.min.y as i32);

                if x >= 0 && y >= 0 {
                    #[allow(clippy::cast_sign_loss)]
                    let ux = x.cast_unsigned();
                    #[allow(clippy::cast_sign_loss)]
                    let uy = y.cast_unsigned();
                    if ux < buf_w && uy < buf_h {
                        let idx = uy as usize * buf_w as usize + ux as usize;
                        if let Some(pixel) = alpha_buf.get_mut(idx) {
                            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                            let val = coverage.mul_add(255.0, f32::from(*pixel)).min(255.0) as u8;
                            *pixel = val;
                        }
                    }
                }
            });
        }

        cursor_x += scaled.h_advance(glyph_id);
    }

    (alpha_buf, buf_w, buf_h)
}

/// Rotate a `width x height` coverage buffer -90 degrees into a
/// `height x width` RGBA buffer, tinting every covered pixel with `rgb`.
///
/// The pixel at `(ox, oy)` in the source lands at `(oy, width - 1 - ox)` in
/// the rotated buffer, so text drawn left-to-right ends up reading
/// bottom-to-top.
#[allow(clippy::arithmetic_side_effects)]
fn rotate_ccw(alpha: &[u8], width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    let rot_w = height;
    let rot_h = width;
    let [r, g, b] = rgb;
    let mut rgba = vec![0u8; rot_w as usize * rot_h as usize * 4];

    for oy in 0..height {
        for ox in 0..width {
            let src_idx = oy as usize * width as usize + ox as usize;
            let Some(&a) = alpha.get(src_idx) else {
                continue;
            };
            if a == 0 {
                continue;
            }
            let nx = oy;
            let ny = width.saturating_sub(1).saturating_sub(ox);
            let dst = (ny as usize * rot_w as usize + nx as usize) * 4;
            if let Some(chunk) = rgba.get_mut(dst..dst + 4) {
                chunk.copy_from_slice(&[r, g, b, a]);
            }
        }
    }

    rgba
}

#[cfg(test)]
mod tests {
    use super::{rotate_ccw, truncate_with_ellipsis};

    #[test]
    fn rotate_ccw_maps_pixels_and_colour() {
        let alpha = [10, 0, 0, 0, 0, 20];
        let rgba = rotate_ccw(&alpha, 3, 2, [1, 2, 3]);

        assert_eq!(rgba.len(), 2 * 3 * 4);
        let pixel = |x: usize, y: usize| {
            let idx = (y * 2 + x) * 4;
            rgba.get(idx..idx + 4)
        };

        assert_eq!(pixel(0, 2), Some(&[1u8, 2, 3, 10][..]));
        assert_eq!(pixel(1, 0), Some(&[1u8, 2, 3, 20][..]));
        assert_eq!(pixel(1, 2), Some(&[0u8, 0, 0, 0][..]));
    }

    #[test]
    fn short_text_is_untouched() {
        assert_eq!(truncate_with_ellipsis("abc", 3), "abc");
    }

    #[test]
    fn long_text_ends_in_an_ellipsis_within_the_limit() {
        assert_eq!(truncate_with_ellipsis("abcdef", 4), "abc\u{2026}");
    }

    #[test]
    fn truncation_counts_characters_not_bytes() {
        assert_eq!(
            truncate_with_ellipsis("\u{e9}\u{e9}\u{e9}", 3),
            "\u{e9}\u{e9}\u{e9}"
        );
        assert_eq!(
            truncate_with_ellipsis("\u{e9}\u{e9}\u{e9}\u{e9}", 3),
            "\u{e9}\u{e9}\u{2026}"
        );
    }
}

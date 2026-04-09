use crate::services::hyprland::WindowInfo;
use crate::style;
use crate::Message;
use ab_glyph::{Font, FontArc, ScaleFont};
use iced::widget::{container, image};
use iced::{Alignment, Element, Length};

/// Truncate a string to fit within `max_chars`, adding ellipsis
fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}\u{2026}")
}

/// Rasterize text using `ab_glyph` with proper anti-aliasing, then rotate the
/// resulting bitmap -90 degrees so it reads bottom-to-top in a vertical bar.
fn render_rotated_text(font: &FontArc, text: &str, font_size: f32) -> image::Handle {
    let scaled = font.as_scaled(font_size);

    let ascent = scaled.ascent();
    let descent = scaled.descent();
    let line_height = ascent - descent;

    let mut total_advance: f32 = 0.0;
    let glyph_ids: Vec<ab_glyph::GlyphId> = text
        .chars()
        .map(|ch| {
            let glyph_id = font.glyph_id(ch);
            total_advance += scaled.h_advance(glyph_id);
            glyph_id
        })
        .collect();

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let buf_w = (total_advance.ceil() as u32).max(1);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let buf_h = (line_height.ceil() as u32).max(1);
    let buf_len = buf_w as usize * buf_h as usize;

    // Rasterize glyphs into an alpha buffer
    let mut alpha_buf = vec![0u8; buf_len];
    let mut cursor_x: f32 = 0.0;

    for &glyph_id in &glyph_ids {
        let glyph = glyph_id.with_scale_and_position(font_size, ab_glyph::point(cursor_x, ascent));

        if let Some(outlined) = font.outline_glyph(glyph) {
            let bb = outlined.px_bounds();
            outlined.draw(|px, py, coverage| {
                #[allow(clippy::cast_possible_truncation)]
                let x = px as i32 + bb.min.x as i32;
                #[allow(clippy::cast_possible_truncation)]
                let y = py as i32 + bb.min.y as i32;

                if x >= 0 && y >= 0 {
                    let ux = x as u32;
                    let uy = y as u32;
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

    // Rotate -90 degrees: original (buf_w x buf_h) -> rotated (buf_h x buf_w)
    let rot_w = buf_h;
    let rot_h = buf_w;
    let [r, g, b, _] = style::M3_PRIMARY.into_rgba8();
    let mut rgba = vec![0u8; rot_w as usize * rot_h as usize * 4];

    for oy in 0..buf_h {
        for ox in 0..buf_w {
            let src_idx = oy as usize * buf_w as usize + ox as usize;
            let Some(&a) = alpha_buf.get(src_idx) else {
                continue;
            };
            if a == 0 {
                continue;
            }
            // Rotate -90deg: new_x = oy, new_y = buf_w - 1 - ox
            let nx = oy;
            let ny = buf_w.saturating_sub(1).saturating_sub(ox);
            let dst = (ny as usize * rot_w as usize + nx as usize) * 4;
            if let Some(chunk) = rgba.get_mut(dst..dst + 4) {
                chunk.copy_from_slice(&[r, g, b, a]);
            }
        }
    }

    image::Handle::from_rgba(rot_w, rot_h, rgba)
}

pub fn view<'a>(window: Option<&WindowInfo>, font: Option<&FontArc>) -> Element<'a, Message> {
    let title = window.map_or_else(
        || "Desktop".into(),
        |w| {
            let parts: Vec<&str> = w.title.split(&['\u{2013}', '\u{2014}', '-'][..]).collect();
            let raw = parts
                .last()
                .map_or_else(|| w.title.clone(), |s| s.trim().to_string());
            truncate_with_ellipsis(&raw, 20)
        },
    );

    let content: Element<'_, Message> = if let Some(f) = font {
        let handle = render_rotated_text(f, &title, style::FONT_SIZE_LARGE);
        image(handle).content_fit(iced::ContentFit::None).into()
    } else {
        iced::widget::text(title)
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_PRIMARY)
            .align_x(Alignment::Center)
            .into()
    };

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
}

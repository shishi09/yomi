use base64::Engine;
use image::imageops::FilterType;
use image::{ImageFormat, Rgba, RgbaImage};
use std::io::Cursor;

use crate::image_proc::Bubble;

const STROKE_RGBA: Rgba<u8> = Rgba([235, 31, 31, 255]);
const BADGE_RGBA: Rgba<u8> = Rgba([224, 20, 20, 255]);
const WHITE_RGBA: Rgba<u8> = Rgba([255, 255, 255, 255]);

fn digit_glyph(d: char) -> [u8; 7] {
    match d {
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111],
        '3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110],
        _ => [0; 7],
    }
}

fn fill_rect(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, color: Rgba<u8>) {
    let cw = img.width() as i32;
    let ch = img.height() as i32;
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(cw);
    let y1 = (y + h).min(ch);
    for py in y0..y1 {
        for px in x0..x1 {
            img.put_pixel(px as u32, py as u32, color);
        }
    }
}

fn draw_box_outline(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, thickness: i32) {
    fill_rect(img, x, y, w, thickness, STROKE_RGBA);
    fill_rect(img, x, y + h - thickness, w, thickness, STROKE_RGBA);
    fill_rect(img, x, y, thickness, h, STROKE_RGBA);
    fill_rect(img, x + w - thickness, y, thickness, h, STROKE_RGBA);
}

fn draw_label(img: &mut RgbaImage, x: i32, y: i32, text: &str, scale: i32) {
    let mut cx = x;
    for ch in text.chars() {
        let glyph = digit_glyph(ch);
        for (ry, row) in glyph.iter().enumerate() {
            for rx in 0..5 {
                if (row >> (4 - rx)) & 1 == 1 {
                    fill_rect(
                        img,
                        cx + rx * scale,
                        y + (ry as i32) * scale,
                        scale,
                        scale,
                        WHITE_RGBA,
                    );
                }
            }
        }
        cx += 6 * scale;
    }
}

pub fn create_som_image_base64(
    image_bytes: &[u8],
    bubbles: &[Bubble],
    max_dim: u32,
) -> Result<String, String> {
    let dyn_img = image::load_from_memory(image_bytes)
        .map_err(|e| format!("Impossible d'ouvrir l'image pour Set-of-Mark : {}", e))?;
    let orig_w = dyn_img.width().max(1);
    let orig_h = dyn_img.height().max(1);
    let max_orig = orig_w.max(orig_h);
    let scale = (max_dim.max(256) as f64 / max_orig as f64).min(1.0);
    let canvas_w = ((orig_w as f64 * scale).round() as u32).max(1);
    let canvas_h = ((orig_h as f64 * scale).round() as u32).max(1);

    let resized = if canvas_w != orig_w || canvas_h != orig_h {
        dyn_img.resize_exact(canvas_w, canvas_h, FilterType::Lanczos3)
    } else {
        dyn_img
    };
    let mut canvas = resized.to_rgba8();

    let digit_scale = if canvas_w >= 900 { 2 } else { 1 };
    let pad_x = 4 * digit_scale;
    let pad_y = 2 * digit_scale;

    for b in bubbles {
        let bx = (b.x * canvas_w as f64).round() as i32;
        let by = (b.y * canvas_h as f64).round() as i32;
        let bw = ((b.w * canvas_w as f64).round() as i32).max(4);
        let bh = ((b.h * canvas_h as f64).round() as i32).max(4);

        draw_box_outline(&mut canvas, bx, by, bw, bh, 2);

        let label = b.id.to_string();
        let text_w = (label.len() as i32 * 6 - 1).max(5) * digit_scale;
        let text_h = 7 * digit_scale;
        let badge_w = (text_w + pad_x * 2).max(text_h + pad_y);
        let badge_h = text_h + pad_y * 2;

        // Place badge right above top-left corner so inner text is never obscured
        let raw_bx = bx;
        let raw_by = by - badge_h - 1;
        let badge_x = raw_bx.clamp(1, (canvas_w as i32 - badge_w - 1).max(1));
        let badge_y = raw_by.clamp(1, (canvas_h as i32 - badge_h - 1).max(1));

        fill_rect(&mut canvas, badge_x, badge_y, badge_w, badge_h, BADGE_RGBA);
        let tx = badge_x + (badge_w - text_w) / 2;
        let ty = badge_y + (badge_h - text_h) / 2;
        draw_label(&mut canvas, tx, ty, &label, digit_scale);
    }

    let rgb = image::DynamicImage::ImageRgba8(canvas).to_rgb8();
    let mut jpg_bytes = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg_bytes, 85);
    encoder
        .encode(
            rgb.as_raw(),
            canvas_w,
            canvas_h,
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| format!("Erreur encodage JPEG Set-of-Mark : {}", e))?;

    let _ = ImageFormat::Jpeg;
    let _ = Cursor::new(&jpg_bytes);
    Ok(base64::engine::general_purpose::STANDARD.encode(&jpg_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    #[test]
    fn renders_valid_jpeg_base64_for_bubbles() {
        let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_pixel(640, 800, Rgb([245, 240, 230]));
        let mut png_bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png).unwrap();

        let bubbles = vec![Bubble {
            id: 12,
            text: "TEST".into(),
            x: 0.2,
            y: 0.3,
            w: 0.4,
            h: 0.15,
            line_count: 1,
            min_conf: 0.95,
            style: "bubble_light".into(),
            fr: None,
            who: None,
        }];
        let b64 = create_som_image_base64(&png_bytes, &bubbles, 512).unwrap();
        let decoded = base64::engine::general_purpose::STANDARD.decode(&b64).unwrap();
        assert!(decoded.starts_with(&[0xFF, 0xD8]));
    }
}

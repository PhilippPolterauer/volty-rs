use image::{Rgba, RgbaImage};
use tray_icon::Icon;

use super::TrayError;
use crate::config::BatteryColorMap;

pub(super) const ICON_SIZE: u32 = 16;
/// Renders an RGBA battery-level icon. Supported sizes are 16 and 32 pixels square.
#[cfg(test)]
fn battery_rgba(percentage: u8, size: u32) -> Result<Vec<u8>, TrayError> {
    battery_rgba_with_color_map(percentage, size, BatteryColorMap::default())
}

fn battery_rgba_with_color_map(
    percentage: u8,
    size: u32,
    battery_color_map: BatteryColorMap,
) -> Result<Vec<u8>, TrayError> {
    validate_icon_size(size)?;
    Ok(render_battery(percentage, size, battery_color_map).into_raw())
}

/// Produces a battery-level icon at 16×16 or 32×32 pixels.
#[cfg(test)]
fn battery_icon_at_size(percentage: u8, size: u32) -> Result<Icon, TrayError> {
    let rgba = battery_rgba(percentage, size)?;
    Icon::from_rgba(rgba, size, size)
        .map_err(|error| TrayError::new(format!("could not create battery icon: {error}")))
}

pub(super) fn battery_icon_at_size_with_color_map(
    percentage: u8,
    size: u32,
    battery_color_map: BatteryColorMap,
) -> Result<Icon, TrayError> {
    let rgba = battery_rgba_with_color_map(percentage, size, battery_color_map)?;
    Icon::from_rgba(rgba, size, size)
        .map_err(|error| TrayError::new(format!("could not create battery icon: {error}")))
}

fn render_battery(percentage: u8, size: u32, battery_color_map: BatteryColorMap) -> RgbaImage {
    let percentage = percentage.min(100);
    let scale = size as f32 * SUPERSAMPLE as f32 / ICON_SIZE as f32;
    let mut image =
        RgbaImage::from_pixel(size * SUPERSAMPLE, size * SUPERSAMPLE, Rgba([0, 0, 0, 0]));
    let fill_color = battery_color_map.color_for(percentage);

    if percentage > 0 {
        let fill_width = 10.25 * (f32::from(percentage) / 100.0).max(0.08);
        fill_rounded_rect(
            &mut image,
            scale,
            RoundedRect {
                x: 2.25,
                y: 3.25,
                width: fill_width,
                height: 9.55,
                radius: 1.2,
            },
            Rgba([fill_color[0], fill_color[1], fill_color[2], 255]),
        );
    }
    draw_battery_shell(&mut image, scale, Rgba([232, 238, 243, 255]));

    downsample_icon(image, size)
}

/// 4×7 bitmap glyphs for the digits 0-9. Bit 3 is the leftmost column.
const DIGIT_GLYPHS: [[u8; 7]; 10] = [
    [0b0110, 0b1001, 0b1001, 0b1001, 0b1001, 0b1001, 0b0110],
    [0b0010, 0b0110, 0b0010, 0b0010, 0b0010, 0b0010, 0b0111],
    [0b0110, 0b1001, 0b0001, 0b0010, 0b0100, 0b1000, 0b1111],
    [0b1110, 0b0001, 0b0001, 0b0110, 0b0001, 0b0001, 0b1110],
    [0b0010, 0b0110, 0b1010, 0b1010, 0b1111, 0b0010, 0b0010],
    [0b1111, 0b1000, 0b1110, 0b0001, 0b0001, 0b1001, 0b0110],
    [0b0110, 0b1000, 0b1000, 0b1110, 0b1001, 0b1001, 0b0110],
    [0b1111, 0b0001, 0b0010, 0b0010, 0b0100, 0b0100, 0b0100],
    [0b0110, 0b1001, 0b1001, 0b0110, 0b1001, 0b1001, 0b0110],
    [0b0110, 0b1001, 0b1001, 0b0111, 0b0001, 0b0001, 0b0110],
];

pub(super) fn percentage_icon_at_size_with_color_map(
    percentage: u8,
    size: u32,
    battery_color_map: BatteryColorMap,
) -> Result<Icon, TrayError> {
    validate_icon_size(size)?;
    let rgba = render_percentage(percentage, size, battery_color_map).into_raw();
    Icon::from_rgba(rgba, size, size)
        .map_err(|error| TrayError::new(format!("could not create percentage icon: {error}")))
}

/// Renders the percentage as number-only digits with no percent sign: at 16 px
/// a mark would crowd out the third digit of `100`. Glyph pixels are one screen
/// pixel at 16×16 and two screen pixels at 32×32, with a transparent background.
fn render_percentage(percentage: u8, size: u32, battery_color_map: BatteryColorMap) -> RgbaImage {
    let percentage = percentage.min(100);
    let mut image = RgbaImage::from_pixel(size, size, Rgba([0, 0, 0, 0]));
    let fill_color = battery_color_map.color_for(percentage);
    let color = Rgba([fill_color[0], fill_color[1], fill_color[2], 255]);

    let text = percentage.to_string();
    let scale = size / ICON_SIZE;
    let glyph_width = 4 * scale;
    let gap = scale;
    let digit_count = text.len() as u32;
    let text_width = digit_count * glyph_width + (digit_count - 1) * gap;
    let text_height = 7 * scale;
    let origin_x = (size - text_width) / 2;
    let origin_y = (size - text_height) / 2;

    for (index, digit) in text.bytes().enumerate() {
        let glyph = DIGIT_GLYPHS[usize::from(digit - b'0')];
        let digit_x = origin_x + index as u32 * (glyph_width + gap);
        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..4 {
                if bits & (0b1000 >> column) == 0 {
                    continue;
                }
                let pixel_x = digit_x + column * scale;
                let pixel_y = origin_y + row as u32 * scale;
                for offset_y in 0..scale {
                    for offset_x in 0..scale {
                        image.put_pixel(pixel_x + offset_x, pixel_y + offset_y, color);
                    }
                }
            }
        }
    }

    image
}

#[cfg(test)]
fn percentage_rgba(percentage: u8, size: u32) -> Result<Vec<u8>, TrayError> {
    validate_icon_size(size)?;
    Ok(render_percentage(percentage, size, BatteryColorMap::default()).into_raw())
}

/// Renders the neutral unknown/error icon as an RGBA buffer (16×16 or 32×32).
pub(super) fn unknown_battery_rgba(size: u32) -> Result<Vec<u8>, TrayError> {
    validate_icon_size(size)?;
    Ok(render_unknown_battery(size).into_raw())
}

pub(super) fn unknown_battery_icon_at_size(size: u32) -> Result<Icon, TrayError> {
    let rgba = unknown_battery_rgba(size)?;
    Icon::from_rgba(rgba, size, size)
        .map_err(|error| TrayError::new(format!("could not create unknown battery icon: {error}")))
}

fn render_unknown_battery(size: u32) -> RgbaImage {
    let scale = size as f32 * SUPERSAMPLE as f32 / ICON_SIZE as f32;
    let mut image =
        RgbaImage::from_pixel(size * SUPERSAMPLE, size * SUPERSAMPLE, Rgba([0, 0, 0, 0]));
    let outline = Rgba([232, 238, 243, 255]);
    draw_battery_shell(&mut image, scale, outline);

    let question_mark = [
        (5.5, 6.45),
        (5.8, 5.8),
        (6.5, 5.3),
        (7.4, 5.1),
        (8.4, 5.3),
        (9.0, 5.85),
        (9.2, 6.5),
        (9.0, 7.2),
        (8.4, 7.8),
        (7.55, 8.45),
    ];
    for points in question_mark.windows(2) {
        draw_line_segment(&mut image, scale, points[0], points[1], 1.25, outline);
    }
    fill_circle(&mut image, scale, 7.55, 10.0, 0.75, outline);

    downsample_icon(image, size)
}

pub(super) fn validate_icon_size(size: u32) -> Result<(), TrayError> {
    if matches!(size, 16 | 32) {
        Ok(())
    } else {
        Err(TrayError::new("tray icon size must be 16 or 32 pixels"))
    }
}

const SUPERSAMPLE: u32 = 8;

#[derive(Clone, Copy)]
struct RoundedRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
}

fn draw_battery_shell(image: &mut RgbaImage, scale: f32, color: Rgba<u8>) {
    stroke_rounded_rect(
        image,
        scale,
        RoundedRect {
            x: 0.65,
            y: 1.55,
            width: 13.4,
            height: 12.9,
            radius: 2.4,
        },
        1.3,
        color,
    );
    fill_rounded_rect(
        image,
        scale,
        RoundedRect {
            x: 13.75,
            y: 6.2,
            width: 1.5,
            height: 3.6,
            radius: 0.75,
        },
        color,
    );
}

fn fill_rounded_rect(image: &mut RgbaImage, scale: f32, rect: RoundedRect, color: Rgba<u8>) {
    let left = (rect.x.max(0.0) * scale).floor() as u32;
    let top = (rect.y.max(0.0) * scale).floor() as u32;
    let right = ((rect.x + rect.width).min(ICON_SIZE as f32) * scale)
        .ceil()
        .min(image.width() as f32) as u32;
    let bottom = ((rect.y + rect.height).min(ICON_SIZE as f32) * scale)
        .ceil()
        .min(image.height() as f32) as u32;

    for pixel_y in top..bottom {
        for pixel_x in left..right {
            let point_x = (pixel_x as f32 + 0.5) / scale;
            let point_y = (pixel_y as f32 + 0.5) / scale;
            if inside_rounded_rect(point_x, point_y, rect) {
                image.put_pixel(pixel_x, pixel_y, color);
            }
        }
    }
}

fn stroke_rounded_rect(
    image: &mut RgbaImage,
    scale: f32,
    rect: RoundedRect,
    thickness: f32,
    color: Rgba<u8>,
) {
    let left = (rect.x.max(0.0) * scale).floor() as u32;
    let top = (rect.y.max(0.0) * scale).floor() as u32;
    let right = ((rect.x + rect.width).min(ICON_SIZE as f32) * scale)
        .ceil()
        .min(image.width() as f32) as u32;
    let bottom = ((rect.y + rect.height).min(ICON_SIZE as f32) * scale)
        .ceil()
        .min(image.height() as f32) as u32;

    for pixel_y in top..bottom {
        for pixel_x in left..right {
            let point_x = (pixel_x as f32 + 0.5) / scale;
            let point_y = (pixel_y as f32 + 0.5) / scale;
            let outer = inside_rounded_rect(point_x, point_y, rect);
            let inner = inside_rounded_rect(
                point_x,
                point_y,
                RoundedRect {
                    x: rect.x + thickness,
                    y: rect.y + thickness,
                    width: rect.width - 2.0 * thickness,
                    height: rect.height - 2.0 * thickness,
                    radius: (rect.radius - thickness).max(0.0),
                },
            );
            if outer && !inner {
                image.put_pixel(pixel_x, pixel_y, color);
            }
        }
    }
}

fn inside_rounded_rect(point_x: f32, point_y: f32, rect: RoundedRect) -> bool {
    let radius = rect.radius.min(rect.width / 2.0).min(rect.height / 2.0);
    let half_width = (rect.width / 2.0 - radius).max(0.0);
    let half_height = (rect.height / 2.0 - radius).max(0.0);
    let delta_x = (point_x - (rect.x + rect.width / 2.0)).abs() - half_width;
    let delta_y = (point_y - (rect.y + rect.height / 2.0)).abs() - half_height;
    delta_x.max(0.0).hypot(delta_y.max(0.0)) + delta_x.max(delta_y).min(0.0) <= radius
}

fn draw_line_segment(
    image: &mut RgbaImage,
    scale: f32,
    start: (f32, f32),
    end: (f32, f32),
    thickness: f32,
    color: Rgba<u8>,
) {
    let radius = thickness / 2.0;
    let left = ((start.0.min(end.0) - radius).max(0.0) * scale).floor() as u32;
    let top = ((start.1.min(end.1) - radius).max(0.0) * scale).floor() as u32;
    let right = ((start.0.max(end.0) + radius).min(ICON_SIZE as f32) * scale)
        .ceil()
        .min(image.width() as f32) as u32;
    let bottom = ((start.1.max(end.1) + radius).min(ICON_SIZE as f32) * scale)
        .ceil()
        .min(image.height() as f32) as u32;
    let vector_x = end.0 - start.0;
    let vector_y = end.1 - start.1;
    let length_squared = vector_x * vector_x + vector_y * vector_y;

    for pixel_y in top..bottom {
        for pixel_x in left..right {
            let point_x = (pixel_x as f32 + 0.5) / scale;
            let point_y = (pixel_y as f32 + 0.5) / scale;
            let projection = (((point_x - start.0) * vector_x + (point_y - start.1) * vector_y)
                / length_squared)
                .clamp(0.0, 1.0);
            let nearest_x = start.0 + projection * vector_x;
            let nearest_y = start.1 + projection * vector_y;
            if (point_x - nearest_x).powi(2) + (point_y - nearest_y).powi(2) <= radius * radius {
                image.put_pixel(pixel_x, pixel_y, color);
            }
        }
    }
}

fn fill_circle(
    image: &mut RgbaImage,
    scale: f32,
    center_x: f32,
    center_y: f32,
    radius: f32,
    color: Rgba<u8>,
) {
    let left = ((center_x - radius).max(0.0) * scale).floor() as u32;
    let top = ((center_y - radius).max(0.0) * scale).floor() as u32;
    let right = ((center_x + radius).min(ICON_SIZE as f32) * scale)
        .ceil()
        .min(image.width() as f32) as u32;
    let bottom = ((center_y + radius).min(ICON_SIZE as f32) * scale)
        .ceil()
        .min(image.height() as f32) as u32;

    for pixel_y in top..bottom {
        for pixel_x in left..right {
            let point_x = (pixel_x as f32 + 0.5) / scale;
            let point_y = (pixel_y as f32 + 0.5) / scale;
            if (point_x - center_x).powi(2) + (point_y - center_y).powi(2) <= radius * radius {
                image.put_pixel(pixel_x, pixel_y, color);
            }
        }
    }
}

fn downsample_icon(image: RgbaImage, size: u32) -> RgbaImage {
    let mut output = RgbaImage::new(size, size);
    let sample_count = u64::from(SUPERSAMPLE * SUPERSAMPLE);

    // Premultiplied averaging keeps transparent edges clean on any taskbar background.
    for y in 0..size {
        for x in 0..size {
            let mut alpha_sum = 0_u64;
            let mut color_sum = [0_u64; 3];
            for sample_y in 0..SUPERSAMPLE {
                for sample_x in 0..SUPERSAMPLE {
                    let pixel =
                        image.get_pixel(x * SUPERSAMPLE + sample_x, y * SUPERSAMPLE + sample_y);
                    let alpha = u64::from(pixel[3]);
                    alpha_sum += alpha;
                    for channel in 0..3 {
                        color_sum[channel] += u64::from(pixel[channel]) * alpha;
                    }
                }
            }

            if alpha_sum > 0 {
                let alpha = ((alpha_sum + sample_count / 2) / sample_count) as u8;
                let color = color_sum.map(|channel| ((channel + alpha_sum / 2) / alpha_sum) as u8);
                output.put_pixel(x, y, Rgba([color[0], color[1], color[2], alpha]));
            }
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_renderer_accepts_boundary_percentages() {
        for percentage in [0, 15, 16, 30, 31, 100] {
            let icon = battery_icon_at_size(percentage, ICON_SIZE).expect("valid 16x16 icon");
            let _ = icon;
        }
    }

    #[test]
    fn rgba_renderers_support_both_dpi_sizes() {
        for size in [16, 32] {
            assert_eq!(
                battery_rgba(85, size).unwrap().len(),
                (size * size * 4) as usize
            );
            assert_eq!(
                unknown_battery_rgba(size).unwrap().len(),
                (size * size * 4) as usize
            );
        }
        assert!(battery_rgba(85, 24).is_err());
    }

    #[test]
    fn rgba_renderers_antialias_icon_edges() {
        for rgba in [
            battery_rgba(85, ICON_SIZE).unwrap(),
            unknown_battery_rgba(ICON_SIZE).unwrap(),
        ] {
            assert!(rgba
                .iter()
                .skip(3)
                .step_by(4)
                .any(|alpha| *alpha > 0 && *alpha < 255));
        }
    }

    #[test]
    fn battery_icon_uses_nearly_the_full_canvas() {
        let rgba = battery_rgba(85, ICON_SIZE).unwrap();
        let mut bounds = [ICON_SIZE, ICON_SIZE, 0, 0];

        for y in 0..ICON_SIZE {
            for x in 0..ICON_SIZE {
                let alpha_index = ((y * ICON_SIZE + x) * 4 + 3) as usize;
                if rgba[alpha_index] > 0 {
                    bounds[0] = bounds[0].min(x);
                    bounds[1] = bounds[1].min(y);
                    bounds[2] = bounds[2].max(x);
                    bounds[3] = bounds[3].max(y);
                }
            }
        }

        assert_eq!(bounds, [0, 1, ICON_SIZE - 1, ICON_SIZE - 2]);
    }

    #[test]
    fn rgba_renderer_uses_custom_color_map_boundaries() {
        let map = BatteryColorMap {
            red_max_percent: 10,
            yellow_max_percent: 40,
            red: [1, 2, 3],
            yellow: [4, 5, 6],
            green: [7, 8, 9],
        };
        let pixel = |percentage| {
            let rgba = battery_rgba_with_color_map(percentage, ICON_SIZE, map).unwrap();
            let index = ((10 * ICON_SIZE + 3) * 4) as usize;
            [rgba[index], rgba[index + 1], rgba[index + 2]]
        };

        assert_eq!(pixel(10), [1, 2, 3]);
        assert_eq!(pixel(11), [4, 5, 6]);
        assert_eq!(pixel(40), [4, 5, 6]);
        assert_eq!(pixel(41), [7, 8, 9]);
    }

    fn opaque_pixels(rgba: &[u8]) -> Vec<[u8; 4]> {
        rgba.as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[3] > 0)
            .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
            .collect()
    }

    #[test]
    fn digit_glyphs_are_complete_and_distinct() {
        for (digit, glyph) in DIGIT_GLYPHS.iter().enumerate() {
            assert!(glyph.iter().any(|row| *row != 0), "digit {digit} is blank");
            assert!(
                glyph.iter().all(|row| row & !0b1111 == 0),
                "digit {digit} exceeds four columns"
            );
            assert!(
                glyph.iter().map(|row| row.count_ones()).sum::<u32>() >= 7,
                "digit {digit} is too sparse"
            );
        }
        for (left, glyph) in DIGIT_GLYPHS.iter().enumerate() {
            for other in DIGIT_GLYPHS.iter().skip(left + 1) {
                assert_ne!(glyph, other);
            }
        }
    }

    #[test]
    fn percentage_rgba_supports_both_dpi_sizes() {
        for size in [16, 32] {
            for percentage in [0, 1, 9, 10, 42, 99, 100] {
                let rgba = percentage_rgba(percentage, size).unwrap();
                assert_eq!(rgba.len(), (size * size * 4) as usize);
                assert!(
                    !opaque_pixels(&rgba).is_empty(),
                    "{percentage}% at {size}px produced no visible pixels"
                );
            }
        }
        assert!(percentage_rgba(85, 24).is_err());
    }

    #[test]
    fn percentage_icon_uses_the_active_color_band() {
        let map = BatteryColorMap::default();
        for (percentage, expected) in [(10, map.red), (20, map.yellow), (60, map.green)] {
            let rgba = render_percentage(percentage, 32, map).into_raw();
            let opaque = opaque_pixels(&rgba);
            assert!(!opaque.is_empty());
            for pixel in opaque {
                assert_eq!(pixel[3], 255);
                assert_eq!(
                    &pixel[..3],
                    &expected,
                    "{percentage}% used the wrong color band"
                );
            }
        }
    }

    #[test]
    fn percentage_renderer_handles_zero_and_one_hundred() {
        for size in [16, 32] {
            let zero = percentage_rgba(0, size).unwrap();
            let hundred = percentage_rgba(100, size).unwrap();
            assert!(!opaque_pixels(&zero).is_empty());
            assert!(!opaque_pixels(&hundred).is_empty());
            assert_ne!(zero, hundred);
        }
    }

    #[test]
    fn percentage_digits_fill_more_of_the_icon() {
        let rgba = percentage_rgba(100, ICON_SIZE).unwrap();
        let mut bounds = [ICON_SIZE, ICON_SIZE, 0, 0];

        for y in 0..ICON_SIZE {
            for x in 0..ICON_SIZE {
                let alpha_index = ((y * ICON_SIZE + x) * 4 + 3) as usize;
                if rgba[alpha_index] > 0 {
                    bounds[0] = bounds[0].min(x);
                    bounds[1] = bounds[1].min(y);
                    bounds[2] = bounds[2].max(x);
                    bounds[3] = bounds[3].max(y);
                }
            }
        }

        assert_eq!(bounds, [2, 4, 14, 10]);
    }

    #[test]
    fn percentage_icons_differ_between_dpi_sizes() {
        let small = percentage_rgba(85, 16).unwrap();
        let large = percentage_rgba(85, 32).unwrap();
        assert_ne!(small, large);
        assert!(large.len() > small.len());
    }
}

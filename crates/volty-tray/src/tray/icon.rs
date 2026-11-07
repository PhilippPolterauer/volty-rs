use std::{path::PathBuf, sync::OnceLock};

use fontdue::Font;
use image::{Rgba, RgbaImage};
use tray_icon::Icon;

use super::TrayError;
use crate::config::BatteryColorMap;

pub(super) const ICON_SIZE: u32 = 16;
static PERCENTAGE_FONT: OnceLock<Result<Font, String>> = OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum IconTheme {
    Light,
    Dark,
}

impl IconTheme {
    const fn outline_color(self) -> [u8; 4] {
        match self {
            Self::Light => [24, 28, 32, 255],
            Self::Dark => [245, 247, 250, 255],
        }
    }
}

/// Renders an RGBA battery-level icon. Supported sizes are 16 and 32 pixels square.
#[cfg(test)]
fn battery_rgba(percentage: u8, size: u32) -> Result<Vec<u8>, TrayError> {
    battery_rgba_with_color_map(
        percentage,
        size,
        BatteryColorMap::default(),
        IconTheme::Light,
    )
}

fn battery_rgba_with_color_map(
    percentage: u8,
    size: u32,
    battery_color_map: BatteryColorMap,
    theme: IconTheme,
) -> Result<Vec<u8>, TrayError> {
    validate_icon_size(size)?;
    Ok(render_battery(percentage, size, battery_color_map, theme).into_raw())
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
    theme: IconTheme,
) -> Result<Icon, TrayError> {
    let rgba = battery_rgba_with_color_map(percentage, size, battery_color_map, theme)?;
    Icon::from_rgba(rgba, size, size)
        .map_err(|error| TrayError::new(format!("could not create battery icon: {error}")))
}

fn render_battery(
    percentage: u8,
    size: u32,
    battery_color_map: BatteryColorMap,
    theme: IconTheme,
) -> RgbaImage {
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
    draw_battery_shell(&mut image, scale, Rgba(theme.outline_color()));

    downsample_icon(image, size)
}

pub(super) fn percentage_icon_at_size_with_color_map(
    percentage: u8,
    size: u32,
    battery_color_map: BatteryColorMap,
) -> Result<Icon, TrayError> {
    validate_icon_size(size)?;
    let rgba = render_percentage(percentage, size, battery_color_map)?.into_raw();
    Icon::from_rgba(rgba, size, size)
        .map_err(|error| TrayError::new(format!("could not create percentage icon: {error}")))
}

/// Renders number-only digits using the Windows Segoe UI font.
fn render_percentage(
    percentage: u8,
    size: u32,
    battery_color_map: BatteryColorMap,
) -> Result<RgbaImage, TrayError> {
    let percentage = percentage.min(100);
    let mut image = RgbaImage::from_pixel(size, size, Rgba([0, 0, 0, 0]));
    let fill_color = battery_color_map.color_for(percentage);
    let color = Rgba([fill_color[0], fill_color[1], fill_color[2], 255]);

    let text = percentage.to_string();
    let font = percentage_font()?;
    let base_font_size = if text.len() == 3 { 8.0 } else { 11.5 };
    let font_size = base_font_size * (size as f32 / ICON_SIZE as f32);
    let mut glyphs = Vec::with_capacity(text.len());
    let mut pen_x = 0.0;
    let mut previous = None;
    let mut left = f32::INFINITY;
    let mut top = f32::INFINITY;
    let mut right = f32::NEG_INFINITY;
    let mut bottom = f32::NEG_INFINITY;

    for character in text.chars() {
        if let Some(previous) = previous {
            pen_x += font
                .horizontal_kern(previous, character, font_size)
                .unwrap_or(0.0);
        }
        let (metrics, bitmap) = font.rasterize(character, font_size);
        let glyph_left = pen_x + metrics.xmin as f32;
        let glyph_top = -metrics.ymin as f32 - metrics.height as f32;
        left = left.min(glyph_left);
        top = top.min(glyph_top);
        right = right.max(glyph_left + metrics.width as f32);
        bottom = bottom.max(glyph_top + metrics.height as f32);
        glyphs.push((metrics, bitmap, pen_x));
        pen_x += metrics.advance_width;
        previous = Some(character);
    }

    let origin_x = (size as f32 - (right - left)) / 2.0 - left;
    let origin_y = (size as f32 - (bottom - top)) / 2.0 - top;
    for (metrics, bitmap, pen_x) in glyphs {
        let glyph_x = (origin_x + pen_x + metrics.xmin as f32).round() as i32;
        let glyph_y = (origin_y - metrics.ymin as f32 - metrics.height as f32).round() as i32;
        for row in 0..metrics.height {
            for column in 0..metrics.width {
                let alpha = bitmap[row * metrics.width + column];
                if alpha == 0 {
                    continue;
                }
                let pixel_x = glyph_x + column as i32;
                let pixel_y = glyph_y + row as i32;
                if pixel_x >= 0 && pixel_x < size as i32 && pixel_y >= 0 && pixel_y < size as i32 {
                    image.put_pixel(
                        pixel_x as u32,
                        pixel_y as u32,
                        Rgba([color[0], color[1], color[2], alpha]),
                    );
                }
            }
        }
    }

    Ok(center_opaque_pixels(image))
}

fn center_opaque_pixels(image: RgbaImage) -> RgbaImage {
    let (width, height) = image.dimensions();
    let mut bounds = [width, height, 0, 0];
    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel[3] > 0 {
            bounds[0] = bounds[0].min(x);
            bounds[1] = bounds[1].min(y);
            bounds[2] = bounds[2].max(x);
            bounds[3] = bounds[3].max(y);
        }
    }

    if bounds[0] == width {
        return image;
    }

    let shift_x = ((width - 1) as i32 - (bounds[0] + bounds[2]) as i32) / 2;
    let shift_y = ((height - 1) as i32 - (bounds[1] + bounds[3]) as i32) / 2;
    let mut centered = RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 0]));
    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel[3] == 0 {
            continue;
        }
        let x = x as i32 + shift_x;
        let y = y as i32 + shift_y;
        if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
            centered.put_pixel(x as u32, y as u32, *pixel);
        }
    }

    centered
}

fn percentage_font() -> Result<&'static Font, TrayError> {
    PERCENTAGE_FONT
        .get_or_init(load_percentage_font)
        .as_ref()
        .map_err(|error| {
            TrayError::new(format!(
                "could not load Segoe UI for tray percentage icon: {error}"
            ))
        })
}

fn load_percentage_font() -> Result<Font, String> {
    let windows_directory = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .ok_or_else(|| "WINDIR environment variable is not set".to_owned())?;
    let font_path = windows_directory.join("Fonts").join("segoeui.ttf");
    let font_bytes = std::fs::read(&font_path)
        .map_err(|error| format!("could not read {}: {error}", font_path.display()))?;
    Font::from_bytes(font_bytes, fontdue::FontSettings::default())
        .map_err(|error| format!("could not parse {}: {error:?}", font_path.display()))
}

#[cfg(test)]
fn percentage_rgba(percentage: u8, size: u32) -> Result<Vec<u8>, TrayError> {
    validate_icon_size(size)?;
    Ok(render_percentage(percentage, size, BatteryColorMap::default())?.into_raw())
}

/// Renders the neutral unknown/error icon as an RGBA buffer (16×16 or 32×32).
pub(super) fn unknown_battery_rgba(size: u32, theme: IconTheme) -> Result<Vec<u8>, TrayError> {
    validate_icon_size(size)?;
    Ok(render_unknown_battery(size, theme).into_raw())
}

pub(super) fn unknown_battery_icon_at_size(size: u32, theme: IconTheme) -> Result<Icon, TrayError> {
    let rgba = unknown_battery_rgba(size, theme)?;
    Icon::from_rgba(rgba, size, size)
        .map_err(|error| TrayError::new(format!("could not create unknown battery icon: {error}")))
}

fn render_unknown_battery(size: u32, theme: IconTheme) -> RgbaImage {
    let scale = size as f32 * SUPERSAMPLE as f32 / ICON_SIZE as f32;
    let mut image =
        RgbaImage::from_pixel(size * SUPERSAMPLE, size * SUPERSAMPLE, Rgba([0, 0, 0, 0]));
    let outline = Rgba(theme.outline_color());
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
        1.6,
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
                unknown_battery_rgba(size, IconTheme::Light).unwrap().len(),
                (size * size * 4) as usize
            );
        }
        assert!(battery_rgba(85, 24).is_err());
    }

    #[test]
    fn rgba_renderers_antialias_icon_edges() {
        for rgba in [
            battery_rgba(85, ICON_SIZE).unwrap(),
            unknown_battery_rgba(ICON_SIZE, IconTheme::Light).unwrap(),
        ] {
            assert!(rgba
                .iter()
                .skip(3)
                .step_by(4)
                .any(|alpha| *alpha > 0 && *alpha < 255));
        }
    }

    #[test]
    fn battery_shell_uses_an_outline_visible_in_each_theme() {
        let map = BatteryColorMap::default();
        for (theme, expected) in [
            (IconTheme::Light, [24, 28, 32, 255]),
            (IconTheme::Dark, [245, 247, 250, 255]),
        ] {
            let rgba = battery_rgba_with_color_map(85, ICON_SIZE, map, theme).unwrap();
            assert!(opaque_pixels(&rgba).contains(&expected));
            let unknown = unknown_battery_rgba(ICON_SIZE, theme).unwrap();
            assert!(opaque_pixels(&unknown).contains(&expected));
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
            let rgba =
                battery_rgba_with_color_map(percentage, ICON_SIZE, map, IconTheme::Light).unwrap();
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
            let rgba = render_percentage(percentage, 32, map).unwrap().into_raw();
            let opaque = opaque_pixels(&rgba);
            assert!(!opaque.is_empty());
            for pixel in &opaque {
                assert_eq!(
                    &pixel[..3],
                    &expected,
                    "{percentage}% used the wrong color band"
                );
            }
            assert!(opaque.iter().any(|pixel| pixel[3] < 255));
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
    fn three_digit_percentage_is_centered() {
        for size in [16, 32] {
            let rgba = percentage_rgba(100, size).unwrap();
            assert_centered(opaque_bounds(&rgba, size), size);
        }
    }

    #[test]
    fn two_digit_percentage_is_larger_and_centered() {
        for size in [16, 32] {
            let rgba = percentage_rgba(94, size).unwrap();
            let bounds = opaque_bounds(&rgba, size);
            assert!(bounds[2] - bounds[0] + 1 >= 10 * size / ICON_SIZE);
            assert!(bounds[3] - bounds[1] + 1 >= 7 * size / ICON_SIZE);
            assert_centered(bounds, size);
        }
    }

    #[test]
    fn percentage_icons_differ_between_dpi_sizes() {
        let small = percentage_rgba(85, 16).unwrap();
        let large = percentage_rgba(85, 32).unwrap();
        assert_ne!(small, large);
        assert!(large.len() > small.len());
    }

    fn opaque_bounds(rgba: &[u8], size: u32) -> [u32; 4] {
        let mut bounds = [size, size, 0, 0];
        for y in 0..size {
            for x in 0..size {
                let alpha_index = ((y * size + x) * 4 + 3) as usize;
                if rgba[alpha_index] > 0 {
                    bounds[0] = bounds[0].min(x);
                    bounds[1] = bounds[1].min(y);
                    bounds[2] = bounds[2].max(x);
                    bounds[3] = bounds[3].max(y);
                }
            }
        }
        bounds
    }

    fn assert_centered(bounds: [u32; 4], size: u32) {
        let horizontal_center_twice = bounds[0] + bounds[2];
        let vertical_center_twice = bounds[1] + bounds[3];
        assert!(
            horizontal_center_twice.abs_diff(size - 1) <= 1,
            "horizontal bounds {bounds:?} are not centered in {size}px"
        );
        assert!(
            vertical_center_twice.abs_diff(size - 1) <= 1,
            "vertical bounds {bounds:?} are not centered in {size}px"
        );
    }
}

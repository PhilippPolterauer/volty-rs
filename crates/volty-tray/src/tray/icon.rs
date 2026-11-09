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

#[cfg(test)]
fn battery_rgba_with_color_map(
    percentage: u8,
    size: u32,
    battery_color_map: BatteryColorMap,
    theme: IconTheme,
) -> Result<Vec<u8>, TrayError> {
    battery_rgba_with_percentage(percentage, size, battery_color_map, theme, false)
}

fn battery_rgba_with_percentage(
    percentage: u8,
    size: u32,
    battery_color_map: BatteryColorMap,
    theme: IconTheme,
    show_percentage: bool,
) -> Result<Vec<u8>, TrayError> {
    validate_icon_size(size)?;
    render_battery(percentage, size, battery_color_map, theme, show_percentage)
        .map(RgbaImage::into_raw)
}

pub(super) fn preview_rgba(
    percentage: u8,
    size: u32,
    battery_color_map: BatteryColorMap,
    theme: IconTheme,
    style: crate::state::TrayIconStyle,
    show_percentage: bool,
) -> Result<Vec<u8>, TrayError> {
    validate_icon_size(size)?;
    match style {
        crate::state::TrayIconStyle::Battery => battery_rgba_with_percentage(
            percentage,
            size,
            battery_color_map,
            theme,
            show_percentage,
        ),
        crate::state::TrayIconStyle::Percentage => {
            Ok(render_percentage(percentage, size, battery_color_map, theme)?.into_raw())
        }
    }
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
    show_percentage: bool,
) -> Result<Icon, TrayError> {
    let rgba =
        battery_rgba_with_percentage(percentage, size, battery_color_map, theme, show_percentage)?;
    Icon::from_rgba(rgba, size, size)
        .map_err(|error| TrayError::new(format!("could not create battery icon: {error}")))
}

fn render_battery(
    percentage: u8,
    size: u32,
    battery_color_map: BatteryColorMap,
    theme: IconTheme,
    show_percentage: bool,
) -> Result<RgbaImage, TrayError> {
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

    let mut rendered = downsample_icon(image, size);
    if show_percentage {
        draw_battery_percentage(&mut rendered, percentage, size, fill_color)?;
    }

    Ok(rendered)
}

pub(super) fn percentage_icon_at_size_with_color_map(
    percentage: u8,
    size: u32,
    battery_color_map: BatteryColorMap,
    theme: IconTheme,
) -> Result<Icon, TrayError> {
    validate_icon_size(size)?;
    let rgba = render_percentage(percentage, size, battery_color_map, theme)?.into_raw();
    Icon::from_rgba(rgba, size, size)
        .map_err(|error| TrayError::new(format!("could not create percentage icon: {error}")))
}

/// Renders number-only digits using the Windows Segoe UI font.
fn render_percentage(
    percentage: u8,
    size: u32,
    battery_color_map: BatteryColorMap,
    theme: IconTheme,
) -> Result<RgbaImage, TrayError> {
    let percentage = percentage.min(100);
    let mut image = RgbaImage::from_pixel(size, size, Rgba([0, 0, 0, 0]));
    let fill_color = battery_color_map.color_for(percentage);
    let text = percentage.to_string();
    let base_font_size = if text.len() == 3 { 8.0 } else { 11.5 };
    let font_size = base_font_size * (size as f32 / ICON_SIZE as f32);
    let mask = percentage_text_mask(
        percentage,
        size,
        font_size,
        (size as f32 / 2.0, size as f32 / 2.0),
    )?
    .0;
    let outline = dilate_mask(
        &mask,
        size,
        size,
        (size as f32 / ICON_SIZE as f32).ceil() as i32,
    );
    let (outline_color, text_color) = match theme {
        IconTheme::Light => {
            let outline = theme.outline_color();
            ([outline[0], outline[1], outline[2]], fill_color)
        }
        IconTheme::Dark => (fill_color, [245, 247, 250]),
    };
    composite_mask(&mut image, &outline, outline_color);
    composite_mask(&mut image, &mask, text_color);
    Ok(image)
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

fn draw_battery_percentage(
    image: &mut RgbaImage,
    percentage: u8,
    size: u32,
    fill_color: [u8; 3],
) -> Result<(), TrayError> {
    let text = percentage.to_string();
    let scale = size as f32 / ICON_SIZE as f32;
    let center = (7.4 * scale, 8.0 * scale);
    let max_width = 9.6 * scale;
    let max_height = 7.2 * scale;
    let base_font_size = if text.len() == 3 { 7.6 } else { 9.0 };
    let mut font_size = base_font_size * scale;
    let mut mask = percentage_text_mask(percentage, image.width(), font_size, center)?;
    let text_width = mask.1[2] - mask.1[0];
    let text_height = mask.1[3] - mask.1[1];
    let fit = (max_width / text_width)
        .min(max_height / text_height)
        .min(1.0);
    if fit < 1.0 {
        font_size *= fit;
        mask = percentage_text_mask(percentage, image.width(), font_size, center)?;
    }

    let luminance = 0.2126 * f32::from(fill_color[0])
        + 0.7152 * f32::from(fill_color[1])
        + 0.0722 * f32::from(fill_color[2]);
    let (text_color, outline_color) = if luminance >= 145.0 {
        ([16, 18, 22], [255, 255, 255])
    } else {
        ([255, 255, 255], [12, 14, 18])
    };
    let outline = dilate_mask(&mask.0, image.width(), image.height(), scale.ceil() as i32);
    let opaque_outline: Vec<u8> = outline
        .iter()
        .map(|alpha| alpha.saturating_mul(2))
        .collect();
    composite_mask(image, &opaque_outline, outline_color);
    let bold_mask: Vec<u8> = mask
        .0
        .iter()
        .map(|alpha| (u16::from(*alpha) * 3 / 2).min(255) as u8)
        .collect();
    composite_mask(image, &bold_mask, text_color);
    Ok(())
}

fn dilate_mask(mask: &[u8], width: u32, height: u32, radius: i32) -> Vec<u8> {
    let mut dilated = vec![0; mask.len()];
    let radius_squared = radius * radius;
    for y in 0..height as i32 {
        for x in 0..width as i32 {
            let alpha = mask[y as usize * width as usize + x as usize];
            if alpha == 0 {
                continue;
            }
            for offset_y in -radius..=radius {
                for offset_x in -radius..=radius {
                    if offset_x * offset_x + offset_y * offset_y > radius_squared {
                        continue;
                    }
                    let target_x = x + offset_x;
                    let target_y = y + offset_y;
                    if target_x >= 0
                        && target_x < width as i32
                        && target_y >= 0
                        && target_y < height as i32
                    {
                        let index = target_y as usize * width as usize + target_x as usize;
                        dilated[index] = dilated[index].max(alpha);
                    }
                }
            }
        }
    }
    dilated
}

fn percentage_text_mask(
    percentage: u8,
    canvas_size: u32,
    font_size: f32,
    center: (f32, f32),
) -> Result<(Vec<u8>, [f32; 4]), TrayError> {
    let text = percentage.to_string();
    let font = percentage_font()?;
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

    let origin_x = center.0 - (left + right) / 2.0;
    let origin_y = center.1 - (top + bottom) / 2.0;
    let mut mask = vec![0; (canvas_size * canvas_size) as usize];
    for (metrics, bitmap, pen_x) in glyphs {
        let glyph_x = (origin_x + pen_x + metrics.xmin as f32).round() as i32;
        let glyph_y = (origin_y - metrics.ymin as f32 - metrics.height as f32).round() as i32;
        for row in 0..metrics.height {
            for column in 0..metrics.width {
                let alpha = bitmap[row * metrics.width + column];
                let pixel_x = glyph_x + column as i32;
                let pixel_y = glyph_y + row as i32;
                if alpha > 0
                    && pixel_x >= 0
                    && pixel_x < canvas_size as i32
                    && pixel_y >= 0
                    && pixel_y < canvas_size as i32
                {
                    let index = pixel_y as usize * canvas_size as usize + pixel_x as usize;
                    mask[index] = mask[index].max(alpha);
                }
            }
        }
    }

    Ok((mask, [left, top, right, bottom]))
}

fn composite_mask(image: &mut RgbaImage, mask: &[u8], color: [u8; 3]) {
    for (index, alpha) in mask.iter().copied().enumerate() {
        if alpha == 0 {
            continue;
        }
        let x = index as u32 % image.width();
        let y = index as u32 / image.width();
        let destination = image.get_pixel(x, y);
        let source_alpha = u32::from(alpha);
        let destination_alpha = u32::from(destination[3]);
        let remaining_alpha = 255 - source_alpha;
        let output_alpha = source_alpha + (destination_alpha * remaining_alpha + 127) / 255;
        let mut output = [0; 4];
        for channel in 0..3 {
            let source = u32::from(color[channel]) * source_alpha;
            let destination = u32::from(destination[channel]) * destination_alpha;
            let premultiplied = source + (destination * remaining_alpha + 127) / 255;
            output[channel] = ((premultiplied + output_alpha / 2) / output_alpha) as u8;
        }
        output[3] = output_alpha as u8;
        image.put_pixel(x, y, Rgba(output));
    }
}

#[cfg(test)]
fn percentage_rgba(percentage: u8, size: u32) -> Result<Vec<u8>, TrayError> {
    validate_icon_size(size)?;
    Ok(render_percentage(
        percentage,
        size,
        BatteryColorMap::default(),
        IconTheme::Light,
    )?
    .into_raw())
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
    fn rgba_renderer_blends_custom_color_map_between_percentage_anchors() {
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
        assert_eq!(pixel(25), [3, 4, 5]);
        assert_eq!(pixel(40), [4, 5, 6]);
        assert_eq!(pixel(70), [6, 7, 8]);
        assert_eq!(pixel(100), [7, 8, 9]);
    }

    #[test]
    fn battery_percentage_overlay_uses_contrasting_text_and_outline() {
        let orange_map = BatteryColorMap {
            red_max_percent: 100,
            yellow_max_percent: 100,
            red: [255, 140, 0],
            yellow: [255, 140, 0],
            green: [255, 140, 0],
        };
        let orange =
            battery_rgba_with_percentage(20, ICON_SIZE, orange_map, IconTheme::Light, true)
                .unwrap();
        let plain_orange =
            battery_rgba_with_color_map(20, ICON_SIZE, orange_map, IconTheme::Light).unwrap();
        assert_ne!(orange, plain_orange);
        let center_pixels = |rgba: &[u8]| {
            rgba.as_chunks::<4>()
                .0
                .iter()
                .enumerate()
                .filter(|(index, pixel)| {
                    let x = *index as u32 % ICON_SIZE;
                    let y = *index as u32 / ICON_SIZE;
                    (5..12).contains(&x) && (5..12).contains(&y) && pixel[3] > 0
                })
                .map(|(_, pixel)| [pixel[0], pixel[1], pixel[2]])
                .collect::<Vec<_>>()
        };
        let orange_text = center_pixels(&orange);
        assert!(orange_text
            .iter()
            .any(|pixel| pixel.iter().all(|c| *c < 100)));
        assert!(orange_text
            .iter()
            .any(|pixel| pixel.iter().all(|c| *c > 200)));

        let dark_map = BatteryColorMap {
            red: [110, 25, 25],
            yellow: [110, 25, 25],
            green: [110, 25, 25],
            ..BatteryColorMap::default()
        };
        let dark =
            battery_rgba_with_percentage(100, ICON_SIZE, dark_map, IconTheme::Dark, true).unwrap();
        let plain_dark =
            battery_rgba_with_color_map(100, ICON_SIZE, dark_map, IconTheme::Dark).unwrap();
        assert_ne!(dark, plain_dark);
        let dark_text = center_pixels(&dark);
        assert!(dark_text.iter().any(|pixel| pixel.iter().all(|c| *c > 200)));
        assert!(
            dark_text.iter().any(|pixel| pixel.iter().all(|c| *c < 50)),
            "{dark_text:?}"
        );

        for percentage in [0, 42, 100] {
            let with_overlay =
                battery_rgba_with_percentage(percentage, 32, dark_map, IconTheme::Dark, true)
                    .unwrap();
            let without_overlay =
                battery_rgba_with_color_map(percentage, 32, dark_map, IconTheme::Dark).unwrap();
            assert_ne!(with_overlay, without_overlay);
        }
    }

    #[test]
    fn generates_readme_graphics_from_the_icon_renderer() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after Unix epoch")
            .as_nanos();
        let output_dir = std::env::var_os("VOLTY_VISUAL_OUTPUT_DIR")
            .map_or_else(std::env::temp_dir, std::path::PathBuf::from);
        std::fs::create_dir_all(&output_dir).unwrap();
        let file_name = |name: &str| {
            if std::env::var_os("VOLTY_VISUAL_OUTPUT_DIR").is_some() {
                format!("{name}.png")
            } else {
                format!("volty-{name}-{}-{nonce}.png", std::process::id())
            }
        };

        let hero_path = output_dir.join(file_name("hero"));
        let tray_path = output_dir.join(file_name("tray-display"));
        render_readme_hero().unwrap().save(&hero_path).unwrap();
        render_tray_display().unwrap().save(&tray_path).unwrap();
        eprintln!("Rendered README hero: {}", hero_path.display());
        eprintln!("Rendered tray display graphic: {}", tray_path.display());
        assert!(hero_path.exists());
        assert!(tray_path.exists());
    }

    fn render_tray_display() -> Result<RgbaImage, TrayError> {
        use image::imageops::{overlay, resize, FilterType};

        const LEVELS: [u8; 4] = [5, 15, 30, 100];
        const PANEL_X: u32 = 36;
        const PANEL_WIDTH: u32 = 1248;
        const PANEL_HEIGHT: u32 = 292;
        const PANEL_GAP: u32 = 18;
        const HEADER_HEIGHT: u32 = 82;
        const ROW_HEIGHT: u32 = 50;
        const LABEL_GUTTER: u32 = 110;
        const COLUMN_WIDTH: u32 = 365;
        const ICON_RENDER_SIZE: u32 = 32;
        const ICON_DISPLAY_SIZE: u32 = 48;

        let mut preview = RgbaImage::from_pixel(
            1320,
            36 + PANEL_HEIGHT * 2 + PANEL_GAP,
            Rgba([246, 248, 251, 255]),
        );
        let color_map = BatteryColorMap::default();
        let columns = ["Battery", "Battery + percentage", "Percentage only"];

        for (theme_index, (theme, label, panel_color, foreground, muted)) in [
            (
                IconTheme::Light,
                "LIGHT TASKBAR",
                [255, 255, 255],
                [31, 41, 55],
                [103, 116, 135],
            ),
            (
                IconTheme::Dark,
                "DARK TASKBAR",
                [31, 37, 48],
                [237, 241, 247],
                [166, 177, 193],
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let panel_y = 18 + theme_index as u32 * (PANEL_HEIGHT + PANEL_GAP);
            fill_preview_rounded_rect(
                &mut preview,
                PANEL_X,
                panel_y,
                PANEL_WIDTH,
                PANEL_HEIGHT,
                18.0,
                Rgba([panel_color[0], panel_color[1], panel_color[2], 255]),
            );
            draw_preview_text(
                &mut preview,
                label,
                (PANEL_X + 34) as f32,
                panel_y as f32 + 22.0,
                13.0,
                [67, 144, 226],
            )?;
            draw_preview_text(
                &mut preview,
                "Same renderer · transparent backgrounds · theme-aware contrast",
                (PANEL_X + PANEL_WIDTH - 230) as f32,
                panel_y as f32 + 24.0,
                9.0,
                muted,
            )?;

            for (column, title) in columns.iter().enumerate() {
                let center_x =
                    (PANEL_X + LABEL_GUTTER + column as u32 * COLUMN_WIDTH + COLUMN_WIDTH / 2)
                        as f32;
                draw_preview_text(
                    &mut preview,
                    title,
                    center_x,
                    panel_y as f32 + 56.0,
                    14.0,
                    foreground,
                )?;
            }

            for (row, percentage) in LEVELS.into_iter().enumerate() {
                let row_y = panel_y + HEADER_HEIGHT + row as u32 * ROW_HEIGHT;
                if row > 0 {
                    fill_preview_rect(
                        &mut preview,
                        PANEL_X + 28,
                        row_y,
                        PANEL_WIDTH - 56,
                        1,
                        Rgba([muted[0], muted[1], muted[2], 55]),
                    );
                }
                draw_preview_text(
                    &mut preview,
                    &format!("{percentage:02}%"),
                    (PANEL_X + 50) as f32,
                    row_y as f32 + 15.0,
                    14.0,
                    color_map.color_for(percentage),
                )?;

                let icon_variants = [
                    battery_rgba_with_percentage(
                        percentage,
                        ICON_RENDER_SIZE,
                        color_map,
                        theme,
                        false,
                    )?,
                    battery_rgba_with_percentage(
                        percentage,
                        ICON_RENDER_SIZE,
                        color_map,
                        theme,
                        true,
                    )?,
                    render_percentage(percentage, ICON_RENDER_SIZE, color_map, theme)?.into_raw(),
                ];
                for (column, rgba) in icon_variants.into_iter().enumerate() {
                    let center_x =
                        PANEL_X + LABEL_GUTTER + column as u32 * COLUMN_WIDTH + COLUMN_WIDTH / 2;
                    let icon = RgbaImage::from_raw(ICON_RENDER_SIZE, ICON_RENDER_SIZE, rgba)
                        .expect("renderer returns a complete RGBA icon");
                    let icon = resize(
                        &icon,
                        ICON_DISPLAY_SIZE,
                        ICON_DISPLAY_SIZE,
                        FilterType::Nearest,
                    );
                    overlay(
                        &mut preview,
                        &icon,
                        i64::from(center_x - ICON_DISPLAY_SIZE / 2),
                        i64::from(row_y + (ROW_HEIGHT - ICON_DISPLAY_SIZE) / 2),
                    );
                }
            }
        }

        Ok(preview)
    }

    fn render_readme_hero() -> Result<RgbaImage, TrayError> {
        use image::imageops::{overlay, resize, FilterType};
        use std::path::Path;

        const WIDTH: u32 = 1600;
        const HEIGHT: u32 = 800;
        let mut hero = RgbaImage::new(WIDTH, HEIGHT);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let horizontal = x as f32 / (WIDTH - 1) as f32;
                let vertical = y as f32 / (HEIGHT - 1) as f32;
                let blend = (horizontal * 0.7 + vertical * 0.3).clamp(0.0, 1.0);
                hero.put_pixel(
                    x,
                    y,
                    Rgba([
                        (12.0 + 13.0 * blend) as u8,
                        (20.0 + 17.0 * blend) as u8,
                        (38.0 + 27.0 * blend) as u8,
                        255,
                    ]),
                );
            }
        }

        draw_preview_text(
            &mut hero,
            "VOLTY  /  WINDOWS BATTERY MONITOR",
            800.0,
            42.0,
            15.0,
            [99, 217, 154],
        )?;
        draw_preview_text(
            &mut hero,
            "Battery health, at a glance.",
            800.0,
            78.0,
            54.0,
            [248, 250, 253],
        )?;
        draw_preview_text(
            &mut hero,
            "A quiet, local-first companion for the devices you use every day.",
            800.0,
            150.0,
            20.0,
            [178, 193, 214],
        )?;

        fill_preview_rounded_rect(&mut hero, 66, 232, 660, 490, 22.0, Rgba([24, 35, 55, 255]));
        draw_preview_text(
            &mut hero,
            "LIVE IN YOUR SYSTEM TRAY",
            396.0,
            260.0,
            13.0,
            [99, 217, 154],
        )?;
        draw_preview_text(
            &mut hero,
            "Three ways to check charge",
            396.0,
            290.0,
            23.0,
            [245, 248, 252],
        )?;

        let color_map = BatteryColorMap::default();
        const ICON_RENDER_SIZE: u32 = 32;
        let icons = [
            battery_rgba_with_percentage(30, ICON_RENDER_SIZE, color_map, IconTheme::Dark, false)?,
            battery_rgba_with_percentage(30, ICON_RENDER_SIZE, color_map, IconTheme::Dark, true)?,
            render_percentage(30, ICON_RENDER_SIZE, color_map, IconTheme::Dark)?.into_raw(),
        ];
        let labels = ["Battery", "Battery + %", "Digits"];
        for (index, (rgba, label)) in icons.into_iter().zip(labels).enumerate() {
            let center_x = 190 + index as u32 * 205;
            fill_preview_rounded_rect(
                &mut hero,
                center_x - 74,
                348,
                148,
                116,
                15.0,
                Rgba([35, 48, 69, 255]),
            );
            let icon = RgbaImage::from_raw(ICON_RENDER_SIZE, ICON_RENDER_SIZE, rgba)
                .expect("renderer returns a complete RGBA icon");
            let icon = resize(&icon, 64, 64, FilterType::Nearest);
            overlay(&mut hero, &icon, i64::from(center_x - 32), 354);
            draw_preview_text(
                &mut hero,
                label,
                center_x as f32,
                425.0,
                12.0,
                [219, 228, 240],
            )?;
        }
        draw_preview_text(
            &mut hero,
            "Theme-aware icons  ·  configurable alerts  ·  nothing uploaded",
            396.0,
            480.0,
            13.0,
            [178, 193, 214],
        )?;
        draw_preview_text(
            &mut hero,
            "LOW-BATTERY ALERTS",
            396.0,
            528.0,
            12.0,
            [99, 217, 154],
        )?;

        let asset_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("docs");
        fill_preview_rounded_rect(&mut hero, 118, 548, 556, 142, 14.0, Rgba([40, 50, 67, 255]));
        let alert_icon =
            battery_rgba_with_percentage(5, ICON_RENDER_SIZE, color_map, IconTheme::Dark, false)?;
        let alert_icon = RgbaImage::from_raw(ICON_RENDER_SIZE, ICON_RENDER_SIZE, alert_icon)
            .expect("renderer returns a complete RGBA icon");
        let alert_icon = resize(&alert_icon, 42, 42, FilterType::Lanczos3);
        overlay(&mut hero, &alert_icon, 139, 563);
        draw_preview_text(
            &mut hero,
            "VOLTY  ·  BATTERY ALERT",
            432.0,
            563.0,
            11.0,
            [179, 194, 214],
        )?;
        draw_preview_text(
            &mut hero,
            "Wireless mouse is at 5% battery.",
            432.0,
            591.0,
            16.0,
            [248, 250, 253],
        )?;
        draw_preview_text(
            &mut hero,
            "Connect charging cable to keep playing.",
            432.0,
            620.0,
            12.0,
            [190, 201, 218],
        )?;

        let dashboard_path = asset_dir.join("dashboard.png");
        let dashboard = image::open(&dashboard_path)
            .map_err(|error| {
                TrayError::new(format!(
                    "could not load README dashboard preview {}: {error}",
                    dashboard_path.display()
                ))
            })?
            .to_rgba8();
        let crop_height = dashboard.height().min(935);
        let dashboard_crop =
            image::imageops::crop_imm(&dashboard, 0, 0, dashboard.width(), crop_height).to_image();
        let dashboard_preview = resize(&dashboard_crop, 742, 482, FilterType::Lanczos3);
        fill_preview_rounded_rect(&mut hero, 772, 232, 762, 510, 20.0, Rgba([49, 63, 84, 255]));
        overlay(&mut hero, &dashboard_preview, 782, 246);
        draw_preview_text(
            &mut hero,
            "BATTERY HISTORY  ·  ESTIMATES  ·  DEVICE HEALTH",
            1152.0,
            207.0,
            12.0,
            [178, 193, 214],
        )?;
        draw_preview_text(
            &mut hero,
            "Open source  ·  Windows 10/11  ·  HID + XInput + Bluetooth",
            800.0,
            758.0,
            13.0,
            [178, 193, 214],
        )?;
        Ok(hero)
    }

    fn fill_preview_rect(
        image: &mut RgbaImage,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        color: Rgba<u8>,
    ) {
        for pixel_y in y..y + height {
            for pixel_x in x..x + width {
                image.put_pixel(pixel_x, pixel_y, color);
            }
        }
    }

    fn fill_preview_rounded_rect(
        image: &mut RgbaImage,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        radius: f32,
        color: Rgba<u8>,
    ) {
        let rect = RoundedRect {
            x: x as f32,
            y: y as f32,
            width: width as f32,
            height: height as f32,
            radius,
        };
        for pixel_y in y..(y + height).min(image.height()) {
            for pixel_x in x..(x + width).min(image.width()) {
                if inside_rounded_rect(pixel_x as f32 + 0.5, pixel_y as f32 + 0.5, rect) {
                    image.put_pixel(pixel_x, pixel_y, color);
                }
            }
        }
    }

    fn draw_preview_text(
        image: &mut RgbaImage,
        text: &str,
        center_x: f32,
        top_y: f32,
        font_size: f32,
        color: [u8; 3],
    ) -> Result<(), TrayError> {
        let font = percentage_font()?;
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

        let origin_x = center_x - (left + right) / 2.0;
        let origin_y = top_y - top;
        let mut mask = vec![0; (image.width() * image.height()) as usize];
        for (metrics, bitmap, pen_x) in glyphs {
            let glyph_x = (origin_x + pen_x + metrics.xmin as f32).round() as i32;
            let glyph_y = (origin_y - metrics.ymin as f32 - metrics.height as f32).round() as i32;
            for row in 0..metrics.height {
                for column in 0..metrics.width {
                    let alpha = bitmap[row * metrics.width + column];
                    let pixel_x = glyph_x + column as i32;
                    let pixel_y = glyph_y + row as i32;
                    if alpha > 0
                        && pixel_x >= 0
                        && pixel_x < image.width() as i32
                        && pixel_y >= 0
                        && pixel_y < image.height() as i32
                    {
                        let index = pixel_y as usize * image.width() as usize + pixel_x as usize;
                        mask[index] = mask[index].max(alpha);
                    }
                }
            }
        }
        composite_mask(image, &mask, color);
        Ok(())
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
        for theme in [IconTheme::Light, IconTheme::Dark] {
            for (percentage, expected) in [
                (map.red_max_percent, map.red),
                (map.yellow_max_percent, map.yellow),
                (100, map.green),
            ] {
                let rgba = render_percentage(percentage, 32, map, theme)
                    .unwrap()
                    .into_raw();
                let opaque = opaque_pixels(&rgba);
                assert!(!opaque.is_empty());
                assert!(opaque.iter().any(|pixel| pixel[..3] == expected));
                let expected_outline = match theme {
                    IconTheme::Light => {
                        let outline = theme.outline_color();
                        [outline[0], outline[1], outline[2]]
                    }
                    IconTheme::Dark => [245, 247, 250],
                };
                assert!(opaque.iter().any(|pixel| pixel[..3] == expected_outline));
                assert!(opaque.iter().any(|pixel| pixel[3] < 255));
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

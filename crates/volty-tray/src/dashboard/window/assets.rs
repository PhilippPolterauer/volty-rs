use eframe::egui::{self, Color32, Context, Image, Painter, Pos2, Rect, TextureHandle, Ui, Vec2};

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Brand,
    Bolt,
    Mouse,
    Headset,
    Controller,
    Battery,
    File,
}

impl Icon {
    // Rasterized directly from the supplied guide's inline SVGs at 4× scale.
    fn texture(self, ctx: &Context) -> TextureHandle {
        let (name, bytes): (&str, &[u8]) = match self {
            Self::Brand => (
                "brand",
                include_bytes!("../../../assets/dashboard/brand.png"),
            ),
            Self::Bolt => ("bolt", include_bytes!("../../../assets/dashboard/bolt.png")),
            Self::Mouse => (
                "mouse",
                include_bytes!("../../../assets/dashboard/mouse.png"),
            ),
            Self::Headset => (
                "headset",
                include_bytes!("../../../assets/dashboard/headset.png"),
            ),
            Self::Battery => (
                "battery",
                include_bytes!("../../../assets/dashboard/battery.png"),
            ),
            Self::File => ("file", include_bytes!("../../../assets/dashboard/file.png")),
            Self::Controller => (
                "controller",
                include_bytes!("../../../assets/dashboard/pad.png"),
            ),
        };
        let id = egui::Id::unique(("dashboard asset", name));
        if let Some(texture) = ctx.data(|data| data.get_temp::<TextureHandle>(id)) {
            return texture;
        }
        let rgba = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
            .expect("embedded dashboard PNG should decode")
            .into_rgba8();
        let pixels = egui::ColorImage::from_rgba_unmultiplied(
            [rgba.width() as usize, rgba.height() as usize],
            rgba.as_raw(),
        );
        let texture = ctx.load_texture(name, pixels, egui::TextureOptions::LINEAR);
        ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
        texture
    }

    pub(super) fn show(self, ui: &mut Ui, size: f32, color: Color32) {
        ui.add(Image::new((self.texture(ui.ctx()).id(), Vec2::splat(size))).tint(color));
    }

    pub(super) fn paint(self, painter: &Painter, center: Pos2, size: f32, color: Color32) {
        painter.image(
            self.texture(painter.ctx()).id(),
            Rect::from_center_size(center, Vec2::splat(size)),
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            color,
        );
    }
}

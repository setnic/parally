use eframe::egui::{
    self, Color32, CornerRadius, Frame, Margin, Rect, RichText, Stroke, Ui, pos2, vec2,
};

static LIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn color(dark: u8, light: u8) -> Color32 {
    Color32::from_gray(if LIGHT.load(std::sync::atomic::Ordering::Relaxed) {
        light
    } else {
        dark
    })
}

pub fn muted() -> Color32 {
    color(211, 57)
}
pub fn popup() -> Color32 {
    color(42, 249)
}
pub const WINDOW_RADIUS: u8 = 24;
fn rim() -> Color32 {
    color(102, 157)
}

pub fn style(ctx: &egui::Context, light: bool) {
    LIGHT.store(light, std::sync::atomic::Ordering::Relaxed);
    let mut fonts = egui::FontDefinitions::default();
    if let Some(windows) = std::env::var_os("WINDIR") {
        let font_dir = std::path::PathBuf::from(windows).join("Fonts");
        if let Ok(font) = std::fs::read(font_dir.join("segoeui.ttf")) {
            fonts
                .font_data
                .insert("segoe".into(), egui::FontData::from_owned(font).into());
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "segoe".into());
        }
    }
    ctx.set_fonts(fonts);
    let mut visuals = if light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    visuals.override_text_color = Some(color(245, 23));
    visuals.panel_fill = Color32::TRANSPARENT;
    visuals.window_fill = popup();
    visuals.extreme_bg_color = color(27, 244);
    visuals.widgets.inactive.bg_fill = color(56, 230);
    visuals.widgets.hovered.bg_fill = color(77, 210);
    visuals.widgets.active.bg_fill = color(91, 199);
    visuals.widgets.inactive.fg_stroke.color = muted();
    visuals.selection.bg_fill = color(96, 193);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(8);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(8);
    visuals.widgets.active.corner_radius = CornerRadius::same(8);
    visuals.widgets.inactive.weak_bg_fill = color(61, 226);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, Color32::from_gray(110));
    ctx.set_visuals(visuals);
    ctx.style_mut(|style| {
        style.spacing.item_spacing = vec2(8.0, 6.0);
        style.spacing.button_padding = vec2(13.0, 7.0);
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
    });
}

pub fn background(ui: &Ui, transparency: u8) {
    let alpha = 255 - (u16::from(transparency) * 255 / 100) as u8;
    let rect = ui.max_rect().shrink(1.5);
    let shade = color(38, 237).r();
    ui.painter().rect(
        rect,
        WINDOW_RADIUS - 1,
        Color32::from_rgba_unmultiplied(shade, shade, shade, alpha),
        Stroke::new(1.0_f32, rim()),
        egui::StrokeKind::Inside,
    );
}

pub fn card() -> Frame {
    Frame::new()
        .fill(color(24, 246))
        .stroke(Stroke::new(1.0_f32, rim()))
        .corner_radius(19)
        .inner_margin(Margin::symmetric(16, 12))
        .shadow(egui::Shadow {
            offset: [0, 5],
            blur: 20,
            spread: 0,
            color: Color32::from_black_alpha(36),
        })
}

pub fn field(ui: &mut Ui, id: &str, value: &mut String, hint: &str, width: f32) -> egui::Response {
    ui.add_sized(
        [width, 34.0],
        egui::TextEdit::singleline(value)
            .id_salt(id)
            .hint_text(crate::i18n::text(hint))
            .margin(Margin::symmetric(11, 7))
            .background_color(color(15, 232)),
    )
}

pub fn button(label: &str, primary: bool, width: f32) -> egui::Button<'_> {
    let fill = if primary {
        color(235, 49)
    } else {
        color(55, 231)
    };
    let foreground = if primary {
        color(18, 249)
    } else {
        color(245, 23)
    };
    egui::Button::new(
        RichText::new(crate::i18n::text(label))
            .color(foreground)
            .strong(),
    )
    .fill(fill)
    .stroke(Stroke::new(
        1.0_f32,
        if primary {
            Color32::from_gray(251)
        } else {
            rim()
        },
    ))
    .corner_radius(11)
    .min_size(vec2(width, 34.0))
}

pub fn progress(
    ui: &mut Ui,
    fraction: Option<f32>,
    active: bool,
    segments: &[parallel_downloader::SegmentProgress],
) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), egui::Sense::hover());
    if segments.len() > 1 {
        let mut left = rect.left();
        let gap = 2.0_f32.min(rect.width() * 0.12 / (segments.len() - 1) as f32);
        let width = (rect.width() - gap * (segments.len() - 1) as f32).max(0.0);
        for (portion, filled) in segment_fractions(segments) {
            let span = width * portion;
            let track = Rect::from_min_size(pos2(left, rect.top()), vec2(span, rect.height()));
            ui.painter().rect_filled(track, 2, Color32::from_gray(103));
            if filled > 0.0 {
                let fill = Rect::from_min_size(track.min, vec2(span * filled, track.height()));
                ui.painter().rect_filled(fill, 2, Color32::WHITE);
            }
            left += span + gap;
        }
        return;
    }
    ui.painter().rect_filled(rect, 4, Color32::from_gray(103));
    let (left, width) = if let Some(fraction) = fraction {
        (0.0, rect.width() * fraction.clamp(0.0, 1.0))
    } else if active {
        let cycle = (ui.input(|i| i.time) * 0.65).fract() as f32;
        let width = rect.width() * 0.22;
        (cycle * (rect.width() - width), width)
    } else {
        (0.0, 0.0)
    };
    if width > 0.0 {
        let fill = Rect::from_min_size(rect.min + vec2(left, 0.0), vec2(width, rect.height()));
        ui.painter().rect_filled(fill, 4, Color32::WHITE);
    }
}

fn segment_fractions(
    segments: &[parallel_downloader::SegmentProgress],
) -> impl Iterator<Item = (f32, f32)> + '_ {
    let total: u64 = segments.iter().map(|segment| segment.length).sum();
    segments.iter().map(move |segment| {
        (
            segment.length as f32 / total.max(1) as f32,
            (segment.bytes as f32 / segment.length.max(1) as f32).clamp(0.0, 1.0),
        )
    })
}

pub fn window_button(ui: &mut Ui, glyph: &str, label: &str) -> egui::Response {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = vec2(4.0, 2.0);
        ui.add(
            egui::Button::new(RichText::new(glyph).size(20.0).color(color(245, 23)))
                .corner_radius(8)
                .min_size(vec2(31.0, 29.0))
                .frame_when_inactive(false),
        )
        .on_hover_text(crate::i18n::text(label))
    })
    .inner
}

pub fn logo() -> image::RgbaImage {
    image::load_from_memory_with_format(
        include_bytes!("../assets/parally.png"),
        image::ImageFormat::Png,
    )
    .expect("bundled Parally logo is a PNG")
    .into_rgba8()
}

#[cfg(windows)]
pub fn tray_icon() -> (Vec<u8>, u32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};
    // Windows draws tray icons at the small-icon size for the current DPI; use the drawing made for it.
    let png: &[u8] = match unsafe { GetSystemMetrics(SM_CXSMICON) } {
        ..=16 => include_bytes!("../assets/tray/16.png"),
        17..=20 => include_bytes!("../assets/tray/20.png"),
        21..=24 => include_bytes!("../assets/tray/24.png"),
        _ => include_bytes!("../assets/tray/32.png"),
    };
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .expect("bundled tray icon is a PNG")
        .into_rgba8();
    let size = image.width();
    (image.into_raw(), size)
}

pub fn icon_data(logo: &image::RgbaImage) -> egui::IconData {
    let small = image::imageops::resize(logo, 64, 64, image::imageops::FilterType::Lanczos3);
    egui::IconData {
        rgba: small.into_raw(),
        width: 64,
        height: 64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parallel_downloader::SegmentProgress;
    #[test]
    fn segment_width_and_fill_use_actual_range_lengths() {
        let items = [
            SegmentProgress {
                length: 5,
                bytes: 2,
            },
            SegmentProgress {
                length: 4,
                bytes: 4,
            },
            SegmentProgress {
                length: 1,
                bytes: 0,
            },
        ];
        assert_eq!(
            segment_fractions(&items).collect::<Vec<_>>(),
            [(0.5, 0.4), (0.4, 1.0), (0.1, 0.0)]
        );
    }
}

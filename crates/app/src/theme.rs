//! Look of the operator window: the dark OpenPresenter theme (docs/branding/ICON.md) and the round
//! coloured toolbar icons (Phosphor Bold, bundled; see `assets/README.md`).

use egui::{
    Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Rect, Response,
    RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
};

const ICON_FONT: &[u8] = include_bytes!("../assets/Phosphor-Bold.ttf");
const ICON_FAMILY: &str = "icons";

/// Phosphor codepoints used by the UI.
pub mod icon {
    pub const X: char = '\u{E4F6}';
    pub const TEXT_T_SLASH: char = '\u{E488}';
    pub const IMAGE: char = '\u{E2CA}';
    pub const PENCIL: char = '\u{E3AE}';
    pub const FILM_STRIP: char = '\u{E792}';
    pub const MONITOR: char = '\u{E32E}';
    pub const PROJECTOR_SCREEN: char = '\u{E658}';
    pub const DEVICE_MOBILE: char = '\u{E1E0}';
    pub const GEAR: char = '\u{E270}';
    pub const WARNING: char = '\u{E4E0}';
    pub const REPEAT: char = '\u{E3F6}';
    pub const PLUS: char = '\u{E3D4}';
    pub const ARROW_UP: char = '\u{E08E}';
    pub const ARROW_DOWN: char = '\u{E03E}';
    pub const TRASH: char = '\u{E4A6}';
    pub const FILE_PLUS: char = '\u{E236}';
    pub const FOLDER_OPEN: char = '\u{E256}';
    pub const FOLDER_PLUS: char = '\u{E258}';
    pub const CARET_RIGHT: char = '\u{E13A}';
    pub const CARET_DOWN: char = '\u{E136}';
    pub const MAGNIFYING_GLASS: char = '\u{E30C}';
    pub const STOP: char = '\u{E46C}';
    pub const SQUARES_FOUR: char = '\u{E464}';
    pub const LIST: char = '\u{E2F0}';
    pub const PLAY: char = '\u{E3D0}';
    pub const PAUSE: char = '\u{E39E}';
    pub const ARROW_COUNTER_CLOCKWISE: char = '\u{E038}';
    pub const VIDEO_CAMERA: char = '\u{E4DA}';
    pub const SNOWFLAKE: char = '\u{E5AA}';
    pub const SQUARE: char = '\u{E45E}';
    /// Stopwatch (slide timers).
    pub const TIMER: char = '\u{E492}';
}

/// The brand palette (docs/branding/ICON.md, Colours). New UI uses these: dark neutral
/// ground, white ink, one orange accent for what's live or what advances.
pub mod brand {
    use egui::Color32;
    /// Tile gradient, top and bottom.
    pub const BG_TOP: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x31);
    pub const BG_BOTTOM: Color32 = Color32::from_rgb(0x12, 0x12, 0x15);
    /// Accent gradient start (also the live text colour), and the flat midpoint for
    /// ordinary UI. (The gradient end, `#F06A1D`, is only used in the logo.)
    pub const ACCENT_START: Color32 = Color32::from_rgb(0xFF, 0xA8, 0x45);
    pub const ACCENT: Color32 = Color32::from_rgb(0xF8, 0x8A, 0x31);
    pub const INK: Color32 = Color32::WHITE;
}

/// Colours shared by the panels.
pub mod color {
    use super::brand;
    use egui::Color32;
    /// Live slide / live item: the brand orange marks what's on screen.
    pub const LIVE: Color32 = brand::ACCENT;
    pub const LIVE_TEXT: Color32 = brand::ACCENT_START;
    /// The on-air dot above the preview: red while the output follows what's live,
    /// grey while it's frozen or blanked.
    pub const ON_AIR: Color32 = Color32::from_rgb(226, 52, 46);
    pub const OFF_AIR: Color32 = Color32::from_gray(140);
    /// Section header bars.
    pub const HEADER: Color32 = Color32::from_rgb(26, 26, 31);
    pub const PANEL: Color32 = brand::BG_TOP;
    pub const LIST: Color32 = Color32::from_rgb(34, 34, 40);
    /// Selection (kept blue so "selected" never reads as "live").
    pub const ACCENT: Color32 = Color32::from_rgb(46, 112, 204);
}

/// Install the icon font and the dark theme.
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert(ICON_FAMILY.into(), FontData::from_static(ICON_FONT).into());
    fonts.families.insert(
        FontFamily::Name(ICON_FAMILY.into()),
        vec![ICON_FAMILY.into()],
    );
    // Also a fallback for ordinary text, so an icon can sit inside a label.
    if let Some(p) = fonts.families.get_mut(&FontFamily::Proportional) {
        p.push(ICON_FAMILY.into());
    }
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::Theme::Dark);
    let mut v = egui::Visuals::dark();
    v.panel_fill = color::PANEL;
    v.window_fill = Color32::from_rgb(48, 48, 55);
    v.extreme_bg_color = brand::BG_BOTTOM;
    v.faint_bg_color = Color32::from_rgb(38, 38, 45);
    v.selection.bg_fill = color::ACCENT;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.window_corner_radius = CornerRadius::same(6);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(3);
    }
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(58, 58, 66);
    v.widgets.inactive.bg_fill = Color32::from_rgb(58, 58, 66);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(72, 72, 82);
    v.widgets.hovered.bg_fill = Color32::from_rgb(72, 72, 82);
    ctx.set_visuals_of(egui::Theme::Dark, v);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = Vec2::new(6.0, 4.0);
        s.spacing.button_padding = Vec2::new(6.0, 3.0);
    });
}

pub fn icon_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(ICON_FAMILY.into()))
}

/// An icon as text, for use inside buttons and labels.
pub fn icon_text(c: char, size: f32) -> RichText {
    RichText::new(c.to_string()).font(icon_font(size))
}

/// A small icon-only button (lists, footers).
pub fn small_icon(ui: &mut Ui, c: char, hover: &str) -> Response {
    ui.add(
        egui::Button::new(icon_text(c, 13.0))
            .frame(false)
            .min_size(Vec2::splat(20.0)),
    )
    .on_hover_text(hover)
}

/// A toolbar button: a coloured disc with a white glyph and a caption under it.
/// `active` draws a highlight (e.g. a layer is cleared, output is on).
pub fn tool_button(
    ui: &mut Ui,
    glyph: char,
    caption: &str,
    disc: Color32,
    active: bool,
    enabled: bool,
) -> Response {
    // Wide enough for the caption ("Video/Image").
    let caption_w = ui
        .painter()
        .layout_no_wrap(
            caption.to_owned(),
            FontId::proportional(11.0),
            Color32::WHITE,
        )
        .size()
        .x;
    let size = Vec2::new((caption_w + 14.0).max(58.0), 54.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let p = ui.painter();
    if active {
        p.rect_filled(rect, 5.0, Color32::from_rgb(70, 70, 76));
        p.rect_stroke(rect, 5.0, Stroke::new(1.0, disc), StrokeKind::Inside);
    } else if resp.hovered() && enabled {
        p.rect_filled(rect, 5.0, Color32::from_rgb(56, 56, 60));
    }
    let alpha = if enabled { 255 } else { 90 };
    let disc = Color32::from_rgba_unmultiplied(disc.r(), disc.g(), disc.b(), alpha);
    let center = rect.center_top() + Vec2::new(0.0, 19.0);
    let pressed = resp.is_pointer_button_down_on();
    let r = if pressed { 13.0 } else { 14.0 };
    p.circle_filled(center, r, disc);
    p.text(
        center,
        Align2::CENTER_CENTER,
        glyph,
        icon_font(15.0),
        Color32::from_white_alpha(alpha),
    );
    p.text(
        rect.center_bottom() - Vec2::new(0.0, 6.0),
        Align2::CENTER_BOTTOM,
        caption,
        FontId::proportional(11.0),
        Color32::from_gray(if enabled { 215 } else { 110 }),
    );
    resp
}

/// A dark title bar across the top of a panel section, with room for controls on the
/// right (search field, buttons).
pub fn section_header<R>(ui: &mut Ui, title: &str, right: impl FnOnce(&mut Ui) -> R) -> R {
    let height = 26.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, color::HEADER);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(
                rect.min + Vec2::new(8.0, 0.0),
                rect.max - Vec2::new(6.0, 0.0),
            ))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.label(RichText::new(title).strong().color(Color32::from_gray(225)));
    child
        .with_layout(egui::Layout::right_to_left(egui::Align::Center), right)
        .inner
}

/// A heading inside a menu: a dark band with small capitals in the accent colour, so it
/// reads as a title, not an item to click.
pub fn menu_heading(ui: &mut Ui, title: &str) {
    let h = 22.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
    let band = rect.expand2(Vec2::new(ui.spacing().menu_margin.left as f32, 0.0));
    ui.painter().rect_filled(band, 0.0, color::HEADER);
    ui.painter().text(
        rect.left_center() + Vec2::new(2.0, 0.0),
        Align2::LEFT_CENTER,
        title.to_uppercase(),
        FontId::proportional(11.0),
        brand::ACCENT_START,
    );
}

const LOGO_PNG: &[u8] = include_bytes!("../assets/openpresenter-64.png");
/// The full GPL-3.0 text, shown from Settings (GPL section 0, "Appropriate Legal Notices").
pub const LICENSE_TEXT: &str = include_str!("../../../LICENSE");

/// The program's logo as a texture, decoded once per context.
fn logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let id = egui::Id::unique("openpresenter-logo");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(t);
    }
    let img = image::load_from_memory(LOGO_PNG).ok()?.into_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let tex = ctx.load_texture(
        "openpresenter-logo",
        egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()),
        egui::TextureOptions::LINEAR,
    );
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    Some(tex)
}

/// Logo, name, version and the licence notice (Settings → General).
pub fn brand_header(ui: &mut Ui, version: &str) {
    ui.horizontal(|ui| {
        if let Some(tex) = logo(ui.ctx()) {
            ui.add(egui::Image::new((tex.id(), Vec2::splat(48.0))));
        }
        ui.vertical(|ui| {
            ui.label(
                RichText::new("OpenPresenter")
                    .size(20.0)
                    .strong()
                    .color(brand::INK),
            );
            ui.weak(format!("Version {version}"));
        });
    });
    ui.label(
        RichText::new(
            "Copyright (C) 2026 John Peterson. Free software under the GNU General Public \
             License v3.0: you may share and change it under its terms. It comes with \
             NO WARRANTY.",
        )
        .small(),
    );
    egui::CollapsingHeader::new("GNU General Public License v3.0")
        .id_salt("settings-license")
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .id_salt("settings-license-text")
                .show(ui, |ui| {
                    ui.label(RichText::new(LICENSE_TEXT).monospace().small());
                });
        });
}

//! Look of the operator window: a dark, ProPresenter 6-like theme and the round
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
    pub const FLOPPY_DISK: char = '\u{E248}';
    pub const MAGNIFYING_GLASS: char = '\u{E30C}';
    pub const STOP: char = '\u{E46C}';
    pub const SQUARES_FOUR: char = '\u{E464}';
}

/// Colours shared by the panels.
pub mod color {
    use egui::Color32;
    /// Live slide / live item (PP6 uses orange for what's on screen).
    pub const LIVE: Color32 = Color32::from_rgb(255, 150, 30);
    pub const LIVE_TEXT: Color32 = Color32::from_rgb(255, 175, 70);
    /// Section header bars.
    pub const HEADER: Color32 = Color32::from_rgb(30, 30, 32);
    pub const PANEL: Color32 = Color32::from_rgb(42, 42, 45);
    pub const LIST: Color32 = Color32::from_rgb(34, 34, 36);
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
    v.window_fill = Color32::from_rgb(48, 48, 51);
    v.extreme_bg_color = Color32::from_rgb(24, 24, 26);
    v.faint_bg_color = Color32::from_rgb(40, 40, 43);
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
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(58, 58, 62);
    v.widgets.inactive.bg_fill = Color32::from_rgb(58, 58, 62);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(72, 72, 78);
    v.widgets.hovered.bg_fill = Color32::from_rgb(72, 72, 78);
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

/// A PP6-style toolbar button: a coloured disc with a white glyph and a caption under it.
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

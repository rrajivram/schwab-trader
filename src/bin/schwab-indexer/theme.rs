//! Visual identity: palette (light + dark, following the OS), typefaces
//! (Public Sans for UI, IBM Plex Mono for tickers and figures so digits
//! line up), egui style, and the few shared components built on them.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, CursorIcon, FontData, FontDefinitions, FontFamily, FontId, Margin, Response, RichText,
    Sense, Stroke, TextStyle, Theme, ThemePreference, Vec2,
};

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub surface: Color32,
    pub surface_alt: Color32,
    pub border: Color32,
    pub ink: Color32,
    pub muted: Color32,
    pub accent: Color32,
    /// Text drawn on an accent fill.
    pub on_accent: Color32,
    pub accent_soft: Color32,
    pub gain: Color32,
    pub gain_soft: Color32,
    pub loss: Color32,
    pub loss_soft: Color32,
    pub warn: Color32,
    pub warn_soft: Color32,
    pub neutral_soft: Color32,
}

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Ledger paper: a faintly green-grey ground, navy ink, deep-blue accent.
pub const LIGHT: Palette = Palette {
    bg: hex(0xF3F4F1),
    surface: hex(0xFFFFFF),
    surface_alt: hex(0xF7F8F6),
    border: hex(0xDDE1E3),
    ink: hex(0x1B2433),
    muted: hex(0x667085),
    accent: hex(0x1F5F8B),
    on_accent: hex(0xFFFFFF),
    accent_soft: hex(0xE3EEF5),
    gain: hex(0x1E8E5A),
    gain_soft: hex(0xE2F3EA),
    loss: hex(0xC4453C),
    loss_soft: hex(0xF8E4E1),
    warn: hex(0xB7791F),
    warn_soft: hex(0xF7EDD8),
    neutral_soft: hex(0xECEEF0),
};

pub const DARK: Palette = Palette {
    bg: hex(0x12161B),
    surface: hex(0x1A2027),
    surface_alt: hex(0x1F262E),
    border: hex(0x2C353F),
    ink: hex(0xE4E8EE),
    muted: hex(0x8C96A6),
    accent: hex(0x6AAED6),
    on_accent: hex(0x0E1A24),
    accent_soft: hex(0x1D3342),
    gain: hex(0x4CC28A),
    gain_soft: hex(0x173228),
    loss: hex(0xEE7468),
    loss_soft: hex(0x3A1E1C),
    warn: hex(0xE0A84A),
    warn_soft: hex(0x372B16),
    neutral_soft: hex(0x262E37),
};

pub fn pal(ui: &egui::Ui) -> &'static Palette {
    if ui.visuals().dark_mode { &DARK } else { &LIGHT }
}

// Type scale.
pub const SMALL: f32 = 11.5;
pub const BODY: f32 = 13.5;
pub const TITLE: f32 = 16.0;
pub const HEADING: f32 = 20.0;

pub fn sans(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}
pub fn sans_medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("sans-medium".into()))
}
pub fn sans_semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("sans-semibold".into()))
}
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}
pub fn mono_medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("mono-medium".into()))
}
pub fn mono_semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("mono-semibold".into()))
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    // egui's bundled fonts stay as fallbacks for symbols (▲ ▼ ⇅ ✓ …).
    let fallbacks = fonts.families[&FontFamily::Proportional].clone();
    let faces: [(&str, &'static [u8]); 6] = [
        ("PublicSans-Regular", include_bytes!("../../../assets/fonts/PublicSans-Regular.ttf")),
        ("PublicSans-Medium", include_bytes!("../../../assets/fonts/PublicSans-Medium.ttf")),
        ("PublicSans-SemiBold", include_bytes!("../../../assets/fonts/PublicSans-SemiBold.ttf")),
        ("IBMPlexMono-Regular", include_bytes!("../../../assets/fonts/IBMPlexMono-Regular.ttf")),
        ("IBMPlexMono-Medium", include_bytes!("../../../assets/fonts/IBMPlexMono-Medium.ttf")),
        ("IBMPlexMono-SemiBold", include_bytes!("../../../assets/fonts/IBMPlexMono-SemiBold.ttf")),
    ];
    for (name, bytes) in faces {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    // Public Sans lacks ✓ → ←; IBM Plex Mono has them, so it backs up the
    // sans families before egui's own fonts.
    let family = |primary: &str| {
        let mut v = vec![primary.to_string()];
        if !primary.starts_with("IBMPlexMono") {
            v.push("IBMPlexMono-Regular".to_string());
        }
        v.extend(fallbacks.iter().cloned());
        v
    };
    fonts.families.insert(FontFamily::Proportional, family("PublicSans-Regular"));
    fonts.families.insert(FontFamily::Monospace, family("IBMPlexMono-Regular"));
    fonts.families.insert(FontFamily::Name("sans-medium".into()), family("PublicSans-Medium"));
    fonts.families.insert(FontFamily::Name("sans-semibold".into()), family("PublicSans-SemiBold"));
    fonts.families.insert(FontFamily::Name("mono-medium".into()), family("IBMPlexMono-Medium"));
    fonts.families.insert(FontFamily::Name("mono-semibold".into()), family("IBMPlexMono-SemiBold"));
    ctx.set_fonts(fonts);

    ctx.style_mut_of(Theme::Light, |s| apply(s, &LIGHT, false));
    ctx.style_mut_of(Theme::Dark, |s| apply(s, &DARK, true));
    ctx.set_theme(ThemePreference::System);
}

fn apply(style: &mut egui::Style, p: &Palette, dark: bool) {
    style.text_styles = [
        (TextStyle::Small, sans(SMALL)),
        (TextStyle::Body, sans(BODY)),
        (TextStyle::Button, sans_medium(BODY)),
        (TextStyle::Heading, sans_semibold(HEADING)),
        (TextStyle::Monospace, mono(13.0)),
    ]
    .into();

    let s = &mut style.spacing;
    s.item_spacing = Vec2::new(8.0, 6.0);
    s.button_padding = Vec2::new(10.0, 5.0);
    s.interact_size.y = 26.0;
    s.window_margin = Margin::same(18);

    let v = &mut style.visuals;
    *v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let radius = CornerRadius::same(6);
    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(10);
    v.extreme_bg_color = p.surface;
    v.faint_bg_color = p.surface_alt;
    v.code_bg_color = p.surface_alt;
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.loss;
    v.weak_text_color = Some(p.muted);
    v.selection.bg_fill = p.accent_soft;
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.collapsing_header_frame = false;
    v.indent_has_left_vline = false;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.ink);
    w.noninteractive.corner_radius = radius;
    for (state, fill, stroke) in [
        (&mut w.inactive, p.surface, p.border),
        (&mut w.hovered, p.surface_alt, p.accent),
        (&mut w.active, p.accent_soft, p.accent),
        (&mut w.open, p.surface_alt, p.accent),
    ] {
        state.weak_bg_fill = fill;
        state.bg_fill = if fill == p.surface { p.neutral_soft } else { fill };
        state.bg_stroke = Stroke::new(1.0, stroke);
        state.fg_stroke = Stroke::new(1.0, p.ink);
        state.corner_radius = radius;
        state.expansion = 0.0;
    }
}

// ── Components ────────────────────────────────────────────────────────────

/// Small uppercase section label.
pub fn eyebrow(ui: &egui::Ui, text: &str) -> RichText {
    RichText::new(text.to_uppercase()).font(sans_semibold(SMALL - 0.5)).color(pal(ui).muted).extra_letter_spacing(0.8)
}

/// The one prominent action in a region.
pub fn primary(ui: &egui::Ui, text: &str) -> egui::Button<'static> {
    let p = pal(ui);
    egui::Button::new(RichText::new(text).font(sans_semibold(BODY)).color(p.on_accent))
        .fill(p.accent)
        .stroke(Stroke::NONE)
        .min_size(Vec2::new(0.0, 30.0))
}

/// Rounded status/tag label.
pub fn pill(ui: &mut egui::Ui, text: &str, fg: Color32, bg: Color32) -> Response {
    egui::Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::symmetric(7, 1))
        .show(ui, |ui| ui.label(RichText::new(text).font(sans_semibold(SMALL)).color(fg)))
        .response
}

pub enum Tone {
    Neutral,
    Accent,
    Gain,
    Loss,
    Warn,
}

pub fn tone_pill(ui: &mut egui::Ui, text: &str, tone: Tone) -> Response {
    let p = pal(ui);
    let (fg, bg) = match tone {
        Tone::Neutral => (p.muted, p.neutral_soft),
        Tone::Accent => (p.accent, p.accent_soft),
        Tone::Gain => (p.gain, p.gain_soft),
        Tone::Loss => (p.loss, p.loss_soft),
        Tone::Warn => (p.warn, p.warn_soft),
    };
    pill(ui, text, fg, bg)
}

/// White card with a hairline border, for grouped content.
pub fn card(ui: &egui::Ui) -> egui::Frame {
    let p = pal(ui);
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(14))
}

/// Label + figure, used in summary strips.
pub fn stat(ui: &mut egui::Ui, label: &str, value: &str, color: Option<Color32>) {
    let p = pal(ui);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.label(eyebrow(ui, label));
        ui.label(RichText::new(value).font(mono_semibold(TITLE)).color(color.unwrap_or(p.ink)));
    });
}

/// A clickable column header that looks clickable: a stacked pair of
/// triangles (both faint when sortable, the active one in the accent color
/// when sorted), pointer cursor, underline on hover, and a tooltip.
/// `active` is Some(ascending) for the sorted column. The triangles are
/// painted rather than typed: none of the bundled fonts has ▲ ▼ ⇅.
pub fn sort_header(ui: &mut egui::Ui, label: &str, active: Option<bool>, hint: &str) -> bool {
    let p = pal(ui);
    let label_color = if active.is_some() { p.accent } else { p.ink };
    let inner = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        let text = ui.add(egui::Label::new(RichText::new(label).font(sans_semibold(SMALL + 0.5)).color(label_color)).selectable(false));
        let (rect, _) = ui.allocate_exact_size(Vec2::new(7.0, 12.0), Sense::hover());
        (text.rect, rect)
    });
    let (text_rect, arrows) = inner.inner;
    let resp = ui
        .interact(inner.response.rect, ui.id().with(("sort", label)), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);

    let faint = p.muted.gamma_multiply(0.45);
    let (up, down) = match active {
        Some(true) => (p.accent, Color32::TRANSPARENT),
        Some(false) => (Color32::TRANSPARENT, p.accent),
        None => (faint, faint),
    };
    let c = arrows.center();
    let painter = ui.painter();
    painter.add(egui::Shape::convex_polygon(
        vec![egui::pos2(c.x, c.y - 5.5), egui::pos2(c.x + 3.5, c.y - 1.0), egui::pos2(c.x - 3.5, c.y - 1.0)],
        up,
        Stroke::NONE,
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![egui::pos2(c.x - 3.5, c.y + 1.0), egui::pos2(c.x + 3.5, c.y + 1.0), egui::pos2(c.x, c.y + 5.5)],
        down,
        Stroke::NONE,
    ));
    if resp.hovered() {
        let y = text_rect.bottom() + 1.0;
        painter.line_segment([egui::pos2(text_rect.left(), y), egui::pos2(text_rect.right(), y)], Stroke::new(1.0, label_color));
    }

    let tip = match active {
        Some(true) => format!("Sorted by {label}, low to high. Click to reverse."),
        Some(false) => format!("Sorted by {label}, high to low. Click to reverse."),
        None => format!("Click to sort by {label}"),
    };
    let tip = if hint.is_empty() { tip } else { format!("{hint}\n{tip}") };
    resp.on_hover_text(tip).clicked()
}

/// On/off button for row actions (Add / In basket, Discard, Block) —
/// labelled so what a click does is never a guess.
pub fn toggle_button(
    ui: &mut egui::Ui,
    on: bool,
    off_label: &str,
    on_label: &str,
    tone_on: Tone,
    enabled: bool,
) -> Response {
    let p = pal(ui);
    let (fg, bg, stroke) = if on {
        match tone_on {
            Tone::Loss => (p.loss, p.loss_soft, p.loss),
            Tone::Warn => (p.warn, p.warn_soft, p.warn),
            _ => (p.accent, p.accent_soft, p.accent),
        }
    } else {
        (p.muted, p.surface, p.border)
    };
    let text = RichText::new(if on { on_label } else { off_label }).font(sans_medium(SMALL)).color(fg);
    ui.add_enabled(
        enabled,
        egui::Button::new(text)
            .fill(bg)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(CornerRadius::same(11))
            .min_size(Vec2::new(0.0, 20.0)),
    )
    .on_hover_cursor(CursorIcon::PointingHand)
}

/// Thin horizontal bar showing `fraction` of the full width.
pub fn weight_bar(ui: &mut egui::Ui, fraction: f32, width: f32) {
    let p = pal(ui);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 5.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(3), p.neutral_soft);
    let mut fill = rect;
    fill.set_width((width * fraction.clamp(0.0, 1.0)).max(2.0));
    painter.rect_filled(fill, CornerRadius::same(3), p.accent);
}

/// Colored stripe on a table cell's left edge (held position gain/loss).
pub fn left_stripe(ui: &egui::Ui, color: Color32) {
    // Inside the cell: painting outside it is clipped away.
    let r = ui.max_rect();
    let stripe = egui::Rect::from_min_size(r.left_top(), Vec2::new(3.0, r.height()));
    ui.painter().rect_filled(stripe, CornerRadius::same(1), color);
}

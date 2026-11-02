//! Look and feel + small painted widgets for the companion window.

use crate::model::Tier;
use egui::{
    vec2, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Rect, Response, Rounding, Sense, Stroke,
    TextStyle, Ui, Vec2,
};

pub const BG: Color32 = Color32::from_rgb(0x0d, 0x10, 0x16);
pub const PANEL: Color32 = Color32::from_rgb(0x12, 0x16, 0x1e);
pub const CARD: Color32 = Color32::from_rgb(0x17, 0x1c, 0x26);
pub const CARD_HI: Color32 = Color32::from_rgb(0x1f, 0x26, 0x33);
pub const BORDER: Color32 = Color32::from_rgb(0x26, 0x2e, 0x3b);
pub const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe9, 0xef);
pub const DIM: Color32 = Color32::from_rgb(0x8b, 0x95, 0xa3);
pub const FAINT: Color32 = Color32::from_rgb(0x5d, 0x66, 0x73);
pub const ACCENT: Color32 = Color32::from_rgb(0x5b, 0x8c, 0xff);
pub const GOOD: Color32 = Color32::from_rgb(0x4a, 0xde, 0x80);
pub const BAD: Color32 = Color32::from_rgb(0xf8, 0x71, 0x71);
pub const WARN: Color32 = Color32::from_rgb(0xfb, 0x92, 0x3c);

pub fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn semi(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn reg(size: f32) -> FontId {
    FontId::proportional(size)
}

/// Segoe UI from Windows (falls back to egui's built-in fonts) + a dark theme.
pub fn setup(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let dir = std::path::Path::new(r"C:\Windows\Fonts");
    let mut add = |key: &str, file: &str| -> bool {
        match std::fs::read(dir.join(file)) {
            Ok(bytes) => {
                fonts.font_data.insert(key.into(), FontData::from_owned(bytes));
                true
            }
            Err(_) => false,
        }
    };
    let has_reg = add("segoe", "segoeui.ttf");
    let has_sb = add("segoe_sb", "seguisb.ttf");
    let has_b = add("segoe_b", "segoeuib.ttf");
    let has_sym = add("segoe_sym", "seguisym.ttf");
    let base = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let family = |first: Option<&str>| {
        let mut v: Vec<String> = first.into_iter().map(String::from).collect();
        if has_reg {
            v.push("segoe".into());
        }
        if has_sym {
            v.push("segoe_sym".into());
        }
        v.extend(base.iter().cloned());
        v
    };
    let proportional = family(None);
    let semibold = family(has_sb.then_some("segoe_sb"));
    let boldf = family(has_b.then_some("segoe_b"));
    fonts.families.insert(FontFamily::Proportional, proportional);
    fonts.families.insert(FontFamily::Name("semibold".into()), semibold);
    fonts.families.insert(FontFamily::Name("bold".into()), boldf);
    ctx.set_fonts(fonts);

    ctx.style_mut(|s| {
        s.text_styles.insert(TextStyle::Body, reg(14.0));
        s.text_styles.insert(TextStyle::Button, semi(14.0));
        s.text_styles.insert(TextStyle::Small, reg(12.0));
        s.text_styles.insert(TextStyle::Heading, bold(22.0));
        s.spacing.item_spacing = vec2(8.0, 6.0);
        s.spacing.button_padding = vec2(12.0, 6.0);
        s.spacing.scroll.bar_width = 8.0;
        let v = &mut s.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = BG;
        v.window_fill = PANEL;
        v.extreme_bg_color = Color32::from_rgb(0x0a, 0x0c, 0x11);
        v.faint_bg_color = CARD;
        v.override_text_color = Some(TEXT);
        v.selection.bg_fill = ACCENT.linear_multiply(0.6);
        v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
        v.window_rounding = Rounding::same(10.0);
        v.window_stroke = Stroke::new(1.0_f32, BORDER);
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.rounding = Rounding::same(6.0);
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
        v.widgets.inactive.weak_bg_fill = CARD_HI;
        v.widgets.inactive.bg_fill = CARD_HI;
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x2a, 0x33, 0x44);
        v.widgets.hovered.bg_fill = Color32::from_rgb(0x2a, 0x33, 0x44);
        v.widgets.active.weak_bg_fill = ACCENT.linear_multiply(0.5);
    });
}

/// A coloured rounded label, painted at `pos` (left-centre). Returns its width.
pub fn paint_pill(ui: &Ui, pos: egui::Pos2, text: &str, font: FontId, bg: Color32, fg: Color32) -> f32 {
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, fg);
    let pad = vec2(galley.size().y * 0.38, galley.size().y * 0.08);
    let size = galley.size() + pad * 2.0;
    let rect = Rect::from_min_size(egui::pos2(pos.x, pos.y - size.y / 2.0), size);
    ui.painter().rect_filled(rect, Rounding::same(4.0), bg);
    ui.painter().galley(rect.min + pad, galley, fg);
    size.x
}

pub fn tier_pill_at(ui: &Ui, pos: egui::Pos2, tier: Tier, size: f32) -> f32 {
    let t = tier.text_rgb();
    paint_pill(ui, pos, tier.label(), bold(size), rgb(tier.rgb()), rgb(t))
}

/// Tier pill as a layout widget.
pub fn tier_pill(ui: &mut Ui, tier: Tier, size: f32) -> Response {
    let galley = ui.painter().layout_no_wrap(tier.label().into(), bold(size), Color32::BLACK);
    let w = galley.size().x + galley.size().y * 0.76;
    let (rect, resp) = ui.allocate_exact_size(vec2(w.max(size * 1.7), galley.size().y * 1.16), Sense::hover());
    let painted = tier_pill_at(ui, rect.left_center(), tier, size);
    let _ = painted;
    resp
}

pub fn paint_icon(ui: &Ui, rect: Rect, tex: Option<egui::TextureId>, rounding: f32) {
    match tex {
        Some(id) => {
            egui::Image::new(egui::load::SizedTexture::new(id, rect.size())).rounding(rounding).paint_at(ui, rect);
        }
        None => {
            ui.painter().rect_filled(rect, Rounding::same(rounding), CARD_HI);
        }
    }
}

pub fn icon(ui: &mut Ui, tex: Option<egui::TextureId>, size: f32, rounding: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_icon(ui, rect, tex, rounding);
    resp
}

pub fn section(ui: &mut Ui, title: &str) {
    ui.add_space(14.0);
    ui.label(egui::RichText::new(title).font(bold(12.5)).color(DIM));
    ui.add_space(4.0);
}

pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::none()
        .fill(CARD)
        .rounding(Rounding::same(8.0))
        .stroke(Stroke::new(1.0_f32, BORDER))
        .inner_margin(egui::Margin::same(12.0))
        .show(ui, add)
        .inner
}

pub fn pct(wr: f64) -> String {
    if wr.is_finite() {
        format!("{:.1}%", wr * 100.0)
    } else {
        "–".into()
    }
}

pub fn delta_text(d: f64) -> (String, Color32) {
    let s = format!("{}{:.1}%", if d >= 0.0 { "+" } else { "−" }, d.abs() * 100.0);
    let c = if d > 0.005 {
        GOOD
    } else if d < -0.005 {
        BAD
    } else {
        DIM
    };
    (s, c)
}

pub fn text_at(ui: &Ui, pos: egui::Pos2, align: Align2, text: &str, font: FontId, color: Color32) -> Rect {
    ui.painter().text(pos, align, text, font, color)
}

//! 主题：浅色底 + 白色圆角卡片 + 紫色主色（参考现代客户端风格）。

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Visuals};

pub const BG: Color32 = Color32::from_rgb(0xF4, 0xF5, 0xF9);
pub const CARD: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
pub const TEXT: Color32 = Color32::from_rgb(0x17, 0x1A, 0x23);
pub const FAINT: Color32 = Color32::from_rgb(0xB9, 0xBE, 0xCA);
pub const DIM: Color32 = Color32::from_rgb(0x95, 0x9B, 0xAA);
pub const ACCENT: Color32 = Color32::from_rgb(0x7C, 0x5C, 0xFC);
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(0xF0, 0xEC, 0xFF);
pub const OK: Color32 = Color32::from_rgb(0x12, 0xB7, 0x6A);
pub const WARN: Color32 = Color32::from_rgb(0xF7, 0x90, 0x09);
pub const DANGER: Color32 = Color32::from_rgb(0xF0, 0x44, 0x38);
pub const FIELD: Color32 = Color32::from_rgb(0xF4, 0xF6, 0xFA);
pub const BORDER: Color32 = Color32::from_rgb(0xE6, 0xE9, 0xF1);
pub const BORDER_HI: Color32 = Color32::from_rgb(0xD5, 0xDA, 0xE6);
pub const ACCENT_HI: Color32 = Color32::from_rgb(0x8B, 0x6F, 0xFF);
pub const ACCENT_LO: Color32 = Color32::from_rgb(0x6A, 0x4C, 0xF0);
pub const HOVER: Color32 = Color32::from_rgb(0xF7, 0xF8, 0xFB);
pub const PRESS: Color32 = Color32::from_rgb(0xEF, 0xF1, 0xF6);
pub const CHIP: Color32 = Color32::from_rgb(0xF3, 0xF5, 0xF9);
pub const CHIP_HI: Color32 = Color32::from_rgb(0xE9, 0xEC, 0xF4);
pub const SCROLL: Color32 = Color32::from_rgb(0xC4, 0xCA, 0xDB);
pub const SCROLL_HI: Color32 = Color32::from_rgb(0xAD, 0xB5, 0xCA);
pub const SCROLL_LO: Color32 = Color32::from_rgb(0x96, 0x9F, 0xB8);

pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Light);
    ctx.all_styles_mut(|style| {
        let mut v = Visuals::light();
        v.panel_fill = BG;
        v.window_fill = CARD;
        v.extreme_bg_color = FIELD;
        v.faint_bg_color = Color32::from_rgb(0xF8, 0xF9, 0xFC);
        v.hyperlink_color = ACCENT;
        v.selection.bg_fill = ACCENT.gamma_multiply(0.28);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.window_corner_radius = CornerRadius::same(16);
        v.menu_corner_radius = CornerRadius::same(12);
        v.window_shadow = Shadow {
            offset: [0, 6],
            blur: 28,
            spread: 0,
            color: Color32::from_black_alpha(38),
        };
        v.popup_shadow = Shadow {
            offset: [0, 4],
            blur: 16,
            spread: 0,
            color: Color32::from_black_alpha(26),
        };

        // 控件圆角与配色
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = CornerRadius::same(9);
        }
        v.widgets.noninteractive.bg_fill = CARD;
        v.widgets.noninteractive.weak_bg_fill = CARD;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);

        v.widgets.inactive.bg_fill = SCROLL;
        v.widgets.inactive.weak_bg_fill = FIELD;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);

        v.widgets.hovered.bg_fill = SCROLL_HI;
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0xEE, 0xEA, 0xFF);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.5));
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);

        v.widgets.active.bg_fill = SCROLL_LO;
        v.widgets.active.weak_bg_fill = ACCENT_SOFT;
        v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
        v.widgets.active.fg_stroke = Stroke::new(1.0, TEXT);

        style.visuals = v;
        // 滚动条：保持静止时的浅灰色，悬停和拖动只改变宽度，不再加深。
        let mut scroll = egui::style::ScrollStyle::floating();
        scroll.bar_width = 8.0;
        scroll.floating_width = 3.5;
        scroll.floating_allocated_width = 0.0;
        scroll.bar_inner_margin = 4.0;
        scroll.bar_outer_margin = 2.0;
        scroll.dormant_background_opacity = 0.0;
        scroll.active_background_opacity = 0.0;
        scroll.interact_background_opacity = 0.0;
        scroll.dormant_handle_opacity = 0.18;
        scroll.active_handle_opacity = 0.18;
        scroll.interact_handle_opacity = 0.18;
        style.spacing.scroll = scroll;
        style.spacing.item_spacing = egui::vec2(8.0, 7.0);
        style.spacing.button_padding = egui::vec2(14.0, 7.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.tooltip_width = 280.0;
        style.interaction.tooltip_delay = 0.4;
        style.text_styles = [
            (TextStyle::Heading, FontId::new(19.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(13.5, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
        ]
        .into();
    });
}

/// 卡片背景
pub fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(11))
        .shadow(Shadow {
            offset: [0, 1],
            blur: 6,
            spread: 0,
            color: Color32::from_rgb(0x1B, 0x1F, 0x2B).gamma_multiply(0.05),
        })
}

//! 自绘小部件：卡片、主按钮、状态胶囊、徽标、输入框、细进度条。

#![allow(dead_code)] // 旧「文件夹加密」界面留下的通用小部件，部分留作备用

use egui::{Color32, CornerRadius, Margin, Rect, RichText, Sense, Stroke, Vec2};

use super::theme;

/// 统一的悬停提示：正文与快捷键分层，保留 egui 的延迟、避让和自动关闭行为。
pub fn tooltip(response: egui::Response, title: &str, shortcut: Option<&str>) -> egui::Response {
    let mut tooltip = egui::Tooltip::for_enabled(&response).gap(6.0);
    tooltip.popup = tooltip.popup
        .align(egui::emath::RectAlign::BOTTOM)
        .frame(
            egui::Frame::new()
                .fill(theme::CARD)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(10, 7))
                .shadow(egui::Shadow {
                    offset: [0, 2],
                    blur: 10,
                    spread: 0,
                    color: Color32::from_black_alpha(20),
                }),
        );
    tooltip.show(|ui| {
        ui.set_max_width(ui.spacing().tooltip_width);
        let mut text = egui::text::LayoutJob::default();
        text.append(title, 0.0, egui::TextFormat {
            font_id: egui::FontId::proportional(13.0),
            color: theme::TEXT,
            line_height: Some(18.0),
            ..Default::default()
        });
        if let Some(shortcut) = shortcut {
            text.append(shortcut, 12.0, egui::TextFormat {
                font_id: egui::FontId::proportional(12.0),
                color: theme::DIM,
                line_height: Some(18.0),
                ..Default::default()
            });
        }
        ui.add(egui::Label::new(text).wrap());
    });
    response
}

/// 画一条圆角虚线边框（egui 没有现成的虚线圆角矩形）。
fn dashed_rect(p: &egui::Painter, rect: Rect, radius: f32, stroke: Stroke) {
    let r = radius.min(rect.width() * 0.5).min(rect.height() * 0.5);
    let corners = [
        (egui::pos2(rect.max.x - r, rect.min.y + r), -90.0f32),
        (egui::pos2(rect.max.x - r, rect.max.y - r), 0.0),
        (egui::pos2(rect.min.x + r, rect.max.y - r), 90.0),
        (egui::pos2(rect.min.x + r, rect.min.y + r), 180.0),
    ];
    let mut pts: Vec<egui::Pos2> = Vec::with_capacity(40);
    for (center, start) in corners {
        for i in 0..=8 {
            let a = (start + 90.0 * (i as f32 / 8.0)).to_radians();
            pts.push(center + egui::vec2(a.cos(), a.sin()) * r);
        }
    }
    pts.push(pts[0]);
    p.add(egui::Shape::dashed_line(&pts, stroke, 5.0, 4.0));
}

/// 收件箱图标（选择文件/拖拽区域的图形）
fn inbox_icon(p: &egui::Painter, center: egui::Pos2, size: f32, color: Color32) {
    let w = size;
    let h = size * 0.8;
    let rect = Rect::from_center_size(center, Vec2::new(w, h));
    p.rect_stroke(
        rect,
        CornerRadius::same(3),
        Stroke::new(1.8, color),
        egui::StrokeKind::Inside,
    );
    let y = rect.top() + h * 0.52;
    let pts = vec![
        egui::pos2(rect.left(), y),
        egui::pos2(rect.left() + w * 0.2, y),
        egui::pos2(rect.left() + w * 0.32, y + h * 0.2),
        egui::pos2(rect.right() - w * 0.32, y + h * 0.2),
        egui::pos2(rect.right() - w * 0.2, y),
        egui::pos2(rect.right(), y),
    ];
    p.add(egui::Shape::line(pts, Stroke::new(1.8, color)));
}

/// 虚线拖拽区：整块可点击，拖拽经过时高亮。
pub fn drop_zone(
    ui: &mut egui::Ui,
    hot: bool,
    title: &str,
    subtitle: &str,
    height: f32,
) -> egui::Response {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let p = ui.painter().with_clip_rect(rect);

    let radius = 12.0;
    p.rect_filled(
        rect,
        CornerRadius::same(radius as u8),
        if hot { theme::ACCENT_SOFT } else { theme::FIELD },
    );
    dashed_rect(
        &p,
        rect.shrink(0.7),
        radius,
        Stroke::new(
            1.2,
            if hot {
                theme::ACCENT
            } else {
                theme::BORDER_HI
            },
        ),
    );

    let accent = if hot { theme::ACCENT } else { theme::ACCENT_LO };
    inbox_icon(&p, egui::pos2(rect.center().x, rect.top() + 25.0), 25.0, accent);

    let title_galley = p.layout_no_wrap(
        title.to_owned(),
        egui::FontId::proportional(12.0),
        if hot { theme::ACCENT } else { theme::TEXT },
    );
    p.galley(
        egui::pos2(
            rect.center().x - title_galley.size().x / 2.0,
            rect.top() + 44.0,
        ),
        title_galley,
        theme::TEXT,
    );

    let sub_galley = p.layout_no_wrap(
        subtitle.to_owned(),
        egui::FontId::proportional(10.5),
        theme::DIM,
    );
    p.galley(
        egui::pos2(
            rect.center().x - sub_galley.size().x / 2.0,
            rect.top() + 65.0,
        ),
        sub_galley,
        theme::DIM,
    );

    resp
}

/// 白底圆角卡片
pub fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
}

/// 卡片标题行：左侧小标题，右侧可选状态胶囊
pub fn card_title(ui: &mut egui::Ui, title: &str, badge: Option<(&str, Color32)>) {
    match badge {
        Some((text, color)) => {
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).size(13.0).strong().color(theme::TEXT));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    status_pill(ui, text, color);
                });
            });
        }
        None => {
            ui.label(RichText::new(title).size(13.0).strong().color(theme::TEXT));
        }
    }
}

/// 圆角小胶囊：一个圆点 + 文案，底色是主色的淡色
pub fn status_pill(ui: &mut egui::Ui, text: &str, color: Color32) {
    let font = egui::FontId::proportional(11.5);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, color);
    let w = galley.size().x + 26.0;
    let h = 22.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(11), color.gamma_multiply(0.12));
    p.circle_filled(
        egui::pos2(rect.left() + 11.0, rect.center().y),
        3.2,
        color,
    );
    p.galley(
        egui::pos2(rect.left() + 19.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
}

/// 品牌徽标：紫色圆角方块 + 白色挂锁

/// 主按钮（实心紫）
/// 按钮风格：主按钮（紫底白字）、次按钮（白底描边）、胶囊小按钮（浅灰无边框）、关闭叉。
#[derive(Clone, Copy, PartialEq)]
pub enum Btn {
    Primary,
    Ghost,
    Chip,
    Close,
}

/// 统一的按钮绘制：所有状态（默认/悬停/按下/禁用）都显式指定颜色，
/// 避免依赖 egui 默认的弱背景色，保证浅色主题下三处按钮完全一致。
fn paint(
    ui: &mut egui::Ui,
    text: &str,
    enabled: bool,
    width: f32,
    height: f32,
    kind: Btn,
) -> bool {
    let radius = match kind {
        Btn::Chip | Btn::Close => 8,
        _ => 11,
    };
    let (
        fill,
        fill_hover,
        fill_press,
        fill_off,
        stroke,
        stroke_hover,
        stroke_off,
        fg,
        fg_off,
    ) = match kind {
        Btn::Primary => (
            theme::ACCENT,
            theme::ACCENT_HI,
            theme::ACCENT_LO,
            theme::ACCENT_SOFT,
            Stroke::NONE,
            Stroke::NONE,
            Stroke::NONE,
            Color32::WHITE,
            theme::ACCENT.gamma_multiply(0.78),
        ),
        Btn::Ghost => (
            theme::CARD,
            theme::HOVER,
            theme::PRESS,
            theme::CARD,
            Stroke::new(1.0, theme::BORDER),
            Stroke::new(1.0, theme::BORDER_HI),
            Stroke::new(1.0, theme::BORDER),
            theme::TEXT,
            theme::FAINT,
        ),
        Btn::Chip => (
            theme::CHIP,
            theme::CHIP_HI,
            theme::CHIP_HI,
            theme::CHIP,
            Stroke::NONE,
            Stroke::NONE,
            Stroke::NONE,
            theme::TEXT,
            theme::FAINT,
        ),
        Btn::Close => (
            Color32::TRANSPARENT,
            theme::PRESS,
            theme::PRESS,
            Color32::TRANSPARENT,
            Stroke::NONE,
            Stroke::NONE,
            Stroke::NONE,
            theme::DIM,
            theme::FAINT,
        ),
    };
    let font_size = if kind == Btn::Chip { 12.0 } else { 13.5 };

    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.weak_bg_fill = fill;
        w.inactive.bg_stroke = stroke;
        w.inactive.corner_radius = CornerRadius::same(radius);

        w.hovered.weak_bg_fill = fill_hover;
        w.hovered.bg_stroke = stroke_hover;
        w.hovered.corner_radius = CornerRadius::same(radius);

        w.active.weak_bg_fill = fill_press;
        w.active.bg_stroke = stroke_hover;
        w.active.corner_radius = CornerRadius::same(radius);

        w.noninteractive.weak_bg_fill = fill_off;
        w.noninteractive.bg_stroke = stroke_off;
        w.noninteractive.corner_radius = CornerRadius::same(radius);

        let btn = egui::Button::new(
            RichText::new(text)
                .size(font_size)
                .strong()
                .color(if enabled { fg } else { fg_off }),
        )
        .min_size(Vec2::new(width, height));
        ui.add_enabled(enabled, btn).clicked()
    })
    .inner
}

/// 主按钮：紫底白字，宽度固定。
pub fn primary_button(ui: &mut egui::Ui, text: &str, enabled: bool, width: f32) -> bool {
    paint(ui, text, enabled, width, 38.0, Btn::Primary)
}

/// 次按钮：白底细描边。
pub fn ghost_button(ui: &mut egui::Ui, text: &str, enabled: bool, width: f32) -> bool {
    paint(ui, text, enabled, width, 38.0, Btn::Ghost)
}

/// 胶囊小按钮：用于「知道了」「取消」这类行内次要操作，高 26。
pub fn chip_button(ui: &mut egui::Ui, text: &str) -> bool {
    chip_button_enabled(ui, text, true)
}

/// 同上，但可置灰（例如没有内容可清空时）。
pub fn chip_button_enabled(ui: &mut egui::Ui, text: &str, enabled: bool) -> bool {
    paint(ui, text, enabled, 0.0, 26.0, Btn::Chip)
}

/// 关闭叉：透明底，悬停时出现浅灰底。
pub fn close_button(ui: &mut egui::Ui) -> bool {
    paint(ui, "✕", true, 24.0, 24.0, Btn::Close)
}

pub fn field(ui: &mut egui::Ui, value: &mut String, width: f32, hint: &str) -> egui::Response {
    let resp = ui.add(
        egui::TextEdit::singleline(value)
            .password(true)
            .desired_width(width)
            .margin(Margin::symmetric(10, 7))
            // 默认对齐是布局的 LEFT_TOP，密码圆点会贴在输入框顶部；
            // 显式设成 LEFT_CENTER 让文字在框内垂直居中。
            .align(egui::Align2::LEFT_CENTER)
            .text_color(theme::TEXT)
            .frame(
                egui::Frame::new()
                    .fill(theme::FIELD)
                    .corner_radius(CornerRadius::same(9))
                    .stroke(Stroke::new(1.0, theme::FIELD))
                    .inner_margin(Margin {
                        left: 13,
                        right: 13,
                        top: 7,
                        bottom: 7,
                    }),
            ),
    );

    // egui 的 hint_text 用 weak_text_color()（把文字颜色乘暗），浅色主题下几乎看不见，
    // 所以自己在空值时画一行浅灰占位文字。
    if value.is_empty() && !hint.is_empty() {
        ui.painter().text(
            resp.rect.left_center() + egui::vec2(13.0, 0.0),
            egui::Align2::LEFT_CENTER,
            hint,
            egui::FontId::proportional(13.0),
            theme::FAINT,
        );
    }
    resp
}

/// 细进度条（高 8，全圆角）
pub fn thin_bar(ui: &mut egui::Ui, frac: f32, color: Color32) {
    let h = 8.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(4), theme::FIELD);
    let w = (rect.width() * frac.clamp(0.0, 1.0)).max(0.0);
    if w > 0.5 {
        let bar = Rect::from_min_size(rect.min, Vec2::new(w, h));
        p.rect_filled(bar, CornerRadius::same(4), color);
    }
}

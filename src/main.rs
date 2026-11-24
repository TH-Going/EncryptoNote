#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod crypto;
mod editor;
mod format;
mod model;
mod recent;
mod settings;
mod store;
mod stream;
mod ui;

/// 初始窗口尺寸；用户可自由缩放、最大化或全屏。
const WIN_W: f32 = 1180.0;
const WIN_H: f32 = 780.0;

/// 窗口/任务栏图标（PNG 是 256×256，eframe 会按需缩放）
fn app_icon() -> egui::IconData {
    const ICON_PNG: &[u8] = include_bytes!("../app.png");
    let image = image::load_from_memory(ICON_PNG)
        .expect("内置图标损坏")
        .to_rgba8();
    let (width, height) = image.dimensions();
    egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([WIN_W, WIN_H])
            .with_min_inner_size([900.0, 600.0])
            .with_resizable(true)
            .with_drag_and_drop(true)
            .with_maximize_button(true)
            .with_icon(std::sync::Arc::new(app_icon()))
            .with_title("EncryptoNote"),
        ..Default::default()
    };

    eframe::run_native(
        "EncryptoNote",
        options,
        Box::new(|cc| {
            ui::theme::apply(&cc.egui_ctx);
            ui::fonts::install(&cc.egui_ctx);
            Ok(Box::new(app::App::default()))
        }),
    )
}

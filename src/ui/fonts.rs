//! 中文字体注入。
//!
//! egui 自带字体不含 CJK，必须挂系统字体；另外 `TextFormat` 没有 bold 位，
//! 富文本的粗体/标题通过一个名为 `bold` 的独立字体族实现——优先用
//! 微软雅黑的粗体面（msyhbd.ttc），找不到时退回普通雅黑。

use egui::{FontData, FontDefinitions, FontFamily};

const REGULAR: &[&str] = &[
    r"C:\Windows\Fonts\msyh.ttc",
    r"C:\Windows\Fonts\msyh.ttf",
    r"C:\Windows\Fonts\simhei.ttf",
    r"C:\Windows\Fonts\simsun.ttc",
];

const BOLD: &[&str] = &[
    r"C:\Windows\Fonts\msyhbd.ttc",
    r"C:\Windows\Fonts\msyhbd.ttf",
    r"C:\Windows\Fonts\simhei.ttf",
];

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    let regular = load_first(REGULAR);
    if let Some(bytes) = &regular {
        fonts
            .font_data
            .insert("cjk".to_owned(), FontData::from_owned(bytes.clone()).into());
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "cjk".to_owned());
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .push("cjk".to_owned());
    }

    if let Some(bytes) = load_first(BOLD).or(regular) {
        fonts
            .font_data
            .insert("cjk-bold".to_owned(), FontData::from_owned(bytes).into());
        fonts.families.insert(
            FontFamily::Name("bold".into()),
            vec!["cjk-bold".to_owned()],
        );
    }

    ctx.set_fonts(fonts);
}

fn load_first(paths: &[&str]) -> Option<Vec<u8>> {
    for p in paths {
        if let Ok(bytes) = std::fs::read(p) {
            return Some(bytes);
        }
    }
    None
}

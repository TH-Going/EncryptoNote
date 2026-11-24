//! EncryptoNote：加密笔记（`.ecnote`）应用主界面。

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Instant;

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Id, Layout, Margin, RichText, Sense, Stroke,
    pos2, vec2,
};
use egui_richedit::{Position, Selection};

use crate::editor::{self, ActiveSheet, SheetEditor, mark_of};
use crate::model::{Align as DocAlign, Block, BlockKind, ImageAsset, Run, Sheet};
use crate::recent::{self, RecentEntry};
use crate::store::{self, Opened};
use crate::ui::{theme, widgets};

const PAGE_SIDE: f32 = 80.0;
const PAGE_TOP: f32 = 76.0;
const PAGE_BOTTOM: f32 = 128.0;

const SHEET_COLORS: [(&str, &str); 8] = [
    ("无颜色", ""),
    ("红", "#E5484D"),
    ("橙", "#F79009"),
    ("黄", "#EAB308"),
    ("绿", "#12B76A"),
    ("青", "#06AED4"),
    ("蓝", "#3B82F6"),
    ("紫", "#7C5CFC"),
];

// ---------------------------------------------------------------------------
// 状态
// ---------------------------------------------------------------------------

enum Task {
    Create,
    Open,
}

struct Pending {
    rx: Receiver<Result<Opened, String>>,
    task: Task,
}

#[derive(Default)]
struct NewState {
    open: bool,
    name: String,
    dir: Option<PathBuf>,
    password: String,
    confirm: String,
    error: Option<String>,
}

struct UnlockState {
    path: PathBuf,
    title: String,
    password: String,
    error: Option<String>,
    busy: bool,
}

struct PwDialog {
    open: bool,
    p1: String,
    p2: String,
    error: Option<String>,
}

struct Toast {
    text: String,
    until: Instant,
}

pub struct EditorCtx {
    opened: Opened,
    sheets: Vec<SheetEditor>,
    active: usize,
    dirty: bool,
    pw: PwDialog,
    rename: Option<(usize, String, bool)>,
    delete_confirm: Option<usize>,
    /// 切表后下一帧把键盘焦点交给新表，避免「点了标签却打不了字」
    focus_editor: bool,
}

impl EditorCtx {
    fn new(opened: Opened) -> Self {
        let active = opened.doc.active_index();
        let sheets = opened
            .doc
            .sheets
            .iter()
            .map(|s| {
                let mut e = SheetEditor::new(Id::unique(("ecnote-editor", &s.id)));
                e.editor
                    .select(Selection::caret(Position::new(0usize, 0usize)));
                e
            })
            .collect();
        Self {
            opened,
            sheets,
            active,
            dirty: false,
            pw: PwDialog {
                open: false,
                p1: String::new(),
                p2: String::new(),
                error: None,
            },
            rename: None,
            delete_confirm: None,
            focus_editor: false,
        }
    }

}

#[derive(PartialEq)]
enum View {
    Library,
    Editor,
}

pub struct App {
    view: View,
    entries: Vec<RecentEntry>,
    new: NewState,
    unlock: Option<UnlockState>,
    pending: Option<Pending>,
    editor: Option<EditorCtx>,
    toast: Option<Toast>,
    new_dir: Option<PathBuf>,
    open_error: Option<String>,
}

impl Default for App {
    fn default() -> Self {
        let mut app = Self {
            view: View::Library,
            entries: recent::list(),
            new: NewState::default(),
            unlock: None,
            pending: None,
            editor: None,
            toast: None,
            new_dir: None,
            open_error: None,
        };
        if let Some(home) = std::env::var_os("ECNOTE_HOME") {
            let dir = PathBuf::from(home);
            app.new_dir = Some(dir.clone());
            app.new.dir = Some(dir);
        }
        #[cfg(debug_assertions)]
        if std::env::var_os("ECNOTE_DEMO").is_some() {
            app.demo();
        }
        app
    }
}

#[cfg(debug_assertions)]
impl App {
    /// 调试用：生成一份演示笔记并直接进入编辑器（仅在设置了 ECNOTE_DEMO 时）。
    fn demo(&mut self) {
        let dir = std::env::temp_dir().join("ecnote-demo");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("演示笔记.ecnote");
        let _ = std::fs::remove_file(&path);
        let mut opened = match store::create(&dir, "演示笔记", "demo1234") {
            Ok(o) => o,
            Err(_) => return,
        };
        {
            let doc = &mut opened.doc;
            doc.title = "演示笔记".to_owned();
            doc.sheets.clear();
            let mut s1 = Sheet::new("s1", "需求");
            s1.color = Some("#12B76A".to_owned());
            let mut h = Block::with_text("EncryptoNote · 需求清单");
            h.kind = BlockKind::H1;
            s1.blocks = vec![
                h,
                Block::with_text("这是第一段正文，支持粗体、斜体、下划线与删除线。"),
                {
                    let mut b = Block::empty();
                    b.runs = vec![
                        Run::plain("加粗的"),
                        Run {
                            text: "重点".to_owned(),
                            marks: Run::BOLD | Run::UNDERLINE,
                            color: Some("#7C5CFC".to_owned()),
                            bg: None,
                            size: None,
                            mono: false,
                        },
                        Run::plain("，以及普通文字。"),
                    ];
                    b
                },
                {
                    let mut b = Block::with_text("项目符号一：设计评审");
                    b.kind = BlockKind::Bullet;
                    b
                },
                {
                    let mut b = Block::with_text("项目符号二：接口联调");
                    b.kind = BlockKind::Bullet;
                    b
                },
                {
                    let mut b = Block::empty();
                    b.kind = BlockKind::Divider;
                    b
                },
                Block::with_text("分割线下方的数字编号："),
                {
                    let mut b = Block::with_text("第一项：加密落盘");
                    b.kind = BlockKind::Number;
                    b
                },
                {
                    let mut b = Block::with_text("第二项：内存清零");
                    b.kind = BlockKind::Number;
                    b
                },
                {
                    let mut b = Block::with_text("引用：密码只存在于内存中，落盘前一定会被加密。");
                    b.kind = BlockKind::Quote;
                    b
                },
                Block::with_text("第二张工作表演示「会议记录」，在底部切换。"),
            ];
            doc.sheets.push(s1);

            let mut s2 = Sheet::new("s2", "会议记录");
            s2.color = Some("#F79009".to_owned());
            let mut h2 = Block::with_text("2026-11-23 周会");
            h2.kind = BlockKind::H2;
            s2.blocks = vec![
                h2,
                Block::with_text("1. 完成 .ecnote 文件格式 v2 的读写。"),
                Block::with_text("2. 编辑器接入富文本与多工作表。"),
                {
                    let mut b = Block::with_text("待办：图片粘贴、快捷键打磨。");
                    b.kind = BlockKind::Number;
                    b
                },
            ];
            doc.sheets.push(s2);
            if std::env::var_os("ECNOTE_IMG_DEMO").is_some() {
                use base64::Engine;
                let mut img = image::RgbaImage::new(420, 240);
                for (x, _y, p) in img.enumerate_pixels_mut() {
                    *p = image::Rgba([(60 + x % 160) as u8, 110, 230, 255]);
                }
                let mut png = Vec::new();
                if img
                    .write_to(
                        &mut std::io::Cursor::new(&mut png),
                        image::ImageFormat::Png,
                    )
                    .is_ok()
                {
                    doc.images.push(ImageAsset {
                        id: "demo-img".to_owned(),
                        mime: "image/png".to_owned(),
                        w: 420,
                        h: 240,
                        data: base64::engine::general_purpose::STANDARD.encode(&png),
                    });
                    let mut b = Block::empty();
                    b.kind = BlockKind::Image;
                    b.runs = vec![Run::plain("demo-img")];
                    doc.sheets[0].blocks.insert(1, b);
                }
            }
            doc.active_sheet = "s1".to_owned();
        }
        if opened.save().is_err() {
            return;
        }
        let size = std::fs::metadata(&opened.path).map(|m| m.len()).unwrap_or(0);
        recent::touch(&opened.path, &opened.doc.title, size);
        if std::env::var("ECNOTE_DEMO").as_deref() == Ok("library") {
            self.refresh_entries();
            return;
        }
        self.editor = Some(EditorCtx::new(opened));
        self.view = View::Editor;
    }
}

impl App {
    fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            until: Instant::now() + std::time::Duration::from_secs(3),
        });
    }

    fn refresh_entries(&mut self) {
        self.entries = recent::list();
    }

    fn enter_editor(&mut self, opened: Opened) {
        let name = opened.file_name();
        if std::env::var("ECNOTE_DEMO").as_deref() == Ok("library") {
            self.refresh_entries();
            return;
        }
        self.editor = Some(EditorCtx::new(opened));
        self.view = View::Editor;
        self.new.open = false;
        self.unlock = None;
        self.pending = None;
        self.refresh_entries();
        self.toast(format!("已解锁 {name}"));
    }

    fn leave_editor(&mut self) {
        if let Some(ed) = &mut self.editor
            && ed.dirty
        {
            if ed.opened.save().is_err() {
                self.toast("保存失败，已留在编辑器");
                return;
            }
        }
        self.editor = None;
        self.view = View::Library;
        self.refresh_entries();
    }

    fn save_now(&mut self) {
        let mut notify: Option<String> = None;
        if let Some(ed) = &mut self.editor {
            if ed.opened.over_limit() {
                notify = Some("内容已超过 100 万字符，无法保存；请删减内容".to_owned());
            } else {
                match ed.opened.save() {
                    Ok(()) => {
                        ed.dirty = false;
                        notify = Some("已保存".to_owned());
                    }
                    Err(e) => notify = Some(format!("保存失败：{e:#}")),
                }
            }
        }
        if let Some(msg) = notify {
            self.toast(msg);
        }
        self.refresh_entries();
    }

    fn poll_pending(&mut self) {
        let Some(p) = &self.pending else { return };
        match p.rx.try_recv() {
            Ok(Ok(opened)) => {
                let task = matches!(p.task, Task::Create);
                self.pending = None;
                if let Some(ed) = &mut self.editor {
                    // 重复拖入当前笔记时保留内存中的编辑，避免用磁盘旧内容覆盖。
                    let same_file = ed.opened.path == opened.path
                        || std::fs::canonicalize(&ed.opened.path).ok()
                            .zip(std::fs::canonicalize(&opened.path).ok())
                            .is_some_and(|(current, next)| current == next);
                    if same_file {
                        ed.focus_editor = true;
                        self.unlock = None;
                        return;
                    }
                    // 新笔记验证成功后再保存、切换；验证失败不碰当前笔记。
                    if ed.dirty
                        && let Err(e) = ed.opened.save()
                    {
                        if let Some(u) = &mut self.unlock {
                            u.error = Some(format!("当前笔记保存失败：{e:#}"));
                            u.busy = false;
                        }
                        return;
                    }
                }
                if task {
                    self.toast(format!("已创建「{}」", opened.file_name()));
                }
                let size = std::fs::metadata(&opened.path).map(|m| m.len()).unwrap_or(0);
                recent::touch(&opened.path, &opened.doc.title, size);
                self.enter_editor(opened);
            }
            Ok(Err(e)) => {
                let task = matches!(p.task, Task::Create);
                self.pending = None;
                if task {
                    self.new.error = Some(e);
                } else if let Some(u) = &mut self.unlock {
                    u.error = Some(e);
                    u.busy = false;
                }
            }
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                self.toast("后台任务异常结束");
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    fn start_create(&mut self, ctx: &egui::Context) {
        let Some(dir) = self.new.dir.clone() else {
            self.new.error = Some("请先选择保存位置".to_owned());
            return;
        };
        let name = self.new.name.trim().to_owned();
        if name.is_empty() {
            self.new.error = Some("请填写文件名".to_owned());
            return;
        }
        if self.new.password.chars().count() < 6 {
            self.new.error = Some("密码至少 6 个字符".to_owned());
            return;
        }
        if self.new.password != self.new.confirm {
            self.new.error = Some("两次输入的密码不一致".to_owned());
            return;
        }
        self.new.error = None;
        self.new_dir = Some(dir.clone());
        let pw = self.new.password.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let res = store::create(&dir, &name, &pw).map_err(|e| format!("{e:#}"));
            let _ = tx.send(res);
        });
        self.pending = Some(Pending {
            rx,
            task: Task::Create,
        });
        ctx.request_repaint();
    }

    fn start_open(&mut self, ctx: &egui::Context) {
        let Some(u) = &mut self.unlock else { return };
        if u.password.is_empty() {
            u.error = Some("请输入密码".to_owned());
            return;
        }
        let path = u.path.clone();
        let pw = u.password.clone();
        u.error = None;
        u.busy = true;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let res = store::open(&path, &pw).map_err(|e| format!("{e:#}"));
            let _ = tx.send(res);
        });
        self.pending = Some(Pending {
            rx,
            task: Task::Open,
        });
        ctx.request_repaint();
    }

    fn open_path(&mut self, path: PathBuf) {
        let title = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.unlock = Some(UnlockState {
            path,
            title,
            password: String::new(),
            error: None,
            busy: false,
        });
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        if self.pending.is_some() || self.new.open
            || self.editor.as_ref().is_some_and(|ed| ed.pw.open || ed.delete_confirm.is_some())
        {
            return;
        }
        let files = ctx.input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
        if let Some(path) = files.iter().map(|file| file.path()).find(|path| {
            path.extension().and_then(|ext| ext.to_str()) == Some(store::EXT) && path.is_file()
        }) {
            self.open_path(path.to_path_buf());
        }
    }
}

// ---------------------------------------------------------------------------
// eframe 入口
// ---------------------------------------------------------------------------

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_dropped_files(&ctx);
        if self.new.open
            || self.unlock.is_some()
            || self.editor.as_ref().is_some_and(|e| e.pw.open)
        {
            // 对话框内容会随输入增长，保持连续重绘让窗口尺寸/命中区域及时更新
            ctx.request_repaint();
        }
        self.poll_pending();

        if let Some(t) = &self.toast
            && Instant::now() > t.until
        {
            self.toast = None;
        }

        match self.view {
            View::Library => self.library(ui),
            View::Editor => self.editor_ui(ui),
        }

        self.new_dialog(&ctx);
        self.unlock_dialog(&ctx);
        if let Some(ed) = &mut self.editor {
            Self::change_pw_dialog(&ctx, ed, &mut self.toast);
        }
        self.sheet_dialogs(&ctx);
        self.toast_ui(&ctx);
    }
}

// ---------------------------------------------------------------------------
// 文件库
// ---------------------------------------------------------------------------

impl App {
    fn library(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("lib_top")
            .frame(
                egui::Frame::new()
                    .fill(theme::BG)
                    .inner_margin(Margin {
                        left: 24,
                        right: 24,
                        top: 18,
                        bottom: 8,
                    }),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("最近打开")
                            .size(20.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let new = ui.add(
                            egui::Button::new(RichText::new("＋ 新建笔记").color(Color32::WHITE))
                                .fill(theme::ACCENT)
                                .corner_radius(9)
                                .min_size(vec2(104.0, 34.0)),
                        );
                        if new.clicked() {
                            self.new = NewState {
                                open: true,
                                dir: self.new_dir.clone(),
                                ..Default::default()
                            };
                        }
                        let open = ui.add(
                            egui::Button::new(RichText::new("打开文件…").color(theme::TEXT))
                                .fill(theme::CARD)
                                .stroke(Stroke::new(1.0, theme::BORDER))
                                .corner_radius(9)
                                .min_size(vec2(96.0, 34.0)),
                        );
                        if open.clicked()
                            && let Some(path) = rfd::FileDialog::new()
                                .set_title("打开 EncryptoNote 笔记")
                                .add_filter("EncryptoNote 笔记", &["ecnote"])
                                .pick_file()
                        {
                            self.open_path(path);
                        }
                    });
                });
            });

        if let Some(err) = self.open_error.take() {
            self.toast(err);
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::BG)
                    .inner_margin(Margin::same(24)),
            )
            .show(ui, |ui| {
                if self.entries.is_empty() {
                    self.library_empty(ui);
                    return;
                }
                widgets::card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(ui.available_height());
                    // 表头
                    let header_h = 34.0;
                    let (hrect, _) = ui
                        .allocate_exact_size(vec2(ui.available_width(), header_h), Sense::hover());
                    let p = ui.painter();
                    p.text(
                        hrect.left_center() + vec2(18.0, 0.0),
                        Align2::LEFT_CENTER,
                        "名称",
                        FontId::proportional(12.0),
                        theme::DIM,
                    );
                    p.text(
                        hrect.right_center() - vec2(250.0, 0.0),
                        Align2::RIGHT_CENTER,
                        "大小",
                        FontId::proportional(12.0),
                        theme::DIM,
                    );
                    p.text(
                        hrect.right_center() - vec2(24.0, 0.0),
                        Align2::RIGHT_CENTER,
                        "最后打开",
                        FontId::proportional(12.0),
                        theme::DIM,
                    );

                    let entries = self.entries.clone();
                    // 列表占满卡片剩余高度；只有条目超出时才出现滚动条
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for e in &entries {
                                self.row(ui, e);
                            }
                            ui.add_space(4.0);
                        });
                });
            });
        let _ = ctx;
    }

    /// 返回 true 表示这一行被双击打开。
    fn row(&mut self, ui: &mut egui::Ui, e: &RecentEntry) -> bool {
        let h = 44.0;
        let (rect, resp) = ui.allocate_exact_size(
            vec2(ui.available_width(), h),
            Sense::click(),
        );
        if resp.hovered() {
            ui.painter().rect_filled(
                rect.shrink2(vec2(6.0, 0.0)),
                8.0,
                theme::HOVER,
            );
        }
        let ink = if resp.hovered() { theme::TEXT } else { theme::TEXT };
        ui.painter().text(
            rect.left_center() + vec2(18.0, 0.0),
            Align2::LEFT_CENTER,
            e.path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| e.title.clone()),
            FontId::proportional(14.0),
            ink,
        );
        lock_glyph(ui.painter(), rect.left_center() + vec2(6.0, 0.0));
        ui.painter().text(
            rect.right_center() - vec2(250.0, 0.0),
            Align2::RIGHT_CENTER,
            recent::human_size(e.size),
            FontId::proportional(12.5),
            theme::DIM,
        );
        ui.painter().text(
            rect.right_center() - vec2(24.0, 0.0),
            Align2::RIGHT_CENTER,
            recent::display_time(e.opened),
            FontId::proportional(12.5),
            theme::DIM,
        );

        if resp.double_clicked() {
            self.open_path(e.path.clone());
            return true;
        }
        let path = e.path.clone();
        resp.context_menu(|ui| {
            ui.spacing_mut().button_padding = vec2(20.0, 6.0);
            if ui.button("打开").clicked() {
                self.open_path(path.clone());
                ui.close();
            }
            if ui.button("在文件夹打开").clicked() {
                reveal_in_explorer(&path);
                ui.close();
            }
            if ui.button("从列表中移除").clicked() {
                recent::forget(&path);
                self.entries = recent::list();
                ui.close();
            }
        });
        false
    }

    fn library_empty(&mut self, ui: &mut egui::Ui) {
        ui.add_space(60.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("还没有打开过笔记").size(16.0).color(theme::TEXT));
            ui.add_space(6.0);
            ui.label(
                RichText::new("新建一个 .ecnote 文件，或打开已有笔记")
                    .size(12.5)
                    .color(theme::DIM),
            );
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                let w = 150.0;
                let total = w * 2.0 + 10.0;
                ui.add_space((ui.available_width() - total).max(0.0) / 2.0);
                if widgets::primary_button(ui, "＋ 新建笔记", true, w) {
                    self.new = NewState {
                        open: true,
                        dir: self.new_dir.clone(),
                        ..Default::default()
                    };
                }
                if widgets::ghost_button(ui, "打开文件…", true, w)
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("EncryptoNote 笔记", &["ecnote"])
                        .pick_file()
                {
                    self.open_path(path);
                }
            });
        });
    }
}

// ---------------------------------------------------------------------------
// 编辑器
// ---------------------------------------------------------------------------

impl App {
    fn apply_image_action(ctx: &egui::Context, ed: &mut EditorCtx, action: editor::ImageAction) {
        use editor::ImageAction as A;
        let active = ed.active;
        let id = match &action {
            A::Delete(id) | A::Width(id, _) | A::Align(id, _) | A::Move(id, _) | A::MoveTo(id, _) => id,
        }.clone();
        let blocks = &mut ed.opened.doc.sheets[active].blocks;
        let rt = &mut ed.sheets[active];
        let Some(index) = editor::image_index(blocks, &id) else { return };
        rt.select_image(ctx, id.clone(), false);
        let action = if let A::Move(_, direction) = action {
            let next = index as i32 + direction;
            if next < 0 || next as usize >= blocks.len() {
                return;
            }
            A::MoveTo(id.clone(), if direction > 0 { next as usize + 1 } else { next as usize })
        } else { action };
        let mut reveal = false;
        match action {
            A::Width(_, factor) => {
                let factor = factor.clamp(0.15, 1.0);
                if blocks[index].image_width == Some(factor) {
                    return;
                }
                rt.undo.snapshot(blocks);
                blocks[index].image_width = Some(factor);
            }
            A::Align(_, align) => {
                if blocks[index].align.unwrap_or(DocAlign::Center) == align {
                    return;
                }
                rt.undo.snapshot(blocks);
                blocks[index].align = Some(align);
            }
            A::MoveTo(_, gap) => {
                let target = if gap > index { gap - 1 } else { gap };
                if target == index {
                    return;
                }
                rt.undo.snapshot(blocks);
                let image = blocks.remove(index);
                blocks.insert(target, image);
                reveal = true;
            }
            A::Delete(_) => {
                rt.undo.snapshot(blocks);
                blocks.remove(index);
                if !blocks.get(index).is_some_and(|b| !matches!(b.kind, BlockKind::Image | BlockKind::Divider)) {
                    blocks.insert(index, Block::empty());
                }
                rt.editor.document_replaced();
                rt.focus_text(Position::new(index, 0));
                ed.dirty = true;
                return;
            }
            A::Move(_, _) => unreachable!(),
        }
        rt.editor.document_replaced();
        rt.select_image(ctx, id, reveal);
        ed.dirty = true;
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(mut ed) = self.editor.take() else {
            self.view = View::Library;
            return;
        };

        // 图片被删后清掉残留的空图片块，避免留下「图片」占位
        if ed.opened.doc.prune_images() {
            ed.dirty = true;
            // 段落数变少时把光标夹回最后一段，避免指向已删除的图片块
            let active = ed.active;
            let len = ed.opened.doc.sheets[active].blocks.len();
            if let Some(sel) = ed.sheets[active].editor.selection()
                && sel.focus.paragraph >= len
            {
                let last = len.saturating_sub(1);
                let off = ed.opened.doc.sheets[active].blocks[last].char_len();
                ed.sheets[active]
                    .editor
                    .select(Selection::caret(Position::new(last, off)));
            }
        }

        let mut go_back = false;
        let mut save_clicked = false;
        let mut lock_clicked = false;

        let file_name = ed.opened.file_name();
        egui::Panel::top("ed_top")
            .frame(
                egui::Frame::new()
                    .fill(theme::CARD)
                    .inner_margin(Margin {
                        left: 18,
                        right: 18,
                        top: 10,
                        bottom: 10,
                    }),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if back_button(ui).clicked() {
                        go_back = true;
                    }
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(&file_name)
                            .size(14.5)
                            .strong()
                            .color(theme::TEXT),
                    );
                    if ed.dirty {
                        ui.label(RichText::new("●").size(10.0).color(theme::WARN));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icon_button(ui, Icon::Lock, 30.0, false, true)
                            .clicked()
                        {
                            lock_clicked = true;
                        }
                        ui.add_space(2.0);
                        if icon_button(ui, Icon::Key, 30.0, false, true)
                            .clicked()
                        {
                            ed.pw = PwDialog {
                                open: true,
                                p1: String::new(),
                                p2: String::new(),
                                error: None,
                            };
                        }
                    });
                });
            });

        let active = ed.active;
        Self::toolbar(ui, &mut ed, &mut save_clicked);

        // 工作表栏（底部）
        egui::Panel::bottom("ed_sheets")
            .exact_size(44.0)
            .frame(
                egui::Frame::new()
                    .fill(theme::BG)
                    .stroke(egui::Stroke::new(1.0, theme::BORDER))
                    .inner_margin(Margin {
                        left: 16,
                        right: 12,
                        top: 9,
                        bottom: 9,
                    }),
            )
            .show(ui, |ui| {
                Self::sheet_bar(ui, &mut ed);
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::BG)
                    .inner_margin(egui::Margin::same(0)),
            )
            .show(ui, |ui| {
                let user_scrolled = ui.input(|i| i.smooth_scroll_delta.y.abs() > 0.5);
                if user_scrolled {
                    // 用户主动滚动时立刻解除钉住，避免滚轮被吞掉
                    ed.sheets[active].scroll_pin = None;
                }
                let pinned = ed.sheets[active].scroll_pin.map(|(y, _)| y);
                const GAP: f32 = 20.0;
                // 纸张至少铺满可视区，内容增多后由布局撑高，并与正文一起滚动。
                let min_content_height =
                    (ui.available_height() - PAGE_TOP - PAGE_BOTTOM).max(0.0);
                let scroll_out = egui::ScrollArea::vertical()
                    .id_salt(("paper", &ed.opened.doc.sheets[active].id))
                    .auto_shrink([false, false])
                    .vertical_scroll_offset(pinned.unwrap_or(ed.sheets[active].scroll))
                    .show(ui, |ui| {
                        let inner_w =
                            (ui.available_width() - GAP * 2.0 - PAGE_SIDE * 2.0).max(320.0);
                        egui::Frame::new()
                            .fill(theme::CARD)
                            .corner_radius(10)
                            .shadow(egui::Shadow {
                                offset: [0, 2],
                                blur: 0,
                                spread: 0,
                                color: Color32::from_black_alpha(12),
                            })
                            .outer_margin(GAP as i8)
                            .inner_margin(Margin {
                                left: PAGE_SIDE as i8,
                                right: PAGE_SIDE as i8,
                                top: (PAGE_TOP - GAP) as i8,
                                bottom: (PAGE_BOTTOM - GAP) as i8,
                            })
                            .show(ui, |ui| {
                                ui.set_width(inner_w);
                                ui.set_min_height(min_content_height);
                                let mut images = std::mem::take(&mut ed.opened.doc.images);
                                let sheet = &mut ed.opened.doc.sheets[active];
                                let rt = &mut ed.sheets[active];
                                let pasted_image = ui.input(|i| {
                                    i.events.iter().find_map(|e| match e {
                                        egui::Event::PasteImage(img) => Some(img.clone()),
                                        _ => None,
                                    })
                                });
                                if let Some(img) = pasted_image
                                    && (rt.editor.has_focus(ui.ctx())
                                        || ui.memory(|m| m.has_focus(rt.id.with("image"))))
                                    && let Some(asset) = asset_from_color_image(&img)
                                {
                                    ui.input_mut(|i| {
                                        i.events
                                            .retain(|e| !matches!(e, egui::Event::PasteImage(_)));
                                    });
                                    let id = asset.id.clone();
                                    images.push(asset);
                                    rt.insert_image(ui.ctx(), &mut sheet.blocks, id);
                                    ed.dirty = true;
                                }
                                let mut img_action = rt.image_input(ui, &mut sheet.blocks, &mut ed.dirty);
                                let mut model = ActiveSheet {
                                    blocks: &mut sheet.blocks,
                                    undo: &mut rt.undo,
                                    pending: &mut rt.pending,
                                    dirty: &mut ed.dirty,
                                };
                                let changed = rt.editor.input(ui, &mut model);
                                if changed {
                                    rt.selected_image = None;
                                }
                                if let Some(sel) = rt.editor.selection()
                                    && sel.is_caret()
                                    && let Some(block) = sheet.blocks.get(sel.focus.paragraph)
                                    && block.kind == BlockKind::Image
                                {
                                    rt.select_image(ui.ctx(), block.runs[0].text.clone(), true);
                                }
                                let ink = theme::TEXT;
                                editor::draw_document(
                                    ui,
                                    &sheet.blocks,
                                    rt,
                                    inner_w,
                                    ink,
                                    &images,
                                    &mut img_action,
                                );
                                ed.opened.doc.images = images;
                                if let Some(a) = img_action {
                                    Self::apply_image_action(ui.ctx(), &mut ed, a);
                                }
                            });
                    });
                if let Some((y, n)) = ed.sheets[active].scroll_pin {
                    // 撤销/重做的那一帧 RichEdit 可能请求「把光标带回视野」，
                    // 这里把纸面钉回原来的滚动位置，几帧后自然解除。
                    ed.sheets[active].scroll = y;
                    ed.sheets[active].scroll_pin =
                        if n > 1 { Some((y, n - 1)) } else { None };
                } else {
                    ed.sheets[active].scroll = scroll_out.state.clamped_offset().y;
                }
            });

        // 在控件注册后恢复焦点，避免刚关闭的模态层让控件再次丢失焦点。
        if ed.focus_editor {
            let rt = &ed.sheets[ed.active];
            let focus = if rt.selected_image.is_some() { rt.id.with("image") } else { rt.id };
            ui.memory_mut(|m| m.request_focus(focus));
            ed.focus_editor = false;
        }

        let sc = if self.unlock.is_none() {
            Self::shortcuts(&ctx, &mut ed)
        } else {
            ShortcutOut::default()
        };
        if sc.save {
            save_clicked = true;
        }
        if sc.lock {
            lock_clicked = true;
        }

        self.editor = Some(ed);

        if go_back || lock_clicked {
            let dirty = self.editor.as_ref().is_some_and(|e| e.dirty);
            if dirty {
                self.save_now();
            }
        }
        if go_back {
            self.leave_editor();
        }
        if lock_clicked {
            self.leave_editor();
            self.toast("已锁定");
        }
        if save_clicked {
            self.save_now();
        }
        self.refresh_entries();
    }

    fn toolbar(ui: &mut egui::Ui, ed: &mut EditorCtx, save_clicked: &mut bool) {
        let active = ed.active;
        let mut kind_change: Option<BlockKind> = None;
        let mut align_change: Option<DocAlign> = None;
        let mut align_for: Option<usize> = None;

        egui::Panel::top("ed_toolbar")
            .frame(
                egui::Frame::new()
                    .fill(theme::CARD)
                    .inner_margin(Margin {
                        left: 14,
                        right: 14,
                        top: 6,
                        bottom: 6,
                    })
                    .stroke(Stroke::new(1.0, theme::BORDER)),
            )
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let rt = &mut ed.sheets[active];
                    let has_undo = rt.undo.can_undo();
                    let has_redo = rt.undo.can_redo();
                    if icon_button(ui, Icon::Undo, 28.0, false, has_undo).clicked() {
                        let sheet = &mut ed.opened.doc.sheets[active];
                        let caret = rt.focus_position(&sheet.blocks);
                        if let Some(prev) = rt.undo.undo_step(&sheet.blocks) {
                            sheet.blocks = prev;
                            restore_history_selection(ui.ctx(), rt, &sheet.blocks, caret);
                            ed.dirty = true;
                        }
                    }
                    if icon_button(ui, Icon::Redo, 28.0, false, has_redo).clicked() {
                        let sheet = &mut ed.opened.doc.sheets[active];
                        let caret = rt.focus_position(&sheet.blocks);
                        if let Some(next) = rt.undo.redo_step(&sheet.blocks) {
                            sheet.blocks = next;
                            restore_history_selection(ui.ctx(), rt, &sheet.blocks, caret);
                            ed.dirty = true;
                        }
                    }
                    ui.separator();

                    // 段落样式
                    let block_kind = current_block_kind(ed, active);
                    egui::ComboBox::from_id_salt("block_kind")
                        .selected_text(block_kind.label())
                        .width(92.0)
                        .show_ui(ui, |ui| {
                            for k in [
                                BlockKind::Body,
                                BlockKind::H1,
                                BlockKind::H2,
                                BlockKind::H3,
                                BlockKind::Quote,
                                BlockKind::Code,
                                BlockKind::Bullet,
                                BlockKind::Number,
                                BlockKind::Divider,
                            ] {
                                if ui.selectable_label(block_kind == k, k.label()).clicked() {
                                    kind_change = Some(k);
                                    ui.close();
                                }
                            }
                        });
                    ui.separator();
                    // 分割线 / 项目符号 / 编号：与段落样式下拉框等价，但一键可达
                    let block_kind_now = current_block_kind(ed, active);
                    if icon_button(ui, Icon::Divider, 28.0, block_kind_now == BlockKind::Divider, true)
                        .clicked()
                    {
                        kind_change = Some(BlockKind::Divider);
                    }
                    for (icon, kind) in [
                        (Icon::Bullet, BlockKind::Bullet),
                        (Icon::Number, BlockKind::Number),
                    ] {
                        if icon_button(ui, icon, 28.0, block_kind_now == kind, true)
                            .clicked()
                        {
                            // 已经是同一种时再点一次，退回正文（开关语义）
                            kind_change = Some(if block_kind_now == kind {
                                BlockKind::Body
                            } else {
                                kind
                            });
                        }
                    }
                    ui.separator();

                    // 行内格式
                    for mark in [
                        egui_richedit::Mark::Bold,
                        egui_richedit::Mark::Italic,
                        egui_richedit::Mark::Underline,
                        egui_richedit::Mark::Strike,
                    ] {
                        let mut model_sel = selection_of(ed, active);
                        let on = {
                            let sheet = &ed.opened.doc.sheets[active];
                            let rt = &ed.sheets[active];
                            match model_sel {
                                Some((a, b)) => {
                                    let sheet_ref = sheet;
                                    let _ = rt;
                                    marked_between(sheet_ref, a, b, mark)
                                }
                                None => None,
                            }
                        };
                        if icon_button(ui, Icon::Format(mark), 28.0, on == Some(true), true).clicked() {
                            let (from, to) = model_sel.unwrap_or_else(|| caret_of(ed, active));
                            let bit = match mark {
                                egui_richedit::Mark::Bold => Run::BOLD,
                                egui_richedit::Mark::Italic => Run::ITALIC,
                                egui_richedit::Mark::Underline => Run::UNDERLINE,
                                egui_richedit::Mark::Strike => Run::STRIKE,
                            };
                            apply_marks(ed, active, from, to, bit);
                        }
                        let _ = &mut model_sel;
                    }

                    ui.separator();
                    // 字号
                    let cur_size = current_size(ed, active).unwrap_or(15.5);
                    let mut size = cur_size;
                    if ui
                        .add(
                            egui::DragValue::new(&mut size)
                                .range(9.0..=72.0)
                                .speed(0.5)
                                .fixed_decimals(1)
                                .suffix(" pt"),
                        )
                        .changed()
                    {
                        let (from, to) = selection_of(ed, active)
                            .unwrap_or_else(|| caret_of(ed, active));
                        if from != to {
                            push_undo(ed, active);
                            let sheet = &mut ed.opened.doc.sheets[active];
                            editor::format_size(&mut sheet.blocks, from, to, size);
                            ed.dirty = true;
                        } else {
                            ed.sheets[active].pending.size = Some(size);
                        }
                    }

                    ui.separator();
                    // 颜色 / 高亮
                    color_menu(ui, "A", theme::ACCENT, ed, active, false);
                    color_menu(ui, "ab", theme::WARN, ed, active, true);
                    ui.separator();

                    // 对齐
                    let cur_align = current_align(ed, active);
                    for a in [
                        DocAlign::Left,
                        DocAlign::Center,
                        DocAlign::Right,
                        DocAlign::Justify,
                    ] {
                        let on = cur_align == Some(a);
                        if icon_button(ui, Icon::Align(a), 28.0, on, true).clicked() {
                            align_change = Some(a);
                            align_for = Some(caret_of(ed, active).0.0);
                        }
                    }

                    ui.separator();
                    if icon_button(ui, Icon::Image, 28.0, false, true)
                        .clicked()
                        && let Some(asset) = clipboard_image_asset()
                    {
                        let id = asset.id.clone();
                        ed.opened.doc.images.push(asset);
                        let doc = &mut ed.opened.doc;
                        let sheet = &mut doc.sheets[active];
                        let rt = &mut ed.sheets[active];
                        rt.insert_image(ui.ctx(), &mut sheet.blocks, id);
                        ed.dirty = true;
                    }
                    if icon_button(ui, Icon::Clear, 28.0, false, true)
                        .clicked()
                    {
                        let (from, to) = selection_of(ed, active)
                            .unwrap_or_else(|| caret_of(ed, active));
                        push_undo(ed, active);
                        let sheet = &mut ed.opened.doc.sheets[active];
                        editor::clear_format(&mut sheet.blocks, from, to);
                        ed.dirty = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icon_button(ui, Icon::Save, 30.0, false, true)
                            .clicked()
                        {
                            *save_clicked = true;
                        }
                    });
                });
            });

        if let Some(kind) = kind_change {
            let (from, to) = selection_of(ed, active).unwrap_or_else(|| caret_of(ed, active));
            push_undo(ed, active);
            let sheet = &mut ed.opened.doc.sheets[active];
            let rt = &mut ed.sheets[active];
            if kind == BlockKind::Divider {
                let next = editor::insert_divider_after(&mut sheet.blocks, to.0);
                rt.editor.select(Selection::caret(Position::new(next, 0)));
            } else {
                let (a, b) = if from.0 <= to.0 { (from.0, to.0) } else { (to.0, from.0) };
                for p in a..=b.min(sheet.blocks.len().saturating_sub(1)) {
                    if !matches!(sheet.blocks[p].kind, BlockKind::Image | BlockKind::Divider) {
                        sheet.blocks[p].kind = kind;
                    }
                }
                rt.editor.select(Selection {
                    anchor: Position::new(from.0, from.1),
                    focus: Position::new(to.0, to.1),
                });
            }
            rt.selected_image = None;
            ed.dirty = true;
        }
        if let Some(a) = align_change {
            push_undo(ed, active);
            let sheet = &mut ed.opened.doc.sheets[active];
            let p = align_for.unwrap_or(0);
            if let Some(b) = sheet.blocks.get_mut(p) {
                b.align = Some(a);
            }
            if let Some(id) = ed.sheets[active].selected_image.clone() {
                ed.sheets[active].select_image(ui.ctx(), id, false);
            }
            ed.dirty = true;
        }
    }

    fn shortcuts(ctx: &egui::Context, ed: &mut EditorCtx) -> ShortcutOut {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let mut out = ShortcutOut::default();
        let save = KeyboardShortcut::new(Modifiers::CTRL, Key::S);
        if ctx.input_mut(|i| i.consume_shortcut(&save)) {
            out.save = true;
            return out;
        }
        let lock = KeyboardShortcut::new(Modifiers::CTRL, Key::L);
        if ctx.input_mut(|i| i.consume_shortcut(&lock)) {
            out.lock = true;
            return out;
        }
        let undo = KeyboardShortcut::new(Modifiers::CTRL, Key::Z);
        let redo = KeyboardShortcut::new(Modifiers::CTRL, Key::Y);
        if ctx.input_mut(|i| i.consume_shortcut(&undo)) {
            let active = ed.active;
            let caret = ed.sheets[active].focus_position(&ed.opened.doc.sheets[active].blocks);
            let prev = {
                let sheet = &ed.opened.doc.sheets[active];
                ed.sheets[active].undo.undo_step(&sheet.blocks)
            };
            if let Some(prev) = prev {
                ed.opened.doc.sheets[active].blocks = prev;
                restore_history_selection(ctx, &mut ed.sheets[active], &ed.opened.doc.sheets[active].blocks, caret);
                ed.dirty = true;
            }
            return out;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&redo)) {
            let active = ed.active;
            let caret = ed.sheets[active].focus_position(&ed.opened.doc.sheets[active].blocks);
            let next = {
                let sheet = &ed.opened.doc.sheets[active];
                ed.sheets[active].undo.redo_step(&sheet.blocks)
            };
            if let Some(next) = next {
                ed.opened.doc.sheets[active].blocks = next;
                restore_history_selection(ctx, &mut ed.sheets[active], &ed.opened.doc.sheets[active].blocks, caret);
                ed.dirty = true;
            }
            return out;
        }
        let prev_tab = KeyboardShortcut::new(Modifiers::CTRL, Key::PageUp);
        let next_tab = KeyboardShortcut::new(Modifiers::CTRL, Key::PageDown);
        for (sc, delta) in [(prev_tab, -1i32), (next_tab, 1i32)] {
            if ctx.input_mut(|i| i.consume_shortcut(&sc)) {
                Self::switch_sheet(ed, delta);
                return out;
            }
        }
        let new_sheet = KeyboardShortcut::new(Modifiers::CTRL, Key::N);
        if ctx.input_mut(|i| i.consume_shortcut(&new_sheet)) {
            Self::add_sheet(ed);
            return out;
        }
        for (n, key) in [
            (1, Key::Num1),
            (2, Key::Num2),
            (3, Key::Num3),
            (4, Key::Num4),
            (5, Key::Num5),
        ] {
            let sc = KeyboardShortcut::new(Modifiers::CTRL, key);
            if ctx.input_mut(|i| i.consume_shortcut(&sc)) {
                if n - 1 < ed.opened.doc.sheets.len() {
                    ed.active = n - 1;
                    ed.opened.doc.active_sheet =
                        ed.opened.doc.sheets[ed.active].id.clone();
                }
                return out;
            }
        }
        out
    }

    // ---- 工作表 ----

    fn switch_sheet(ed: &mut EditorCtx, delta: i32) {
        let n = ed.opened.doc.sheets.len() as i32;
        if n == 0 {
            return;
        }
        ed.active = (ed.active as i32 + delta).rem_euclid(n) as usize;
        Self::focus_active_sheet(ed);
    }

    fn focus_active_sheet(ed: &mut EditorCtx) {
        let active = ed.active;
        ed.opened.doc.active_sheet = ed.opened.doc.sheets[active].id.clone();
        ed.sheets[active].focus_text(Position::new(0, 0));
        ed.focus_editor = true;
    }

    fn add_sheet(ed: &mut EditorCtx) {
        let mut n = ed.opened.doc.sheets.len() + 1;
        while ed
            .opened
            .doc
            .sheets
            .iter()
            .any(|s| s.name == format!("Sheet{n}"))
        {
            n += 1;
        }
        let id = format!("s{}", recent::random_id());
        let sheet = Sheet::new(id.clone(), format!("Sheet{n}"));
        let at = ed.active + 1;
        ed.opened.doc.sheets.insert(at, sheet);
        ed.sheets.insert(at, {
            let mut e = SheetEditor::new(Id::unique(("ecnote-editor", &id)));
            e.editor
                .select(Selection::caret(Position::new(0usize, 0usize)));
            e
        });
        ed.active = at;
        ed.opened.doc.active_sheet = id;
        ed.dirty = true;
        ed.focus_editor = true;
    }

    fn duplicate_sheet(ed: &mut EditorCtx, idx: usize) {
        let Some(src) = ed.opened.doc.sheets.get(idx).cloned() else {
            return;
        };
        let mut n = 2;
        let base = src.name.clone();
        while ed
            .opened
            .doc
            .sheets
            .iter()
            .any(|s| s.name == format!("{base} ({n})"))
        {
            n += 1;
        }
        let id = format!("s{}", recent::random_id());
        let sheet = Sheet {
            id: id.clone(),
            name: format!("{base} ({n})"),
            color: src.color.clone(),
            blocks: src.blocks,
        };
        ed.opened.doc.sheets.insert(idx + 1, sheet);
        let mut e = SheetEditor::new(Id::unique(("ecnote-editor", &id)));
        e.editor
            .select(Selection::caret(Position::new(0usize, 0usize)));
        ed.sheets.insert(idx + 1, e);
        ed.active = idx + 1;
        ed.opened.doc.active_sheet = id;
        ed.dirty = true;
        ed.focus_editor = true;
    }

    fn delete_sheet(ed: &mut EditorCtx, idx: usize) {
        if ed.opened.doc.sheets.len() <= 1 {
            let sheet = Sheet::new("s1", "Sheet1");
            ed.opened.doc.sheets[0] = sheet;
            ed.sheets[0].undo.clear();
            ed.active = 0;
        } else {
            ed.opened.doc.sheets.remove(idx);
            ed.sheets.remove(idx);
            if ed.active >= ed.opened.doc.sheets.len() {
                ed.active = ed.opened.doc.sheets.len() - 1;
            }
        }
        ed.opened.doc.active_sheet = ed.opened.doc.sheets[ed.active].id.clone();
        ed.dirty = true;
        ed.focus_editor = true;
    }

    fn move_sheet(ed: &mut EditorCtx, idx: usize, delta: i32) {
        let n = ed.opened.doc.sheets.len() as i32;
        let target = (idx as i32 + delta).clamp(0, n - 1) as usize;
        if target == idx {
            return;
        }
        ed.opened.doc.sheets.swap(idx, target);
        ed.sheets.swap(idx, target);
        ed.active = target;
        ed.dirty = true;
    }

    fn sheet_bar(ui: &mut egui::Ui, ed: &mut EditorCtx) {
        let n = ed.opened.doc.sheets.len();
        let mut action: Option<(usize, SheetAction)> = None;

        ui.horizontal(|ui| {
            for i in 0..n {
                let name = ed.opened.doc.sheets[i].name.clone();
                let color = ed.opened.doc.sheets[i]
                    .color
                    .as_deref()
                    .and_then(editor::parse_color);
                let selected = i == ed.active;
                let renaming = matches!(&ed.rename, Some((r, _, _)) if *r == i);

                if renaming {
                    let mut buf = match &mut ed.rename {
                        Some((_, b, _)) => std::mem::take(b),
                        _ => name.clone(),
                    };
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut buf)
                            .desired_width(90.0)
                            .font(FontId::proportional(12.5)),
                    );
                    let enter = resp.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if resp.lost_focus() || enter {
                        action = Some((i, SheetAction::RenameDone(buf.clone())));
                    } else {
                        if let Some((_, b, _)) = &mut ed.rename {
                            *b = buf;
                        }
                    }
                    continue;
                }

                let font = FontId::proportional(12.5);
                let text_w = ui
                    .painter()
                    .layout_no_wrap(name.clone(), font.clone(), theme::TEXT)
                    .size()
                    .x;
                let tab_w = (14.0 + text_w + 14.0).max(56.0);
                let (rect, resp) = ui.allocate_exact_size(
                    vec2(tab_w, 26.0),
                    Sense::click_and_drag(),
                );
                // 与「＋」统一：同高度、同圆角、同 hover；选中是灰底上的白卡
                let bg = if selected {
                    theme::CARD
                } else if resp.hovered() {
                    theme::PRESS
                } else {
                    Color32::TRANSPARENT
                };
                ui.painter().rect_filled(rect, 7.0, bg);
                if selected {
                    ui.painter().rect_stroke(
                        rect,
                        7.0,
                        egui::Stroke::new(1.0, theme::BORDER),
                        egui::StrokeKind::Inside,
                    );
                }
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    &name,
                    font,
                    if selected {
                        color.unwrap_or(theme::TEXT)
                    } else {
                        theme::DIM
                    },
                );
                if resp.clicked() {
                    action = Some((i, SheetAction::Select));
                }
                if resp.double_clicked() {
                    action = Some((i, SheetAction::StartRename));
                }
                resp.context_menu(|ui| {
                    ui.spacing_mut().button_padding = vec2(20.0, 6.0);
                    if ui.button("重命名").clicked() {
                        action = Some((i, SheetAction::StartRename));
                        ui.close();
                    }
                    if ui.button("复制工作表").clicked() {
                        action = Some((i, SheetAction::Duplicate));
                        ui.close();
                    }
                    if ui.button("左移").clicked() {
                        action = Some((i, SheetAction::Move(-1)));
                        ui.close();
                    }
                    if ui.button("右移").clicked() {
                        action = Some((i, SheetAction::Move(1)));
                        ui.close();
                    }
                    ui.menu_button("标签颜色", |ui| {
                        ui.spacing_mut().button_padding = vec2(20.0, 6.0);
                        for (label, hex) in SHEET_COLORS {
                            if ui.button(label).clicked() {
                                action = Some((
                                    i,
                                    SheetAction::Color(if hex.is_empty() {
                                        None
                                    } else {
                                        Some(hex.to_owned())
                                    }),
                                ));
                                ui.close();
                            }
                        }
                    });
                    ui.separator();
                    if ui.button("删除工作表").clicked() {
                        action = Some((i, SheetAction::Delete));
                        ui.close();
                    }
                });
            }

            ui.add_space(4.0);
            // 「＋」与标签同一套视觉：透明底，悬停才出现浅灰圆角
            let (prect, presp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
            let pbg = if presp.hovered() {
                theme::PRESS
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(prect, 7.0, pbg);
            // 自绘加号，比「＋」字形更清晰、粗细与其它图标一致
            let pc = prect.center();
            let pcol = if presp.hovered() { theme::TEXT } else { theme::DIM };
            let arm = 5.5;
            ui.painter().line_segment(
                [pos2(pc.x - arm, pc.y), pos2(pc.x + arm, pc.y)],
                egui::Stroke::new(1.5, pcol),
            );
            ui.painter().line_segment(
                [pos2(pc.x, pc.y - arm), pos2(pc.x, pc.y + arm)],
                egui::Stroke::new(1.5, pcol),
            );
            if presp.clicked() {
                action = Some((usize::MAX, SheetAction::Add));
            }
            widgets::tooltip(presp, "新建工作表", Some("Ctrl+N"));
        });

        if let Some((idx, act)) = action {
            match act {
                SheetAction::Add => Self::add_sheet(ed),
                SheetAction::Select => {
                    ed.active = idx;
                    ed.opened.doc.active_sheet = ed.opened.doc.sheets[idx].id.clone();
                    ed.focus_editor = true;
                }
                SheetAction::StartRename => {
                    let name = ed.opened.doc.sheets[idx].name.clone();
                    ed.rename = Some((idx, name, true));
                }
                SheetAction::RenameDone(buf) => {
                    if let Some(name) = clean_sheet_name(&buf, &ed.opened.doc.sheets, idx) {
                        ed.opened.doc.sheets[idx].name = name;
                        ed.dirty = true;
                    }
                    ed.rename = None;
                }
                SheetAction::Duplicate => Self::duplicate_sheet(ed, idx),
                SheetAction::Delete => ed.delete_confirm = Some(idx),
                SheetAction::Move(d) => Self::move_sheet(ed, idx, d),
                SheetAction::Color(c) => {
                    ed.opened.doc.sheets[idx].color = c;
                    ed.dirty = true;
                }
            }
        }
    }

    fn sheet_dialogs(&mut self, ctx: &egui::Context) {
        let Some(idx) = self.editor.as_ref().and_then(|e| e.delete_confirm) else {
            return;
        };
        let name = self
            .editor
            .as_ref()
            .and_then(|e| e.opened.doc.sheets.get(idx))
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let mut do_delete = false;
        let mut cancel = false;
        let dismissed = modal(ctx, "delete_sheet", 340.0, |ui| {
            ui.label(RichText::new("删除工作表").size(15.0).strong());
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("「{name}」中的内容会一并删除，无法恢复。"))
                    .size(12.5)
                    .color(theme::DIM),
            );
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                let total = 100.0 * 2.0 + ui.spacing().item_spacing.x;
                ui.add_space((ui.available_width() - total).max(0.0));
                if widgets::primary_button(ui, "删除", true, 100.0) {
                    do_delete = true;
                }
                if widgets::ghost_button(ui, "取消", true, 100.0) {
                    cancel = true;
                }
            });
        });
        if do_delete {
            if let Some(ed) = &mut self.editor {
                ed.delete_confirm = None;
                Self::delete_sheet(ed, idx);
            }
        } else if (cancel || dismissed) && let Some(ed) = &mut self.editor {
            ed.delete_confirm = None;
        }
    }

    fn change_pw_dialog(ctx: &egui::Context, ed: &mut EditorCtx, toast: &mut Option<Toast>) {
        if !ed.pw.open {
            return;
        }
        let mut submit = false;
        let dismissed = modal(ctx, "change_pw", 380.0, |ui| {
            ui.label(RichText::new("修改密码").size(15.0).strong());
            ui.add_space(4.0);
            ui.label(
                RichText::new("只重新包装文件密钥，正文不会重新加密。")
                    .size(11.5)
                    .color(theme::DIM),
            );
            ui.add_space(10.0);
            pw_field(ui, &mut ed.pw.p1, "新密码（至少 6 位）");
            ui.add_space(6.0);
            pw_field(ui, &mut ed.pw.p2, "再次输入新密码");
            if let Some(e) = &ed.pw.error {
                ui.add_space(6.0);
                ui.label(RichText::new(e).size(11.5).color(theme::DANGER));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let bw = 96.0;
                let total = bw * 2.0 + ui.spacing().item_spacing.x;
                ui.add_space((ui.available_width() - total).max(0.0));
                if widgets::primary_button(ui, "确认修改", true, bw) {
                    submit = true;
                }
                if widgets::ghost_button(ui, "取消", true, bw) {
                    ed.pw.open = false;
                }
            });
        });
        if dismissed {
            ed.pw.open = false;
            return;
        }
        if submit {
            if ed.pw.p1.chars().count() < 6 {
                ed.pw.error = Some("新密码至少 6 个字符".to_owned());
            } else if ed.pw.p1 != ed.pw.p2 {
                ed.pw.error = Some("两次输入不一致".to_owned());
            } else {
                match ed.opened.change_password(&ed.pw.p1) {
                    Ok(()) => {
                        ed.pw.open = false;
                        *toast = Some(Toast {
                            text: "密码已修改".to_owned(),
                            until: Instant::now() + std::time::Duration::from_secs(3),
                        });
                    }
                    Err(e) => ed.pw.error = Some(format!("{e:#}")),
                }
            }
        }
    }

    // ---- 对话框 ----

    fn new_dialog(&mut self, ctx: &egui::Context) {
        if !self.new.open {
            return;
        }
        let mut create = false;
        let mut cancel = false;
        let mut pick = false;
        let busy = self.pending.is_some();

        let dismissed = modal(ctx, "new_note", 460.0, |ui| {
            ui.label(RichText::new("新建笔记").size(16.0).strong());
            ui.add_space(2.0);
            ui.label(
                RichText::new("文件以 .ecnote 保存，内容全程加密。")
                    .size(11.5)
                    .color(theme::DIM),
            );
            ui.add_space(12.0);

            ui.label(RichText::new("文件名").size(12.0).color(theme::DIM));
            ui.add_space(4.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.new.name)
                    .hint_text("未命名")
                    .desired_width(f32::INFINITY)
                    .margin(Margin::symmetric(10, 8)),
            );
            ui.add_space(10.0);

            ui.label(RichText::new("保存位置").size(12.0).color(theme::DIM));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let path = self
                    .new
                    .dir
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "尚未选择".to_owned());
                let color = if self.new.dir.is_some() { theme::TEXT } else { theme::FAINT };
                egui::Frame::new()
                    .fill(theme::FIELD)
                    .corner_radius(CornerRadius::same(9))
                    .inner_margin(Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width() - 88.0);
                        ui.label(RichText::new(path).size(12.5).color(color));
                    });
                if widgets::chip_button(ui, "选择…") {
                    pick = true;
                }
            });
            ui.add_space(10.0);

            ui.label(RichText::new("密码").size(12.0).color(theme::DIM));
            ui.add_space(4.0);
            let r1 = pw_field(ui, &mut self.new.password, "至少 6 位，务必记牢");
            ui.add_space(6.0);
            let r2 = pw_field(ui, &mut self.new.confirm, "再次输入密码");
            let _ = (r1, r2);
            if !self.new.password.is_empty() {
                ui.add_space(6.0);
                let (frac, label, color) = strength(&self.new.password);
                widgets::thin_bar(ui, frac, color);
                ui.add_space(3.0);
                ui.label(RichText::new(label).size(11.0).color(color));
            }
            if let Some(e) = &self.new.error {
                ui.add_space(6.0);
                ui.label(RichText::new(e).size(11.5).color(theme::DANGER));
            }
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                let bw = 96.0;
                let total = bw * 2.0 + ui.spacing().item_spacing.x;
                ui.add_space((ui.available_width() - total).max(0.0));
                if widgets::primary_button(ui, if busy { "创建中…" } else { "创建" }, !busy, bw) {
                    create = true;
                }
                if widgets::ghost_button(ui, "取消", !busy, bw) {
                    cancel = true;
                }
            });
        });

        if pick && let Some(dir) = rfd::FileDialog::new().set_title("选择保存位置").pick_folder() {
            self.new.dir = Some(dir);
        }
        if cancel || dismissed {
            self.new.open = false;
        }
        if create {
            self.start_create(ctx);
        }
    }

    fn unlock_dialog(&mut self, ctx: &egui::Context) {
        let Some(u) = &mut self.unlock else { return };
        let mut submit = false;
        let mut cancel = false;
        let busy = u.busy;
        let title = u.title.clone();
        let path = u.path.display().to_string();
        let mut password = std::mem::take(&mut u.password);

        let dismissed = modal(ctx, "unlock", 400.0, |ui| {
            ui.label(RichText::new("解锁笔记").size(16.0).strong());
            ui.add_space(4.0);
            ui.label(RichText::new(&title).size(12.0).color(theme::DIM));
            ui.label(
                RichText::new(&path)
                    .size(10.5)
                    .color(theme::FAINT),
            );
            ui.add_space(12.0);
            let resp = pw_field(ui, &mut password, "输入密码");
            if !busy {
                resp.request_focus();
            }
            if resp.changed() {
                u.error = None;
            }
            // 固定高度：错误出现/消失时不改变按钮位置，避免点击落空
            ui.allocate_ui(vec2(ui.available_width(), 20.0), |ui| {
                ui.add_space(4.0);
                if let Some(e) = u.error.as_ref() {
                    ui.label(RichText::new(e).size(11.5).color(theme::DANGER));
                }
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let bw = 96.0;
                let total = bw * 2.0 + ui.spacing().item_spacing.x;
                ui.add_space((ui.available_width() - total).max(0.0));
                if widgets::primary_button(ui, if busy { "解锁中…" } else { "解锁" }, !busy, bw) {
                    submit = true;
                }
                if widgets::ghost_button(ui, "取消", !busy, bw) {
                    cancel = true;
                }
            });
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) && !busy && !password.is_empty() {
                submit = true;
            }
        });

        if let Some(u) = &mut self.unlock {
            u.password = password;
        }
        if (cancel || dismissed) && !busy {
            self.unlock = None;
            if let Some(ed) = &mut self.editor {
                ed.focus_editor = true;
            }
        }
        if submit {
            self.start_open(ctx);
        }
    }

    fn toast_ui(&self, ctx: &egui::Context) {
        let Some(t) = &self.toast else { return };
        ctx.request_repaint_after(t.until.saturating_duration_since(Instant::now()));
        egui::Area::new(Id::unique("toast"))
            .order(egui::Order::Foreground)
            .interactable(false)
            .anchor(Align2::CENTER_TOP, egui::vec2(0.0, 14.0))
            .show(ctx, |ui| {
                // 按当前消息排版，避免沿用上一条短提示的宽度把文件名挤成竖排。
                let galley = ui.painter().layout(
                    t.text.clone(),
                    FontId::proportional(13.0),
                    Color32::WHITE,
                    (ctx.content_rect().width() - 64.0).min(520.0),
                );
                egui::Frame::new()
                    .fill(Color32::from_rgb(0x23, 0x27, 0x33))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::symmetric(14, 10))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 4],
                        blur: 16,
                        spread: 0,
                        color: Color32::from_black_alpha(40),
                    })
                    .show(ui, |ui| {
                        let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
                        ui.painter().galley(rect.min, galley, Color32::WHITE);
                    });
            });
    }
}

#[derive(Default)]
struct ShortcutOut {
    save: bool,
    lock: bool,
}

#[derive(Clone, Debug)]
enum SheetAction {
    Add,
    Select,
    StartRename,
    RenameDone(String),
    Duplicate,
    Delete,
    Move(i32),
    Color(Option<String>),
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

fn clean_sheet_name(buf: &str, sheets: &[Sheet], idx: usize) -> Option<String> {
    let name: String = buf
        .trim()
        .chars()
        .filter(|c| !matches!(c, ':' | '\\' | '/' | '?' | '*' | '[' | ']'))
        .take(31)
        .collect();
    if name.is_empty() {
        return None;
    }
    if sheets
        .iter()
        .enumerate()
        .any(|(i, s)| i != idx && s.name == name)
    {
        return None;
    }
    Some(name)
}


/// 顶栏「‹ 笔记库」：图标与文字都以行中心对齐，避免字形基线错位

/// 在资源管理器中定位文件（选中该文件）
fn reveal_in_explorer(path: &std::path::Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let arg = format!("/select,{}", path.display());
        let _ = std::process::Command::new("explorer")
            .arg(arg)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
}

fn back_button(ui: &mut egui::Ui) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(84.0, 26.0), Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(8), theme::HOVER);
    }
    let col = if resp.hovered() { theme::TEXT } else { theme::DIM };
    let c = pos2(rect.left() + 12.0, rect.center().y);
    let stroke = Stroke::new(1.6, col);
    ui.painter()
        .line_segment([pos2(c.x + 3.5, c.y - 4.0), c], stroke);
    ui.painter()
        .line_segment([c, pos2(c.x + 3.5, c.y + 4.0)], stroke);
    ui.painter().text(
        pos2(rect.left() + 22.0, rect.center().y),
        Align2::LEFT_CENTER,
        "笔记库",
        FontId::proportional(13.5),
        col,
    );
    resp
}

fn lock_glyph(p: &egui::Painter, center: egui::Pos2) {
    let body = egui::Rect::from_center_size(center, vec2(9.0, 7.0));
    p.rect_stroke(
        body,
        1.5,
        Stroke::new(1.2, theme::FAINT),
        egui::StrokeKind::Inside,
    );
    let shackle = egui::Rect::from_center_size(center - vec2(0.0, 5.0), vec2(6.0, 6.0));
    p.circle_stroke(
        shackle.center() + vec2(0.0, 1.0),
        3.0,
        Stroke::new(1.2, theme::FAINT),
    );
}

#[derive(Clone, Copy)]
enum Icon {
    Undo,
    Redo,
    Divider,
    Bullet,
    Number,
    Format(egui_richedit::Mark),
    Align(DocAlign),
    Key,
    Lock,
    Image,
    Clear,
    Save,
}

impl Icon {
    fn label(self) -> &'static str {
        use egui_richedit::Mark;
        match self {
            Self::Undo => "撤销",
            Self::Redo => "重做",
            Self::Divider => "插入分割线",
            Self::Bullet => "项目符号",
            Self::Number => "编号",
            Self::Format(Mark::Bold) => "加粗",
            Self::Format(Mark::Italic) => "斜体",
            Self::Format(Mark::Underline) => "下划线",
            Self::Format(Mark::Strike) => "删除线",
            Self::Align(DocAlign::Left) => "左对齐",
            Self::Align(DocAlign::Center) => "居中对齐",
            Self::Align(DocAlign::Right) => "右对齐",
            Self::Align(DocAlign::Justify) => "两端对齐",
            Self::Key => "修改密码",
            Self::Lock => "锁定",
            Self::Image => "插入剪贴板图片",
            Self::Clear => "清除格式",
            Self::Save => "保存",
        }
    }

    fn shortcut(self) -> Option<&'static str> {
        use egui_richedit::Mark;
        match self {
            Self::Undo => Some("Ctrl+Z"),
            Self::Redo => Some("Ctrl+Y"),
            Self::Format(Mark::Bold) => Some("Ctrl+B"),
            Self::Format(Mark::Italic) => Some("Ctrl+I"),
            Self::Format(Mark::Underline) => Some("Ctrl+U"),
            Self::Lock => Some("Ctrl+L"),
            Self::Save => Some("Ctrl+S"),
            _ => None,
        }
    }

    /// 图形统一放在中心 18px 范围内，以 1.5px 线条绘制，不依赖字体图标。
    fn paint(self, p: &egui::Painter, c: egui::Pos2, color: Color32) {
        use egui_richedit::Mark;
        let stroke = Stroke::new(1.5, color);
        let point = |x, y| c + vec2(x, y);
        let line = |points: &[(f32, f32)]| {
            p.add(egui::Shape::line(
                points.iter().map(|&(x, y)| point(x, y)).collect(), stroke,
            ));
        };
        let curve = |points: [(f32, f32); 4]| {
            p.add(egui::epaint::CubicBezierShape::from_points_stroke(
                points.map(|(x, y)| point(x, y)), false, Color32::TRANSPARENT, stroke,
            ));
        };
        match self {
            Self::Undo | Self::Redo => {
                let direction = if matches!(self, Self::Redo) { -1.0 } else { 1.0 };
                curve([(6.0 * direction, 6.0), (7.0 * direction, -3.0),
                    (2.0 * direction, -4.0), (-6.0 * direction, -4.0)]);
                line(&[(-2.0 * direction, -8.0), (-6.0 * direction, -4.0),
                    (-2.0 * direction, 0.0)]);
            }
            Self::Divider => {
                for y in [-6.0, 6.0] {
                    p.line_segment([point(-5.0, y), point(5.0, y)],
                        Stroke::new(1.5, color.gamma_multiply(0.45)));
                }
                line(&[(-8.0, 0.0), (8.0, 0.0)]);
            }
            Self::Bullet => {
                for y in [-6.0, 0.0, 6.0] {
                    p.circle_filled(point(-6.5, y), 1.25, color);
                    line(&[(-2.0, y), (7.5, y)]);
                }
            }
            Self::Number => {
                line(&[(-8.0, -6.0), (-6.5, -7.5), (-6.5, -2.5)]);
                line(&[(-8.0, -2.5), (-5.0, -2.5)]);
                curve([(-8.0, 3.0), (-8.0, 0.0), (-3.5, 0.0), (-4.5, 3.5)]);
                line(&[(-4.5, 3.5), (-8.0, 7.0), (-4.0, 7.0)]);
                for y in [-5.0, 5.0] {
                    line(&[(-0.5, y), (7.5, y)]);
                }
            }
            Self::Format(Mark::Bold) => {
                line(&[(-4.5, 7.0), (-4.5, -7.0), (0.0, -7.0)]);
                curve([(0.0, -7.0), (6.5, -7.0), (6.5, 0.0), (0.0, 0.0)]);
                line(&[(-4.5, 0.0), (0.0, 0.0)]);
                curve([(0.0, 0.0), (7.5, 0.0), (7.5, 7.0), (0.0, 7.0)]);
                line(&[(0.0, 7.0), (-4.5, 7.0)]);
            }
            Self::Format(Mark::Italic) => {
                line(&[(0.0, -7.0), (7.0, -7.0)]);
                line(&[(3.5, -7.0), (-3.5, 7.0)]);
                line(&[(-7.0, 7.0), (0.0, 7.0)]);
            }
            Self::Format(Mark::Underline) => {
                line(&[(-5.0, -7.5), (-5.0, 0.0)]);
                curve([(-5.0, 0.0), (-5.0, 6.5), (5.0, 6.5), (5.0, 0.0)]);
                line(&[(5.0, 0.0), (5.0, -7.5)]);
                line(&[(-7.0, 8.0), (7.0, 8.0)]);
            }
            Self::Format(Mark::Strike) => {
                curve([(5.0, -5.5), (-4.0, -10.0), (-8.0, -1.0), (0.0, 0.0)]);
                curve([(0.0, 0.0), (8.0, 1.0), (4.0, 10.0), (-5.0, 5.5)]);
                line(&[(-8.0, 0.0), (8.0, 0.0)]);
            }
            Self::Align(align) => {
                for (i, width) in [14.0, 10.0, 14.0, 10.0].into_iter().enumerate() {
                    let width = if align == DocAlign::Justify { 14.0 } else { width };
                    let left = match align {
                        DocAlign::Right => 7.0 - width,
                        DocAlign::Center => -width * 0.5,
                        _ => -7.0,
                    };
                    let y = -6.0 + i as f32 * 4.0;
                    line(&[(left, y), (left + width, y)]);
                }
            }
            Self::Key => {
                p.circle_stroke(point(-4.0, -4.0), 3.5, stroke);
                line(&[(-1.5, -1.5), (7.0, 7.0), (8.5, 5.5)]);
                line(&[(3.5, 3.5), (5.5, 1.5)]);
            }
            Self::Lock => {
                p.rect_stroke(egui::Rect::from_min_max(point(-6.0, -1.0), point(6.0, 8.0)),
                    2.0, stroke, egui::StrokeKind::Middle);
                line(&[(-3.5, -1.0), (-3.5, -4.0)]);
                curve([(-3.5, -4.0), (-3.5, -9.0), (3.5, -9.0), (3.5, -4.0)]);
                line(&[(3.5, -4.0), (3.5, -1.0)]);
                line(&[(0.0, 2.5), (0.0, 5.0)]);
            }
            Self::Image => {
                p.rect_stroke(egui::Rect::from_center_size(c, vec2(16.0, 14.0)),
                    2.0, stroke, egui::StrokeKind::Middle);
                p.circle_stroke(point(-3.5, -3.0), 1.2, stroke);
                line(&[(-7.0, 4.5), (-2.5, 0.0), (2.5, 5.0)]);
                line(&[(0.5, 3.0), (4.0, -0.5), (7.0, 2.5)]);
            }
            Self::Clear => {
                line(&[(-7.0, -6.5), (3.0, -6.5)]);
                line(&[(-2.0, -6.5), (-4.0, 6.0)]);
                line(&[(2.5, 2.0), (7.5, 7.0)]);
                line(&[(7.5, 2.0), (2.5, 7.0)]);
            }
            Self::Save => {
                line(&[(-7.0, -7.5), (4.0, -7.5), (7.5, -4.0),
                    (7.5, 7.5), (-7.0, 7.5), (-7.0, -7.5)]);
                line(&[(-3.0, -7.5), (-3.0, -2.0), (2.5, -2.0), (2.5, -7.5)]);
                line(&[(-3.0, 7.5), (-3.0, 2.0), (3.5, 2.0), (3.5, 7.5)]);
            }
        }
    }
}

fn icon_button(
    ui: &mut egui::Ui, icon: Icon, size: f32, on: bool, enabled: bool,
) -> egui::Response {
    let enabled = enabled && ui.is_enabled();
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), sense);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, icon.label()));
    if ui.is_rect_visible(rect) {
        let bg = if on {
            theme::ACCENT_SOFT
        } else if enabled && resp.is_pointer_button_down_on() {
            theme::PRESS
        } else if enabled && resp.hovered() {
            theme::CHIP
        } else {
            Color32::TRANSPARENT
        };
        let color = if !enabled {
            theme::FAINT
        } else if on {
            theme::ACCENT
        } else {
            Color32::from_rgb(0x5B, 0x63, 0x74)
        };
        ui.painter().rect_filled(rect, 6.0, bg);
        if resp.has_focus() {
            ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.0, theme::ACCENT),
                egui::StrokeKind::Inside);
        }
        icon.paint(ui.painter(), rect.center(), color);
    }
    widgets::tooltip(resp, icon.label(), icon.shortcut())
}

fn pw_field(ui: &mut egui::Ui, value: &mut String, hint: &str) -> egui::Response {
    ui.add(
        egui::TextEdit::singleline(value)
            .password(true)
            .hint_text(hint)
            .desired_width(f32::INFINITY)
            .margin(Margin::symmetric(10, 8)),
    )
}

fn strength(pw: &str) -> (f32, &'static str, Color32) {
    let n = pw.chars().count();
    let has_digit = pw.chars().any(|c| c.is_ascii_digit());
    let has_alpha = pw.chars().any(|c| c.is_alphabetic());
    let has_other = pw.chars().any(|c| !c.is_alphanumeric());
    let mut score = 0;
    if n >= 8 {
        score += 1;
    }
    if n >= 12 {
        score += 1;
    }
    if has_digit {
        score += 1;
    }
    if has_alpha {
        score += 1;
    }
    if has_other {
        score += 1;
    }
    match score {
        0..=1 => (0.25, "弱", theme::DANGER),
        2..=3 => (0.55, "中等", theme::WARN),
        _ => (1.0, "强", theme::OK),
    }
}

fn modal(
    ctx: &egui::Context,
    id: &str,
    width: f32,
    add: impl FnOnce(&mut egui::Ui),
) -> bool {
    egui::Modal::new(Id::unique(id))
        .backdrop_color(Color32::from_black_alpha(80))
        .frame(
            egui::Frame::new()
                .fill(theme::CARD)
                .corner_radius(CornerRadius::same(16))
                .inner_margin(Margin::same(20))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 10],
                    blur: 30,
                    spread: 0,
                    color: Color32::from_black_alpha(50),
                }),
        )
        .show(ctx, |ui| {
            ui.set_width(width);
            add(ui);
        })
        .should_close()
}

// ---- 选区/格式辅助 ----

fn selection_of(ed: &EditorCtx, active: usize) -> Option<((usize, usize), (usize, usize))> {
    let sel = ed.sheets[active].editor.selection()?;
    Some((
        (sel.anchor.paragraph, sel.anchor.offset),
        (sel.focus.paragraph, sel.focus.offset),
    ))
}

fn caret_of(ed: &EditorCtx, active: usize) -> ((usize, usize), (usize, usize)) {
    let p = ed.sheets[active].focus_position(&ed.opened.doc.sheets[active].blocks)
        .map(|p| (p.paragraph, p.offset))
        .unwrap_or((0, 0));
    (p, p)
}

fn marked_between(
    sheet: &Sheet,
    a: (usize, usize),
    b: (usize, usize),
    mark: egui_richedit::Mark,
) -> Option<bool> {
    let bit = match mark {
        egui_richedit::Mark::Bold => Run::BOLD,
        egui_richedit::Mark::Italic => Run::ITALIC,
        egui_richedit::Mark::Underline => Run::UNDERLINE,
        egui_richedit::Mark::Strike => Run::STRIKE,
    };
    if a == b {
        return None;
    }
    let mut value = None;
    for p in a.0..=b.0.min(sheet.blocks.len().saturating_sub(1)) {
        for r in &sheet.blocks[p].runs {
            if r.text.is_empty() {
                continue;
            }
            let v = r.marks & bit != 0;
            match value {
                None => value = Some(v),
                Some(prev) if prev != v => return None,
                _ => {}
            }
        }
    }
    value
}

/// 外部格式命令（工具栏）改 blocks 前先压一份撤销快照。
fn push_undo(ed: &mut EditorCtx, active: usize) {
    let blocks = ed.opened.doc.sheets[active].blocks.clone();
    ed.sheets[active].undo.snapshot(&blocks);
}

fn apply_marks(
    ed: &mut EditorCtx,
    active: usize,
    from: (usize, usize),
    to: (usize, usize),
    bit: u8,
) {
    if from == to {
        let pending = &mut ed.sheets[active].pending;
        if pending.marks & bit != 0 {
            pending.marks &= !bit;
        } else {
            pending.marks |= bit;
        }
        return;
    }
    let on = marked_between(&ed.opened.doc.sheets[active], from, to, mark_of(bit).unwrap())
        == Some(true);
    push_undo(ed, active);
    let sheet = &mut ed.opened.doc.sheets[active];
    editor::format_range_marks(&mut sheet.blocks, from, to, bit, !on);
    ed.dirty = true;
}

fn current_block_kind(ed: &EditorCtx, active: usize) -> BlockKind {
    let p = caret_of(ed, active).0.0;
    ed.opened.doc.sheets[active]
        .blocks
        .get(p)
        .map(|b| b.kind)
        .unwrap_or_default()
}

fn current_align(ed: &EditorCtx, active: usize) -> Option<DocAlign> {
    let p = caret_of(ed, active).0.0;
    ed.opened.doc.sheets[active]
        .blocks
        .get(p)
        .and_then(|b| b.align)
}

fn current_size(ed: &EditorCtx, active: usize) -> Option<f32> {
    let (from, to) = selection_of(ed, active)?;
    editor::uniform_size_public(&ed.opened.doc.sheets[active].blocks, from, to)
}

fn color_menu(
    ui: &mut egui::Ui,
    label: &str,
    accent: Color32,
    ed: &mut EditorCtx,
    active: usize,
    highlight: bool,
) {
    ui.menu_button(
        RichText::new(label).color(accent),
        |ui| {
            for hex in [
                "#171A23", "#E5484D", "#F79009", "#EAB308", "#12B76A", "#06AED4", "#3B82F6",
                "#7C5CFC", "#B9BFCA",
            ] {
                let c = editor::parse_color(hex).unwrap_or(theme::TEXT);
                let (rect, resp) = ui.allocate_exact_size(vec2(150.0, 26.0), Sense::click());
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(rect.left_top() + vec2(4.0, 7.0), vec2(12.0, 12.0)),
                    3.0,
                    c,
                );
                ui.painter().text(
                    rect.left_center() + vec2(26.0, 0.0),
                    Align2::LEFT_CENTER,
                    hex,
                    FontId::proportional(11.0),
                    theme::TEXT,
                );
                if resp.clicked() {
                    let (from, to) = selection_of(ed, active)
                        .unwrap_or_else(|| caret_of(ed, active));
                    if from != to {
                        push_undo(ed, active);
                        let sheet = &mut ed.opened.doc.sheets[active];
                        editor::set_text_color(&mut sheet.blocks, from, to, hex, highlight);
                        ed.dirty = true;
                    } else if highlight {
                        ed.sheets[active].pending.bg = Some(hex.to_owned());
                    } else {
                        ed.sheets[active].pending.color = Some(hex.to_owned());
                    }
                    ui.close();
                }
            }
        },
    );
}

/// 从剪贴板读取位图并编码为 PNG（超过 1600px 宽会等比缩小）。
fn clipboard_image_asset() -> Option<ImageAsset> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let img = clipboard.get_image().ok()?;
    if img.width == 0 || img.height == 0 {
        return None;
    }
    let rgba = image::RgbaImage::from_raw(
        img.width as u32,
        img.height as u32,
        img.bytes.into_owned(),
    )?;
    let rgba = if rgba.width() > 1600 {
        let h = (rgba.height() as u64 * 1600 / rgba.width() as u64).max(1) as u32;
        image::imageops::resize(&rgba, 1600, h, image::imageops::FilterType::Lanczos3)
    } else {
        rgba
    };
    let (w, h) = (rgba.width(), rgba.height());
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut png, image::ImageFormat::Png)
        .ok()?;
    let bytes = png.into_inner();
    if bytes.len() > 4 * 1024 * 1024 {
        return None;
    }
    use base64::Engine;
    Some(ImageAsset {
        id: format!("img-{}", recent::random_id()),
        mime: "image/png".to_owned(),
        w,
        h,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// 把 egui 的粘贴位图编码为 PNG 资产（超过 1600px 宽会等比缩小）。
fn asset_from_color_image(img: &egui::ColorImage) -> Option<ImageAsset> {
    let (w, h) = (img.size[0] as u32, img.size[1] as u32);
    if w == 0 || h == 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity((w * h * 4) as usize);
    for px in &img.pixels {
        bytes.extend_from_slice(&[px.r(), px.g(), px.b(), px.a()]);
    }
    let rgba = image::RgbaImage::from_raw(w, h, bytes)?;
    let rgba = if rgba.width() > 1600 {
        let nh = (rgba.height() as u64 * 1600 / rgba.width() as u64).max(1) as u32;
        image::imageops::resize(&rgba, 1600, nh, image::imageops::FilterType::Lanczos3)
    } else {
        rgba
    };
    let (w, h) = (rgba.width(), rgba.height());
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut png, image::ImageFormat::Png)
        .ok()?;
    let bytes = png.into_inner();
    if bytes.len() > 4 * 1024 * 1024 {
        return None;
    }
    use base64::Engine;
    Some(ImageAsset {
        id: format!("img-{}", recent::random_id()),
        mime: "image/png".to_owned(),
        w,
        h,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// 撤销/重做后把光标夹回合法位置：段落数变少时落到最后一段，
/// 偏移超过段落长度时贴到行尾。
fn clamp_caret(blocks: &[Block], pos: Option<Position<usize>>) -> Position<usize> {
    let Some(pos) = pos else {
        return Position::new(0, 0);
    };
    let p = pos.paragraph.min(blocks.len().saturating_sub(1));
    let max = blocks.get(p).map(|b| {
        if b.kind == BlockKind::Image { 1 } else { b.char_len() }
    }).unwrap_or(0);
    Position::new(p, pos.offset.min(max))
}

fn restore_history_selection(
    ctx: &egui::Context,
    rt: &mut SheetEditor,
    blocks: &[Block],
    caret: Option<Position<usize>>,
) {
    rt.editor.document_replaced();
    if let Some(id) = rt.selected_image.clone()
        && editor::image_index(blocks, &id).is_some()
    {
        rt.select_image(ctx, id, false);
    } else {
        rt.focus_text(clamp_caret(blocks, caret));
    }
    rt.scroll_pin = Some((rt.scroll, 20));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct DroppedNote(PathBuf);

    impl egui::DroppedFile for DroppedNote {
        fn path(&self) -> &std::path::Path {
            &self.0
        }

        fn bytes(&self) -> Result<Vec<u8>, String> {
            std::fs::read(&self.0).map_err(|e| e.to_string())
        }
    }

    fn dropped_file_frame(
        ctx: &egui::Context,
        app: &mut App,
        paths: &[PathBuf],
        events: Vec<egui::Event>,
    ) {
        let mut frame = eframe::Frame::_new_kittest();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO, vec2(1180.0, 780.0),
                )),
                dropped_files: paths.iter().map(|path| {
                    std::sync::Arc::new(DroppedNote(path.clone())) as egui::DroppedFileHandle
                }).collect(),
                events,
                ..Default::default()
            },
            |ui| eframe::App::ui(app, ui, &mut frame),
        );
        output.textures_delta.clear();
    }

    fn wait_for_open(ctx: &egui::Context, app: &mut App) {
        let deadline = Instant::now() + std::time::Duration::from_secs(20);
        while app.pending.is_some() {
            assert!(Instant::now() < deadline, "解锁任务超时");
            std::thread::sleep(std::time::Duration::from_millis(10));
            dropped_file_frame(ctx, app, &[], Vec::new());
        }
    }

    #[test]
    fn dropped_files_open_only_notes_and_do_not_replace_an_inflight_unlock() {
        let dir = std::env::temp_dir().join(format!("mijian-drop-{}", recent::random_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = dir.join("说明.txt");
        let folder = dir.join("文件夹.ecnote");
        let first = dir.join("第一份.ecnote");
        let second = dir.join("第二份.ecnote");
        for path in [&text, &first, &second] {
            std::fs::write(path, []).unwrap();
        }
        std::fs::create_dir(&folder).unwrap();
        let mut app = App {
            view: View::Library,
            entries: Vec::new(),
            new: NewState::default(),
            unlock: None,
            pending: None,
            editor: None,
            toast: None,
            new_dir: None,
            open_error: None,
        };
        let ctx = egui::Context::default();
        dropped_file_frame(&ctx, &mut app, &[text.clone(), folder.clone()], Vec::new());
        assert!(app.unlock.is_none());
        dropped_file_frame(&ctx, &mut app, &[first.clone(), second.clone()], Vec::new());
        assert!(app.view == View::Library);
        assert!(app.editor.is_none());
        assert_eq!(app.unlock.as_ref().unwrap().path, first);
        assert!(app.entries.is_empty());

        let (_tx, rx) = channel();
        app.pending = Some(Pending { rx, task: Task::Open });
        app.unlock.as_mut().unwrap().busy = true;
        dropped_file_frame(&ctx, &mut app, &[second.clone()], Vec::new());
        assert_eq!(app.unlock.as_ref().unwrap().path, first);
        assert!(app.pending.is_some());
        dropped_file_frame(&ctx, &mut app, &[], vec![
            image_key(egui::Key::Escape, egui::Modifiers::NONE),
        ]);
        assert!(app.unlock.is_some());
        for path in [text, first, second] {
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir(folder).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn dropped_note_keeps_edits_on_wrong_password_and_saves_before_switching() {
        let dir = std::env::temp_dir().join(format!("mijian-drop-{}", recent::random_id()));
        let current = store::create(&dir, "当前笔记", "current-password").unwrap();
        let target = store::create(&dir, "拖入笔记", "target-password").unwrap().path;
        let current_path = current.path.clone();
        let current_bytes = std::fs::read(&current_path).unwrap();
        let target_bytes = std::fs::read(&target).unwrap();
        let mut ed = EditorCtx::new(current);
        ed.opened.doc.sheets[0].blocks = vec![Block::with_text("尚未保存的编辑")];
        ed.sheets[0].undo.snapshot(&[Block::with_text("修改前")]);
        ed.sheets[0].editor.select(Selection::caret(Position::new(0, 3)));
        ed.dirty = true;
        let mut app = App {
            view: View::Editor,
            entries: Vec::new(),
            new: NewState::default(),
            unlock: None,
            pending: None,
            editor: Some(ed),
            toast: None,
            new_dir: None,
            open_error: None,
        };
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        for _ in 0..3 {
            dropped_file_frame(&ctx, &mut app, &[], Vec::new());
        }
        let original = serde_json::to_vec(&app.editor.as_ref().unwrap().opened.doc).unwrap();
        let selection = app.editor.as_ref().unwrap().sheets[0].editor.selection().cloned();
        dropped_file_frame(&ctx, &mut app, &[target.clone()], Vec::new());
        dropped_file_frame(&ctx, &mut app, &[], vec![
            image_key(egui::Key::Z, egui::Modifiers::CTRL),
            image_key(egui::Key::N, egui::Modifiers::CTRL),
        ]);
        app.unlock.as_mut().unwrap().password = "wrong-password".to_owned();
        app.start_open(&ctx);
        wait_for_open(&ctx, &mut app);
        assert_eq!(app.unlock.as_ref().unwrap().error.as_deref(), Some("密码不正确"));
        let ed = app.editor.as_ref().unwrap();
        assert!(app.view == View::Editor);
        assert_eq!(ed.opened.path, current_path);
        assert_eq!(serde_json::to_vec(&ed.opened.doc).unwrap(), original);
        assert_eq!(ed.sheets[0].editor.selection(), selection.as_ref());
        assert_eq!(ed.sheets[0].undo.undo.len(), 1);
        assert!(ed.dirty);
        assert_eq!(std::fs::read(&current_path).unwrap(), current_bytes);
        assert_eq!(std::fs::read(&target).unwrap(), target_bytes);

        dropped_file_frame(&ctx, &mut app, &[], vec![
            image_key(egui::Key::Escape, egui::Modifiers::NONE),
        ]);
        assert!(app.unlock.is_none());
        dropped_file_frame(&ctx, &mut app, &[], Vec::new());
        assert_eq!(serde_json::to_vec(&app.editor.as_ref().unwrap().opened.doc).unwrap(), original);
        assert!(ctx.memory(|m| m.has_focus(app.editor.as_ref().unwrap().sheets[0].id)));

        dropped_file_frame(&ctx, &mut app, &[target.clone()], Vec::new());
        app.unlock.as_mut().unwrap().password = "target-password".to_owned();
        app.start_open(&ctx);
        wait_for_open(&ctx, &mut app);
        assert!(app.unlock.is_none());
        assert_eq!(app.editor.as_ref().unwrap().opened.path, target);
        assert_eq!(
            store::open(&current_path, "current-password").unwrap().doc.sheets[0].blocks[0].text(),
            "尚未保存的编辑",
        );
        assert_eq!(std::fs::read(&target).unwrap(), target_bytes);

        let ed = app.editor.as_mut().unwrap();
        ed.opened.doc.sheets[0].blocks = vec![Block::with_text("重复拖入时保留")];
        ed.dirty = true;
        dropped_file_frame(&ctx, &mut app, &[target.clone()], Vec::new());
        app.unlock.as_mut().unwrap().password = "target-password".to_owned();
        app.start_open(&ctx);
        wait_for_open(&ctx, &mut app);
        assert!(app.unlock.is_none());
        let ed = app.editor.as_ref().unwrap();
        assert!(ed.dirty);
        assert_eq!(ed.opened.doc.sheets[0].blocks[0].text(), "重复拖入时保留");
        assert_eq!(std::fs::read(&target).unwrap(), target_bytes);

        recent::forget(&target);
        std::fs::remove_file(current_path).unwrap();
        std::fs::remove_file(target).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn dropped_note_keeps_image_selection_and_stays_put_if_saving_fails() {
        let mut app = image_test_app();
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let ed = app.editor.as_mut().unwrap();
        let image_id = ed.opened.doc.images[0].id.clone();
        ed.sheets[0].select_image(&ctx, image_id.clone(), false);
        ed.dirty = true;
        for _ in 0..3 {
            dropped_file_frame(&ctx, &mut app, &[], Vec::new());
        }
        let original = serde_json::to_value(&app.editor.as_ref().unwrap().opened.doc.sheets).unwrap();
        let dir = std::env::temp_dir().join(format!("mijian-drop-{}", recent::random_id()));
        let opened = store::create(&dir, "拖入笔记", "target-password").unwrap();
        let target = opened.path.clone();
        dropped_file_frame(&ctx, &mut app, &[target.clone()], Vec::new());
        dropped_file_frame(&ctx, &mut app, &[], Vec::new());
        dropped_file_frame(&ctx, &mut app, &[], vec![
            egui::Event::PointerMoved(pos2(590.0, 390.0)),
            egui::Event::PointerButton {
                pos: pos2(590.0, 390.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        assert_eq!(app.editor.as_ref().unwrap().sheets[0].selected_image, Some(image_id));
        let (tx, rx) = channel();
        tx.send(Ok(opened)).unwrap();
        app.pending = Some(Pending { rx, task: Task::Open });
        app.poll_pending();
        assert!(app.view == View::Editor);
        assert!(app.editor.as_ref().unwrap().dirty);
        assert_eq!(serde_json::to_value(&app.editor.as_ref().unwrap().opened.doc.sheets).unwrap(), original);
        assert!(app.unlock.as_ref().unwrap().error.as_deref().unwrap().contains("当前笔记保存失败"));
        assert!(!app.unlock.as_ref().unwrap().busy);
        std::fs::remove_file(target).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    fn image_test_app() -> App {
        let dir = std::env::temp_dir().join(format!("mijian-image-{}", recent::random_id()));
        let opened = store::create(&dir, "image-interaction", "test-only").unwrap();
        std::fs::remove_file(&opened.path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        let mut ed = EditorCtx::new(opened);
        let asset =
            asset_from_color_image(&egui::ColorImage::filled([320, 240], theme::FIELD)).unwrap();
        let mut image = Block::with_text(&asset.id);
        image.kind = BlockKind::Image;
        ed.opened.doc.images.push(asset);
        ed.opened.doc.sheets[0].blocks = vec![
            Block::with_text("Before image."),
            image,
            Block::with_text("After image."),
        ];
        App {
            view: View::Editor,
            entries: Vec::new(),
            new: NewState::default(),
            unlock: None,
            pending: None,
            editor: Some(ed),
            toast: None,
            new_dir: None,
            open_error: None,
        }
    }

    fn image_frame(ctx: &egui::Context, app: &mut App, events: Vec<egui::Event>) -> egui::FullOutput {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO, vec2(1180.0, 780.0),
                )),
                events,
                ..Default::default()
            },
            |ui| app.editor_ui(ui),
        );
        output.textures_delta.clear();
        output
    }

    fn image_key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    #[test]
    fn image_selection_releases_text_and_clicking_text_releases_image() {
        let mut app = image_test_app();
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let frame = |app: &mut App, events| image_frame(&ctx, app, events);
        for _ in 0..3 {
            frame(&mut app, Vec::new());
        }
        let output = frame(&mut app, Vec::new());
        let image = output.shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Rect(r)
                if r.stroke.color == theme::BORDER && r.rect.height() > 200.0 =>
            {
                Some(r.rect)
            }
            _ => None,
        }).unwrap();
        let text = output.shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == "After image." => Some(t.pos),
            _ => None,
        }).unwrap();
        let click = |app: &mut App, pos| {
            for pressed in [true, false] {
                frame(app, vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
            }
        };
        click(&mut app, image.center());
        assert!(app.editor.as_ref().unwrap().sheets[0].selected_image.is_some());
        assert!(
            app.editor.as_ref().unwrap().sheets[0].editor.selection().is_none(),
            "选中图片时不能同时保留文本光标"
        );
        click(&mut app, pos2(300.0, 20.0));
        assert!(
            app.editor.as_ref().unwrap().sheets[0].selected_image.is_some(),
            "操作编辑区外的工具栏时应保留图片选择"
        );
        click(&mut app, text + vec2(22.0, 8.0));
        assert!(
            app.editor.as_ref().unwrap().sheets[0].selected_image.is_none(),
            "点回正文应立即退出图片选择"
        );
        frame(&mut app, vec![egui::Event::Key {
            key: egui::Key::Delete,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert_eq!(app.editor.as_ref().unwrap().opened.doc.sheets[0].blocks[1].kind, BlockKind::Image);
    }

    #[test]
    fn image_drag_actions_and_keyboard_handoff_have_undo_boundaries() {
        let mut app = image_test_app();
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let frame = |app: &mut App, events| image_frame(&ctx, app, events);
        let image_rect = |output: &egui::FullOutput| output.shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Rect(r)
                if r.stroke.color == theme::BORDER && r.rect.height() > 150.0 =>
            {
                Some(r.rect)
            }
            _ => None,
        }).unwrap();
        let button = |pos, pressed| egui::Event::PointerButton {
            pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
        };
        let drag = |app: &mut App, from, to| {
            frame(app, vec![egui::Event::PointerMoved(from), button(from, true)]);
            frame(app, vec![egui::Event::PointerMoved(to)]);
            frame(app, Vec::new());
            frame(app, vec![button(to, false)]);
        };
        for _ in 0..3 { frame(&mut app, Vec::new()); }
        let original = image_rect(&frame(&mut app, Vec::new()));
        let image_id = app.editor.as_ref().unwrap().opened.doc.images[0].id.clone();
        for pressed in [true, false] {
            frame(&mut app, vec![egui::Event::PointerMoved(original.center()), button(original.center(), pressed)]);
        }
        frame(&mut app, Vec::new());
        drag(&mut app, original.right_bottom(), original.right_bottom() + vec2(80.0, 60.0));
        let output = frame(&mut app, Vec::new());
        let resized = image_rect(&output);
        assert!((resized.width() - original.width() - 80.0).abs() < 1.0);
        assert!((resized.height() / resized.width() - 0.75).abs() < 0.001);
        assert_eq!(app.editor.as_ref().unwrap().sheets[0].undo.undo.len(), 1);
        assert!(app.editor.as_ref().unwrap().sheets[0].editor.selection().is_none());

        let after = output.shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == "After image." => Some(t.pos),
            _ => None,
        }).unwrap();
        drag(&mut app, pos2(resized.left() - 19.0, resized.top() + 15.0), after + vec2(50.0, 35.0));
        assert_eq!(editor::image_index(&app.editor.as_ref().unwrap().opened.doc.sheets[0].blocks, &image_id), Some(2));
        assert_eq!(app.editor.as_ref().unwrap().sheets[0].undo.undo.len(), 2);
        for _ in 0..2 {
            frame(&mut app, vec![image_key(egui::Key::Z, egui::Modifiers::CTRL)]);
            frame(&mut app, Vec::new());
        }
        let ed = app.editor.as_ref().unwrap();
        assert_eq!(ed.opened.doc.sheets[0].blocks[1].image_width, None);
        assert_eq!(ed.sheets[0].selected_image.as_deref(), Some(image_id.as_str()));
        assert!(ed.sheets[0].undo.undo.is_empty());

        App::apply_image_action(&ctx, app.editor.as_mut().unwrap(), editor::ImageAction::Align(image_id.clone(), DocAlign::Right));
        assert_eq!(app.editor.as_ref().unwrap().sheets[0].undo.undo.len(), 1);
        assert!(app.editor.as_ref().unwrap().sheets[0].undo.redo.is_empty());
        frame(&mut app, vec![image_key(egui::Key::Delete, egui::Modifiers::NONE)]);
        assert!(editor::image_index(&app.editor.as_ref().unwrap().opened.doc.sheets[0].blocks, &image_id).is_none());
        frame(&mut app, vec![image_key(egui::Key::Z, egui::Modifiers::CTRL)]);
        frame(&mut app, Vec::new());
        assert_eq!(app.editor.as_ref().unwrap().sheets[0].selected_image.as_deref(), Some(image_id.as_str()));

        let count = app.editor.as_ref().unwrap().opened.doc.sheets[0].blocks.len();
        frame(&mut app, vec![image_key(egui::Key::Enter, egui::Modifiers::NONE)]);
        let ed = app.editor.as_ref().unwrap();
        assert_eq!(ed.opened.doc.sheets[0].blocks.len(), count + 1);
        assert!(ed.opened.doc.sheets[0].blocks[2].is_empty());
        assert_eq!(ed.sheets[0].editor.selection().unwrap().focus, Position::new(2, 0));
        frame(&mut app, vec![egui::Event::Text("继续写".to_owned())]);
        assert_eq!(app.editor.as_ref().unwrap().opened.doc.sheets[0].blocks[2].text(), "继续写");

        app.editor.as_mut().unwrap().sheets[0].select_image(&ctx, image_id.clone(), false);
        frame(&mut app, Vec::new());
        frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Preedit {
            text: "中文".to_owned(), active_range_chars: None,
        })]);
        frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("中文".to_owned()))]);
        let ed = app.editor.as_ref().unwrap();
        assert_eq!(ed.opened.doc.sheets[0].blocks[2].text(), "中文");
        assert_eq!(ed.opened.doc.sheets[0].blocks[3].text(), "继续写");
        assert_eq!(ed.opened.doc.sheets[0].blocks[1].text(), image_id);
    }

    #[test]
    fn image_resize_corners_follow_pointer_and_clicks_do_not_create_undo() {
        let mut app = image_test_app();
        let original_blocks = app.editor.as_ref().unwrap().opened.doc.sheets[0].blocks.clone();
        for align in [DocAlign::Left, DocAlign::Center, DocAlign::Right] {
            for corner in [vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(-1.0, 1.0), vec2(1.0, 1.0)] {
                let ctx = egui::Context::default();
                theme::apply(&ctx);
                let ed = app.editor.as_mut().unwrap();
                ed.opened.doc.sheets[0].blocks = original_blocks.clone();
                ed.sheets[0] = SheetEditor::new(ed.sheets[0].id);
                ed.dirty = false;
                ed.opened.doc.sheets[0].blocks[1].align = Some(align);
                ed.sheets[0].select_image(&ctx, ed.opened.doc.images[0].id.clone(), false);
                let frame = |app: &mut App, events| image_frame(&ctx, app, events);
                let image_rect = |output: &egui::FullOutput| output.shapes.iter().find_map(|s| match &s.shape {
                    egui::Shape::Rect(r) if r.stroke.color == theme::BORDER && r.rect.height() > 150.0 => Some(r.rect),
                    _ => None,
                }).unwrap();
                let button = |pos, pressed| egui::Event::PointerButton {
                    pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
                };
                for _ in 0..3 { frame(&mut app, Vec::new()); }
                let original = image_rect(&frame(&mut app, Vec::new()));
                let from = original.center() + original.size() * corner * 0.5;
                for pressed in [true, false] {
                    frame(&mut app, vec![egui::Event::PointerMoved(from), button(from, pressed)]);
                }
                assert!(app.editor.as_ref().unwrap().sheets[0].undo.undo.is_empty());
                assert!(!app.editor.as_ref().unwrap().dirty);
                let to = from + vec2(24.0, 18.0) * corner;
                frame(&mut app, vec![egui::Event::PointerMoved(from), button(from, true)]);
                let preview = image_rect(&frame(&mut app, vec![egui::Event::PointerMoved(to)]));
                let handle = preview.center() + preview.size() * corner * 0.5;
                assert!(handle.distance(to) < 1.0, "{align:?} {corner:?}: {handle:?} != {to:?}");
                frame(&mut app, vec![button(to, false)]);
                let committed = image_rect(&frame(&mut app, Vec::new()));
                assert!((committed.width() - original.width() - 24.0).abs() < 1.0);
                assert!((committed.height() / committed.width() - 0.75).abs() < 0.001);
                assert!((committed.top() - original.top()).abs() < 1.0);
                let alignment_shift = match align {
                    DocAlign::Left => committed.left() - original.left(),
                    DocAlign::Right => committed.right() - original.right(),
                    _ => committed.center().x - original.center().x,
                };
                assert!(alignment_shift.abs() < 1.0);
                assert_eq!(app.editor.as_ref().unwrap().sheets[0].undo.undo.len(), 1);
            }
        }
    }

    #[test]
    fn image_input_lifecycle_and_redo_keep_the_editing_position() {
        let mut app = image_test_app();
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let frame = |app: &mut App, events| image_frame(&ctx, app, events);
        let ed = app.editor.as_mut().unwrap();
        let id = ed.opened.doc.images[0].id.clone();
        ed.sheets[0].select_image(&ctx, id.clone(), false);
        for _ in 0..3 { frame(&mut app, Vec::new()); }
        for event in [
            egui::Event::Ime(egui::ImeEvent::Preedit { text: String::new(), active_range_chars: None }),
            egui::Event::Ime(egui::ImeEvent::Commit(String::new())),
        ] {
            frame(&mut app, vec![event]);
            let ed = app.editor.as_ref().unwrap();
            assert_eq!(ed.sheets[0].selected_image.as_deref(), Some(id.as_str()));
            assert_eq!(ed.opened.doc.sheets[0].blocks.len(), 3);
            assert!(!ed.dirty);
        }
        ctx.memory_mut(|m| m.request_focus(egui::Id::unique("toolbar-button")));
        App::apply_image_action(&ctx, app.editor.as_mut().unwrap(), editor::ImageAction::Align(id.clone(), DocAlign::Center));
        let ed = app.editor.as_ref().unwrap();
        assert!(ctx.memory(|m| m.has_focus(ed.sheets[0].id.with("image"))));
        assert!(ed.sheets[0].undo.undo.is_empty());
        frame(&mut app, vec![image_key(egui::Key::Delete, egui::Modifiers::NONE)]);
        frame(&mut app, vec![image_key(egui::Key::Z, egui::Modifiers::CTRL)]);
        frame(&mut app, Vec::new());
        assert_eq!(app.editor.as_ref().unwrap().sheets[0].selected_image.as_deref(), Some(id.as_str()));
        frame(&mut app, vec![image_key(egui::Key::Y, egui::Modifiers::CTRL)]);
        frame(&mut app, vec![egui::Event::Text("接着写".to_owned())]);
        let ed = app.editor.as_ref().unwrap();
        assert_eq!(ed.opened.doc.sheets[0].blocks[0].text(), "Before image.");
        assert_eq!(ed.opened.doc.sheets[0].blocks[1].text(), "接着写After image.");
    }

    #[test]
    fn empty_text_between_images_can_select_either_neighbor() {
        let mut app = image_test_app();
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let ed = app.editor.as_mut().unwrap();
        let mut next = ed.opened.doc.images[0].clone();
        next.id.push_str("-next");
        let previous_id = ed.opened.doc.images[0].id.clone();
        let next_id = next.id.clone();
        ed.opened.doc.images.push(next);
        let mut image = ed.opened.doc.sheets[0].blocks[1].clone();
        image.runs[0].text = next_id.clone();
        ed.opened.doc.sheets[0].blocks = vec![
            ed.opened.doc.sheets[0].blocks[1].clone(), Block::empty(), image,
        ];
        for (key, expected) in [(egui::Key::Backspace, previous_id), (egui::Key::Delete, next_id)] {
            app.editor.as_mut().unwrap().sheets[0].focus_text(Position::new(1, 0));
            for _ in 0..3 { image_frame(&ctx, &mut app, Vec::new()); }
            image_frame(&ctx, &mut app, vec![image_key(key, egui::Modifiers::NONE)]);
            let ed = app.editor.as_ref().unwrap();
            assert_eq!(ed.sheets[0].selected_image.as_deref(), Some(expected.as_str()));
            assert_eq!(ed.opened.doc.sheets[0].blocks.len(), 3);
        }
    }

    #[test]
    fn pasting_image_splits_the_focused_text_and_selects_the_new_image() {
        let mut app = image_test_app();
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let ed = app.editor.as_mut().unwrap();
        let original = ed.opened.doc.sheets[0].blocks.clone();
        ed.sheets[0].focus_text(Position::new(0, 7));
        for _ in 0..3 { image_frame(&ctx, &mut app, Vec::new()); }
        image_frame(&ctx, &mut app, vec![egui::Event::PasteImage(std::sync::Arc::new(
            egui::ColorImage::filled([160, 80], theme::ACCENT),
        ))]);
        let ed = app.editor.as_ref().unwrap();
        let blocks = &ed.opened.doc.sheets[0].blocks;
        assert_eq!(blocks[0].text(), "Before ");
        assert_eq!(blocks[1].kind, BlockKind::Image);
        assert_eq!(blocks[2].text(), "image.");
        assert_eq!(blocks[3..], original[1..]);
        assert_eq!(ed.sheets[0].selected_image.as_deref(), Some(blocks[1].text().as_str()));
        assert!(ed.sheets[0].editor.selection().is_none());
        assert_eq!(ed.sheets[0].undo.undo.len(), 1);
        image_frame(&ctx, &mut app, vec![image_key(egui::Key::Z, egui::Modifiers::CTRL)]);
        assert_eq!(app.editor.as_ref().unwrap().opened.doc.sheets[0].blocks, original);
    }

    #[test]
    fn paper_fills_viewport_grows_with_content_and_scrolls_with_it() {
        fn paper_rect(
            shape: &egui::Shape,
            clip: egui::Rect,
        ) -> Option<(egui::Rect, egui::Rect)> {
            match shape {
                egui::Shape::Rect(rect)
                    if rect.fill == theme::CARD
                        && rect.rect.width() > 400.0
                        && rect.rect.height() > 200.0 =>
                {
                    Some((rect.rect, clip))
                }
                egui::Shape::Vec(shapes) => {
                    shapes.iter().find_map(|shape| paper_rect(shape, clip))
                }
                _ => None,
            }
        }

        let dir = std::env::temp_dir().join(format!("mijian-paper-{}", recent::random_id()));
        let opened = store::create(&dir, "paper-layout", "test-only").unwrap();
        std::fs::remove_file(&opened.path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        let mut app = App {
            view: View::Editor,
            entries: Vec::new(),
            new: NewState::default(),
            unlock: None,
            pending: None,
            editor: Some(EditorCtx::new(opened)),
            toast: None,
            new_dir: None,
            open_error: None,
        };
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let render = |app: &mut App, screen: egui::Vec2| {
            let mut paper = None;
            for _ in 0..3 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
                        ..Default::default()
                    },
                    |ui| app.editor_ui(ui),
                );
                paper = output
                    .shapes
                    .iter()
                    .find_map(|shape| paper_rect(&shape.shape, shape.clip_rect));
                output.textures_delta.clear();
            }
            paper.expect("编辑界面应绘制纸张")
        };
        let screen = vec2(1180.0, 780.0);
        let (short, viewport) = render(&mut app, screen);
        assert!((short.height() + 40.0 - viewport.height()).abs() < 1.0);
        assert_eq!(app.editor.as_ref().unwrap().sheets[0].scroll, 0.0);

        let ed = app.editor.as_mut().unwrap();
        let image = asset_from_color_image(&egui::ColorImage::filled(
            [320, 480],
            theme::FIELD,
        ))
        .unwrap();
        editor::insert_image_at(&mut ed.opened.doc.sheets[0].blocks, Position::new(0, 0), image.id.clone());
        ed.opened.doc.images.push(image);
        ed.opened.doc.sheets[0]
            .blocks
            .extend((0..30).map(|_| Block::with_text("Paper layout regression.")));
        let (long, _) = render(&mut app, screen);
        assert!(
            long.height() > viewport.height() + 480.0,
            "长内容应撑高纸张，纸张高度 {}，可视区高度 {}",
            long.height(),
            viewport.height()
        );

        app.editor.as_mut().unwrap().sheets[0].scroll = 240.0;
        let (scrolled, _) = render(&mut app, screen);
        assert!((long.top() - scrolled.top() - 240.0).abs() < 1.0);
        assert!((long.height() - scrolled.height()).abs() < 1.0);

        app.editor.as_mut().unwrap().sheets[0].scroll =
            long.height() + 40.0 - viewport.height();
        let (bottom, viewport) = render(&mut app, screen);
        assert!(
            (bottom.bottom() + 20.0 - viewport.bottom()).abs() < 1.0,
            "滚动到底后应保留纸张底边：纸张 {bottom:?}，可视区 {viewport:?}，滚动距离 {}",
            app.editor.as_ref().unwrap().sheets[0].scroll
        );

        let ed = app.editor.as_mut().unwrap();
        ed.opened.doc.sheets[0].blocks = vec![Block::empty()];
        let (short_again, _) = render(&mut app, screen);
        assert!((short_again.height() - short.height()).abs() < 1.0);
        assert_eq!(app.editor.as_ref().unwrap().sheets[0].scroll, 0.0);

        let (resized, viewport) = render(&mut app, vec2(900.0, 600.0));
        assert!((resized.height() + 40.0 - viewport.height()).abs() < 1.0);
        assert_eq!(app.editor.as_ref().unwrap().sheets[0].scroll, 0.0);
    }

    #[test]
    fn toast_resizes_between_messages_and_wraps_only_at_its_limit() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut app = App {
            view: View::Library,
            entries: Vec::new(),
            new: NewState::default(),
            unlock: None,
            pending: None,
            editor: None,
            toast: None,
            new_dir: None,
            open_error: None,
        };
        let mut show = |message: &str| {
            app.toast(message);
            for _ in 0..3 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            vec2(900.0, 600.0),
                        )),
                        ..Default::default()
                    },
                    |_| app.toast_ui(&ctx),
                );
                output.textures_delta.clear();
            }
            ctx.memory(|m| m.area_rect(Id::unique("toast")).unwrap())
        };

        let short = show("已保存");
        let filename = show("已解锁 test.ecnote");
        assert!(filename.width() > short.width() + 30.0);
        assert!((filename.height() - short.height()).abs() < 1.0);
        let long = show(&"长文件名".repeat(100));
        assert!(long.width() <= 560.0);
        assert!(long.height() > short.height());
        let short_again = show("已保存");
        assert!((short_again.width() - short.width()).abs() < 1.0);
        assert!((short_again.height() - short.height()).abs() < 1.0);
    }
}

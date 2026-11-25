//! 富文本编辑器：把 `egui_richedit` 的光标/选区接到 [`Doc`] 的 blocks 上，
//! 并负责每段的排版与绘制。

use std::collections::HashMap;

use egui::{
    Align, Color32, FontFamily, FontId, Sense, Stroke, TextFormat, Ui, text::LayoutJob, pos2, vec2,
};
use egui_richedit::{
    Edit, Laid, Mark, Model, ParagraphJob, Position, Property, RichEdit, Selection, Value,
};

use crate::model::{
    Align as DocAlign, Block, BlockKind, Run, format_range, replace_range, split_block_at,
};
use crate::ui::{theme, widgets};

/// 撤销/重做：整表快照，128 步上限。
pub struct UndoStack {
    pub undo: Vec<Vec<Block>>,
    pub redo: Vec<Vec<Block>>,
}

impl Default for UndoStack {
    fn default() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
}

impl UndoStack {
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    pub fn snapshot(&mut self, blocks: &[Block]) {
        self.redo.clear();
        self.undo.push(blocks.to_vec());
        if self.undo.len() > 128 {
            self.undo.remove(0);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// 返回要恢复的 blocks。
    pub fn undo_step(&mut self, current: &[Block]) -> Option<Vec<Block>> {
        let prev = self.undo.pop()?;
        self.redo.push(current.to_vec());
        Some(prev)
    }

    pub fn redo_step(&mut self, current: &[Block]) -> Option<Vec<Block>> {
        let next = self.redo.pop()?;
        self.undo.push(current.to_vec());
        Some(next)
    }
}

/// 光标处「下一次输入」的格式。
#[derive(Clone, Default)]
pub struct PendingFmt {
    pub marks: u8,
    pub color: Option<String>,
    pub bg: Option<String>,
    pub size: Option<f32>,
}

/// 一张工作表的编辑状态：RichEdit（光标/选区）+ 撤销栈 + 待输入格式。
pub struct SheetEditor {
    /// 与 RichEdit 共用的 Id，切表后用它把键盘焦点交还给编辑器
    pub id: egui::Id,
    pub editor: RichEdit<usize>,
    pub undo: UndoStack,
    pub pending: PendingFmt,
    pub scroll: f32,
    /// 撤销/重做后短暂锁住滚动位置（像素, 剩余帧数）
    pub scroll_pin: Option<(f32, u8)>,
    pub textures: HashMap<String, egui::TextureHandle>,
    /// 当前选中的图片资产 id（点击图片后显示工具条）
    pub selected_image: Option<String>,
    pub image_drag: Option<ImageDrag>,
    pub reveal_image: bool,
}

impl SheetEditor {
    pub fn new(id: egui::Id) -> Self {
        Self {
            id,
            editor: RichEdit::new(id),
            undo: UndoStack::default(),
            pending: PendingFmt::default(),
            scroll: 0.0,
            scroll_pin: None,
            textures: HashMap::new(),
            selected_image: None,
            image_drag: None,
            reveal_image: false,
        }
    }

    pub fn select_image(&mut self, ctx: &egui::Context, id: String, reveal: bool) {
        self.editor.clear();
        self.selected_image = Some(id);
        self.image_drag = None;
        self.reveal_image = reveal;
        ctx.memory_mut(|m| m.request_focus(self.id.with("image")));
    }

    pub fn focus_text(&mut self, at: Position<usize>) {
        self.selected_image = None;
        self.image_drag = None;
        self.reveal_image = false;
        self.editor.select(Selection::caret(at));
    }

    pub fn focus_position(&self, blocks: &[Block]) -> Option<Position<usize>> {
        self.editor.selection().map(|s| s.focus.clone())
            .or_else(|| self.selected_image.as_deref()
                .and_then(|id| image_index(blocks, id))
                .map(|p| Position::new(p, 0)))
    }

    pub fn insert_image(
        &mut self,
        ctx: &egui::Context,
        blocks: &mut Vec<Block>,
        id: String,
    ) {
        let at = self.selected_image.as_deref()
            .and_then(|id| image_index(blocks, id))
            .map(|p| Position::new(p, 1))
            .or_else(|| self.editor.selection().map(|s| s.focus.clone()))
            .unwrap_or_else(|| Position::new(0, 0));
        self.undo.snapshot(blocks);
        insert_image_at(blocks, at, id.clone());
        self.editor.document_replaced();
        self.select_image(ctx, id, true);
    }

    /// 图片使用独立键盘焦点；正文仍由 RichEdit 接收输入和维护布局。
    pub fn image_input(
        &mut self,
        ui: &mut Ui,
        blocks: &mut Vec<Block>,
        dirty: &mut bool,
    ) -> Option<ImageAction> {
        if self.selected_image.is_none() && self.editor.has_focus(ui.ctx())
            && let Some(sel) = self.editor.selection()
            && sel.is_caret()
        {
            let p = sel.focus.paragraph;
            let at = sel.focus.offset;
            let adjacent = if at == 0 && p > 0 && blocks[p - 1].kind == BlockKind::Image
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace))
            {
                Some(p - 1)
            } else if at == blocks[p].char_len() && p + 1 < blocks.len()
                && blocks[p + 1].kind == BlockKind::Image
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete))
            {
                Some(p + 1)
            } else { None };
            if let Some(index) = adjacent {
                self.select_image(ui.ctx(), blocks[index].runs[0].text.clone(), true);
                return None;
            }
        }
        let id = self.selected_image.clone()?;
        let focus = self.id.with("image");
        ui.interact(ui.clip_rect(), focus, Sense::focusable_noninteractive());
        if !ui.memory(|m| m.has_focus(focus)) {
            return None;
        }
        ui.memory_mut(|m| m.set_focus_lock_filter(focus, egui::EventFilter {
            horizontal_arrows: true,
            vertical_arrows: true,
            escape: true,
            ..Default::default()
        }));
        let index = image_index(blocks, &id)?;
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::A)) {
            self.selected_image = None;
            self.image_drag = None;
            let last = blocks.len() - 1;
            self.editor.select(Selection {
                anchor: Position::new(0, 0),
                focus: Position::new(last, if blocks[last].kind == BlockKind::Image {
                    1
                } else {
                    blocks[last].char_len()
                }),
            });
            return None;
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            if self.image_drag.take().is_none() {
                self.selected_image = None;
                ui.memory_mut(|m| m.surrender_focus(focus));
            }
            return None;
        }
        if self.image_drag.is_some() {
            return None;
        }
        if ui.input_mut(|i| {
            i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)
        }) {
            return Some(ImageAction::Delete(id));
        }
        for (key, before) in [(egui::Key::ArrowUp, true), (egui::Key::ArrowDown, false)] {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key)) {
                let target = if before {
                    (0..index).rev().find(|&p| is_text_block(&blocks[p]))
                } else {
                    (index + 1..blocks.len()).find(|&p| is_text_block(&blocks[p]))
                };
                let target = target.unwrap_or_else(|| {
                    self.undo.snapshot(blocks);
                    let p = if before { index } else { index + 1 };
                    blocks.insert(p, Block::empty());
                    *dirty = true;
                    p
                });
                self.focus_text(Position::new(target, if before { blocks[target].char_len() } else { 0 }));
                return None;
            }
        }
        let enter = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        let typing = ui.input(|i| i.events.iter().any(|event| match event {
            egui::Event::Text(text) | egui::Event::Paste(text)
                | egui::Event::Ime(egui::ImeEvent::Commit(text))
                | egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => !text.is_empty(),
            _ => false,
        }));
        if enter || typing {
            let next = index + 1;
            if !blocks.get(next).is_some_and(|b| b.kind == BlockKind::Body && b.is_empty()) {
                self.undo.snapshot(blocks);
                blocks.insert(next, Block::empty());
                *dirty = true;
            }
            self.focus_text(Position::new(next, 0));
        }
        None
    }
}

pub enum ImageDrag {
    Resize {
        id: String,
        start_rect: egui::Rect,
        start_factor: f32,
        factor: f32,
        corner: egui::Vec2,
        pointer: egui::Pos2,
    },
    Move { id: String },
}

pub fn image_index(blocks: &[Block], id: &str) -> Option<usize> {
    blocks.iter().position(|b| b.kind == BlockKind::Image && b.runs[0].text == id)
}

fn is_text_block(block: &Block) -> bool {
    !matches!(block.kind, BlockKind::Image | BlockKind::Divider)
}

/// 把一张工作表的 blocks / 撤销栈 / 格式状态借给 `egui_richedit`。
pub struct ActiveSheet<'a> {
    pub blocks: &'a mut Vec<Block>,
    pub undo: &'a mut UndoStack,
    pub pending: &'a mut PendingFmt,
    pub dirty: &'a mut bool,
}

fn mark_bit(mark: Mark) -> u8 {
    match mark {
        Mark::Bold => Run::BOLD,
        Mark::Italic => Run::ITALIC,
        Mark::Underline => Run::UNDERLINE,
        Mark::Strike => Run::STRIKE,
    }
}

/// 遍历 [from, to) 覆盖到的每个 run 片段。
fn for_each_run(
    blocks: &[Block],
    from: (usize, usize),
    to: (usize, usize),
    mut f: impl FnMut(&Block, &Run),
) {
    if from > to {
        return;
    }
    for p in from.0..=to.0 {
        let Some(block) = blocks.get(p) else { continue };
        if block.kind == BlockKind::Image {
            continue;
        }
        let len = block.char_len();
        let start = if p == from.0 { from.1.min(len) } else { 0 };
        let end = if p == to.0 { to.1.min(len) } else { len };
        if start >= end {
            continue;
        }
        let mut pos = 0;
        for run in &block.runs {
            let run_len = run.char_len();
            let (a, z) = (pos.max(start), (pos + run_len).min(end));
            if a < z {
                f(block, run);
            }
            pos += run_len;
        }
    }
}

fn uniform_mark(
    blocks: &[Block],
    from: (usize, usize),
    to: (usize, usize),
    bit: u8,
) -> Option<bool> {
    let mut state: Option<bool> = None;
    let mut conflict = false;
    for_each_run(blocks, from, to, |_, run| {
        let v = run.marks & bit != 0;
        match state {
            None => state = Some(v),
            Some(prev) if prev != v => conflict = true,
            _ => {}
        }
    });
    if conflict { None } else { state }
}

fn uniform_size(blocks: &[Block], from: (usize, usize), to: (usize, usize)) -> Option<f32> {
    let mut state: Option<f32> = None;
    let mut conflict = false;
    for_each_run(blocks, from, to, |block, run| {
        let size = run.size.unwrap_or_else(|| size_of(block.kind));
        match state {
            None => state = Some(size),
            Some(prev) if (prev - size).abs() > f32::EPSILON => conflict = true,
            _ => {}
        }
    });
    if conflict { None } else { state }
}

impl Model for ActiveSheet<'_> {
    type Paragraph = usize;

    fn text(&self, paragraph: &usize) -> Option<String> {
        self.blocks.get(*paragraph).map(|b| {
            if b.kind == BlockKind::Image { "\u{fffc}".to_owned() } else { b.text() }
        })
    }

    fn next(&self, paragraph: &usize) -> Option<usize> {
        (*paragraph + 1 < self.blocks.len()).then_some(paragraph + 1)
    }

    fn previous(&self, paragraph: &usize) -> Option<usize> {
        paragraph.checked_sub(1)
    }

    fn first(&self) -> Option<usize> {
        (!self.blocks.is_empty()).then_some(0)
    }

    fn last(&self) -> Option<usize> {
        self.blocks.len().checked_sub(1)
    }

    fn marked(
        &self,
        from: &Position<usize>,
        to: &Position<usize>,
        mark: Mark,
    ) -> Option<bool> {
        let bit = mark_bit(mark);
        if from == to {
            return Some(self.pending.marks & bit != 0);
        }
        uniform_mark(
            self.blocks,
            (from.paragraph, from.offset),
            (to.paragraph, to.offset),
            bit,
        )
        .or(Some(self.pending.marks & bit != 0))
    }

    fn value(&self, from: &Position<usize>, to: &Position<usize>, property: Property) -> Option<Value> {
        match property {
            Property::Size => {
                if from == to {
                    return self
                        .pending
                        .size
                        .map(|s| Value::Size((s * 20.0).round() as u32));
                }
                uniform_size(self.blocks, (from.paragraph, from.offset), (to.paragraph, to.offset))
                    .map(|s| Value::Size((s * 20.0).round() as u32))
            }
            Property::Family => Some(Value::Family("Proportional".to_owned())),
        }
    }

    fn apply(&mut self, edit: Edit<'_, usize>, new_step: bool) -> Option<Position<usize>> {
        match edit {
            Edit::Replace { from, to, text } => {
                if self.blocks.get(from.paragraph).is_none() || self.blocks.get(to.paragraph).is_none() {
                    return None;
                }
                if new_step {
                    self.undo.snapshot(self.blocks);
                }
                let at = replace_objects(
                    self.blocks,
                    (from.paragraph, from.offset),
                    (to.paragraph, to.offset),
                    text,
                );
                // 在分割线那一行打字时，把它变回普通段落，否则输入看不见
                if !text.is_empty()
                    && let Some(b) = self.blocks.get_mut(at.paragraph)
                    && b.kind == BlockKind::Divider
                {
                    b.kind = BlockKind::Body;
                }
                *self.dirty = true;
                Some(at)
            }
            Edit::Split { at } => {
                if self.blocks.get(at.paragraph).is_none() {
                    return None;
                }
                if new_step {
                    self.undo.snapshot(self.blocks);
                }
                let block = &mut self.blocks[at.paragraph];
                if block.kind == BlockKind::Image {
                    let next = at.paragraph + usize::from(at.offset > 0);
                    self.blocks.insert(next, Block::empty());
                    *self.dirty = true;
                    return Some(Position::new(next, 0));
                }
                if matches!(block.kind, BlockKind::Bullet | BlockKind::Number) && block.is_empty() {
                    block.kind = BlockKind::Body;
                    *self.dirty = true;
                    return Some(Position::new(at.paragraph, 0));
                }
                let next = split_block_at(self.blocks, (at.paragraph, at.offset))?;
                // 分割线后回车：新起的是正文；项目符号/编号则延续列表
                if let Some(b) = self.blocks.get_mut(next.0)
                    && b.kind == BlockKind::Divider
                {
                    b.kind = BlockKind::Body;
                }
                *self.dirty = true;
                Some(Position::new(next.0, next.1))
            }
            Edit::Format {
                from,
                to,
                mark,
                on,
            } => {
                let bit = mark_bit(mark);
                if from == to {
                    if on {
                        self.pending.marks |= bit;
                    } else {
                        self.pending.marks &= !bit;
                    }
                    return Some(from);
                }
                if new_step {
                    self.undo.snapshot(self.blocks);
                }
                format_range(
                    self.blocks,
                    (from.paragraph, from.offset),
                    (to.paragraph, to.offset),
                    &mut |r| {
                        if on {
                            r.marks |= bit;
                        } else {
                            r.marks &= !bit;
                        }
                    },
                );
                *self.dirty = true;
                Some(to)
            }
            Edit::Set { from, to, value } => {
                if from == to {
                    match &value {
                        Value::Size(v) => self.pending.size = Some(*v as f32 / 20.0),
                        Value::Family(f) => {
                            self.pending.marks &= !0;
                            let _ = f;
                        }
                    }
                    return Some(from);
                }
                if new_step {
                    self.undo.snapshot(self.blocks);
                }
                match value {
                    Value::Size(v) => {
                        let pt = v as f32 / 20.0;
                        format_range(
                            self.blocks,
                            (from.paragraph, from.offset),
                            (to.paragraph, to.offset),
                            &mut |r| r.size = Some(pt),
                        );
                    }
                    Value::Family(f) => {
                        let mono = f.eq_ignore_ascii_case("monospace");
                        format_range(
                            self.blocks,
                            (from.paragraph, from.offset),
                            (to.paragraph, to.offset),
                            &mut |r| r.mono = mono,
                        );
                    }
                }
                *self.dirty = true;
                Some(to)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 排版与绘制
// ---------------------------------------------------------------------------

pub fn parse_color(s: &str) -> Option<Color32> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    Some(Color32::from_rgb(r, g, b))
}

pub fn size_of(kind: BlockKind) -> f32 {
    match kind {
        BlockKind::H1 => 30.0,
        BlockKind::H2 => 24.0,
        BlockKind::H3 => 19.0,
        BlockKind::Code => 13.5,
        BlockKind::Quote => 15.5,
        _ => 15.5,
    }
}

fn family_for(run: &Run, kind: BlockKind) -> FontFamily {
    if run.mono || kind == BlockKind::Code {
        return FontFamily::Monospace;
    }
    if run.marks & Run::BOLD != 0 || matches!(kind, BlockKind::H1 | BlockKind::H2 | BlockKind::H3) {
        return FontFamily::Name("bold".into());
    }
    FontFamily::Proportional
}

fn format_for(run: &Run, kind: BlockKind, ink: Color32) -> TextFormat {
    let size = run.size.unwrap_or_else(|| size_of(kind));
    let color = run
        .color
        .as_deref()
        .and_then(parse_color)
        .unwrap_or(ink);
    let bg = run.bg.as_deref().and_then(parse_color);
    TextFormat {
        font_id: FontId::new(size, family_for(run, kind)),
        line_height: Some((size * 1.62).round()),
        color,
        background: bg.unwrap_or(Color32::TRANSPARENT),
        italics: run.marks & Run::ITALIC != 0,
        underline: if run.marks & Run::UNDERLINE != 0 {
            Stroke::new(1.2, color)
        } else {
            Stroke::NONE
        },
        strikethrough: if run.marks & Run::STRIKE != 0 {
            Stroke::new(1.2, color)
        } else {
            Stroke::NONE
        },
        ..Default::default()
    }
}

pub struct ParagraphMetrics {
    pub indent: f32,
    pub prefix: Option<String>,
}

fn prefix_for(kind: BlockKind, number: usize, number_indent: f32) -> ParagraphMetrics {
    match kind {
        BlockKind::Bullet => ParagraphMetrics {
            indent: 24.0,
            prefix: Some("•".to_owned()),
        },
        BlockKind::Number => ParagraphMetrics {
            indent: number_indent,
            prefix: Some(format!("{number}.")),
        },
        BlockKind::Quote => ParagraphMetrics {
            indent: 14.0,
            prefix: None,
        },
        _ => ParagraphMetrics {
            indent: 0.0,
            prefix: None,
        },
    }
}

fn halign(align: Option<DocAlign>) -> Align {
    match align.unwrap_or_default() {
        DocAlign::Left => Align::LEFT,
        DocAlign::Center => Align::Center,
        DocAlign::Right => Align::RIGHT,
        DocAlign::Justify => Align::LEFT,
    }
}

/// 按文档顺序排版并绘制所有段落。必须每帧对**每一段**调用
/// `editor.paragraph`，即使它在屏幕外。
fn texture_for(
    ctx: &egui::Context,
    cache: &mut HashMap<String, egui::TextureHandle>,
    a: &crate::model::ImageAsset,
) -> Option<egui::TextureHandle> {
    if let Some(t) = cache.get(&a.id) {
        return Some(t.clone());
    }
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(a.data.as_bytes())
        .ok()?;
    let rgba = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    let tex = ctx.load_texture(&a.id, color, egui::TextureOptions::LINEAR);
    cache.insert(a.id.clone(), tex.clone());
    Some(tex)
}

fn replace_objects(
    blocks: &mut Vec<Block>,
    from: (usize, usize),
    to: (usize, usize),
    text: &str,
) -> Position<usize> {
    let ((mut fp, mut fo), (mut tp, mut end)) = if from <= to { (from, to) } else { (to, from) };
    // 图片在编辑模型中占一个字符；其资产 id 不参与文本拆分。
    if fp == tp && fo == end && blocks[fp].kind == BlockKind::Image {
        fp += usize::from(fo > 0);
        blocks.insert(fp, Block::empty());
        tp = fp;
        fo = 0;
        end = 0;
    } else {
        if blocks[tp].kind == BlockKind::Image {
            if end == 0 {
                blocks.insert(tp, Block::empty());
            } else {
                blocks[tp] = Block::empty();
                if fp == tp {
                    fo = 0;
                }
            }
            end = 0;
        }
        if blocks[fp].kind == BlockKind::Image {
            if fo > 0 {
                fp += 1;
                blocks.insert(fp, Block::empty());
                tp += 1;
            } else {
                blocks[fp] = Block::empty();
            }
            fo = 0;
        }
    }
    replace_range(blocks, (fp, fo), (tp, end), text);
    let lines: Vec<_> = text.split('\n').collect();
    Position::new(fp + lines.len() - 1, if lines.len() == 1 {
        fo + text.chars().count()
    } else {
        lines.last().unwrap().chars().count()
    })
}

pub fn insert_image_at(blocks: &mut Vec<Block>, at: Position<usize>, id: String) -> usize {
    let p = at.paragraph;
    let index = if blocks[p].kind == BlockKind::Image {
        p + 1
    } else if blocks[p].kind == BlockKind::Divider {
        p + 1
    } else if blocks[p].is_empty() {
        blocks.remove(p);
        p
    } else if at.offset == 0 {
        p
    } else if at.offset >= blocks[p].char_len() {
        p + 1
    } else {
        split_block_at(blocks, (p, at.offset));
        p + 1
    };
    let mut block = Block::empty();
    block.kind = BlockKind::Image;
    block.runs = vec![Run::plain(id)];
    blocks.insert(index, block);
    if index + 1 == blocks.len() {
        blocks.push(Block::empty());
    }
    index
}

/// 在 `at` 处放一条分割线，返回其下方空段落的下标（光标落点）。
///
/// 当前段落是空白的就用它当作分割线（不额外多出空行）；否则在它下面插入。
pub fn insert_divider_after(blocks: &mut Vec<Block>, at: usize) -> usize {
    let at = at.min(blocks.len().saturating_sub(1));
    let blank = blocks
        .get(at)
        .is_some_and(|b| b.char_len() == 0 && b.kind != BlockKind::Image);
    if blank {
        blocks[at].kind = BlockKind::Divider;
        blocks[at].align = None;
        blocks.insert(at + 1, Block::empty());
        return at + 1;
    }
    let mut block = Block::empty();
    block.kind = BlockKind::Divider;
    let index = at + 1;
    blocks.insert(index, block);
    // 分割线本身不承载文字，后面补一个空正文段，便于继续输入
    blocks.insert(index + 1, Block::empty());
    index + 1
}

fn img_btn(ui: &mut Ui, label: &str, selected: bool, danger: bool) -> egui::Response {
    let color = if danger { theme::DANGER } else if selected { theme::ACCENT } else { theme::TEXT };
    ui.add(
        egui::Button::new(egui::RichText::new(label).size(11.0).color(color))
            .min_size(egui::vec2(24.0, 20.0))
            .selected(selected)
            .frame_when_inactive(selected),
    )
}

fn image_toolbar(
    ui: &mut Ui,
    img_rect: egui::Rect,
    id: &str,
    factor: f32,
    align: DocAlign,
    action: &mut Option<ImageAction>,
) -> egui::Rect {
    let area_id = ui.scope_id().with(("img_tb", id));
    let view = ui.clip_rect().intersect(ui.ctx().content_rect()).shrink(6.0);
    let size = egui::AreaState::load(ui.ctx(), area_id)
        .and_then(|s| s.size)
        .unwrap_or(vec2(440.0, 34.0));
    let y = if img_rect.top() - size.y - 8.0 >= view.top() {
        img_rect.top() - size.y - 8.0
    } else if img_rect.bottom() + size.y + 8.0 <= view.bottom() {
        img_rect.bottom() + 8.0
    } else {
        view.top()
    };
    egui::Area::new(area_id)
        .order(egui::Order::Foreground)
        .movable(false)
        .constrain_to(view)
        .fixed_pos(pos2(
            (img_rect.center().x - size.x * 0.5)
                .clamp(view.left(), (view.right() - size.x).max(view.left())),
            y,
        ))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::symmetric(6, 3))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        ui.spacing_mut().button_padding = vec2(5.0, 4.0);
                        ui.label(egui::RichText::new(format!("{:.0}%", factor * 100.0))
                            .size(11.0).color(theme::ACCENT));
                        ui.separator();
                        for (label, f) in [
                            ("25%", 0.25f32),
                            ("50%", 0.5),
                            ("75%", 0.75),
                            ("100%", 1.0),
                        ] {
                            if widgets::tooltip(
                                img_btn(ui, label, (factor - f).abs() < 0.005, false),
                                &format!("图片宽度设为正文的 {label}"),
                                None,
                            ).clicked()
                            {
                                *action = Some(ImageAction::Width(id.to_owned(), f));
                            }
                        }
                        ui.add_space(4.0);
                        for (label, al, hint) in [
                            ("左", DocAlign::Left, "图片左对齐"),
                            ("中", DocAlign::Center, "图片居中对齐"),
                            ("右", DocAlign::Right, "图片右对齐"),
                        ] {
                            if widgets::tooltip(img_btn(ui, label, align == al, false), hint, None).clicked() {
                                *action = Some(ImageAction::Align(id.to_owned(), al));
                            }
                        }
                        ui.add_space(4.0);
                        if widgets::tooltip(img_btn(ui, "上移", false, false), "向上移动一个段落", None).clicked() {
                            *action = Some(ImageAction::Move(id.to_owned(), -1));
                        }
                        if widgets::tooltip(img_btn(ui, "下移", false, false), "向下移动一个段落", None).clicked() {
                            *action = Some(ImageAction::Move(id.to_owned(), 1));
                        }
                        ui.add_space(4.0);
                        if widgets::tooltip(img_btn(ui, "删除", false, true), "删除图片", Some("Delete")).clicked() {
                            *action = Some(ImageAction::Delete(id.to_owned()));
                        }
                    });
                });
        }).response.rect
}

/// 图片的交互动作，由调用方落盘到文档模型。
pub enum ImageAction {
    Delete(String),
    Width(String, f32),
    Align(String, DocAlign),
    Move(String, i32),
    MoveTo(String, usize),
}

pub fn draw_document(
    ui: &mut Ui,
    blocks: &[Block],
    rt: &mut SheetEditor,
    width: f32,
    ink: Color32,
    images: &[crate::model::ImageAsset],
    action: &mut Option<ImageAction>,
) {
    let align_default = DocAlign::Justify;
    let mut paragraph_rects = Vec::with_capacity(blocks.len());
    let mut image_hit = false;
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let mut number = 0;
    let mut number_indent = 26.0;
    for (index, block) in blocks.iter().enumerate() {
        if block.kind == BlockKind::Number {
            if number == 0 {
                let count = blocks[index..].iter()
                    .take_while(|block| block.kind == BlockKind::Number).count();
                let font = FontId::proportional(size_of(BlockKind::Number));
                // 同一段列表统一缩进，并按实际字体宽度容纳两位及更长编号。
                let widest = ui.fonts_mut(|fonts| (1..=count)
                    .map(|number| fonts.layout_no_wrap(
                        format!("{number}."), font.clone(), ink,
                    ).size().x)
                    .fold(0.0_f32, f32::max));
                number_indent = (widest + 12.0).max(26.0);
            }
            number += 1;
        } else {
            number = 0;
        }
        if block.kind == BlockKind::Image {
            let id = block.runs.first().map(|r| r.text.clone()).unwrap_or_default();
            let asset = images.iter().find(|a| a.id == id);
            let max_w = (width - 8.0).max(1.0);
            let aspect = asset.map_or(0.66, |a| a.h.max(1) as f32 / a.w.max(1) as f32);
            let initial = asset.map_or(1.0, |a| a.w as f32 / max_w);
            let mut factor = block.image_width.unwrap_or(initial).clamp(0.15, 1.0);
            let shown = vec2(max_w * factor, max_w * factor * aspect);
            let (rect, layout_response) =
                ui.allocate_exact_size(vec2(width, shown.y + 8.0), Sense::hover());
            paragraph_rects.push(rect);
            let align_img = block.align.unwrap_or(DocAlign::Center);
            let mut img_rect = match align_img {
                DocAlign::Left => egui::Rect::from_min_size(
                    rect.left_top() + vec2(4.0, 4.0),
                    shown,
                ),
                DocAlign::Right => egui::Rect::from_min_size(
                    egui::pos2(rect.right() - shown.x - 4.0, rect.top() + 4.0),
                    shown,
                ),
                _ => egui::Rect::from_center_size(rect.center(), shown),
            };
            let resizing = if let Some(ImageDrag::Resize {
                id: drag_id, start_rect, factor: preview, corner, pointer: start, ..
            }) = &mut rt.image_drag
                && drag_id == &id
                && let Some(pointer) = pointer
            {
                // 拖动时固定对角预览，正文保持原占位；松手后按原对齐重新排版。
                let scroll_delta = img_rect.min - start_rect.min;
                let delta = pointer - *start - scroll_delta;
                let growth = (delta.x * corner.x + delta.y * corner.y * aspect)
                    / (1.0 + aspect * aspect);
                *preview = ((start_rect.width() + growth) / max_w).clamp(0.15, 1.0);
                factor = *preview;
                let anchor = start_rect.center() - start_rect.size() * *corner * 0.5 + scroll_delta;
                let dragged = anchor + vec2(max_w * factor, max_w * factor * aspect) * *corner;
                img_rect = egui::Rect::from_two_pos(anchor, dragged);
                true
            } else {
                false
            };
            let response = ui.interact(img_rect, rt.id.with(("image", &id)), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            image_hit |= response.contains_pointer();
            if response.clicked() {
                rt.select_image(ui.ctx(), id.clone(), false);
            }
            let is_selected = rt.selected_image.as_deref() == Some(id.as_str());
            let painter = if resizing {
                ui.painter().clone().with_layer_id(egui::LayerId::new(
                    egui::Order::Foreground, rt.id.with(("image_resize", &id)),
                ))
            } else {
                ui.painter().clone()
            };
            if is_selected || response.hovered() {
                painter.rect_stroke(
                    img_rect.expand(2.0),
                    9.0,
                    Stroke::new(if is_selected { 2.0 } else { 1.0 }, theme::ACCENT),
                    egui::StrokeKind::Outside,
                );
            }
            if let Some(tex) = asset.and_then(|a| texture_for(ui.ctx(), &mut rt.textures, a)) {
                painter.image(
                    tex.id(),
                    img_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            } else {
                painter.rect_filled(img_rect, 8.0, theme::FIELD);
                painter.text(
                    img_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "图片",
                    FontId::proportional(13.0),
                    theme::DIM,
                );
            }
            painter.rect_stroke(
                img_rect,
                8.0,
                Stroke::new(1.0, theme::BORDER),
                egui::StrokeKind::Inside,
            );
            if is_selected {
                if rt.reveal_image {
                    ui.scroll_to_rect(
                        egui::Rect::from_min_size(img_rect.min, vec2(shown.x, shown.y.min(100.0))),
                        None,
                    );
                    rt.reveal_image = false;
                }
                for (corner_index, (point, corner)) in [
                    (img_rect.left_top(), vec2(-1.0, -1.0)),
                    (img_rect.right_top(), vec2(1.0, -1.0)),
                    (img_rect.left_bottom(), vec2(-1.0, 1.0)),
                    (img_rect.right_bottom(), vec2(1.0, 1.0)),
                ].into_iter().enumerate() {
                    let handle = ui.interact(
                        egui::Rect::from_center_size(point, vec2(16.0, 16.0)),
                        rt.id.with(("resize", &id, corner_index)),
                        Sense::drag(),
                    ).on_hover_cursor(if corner.x == corner.y {
                        egui::CursorIcon::ResizeNwSe
                    } else {
                        egui::CursorIcon::ResizeNeSw
                    });
                    let handle = widgets::tooltip(handle, "拖动等比缩放图片", None);
                    image_hit |= handle.contains_pointer();
                    painter.rect(
                        egui::Rect::from_center_size(point, vec2(7.0, 7.0)),
                        2.0, theme::CARD, Stroke::new(1.3, theme::ACCENT),
                        egui::StrokeKind::Outside,
                    );
                    if handle.drag_started() {
                        let pointer = ui.input(|i| i.pointer.press_origin()).unwrap();
                        rt.image_drag = Some(ImageDrag::Resize {
                            id: id.clone(), start_rect: img_rect, start_factor: factor,
                            factor, corner, pointer,
                        });
                        ui.memory_mut(|m| m.request_focus(rt.id.with("image")));
                    }
                }
                let grip_rect = egui::Rect::from_center_size(
                    pos2(img_rect.left() - 19.0, img_rect.top() + 15.0),
                    vec2(20.0, 28.0),
                );
                let grip = ui.interact(grip_rect, rt.id.with(("move", &id)), Sense::drag())
                    .on_hover_cursor(egui::CursorIcon::Grab);
                let grip = widgets::tooltip(grip, "拖动调整图片位置", None);
                image_hit |= grip.contains_pointer();
                painter.rect_filled(grip_rect, 5.0, theme::FIELD);
                for x in [-3.0, 3.0] {
                    for y in [-6.0, 0.0, 6.0] {
                        painter.circle_filled(grip_rect.center() + vec2(x, y), 1.2, theme::DIM);
                    }
                }
                if grip.drag_started() {
                    rt.image_drag = Some(ImageDrag::Move { id: id.clone() });
                    ui.memory_mut(|m| m.request_focus(rt.id.with("image")));
                }
                if rt.image_drag.is_none() && img_rect.intersects(ui.clip_rect()) {
                    let toolbar = image_toolbar(ui, img_rect, &id, factor, align_img, action);
                    image_hit |= pointer.is_some_and(|p| toolbar.contains(p));
                }
                if ui.memory(|m| m.has_focus(rt.id.with("image"))) {
                    ui.output_mut(|o| o.ime = Some(egui::output::IMEOutput {
                        purpose: egui::IMEPurpose::Normal,
                        rect: img_rect,
                        cursor_rect: egui::Rect::from_min_size(img_rect.left_bottom(), vec2(1.0, 20.0)),
                        should_interrupt_composition: false,
                    }));
                }
            }
            // 保留原子段落的布局；图片点击不再交给文本选区处理。
            let mut job = ParagraphJob::new(LayoutJob::default());
            job.atom("\u{fffc}", 1, TextFormat {
                font_id: FontId::proportional(8.0),
                color: Color32::TRANSPARENT,
                ..Default::default()
            });
            let (layout, map) = job.into_parts();
            let galley = ui.fonts_mut(|f| f.layout_job(layout));
            let laid = Laid {
                galley,
                map,
                origin: rect.left_top(),
            };
            rt.editor.paragraph_moved(ui, &layout_response, &index, laid, None);
            ui.add_space(8.0);
            continue;
        }
        if block.kind == BlockKind::Divider {
            // 分割线：一条 1px 细线，上下留白；仍然要交给 RichEdit 一个可落脚
            // 的段落，否则上下方向键会在它上面「跳过」。
            let height = 22.0;
            let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
            paragraph_rects.push(rect);
            let y = rect.center().y;
            let painter = ui.painter();
            painter.line_segment(
                [
                    pos2(rect.left() + 2.0, y),
                    pos2(rect.right() - 2.0 - 12.0, y),
                ],
                Stroke::new(1.0, ink.gamma_multiply(0.28)),
            );
            let mut job = ParagraphJob::new(LayoutJob::default());
            job.atom("\u{200b}", 0, TextFormat {
                font_id: FontId::proportional(8.0),
                color: Color32::TRANSPARENT,
                ..Default::default()
            });
            let (layout, map) = job.into_parts();
            let galley = ui.fonts_mut(|f| f.layout_job(layout));
            let laid = Laid {
                galley,
                map,
                origin: rect.left_top(),
            };
            rt.editor.paragraph(ui, &response, &index, laid);
            ui.add_space(6.0);
            continue;
        }
        let metrics = prefix_for(block.kind, number, number_indent);
        let text_width = (width - metrics.indent - 12.0).max(80.0);
        let job_len = text_width;
        let mut job = ParagraphJob::new(LayoutJob {
            wrap: egui::text::TextWrapping {
                max_width: job_len,
                ..Default::default()
            },
            halign: halign(block.align.or(Some(align_default))),
            justify: matches!(block.align.unwrap_or_default(), DocAlign::Justify),
            break_on_newline: true,
            ..LayoutJob::default()
        });

        let base_run = block
            .runs
            .first()
            .cloned()
            .unwrap_or_else(|| Run::plain(""));
        for run in &block.runs {
            if run.text.is_empty() {
                continue;
            }
            job.text(&run.text, format_for(run, block.kind, ink));
        }
        if job.is_empty() {
            job.atom(" ", 0, format_for(&base_run, block.kind, ink));
        }

        let (job, map) = job.into_parts();
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        let pad_top = if block.kind == BlockKind::Code { 8.0 } else { 2.0 };
        let pad_bottom = if block.kind == BlockKind::Code { 8.0 } else { 2.0 };
        let height = galley.size().y + pad_top + pad_bottom;
        let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
        paragraph_rects.push(rect);
        if response.is_pointer_button_down_on() {
            rt.selected_image = None;
            rt.image_drag = None;
        }

        let painter = ui.painter();
        if block.kind == BlockKind::Code {
            painter.rect_filled(rect, 8.0, theme::FIELD);
        }
        if block.kind == BlockKind::Quote {
            let bar = egui::Rect::from_min_size(
                rect.left_top() + vec2(2.0, 0.0),
                vec2(3.0, rect.height()),
            );
            painter.rect_filled(bar, 2.0, theme::ACCENT.gamma_multiply(0.45));
        }
        if let Some(prefix) = &metrics.prefix {
            let (x, anchor) = if block.kind == BlockKind::Number {
                (metrics.indent - 8.0, egui::Align2::RIGHT_TOP)
            } else {
                (metrics.indent - 14.0, egui::Align2::LEFT_TOP)
            };
            painter.text(
                rect.left_top() + vec2(x, pad_top + 2.0),
                anchor,
                prefix,
                FontId::new(size_of(block.kind), FontFamily::Proportional),
                ink.gamma_multiply(0.65),
            );
        }

        let origin = rect.left_top() + vec2(metrics.indent, pad_top);
        let laid = Laid {
            galley,
            map,
            origin,
        };
        rt.editor.paragraph(ui, &response, &index, laid);

        let gap = match block.kind {
            BlockKind::H1 => 14.0,
            BlockKind::H2 => 10.0,
            BlockKind::H3 => 8.0,
            _ => 6.0,
        };
        ui.add_space(gap);
    }
    if let Some(ImageDrag::Move { id }) = &rt.image_drag
        && let Some(pointer) = pointer
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let gap = paragraph_rects.iter().position(|r| pointer.y < r.center().y)
            .unwrap_or(paragraph_rects.len());
        let first = paragraph_rects.first().unwrap();
        let last = paragraph_rects.last().unwrap();
        let y = if gap == 0 { first.top() } else if gap == paragraph_rects.len() {
            last.bottom() + 4.0
        } else {
            (paragraph_rects[gap - 1].bottom() + paragraph_rects[gap].top()) * 0.5
        };
        ui.painter().line_segment(
            [pos2(first.left(), y), pos2(first.right(), y)],
            Stroke::new(2.0, theme::ACCENT),
        );
        let view = ui.clip_rect();
        let speed = if pointer.y < view.top() + 32.0 {
            ((view.top() + 32.0 - pointer.y) * 12.0).min(600.0)
        } else if pointer.y > view.bottom() - 32.0 {
            -((pointer.y - view.bottom() + 32.0) * 12.0).min(600.0)
        } else {
            0.0
        };
        if speed != 0.0 {
            ui.scroll_with_delta_animation(
                vec2(0.0, speed * ui.input(|i| i.stable_dt.min(0.05))),
                egui::style::ScrollAnimation::none(),
            );
            ui.ctx().request_repaint();
        }
        if ui.input(|i| i.pointer.button_released(egui::PointerButton::Primary)) {
            *action = Some(ImageAction::MoveTo(id.clone(), gap));
        }
    }
    if ui.input(|i| i.pointer.button_released(egui::PointerButton::Primary))
        && let Some(ImageDrag::Resize { id, factor, start_factor, .. }) = &rt.image_drag
        && (factor - start_factor).abs() > 0.0001
    {
        *action = Some(ImageAction::Width(id.clone(), *factor));
    }
    if ui.input(|i| i.pointer.button_released(egui::PointerButton::Primary)) {
        rt.image_drag = None;
    }
    if ui.input(|i| i.pointer.any_pressed())
        && ui.rect_contains_pointer(ui.clip_rect())
        && !image_hit && rt.image_drag.is_none()
    {
        rt.selected_image = None;
        ui.memory_mut(|m| m.surrender_focus(rt.id.with("image")));
    }
}

/// 一键清除格式：清掉当前段（或选区覆盖段）的所有 run 级样式。
pub fn clear_format(blocks: &mut [Block], from: (usize, usize), to: (usize, usize)) {
    format_range(blocks, from, to, &mut |r| {
        r.marks = 0;
        r.color = None;
        r.bg = None;
        r.size = None;
        r.mono = false;
    });
}

pub fn mark_of(bit: u8) -> Option<Mark> {
    match bit {
        Run::BOLD => Some(Mark::Bold),
        Run::ITALIC => Some(Mark::Italic),
        Run::UNDERLINE => Some(Mark::Underline),
        Run::STRIKE => Some(Mark::Strike),
        _ => None,
    }
}

/// 对选区应用字号。
pub fn format_size(
    blocks: &mut [Block],
    from: (usize, usize),
    to: (usize, usize),
    size: f32,
) {
    format_range(blocks, from, to, &mut |r| r.size = Some(size));
}

/// 选区内的字号是否统一；没有选中内容时返回 None。
pub fn uniform_size_public(
    blocks: &[Block],
    from: (usize, usize),
    to: (usize, usize),
) -> Option<f32> {
    if blocks.is_empty() || from == to {
        return None;
    }
    let (fp, tp) = if from.0 <= to.0 {
        (from.0, to.0)
    } else {
        (to.0, from.0)
    };
    let mut value: Option<f32> = None;
    for p in fp..=tp.min(blocks.len() - 1) {
        let block = &blocks[p];
        let base = size_of(block.kind);
        for r in &block.runs {
            let v = r.size.unwrap_or(base);
            match value {
                None => value = Some(v),
                Some(prev) if (prev - v).abs() > 0.01 => return None,
                _ => {}
            }
        }
    }
    value
}

/// 对选区开关某个位标记（粗/斜/下划线/删除线）。
pub fn format_range_marks(
    blocks: &mut [Block],
    from: (usize, usize),
    to: (usize, usize),
    bit: u8,
    on: bool,
) {
    format_range(blocks, from, to, &mut |r| {
        if on {
            r.marks |= bit;
        } else {
            r.marks &= !bit;
        }
    });
}

/// 对选区设置文字颜色或高亮底色。
pub fn set_text_color(
    blocks: &mut [Block],
    from: (usize, usize),
    to: (usize, usize),
    hex: &str,
    highlight: bool,
) {
    let hex = hex.to_owned();
    format_range(blocks, from, to, &mut |r| {
        if highlight {
            r.bg = Some(hex.clone());
        } else {
            r.color = Some(hex.clone());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Doc;

    #[test]
    fn numbered_prefix_does_not_overlap_body() {
        for count in [1, 9, 10, 99, 100, 999, 1000] {
            let ctx = egui::Context::default();
            let mut rt = SheetEditor::new(egui::Id::unique(("number-layout", count)));
            let mut blocks: Vec<Block> = (0..count).map(|index| {
                let mut block = Block::with_text(if index + 1 == count { "891 102" } else { "row" });
                block.kind = BlockKind::Number;
                block
            }).collect();
            let mut dirty = false;
            let mut bounds = None;
            for frame in 0..3 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO, vec2(640.0, 50.0 * count as f32 + 100.0),
                        )),
                        time: Some(frame as f64 / 60.0),
                        ..Default::default()
                    },
                    |ui| {
                        rt.editor.input(ui, &mut ActiveSheet {
                            blocks: &mut blocks, undo: &mut rt.undo,
                            pending: &mut rt.pending, dirty: &mut dirty,
                        });
                        draw_document(ui, &blocks, &mut rt, 480.0, theme::TEXT, &[], &mut None);
                    },
                );
                let text_rect = |needle: &str| output.shapes.iter().find_map(|shape| {
                    if let egui::Shape::Text(text) = &shape.shape
                        && text.galley.text() == needle
                    {
                        Some(text.visual_bounding_rect())
                    } else {
                        None
                    }
                }).unwrap_or_else(|| panic!("未绘制 {needle}"));
                bounds = Some((text_rect(&format!("{count}.")), text_rect("891 102")));
                if count > 1 {
                    assert!((text_rect("row").left() - text_rect("891 102").left()).abs() < 1.0,
                        "同一编号列表的正文应对齐");
                }
                output.textures_delta.clear();
            }
            let (prefix, body) = bounds.unwrap();
            assert!(prefix.bottom() > body.top() && prefix.top() < body.bottom());
            assert!(prefix.left() >= 0.0, "编号 {count} 超出了段落左边界");
            assert!(prefix.right() + 6.0 <= body.left(),
                "编号 {count} 与正文重叠：编号右侧 {}，正文左侧 {}", prefix.right(), body.left());
        }
    }

    fn image_block() -> Block {
        let mut image = Block::with_text("img-regression-asset");
        image.kind = BlockKind::Image;
        image
    }

    #[test]
    fn image_insertion_preserves_unicode_runs_at_start_middle_and_end() {
        let mut original = Block::with_text("中文");
        original.runs[0].marks = Run::BOLD;
        original.runs.push(Run::plain("测试"));
        original.align = Some(DocAlign::Right);
        let ctx = egui::Context::default();
        for offset in [0, 2, 4] {
            let mut blocks = vec![original.clone()];
            let mut rt = SheetEditor::new(egui::Id::unique(("insert", offset)));
            rt.editor.select(Selection::caret(Position::new(0, offset)));
            rt.insert_image(&ctx, &mut blocks, "img-new".to_owned());
            let image = image_index(&blocks, "img-new").unwrap();
            assert_eq!(blocks[..image].iter().map(Block::text).collect::<String>(),
                original.text().chars().take(offset).collect::<String>());
            assert_eq!(blocks[image + 1..].iter().map(Block::text).collect::<String>(),
                original.text().chars().skip(offset).collect::<String>());
            if offset == 2 {
                assert_eq!(blocks[0].runs[0].marks, Run::BOLD);
                assert_eq!(blocks[2].runs[0].marks, 0);
                assert_eq!(blocks[2].align, original.align);
            }
            assert!(rt.editor.selection().is_none());
            assert_eq!(rt.selected_image.as_deref(), Some("img-new"));
            assert_eq!(rt.undo.undo.len(), 1);
            assert_eq!(rt.undo.undo_step(&blocks).unwrap(), vec![original.clone()]);
        }
        let mut divider = Block::empty();
        divider.kind = BlockKind::Divider;
        let mut blocks = vec![divider.clone(), Block::with_text("后文")];
        assert_eq!(insert_image_at(&mut blocks, Position::new(0, 0), "img-new".to_owned()), 1);
        assert_eq!(blocks[0], divider);
        assert_eq!(blocks[2].text(), "后文");
    }

    #[test]
    fn replacing_across_image_boundaries_never_slices_asset_ids() {
        for (from, to, expected, image_remains) in [
            ((0, 1), (1, 0), vec!["前替", "img-regression-asset", "后文"], true),
            ((0, 1), (1, 1), vec!["前替", "后文"], false),
            ((1, 0), (2, 1), vec!["前文", "替文"], false),
            ((1, 1), (2, 1), vec!["前文", "img-regression-asset", "替文"], true),
        ] {
            for (from, to) in [(from, to), (to, from)] {
                let mut blocks = vec![Block::with_text("前文"), image_block(), Block::with_text("后文")];
                let at = replace_objects(&mut blocks, from, to, "替");
                assert_eq!(blocks.iter().map(Block::text).collect::<Vec<_>>(), expected);
                assert_eq!(image_index(&blocks, "img-regression-asset").is_some(), image_remains);
                assert!(at.paragraph < blocks.len());
                assert!(at.offset <= blocks[at.paragraph].char_len());
            }
        }
    }

    #[test]
    fn images_are_atomic_for_text_split_and_formatting() {
        for offset in [0, 1] {
            let mut blocks = vec![image_block()];
            let mut undo = UndoStack::default();
            let mut pending = PendingFmt::default();
            let mut dirty = false;
            let mut model = ActiveSheet {
                blocks: &mut blocks, undo: &mut undo, pending: &mut pending, dirty: &mut dirty,
            };
            assert_eq!(model.text(&0).as_deref(), Some("\u{fffc}"));
            let at = model.apply(Edit::Split { at: Position::new(0, offset) }, true).unwrap();
            assert_eq!(at, Position::new(offset, 0));
            assert_eq!(blocks[1 - offset], image_block());
            format_range(&mut blocks, (0, 0), (1, 1), &mut |run| run.marks = Run::BOLD);
            assert_eq!(blocks[1 - offset], image_block());
        }
    }

    #[test]
    fn enter_then_cleanup_keeps_the_new_paragraph_editable() {
        for kind in [BlockKind::Body, BlockKind::Bullet, BlockKind::Number, BlockKind::Divider] {
            let mut doc = Doc::new("换行");
            let mut block = Block::with_text("第一项");
            block.kind = kind;
            if kind == BlockKind::Divider {
                block.runs.clear();
            }
            let offset = block.char_len();
            doc.sheets[0].blocks = vec![block];
            let mut undo = UndoStack::default();
            let mut pending = PendingFmt::default();
            let mut dirty = false;
            let next = ActiveSheet {
                blocks: &mut doc.sheets[0].blocks,
                undo: &mut undo,
                pending: &mut pending,
                dirty: &mut dirty,
            }
            .apply(Edit::Split { at: Position::new(0, offset) }, true)
            .unwrap();

            // 编辑器下一帧会先清理图片，再接收新行的输入。
            assert!(!doc.prune_images(), "{kind:?} 的新行被清理");
            assert_eq!(doc.sheets[0].blocks.len(), 2);
            let expected_kind = if kind == BlockKind::Divider { BlockKind::Body } else { kind };
            assert_eq!(doc.sheets[0].blocks[1].kind, expected_kind);
            ActiveSheet {
                blocks: &mut doc.sheets[0].blocks,
                undo: &mut undo,
                pending: &mut pending,
                dirty: &mut dirty,
            }
            .apply(Edit::Replace { from: next.clone(), to: next, text: "下一项" }, true)
            .unwrap();
            assert_eq!(doc.sheets[0].blocks[1].text(), "下一项");
        }
    }

    #[test]
    fn enter_on_empty_list_exits_in_place_and_can_be_undone() {
        for kind in [BlockKind::Bullet, BlockKind::Number] {
            let mut blocks = vec![Block::with_text("第一项"), Block::empty()];
            for block in &mut blocks {
                block.kind = kind;
            }
            let original = blocks.clone();
            let mut undo = UndoStack::default();
            let mut pending = PendingFmt::default();
            let mut dirty = false;
            let next = ActiveSheet {
                blocks: &mut blocks,
                undo: &mut undo,
                pending: &mut pending,
                dirty: &mut dirty,
            }
            .apply(Edit::Split { at: Position::new(1, 0) }, true)
            .unwrap();

            assert_eq!(next, Position::new(1, 0));
            assert_eq!(blocks.len(), 2);
            assert_eq!(blocks[0].kind, kind);
            assert_eq!(blocks[1].kind, BlockKind::Body);
            assert!(dirty);
            let restored = undo.undo_step(&blocks).unwrap();
            assert_eq!(restored, original);
            assert_eq!(undo.redo_step(&restored).unwrap(), blocks);
        }
    }

    #[test]
    fn divider_insertion_keeps_text_and_the_following_empty_paragraph() {
        for text in ["", "已有文字"] {
            let mut doc = Doc::new("分割线");
            doc.sheets[0].blocks = vec![Block::with_text(text)];
            let next = insert_divider_after(&mut doc.sheets[0].blocks, 0);
            assert!(!doc.prune_images());
            let blocks = &doc.sheets[0].blocks;
            assert_eq!(blocks[next - 1].kind, BlockKind::Divider);
            assert_eq!(blocks[next].kind, BlockKind::Body);
            assert!(blocks[next].is_empty());
            if !text.is_empty() {
                assert_eq!(blocks[0].text(), text);
            }
        }
    }
}

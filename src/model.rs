//! 文档模型：`.ecnote` 解密后的 JSON 结构，以及编辑操作。
//!
//! 与加密无关——这一层只关心「多张工作表 + 每张表若干段落 + 段内 runs」。

use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "ecnote";
pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_SHEETS: usize = 64;
pub const MAX_SHEET_NAME: usize = 31;
pub const MAX_DOC_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_DOC_CHARS: usize = 1_000_000;

/// 段落对齐（缺省为两端对齐，与 Word 的中文默认一致）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    Left,
    Center,
    Right,
    #[default]
    Justify,
}

/// 段落类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BlockKind {
    #[default]
    Body,
    H1,
    H2,
    H3,
    Quote,
    Code,
    Bullet,
    Number,
    /// 分割线：一条水平细线，不承载文字（仅作分隔）。
    Divider,
    Image,
}

impl BlockKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Body => "正文",
            Self::H1 => "标题 1",
            Self::H2 => "标题 2",
            Self::H3 => "标题 3",
            Self::Quote => "引用",
            Self::Code => "代码块",
            Self::Bullet => "项目符号",
            Self::Number => "编号",
            Self::Divider => "分割线",
            Self::Image => "图片",
        }
    }
}

/// 一段连续、格式相同的文字。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Run {
    #[serde(default)]
    pub text: String,
    /// 位掩码：1 粗体 2 斜体 4 下划线 8 删除线
    #[serde(default)]
    pub marks: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f32>,
    /// 等宽字体（代码）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mono: bool,
}

impl Run {
    pub const BOLD: u8 = 1;
    pub const ITALIC: u8 = 2;
    pub const UNDERLINE: u8 = 4;
    pub const STRIKE: u8 = 8;

    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            marks: 0,
            color: None,
            bg: None,
            size: None,
            mono: false,
        }
    }

    #[allow(dead_code)]
    pub fn marked(&self, bit: u8) -> bool {
        self.marks & bit != 0
    }

    fn same_format(&self, other: &Self) -> bool {
        self.marks == other.marks
            && self.color == other.color
            && self.bg == other.bg
            && self.size == other.size
            && self.mono == other.mono
    }

    pub fn char_len(&self) -> usize {
        self.text.chars().count()
    }
}

/// 段落。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Block {
    #[serde(default)]
    pub kind: BlockKind,
    #[serde(default)]
    pub runs: Vec<Run>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<Align>,
    /// 图片块显示宽度占内容宽度的比例（0.15..=1.0）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_width: Option<f32>,
}

impl Block {
    pub fn empty() -> Self {
        Self {
            kind: BlockKind::Body,
            runs: Vec::new(),
            align: None,
            image_width: None,
        }
    }

    #[allow(dead_code)]
    pub fn with_text(text: impl Into<String>) -> Self {
        Self {
            runs: vec![Run::plain(text)],
            ..Self::empty()
        }
    }

    pub fn text(&self) -> String {
        let mut s = String::new();
        for r in &self.runs {
            s.push_str(&r.text);
        }
        s
    }

    pub fn char_len(&self) -> usize {
        self.runs.iter().map(Run::char_len).sum()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.runs.iter().all(|r| r.text.is_empty())
    }

    /// 合并相邻的、格式完全相同的 runs，避免 JSON 膨胀。
    pub fn normalize(&mut self) {
        let mut out: Vec<Run> = Vec::with_capacity(self.runs.len());
        for r in self.runs.drain(..) {
            if r.text.is_empty() {
                continue;
            }
            if let Some(last) = out.last_mut()
                && last.same_format(&r)
            {
                last.text.push_str(&r.text);
                continue;
            }
            out.push(r);
        }
        self.runs = out;
    }

    /// 该段落第一个字符的格式（用于在段首插入文字）。
    #[allow(dead_code)]
    pub fn first_format(&self) -> Run {
        let mut base = self
            .runs
            .first()
            .cloned()
            .unwrap_or_else(|| Run::plain(""));
        base.text.clear();
        base
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sheet {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default)]
    pub blocks: Vec<Block>,
}

impl Sheet {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            color: None,
            blocks: vec![Block::empty()],
        }
    }

    pub fn plain_text(&self) -> String {
        self.blocks
            .iter()
            .map(Block::text)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// 内嵌图片。`data` 为 base64 编码的 PNG，随文档一起加密。
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ImageAsset {
    pub id: String,
    pub mime: String,
    pub w: u32,
    pub h: u32,
    pub data: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Doc {
    pub schema: String,
    pub schema_version: u32,
    pub doc_id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub active_sheet: String,
    pub sheets: Vec<Sheet>,
    #[serde(default)]
    pub images: Vec<ImageAsset>,
}

impl Doc {
    pub fn new(title: impl Into<String>) -> Self {
        let title = title.into();
        let sheet = Sheet::new("s1", "Sheet1");
        let now = crate::recent::now_string();
        Self {
            schema: SCHEMA.to_owned(),
            schema_version: SCHEMA_VERSION,
            doc_id: crate::recent::random_id(),
            title,
            created_at: now.clone(),
            updated_at: now,
            active_sheet: sheet.id.clone(),
            sheets: vec![sheet],
            images: Vec::new(),
        }
    }

    pub fn sheet_index(&self, id: &str) -> Option<usize> {
        self.sheets.iter().position(|s| s.id == id)
    }

    pub fn active_index(&self) -> usize {
        self.sheet_index(&self.active_sheet).unwrap_or(0)
    }

    pub fn normalize(&mut self) {
        if self.sheets.is_empty() {
            let s = Sheet::new("s1", "Sheet1");
            self.active_sheet = s.id.clone();
            self.sheets.push(s);
        }
        // 名字兜底 + 去重
        let mut seen: Vec<String> = Vec::new();
        for (i, s) in self.sheets.iter_mut().enumerate() {
            if s.name.trim().is_empty() {
                s.name = format!("Sheet{}", i + 1);
            }
            s.name = s.name.chars().take(MAX_SHEET_NAME).collect();
            if s.name.contains([':', '\\', '/', '?', '*', '[', ']']) {
                s.name = s.name.replace([':', '\\', '/', '?', '*', '[', ']'], "");
            }
            let mut base = s.name.clone();
            let mut n = 2;
            while seen.iter().any(|x| x == &s.name) {
                let suffix = format!(" ({n})");
                s.name = format!("{}{}", base.chars().take(MAX_SHEET_NAME - suffix.len()).collect::<String>(), suffix);
                n += 1;
            }
            base.clear();
            seen.push(s.name.clone());
            if s.id.is_empty() {
                s.id = format!("s{}", i + 1);
            }
            if s.blocks.is_empty() {
                s.blocks.push(Block::empty());
            }
        }
        if self.sheet_index(&self.active_sheet).is_none() {
            self.active_sheet = self.sheets[0].id.clone();
        }
        self.sheets.truncate(MAX_SHEETS);
        self.updated_at = crate::recent::now_string();
    }

    /// 图片被删掉后，块里可能残留一个空壳（id 已失效）。
    /// 把这类块清掉，避免纸面上留下一块「图片」占位。
    pub fn prune_images(&mut self) -> bool {
        let known: Vec<String> = self.images.iter().map(|a| a.id.clone()).collect();
        let mut changed = false;
        for sheet in &mut self.sheets {
            let mut changed_here = false;
            for block in &mut sheet.blocks {
                let was_image = block.kind == BlockKind::Image;
                // 正常图片块：文本恰好是某个资产 id
                if was_image && known.iter().any(|k| &block.text() == k) {
                    continue;
                }
                // 图片段落被前后段落合并时，id 会粘在正文开头或结尾，去掉它
                let mut stripped = false;
                loop {
                    let text = block.text();
                    match known
                        .iter()
                        .find(|k| text.starts_with(k.as_str()) && text.len() > k.len())
                    {
                        Some(k) => {
                            block.runs = split_runs(&block.runs, k.chars().count()).1;
                            stripped = true;
                        }
                        None => break,
                    }
                }
                loop {
                    let text = block.text();
                    match known
                        .iter()
                        .find(|k| text.ends_with(k.as_str()) && text.len() > k.len())
                    {
                        Some(k) => {
                            let cut = text.chars().count() - k.chars().count();
                            block.runs = split_runs(&block.runs, cut).0;
                            stripped = true;
                        }
                        None => break,
                    }
                }
                if was_image {
                    changed_here = true;
                    if !stripped || block.text().is_empty() {
                        // id 被删残，或合并后什么都不剩：整块丢弃
                        block.runs.clear();
                    } else {
                        block.kind = BlockKind::Body;
                    }
                } else if stripped {
                    changed_here = true;
                }
            }
            let before = sheet.blocks.len();
            // 只删除失效图片，空正文和空列表项都是有效的编辑位置。
            sheet.blocks
                .retain(|b| b.kind != BlockKind::Image || !b.runs.is_empty());
            if sheet.blocks.is_empty() {
                sheet.blocks.push(Block::empty());
                changed_here = true;
            }
            if sheet.blocks.len() != before {
                changed_here = true;
            }
            changed |= changed_here;
        }
        changed
    }
}

// ---------------------------------------------------------------------------
// 编辑操作（供 RichEdit 的 Model 实现调用）
// ---------------------------------------------------------------------------

pub fn char_to_byte(s: &str, offset: usize) -> usize {
    s.char_indices().nth(offset).map_or(s.len(), |(i, _)| i)
}

/// 把 runs 在 `char_at` 处切成两半。
fn split_runs(runs: &[Run], char_at: usize) -> (Vec<Run>, Vec<Run>) {
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut pos = 0usize;
    let mut done = false;
    for r in runs {
        let n = r.char_len();
        if done {
            right.push(r.clone());
            continue;
        }
        if char_at >= pos + n {
            left.push(r.clone());
            pos += n;
            continue;
        }
        let cut = char_at - pos;
        let b = char_to_byte(&r.text, cut);
        let mut a = r.clone();
        a.text = r.text[..b].to_owned();
        let mut c = r.clone();
        c.text = r.text[b..].to_owned();
        left.push(a);
        right.push(c);
        done = true;
        pos = char_at;
    }
    (left, right)
}

fn style_at(runs: &[Run], char_at: usize) -> Run {
    let mut pos = 0usize;
    let mut style = runs.first().cloned().unwrap_or_else(|| Run::plain(""));
    for r in runs {
        let n = r.char_len();
        if char_at <= pos + n {
            style = r.clone();
            break;
        }
        pos += n;
    }
    style.text.clear();
    style
}

fn split_text_lines(text: &str) -> Vec<&str> {
    text.split('\n').collect()
}

/// 用 `text` 替换 `[from, to)`。支持跨段落与粘贴的换行。
pub fn replace_range(
    blocks: &mut Vec<Block>,
    from: (usize, usize),
    to: (usize, usize),
    text: &str,
) {
    if blocks.is_empty() {
        blocks.push(Block::empty());
    }
    let (fp, fo) = from;
    let (tp, too) = to;
    let fp = fp.min(blocks.len() - 1);
    let tp = tp.min(blocks.len() - 1);
    let (fp, tp) = if fp <= tp { (fp, tp) } else { (tp, fp) };

    let src_runs = blocks[fp].runs.clone();
    let (prefix, mut rest) = split_runs(&src_runs, fo.min(blocks[fp].char_len()));

    let tail_runs = if tp > fp {
        let last = blocks[tp].runs.clone();
        let (_, tail) = split_runs(&last, too.min(blocks[tp].char_len()));
        tail
    } else {
        let (_, tail) = split_runs(&rest, too.saturating_sub(fo).min(blocks[fp].char_len().saturating_sub(fo)));
        tail
    };

    let style = style_at(&src_runs, fo);
    let kind = blocks[fp].kind;
    let align = blocks[fp].align;

    // 删除跨段范围（保留 fp 作为合并后的段落）
    if tp > fp {
        blocks.drain(fp + 1..=tp);
    }
    rest.clear();

    let lines = split_text_lines(text);
    let mut merged: Vec<Block> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let mut runs: Vec<Run> = Vec::new();
        if i == 0 {
            runs.extend(prefix.iter().cloned());
        }
        if !line.is_empty() {
            let mut r = style.clone();
            r.text = (*line).to_owned();
            runs.push(r);
        }
        if i == lines.len() - 1 {
            runs.extend(tail_runs.iter().cloned());
        }
        let mut b = Block {
            kind,
            runs,
            align,
            image_width: None,
        };
        b.normalize();
        merged.push(b);
    }
    if merged.is_empty() {
        merged.push(Block {
            kind,
            runs: Vec::new(),
            align,
            image_width: None,
        });
    }

    let insert_at = fp;
    let count = merged.len();
    blocks.splice(insert_at..insert_at + 1, merged);
    let _ = count;
    // 分割线被合并进正文（例如在它后面退格）时，别让文字藏在「看不见」的块里
    if let Some(b) = blocks.get_mut(insert_at)
        && b.kind == BlockKind::Divider
        && b.char_len() > 0
    {
        b.kind = BlockKind::Body;
    }
}

/// 在 `at` 处拆分段落，返回新段落位置。
pub fn split_block_at(blocks: &mut Vec<Block>, at: (usize, usize)) -> Option<(usize, usize)> {
    let (p, off) = at;
    let block = blocks.get_mut(p)?;
    let off = off.min(block.char_len());
    let (left, right) = split_runs(&block.runs, off);
    block.runs = left;
    block.normalize();
    let mut second = Block {
        kind: block.kind,
        runs: right,
        align: block.align,
        image_width: block.image_width,
    };
    second.normalize();
    blocks.insert(p + 1, second);
    Some((p + 1, 0))
}

/// 对 `[from, to)` 内的所有 runs 应用格式修改，自动在边界处切开 run。
pub fn format_range(
    blocks: &mut [Block],
    from: (usize, usize),
    to: (usize, usize),
    f: &mut dyn FnMut(&mut Run),
) {
    if blocks.is_empty() {
        return;
    }
    let (fp, fo) = from;
    let (tp, too) = to;
    let (fp, tp) = if fp <= tp { (fp, tp) } else { (tp, fp) };
    for p in fp..=tp.min(blocks.len() - 1) {
        let block = &mut blocks[p];
        if block.kind == BlockKind::Image {
            continue;
        }
        let len = block.char_len();
        let start = if p == fp { fo.min(len) } else { 0 };
        let end = if p == tp { too.min(len) } else { len };
        if start >= end {
            continue;
        }
        let mut out: Vec<Run> = Vec::new();
        let mut pos = 0usize;
        for r in block.runs.drain(..) {
            let n = r.char_len();
            let a = pos;
            let b = pos + n;
            pos = b;
            if b <= start || a >= end {
                out.push(r);
                continue;
            }
            let cut_a = start.saturating_sub(a).min(n);
            let cut_b = end.saturating_sub(a).min(n);
            let ba = char_to_byte(&r.text, cut_a);
            let bb = char_to_byte(&r.text, cut_b);
            if cut_a > 0 {
                let mut before = r.clone();
                before.text = r.text[..ba].to_owned();
                out.push(before);
            }
            let mut mid = r.clone();
            mid.text = r.text[ba..bb].to_owned();
            f(&mut mid);
            if !mid.text.is_empty() {
                out.push(mid);
            }
            if cut_b < n {
                let mut after = r.clone();
                after.text = r.text[bb..].to_owned();
                out.push(after);
            }
        }
        block.runs = out;
        block.normalize();
    }
}

/// 在光标处插入一个图片块（v1 以独立段落呈现）。
#[allow(dead_code)]
pub fn insert_block_after(doc_blocks: &mut Vec<Block>, at: usize, block: Block) -> usize {
    let idx = (at + 1).min(doc_blocks.len());
    doc_blocks.insert(idx, block);
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(b: &Block) -> String {
        b.text()
    }

    #[test]
    fn split_keeps_format() {
        let mut b = Block::with_text("hello世界");
        b.runs[0].marks = Run::BOLD;
        let (l, r) = split_runs(&b.runs, 5);
        assert_eq!(l[0].text, "hello");
        assert_eq!(r[0].text, "世界");
        assert_eq!(l[0].marks, Run::BOLD);
        assert_eq!(r[0].marks, Run::BOLD);
    }

    #[test]
    fn replace_inside_one_block() {
        let mut blocks = vec![Block::with_text("abcdef")];
        replace_range(&mut blocks, (0, 2), (0, 4), "XY");
        assert_eq!(texts(&blocks[0]), "abXYef");
    }

    #[test]
    fn replace_joins_blocks() {
        let mut blocks = vec![
            Block::with_text("hello"),
            Block::with_text("world"),
        ];
        replace_range(&mut blocks, (0, 3), (1, 2), "-");
        assert_eq!(blocks.len(), 1);
        assert_eq!(texts(&blocks[0]), "hel-rld");
    }

    #[test]
    fn paste_multiline_splits() {
        let mut blocks = vec![Block::with_text("ac")];
        replace_range(&mut blocks, (0, 1), (0, 1), "1\n2\n3");
        assert_eq!(blocks.len(), 3);
        assert_eq!(texts(&blocks[0]), "a1");
        assert_eq!(texts(&blocks[1]), "2");
        assert_eq!(texts(&blocks[2]), "3c");
    }

    #[test]
    fn format_crosses_blocks() {
        let mut blocks = vec![Block::with_text("abc"), Block::with_text("def")];
        format_range(&mut blocks, (0, 1), (1, 2), &mut |r| r.marks |= Run::BOLD);
        assert_eq!(blocks[0].text(), "abc");
        assert_eq!(blocks[0].runs[1].text, "bc");
        assert!(blocks[0].runs[1].marked(Run::BOLD));
        assert_eq!(blocks[1].runs[0].text, "de");
        assert!(blocks[1].runs[0].marked(Run::BOLD));
        assert!(!blocks[1].runs[1].marked(Run::BOLD));
    }

    #[test]
    fn normalize_merges() {
        let mut b = Block {
            kind: BlockKind::Body,
            runs: vec![Run::plain("a"), Run::plain("b")],
            align: None,
            image_width: None,
        };
        b.normalize();
        assert_eq!(b.runs.len(), 1);
        assert_eq!(b.text(), "ab");
    }

    #[test]
    fn doc_normalize_repairs() {
        let mut d = Doc::new("t");
        d.sheets.clear();
        d.normalize();
        assert_eq!(d.sheets.len(), 1);
        assert_eq!(d.active_sheet, d.sheets[0].id);
    }
}

#[cfg(test)]
mod prune_tests {
    use super::*;

    #[test]
    fn orphan_image_block_is_removed() {
        let mut doc = Doc::new("t");
        let mut img = Block::empty();
        img.kind = BlockKind::Image;
        img.runs = vec![Run::plain("missing-id")];
        doc.sheets[0].blocks = vec![Block::with_text("正文"), img];

        assert!(doc.prune_images());
        assert_eq!(doc.sheets[0].blocks.len(), 1);
        assert_eq!(doc.sheets[0].blocks[0].text(), "正文");
        // 再跑一次不应有任何变化
        assert!(!doc.prune_images());
    }

    #[test]
    fn valid_image_block_survives() {
        let mut doc = Doc::new("t");
        doc.images.push(ImageAsset {
            id: "img-1".to_owned(),
            mime: "image/png".to_owned(),
            w: 10,
            h: 10,
            data: String::new(),
        });
        let mut img = Block::empty();
        img.kind = BlockKind::Image;
        img.runs = vec![Run::plain("img-1")];
        doc.sheets[0].blocks = vec![img];
        assert!(!doc.prune_images());
        assert_eq!(doc.sheets[0].blocks.len(), 1);
    }
}

#[cfg(test)]
mod prune_merge_tests {
    use super::*;

    fn asset(id: &str) -> ImageAsset {
        ImageAsset {
            id: id.to_owned(),
            mime: "image/png".to_owned(),
            w: 1,
            h: 1,
            data: String::new(),
        }
    }

    #[test]
    fn partial_id_is_dropped() {
        let mut doc = Doc::new("t");
        doc.images.push(asset("img-abcdef"));
        let mut img = Block::empty();
        img.kind = BlockKind::Image;
        img.runs = vec![Run::plain("img-abcd")];
        doc.sheets[0].blocks = vec![img];
        assert!(doc.prune_images());
        assert_eq!(doc.sheets[0].blocks.len(), 1);
        assert_eq!(doc.sheets[0].blocks[0].kind, BlockKind::Body);
        assert!(doc.sheets[0].blocks[0].text().is_empty());
    }

    #[test]
    fn merged_text_survives_without_id() {
        let mut doc = Doc::new("t");
        doc.images.push(asset("img-1"));
        let mut img = Block::empty();
        img.kind = BlockKind::Image;
        img.runs = vec![Run::plain("img-1"), Run::plain("后面的正文")];
        doc.sheets[0].blocks = vec![img];
        assert!(doc.prune_images());
        assert_eq!(doc.sheets[0].blocks[0].kind, BlockKind::Body);
        assert_eq!(doc.sheets[0].blocks[0].text(), "后面的正文");
    }
}

#[cfg(test)]
mod prune_merge_suffix_tests {
    use super::*;

    #[test]
    fn heading_merged_with_image_id_keeps_heading() {
        let mut doc = Doc::new("t");
        doc.images.push(ImageAsset {
            id: "img-1".to_owned(),
            mime: "image/png".to_owned(),
            w: 1,
            h: 1,
            data: String::new(),
        });
        let mut h = Block::with_text("标题");
        h.kind = BlockKind::H1;
        h.runs.push(Run::plain("img-1"));
        doc.sheets[0].blocks = vec![h];
        assert!(doc.prune_images());
        assert_eq!(doc.sheets[0].blocks[0].kind, BlockKind::H1);
        assert_eq!(doc.sheets[0].blocks[0].text(), "标题");
    }
}

#[cfg(test)]
mod divider_tests {
    use super::*;

    #[test]
    fn empty_divider_survives_prune() {
        let mut doc = Doc::new("t");
        let mut d = Block::empty();
        d.kind = BlockKind::Divider;
        doc.sheets[0].blocks = vec![Block::with_text("上"), d, Block::with_text("下")];
        doc.prune_images();
        assert_eq!(doc.sheets[0].blocks.len(), 3);
        assert_eq!(doc.sheets[0].blocks[1].kind, BlockKind::Divider);
    }

    #[test]
    fn typing_into_divider_turns_it_into_body() {
        let mut blocks = vec![Block::with_text("abc")];
        blocks[0].kind = BlockKind::Divider;
        blocks[0].runs.clear();
        replace_range(&mut blocks, (0, 0), (0, 0), "x");
        assert_eq!(blocks[0].kind, BlockKind::Body);
        assert_eq!(blocks[0].text(), "x");
    }

    #[test]
    fn merging_into_divider_keeps_text_visible() {
        let mut blocks = vec![Block::with_text("上"), Block::empty(), Block::with_text("下")];
        blocks[1].kind = BlockKind::Divider;
        // 在分割线段首退格：跨段删除会把它并进第一段
        replace_range(&mut blocks, (0, 1), (1, 0), "");
        assert_eq!(blocks.len(), 2);
        assert_ne!(blocks[0].kind, BlockKind::Divider);
        assert_eq!(blocks[0].text(), "上");
        assert_eq!(blocks[1].text(), "下");
    }
}

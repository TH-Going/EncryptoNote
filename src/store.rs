//! 笔记文件的创建 / 解锁 / 保存 / 改密码。
//!
//! 布局：4 KiB 头 + 图片区密文（0x1000 起，可空）+ 文档区密文（`doc_off`）。
//! 正文保存把新的文档密文追加到文件尾，再原子地切换头槽；图片区保持不动。

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::crypto::{self, Argon2Params};
use crate::format::{
    DATA_OFFSET, DEK_LEN, FileHeader, HeaderError, SHA256_LEN, Slot, ciphertext_len,
    chunk_count,
};
use crate::model::{Doc, MAX_DOC_BYTES, MAX_DOC_CHARS};
use crate::recent;
use crate::stream::{self, AAD_DOC};

pub const EXT: &str = "ecnote";

pub struct Opened {
    pub path: PathBuf,
    pub doc: Doc,
    pub header: FileHeader,
    pub slot: Slot,
    /// 当前会话的 DEK，锁定/退出时随结构体一起清零。
    dek: Zeroizing<[u8; DEK_LEN]>,
}

impl std::fmt::Debug for Opened {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Opened")
            .field("path", &self.path)
            .field("title", &self.doc.title)
            .field("sheets", &self.doc.sheets.len())
            .finish()
    }
}

fn random_nonce() -> [u8; 8] {
    crypto::random_stream_nonce()
}

fn encrypt_doc(
    dek: &[u8; DEK_LEN],
    doc: &Doc,
    nonce: &[u8; 8],
) -> Result<(Vec<u8>, [u8; SHA256_LEN], u64)> {
    let json = serde_json::to_vec(doc).context("序列化文档失败")?;
    if json.len() as u64 > MAX_DOC_BYTES {
        bail!("笔记内容超过 8 MiB 上限，请拆分到多个笔记");
    }
    let mut ct = Vec::with_capacity(json.len() + 16 * 128);
    let stats = stream::encrypt_stream(
        dek,
        nonce,
        AAD_DOC,
        json.len() as u64,
        chunk_count(json.len() as u64),
        &json[..],
        &mut ct,
    )
    .context("加密文档失败")?;
    Ok((ct, stats.sha256, json.len() as u64))
}

fn decrypt_doc(dek: &[u8; DEK_LEN], slot: &Slot, file: &mut File) -> Result<Doc> {
    file.seek(SeekFrom::Start(slot.doc_off))?;
    let mut ct = vec![0u8; ciphertext_len(slot.doc_len) as usize];
    file.read_exact(&mut ct)
        .context("文件被截断：文档区不完整")?;
    let mut pt = Vec::with_capacity(slot.doc_len as usize);
    let stats = stream::decrypt_stream(
        dek,
        &slot.doc_nonce,
        AAD_DOC,
        slot.doc_len,
        chunk_count(slot.doc_len),
        &ct[..],
        &mut pt,
    )
    .context("解密文档失败")?;
    if stats.sha256 != slot.doc_sha256 {
        bail!("文档校验失败：文件已损坏或被篡改");
    }
    let json: Value = serde_json::from_slice(&pt).context("文档不是有效的 JSON")?;
    if json.get("schema").and_then(Value::as_str) != Some(crate::model::SCHEMA) {
        bail!("文档格式不支持（schema 不匹配）");
    }
    let mut doc: Doc = serde_json::from_value(json).context("文档字段缺失或类型错误")?;
    doc.normalize();
    Ok(doc)
}

fn decrypt_images(_dek: &[u8; DEK_LEN], slot: &Slot, _file: &mut File) -> Result<Vec<u8>> {
    if slot.img_len == 0 {
        return Ok(Vec::new());
    }
    // v1 图片尚未启用，但保留结构与校验路径
    Ok(Vec::new())
}

/// 新建笔记并落盘。
pub fn create(dir: &Path, file_stem: &str, password: &str) -> Result<Opened> {
    create_with_params(dir, file_stem, password, Argon2Params::DEFAULT)
}

fn create_with_params(
    dir: &Path,
    file_stem: &str,
    password: &str,
    params: Argon2Params,
) -> Result<Opened> {
    if password.is_empty() {
        bail!("密码不能为空");
    }
    let stem = recent::sanitize_file_name(file_stem);
    let path = dir.join(format!("{stem}.{EXT}"));
    if path.exists() {
        bail!("同目录下已存在「{}」", path.file_name().unwrap().to_string_lossy());
    }
    std::fs::create_dir_all(dir)?;

    let mut doc = Doc::new(stem.clone());
    doc.normalize();

    let dek = crypto::random_dek();
    let salt = crypto::random_salt();
    let kek = crypto::derive_kek(password, &salt, &params)?;
    let (wrapped, wrap_nonce) = crypto::wrap_dek(&kek, &dek)?;

    let doc_nonce = random_nonce();
    let (doc_ct, doc_sha, doc_len) = encrypt_doc(&dek, &doc, &doc_nonce)?;

    let slot = Slot {
        params,
        salt,
        wrapped_dek: wrapped,
        wrap_nonce,
        doc_nonce,
        doc_len,
        doc_sha256: doc_sha,
        doc_off: DATA_OFFSET,
        img_nonce: [0u8; 8],
        img_sha256: [0u8; SHA256_LEN],
        img_len: 0,
        flags: 0,
    };
    let header = FileHeader::new(slot.clone());

    let mut file = File::create(&path).with_context(|| format!("无法创建 {}", path.display()))?;
    header.write_to(&mut file)?;
    file.seek(SeekFrom::Start(DATA_OFFSET))?;
    file.write_all(&doc_ct)?;
    file.sync_all()?;

    Ok(Opened {
        path,
        doc,
        header,
        slot,
        dek,
    })
}

/// 打开已有笔记。密码错误时返回可读错误。
pub fn open(path: &Path, password: &str) -> Result<Opened> {
    let mut file = File::open(path).with_context(|| format!("无法打开 {}", path.display()))?;
    if path.extension().and_then(|e| e.to_str()) != Some(EXT) {
        bail!("不是 .ecnote 笔记文件");
    }
    let header = match FileHeader::read_from(&mut file)? {
        Ok(h) => h,
        Err(e) => bail!("{e}"),
    };
    let slot = header
        .read()
        .cloned()
        .ok_or_else(|| anyhow!("{}", HeaderError::NoValidSlot))?;

    let kek = crypto::derive_kek(password, &slot.salt, &slot.params)?;
    let dek = crypto::unwrap_dek(&kek, &slot.wrapped_dek, &slot.wrap_nonce)
        .ok_or_else(|| anyhow!("密码不正确"))?;

    let doc = decrypt_doc(&dek, &slot, &mut file)?;
    let _img = decrypt_images(&dek, &slot, &mut file)?;
    Ok(Opened {
        path: path.to_path_buf(),
        doc,
        header,
        slot,
        dek,
    })
}

impl Opened {
    /// 原地保存：新文档密文追加到文件尾 + 切换头槽。
    pub fn save(&mut self) -> Result<()> {
        self.doc.normalize();

        let doc_nonce = random_nonce();
        let (ct, sha, len) = encrypt_doc(&self.dek, &self.doc, &doc_nonce)?;

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.path)
            .with_context(|| format!("无法写入 {}", self.path.display()))?;
        let file_len = file.metadata()?.len();
        let img_end = DATA_OFFSET + ciphertext_len(self.slot.img_len);
        let append_at = file_len.max(img_end).max(DATA_OFFSET);

        file.seek(SeekFrom::Start(append_at))?;
        file.write_all(&ct)?;
        file.flush()?;

        let mut slot = self.slot.clone();
        slot.doc_nonce = doc_nonce;
        slot.doc_len = len;
        slot.doc_sha256 = sha;
        slot.doc_off = append_at;

        let (next_header, target) = self.header.with_updated_slot(slot.clone());
        write_slot(&mut file, &next_header, target)?;
        file.sync_all()?;

        self.header = next_header;
        self.slot = slot;

        // 追加式保存会留下旧文档密文：垃圾超过 64 KiB，或文件已超过有效数据两倍时压缩重写，
        // 避免反复编辑/清空后文件只增不减。
        let live = ciphertext_len(len);
        let doc_end = append_at.saturating_add(live);
        let garbage = append_at.saturating_sub(DATA_OFFSET)
            + file_len.saturating_sub(doc_end);
        if garbage > 64 * 1024 || append_at > DATA_OFFSET.saturating_add(live.saturating_mul(2)) {
            let _ = self.compact();
        }
        Ok(())
    }

    /// 丢弃追加产生的垃圾：头 + 图片 + 文档重排到紧凑布局。
    fn compact(&mut self) -> Result<()> {
        if self.slot.img_len != 0 {
            return Ok(()); // v1 有图片时暂不压缩
        }
        let mut file = OpenOptions::new().read(true).write(true).open(&self.path)?;
        let mut ct = vec![0u8; ciphertext_len(self.slot.doc_len) as usize];
        file.seek(SeekFrom::Start(self.slot.doc_off))?;
        file.read_exact(&mut ct)?;

        let mut slot = self.slot.clone();
        slot.doc_off = DATA_OFFSET;
        let header = FileHeader::new(slot.clone());
        file.seek(SeekFrom::Start(0))?;
        header.write_to(&mut file)?;
        file.seek(SeekFrom::Start(DATA_OFFSET))?;
        file.write_all(&ct)?;
        file.set_len(DATA_OFFSET + ct.len() as u64)?;
        file.sync_all()?;
        self.header = header;
        self.slot = slot;
        Ok(())
    }

    /// 改密码：同一 DEK 用新密码重新包装，只重写一个头槽。
    pub fn change_password(&mut self, new_password: &str) -> Result<()> {
        if new_password.is_empty() {
            bail!("新密码不能为空");
        }
        let salt = crypto::random_salt();
        let kek = crypto::derive_kek(new_password, &salt, &self.slot.params)?;
        let (wrapped, wrap_nonce) = crypto::wrap_dek(&kek, &self.dek)?;

        let mut slot = self.slot.clone();
        slot.salt = salt;
        slot.wrapped_dek = wrapped;
        slot.wrap_nonce = wrap_nonce;

        let mut file = OpenOptions::new().read(true).write(true).open(&self.path)?;
        let (next, target) = self.header.with_updated_slot(slot.clone());
        write_slot(&mut file, &next, target)?;
        file.sync_all()?;
        self.header = next;
        self.slot = slot;
        Ok(())
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// 文档字符数（用于容量提示）。
    pub fn char_count(&self) -> usize {
        self.doc
            .sheets
            .iter()
            .map(|s| s.plain_text().chars().count())
            .sum()
    }

    pub fn over_limit(&self) -> bool {
        self.char_count() > MAX_DOC_CHARS
    }
}

/// 只写序列号与目标槽：两次小幅写入，尽量缩小掉电窗口。
fn write_slot(file: &mut File, header: &FileHeader, target: usize) -> std::io::Result<()> {
    let seq_off = 8 + target as u64 * 8;
    file.seek(SeekFrom::Start(seq_off))?;
    file.write_all(&header.seqs[target].to_le_bytes())?;

    let slot_off = if target == 0 {
        crate::format::SLOT_A_OFFSET as u64
    } else {
        crate::format::SLOT_B_OFFSET as u64
    };
    let bytes = header.slots[target].as_ref().unwrap().encode();
    file.seek(SeekFrom::Start(slot_off))?;
    file.write_all(&bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: Argon2Params = Argon2Params {
        m_cost: 8 * 1024,
        t_cost: 1,
        p_cost: 1,
    };

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("mijian-test-{}", recent::random_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn create_open_roundtrip() {
        let d = dir();
        let mut o = create_with_params(&d, "项目笔记", "pw123456", FAST).unwrap();
        o.doc.sheets[0].blocks[0] = crate::model::Block::with_text("你好");
        o.save().unwrap();
        drop(o);

        let o2 = open(&d.join("项目笔记.ecnote"), "pw123456").unwrap();
        assert_eq!(o2.doc.sheets[0].blocks[0].text(), "你好");
        assert_eq!(o2.doc.title, "项目笔记");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn wrong_password_fails() {
        let d = dir();
        create_with_params(&d, "a", "right", FAST).unwrap();
        let err = open(&d.join("a.ecnote"), "wrong").unwrap_err().to_string();
        assert!(err.contains("密码"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn save_rotates_slots_and_keeps_data() {
        let d = dir();
        let mut o = create_with_params(&d, "b", "pw", FAST).unwrap();
        let first_seq = o.header.seqs[0];
        o.doc.sheets[0].blocks[0] = crate::model::Block::with_text("v1");
        o.save().unwrap();
        assert_eq!(o.header.seqs[1], first_seq + 1);
        o.doc.sheets[0].blocks[0] = crate::model::Block::with_text("v2");
        o.save().unwrap();
        assert_eq!(o.header.seqs[0], first_seq + 2);
        drop(o);
        let o = open(&d.join("b.ecnote"), "pw").unwrap();
        assert_eq!(o.doc.sheets[0].blocks[0].text(), "v2");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn change_password_reopens_with_new() {
        let d = dir();
        let mut o = create_with_params(&d, "c", "old", FAST).unwrap();
        o.doc.sheets[0].blocks[0] = crate::model::Block::with_text("secret");
        o.save().unwrap();
        o.change_password("new").unwrap();
        let p = d.join("c.ecnote");
        assert!(open(&p, "old").is_err());
        let o = open(&p, "new").unwrap();
        assert_eq!(o.doc.sheets[0].blocks[0].text(), "secret");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn clearing_content_shrinks_file() {
        let d = dir();
        let mut o = create_with_params(&d, "s", "pw", FAST).unwrap();
        let base = std::fs::metadata(&o.path).unwrap().len();
        o.doc.sheets[0].blocks = (0..3000)
            .map(|i| crate::model::Block::with_text(&format!("第 {i} 行正文，用来把文件撑大。")))
            .collect();
        o.save().unwrap();
        let big = std::fs::metadata(&o.path).unwrap().len();
        assert!(big > base + 200 * 1024, "big={big} base={base}");
        o.doc.sheets[0].blocks = vec![crate::model::Block::empty()];
        o.save().unwrap();
        let small = std::fs::metadata(&o.path).unwrap().len();
        assert!(small < 64 * 1024, "small={small}");
        drop(o);
        let o2 = open(&d.join("s.ecnote"), "pw").unwrap();
        assert_eq!(o2.doc.sheets[0].blocks.len(), 1);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn refuse_overwrite() {
        let d = dir();
        create_with_params(&d, "dup", "pw", FAST).unwrap();
        assert!(create_with_params(&d, "dup", "pw", FAST).is_err());
        std::fs::remove_dir_all(&d).ok();
    }
}

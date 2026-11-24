//! `.ecnote` 文件头（v2）。
//!
//! 物理布局（小端）：
//!
//! ```text
//! 0x0000  4   magic  "ECNT"
//! 0x0004  2   version = 2
//! 0x0006  2   保留
//! 0x0008  8   槽 A 序列号
//! 0x0010  8   槽 B 序列号
//! 0x0020  480 槽 A
//! 0x0200  480 槽 B
//! 0x1000  ..  图片区密文（可空）
//! doc_off ..  文档区密文
//! ```
//!
//! 两个槽互为备份：每次保存写「不是当前有效」的那个槽，序列号递增；
//! 读取时取 CRC 有效且序列号更大的槽。改密码只重写槽，不动数据区。
//!
//! 槽体把「文档区」和「图片区」各存一套 nonce / 长度 / SHA-256，正文保存
//! 只重写文档区，图片原地保留。

use std::io::{Read, Write};

use crate::crypto::Argon2Params;

pub const MAGIC: [u8; 4] = *b"ECNT";
pub const VERSION: u16 = 2;
pub const HEADER_SIZE: usize = 0x1000;
pub const DATA_OFFSET: u64 = 0x1000;

pub const SLOT_A_OFFSET: usize = 0x20;
pub const SLOT_B_OFFSET: usize = 0x200;
pub const SLOT_SIZE: usize = 480;

pub const CHUNK_SIZE: usize = 64 * 1024;

pub const SALT_LEN: usize = 16;
pub const DEK_LEN: usize = 32;
pub const WRAPPED_DEK_LEN: usize = 48;
pub const WRAP_NONCE_LEN: usize = 24;
pub const STREAM_NONCE_LEN: usize = 8;
pub const SHA256_LEN: usize = 32;

// ---- 槽体字段偏移 ----
pub const OFF_PARAMS: usize = 0; // 12
pub const OFF_SALT: usize = 12; // 16
pub const OFF_WRAPPED_DEK: usize = 28; // 48
pub const OFF_WRAP_NONCE: usize = 76; // 24
pub const OFF_DOC_NONCE: usize = 100; // 8
pub const OFF_DOC_LEN: usize = 108; // 8
pub const OFF_DOC_SHA: usize = 116; // 32
pub const OFF_DOC_OFF: usize = 148; // 8
pub const OFF_IMG_NONCE: usize = 156; // 8
pub const OFF_IMG_SHA: usize = 164; // 32
pub const OFF_IMG_LEN: usize = 196; // 8
pub const OFF_FLAGS: usize = 204; // 4
pub const CRC_OFF: usize = 208;
pub const SLOT_BODY: usize = 208;

const _: () = assert!(CRC_OFF + 4 <= SLOT_SIZE);
const _: () = assert!(SLOT_B_OFFSET + SLOT_SIZE <= HEADER_SIZE);

/// 一个文件头槽。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub params: Argon2Params,
    pub salt: [u8; SALT_LEN],
    pub wrapped_dek: [u8; WRAPPED_DEK_LEN],
    pub wrap_nonce: [u8; WRAP_NONCE_LEN],
    pub doc_nonce: [u8; STREAM_NONCE_LEN],
    pub doc_len: u64,
    pub doc_sha256: [u8; SHA256_LEN],
    pub doc_off: u64,
    pub img_nonce: [u8; STREAM_NONCE_LEN],
    pub img_sha256: [u8; SHA256_LEN],
    pub img_len: u64,
    pub flags: u32,
}

impl Slot {
    pub fn encode(&self) -> [u8; SLOT_SIZE] {
        let mut b = [0u8; SLOT_SIZE];
        b[OFF_PARAMS..OFF_PARAMS + 4].copy_from_slice(&self.params.m_cost.to_le_bytes());
        b[OFF_PARAMS + 4..OFF_PARAMS + 8].copy_from_slice(&self.params.t_cost.to_le_bytes());
        b[OFF_PARAMS + 8..OFF_PARAMS + 12].copy_from_slice(&self.params.p_cost.to_le_bytes());
        b[OFF_SALT..OFF_SALT + SALT_LEN].copy_from_slice(&self.salt);
        b[OFF_WRAPPED_DEK..OFF_WRAPPED_DEK + WRAPPED_DEK_LEN].copy_from_slice(&self.wrapped_dek);
        b[OFF_WRAP_NONCE..OFF_WRAP_NONCE + WRAP_NONCE_LEN].copy_from_slice(&self.wrap_nonce);
        b[OFF_DOC_NONCE..OFF_DOC_NONCE + STREAM_NONCE_LEN].copy_from_slice(&self.doc_nonce);
        b[OFF_DOC_LEN..OFF_DOC_LEN + 8].copy_from_slice(&self.doc_len.to_le_bytes());
        b[OFF_DOC_SHA..OFF_DOC_SHA + SHA256_LEN].copy_from_slice(&self.doc_sha256);
        b[OFF_DOC_OFF..OFF_DOC_OFF + 8].copy_from_slice(&self.doc_off.to_le_bytes());
        b[OFF_IMG_NONCE..OFF_IMG_NONCE + STREAM_NONCE_LEN].copy_from_slice(&self.img_nonce);
        b[OFF_IMG_SHA..OFF_IMG_SHA + SHA256_LEN].copy_from_slice(&self.img_sha256);
        b[OFF_IMG_LEN..OFF_IMG_LEN + 8].copy_from_slice(&self.img_len.to_le_bytes());
        b[OFF_FLAGS..OFF_FLAGS + 4].copy_from_slice(&self.flags.to_le_bytes());

        let crc = crc32fast::hash(&b[..SLOT_BODY]);
        b[CRC_OFF..CRC_OFF + 4].copy_from_slice(&crc.to_le_bytes());
        b
    }

    fn decode(b: &[u8]) -> Option<Self> {
        let crc = u32::from_le_bytes(b[CRC_OFF..CRC_OFF + 4].try_into().ok()?);
        if crc32fast::hash(&b[..SLOT_BODY]) != crc {
            return None;
        }
        let s = Self {
            params: Argon2Params {
                m_cost: u32::from_le_bytes(b[OFF_PARAMS..OFF_PARAMS + 4].try_into().ok()?),
                t_cost: u32::from_le_bytes(b[OFF_PARAMS + 4..OFF_PARAMS + 8].try_into().ok()?),
                p_cost: u32::from_le_bytes(b[OFF_PARAMS + 8..OFF_PARAMS + 12].try_into().ok()?),
            },
            salt: b[OFF_SALT..OFF_SALT + SALT_LEN].try_into().ok()?,
            wrapped_dek: b[OFF_WRAPPED_DEK..OFF_WRAPPED_DEK + WRAPPED_DEK_LEN]
                .try_into()
                .ok()?,
            wrap_nonce: b[OFF_WRAP_NONCE..OFF_WRAP_NONCE + WRAP_NONCE_LEN]
                .try_into()
                .ok()?,
            doc_nonce: b[OFF_DOC_NONCE..OFF_DOC_NONCE + STREAM_NONCE_LEN]
                .try_into()
                .ok()?,
            doc_len: u64::from_le_bytes(b[OFF_DOC_LEN..OFF_DOC_LEN + 8].try_into().ok()?),
            doc_sha256: b[OFF_DOC_SHA..OFF_DOC_SHA + SHA256_LEN].try_into().ok()?,
            doc_off: u64::from_le_bytes(b[OFF_DOC_OFF..OFF_DOC_OFF + 8].try_into().ok()?),
            img_nonce: b[OFF_IMG_NONCE..OFF_IMG_NONCE + STREAM_NONCE_LEN]
                .try_into()
                .ok()?,
            img_sha256: b[OFF_IMG_SHA..OFF_IMG_SHA + SHA256_LEN].try_into().ok()?,
            img_len: u64::from_le_bytes(b[OFF_IMG_LEN..OFF_IMG_LEN + 8].try_into().ok()?),
            flags: u32::from_le_bytes(b[OFF_FLAGS..OFF_FLAGS + 4].try_into().ok()?),
        };
        // 参数兜底：拒绝被手改成天文数字的 Argon2 参数
        if s.params.m_cost == 0
            || s.params.t_cost == 0
            || s.params.p_cost == 0
            || s.params.m_cost > 4 * 1024 * 1024
        {
            return None;
        }
        Some(s)
    }
}

/// 完整文件头：两个槽 + 序列号。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHeader {
    pub seqs: [u64; 2],
    pub slots: [Option<Slot>; 2],
}

impl FileHeader {
    pub fn new(slot: Slot) -> Self {
        Self {
            seqs: [1, 0],
            slots: [Some(slot), None],
        }
    }

    pub fn read(&self) -> Option<&Slot> {
        let idx = self.active_index()?;
        self.slots[idx].as_ref()
    }

    pub fn next_seq(&self) -> u64 {
        self.seqs[0].max(self.seqs[1]) + 1
    }

    pub fn active_index(&self) -> Option<usize> {
        match (&self.slots[0], &self.slots[1]) {
            (Some(_), Some(_)) => Some(if self.seqs[0] >= self.seqs[1] { 0 } else { 1 }),
            (Some(_), None) => Some(0),
            (None, Some(_)) => Some(1),
            (None, None) => None,
        }
    }

    /// 计划把 `slot` 写进备用槽。
    pub fn with_updated_slot(&self, slot: Slot) -> (Self, usize) {
        let target = match self.active_index() {
            Some(active) => 1 - active,
            None => 0,
        };
        let mut next = self.clone();
        next.seqs[target] = self.next_seq();
        next.slots[target] = Some(slot);
        (next, target)
    }

    pub fn encode(&self) -> [u8; HEADER_SIZE] {
        let mut b = [0u8; HEADER_SIZE];
        b[0..4].copy_from_slice(&MAGIC);
        b[4..6].copy_from_slice(&VERSION.to_le_bytes());
        b[8..16].copy_from_slice(&self.seqs[0].to_le_bytes());
        b[16..24].copy_from_slice(&self.seqs[1].to_le_bytes());
        if let Some(s) = &self.slots[0] {
            b[SLOT_A_OFFSET..SLOT_A_OFFSET + SLOT_SIZE].copy_from_slice(&s.encode());
        }
        if let Some(s) = &self.slots[1] {
            b[SLOT_B_OFFSET..SLOT_B_OFFSET + SLOT_SIZE].copy_from_slice(&s.encode());
        }
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, HeaderError> {
        if b.len() < HEADER_SIZE {
            return Err(HeaderError::TooShort);
        }
        if b[0..4] != MAGIC {
            return Err(HeaderError::NotOurs);
        }
        let version = u16::from_le_bytes([b[4], b[5]]);
        if version != VERSION {
            return Err(HeaderError::Version(version));
        }
        Ok(Self {
            seqs: [
                u64::from_le_bytes(b[8..16].try_into().unwrap()),
                u64::from_le_bytes(b[16..24].try_into().unwrap()),
            ],
            slots: [
                Slot::decode(&b[SLOT_A_OFFSET..SLOT_A_OFFSET + SLOT_SIZE]),
                Slot::decode(&b[SLOT_B_OFFSET..SLOT_B_OFFSET + SLOT_SIZE]),
            ],
        })
    }

    pub fn read_from<R: Read>(r: &mut R) -> std::io::Result<Result<Self, HeaderError>> {
        let mut buf = [0u8; HEADER_SIZE];
        match r.read_exact(&mut buf) {
            Ok(()) => Ok(Self::decode(&buf)),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(Err(HeaderError::TooShort)),
            Err(e) => Err(e),
        }
    }

    pub fn write_to<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        w.write_all(&self.encode())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderError {
    TooShort,
    NotOurs,
    Version(u16),
    NoValidSlot,
}

impl std::fmt::Display for HeaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort => f.write_str("文件太短，不是有效的笔记文件"),
            Self::NotOurs => f.write_str("不是 EncryptoNote 笔记文件（magic 不匹配）"),
            Self::Version(v) => write!(f, "不支持的文件版本：{v}"),
            Self::NoValidSlot => f.write_str("文件头两个槽都损坏"),
        }
    }
}

impl std::error::Error for HeaderError {}

pub fn chunk_count(plaintext_len: u64) -> u64 {
    plaintext_len.div_ceil(CHUNK_SIZE as u64).max(1)
}

pub fn ciphertext_len(plaintext_len: u64) -> u64 {
    let chunks = chunk_count(plaintext_len);
    plaintext_len + chunks * crate::stream::TAG_LEN as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot() -> Slot {
        Slot {
            params: Argon2Params::DEFAULT,
            salt: [7; SALT_LEN],
            wrapped_dek: [9; WRAPPED_DEK_LEN],
            wrap_nonce: [3; WRAP_NONCE_LEN],
            doc_nonce: [1; STREAM_NONCE_LEN],
            doc_len: 1234,
            doc_sha256: [2; SHA256_LEN],
            doc_off: 0x2000,
            img_nonce: [4; STREAM_NONCE_LEN],
            img_sha256: [5; SHA256_LEN],
            img_len: 0,
            flags: 0,
        }
    }

    #[test]
    fn slot_roundtrip() {
        let s = slot();
        assert_eq!(Slot::decode(&s.encode()), Some(s.clone()));
    }

    #[test]
    fn slot_crc_detects_damage() {
        let mut b = slot().encode();
        b[OFF_DOC_LEN] ^= 0xFF;
        assert!(Slot::decode(&b).is_none());
    }

    #[test]
    fn header_rotation_picks_newer() {
        let h = FileHeader::new(slot());
        let mut s2 = slot();
        s2.doc_len = 9999;
        let (h2, idx) = h.with_updated_slot(s2.clone());
        assert_eq!(idx, 1);
        assert_eq!(h2.read(), Some(&s2));
        // 再写一次回到槽 A
        let (h3, idx) = h2.with_updated_slot(slot());
        assert_eq!(idx, 0);
        assert_eq!(h3.read(), Some(&slot()));
        assert_eq!(h3.next_seq(), 4);
    }

    #[test]
    fn header_roundtrip_and_version_check() {
        let h = FileHeader::new(slot());
        let bytes = h.encode();
        assert_eq!(FileHeader::decode(&bytes), Ok(h));
        let mut bad = bytes;
        bad[4] = 3;
        assert_eq!(FileHeader::decode(&bad), Err(HeaderError::Version(3)));
        let mut bad = bytes;
        bad[0] = b'X';
        assert_eq!(FileHeader::decode(&bad), Err(HeaderError::NotOurs));
    }

    #[test]
    fn losing_both_slots_is_error() {
        let mut h = FileHeader::new(slot());
        h.slots[0] = None;
        assert_eq!(h.read(), None);
        assert_eq!(h.active_index(), None);
    }
}

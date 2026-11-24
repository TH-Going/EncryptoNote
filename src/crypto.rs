//! 密钥派生（密码 → KEK）与 DEK 包装。
//!
//! 设计见 `设计方案.md` §5：
//! - 全局随机 salt 与 Argon2id 参数随文件头一起保存，避免每个文件都重新跑一次 Argon2；
//! - KEK 只用来解包 DEK，不直接加密数据；
//! - 每个文件独立随机 DEK，DEK 用 XChaCha20-Poly1305 包装（24 字节 nonce 足够随机）。

use anyhow::{anyhow, bail, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use rand::{rngs::OsRng, RngCore};
use zeroize::Zeroizing;

use crate::format::{DEK_LEN, SALT_LEN, STREAM_NONCE_LEN, WRAPPED_DEK_LEN, WRAP_NONCE_LEN};

/// DEK 包装专用的 AAD，防止密文在"数据块"与"密钥块"之间被挪用
const AAD_WRAP: &[u8] = b"ECNT-KEK-wrap-v1";

/// Argon2id 参数，随文件头保存，保证老文件永远能打开。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Argon2Params {
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl Argon2Params {
    /// 256 MiB / 3 轮 / 1 并行，单次派生约 0.4–0.8 s。
    pub const DEFAULT: Self = Self {
        m_cost: 256 * 1024,
        t_cost: 3,
        p_cost: 1,
    };
}

/// 派生出来的主密钥，离开作用域自动清零
pub type Kek = Zeroizing<[u8; 32]>;

fn fill(buf: &mut [u8]) {
    OsRng.fill_bytes(buf);
}

pub fn random_salt() -> [u8; SALT_LEN] {
    let mut b = [0u8; SALT_LEN];
    fill(&mut b);
    b
}

pub fn random_dek() -> Zeroizing<[u8; DEK_LEN]> {
    let mut b = Zeroizing::new([0u8; DEK_LEN]);
    fill(b.as_mut());
    b
}

pub fn random_wrap_nonce() -> [u8; WRAP_NONCE_LEN] {
    let mut b = [0u8; WRAP_NONCE_LEN];
    fill(&mut b);
    b
}

pub fn random_stream_nonce() -> [u8; STREAM_NONCE_LEN] {
    let mut b = [0u8; STREAM_NONCE_LEN];
    fill(&mut b);
    b
}

/// 密码 → KEK。`params` 来自文件头，允许将来的文件用不同参数打开。
pub fn derive_kek(password: &str, salt: &[u8; SALT_LEN], params: &Argon2Params) -> Result<Kek> {
    let p = Params::new(params.m_cost, params.t_cost, params.p_cost, Some(32))
        .map_err(|e| anyhow!("Argon2 参数非法：{e}"))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, p);

    let mut out = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(password.as_bytes(), salt, &mut out[..])
        .map_err(|e| anyhow!("Argon2 密钥派生失败：{e}"))?;

    Ok(out)
}

/// 用 KEK 包装 DEK，返回（包装后的 DEK，nonce）
pub fn wrap_dek(
    kek: &Kek,
    dek: &[u8; DEK_LEN],
) -> Result<([u8; WRAPPED_DEK_LEN], [u8; WRAP_NONCE_LEN])> {
    let nonce = random_wrap_nonce();
    let cipher = XChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(&kek[..]));
    let ct = cipher
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: dek, aad: AAD_WRAP })
        .map_err(|_| anyhow!("DEK 包装失败"))?;

    if ct.len() != WRAPPED_DEK_LEN {
        bail!("DEK 包装结果长度异常：{}", ct.len());
    }
    let mut out = [0u8; WRAPPED_DEK_LEN];
    out.copy_from_slice(&ct);
    Ok((out, nonce))
}

/// 解开 DEK。失败只有两种可能：密码不对，或文件头被改过。
pub fn unwrap_dek(
    kek: &Kek,
    wrapped: &[u8; WRAPPED_DEK_LEN],
    nonce: &[u8; WRAP_NONCE_LEN],
) -> Option<Zeroizing<[u8; DEK_LEN]>> {
    let cipher = XChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(&kek[..]));
    let pt = cipher
        .decrypt(XNonce::from_slice(nonce), Payload { msg: wrapped, aad: AAD_WRAP })
        .ok()?;
    if pt.len() != DEK_LEN {
        return None;
    }
    let mut dek = Zeroizing::new([0u8; DEK_LEN]);
    dek.copy_from_slice(&pt);
    Some(dek)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 测试用极低强度参数，避免每次跑几十秒；真实参数见 format::Argon2Params::DEFAULT
    const FAST: Argon2Params = Argon2Params { m_cost: 8 * 1024, t_cost: 1, p_cost: 1 };

    #[test]
    fn derive_is_deterministic() {
        let salt = [7u8; SALT_LEN];
        let a = derive_kek("正确密码", &salt, &FAST).unwrap();
        let b = derive_kek("正确密码", &salt, &FAST).unwrap();
        assert_eq!(&a[..], &b[..]);
    }

    #[test]
    fn derive_differs_by_password_and_salt() {
        let s1 = [1u8; SALT_LEN];
        let s2 = [2u8; SALT_LEN];
        let a = derive_kek("密码A", &s1, &FAST).unwrap();
        let b = derive_kek("密码B", &s1, &FAST).unwrap();
        let c = derive_kek("密码A", &s2, &FAST).unwrap();
        assert_ne!(&a[..], &b[..]);
        assert_ne!(&a[..], &c[..]);
    }

    #[test]
    fn dek_wrap_roundtrip() {
        let salt = [3u8; SALT_LEN];
        let kek = derive_kek("hunter2", &salt, &FAST).unwrap();
        let dek = random_dek();
        let (wrapped, nonce) = wrap_dek(&kek, &dek).unwrap();
        let back = unwrap_dek(&kek, &wrapped, &nonce).expect("同一 KEK 必须能解开");
        assert_eq!(&back[..], &dek[..]);
    }

    #[test]
    fn wrong_password_cannot_unwrap() {
        let salt = [4u8; SALT_LEN];
        let kek = derive_kek("right", &salt, &FAST).unwrap();
        let bad = derive_kek("wrong", &salt, &FAST).unwrap();
        let dek = random_dek();
        let (wrapped, nonce) = wrap_dek(&kek, &dek).unwrap();
        assert!(unwrap_dek(&bad, &wrapped, &nonce).is_none());
    }

    #[test]
    fn tampered_wrapped_dek_rejected() {
        let salt = [5u8; SALT_LEN];
        let kek = derive_kek("pw", &salt, &FAST).unwrap();
        let dek = random_dek();
        let (mut wrapped, nonce) = wrap_dek(&kek, &dek).unwrap();
        wrapped[0] ^= 0x01;
        assert!(unwrap_dek(&kek, &wrapped, &nonce).is_none());
    }
}

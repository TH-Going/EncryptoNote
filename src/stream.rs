//! 分块 AEAD 数据流（TLS 1.3 同款 STREAM 构造）。
//!
//! 每 64 KiB 一个 ChaCha20-Poly1305 记录，最后一块用 `encrypt_last_in_place`
//! 打上"末块"标记——中途截断的密文会在解密时认证失败。
//!
//! AAD = 12 字节域分隔前缀 + 总块数 + 块序号。文档区与图片区用不同前缀，
//! 防止把图片密文挪到文档区（或反之）还能认证通过。

use std::io::{self, Read, Write};

use chacha20poly1305::{
    aead::{
        generic_array::GenericArray,
        stream::{DecryptorLE31, EncryptorLE31},
    },
    ChaCha20Poly1305, Key,
};
use sha2::{Digest, Sha256};

use crate::format::{CHUNK_SIZE, STREAM_NONCE_LEN};

pub const AAD_DOC: &[u8; 12] = b"ECNT-DOC-v01";
#[allow(dead_code)]
pub const AAD_IMG: &[u8; 12] = b"ECNT-IMG-v01";
const AAD_LEN: usize = 12 + 8 + 8;

/// Poly1305 认证标签长度
pub const TAG_LEN: usize = 16;

#[derive(Debug, Clone, Copy)]
pub struct StreamStats {
    #[allow(dead_code)]
    pub plaintext_len: u64,
    pub sha256: [u8; 32],
}

fn aad(prefix: &[u8; 12], total_chunks: u64, index: u64) -> [u8; AAD_LEN] {
    let mut a = [0u8; AAD_LEN];
    a[..12].copy_from_slice(prefix);
    a[12..20].copy_from_slice(&total_chunks.to_le_bytes());
    a[20..28].copy_from_slice(&index.to_le_bytes());
    a
}

fn auth_failed() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "认证失败：密码错误，或文件内容已被篡改",
    )
}

/// 明文流 → 密文流。返回明文长度与 SHA-256（用于写头与事后校验）。
#[allow(clippy::too_many_arguments)]
pub fn encrypt_stream<R: Read, W: Write>(
    dek: &[u8; 32],
    stream_nonce: &[u8; STREAM_NONCE_LEN],
    aad_prefix: &[u8; 12],
    plaintext_len: u64,
    total_chunks: u64,
    mut src: R,
    dst: &mut W,
) -> io::Result<StreamStats> {
    debug_assert!(total_chunks >= 1);

    let mut enc = EncryptorLE31::<ChaCha20Poly1305>::new(
        Key::from_slice(dek),
        GenericArray::from_slice(stream_nonce),
    );
    let mut hasher = Sha256::new();
    let mut buf: Vec<u8> = Vec::with_capacity(CHUNK_SIZE + TAG_LEN);
    let mut remaining = plaintext_len;

    for index in 0..total_chunks - 1 {
        let want = chunk_plain_len(remaining);
        buf.clear();
        buf.resize(want, 0);
        src.read_exact(&mut buf[..want])?;
        hasher.update(&buf);

        let a = aad(aad_prefix, total_chunks, index);
        enc.encrypt_next_in_place(&a, &mut buf)
            .map_err(|_| auth_failed())?;
        dst.write_all(&buf)?;
        remaining -= want as u64;
    }

    let want = chunk_plain_len(remaining);
    buf.clear();
    buf.resize(want, 0);
    src.read_exact(&mut buf[..want])?;
    hasher.update(&buf);
    let a = aad(aad_prefix, total_chunks, total_chunks - 1);
    enc.encrypt_last_in_place(&a, &mut buf)
        .map_err(|_| auth_failed())?;
    dst.write_all(&buf)?;

    let digest = hasher.finalize();
    let mut sha256 = [0u8; 32];
    sha256.copy_from_slice(&digest);

    Ok(StreamStats {
        plaintext_len,
        sha256,
    })
}

/// 密文流 → 明文流。任何一块认证失败立即返回错误，不写出半截明文。
#[allow(clippy::too_many_arguments)]
pub fn decrypt_stream<R: Read, W: Write>(
    dek: &[u8; 32],
    stream_nonce: &[u8; STREAM_NONCE_LEN],
    aad_prefix: &[u8; 12],
    plaintext_len: u64,
    total_chunks: u64,
    mut src: R,
    dst: &mut W,
) -> io::Result<StreamStats> {
    debug_assert!(total_chunks >= 1);

    let mut dec = DecryptorLE31::<ChaCha20Poly1305>::new(
        Key::from_slice(dek),
        GenericArray::from_slice(stream_nonce),
    );
    let mut hasher = Sha256::new();
    let mut buf: Vec<u8> = Vec::with_capacity(CHUNK_SIZE + TAG_LEN);
    let mut remaining = plaintext_len;

    for index in 0..total_chunks - 1 {
        let want = chunk_plain_len(remaining);
        read_ct(&mut src, &mut buf, want)?;
        let a = aad(aad_prefix, total_chunks, index);
        dec.decrypt_next_in_place(&a, &mut buf)
            .map_err(|_| auth_failed())?;
        hasher.update(&buf);
        dst.write_all(&buf)?;
        remaining -= want as u64;
    }

    let want = chunk_plain_len(remaining);
    read_ct(&mut src, &mut buf, want)?;
    let a = aad(aad_prefix, total_chunks, total_chunks - 1);
    dec.decrypt_last_in_place(&a, &mut buf)
        .map_err(|_| auth_failed())?;
    hasher.update(&buf);
    dst.write_all(&buf)?;

    let digest = hasher.finalize();
    let mut sha256 = [0u8; 32];
    sha256.copy_from_slice(&digest);

    Ok(StreamStats {
        plaintext_len,
        sha256,
    })
}

fn chunk_plain_len(remaining: u64) -> usize {
    remaining.min(CHUNK_SIZE as u64) as usize
}

fn read_ct<R: Read>(src: &mut R, buf: &mut Vec<u8>, plain_len: usize) -> io::Result<()> {
    let need = plain_len + TAG_LEN;
    buf.clear();
    buf.resize(need, 0);
    src.read_exact(&mut buf[..need])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(data: &[u8]) {
        let dek = [9u8; 32];
        let nonce = [1u8; STREAM_NONCE_LEN];
        let chunks = crate::format::chunk_count(data.len() as u64);

        let mut ct = Vec::new();
        let s1 = encrypt_stream(
            &dek,
            &nonce,
            AAD_DOC,
            data.len() as u64,
            chunks,
            data,
            &mut ct,
        )
        .unwrap();
        assert_eq!(
            ct.len() as u64,
            crate::format::ciphertext_len(data.len() as u64)
        );

        let mut pt = Vec::new();
        let s2 = decrypt_stream(
            &dek,
            &nonce,
            AAD_DOC,
            data.len() as u64,
            chunks,
            &ct[..],
            &mut pt,
        )
        .unwrap();
        assert_eq!(pt, data);
        assert_eq!(s1.sha256, s2.sha256);
        assert_eq!(s1.plaintext_len, data.len() as u64);
    }

    #[test]
    fn roundtrip_sizes() {
        roundtrip(b"");
        roundtrip(b"a");
        roundtrip(&vec![7u8; 1000]);
        roundtrip(&vec![7u8; CHUNK_SIZE - 1]);
        roundtrip(&vec![7u8; CHUNK_SIZE]);
        roundtrip(&vec![7u8; CHUNK_SIZE + 1]);
        roundtrip(&vec![7u8; CHUNK_SIZE * 3 + 12345]);
    }

    #[test]
    fn wrong_domain_rejected() {
        let dek = [3u8; 32];
        let nonce = [4u8; STREAM_NONCE_LEN];
        let data = vec![5u8; 100];
        let chunks = crate::format::chunk_count(data.len() as u64);
        let mut ct = Vec::new();
        encrypt_stream(&dek, &nonce, AAD_DOC, data.len() as u64, chunks, &data[..], &mut ct)
            .unwrap();
        let mut pt = Vec::new();
        assert!(
            decrypt_stream(&dek, &nonce, AAD_IMG, data.len() as u64, chunks, &ct[..], &mut pt)
                .is_err()
        );
    }

    #[test]
    fn truncation_detected() {
        let dek = [3u8; 32];
        let nonce = [4u8; STREAM_NONCE_LEN];
        let data = vec![5u8; CHUNK_SIZE * 2];
        let chunks = crate::format::chunk_count(data.len() as u64);

        let mut ct = Vec::new();
        encrypt_stream(&dek, &nonce, AAD_DOC, data.len() as u64, chunks, &data[..], &mut ct)
            .unwrap();

        let cut = ct.len() - TAG_LEN;
        let mut pt = Vec::new();
        assert!(
            decrypt_stream(
                &dek,
                &nonce,
                AAD_DOC,
                data.len() as u64,
                chunks,
                &ct[..cut],
                &mut pt
            )
            .is_err()
        );
    }
}

//! 「最近打开」列表：只记录用户真正打开/新建过的 `.ecnote`，不扫描目录。
//!
//! 存在 `%APPDATA%\EncryptoNote\recent.json`。这里只有路径、标题和时间，没有密码或密钥。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const MAX_ENTRIES: usize = 20;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecentEntry {
    pub path: PathBuf,
    pub title: String,
    #[serde(default)]
    pub size: u64,
    /// Unix 秒
    pub opened: i64,
}

#[derive(Default, Serialize, Deserialize)]
struct RecentFile {
    entries: Vec<RecentEntry>,
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn now_string() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
}

pub fn random_id() -> String {
    use rand::RngCore;
    let mut b = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("EncryptoNote")
}

fn recent_path() -> PathBuf {
    config_dir().join("recent.json")
}

fn load_file() -> RecentFile {
    std::fs::read(recent_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_file(f: &RecentFile) {
    let dir = config_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(bytes) = serde_json::to_vec_pretty(f) {
        let _ = std::fs::write(recent_path(), bytes);
    }
}

/// 文件仍存在、按打开时间倒序、最多 20 条。
pub fn list() -> Vec<RecentEntry> {
    let mut f = load_file();
    f.entries.retain(|e| e.path.is_file());
    f.entries.sort_by_key(|e| -e.opened);
    f.entries.truncate(MAX_ENTRIES);
    f.entries
}

/// 打开或新建成功后调用：去重置顶。
pub fn touch(path: &Path, title: &str, size: u64) {
    let mut f = load_file();
    let canonical = path.to_path_buf();
    f.entries.retain(|e| e.path != canonical);
    f.entries.insert(
        0,
        RecentEntry {
            path: canonical,
            title: title.to_owned(),
            size,
            opened: now_secs(),
        },
    );
    f.entries.truncate(MAX_ENTRIES);
    save_file(&f);
}

pub fn forget(path: &Path) {
    let mut f = load_file();
    f.entries.retain(|e| e.path != path);
    save_file(&f);
}

/// 「今天 10:24」「昨天 21:03」「11-21 16:40」「2025-12-01」。
pub fn display_time(unix: i64) -> String {
    use chrono::{Local, TimeZone};
    let Some(dt) = Local.timestamp_opt(unix, 0).single() else {
        return String::new();
    };
    let now = Local::now();
    let days = (now.date_naive() - dt.date_naive()).num_days();
    if days <= 0 {
        format!("今天 {}", dt.format("%H:%M"))
    } else if days == 1 {
        format!("昨天 {}", dt.format("%H:%M"))
    } else if days < 365 {
        dt.format("%m-%d %H:%M").to_string()
    } else {
        dt.format("%Y-%m-%d").to_string()
    }
}

pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{:.1} MB", b / (KB * KB))
    }
}

pub fn sanitize_file_name(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                ' '
            } else {
                c
            }
        })
        .collect();
    s = s.trim().to_owned();
    let mut chars: Vec<char> = s.chars().collect();
    while chars.last().is_some_and(|c| *c == '.' || *c == ' ') {
        chars.pop();
    }
    s = chars.into_iter().collect();
    if s.is_empty() {
        "未命名".to_owned()
    } else {
        s.chars().take(80).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_chinese() {
        assert_eq!(sanitize_file_name(" 项目:笔记/一 "), "项目 笔记 一");
        assert_eq!(sanitize_file_name("..."), "未命名");
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(900), "900 B");
        assert_eq!(human_size(2048), "2.0 KB");
    }
}

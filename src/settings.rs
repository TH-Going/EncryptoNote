//! 全局行为设置；不保存密码、密钥或笔记内容。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::recent;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "minutes", rename_all = "snake_case")]
pub enum UnlockRetention {
    Immediate,
    Minutes(u16),
    UntilExit,
}

impl UnlockRetention {
    pub fn duration(self) -> Option<Duration> {
        match self {
            Self::Immediate => Some(Duration::ZERO),
            Self::Minutes(minutes) => Some(Duration::from_secs(u64::from(minutes) * 60)),
            Self::UntilExit => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub unlock_retention: UnlockRetention,
    pub auto_save: bool,
    pub backup: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            unlock_retention: UnlockRetention::Minutes(15),
            auto_save: true,
            backup: true,
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        recent::config_dir().join("settings.json")
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .filter(|settings| settings.validate().is_ok())
            .unwrap_or_default()
    }

    fn validate(&self) -> Result<()> {
        if let UnlockRetention::Minutes(minutes) = self.unlock_retention {
            ensure!((1..=1440).contains(&minutes), "自定义时长应为 1–1440 分钟");
        }
        Ok(())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let dir = path.parent().context("设置目录无效")?;
        std::fs::create_dir_all(dir).context("无法创建设置目录")?;
        let temp = dir.join(format!(".settings-{}.tmp", recent::random_id()));
        let result = (|| -> Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            // serde_json 输出 UTF-8，不带 BOM。
            file.write_all(&serde_json::to_vec_pretty(self)?)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temp, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result.context("无法保存设置")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_roundtrip_defaults_and_invalid_values() {
        let dir = std::env::temp_dir().join(format!("ecnote-settings-{}", recent::random_id()));
        let path = dir.join("设置.json");
        assert_eq!(Settings::load(&path), Settings::default());
        for retention in [
            UnlockRetention::Immediate,
            UnlockRetention::Minutes(1),
            UnlockRetention::Minutes(1440),
            UnlockRetention::UntilExit,
        ] {
            let settings = Settings {
                unlock_retention: retention,
                auto_save: false,
                backup: false,
            };
            settings.save(&path).unwrap();
            assert_eq!(Settings::load(&path), settings);
            let bytes = std::fs::read(&path).unwrap();
            assert!(!bytes.starts_with(&[0xEF, 0xBB, 0xBF]));
            assert!(std::str::from_utf8(&bytes).is_ok());
        }
        for invalid in [0, 1441] {
            let settings = Settings {
                unlock_retention: UnlockRetention::Minutes(invalid),
                ..Settings::default()
            };
            assert!(settings.save(&path).is_err());
            std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
            assert_eq!(Settings::load(&path), Settings::default());
        }
        std::fs::write(&path, b"{").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
        std::fs::write(&path, br#"{"auto_save":false}"#).unwrap();
        assert_eq!(
            Settings::load(&path),
            Settings {
                auto_save: false,
                ..Settings::default()
            }
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_settings_write_cleans_temporary_file() {
        let dir = std::env::temp_dir().join(format!("ecnote-settings-{}", recent::random_id()));
        let path = dir.join("settings.json");
        std::fs::create_dir_all(&path).unwrap();
        assert!(Settings::default().save(&path).is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        assert!(path.is_dir());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

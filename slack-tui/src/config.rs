use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::timeutil::SoundMode;

pub struct Config {
    pub token: String,
    pub default_channel: String,
    pub default_at: String,
    pub sound: SoundMode,
    pub db_path: PathBuf,
    pub legacy_dir: PathBuf,
}

impl Config {
    pub fn load() -> Result<Self, String> {
        let home = dirs::home_dir().ok_or("无法确定主目录")?;
        let env_path = std::env::var("SLACK_ENV_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".config/grok-slack/env"));
        let cfg_path = home.join(".config/grok-slack/slack-msg.json");
        let db_path = std::env::var("SLACK_TUI_DB")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".local/state/slack-tui/slack.db"));
        let legacy_dir = std::env::var("SLACK_TUI_LEGACY")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".local/state/slack-msg"));
        Self::from_files(&env_path, &cfg_path, db_path, legacy_dir)
    }

    pub fn from_files(
        env_path: &Path,
        cfg_path: &Path,
        db_path: PathBuf,
        legacy_dir: PathBuf,
    ) -> Result<Self, String> {
        let env = read_env(env_path)?;
        let token = env
            .get("SLACK_USER_TOKEN")
            .cloned()
            .filter(|value| !value.is_empty())
            .or_else(|| env.get("SLACK_BOT_TOKEN").cloned())
            .filter(|value| !value.is_empty())
            .ok_or("没有可用的 Slack 令牌。不打印密钥文件内容。")?;

        let mut default_channel = "pr".to_string();
        let mut default_at = String::new();
        let mut sound = SoundMode::Doorbell;
        if cfg_path.is_file() {
            let content = fs::read_to_string(cfg_path)
                .map_err(|error| format!("无法读取 slack-msg.json：{error}"))?;
            let value: serde_json::Value = serde_json::from_str(&content)
                .map_err(|_| "slack-msg.json 无法解析。没有打印文件内容。".to_string())?;
            if let Some(channel) = value.get("default_channel").and_then(|item| item.as_str()) {
                if !channel.trim().is_empty() {
                    default_channel = channel.trim().trim_start_matches('#').to_string();
                }
            }
            if let Some(at) = value.get("default_at").and_then(|item| item.as_str()) {
                default_at = at.trim().trim_start_matches('@').to_string();
            }
            if let Some(name) = value.get("sound").and_then(|item| item.as_str()) {
                sound = SoundMode::parse(name);
            }
        }

        Ok(Self {
            token,
            default_channel,
            default_at,
            sound,
            db_path,
            legacy_dir,
        })
    }
}

fn read_env(path: &Path) -> Result<HashMap<String, String>, String> {
    let mut map = HashMap::new();
    if !path.is_file() {
        return Ok(map);
    }
    let content =
        fs::read_to_string(path).map_err(|_| "无法读取密钥文件。没有打印文件内容。".to_string())?;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || !line.contains('=') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"').trim_matches('\'');
        map.insert(key.trim().to_string(), value.to_string());
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_channel_and_refuses_to_echo_the_token() {
        let dir = std::env::temp_dir().join(format!("slack-tui-cfg-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("env"), "SLACK_USER_TOKEN=test-token\n").unwrap();
        fs::write(
            dir.join("slack-msg.json"),
            "{\"default_channel\":\"#ops\",\"default_at\":\"@Ada\",\"sound\":\"beep\"}",
        )
        .unwrap();
        let cfg = Config::from_files(
            &dir.join("env"),
            &dir.join("slack-msg.json"),
            dir.join("tui.db"),
            dir.join("legacy"),
        )
        .unwrap();
        assert_eq!(cfg.token, "test-token");
        assert_eq!(cfg.default_channel, "ops");
        assert_eq!(cfg.default_at, "Ada");
        assert_eq!(cfg.sound, SoundMode::Beep);
        let missing = Config::from_files(
            &dir.join("missing"),
            &dir.join("nope.json"),
            dir.join("tui.db"),
            dir.join("legacy"),
        );
        let Err(err) = missing else {
            panic!("缺少令牌时应该失败");
        };
        assert!(!err.contains("test-token"));
        let _ = fs::remove_dir_all(&dir);
    }
}

use std::time::Duration;

use reqwest::Client;
use serde_json::Value;

use crate::db::Store;
use crate::model::{RawMessage, redact};

const API: &str = "https://slack.com/api/";

#[derive(Clone)]
pub struct SlackClient {
    token: String,
    client: Client,
}

impl SlackClient {
    pub fn new(token: String) -> Result<Self, String> {
        if token.is_empty() {
            return Err("没有可用的 Slack 令牌。不打印密钥文件内容。".to_string());
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|error| format!("HTTP 客户端初始化失败：{}", redact(&error.to_string())))?;
        Ok(Self { token, client })
    }

    pub async fn auth_test(&self) -> Result<(String, String), String> {
        let data = self.call("auth.test", vec![]).await?;
        let user_id = data
            .get("user_id")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .to_string();
        let user = data
            .get("user")
            .and_then(|item| item.as_str())
            .unwrap_or("你")
            .to_string();
        if user_id.is_empty() {
            return Err("auth.test 没有返回用户。".to_string());
        }
        Ok((user_id, user))
    }

    pub async fn resolve_channel(
        &self,
        db: &Store,
        channel: &str,
    ) -> Result<(String, String), String> {
        let query = channel.trim().trim_start_matches('#');
        if query.is_empty() {
            return Err("没有频道名。".to_string());
        }
        if let Some(found) = db.find_channel(query)? {
            return Ok(found);
        }
        self.refresh_channels(db).await?;
        db.find_channel(query)?
            .ok_or_else(|| format!("找不到频道 {query}"))
    }

    pub async fn refresh_channels(&self, db: &Store) -> Result<(), String> {
        let channels = self
            .pages(
                "conversations.list",
                vec![
                    ("types".into(), "public_channel,private_channel".into()),
                    ("exclude_archived".into(), "true".into()),
                    ("limit".into(), "200".into()),
                ],
                "channels",
            )
            .await?;
        for channel in channels {
            let id = channel
                .get("id")
                .and_then(|item| item.as_str())
                .unwrap_or("");
            let name = channel
                .get("name")
                .and_then(|item| item.as_str())
                .unwrap_or("");
            db.save_channel(id, name)?;
        }
        Ok(())
    }

    pub async fn user_id(&self, db: &Store, who: &str) -> Result<String, String> {
        if let Some(id) = db.find_user_id(who)? {
            return Ok(id);
        }
        self.refresh_users(db).await?;
        db.find_user_id(who)?
            .ok_or_else(|| format!("找不到这个人：{}", who.trim()))
    }

    pub async fn refresh_users(&self, db: &Store) -> Result<(), String> {
        let members = self
            .pages(
                "users.list",
                vec![("limit".into(), "200".into())],
                "members",
            )
            .await?;
        for member in members {
            let Some(id) = member.get("id").and_then(|item| item.as_str()) else {
                continue;
            };
            let profile = member.get("profile");
            let name = member
                .get("name")
                .and_then(|item| item.as_str())
                .unwrap_or("");
            let real_name = member
                .get("real_name")
                .and_then(|item| item.as_str())
                .or_else(|| {
                    profile
                        .and_then(|item| item.get("real_name"))
                        .and_then(|item| item.as_str())
                })
                .unwrap_or("");
            let display_name = profile
                .and_then(|item| item.get("display_name"))
                .and_then(|item| item.as_str())
                .unwrap_or("");
            let deleted = member
                .get("deleted")
                .and_then(|item| item.as_bool())
                .unwrap_or(false);
            db.save_user(id, name, real_name, display_name, deleted)?;
        }
        Ok(())
    }

    pub async fn history(
        &self,
        channel_id: &str,
        oldest: &str,
        limit: usize,
        scan_cap: usize,
    ) -> Result<(Vec<RawMessage>, usize), String> {
        let mut collected = Vec::new();
        let mut scanned = 0;
        let mut cursor = String::new();
        while collected.len() < limit && scanned < scan_cap {
            let mut params = vec![
                ("channel".to_string(), channel_id.to_string()),
                ("limit".to_string(), "100".to_string()),
            ];
            if !oldest.is_empty() {
                params.push(("oldest".to_string(), oldest.to_string()));
            }
            if !cursor.is_empty() {
                params.push(("cursor".to_string(), cursor.clone()));
            }
            let data = self.call("conversations.history", params).await?;
            let batch = data
                .get("messages")
                .and_then(|item| item.as_array())
                .cloned()
                .unwrap_or_default();
            if batch.is_empty() {
                break;
            }
            for item in batch {
                scanned += 1;
                if let Some(message) = parse_message(&item) {
                    collected.push(message);
                }
                if collected.len() >= limit || scanned >= scan_cap {
                    break;
                }
            }
            cursor = data
                .get("response_metadata")
                .and_then(|item| item.get("next_cursor"))
                .and_then(|item| item.as_str())
                .unwrap_or("")
                .to_string();
            if cursor.is_empty() {
                break;
            }
        }
        Ok((collected, scanned))
    }

    pub async fn post_message(
        &self,
        channel_id: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> Result<String, String> {
        let mut params = vec![
            ("channel".to_string(), channel_id.to_string()),
            ("text".to_string(), text.to_string()),
        ];
        if let Some(thread) = thread_ts.filter(|thread| !thread.is_empty()) {
            params.push(("thread_ts".to_string(), thread.to_string()));
        }
        let data = self.call("chat.postMessage", params).await?;
        data.get("ts")
            .and_then(|item| item.as_str())
            .filter(|ts| !ts.is_empty())
            .map(str::to_string)
            .ok_or_else(|| "chat.postMessage 没有返回 ts。".to_string())
    }

    async fn pages(
        &self,
        method: &str,
        base: Vec<(String, String)>,
        key: &str,
    ) -> Result<Vec<Value>, String> {
        let mut items = Vec::new();
        let mut cursor = String::new();
        for _ in 0..8 {
            let mut params = base.clone();
            if !cursor.is_empty() {
                params.push(("cursor".to_string(), cursor.clone()));
            }
            let data = self.call(method, params).await?;
            if let Some(batch) = data.get(key).and_then(|item| item.as_array()) {
                items.extend(batch.clone());
            }
            cursor = data
                .get("response_metadata")
                .and_then(|item| item.get("next_cursor"))
                .and_then(|item| item.as_str())
                .unwrap_or("")
                .to_string();
            if cursor.is_empty() {
                break;
            }
        }
        Ok(items)
    }

    async fn call(&self, method: &str, params: Vec<(String, String)>) -> Result<Value, String> {
        let response = self
            .client
            .post(format!("{API}{method}"))
            .bearer_auth(&self.token)
            .form(&params)
            .send()
            .await
            .map_err(|error| redact(&format!("网络错误：{error}")))?;
        let data: Value = response
            .json()
            .await
            .map_err(|error| redact(&format!("响应解析失败：{error}")))?;
        if data.get("ok").and_then(|item| item.as_bool()) == Some(true) {
            return Ok(data);
        }
        let reason = data
            .get("error")
            .and_then(|item| item.as_str())
            .unwrap_or("unknown");
        Err(format!("{method}：{reason}"))
    }
}

pub fn parse_message(value: &Value) -> Option<RawMessage> {
    let subtype = value
        .get("subtype")
        .and_then(|item| item.as_str())
        .unwrap_or("");
    if matches!(subtype, "message_changed" | "message_deleted") {
        return None;
    }
    let ts = value
        .get("ts")
        .and_then(|item| item.as_str())
        .filter(|ts| !ts.is_empty())?;
    let user_id = value
        .get("user")
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .to_string();
    let thread_ts = value
        .get("thread_ts")
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .to_string();
    Some(RawMessage {
        ts: ts.to_string(),
        user_id,
        fallback_name: fallback_name(value),
        thread_ts,
        text: message_text(value),
    })
}

fn fallback_name(value: &Value) -> String {
    let username = value
        .get("username")
        .and_then(|item| item.as_str())
        .unwrap_or("");
    if !username.is_empty() {
        return username.to_string();
    }
    let user = value
        .get("user")
        .and_then(|item| item.as_str())
        .unwrap_or("");
    if !user.is_empty() {
        return user.to_string();
    }
    value
        .get("subtype")
        .and_then(|item| item.as_str())
        .filter(|item| !item.is_empty())
        .or_else(|| value.get("bot_id").and_then(|item| item.as_str()))
        .unwrap_or("系统")
        .to_string()
}

fn message_text(value: &Value) -> String {
    let mut text = value
        .get("text")
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .replace('\n', " ");
    if text.trim().is_empty() {
        if let Some(files) = value.get("files").and_then(|item| item.as_array()) {
            let names: Vec<&str> = files
                .iter()
                .filter_map(|file| file.get("name").and_then(|item| item.as_str()))
                .collect();
            if !names.is_empty() {
                text = format!("[文件] {}", names.join(", "));
            }
        }
    }
    if text.trim().is_empty() {
        if let Some(attachments) = value.get("attachments").and_then(|item| item.as_array()) {
            let bits: Vec<&str> = attachments
                .iter()
                .filter_map(|item| {
                    item.get("fallback")
                        .or_else(|| item.get("text"))
                        .and_then(|item| item.as_str())
                })
                .collect();
            if !bits.is_empty() {
                text = bits.join(" ");
            }
        }
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        "[没有文本]".to_string()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_a_normal_message_and_a_file() {
        let message = parse_message(&json!({
            "type": "message",
            "user": "U1",
            "text": "hello\nworld",
            "ts": "10.5",
            "thread_ts": "10.0"
        }))
        .unwrap();
        assert_eq!(message.text, "hello world");
        assert_eq!(message.thread_ts, "10.0");

        let file = parse_message(&json!({
            "type": "message",
            "user": "U1",
            "text": "",
            "ts": "11",
            "files": [{"name": "a.png"}]
        }))
        .unwrap();
        assert_eq!(file.text, "[文件] a.png");
        assert!(parse_message(&json!({"subtype": "message_changed", "ts": "12"})).is_none());
    }
}

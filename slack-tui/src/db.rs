use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, params};
use serde::Deserialize;

use crate::model::Message;

const USER_REFRESH_SECS: u64 = 6 * 60 * 60;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS messages (
    channel_id TEXT NOT NULL,
    ts TEXT NOT NULL,
    channel_name TEXT NOT NULL,
    user_id TEXT NOT NULL DEFAULT '',
    user_name TEXT NOT NULL DEFAULT '',
    thread_ts TEXT NOT NULL DEFAULT '',
    text TEXT NOT NULL,
    kind TEXT NOT NULL DEFAULT '',
    local_reply INTEGER NOT NULL DEFAULT 0,
    reply_to TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (channel_id, ts)
);
CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    real_name TEXT NOT NULL DEFAULT '',
    display_name TEXT NOT NULL DEFAULT '',
    deleted INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS channels (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Deserialize)]
struct LegacyMessage {
    channel: Option<String>,
    channel_id: Option<String>,
    ts: Option<String>,
    user_id: Option<String>,
    user: Option<String>,
    thread_ts: Option<String>,
    text: Option<String>,
    kind: Option<String>,
    local_reply: Option<bool>,
    reply_to: Option<String>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("无法创建数据库目录：{error}"))?;
        }
        let conn =
            Connection::open(path).map_err(|error| format!("无法打开本地数据库：{error}"))?;
        init(&conn)?;
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn open_in_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory().map_err(|error| error.to_string())?;
        init(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T, String>) -> Result<T, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| "本地数据库锁已损坏".to_string())?;
        f(&conn)
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>, String> {
        self.with(|conn| {
            let mut stmt = conn
                .prepare("SELECT value FROM meta WHERE key = ?1")
                .map_err(|error| error.to_string())?;
            let mut rows = stmt
                .query(params![key])
                .map_err(|error| error.to_string())?;
            match rows.next().map_err(|error| error.to_string())? {
                Some(row) => Ok(Some(row.get(0).map_err(|error| error.to_string())?)),
                None => Ok(None),
            }
        })
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), String> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|error| error.to_string())?;
            Ok(())
        })
    }

    pub fn clear_meta(&self, key: &str) -> Result<(), String> {
        self.with(|conn| {
            conn.execute("DELETE FROM meta WHERE key = ?1", params![key])
                .map_err(|error| error.to_string())?;
            Ok(())
        })
    }

    /// Insert a message. Returns true only when this timestamp was not stored before.
    pub fn put_message(&self, message: &Message) -> Result<bool, String> {
        self.with(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT channel_id FROM messages
                     WHERE ts = ?1 AND (channel_id = ?2 OR channel_name = ?3)",
                )
                .map_err(|error| error.to_string())?;
            let existing: Vec<String> = stmt
                .query_map(params![message.ts, message.channel_id, message.channel_name], |row| row.get(0))
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            drop(stmt);
            if existing.iter().any(|id| id == &message.channel_id) {
                return Ok(false);
            }
            for id in &existing {
                conn.execute("DELETE FROM messages WHERE channel_id = ?1 AND ts = ?2", params![id, message.ts])
                    .map_err(|error| error.to_string())?;
            }
            conn.execute(
                "INSERT INTO messages (
                    channel_id, ts, channel_name, user_id, user_name, thread_ts, text, kind, local_reply, reply_to
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    message.channel_id,
                    message.ts,
                    message.channel_name,
                    message.user_id,
                    message.user_name,
                    message.thread_ts,
                    message.text,
                    message.kind,
                    if message.local_reply { 1 } else { 0 },
                    message.reply_to,
                ],
            )
            .map_err(|error| error.to_string())?;
            Ok(existing.is_empty())
        })
    }

    pub fn recent(
        &self,
        channel_id: &str,
        channel_name: &str,
        limit: usize,
    ) -> Result<Vec<Message>, String> {
        self.with(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT channel_id, ts, channel_name, user_id, user_name, thread_ts, text, kind, local_reply, reply_to
                     FROM messages
                     WHERE channel_id = ?1 OR channel_name = ?2
                     ORDER BY CAST(ts AS REAL) DESC
                     LIMIT ?3",
                )
                .map_err(|error| error.to_string())?;
            let rows = stmt
                .query_map(params![channel_id, channel_name, limit as i64], row_message)
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            let mut rows = rows;
            rows.reverse();
            Ok(rows)
        })
    }

    /// Newest thread root in the channel. A message with no `thread_ts` is its own root.
    pub fn recent_thread_roots(
        &self,
        channel_id: &str,
        channel_name: &str,
        limit: usize,
    ) -> Result<Vec<String>, String> {
        self.with(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT CASE WHEN thread_ts != '' THEN thread_ts ELSE ts END AS root
                     FROM messages
                     WHERE channel_id = ?1 OR channel_name = ?2
                     GROUP BY root
                     ORDER BY MAX(CAST(ts AS REAL)) DESC
                     LIMIT ?3",
                )
                .map_err(|error| error.to_string())?;
            let rows = stmt
                .query_map(params![channel_id, channel_name, limit as i64], |row| {
                    row.get(0)
                })
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<String>, _>>()
                .map_err(|error| error.to_string())?;
            Ok(rows)
        })
    }

    pub fn users_stale(&self) -> Result<bool, String> {
        let Some(value) = self.meta("users_refreshed_at")? else {
            return Ok(true);
        };
        let saved = value.parse::<u64>().unwrap_or(0);
        Ok(unix_secs().saturating_sub(saved) > USER_REFRESH_SECS)
    }

    pub fn mark_users_refreshed(&self) -> Result<(), String> {
        self.set_meta("users_refreshed_at", &unix_secs().to_string())
    }

    pub fn newest_ts(
        &self,
        channel_id: &str,
        channel_name: &str,
    ) -> Result<Option<String>, String> {
        self.with(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT ts FROM messages
                     WHERE channel_id = ?1 OR channel_name = ?2
                     ORDER BY CAST(ts AS REAL) DESC
                     LIMIT 1",
                )
                .map_err(|error| error.to_string())?;
            let mut rows = stmt
                .query(params![channel_id, channel_name])
                .map_err(|error| error.to_string())?;
            match rows.next().map_err(|error| error.to_string())? {
                Some(row) => Ok(Some(row.get(0).map_err(|error| error.to_string())?)),
                None => Ok(None),
            }
        })
    }

    pub fn save_user(
        &self,
        id: &str,
        name: &str,
        real_name: &str,
        display_name: &str,
        deleted: bool,
    ) -> Result<(), String> {
        if id.is_empty() {
            return Ok(());
        }
        self.with(|conn| {
            conn.execute(
                "INSERT INTO users (id, name, real_name, display_name, deleted)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name,
                    real_name = excluded.real_name,
                    display_name = excluded.display_name,
                    deleted = excluded.deleted",
                params![
                    id,
                    name,
                    real_name,
                    display_name,
                    if deleted { 1 } else { 0 }
                ],
            )
            .map_err(|error| error.to_string())?;
            Ok(())
        })
    }

    pub fn user_name(&self, id: &str) -> Result<Option<String>, String> {
        if id.is_empty() {
            return Ok(None);
        }
        self.with(|conn| {
            let mut stmt = conn
                .prepare("SELECT display_name, real_name, name FROM users WHERE id = ?1")
                .map_err(|error| error.to_string())?;
            let mut rows = stmt.query(params![id]).map_err(|error| error.to_string())?;
            let Some(row) = rows.next().map_err(|error| error.to_string())? else {
                return Ok(None);
            };
            let display: String = row.get(0).map_err(|error| error.to_string())?;
            let real: String = row.get(1).map_err(|error| error.to_string())?;
            let name: String = row.get(2).map_err(|error| error.to_string())?;
            Ok(Some(
                first_name(&[&display, &real, &name]).unwrap_or_else(|| id.to_string()),
            ))
        })
    }

    pub fn user_names(&self) -> Result<HashMap<String, String>, String> {
        self.with(|conn| {
            let mut stmt = conn
                .prepare("SELECT id, display_name, real_name, name FROM users")
                .map_err(|error| error.to_string())?;
            let rows = stmt
                .query_map([], |row| {
                    let id: String = row.get(0)?;
                    let display: String = row.get(1)?;
                    let real: String = row.get(2)?;
                    let name: String = row.get(3)?;
                    Ok((id, display, real, name))
                })
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            let mut names = HashMap::new();
            for (id, display, real, name) in rows {
                names.insert(
                    id.clone(),
                    first_name(&[&display, &real, &name]).unwrap_or(id),
                );
            }
            Ok(names)
        })
    }

    pub fn find_user_id(&self, query: &str) -> Result<Option<String>, String> {
        let raw = query.trim().trim_start_matches('@');
        if crate::model::looks_like_user_id(raw) {
            return Ok(Some(raw.to_string()));
        }
        let needle = raw.to_lowercase();
        if needle.is_empty() {
            return Ok(None);
        }
        self.with(|conn| {
            let mut stmt = conn
                .prepare("SELECT id, name, real_name, display_name, deleted FROM users")
                .map_err(|error| error.to_string())?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            for (id, name, real_name, display_name, deleted) in rows {
                if deleted != 0 {
                    continue;
                }
                if [name, real_name, display_name]
                    .iter()
                    .any(|item| item.to_lowercase() == needle)
                {
                    return Ok(Some(id));
                }
            }
            Ok(None)
        })
    }

    pub fn save_channel(&self, id: &str, name: &str) -> Result<(), String> {
        if id.is_empty() || name.is_empty() {
            return Ok(());
        }
        self.with(|conn| {
            conn.execute(
                "INSERT INTO channels (id, name) VALUES (?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET name = excluded.name",
                params![id, name],
            )
            .map_err(|error| error.to_string())?;
            Ok(())
        })
    }

    pub fn find_channel(&self, query: &str) -> Result<Option<(String, String)>, String> {
        let query = query.trim().trim_start_matches('#');
        if query.is_empty() {
            return Ok(None);
        }
        self.with(|conn| {
            let mut stmt = conn
                .prepare("SELECT id, name FROM channels")
                .map_err(|error| error.to_string())?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            let found = rows
                .into_iter()
                .find(|(id, name)| id == query || name.eq_ignore_ascii_case(query));
            Ok(found)
        })
    }

    pub fn import_legacy(&self, dir: &Path) -> Result<usize, String> {
        self.with(|conn| {
            if meta_get(conn, "imported_legacy")?.as_deref() == Some("1") {
                return Ok(0);
            }
            let messages = dir.join("mentions.jsonl");
            let names = dir.join("names.json");
            let channels = dir.join("channels.json");
            if !messages.is_file() && !names.is_file() && !channels.is_file() {
                return Ok(0);
            }
            let tx = conn
                .unchecked_transaction()
                .map_err(|error| error.to_string())?;
            if names.is_file() {
                import_names(&tx, &names)?;
            }
            if channels.is_file() {
                import_channels(&tx, &channels)?;
            }
            let count = if messages.is_file() {
                import_jsonl(&tx, &messages)?
            } else {
                0
            };
            tx.execute(
                "INSERT INTO meta (key, value) VALUES ('imported_legacy', '1')
                 ON CONFLICT(key) DO UPDATE SET value = '1'",
                [],
            )
            .map_err(|error| error.to_string())?;
            tx.commit().map_err(|error| error.to_string())?;
            Ok(count)
        })
    }
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn init(conn: &Connection) -> Result<(), String> {
    conn.busy_timeout(std::time::Duration::from_secs(3))
        .map_err(|error| error.to_string())?;
    conn.execute_batch(SCHEMA)
        .map_err(|error| format!("无法初始化本地数据库：{error}"))?;
    Ok(())
}

fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>, String> {
    let mut stmt = conn
        .prepare("SELECT value FROM meta WHERE key = ?1")
        .map_err(|error| error.to_string())?;
    let mut rows = stmt
        .query(params![key])
        .map_err(|error| error.to_string())?;
    match rows.next().map_err(|error| error.to_string())? {
        Some(row) => Ok(Some(row.get(0).map_err(|error| error.to_string())?)),
        None => Ok(None),
    }
}

fn first_name(candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .find(|item| !item.trim().is_empty())
        .map(|item| item.trim().to_string())
}

fn row_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    Ok(Message {
        channel_id: row.get(0)?,
        ts: row.get(1)?,
        channel_name: row.get(2)?,
        user_id: row.get(3)?,
        user_name: row.get(4)?,
        thread_ts: row.get(5)?,
        text: row.get(6)?,
        kind: row.get(7)?,
        local_reply: row.get::<_, i64>(8)? != 0,
        reply_to: row.get(9)?,
    })
}

fn import_names(conn: &Connection, path: &Path) -> Result<(), String> {
    let content = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let map: HashMap<String, String> =
        serde_json::from_str(&content).map_err(|_| "names.json 无法解析".to_string())?;
    for (id, name) in map {
        conn.execute(
            "INSERT INTO users (id, name, real_name, display_name, deleted) VALUES (?1, '', '', ?2, 0)
             ON CONFLICT(id) DO UPDATE SET display_name = excluded.display_name",
            params![id, name],
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn import_channels(conn: &Connection, path: &Path) -> Result<(), String> {
    let content = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let map: HashMap<String, serde_json::Value> =
        serde_json::from_str(&content).map_err(|_| "channels.json 无法解析".to_string())?;
    for (_key, value) in map {
        let id = value.get("id").and_then(|item| item.as_str()).unwrap_or("");
        let name = value
            .get("name")
            .and_then(|item| item.as_str())
            .unwrap_or("");
        if id.is_empty() || name.is_empty() {
            continue;
        }
        conn.execute(
            "INSERT INTO channels (id, name) VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name",
            params![id, name],
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn import_jsonl(conn: &Connection, path: &Path) -> Result<usize, String> {
    let content = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut count = 0;
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(item) = serde_json::from_str::<LegacyMessage>(line) else {
            continue;
        };
        let Some(ts) = item.ts.filter(|ts| !ts.is_empty()) else {
            continue;
        };
        let channel_name = item
            .channel
            .unwrap_or_default()
            .trim()
            .trim_start_matches('#')
            .to_string();
        let channel_id = item
            .channel_id
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| channel_name.clone());
        if channel_id.is_empty() {
            continue;
        }
        let changed = conn
            .execute(
                "INSERT OR IGNORE INTO messages (
                    channel_id, ts, channel_name, user_id, user_name, thread_ts, text, kind, local_reply, reply_to
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    channel_id,
                    ts,
                    channel_name,
                    item.user_id.unwrap_or_default(),
                    item.user.unwrap_or_default(),
                    item.thread_ts.unwrap_or_default(),
                    item.text.unwrap_or_default(),
                    item.kind.unwrap_or_default(),
                    if item.local_reply.unwrap_or(false) { 1 } else { 0 },
                    item.reply_to.unwrap_or_default(),
                ],
            )
            .map_err(|error| error.to_string())?;
        count += changed;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(channel_id: &str, ts: &str, text: &str) -> Message {
        Message {
            channel_id: channel_id.to_string(),
            channel_name: "pr".to_string(),
            ts: ts.to_string(),
            user_id: "U1".to_string(),
            user_name: "Ada".to_string(),
            thread_ts: String::new(),
            text: text.to_string(),
            kind: "#pr".to_string(),
            local_reply: false,
            reply_to: String::new(),
        }
    }

    #[test]
    fn stores_messages_in_time_order_and_dedupes() {
        let db = Store::open_in_memory().unwrap();
        assert!(db.put_message(&sample("C1", "2", "later")).unwrap());
        assert!(db.put_message(&sample("C1", "1", "earlier")).unwrap());
        assert!(!db.put_message(&sample("C1", "1", "again")).unwrap());
        let rows = db.recent("C1", "pr", 10).unwrap();
        assert_eq!(
            rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>(),
            ["earlier", "later"]
        );
        assert_eq!(db.newest_ts("C1", "pr").unwrap().as_deref(), Some("2"));
    }

    #[test]
    fn replaces_a_legacy_row_without_counting_it_as_new() {
        let db = Store::open_in_memory().unwrap();
        assert!(db.put_message(&sample("pr", "5", "hi")).unwrap());
        let mut live = sample("C1", "5", "hi");
        live.channel_name = "pr".to_string();
        assert!(!db.put_message(&live).unwrap());
        let rows = db.recent("C1", "pr", 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].channel_id, "C1");
    }

    #[test]
    fn lists_thread_roots_by_the_newest_message() {
        let db = Store::open_in_memory().unwrap();
        db.put_message(&sample("C1", "10", "root")).unwrap();
        let mut child = sample("C1", "12", "reply");
        child.thread_ts = "10".into();
        db.put_message(&child).unwrap();
        db.put_message(&sample("C1", "30", "plain")).unwrap();
        assert_eq!(
            db.recent_thread_roots("C1", "pr", 8).unwrap(),
            vec!["30".to_string(), "10".to_string()]
        );
    }

    #[test]
    fn refreshes_the_user_directory_after_six_hours() {
        let db = Store::open_in_memory().unwrap();
        assert!(db.users_stale().unwrap());
        db.set_meta("users_refreshed_at", "1").unwrap();
        assert!(db.users_stale().unwrap());
        db.mark_users_refreshed().unwrap();
        assert!(!db.users_stale().unwrap());
    }

    #[test]
    fn imports_legacy_files_once() {
        let dir = std::env::temp_dir().join(format!(
            "slack-tui-db-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("mentions.jsonl"),
            "{\"channel\":\"pr\",\"channel_id\":\"C1\",\"ts\":\"10.1\",\"user_id\":\"U1\",\"user\":\"Ada\",\"text\":\"hi\",\"kind\":\"@你\"}\nnot-json\n",
        )
        .unwrap();
        fs::write(dir.join("names.json"), "{\"U1\":\"Ada\"}").unwrap();
        fs::write(
            dir.join("channels.json"),
            "{\"pr\":{\"id\":\"C1\",\"name\":\"pr\"}}",
        )
        .unwrap();
        let db = Store::open_in_memory().unwrap();
        assert_eq!(db.import_legacy(&dir).unwrap(), 1);
        assert_eq!(db.import_legacy(&dir).unwrap(), 0);
        assert_eq!(db.user_name("U1").unwrap().as_deref(), Some("Ada"));
        assert_eq!(db.find_channel("pr").unwrap().unwrap().0, "C1");
        let _ = fs::remove_dir_all(&dir);
    }
}

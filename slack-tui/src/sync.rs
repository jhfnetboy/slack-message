use std::cmp::Ordering;
use std::collections::HashSet;

use crate::db::Store;
use crate::model::{
    GapMark, Message, RawMessage, build_message, cmp_ts, compose_text, mention_matches, min_ts,
    next_gap, thread_root,
};
use crate::slack::SlackClient;

const INITIAL_CAP: usize = 80;
const HEAD_CAP: usize = 200;
const FILL_CAP: usize = 200;
const THREAD_ROOTS: usize = 8;

#[derive(Clone, Debug)]
pub struct SyncOut {
    pub inserted: Vec<Message>,
    pub scanned: usize,
    /// True when this call filled the channel for the first time.
    pub initial: bool,
    /// True when an earlier page is still missing. The next scan fills one more page.
    pub catching_up: bool,
}

pub async fn sync_channel(
    slack: &SlackClient,
    db: &Store,
    channel_id: &str,
    channel_name: &str,
    my_id: &str,
) -> Result<SyncOut, String> {
    let backfill_key = format!("backfill:{channel_id}");
    let gap_key = format!("gap:{channel_id}");
    let initial = db.meta(&backfill_key)?.as_deref() != Some("1");
    if initial {
        let page = slack.history_range(channel_id, "", "", INITIAL_CAP).await?;
        let raw_roots = roots_in(&page.messages);
        let messages = materialize(
            slack,
            db,
            channel_id,
            channel_name,
            my_id,
            page.messages,
            false,
            "",
        )
        .await?;
        let mut inserted = store_new(db, messages)?;
        db.set_meta(&backfill_key, "1")?;
        inserted.extend(pull_roots(slack, db, channel_id, channel_name, my_id, raw_roots).await);
        return Ok(SyncOut {
            inserted,
            scanned: page.scanned,
            initial: true,
            catching_up: false,
        });
    }

    let watermark = db.newest_ts(channel_id, channel_name)?.unwrap_or_default();
    let head = slack
        .history_range(channel_id, &watermark, "", HEAD_CAP)
        .await?;
    let (saved_floor, saved_edge) = decode_gap(&db.meta(&gap_key)?.unwrap_or_default());
    let (fill_messages, fill_scanned, fill_min, fill_ran, fill_truncated) =
        fill_gap(slack, channel_id, &saved_floor, &saved_edge).await?;
    let head_min = min_ts(head.messages.iter().map(|message| message.ts.as_str()));
    let gap = next_gap(
        &watermark,
        &saved_floor,
        &saved_edge,
        head_min,
        head.truncated,
        fill_min.as_deref(),
        fill_ran,
        fill_truncated,
    );
    if gap.edge.is_empty() {
        db.clear_meta(&gap_key)?;
    } else {
        db.set_meta(&gap_key, &encode_gap(&gap))?;
    }

    let mut raw_roots = roots_in(&head.messages);
    raw_roots.extend(roots_in(&fill_messages));
    let head_messages = materialize(
        slack,
        db,
        channel_id,
        channel_name,
        my_id,
        head.messages,
        false,
        "",
    )
    .await?;
    let stored_fill = materialize(
        slack,
        db,
        channel_id,
        channel_name,
        my_id,
        fill_messages,
        false,
        "",
    )
    .await?;
    let mut inserted = store_new(db, head_messages)?;
    let _older = store_new(db, stored_fill)?;
    inserted.extend(pull_roots(slack, db, channel_id, channel_name, my_id, raw_roots).await);
    Ok(SyncOut {
        inserted,
        scanned: head.scanned + fill_scanned,
        initial: false,
        catching_up: !gap.edge.is_empty(),
    })
}

pub async fn sync_thread(
    slack: &SlackClient,
    db: &Store,
    channel_id: &str,
    channel_name: &str,
    my_id: &str,
    thread_ts: &str,
) -> Result<Vec<Message>, String> {
    let raw = slack.replies(channel_id, thread_ts).await?;
    let messages = materialize(slack, db, channel_id, channel_name, my_id, raw, false, "").await?;
    store_new(db, messages)
}

pub async fn fetch_matching(
    slack: &SlackClient,
    db: &Store,
    channel_id: &str,
    channel_name: &str,
    my_id: &str,
    mention: &str,
    wanted_id: &str,
    limit: usize,
) -> Result<(Vec<Message>, usize), String> {
    let (raw, scanned) = slack.history(channel_id, "", 200, 200).await?;
    let matched: Vec<RawMessage> = raw
        .into_iter()
        .filter(|item| mention_matches(&item.text, mention, my_id, wanted_id))
        .take(limit)
        .collect();
    let messages = materialize(
        slack,
        db,
        channel_id,
        channel_name,
        my_id,
        matched,
        false,
        "",
    )
    .await?;
    for message in &messages {
        let _ = db.put_message(message)?;
    }
    Ok((messages, scanned))
}

pub async fn send_text(
    slack: &SlackClient,
    db: &Store,
    channel_id: &str,
    channel_name: &str,
    my_id: &str,
    my_name: &str,
    body: &str,
    mention_ids: &[String],
    thread_ts: Option<&str>,
) -> Result<Message, String> {
    let posted = compose_text(body, mention_ids, my_id)?;
    let ts = slack.post_message(channel_id, &posted, thread_ts).await?;
    let raw = RawMessage {
        ts,
        user_id: my_id.to_string(),
        fallback_name: my_name.to_string(),
        thread_ts: thread_ts.unwrap_or("").to_string(),
        text: posted,
        reply_count: 0,
    };
    let message = build_message(
        &raw,
        channel_id,
        channel_name,
        my_id,
        my_name,
        true,
        thread_ts.unwrap_or(""),
    );
    let _ = db.put_message(&message)?;
    Ok(message)
}

async fn fill_gap(
    slack: &SlackClient,
    channel_id: &str,
    saved_floor: &str,
    saved_edge: &str,
) -> Result<(Vec<RawMessage>, usize, Option<String>, bool, bool), String> {
    if saved_edge.is_empty() {
        return Ok((Vec::new(), 0, None, false, false));
    }
    let oldest = if !saved_floor.is_empty() && cmp_ts(saved_floor, saved_edge) == Ordering::Less {
        saved_floor
    } else {
        ""
    };
    let page = slack
        .history_range(channel_id, oldest, saved_edge, FILL_CAP)
        .await?;
    let fill_min =
        min_ts(page.messages.iter().map(|message| message.ts.as_str())).map(str::to_string);
    Ok((page.messages, page.scanned, fill_min, true, page.truncated))
}

async fn pull_roots(
    slack: &SlackClient,
    db: &Store,
    channel_id: &str,
    channel_name: &str,
    my_id: &str,
    mut roots: Vec<String>,
) -> Vec<Message> {
    if let Ok(saved) = db.recent_thread_roots(channel_id, channel_name, THREAD_ROOTS) {
        roots.extend(saved);
    }
    let mut seen = HashSet::new();
    let mut chosen = Vec::new();
    for root in roots {
        if root.is_empty() || !seen.insert(root.clone()) {
            continue;
        }
        chosen.push(root);
        if chosen.len() >= THREAD_ROOTS {
            break;
        }
    }
    let mut inserted = Vec::new();
    for root in chosen {
        match sync_thread(slack, db, channel_id, channel_name, my_id, &root).await {
            Ok(messages) => inserted.extend(messages),
            Err(_) => {}
        }
    }
    inserted
}

fn roots_in(raws: &[RawMessage]) -> Vec<String> {
    let mut roots = Vec::new();
    for raw in raws {
        if raw.reply_count == 0 && raw.thread_ts.is_empty() {
            continue;
        }
        let root = thread_root(&raw.thread_ts, &raw.ts);
        if !root.is_empty() {
            roots.push(root.to_string());
        }
    }
    roots
}

fn store_new(db: &Store, messages: Vec<Message>) -> Result<Vec<Message>, String> {
    let mut inserted = Vec::new();
    for message in messages {
        if db.put_message(&message)? {
            inserted.push(message);
        }
    }
    Ok(inserted)
}

fn encode_gap(mark: &GapMark) -> String {
    if mark.edge.is_empty() {
        String::new()
    } else if mark.floor.is_empty() {
        mark.edge.clone()
    } else {
        format!("{} {}", mark.floor, mark.edge)
    }
}

fn decode_gap(value: &str) -> (String, String) {
    let mut parts = value.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some(floor), Some(edge)) => (floor.to_string(), edge.to_string()),
        (Some(edge), None) => (String::new(), edge.to_string()),
        _ => (String::new(), String::new()),
    }
}

async fn materialize(
    slack: &SlackClient,
    db: &Store,
    channel_id: &str,
    channel_name: &str,
    my_id: &str,
    raws: Vec<RawMessage>,
    local_reply: bool,
    reply_to: &str,
) -> Result<Vec<Message>, String> {
    let mut missing = false;
    for raw in &raws {
        if !raw.user_id.is_empty() && db.user_name(&raw.user_id)?.is_none() {
            missing = true;
            break;
        }
    }
    if missing && db.users_stale()? {
        slack.refresh_users(db).await?;
    }
    let mut messages = Vec::with_capacity(raws.len());
    for raw in raws.into_iter().rev() {
        let user_name = if raw.user_id.is_empty() {
            raw.fallback_name.clone()
        } else {
            db.user_name(&raw.user_id)?
                .unwrap_or_else(|| raw.fallback_name.clone())
        };
        messages.push(build_message(
            &raw,
            channel_id,
            channel_name,
            my_id,
            &user_name,
            local_reply,
            reply_to,
        ));
    }
    Ok(messages)
}

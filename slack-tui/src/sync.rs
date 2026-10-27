use crate::db::Store;
use crate::model::{Message, RawMessage, build_message, compose_text, mention_matches};
use crate::slack::SlackClient;

#[derive(Clone, Debug)]
pub struct SyncOut {
    pub inserted: Vec<Message>,
    pub scanned: usize,
    /// True when this call filled the channel for the first time.
    pub initial: bool,
}

pub async fn sync_channel(
    slack: &SlackClient,
    db: &Store,
    channel_id: &str,
    channel_name: &str,
    my_id: &str,
) -> Result<SyncOut, String> {
    let key = format!("backfill:{channel_id}");
    let initial = db.meta(&key)?.as_deref() != Some("1");
    let backfilled = !initial;
    let oldest = if backfilled {
        db.newest_ts(channel_id, channel_name)?.unwrap_or_default()
    } else {
        String::new()
    };
    let (raw, scanned) = slack.history(channel_id, &oldest, 50, 200).await?;
    let messages = materialize(slack, db, channel_id, channel_name, my_id, raw, false, "").await?;
    let mut inserted = Vec::new();
    for message in messages {
        if db.put_message(&message)? {
            inserted.push(message);
        }
    }
    if !backfilled {
        db.set_meta(&key, "1")?;
    }
    Ok(SyncOut {
        inserted,
        scanned,
        initial,
    })
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
    if missing {
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

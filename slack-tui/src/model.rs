use std::collections::HashMap;

/// One Slack message stored locally and shown in the terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub channel_id: String,
    pub channel_name: String,
    pub ts: String,
    pub user_id: String,
    pub user_name: String,
    pub thread_ts: String,
    pub text: String,
    pub kind: String,
    pub local_reply: bool,
    pub reply_to: String,
}

/// Message fields taken from `conversations.history` before names are resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawMessage {
    pub ts: String,
    pub user_id: String,
    pub fallback_name: String,
    pub thread_ts: String,
    pub text: String,
    pub reply_count: u32,
}

pub fn classify(author: &str, my_id: &str, text: &str, channel_name: &str) -> String {
    if !my_id.is_empty() && author == my_id {
        "你发出".to_string()
    } else if !my_id.is_empty() && text.contains(&format!("<@{my_id}>")) {
        "@你".to_string()
    } else {
        format!("#{channel_name}")
    }
}

pub fn mention_matches(text: &str, mention: &str, my_id: &str, wanted_id: &str) -> bool {
    match mention {
        "all" => true,
        "" | "me" | "我" => !my_id.is_empty() && text.contains(&format!("<@{my_id}>")),
        _ => !wanted_id.is_empty() && text.contains(&format!("<@{wanted_id}>")),
    }
}

pub fn mention_label(mention: &str) -> String {
    match mention {
        "all" => "全部".to_string(),
        "" | "me" | "我" => "@你".to_string(),
        other => format!("@{other}"),
    }
}

/// Build the text that will be posted. Empty body is refused.
pub fn compose_text(body: &str, mention_ids: &[String], my_id: &str) -> Result<String, String> {
    let body = body.trim();
    if body.is_empty() {
        return Err("没有正文，没有发送。".to_string());
    }
    let mut tags = Vec::new();
    for id in mention_ids {
        if id.is_empty() || id == my_id {
            continue;
        }
        let tag = format!("<@{id}>");
        if !body.contains(&tag) && !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    if tags.is_empty() {
        Ok(body.to_string())
    } else {
        Ok(format!("{} {body}", tags.join(" ")))
    }
}

pub fn build_message(
    raw: &RawMessage,
    channel_id: &str,
    channel_name: &str,
    my_id: &str,
    user_name: &str,
    local_reply: bool,
    reply_to: &str,
) -> Message {
    let kind = if local_reply {
        "你发出".to_string()
    } else {
        classify(&raw.user_id, my_id, &raw.text, channel_name)
    };
    Message {
        channel_id: channel_id.to_string(),
        channel_name: channel_name.to_string(),
        ts: raw.ts.clone(),
        user_id: raw.user_id.clone(),
        user_name: user_name.to_string(),
        thread_ts: raw.thread_ts.clone(),
        text: raw.text.clone(),
        kind,
        local_reply,
        reply_to: reply_to.to_string(),
    }
}

pub fn looks_like_user_id(query: &str) -> bool {
    let query = query.trim().trim_start_matches('@');
    let mut chars = query.chars();
    match chars.next() {
        Some('U' | 'W') => query.len() >= 2 && chars.all(|ch| ch.is_ascii_alphanumeric()),
        _ => false,
    }
}

/// Replace `<@U123>` and `<#C123|name>` with readable labels.
pub fn prettify(text: &str, names: &HashMap<String, String>) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '<' && index + 2 < chars.len() && matches!(chars[index + 1], '@' | '#') {
            if let Some(end) = chars[index + 2..].iter().position(|ch| *ch == '>') {
                let inside: String = chars[index + 2..index + 2 + end].iter().collect();
                let mut parts = inside.split('|');
                let id = parts.next().unwrap_or("");
                let fallback = parts.next().unwrap_or("");
                let prefix = if chars[index + 1] == '@' { '@' } else { '#' };
                let label = names
                    .get(id)
                    .map(String::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(if fallback.is_empty() { id } else { fallback });
                out.push(prefix);
                out.push_str(label);
                index += end + 3;
                continue;
            }
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

pub fn thread_root<'a>(thread_ts: &'a str, ts: &'a str) -> &'a str {
    if thread_ts.is_empty() { ts } else { thread_ts }
}

pub fn min_ts<'a>(timestamps: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    timestamps
        .into_iter()
        .filter(|ts| !ts.is_empty())
        .min_by(|left, right| cmp_ts(left, right))
}

/// Where the next scan should continue when a page stopped early.
/// An empty result means the hole is closed.
pub fn gap_after_sync(
    saved_before: &str,
    head_min: Option<&str>,
    head_truncated: bool,
    fill_min: Option<&str>,
    fill_ran: bool,
    fill_truncated: bool,
) -> String {
    let mut gap = String::new();
    if head_truncated {
        if let Some(ts) = head_min.filter(|ts| !ts.is_empty()) {
            gap = ts.to_string();
        }
    }
    if saved_before.is_empty() {
        return gap;
    }
    if !fill_ran || fill_truncated {
        gap = older_ts(&gap, saved_before);
        if fill_truncated {
            if let Some(ts) = fill_min.filter(|ts| !ts.is_empty()) {
                gap = older_ts(&gap, ts);
            }
        }
    }
    gap
}

fn older_ts(current: &str, candidate: &str) -> String {
    if current.is_empty() || cmp_ts(candidate, current) == std::cmp::Ordering::Less {
        candidate.to_string()
    } else {
        current.to_string()
    }
}

/// The hole still left after one scan.
/// `floor` is the newest timestamp already contiguous; `edge` is the oldest timestamp
/// the next fill must stay behind. An empty edge means the hole is closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GapMark {
    pub floor: String,
    pub edge: String,
}

pub fn next_gap(
    watermark: &str,
    saved_floor: &str,
    saved_edge: &str,
    head_min: Option<&str>,
    head_truncated: bool,
    fill_min: Option<&str>,
    fill_ran: bool,
    fill_truncated: bool,
) -> GapMark {
    let edge = gap_after_sync(
        saved_edge,
        head_min,
        head_truncated,
        fill_min,
        fill_ran,
        fill_truncated,
    );
    if edge.is_empty() {
        return GapMark {
            floor: String::new(),
            edge,
        };
    }
    let mut floor = if !saved_floor.is_empty() && edge == saved_edge {
        saved_floor.to_string()
    } else if head_truncated && head_min == Some(edge.as_str()) {
        watermark.to_string()
    } else if !saved_floor.is_empty() {
        saved_floor.to_string()
    } else {
        watermark.to_string()
    };
    if !floor.is_empty() && cmp_ts(&floor, &edge) != std::cmp::Ordering::Less {
        floor.clear();
    }
    GapMark { floor, edge }
}

pub fn cmp_ts(left: &str, right: &str) -> std::cmp::Ordering {
    let left_n = left.parse::<f64>().ok();
    let right_n = right.parse::<f64>().ok();
    match (left_n, right_n) {
        (Some(left), Some(right)) => left
            .partial_cmp(&right)
            .unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => left.cmp(right),
    }
}

pub fn char_len(text: &str) -> usize {
    text.chars().count()
}

pub fn split_at_cursor(text: &str, cursor: usize) -> (String, String) {
    let cursor = cursor.min(char_len(text));
    let head: String = text.chars().take(cursor).collect();
    let tail: String = text.chars().skip(cursor).collect();
    (head, tail)
}

pub fn move_cursor(text: &str, cursor: usize, delta: isize) -> usize {
    let len = char_len(text) as isize;
    (cursor as isize + delta).clamp(0, len) as usize
}

pub fn insert_at(input: &mut String, cursor: &mut usize, ch: char) {
    let mut chars: Vec<char> = input.chars().collect();
    let index = (*cursor).min(chars.len());
    chars.insert(index, ch);
    *input = chars.into_iter().collect();
    *cursor = index + 1;
}

pub fn backspace_at(input: &mut String, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }
    let mut chars: Vec<char> = input.chars().collect();
    let index = (*cursor).min(chars.len());
    if index == 0 {
        return;
    }
    chars.remove(index - 1);
    *input = chars.into_iter().collect();
    *cursor = index - 1;
}

pub fn delete_at(input: &mut String, cursor: &mut usize) {
    let mut chars: Vec<char> = input.chars().collect();
    if *cursor >= chars.len() {
        return;
    }
    chars.remove(*cursor);
    *input = chars.into_iter().collect();
}

pub fn delete_word(input: &mut String, cursor: &mut usize) {
    let chars: Vec<char> = input.chars().collect();
    let end = (*cursor).min(chars.len());
    let mut index = end;
    while index > 0 && chars[index - 1].is_whitespace() {
        index -= 1;
    }
    while index > 0 && !chars[index - 1].is_whitespace() {
        index -= 1;
    }
    let mut next = chars;
    next.drain(index..end);
    *input = next.into_iter().collect();
    *cursor = index;
}

/// Hide Slack token material if a transport error echoes a request.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        if let Some(len) = token_len(&text[index..]) {
            out.push('…');
            index += len;
            continue;
        }
        let ch = text[index..].chars().next().unwrap_or('\u{fffd}');
        out.push(ch);
        index += ch.len_utf8();
    }
    if out.chars().count() > 300 {
        out = out.chars().take(300).collect();
        out.push('…');
    }
    out
}

fn token_len(rest: &str) -> Option<usize> {
    const PREFIXES: [&str; 6] = ["xoxb-", "xoxp-", "xoxa-", "xoxs-", "xoxr-", "xapp-"];
    for prefix in PREFIXES {
        if let Some(body) = rest.strip_prefix(prefix) {
            let tail = body
                .find(|ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | ',' | ';'))
                .unwrap_or(body.len());
            return Some(prefix.len() + tail);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_mentions_and_self() {
        assert_eq!(classify("U1", "U1", "hi", "pr"), "你发出");
        assert_eq!(classify("U2", "U1", "hey <@U1> there", "pr"), "@你");
        assert_eq!(classify("U2", "U1", "hello", "pr"), "#pr");
    }

    #[test]
    fn compose_refuses_empty_and_skips_self() {
        assert!(compose_text("  ", &["U2".into()], "U1").is_err());
        assert_eq!(compose_text("在", &["U1".into()], "U1").unwrap(), "在");
        assert_eq!(
            compose_text("在", &["U2".into(), "U2".into()], "U1").unwrap(),
            "<@U2> 在"
        );
        assert_eq!(
            compose_text("<@U2> 在", &["U2".into()], "U1").unwrap(),
            "<@U2> 在"
        );
    }

    #[test]
    fn prettify_replaces_user_and_channel_tags() {
        let mut names = HashMap::new();
        names.insert("U1".to_string(), "Ada".to_string());
        assert_eq!(prettify("hi <@U1|Ada> <#C1|pr>", &names), "hi @Ada #pr");
    }

    #[test]
    fn redacts_tokens() {
        let clean = redact("failed xoxp-123-abc and xapp-1-TOKEN, rest");
        assert!(!clean.contains("xoxp-123"));
        assert!(!clean.contains("xapp-1"));
        assert!(clean.contains("rest"));
    }

    #[test]
    fn deletes_the_word_before_the_cursor() {
        let mut text = "你好 world ".to_string();
        let mut cursor = char_len(&text);
        delete_word(&mut text, &mut cursor);
        assert_eq!(text, "你好 ");
        delete_word(&mut text, &mut cursor);
        assert_eq!(text, "");
        assert_eq!(cursor, 0);
    }

    #[test]
    fn edits_around_a_cursor() {
        let mut text = "ac".to_string();
        let mut cursor = 1;
        insert_at(&mut text, &mut cursor, 'b');
        assert_eq!(text, "abc");
        assert_eq!(cursor, 2);
        backspace_at(&mut text, &mut cursor);
        assert_eq!(text, "ac");
        delete_at(&mut text, &mut cursor);
        assert_eq!(text, "a");
        let (head, tail) = split_at_cursor("你好", 1);
        assert_eq!(head, "你");
        assert_eq!(tail, "好");
    }

    #[test]
    fn keeps_the_older_edge_of_a_truncated_page() {
        assert_eq!(
            gap_after_sync("", Some("50"), true, None, false, false),
            "50"
        );
        assert_eq!(
            gap_after_sync("", Some("50"), false, None, false, false),
            ""
        );
        assert_eq!(
            gap_after_sync("20", Some("50"), true, Some("30"), true, true),
            "20"
        );
        assert_eq!(
            gap_after_sync("20", Some("50"), false, Some("30"), true, false),
            ""
        );
        assert_eq!(
            gap_after_sync("20", Some("40"), true, Some("30"), true, false),
            "40"
        );
    }

    #[test]
    fn remembers_the_floor_while_a_gap_walks_backward() {
        let opened = next_gap("20", "", "", Some("50"), true, None, false, false);
        assert_eq!(opened.edge, "50");
        assert_eq!(opened.floor, "20");

        let walked = next_gap("80", "20", "50", None, false, Some("30"), true, true);
        assert_eq!(walked.edge, "30");
        assert_eq!(walked.floor, "20");

        let closed = next_gap("80", "20", "50", None, false, Some("30"), true, false);
        assert_eq!(closed.edge, "");
        assert_eq!(closed.floor, "");

        let replaced = next_gap("20", "5", "10", Some("40"), true, Some("8"), true, false);
        assert_eq!(replaced.edge, "40");
        assert_eq!(replaced.floor, "20");
    }
}

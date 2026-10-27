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

pub fn delete_word(input: &mut String) {
    let end = input.trim_end().len();
    input.truncate(end);
    if let Some(index) = input.rfind(char::is_whitespace) {
        input.truncate(index + 1);
    } else {
        input.clear();
    }
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
    fn deletes_the_last_word() {
        let mut text = "你好 world ".to_string();
        delete_word(&mut text);
        assert_eq!(text, "你好 ");
        delete_word(&mut text);
        assert_eq!(text, "");
    }
}

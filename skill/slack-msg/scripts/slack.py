#!/usr/bin/env python3
"""Fetch or send Slack messages. Never prints tokens. Does not mark read.

Default fetch: mentions of the current user in #pr.
Default send: post in #pr, mentioning David. Extra --at names are added.
"""

from __future__ import annotations

import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timedelta, timezone
from pathlib import Path

ENV_PATH = Path(os.environ.get("SLACK_ENV_FILE", Path.home() / ".config/grok-slack/env"))
LOCAL_CONFIG = Path.home() / ".config/grok-slack/slack-msg.json"
API = "https://slack.com/api/"
TZ = timezone(timedelta(hours=7))
DEFAULT_CHANNEL = "pr"
DEFAULT_SEND_TO = ""


def local_defaults() -> tuple[str, str]:
    channel = DEFAULT_CHANNEL
    send_to = DEFAULT_SEND_TO
    if LOCAL_CONFIG.is_file():
        data = json.loads(LOCAL_CONFIG.read_text())
        channel = data.get("default_channel") or channel
        send_to = data.get("default_at") or send_to
    return channel, send_to


def load_env() -> dict[str, str]:
    env: dict[str, str] = {}
    if not ENV_PATH.is_file():
        return env
    for line in ENV_PATH.read_text().splitlines():
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        env[key.strip()] = value.strip().strip('"').strip("'")
    return env


def api(token: str, method: str, params: dict[str, str]) -> dict:
    request = urllib.request.Request(
        API + method,
        data=urllib.parse.urlencode(params).encode(),
        headers={
            "Authorization": "Bearer " + token,
            "Content-Type": "application/x-www-form-urlencoded",
        },
    )
    with urllib.request.urlopen(request, timeout=20) as response:
        return json.load(response)


def pages(token: str, method: str, params: dict[str, str], key: str) -> list:
    items: list = []
    cursor = ""
    for _ in range(8):
        body = dict(params)
        if cursor:
            body["cursor"] = cursor
        data = api(token, method, body)
        if not data.get("ok"):
            raise RuntimeError(data.get("error") or method + " failed")
        items.extend(data.get(key) or [])
        cursor = (data.get("response_metadata") or {}).get("next_cursor") or ""
        if not cursor:
            break
    return items


def local_time(ts: str) -> str:
    return datetime.fromtimestamp(float(ts), TZ).strftime("%m-%d %H:%M")


def channel_id(token: str, channel: str) -> tuple[str, str]:
    name = channel.strip()
    if name.startswith("#"):
        name = name[1:]
    channels = pages(
        token,
        "conversations.list",
        {
            "types": "public_channel,private_channel",
            "exclude_archived": "true",
            "limit": "200",
        },
        "channels",
    )
    wanted = name.lower()
    for item in channels:
        if item.get("id") == channel.strip() or (item.get("name") or "").lower() == wanted:
            return item["id"], item.get("name") or name
    raise RuntimeError(f"找不到频道 {channel}")


def members(token: str) -> list:
    return pages(token, "users.list", {"limit": "200"}, "members")


def display(member: dict) -> str:
    profile = member.get("profile") or {}
    return (
        profile.get("display_name")
        or member.get("real_name")
        or member.get("name")
        or member.get("id")
        or "未知"
    )


def user_id(token: str, who: str, roster: list[dict]) -> str:
    query = who.strip()
    if query.startswith("<@"):
        query = query[2:].split("|", 1)[0].rstrip(">")
    if query.startswith("@"):
        query = query[1:]
    if query[:1] == "U" and query[1:].isalnum():
        return query
    needle = query.lower()
    for member in roster:
        if member.get("deleted"):
            continue
        profile = member.get("profile") or {}
        names = {
            (member.get("name") or "").lower(),
            (member.get("real_name") or "").lower(),
            (profile.get("display_name") or "").lower(),
            (profile.get("real_name") or "").lower(),
        }
        if needle in names:
            return member["id"]
    raise RuntimeError(f"找不到这个人：{who}")


def fetch(token: str, channel: str, mention: str, limit: int) -> int:
    cid, cname = channel_id(token, channel)
    me = api(token, "auth.test", {})
    my_id = me.get("user_id") or ""
    roster = members(token)
    names = {member["id"]: display(member) for member in roster if member.get("id")}
    wanted = ""
    if mention not in ("", "all"):
        wanted = my_id if mention in ("me", "我") else user_id(token, mention, roster)
    collected = []
    cursor = ""
    scanned = 0
    while len(collected) < limit and scanned < 200:
        params = {"channel": cid, "limit": "100"}
        if cursor:
            params["cursor"] = cursor
        data = api(token, "conversations.history", params)
        if not data.get("ok"):
            print(f"历史读取失败：{data.get('error')}")
            return 1
        batch = data.get("messages") or []
        scanned += len(batch)
        for message in batch:
            text = message.get("text") or ""
            if wanted and f"<@{wanted}>" not in text:
                continue
            author = message.get("user") or ""
            who = names.get(author) or message.get("subtype") or message.get("bot_id") or "系统"
            collected.append(f"{local_time(message['ts'])}  {who}  {text.replace(chr(10), ' ')}")
            if len(collected) >= limit:
                break
        cursor = (data.get("response_metadata") or {}).get("next_cursor") or ""
        if not cursor or not batch:
            break
    label = "全部" if not wanted else ("@你" if wanted == my_id else f"@{mention}")
    print(f"#{cname}  {label}  清迈时间  扫描 {scanned} 条")
    if not collected:
        print("没有符合条件的消息。")
        return 0
    for line in reversed(collected):
        print(line)
    return 0


def send(token: str, channel: str, text: str, ats: list[str]) -> int:
    body = text.strip()
    if not body:
        print("没有正文，没有发送。")
        return 1
    roster = members(token)
    targets = ats or [DEFAULT_SEND_TO]
    mentions = []
    for who in targets:
        uid = user_id(token, who, roster)
        tag = f"<@{uid}>"
        if tag not in body and tag not in mentions:
            mentions.append(tag)
    posted = (" ".join(mentions) + " " + body).strip() if mentions else body
    cid, cname = channel_id(token, channel)
    result = api(token, "chat.postMessage", {"channel": cid, "text": posted})
    if not result.get("ok"):
        print(f"没有发送：{result.get('error')}")
        return 1
    who = api(token, "auth.test", {})
    print(f"已发送到 #{cname}。身份 {who.get('user')}。ts {result.get('ts')}")
    print(posted)
    return 0


def usage() -> str:
    return """用法：
  python3 scripts/slack.py
  python3 scripts/slack.py fetch [--channel pr] [--mention me|all|名字] [--limit 10]
  python3 scripts/slack.py send [--channel pr] [--at 名字]... 正文

默认收取 #pr 里 @ 你的消息。默认发送到 #pr，并 @ David。可以重复 --at 再点名其他人。
"""


def main(argv: list[str]) -> int:
    env = load_env()
    token = env.get("SLACK_USER_TOKEN") or env.get("SLACK_BOT_TOKEN")
    if not token:
        print("没有可用的 Slack 令牌。不打印密钥文件内容。")
        return 1
    default_channel, default_at = local_defaults()
    if not argv or argv[0] in ("fetch", "收", "收取"):
        args = argv[1:] if argv and argv[0] in ("fetch", "收", "收取") else argv
        channel = default_channel
        mention = "me"
        limit = 10
        index = 0
        while index < len(args):
            item = args[index]
            if item == "--channel" and index + 1 < len(args):
                channel = args[index + 1]
                index += 2
            elif item == "--mention" and index + 1 < len(args):
                mention = args[index + 1]
                index += 2
            elif item == "--limit" and index + 1 < len(args):
                limit = max(1, min(int(args[index + 1]), 30))
                index += 2
            else:
                print(usage())
                return 2
            continue
        try:
            return fetch(token, channel, mention, limit)
        except (RuntimeError, urllib.error.URLError, TimeoutError, json.JSONDecodeError) as error:
            print(str(error))
            return 1
    if argv[0] == "send":
        channel = default_channel
        ats: list[str] = []
        words: list[str] = []
        index = 1
        while index < len(argv):
            item = argv[index]
            if item == "--channel" and index + 1 < len(argv):
                channel = argv[index + 1]
                index += 2
            elif item == "--at" and index + 1 < len(argv):
                ats.append(argv[index + 1])
                index += 2
            else:
                words.append(item)
                index += 1
        if not ats and default_at:
            ats = [default_at]
        try:
            return send(token, channel, " ".join(words), ats)
        except (RuntimeError, urllib.error.URLError, TimeoutError, json.JSONDecodeError) as error:
            print(str(error))
            return 1
    print(usage())
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

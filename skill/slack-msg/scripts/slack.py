#!/usr/bin/env python3
"""Fetch or send Slack messages. Never prints tokens. Does not mark read.

Default fetch: mentions of the current user in #pr.
Default send: post in #pr, mentioning David. Extra --at names are added.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
import time
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


def cached_channel(name: str) -> tuple[str, str] | None:
    path = STATE / "channels.json"
    if not path.is_file():
        return None
    data = json.loads(path.read_text())
    found = data.get(name.lower())
    if not found:
        return None
    return found["id"], found["name"]


def remember_channel(name: str, cid: str, cname: str) -> None:
    STATE.mkdir(parents=True, exist_ok=True)
    path = STATE / "channels.json"
    data = json.loads(path.read_text()) if path.is_file() else {}
    data[name.lower()] = {"id": cid, "name": cname}
    path.write_text(json.dumps(data))


def channel_id(token: str, channel: str) -> tuple[str, str]:
    name = channel.strip()
    if name.startswith("#"):
        name = name[1:]
    cached = cached_channel(name)
    if cached:
        return cached
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
            cid, cname = item["id"], item.get("name") or name
            remember_channel(name, cid, cname)
            return cid, cname
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


STATE = Path.home() / ".local/state/slack-msg"


def load_names() -> dict[str, str]:
    path = STATE / "names.json"
    if not path.is_file():
        return {}
    return json.loads(path.read_text())


def save_names(names: dict[str, str]) -> None:
    STATE.mkdir(parents=True, exist_ok=True)
    (STATE / "names.json").write_text(json.dumps(names, ensure_ascii=False))


def cached_history(channel: str, limit: int) -> list[dict]:
    path = STATE / "mentions.jsonl"
    if not path.is_file():
        return []
    by_ts: dict[str, dict] = {}
    for line in path.read_text().splitlines():
        if not line.strip():
            continue
        item = json.loads(line)
        if channel and item.get("channel") not in (channel, f"#{channel}"):
            continue
        ts = item.get("ts")
        if ts:
            by_ts[ts] = item
    return sorted(by_ts.values(), key=lambda item: float(item["ts"]))[-limit:]


def collect_messages(
    token: str,
    channel: str,
    mention: str,
    limit: int,
    oldest: str = "",
    include_self: bool = False,
) -> tuple[str, list[dict], int]:
    cid, cname = channel_id(token, channel)
    me = api(token, "auth.test", {})
    my_id = me.get("user_id") or ""
    names = load_names()
    wanted = ""
    if mention not in ("", "all"):
        if mention in ("me", "我"):
            wanted = my_id
        else:
            wanted = user_id(token, mention, members(token))
    collected = []
    cursor = ""
    scanned = 0
    while len(collected) < limit and scanned < 200:
        params = {"channel": cid, "limit": "100"}
        if oldest:
            params["oldest"] = oldest
        if cursor:
            params["cursor"] = cursor
        data = api(token, "conversations.history", params)
        if not data.get("ok"):
            raise RuntimeError(data.get("error") or "history failed")
        batch = data.get("messages") or []
        scanned += len(batch)
        for message in batch:
            text = message.get("text") or ""
            author = message.get("user") or ""
            mentioned = not wanted or f"<@{wanted}>" in text
            sent_by_me = include_self and bool(my_id) and author == my_id
            if not mentioned and not sent_by_me:
                continue
            collected.append(
                {
                    "channel": cname,
                    "channel_id": cid,
                    "ts": message.get("ts") or "",
                    "user_id": author,
                    "user": names.get(author) or message.get("subtype") or message.get("bot_id") or author or "系统",
                    "thread_ts": message.get("thread_ts") or "",
                    "text": text.replace("\n", " "),
                }
            )
            if len(collected) >= limit:
                break
        cursor = (data.get("response_metadata") or {}).get("next_cursor") or ""
        if not cursor or not batch:
            break
    unresolved = [item.get("user_id") or "" for item in collected if item.get("user_id") and item.get("user_id") not in names]
    if unresolved:
        roster = {member["id"]: display(member) for member in members(token) if member.get("id")}
        names.update(roster)
        save_names(names)
        for item in collected:
            author = item.get("user_id") or ""
            if author in names:
                item["user"] = names[author]
    return cname, collected, scanned


def fetch(token: str, channel: str, mention: str, limit: int) -> int:
    cname, collected, scanned = collect_messages(token, channel, mention, limit)
    label = "全部" if mention == "all" else ("@你" if mention in ("me", "我", "") else f"@{mention}")
    print(f"#{cname}  {label}  清迈时间  扫描 {scanned} 条")
    if not collected:
        print("没有符合条件的消息。")
        return 0
    for item in reversed(collected):
        print(f"{local_time(item['ts'])}  {item['user']}  {item['text']}")
    return 0


def is_bangkok_work_hours(dt: datetime | None = None) -> bool:
    now = dt or datetime.now(TZ)
    # 曼谷时间下午1:30开始，到晚上11点 (13:30 ~ 23:00)
    current_minutes = now.hour * 60 + now.minute
    return (13 * 60 + 30) <= current_minutes < (23 * 60)


def get_sound_type() -> str:
    if LOCAL_CONFIG.is_file():
        try:
            data = json.loads(LOCAL_CONFIG.read_text())
            return data.get("sound") or "doorbell"
        except Exception:
            pass
    return "doorbell"


def play_alert_sound(sound_type: str = "") -> None:
    try:
        st = sound_type or get_sound_type()
        sound_file = (
            "/System/Library/Sounds/Ping.aiff"
            if st == "doorbell"
            else "/System/Library/Sounds/Pop.aiff"
        )
        if sys.platform == "darwin" and os.path.isfile(sound_file):
            subprocess.Popen(
                ["afplay", sound_file],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
        else:
            sys.stdout.write("\a")
            sys.stdout.flush()
    except Exception:
        sys.stdout.write("\a")
        sys.stdout.flush()


def poll(token: str, channel: str) -> tuple[dict | None, int]:
    """Standalone cron entry. Writes new messages in channel to local file. No model, no send."""
    STATE.mkdir(parents=True, exist_ok=True)
    seen_path = STATE / "seen.txt"
    seen = set(seen_path.read_text().split()) if seen_path.is_file() else set()
    oldest = max(seen, key=float) if seen else ""
    cname, collected, scanned = collect_messages(token, channel, "all", 50, oldest, include_self=True)
    fresh = [item for item in collected if item.get("ts") and item["ts"] not in seen]
    stamp = datetime.now(TZ).strftime("%Y-%m-%d %H:%M")
    if not fresh:
        print(f"{stamp}  没有新的消息。扫描 {scanned} 条。", flush=True)
        return None, 0
    me_id = api(token, "auth.test", {}).get("user_id") or ""
    with (STATE / "mentions.jsonl").open("a", encoding="utf-8") as handle:
        for item in reversed(fresh):
            author = item.get("user_id") or ""
            if author == me_id:
                kind = "你发出"
            elif f"<@{me_id}>" in item.get("text", ""):
                kind = "@你"
            else:
                kind = f"#{cname}"
            item["kind"] = kind
            handle.write(json.dumps(item, ensure_ascii=False) + "\n")
            seen.add(item["ts"])
            print(f"{local_time(item['ts'])}  {kind}  {item['user']}  {item['text']}", flush=True)
            print("─" * 48, flush=True)
    seen_path.write_text("\n".join(sorted(seen)) + "\n")
    play_alert_sound()
    return fresh[0] if fresh else None, len(fresh)


def print_context_line(item: dict) -> None:
    kind = item.get("kind") or ("你发出" if item.get("local_reply") else "")
    prefix = f"{kind}  " if kind else ""
    print(f"{local_time(item['ts'])}  {prefix}{item['user']}  {item['text']}", flush=True)
    print("─" * 48, flush=True)


def remember_dialog(item: dict) -> None:
    STATE.mkdir(parents=True, exist_ok=True)
    with (STATE / "mentions.jsonl").open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(item, ensure_ascii=False) + "\n")
    seen_path = STATE / "seen.txt"
    seen = set(seen_path.read_text().split()) if seen_path.is_file() else set()
    if item.get("ts"):
        seen.add(item["ts"])
    seen_path.write_text("\n".join(sorted(seen)) + "\n")


def show_startup_context(token: str, channel: str) -> None:
    """Print cached history immediately, then ask Slack only for newer messages."""
    STATE.mkdir(parents=True, exist_ok=True)
    cached = cached_history(channel, 10)
    print(f"#{channel}  本地对话，最近 10 条  清迈时间", flush=True)
    if cached:
        print("别人说的和你回的都在这里。退出再进仍按时间顺序显示。", flush=True)
        for item in cached:
            print_context_line(item)
        oldest = cached[-1]["ts"]
    else:
        oldest = ""
        print("本地还没有缓存，正在向 Slack 取最近 10 条。", flush=True)
    cname, fresh, _scanned = collect_messages(token, channel, "all", 10, oldest)
    seen_path = STATE / "seen.txt"
    seen = set(seen_path.read_text().split()) if seen_path.is_file() else set()
    new_items = [item for item in fresh if item.get("ts") and item["ts"] not in {row.get("ts") for row in cached}]
    if new_items:
        print(f"#{cname}  缓存之后的新消息", flush=True)
        with (STATE / "mentions.jsonl").open("a", encoding="utf-8") as handle:
            for item in reversed(new_items):
                item = dict(item)
                item["context"] = True
                handle.write(json.dumps(item, ensure_ascii=False) + "\n")
                seen.add(item["ts"])
                print_context_line(item)
    elif not cached:
        print("没有历史消息。", flush=True)
    for item in cached:
        if item.get("ts"):
            seen.add(item["ts"])
    seen_path.write_text("\n".join(sorted(seen)) + "\n")
    shown = list(cached) + list(new_items)
    if not shown:
        return None
    return max(shown, key=lambda item: float(item.get("ts") or 0))


def watch(token: str, channel: str, initial_every: int | None = None, auto_backoff: bool = True) -> int:
    """Foreground loop. Prints in the terminal that started it, and appends to a file."""
    STATE.mkdir(parents=True, exist_ok=True)
    print(
        f"监听已启动。先显示最近 10 条，收取 #{channel} 内全部新消息并记录到本地。"
        f"\n扫描规则：曼谷时间 13:30-23:00 每 5 分钟扫描并提示声音；其他时间无消息自动降频 (5m->10m->15m...->30m)。"
        f"\n结果写入 {STATE / 'mentions.jsonl'}。Ctrl-C 停止。",
        flush=True,
    )
    latest = {"item": show_startup_context(token, channel)}
    lock = threading.Lock()
    stop = threading.Event()

    def refresh() -> None:
        interval = initial_every or (5 * 60)
        while not stop.is_set():
            if auto_backoff:
                if is_bangkok_work_hours():
                    interval = 5 * 60
            if stop.wait(interval):
                break
            try:
                item, count = poll(token, channel)
                if item:
                    with lock:
                        latest["item"] = item
                if auto_backoff:
                    if is_bangkok_work_hours():
                        interval = 5 * 60
                    else:
                        if count > 0:
                            interval = 5 * 60
                        else:
                            interval = min(30 * 60, interval + 5 * 60)
            except (RuntimeError, urllib.error.URLError, TimeoutError, json.JSONDecodeError) as error:
                print(str(error), flush=True)

    threading.Thread(target=refresh, daemon=True).start()
    print("回复最新一条后回车。空行不发送。Ctrl-C 停止。", flush=True)
    try:
        while True:
            text = input("回复> ")
            if not text.strip():
                continue
            with lock:
                item = latest["item"]
            if not item:
                print("还没有可回复的消息。", flush=True)
                continue
            code, record = send(
                token,
                channel,
                text,
                [],
                thread_ts=item.get("thread_ts") or item.get("ts") or "",
                mention_ids=[item.get("user_id") or ""],
            )
            if code == 0 and record:
                remember_dialog(record)
                print_context_line(record)
                with lock:
                    latest["item"] = record
                print("已记入本地对话。", flush=True)
    except (KeyboardInterrupt, EOFError):
        stop.set()
        print("\n已停止。", flush=True)
        return 0


def send(token: str, channel: str, text: str, ats: list[str], thread_ts: str = "", mention_ids: list[str] | None = None) -> tuple[int, dict | None]:
    body = text.strip()
    if not body:
        print("没有正文，没有发送。")
        return 1, None
    roster = members(token)
    me_id = api(token, "auth.test", {}).get("user_id") or ""
    targets = ats or ([DEFAULT_SEND_TO] if mention_ids is None else [])
    mentions = []
    for who in targets:
        if not who:
            continue
        uid = user_id(token, who, roster)
        tag = f"<@{uid}>"
        if tag not in body and tag not in mentions:
            mentions.append(tag)
    for uid in mention_ids or []:
        if not uid or uid == me_id:
            continue
        tag = f"<@{uid}>"
        if tag not in body and tag not in mentions:
            mentions.append(tag)
    posted = (" ".join(mentions) + " " + body).strip() if mentions else body
    cid, cname = channel_id(token, channel)
    payload = {"channel": cid, "text": posted}
    if thread_ts:
        payload["thread_ts"] = thread_ts
    result = api(token, "chat.postMessage", payload)
    if not result.get("ok"):
        print(f"没有发送：{result.get('error')}")
        return 1, None
    who = api(token, "auth.test", {})
    record = {
        "channel": cname,
        "channel_id": cid,
        "ts": result.get("ts") or "",
        "user_id": me_id,
        "user": who.get("user") or "你",
        "text": posted,
        "kind": "你发出",
        "local_reply": True,
        "reply_to": thread_ts,
    }
    print(f"已发送到 #{cname}。身份 {who.get('user')}。ts {result.get('ts')}")
    print(posted)
    return 0, record


def interval_seconds(token: str) -> int:
    text = token.strip().lower()
    if text.endswith("s") and text[:-1].isdigit():
        return max(30, int(text[:-1]))
    if text.endswith("m") and text[:-1].isdigit():
        return max(60, int(text[:-1]) * 60)
    if text.isdigit():
        return max(60, int(text) * 60)
    raise RuntimeError(f"无法识别刷新时间：{token}。用 --30 表示 30 分钟。")


def usage() -> str:
    return """用法：
  python3 scripts/slack.py
  python3 scripts/slack.py fetch [--channel pr] [--mention me|all|名字] [--limit 10]
  python3 scripts/slack.py watch [--30]
  python3 scripts/slack.py send [--channel pr] [--at 名字]... 正文

默认收取 #pr 里 @ 你的消息。watch 收取该频道的新消息：曼谷时间 13:30–23:00 每 5 分钟，其余时间没有新消息就退避到 30 分钟。--30 或 --every 固定间隔。默认发送到 #pr，并 @ 配置里的 default_at。
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
    if argv[0] in ("poll", "watch"):
        every = None
        auto_backoff = True
        rest = argv[1:]
        index = 0
        while index < len(rest):
            item = rest[index]
            if item == "--every" and index + 1 < len(rest):
                every = interval_seconds(rest[index + 1])
                auto_backoff = False
                index += 2
            elif item.startswith("--") and item[2:].isdigit():
                every = interval_seconds(item[2:])
                auto_backoff = False
                index += 1
            else:
                print(usage())
                return 2
        try:
            if argv[0] == "watch":
                return watch(token, default_channel, every, auto_backoff=auto_backoff)
            poll(token, default_channel)
            return 0
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
            code, record = send(token, channel, " ".join(words), ats)
            if code == 0 and record:
                remember_dialog(record)
            return code
        except (RuntimeError, urllib.error.URLError, TimeoutError, json.JSONDecodeError) as error:
            print(str(error))
            return 1
    print(usage())
    return 2


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except KeyboardInterrupt:
        print("\n已停止。", flush=True)
        sys.exit(0)

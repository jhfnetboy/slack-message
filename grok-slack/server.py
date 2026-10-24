#!/usr/bin/env python3
"""Local stdio MCP for one internal Slack app. Talks only to slack.com.

Secrets are read from ~/.config/grok-slack/env on every call. Nothing is logged
with a token in it. Stdout is reserved for MCP messages.
"""

from __future__ import annotations

import json
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path

ENV_PATH = Path.home() / ".config" / "grok-slack" / "env"
PROTOCOL = "2024-11-05"
SUPPORTED = {"2024-11-05", "2025-03-26", "2025-06-18"}
SLACK = "https://slack.com/api/"

USER_SCOPES = (
    "channels:history,groups:history,im:history,mpim:history,"
    "channels:read,groups:read,"
    "chat:write,users:read,users:read.email,"
    "search:read.public,search:read.private,search:read.im,search:read.mpim,"
    "search:read.users,search:read.files"
)
BOT_SCOPES = (
    "app_mentions:read,channels:history,groups:history,im:history,im:read,"
    "chat:write,users:read,reactions:read"
)


def eprint(text: str) -> None:
    sys.stderr.write(text.rstrip() + "\n")
    sys.stderr.flush()


def load_env() -> dict[str, str]:
    if not ENV_PATH.exists():
        return {}
    values: dict[str, str] = {}
    for raw in ENV_PATH.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        values[key.strip()] = value.strip().strip("'").strip('"')
    return values


def slack(method: str, token: str, payload: dict | None = None) -> dict:
    data = json.dumps(payload or {}).encode("utf-8")
    request = urllib.request.Request(
        SLACK + method,
        data=data,
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json; charset=utf-8",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            body = json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", errors="replace")[:300]
        raise RuntimeError(f"Slack HTTP {exc.code}: {detail}") from exc
    except urllib.error.URLError as exc:
        raise RuntimeError(f"连不上 slack.com：{exc.reason}") from exc
    if not body.get("ok"):
        raise RuntimeError(f"Slack {method} 失败：{body.get('error', 'unknown')}")
    return body


def token_for(env: dict[str, str], identity: str) -> tuple[str, str]:
    identity = (identity or "user").strip().lower()
    if identity == "bot":
        token = env.get("SLACK_BOT_TOKEN", "")
        if not token:
            raise RuntimeError("SLACK_BOT_TOKEN 还是空的。先跑 auth.py 完成授权。")
        return token, "bot"
    token = env.get("SLACK_USER_TOKEN", "")
    if token:
        return token, "user"
    bot = env.get("SLACK_BOT_TOKEN", "")
    if bot:
        return bot, "bot"
    raise RuntimeError(
        "还没有 Slack 令牌。把 Client Secret 写入 "
        f"{ENV_PATH} 后运行 python3 {Path.home() / '.config/grok-slack/auth.py'}"
    )


def pages(method: str, token: str, payload: dict, key: str, cap: int = 8) -> list:
    items: list = []
    cursor = ""
    for _ in range(cap):
        body = dict(payload)
        if cursor:
            body["cursor"] = cursor
        result = slack(method, token, body)
        items.extend(result.get(key) or [])
        cursor = (result.get("response_metadata") or {}).get("next_cursor") or ""
        if not cursor:
            break
    return items


def channel_id(token: str, channel: str) -> str:
    name = channel.strip()
    if name.startswith("#"):
        name = name[1:]
    if re.fullmatch(r"[CGD][A-Z0-9]{8,}", name):
        return name
    wanted = name.lower()
    channels = pages(
        "conversations.list",
        token,
        {
            "types": "public_channel,private_channel",
            "exclude_archived": True,
            "limit": 200,
        },
        "channels",
    )
    for item in channels:
        if item.get("name", "").lower() == wanted or item.get("id") == channel.strip():
            return item["id"]
    raise RuntimeError(f"找不到频道 {channel}。确认你在这个频道里，名字不要带多余空格。")


def users(token: str) -> list:
    return pages("users.list", token, {"limit": 200}, "members")


def user_id(token: str, who: str) -> str:
    query = who.strip()
    if query.startswith("<@"):
        query = query[2:].split("|", 1)[0].rstrip(">")
    if query[:1] == "U" and query[1:].isalnum():
        return query
    needle = query.lstrip("@").lower()
    for member in users(token):
        profile = member.get("profile") or {}
        names = {
            (member.get("name") or "").lower(),
            (member.get("real_name") or "").lower(),
            (profile.get("display_name") or "").lower(),
            (profile.get("real_name") or "").lower(),
        }
        if needle in names or needle == member.get("id", "").lower():
            return member["id"]
    raise RuntimeError(f"找不到这个人：{who}")


def display(member: dict) -> str:
    profile = member.get("profile") or {}
    return profile.get("display_name") or profile.get("real_name") or member.get("name") or member.get("id", "")


def status() -> str:
    env = load_env()
    lines = [
        f"密钥文件：{ENV_PATH}",
        f"Client Secret：{'已写' if env.get('SLACK_CLIENT_SECRET') else '空的'}",
        f"用户令牌：{'已写' if env.get('SLACK_USER_TOKEN') else '空的'}",
        f"Bot 令牌：{'已写' if env.get('SLACK_BOT_TOKEN') else '空的'}",
    ]
    token = env.get("SLACK_USER_TOKEN") or env.get("SLACK_BOT_TOKEN")
    if not token:
        lines.append("还不能发消息。填好 Client Secret 后运行 auth.py，在 Slack 页面点 Allow。")
        return "\n".join(lines)
    try:
        who = slack("auth.test", token, {})
    except RuntimeError as exc:
        lines.append(str(exc))
        return "\n".join(lines)
    kind = "用户" if token == env.get("SLACK_USER_TOKEN") else "bot"
    lines.append(f"当前身份：{who.get('user')} ({kind})，工作区 {who.get('team')}")
    return "\n".join(lines)


def list_channels() -> str:
    token, kind = token_for(load_env(), "user")
    rows = pages(
        "conversations.list",
        token,
        {"types": "public_channel,private_channel", "exclude_archived": True, "limit": 200},
        "channels",
    )
    if not rows:
        return "没有可见频道。"
    lines = [f"用{kind}令牌看到 {len(rows)} 个频道："]
    for item in sorted(rows, key=lambda row: row.get("name") or ""):
        mark = "私有" if item.get("is_private") else "公开"
        lines.append(f"#{item.get('name')}  {item.get('id')}  {mark}")
    return "\n".join(lines)


def find_user(who: str) -> str:
    token, _kind = token_for(load_env(), "user")
    needle = who.strip().lstrip("@").lower()
    hits = []
    for member in users(token):
        if member.get("deleted"):
            continue
        profile = member.get("profile") or {}
        blob = " ".join(
            [
                member.get("name") or "",
                member.get("real_name") or "",
                profile.get("display_name") or "",
            ]
        ).lower()
        if needle in blob or needle == (member.get("id") or "").lower():
            kind = "bot" if member.get("is_bot") or member.get("id") == "USLACKBOT" else "人"
            hits.append(f"{display(member)}  {member.get('id')}  {kind}  @{member.get('name')}")
    if not hits:
        return f"没有匹配：{who}"
    return "\n".join(hits[:30])


def read_messages(channel: str, who: str, limit: int) -> str:
    token, _kind = token_for(load_env(), "user")
    cid = channel_id(token, channel)
    limit = max(1, min(int(limit or 20), 50))
    wanted = user_id(token, who) if who else ""
    names = {member["id"]: display(member) for member in users(token)}
    collected = []
    cursor = ""
    scanned = 0
    while len(collected) < limit and scanned < 400:
        payload = {"channel": cid, "limit": 100}
        if cursor:
            payload["cursor"] = cursor
        result = slack("conversations.history", token, payload)
        batch = result.get("messages") or []
        scanned += len(batch)
        for message in batch:
            author = message.get("user") or message.get("bot_id") or ""
            if wanted and author != wanted and message.get("user") != wanted:
                continue
            text = (message.get("text") or "").replace("\n", " ")
            collected.append(
                f"{message.get('ts')}  {names.get(author, author)}  {text[:500]}"
            )
            if len(collected) >= limit:
                break
        cursor = (result.get("response_metadata") or {}).get("next_cursor") or ""
        if not cursor or not batch:
            break
    if not collected:
        return "这条频道里没有符合条件的消息。"
    return "\n".join(collected)


def search_messages(channel: str, who: str, limit: int) -> str:
    env = load_env()
    token = env.get("SLACK_USER_TOKEN", "")
    if not token:
        raise RuntimeError("搜索要用用户令牌。先完成 auth.py。")
    limit = max(1, min(int(limit or 20), 50))
    parts = []
    if channel:
        cid = channel_id(token, channel)
        info = slack("conversations.info", token, {"channel": cid})
        parts.append(f"in:#{info['channel']['name']}")
    if who:
        uid = user_id(token, who)
        parts.append(f"from:<@{uid}>")
    query = " ".join(parts) if parts else "*"
    result = slack(
        "search.messages",
        token,
        {"query": query, "count": limit, "sort": "timestamp", "sort_dir": "desc"},
    )
    matches = ((result.get("messages") or {}).get("matches")) or []
    if not matches:
        return f"没有搜到。查询：{query}"
    lines = [f"查询：{query}"]
    for match in matches:
        text = (match.get("text") or "").replace("\n", " ")
        lines.append(
            f"{match.get('ts')}  {match.get('username')}  #{(match.get('channel') or {}).get('name')}  {text[:500]}"
        )
    return "\n".join(lines)


def send_message(channel: str, text: str, thread_ts: str, identity: str) -> str:
    if not text or not text.strip():
        raise RuntimeError("文本是空的，没有发送。")
    token, kind = token_for(load_env(), identity)
    cid = channel_id(token, channel)
    payload = {"channel": cid, "text": text}
    if thread_ts:
        payload["thread_ts"] = thread_ts
    result = slack("chat.postMessage", token, payload)
    who = slack("auth.test", token, {})
    where = thread_ts or "频道"
    return (
        f"已发送。频道 {cid}，位置 {where}，ts {result.get('ts')}，"
        f"身份 {who.get('user')}（{kind}）。"
    )


TOOLS = {
    "slack_status": {
        "description": "查看本机 Slack 令牌是否已写好，以及当前身份。不返回令牌本身。",
        "schema": {"type": "object", "properties": {}, "additionalProperties": False},
        "fn": lambda _a: status(),
    },
    "slack_list_channels": {
        "description": "列出当前身份能看到的公开和私有频道。",
        "schema": {"type": "object", "properties": {}, "additionalProperties": False},
        "fn": lambda _a: list_channels(),
    },
    "slack_find_user": {
        "description": "按显示名、真实姓名或 @用户名 找同事或 bot，返回 Slack 用户 id。",
        "schema": {
            "type": "object",
            "properties": {"user": {"type": "string", "description": "姓名、显示名或 @用户名"}},
            "required": ["user"],
            "additionalProperties": False,
        },
        "fn": lambda a: find_user(a.get("user", "")),
    },
    "slack_read_messages": {
        "description": "读取指定频道的最近消息。可以用 user 只留某个人或某个 bot。",
        "schema": {
            "type": "object",
            "properties": {
                "channel": {"type": "string", "description": "频道名或频道 id，例如 pr"},
                "user": {"type": "string", "description": "可选。只看这个人或 bot"},
                "limit": {"type": "integer", "description": "条数，1 到 50，默认 20"},
            },
            "required": ["channel"],
            "additionalProperties": False,
        },
        "fn": lambda a: read_messages(a.get("channel", ""), a.get("user", ""), a.get("limit") or 20),
    },
    "slack_search_messages": {
        "description": "用用户身份在频道里搜索某个人或某个 bot 的消息。",
        "schema": {
            "type": "object",
            "properties": {
                "channel": {"type": "string"},
                "user": {"type": "string"},
                "limit": {"type": "integer"},
            },
            "additionalProperties": False,
        },
        "fn": lambda a: search_messages(a.get("channel", ""), a.get("user", ""), a.get("limit") or 20),
    },
    "slack_send_message": {
        "description": "往指定频道发一条消息。默认用用户身份，所以显示为授权的那个人。不传文本就不会发。",
        "schema": {
            "type": "object",
            "properties": {
                "channel": {"type": "string"},
                "text": {"type": "string"},
                "thread_ts": {"type": "string", "description": "可选。有它就是回复那条 thread"},
                "as": {"type": "string", "description": "user 或 bot，默认 user"},
            },
            "required": ["channel", "text"],
            "additionalProperties": False,
        },
        "fn": lambda a: send_message(
            a.get("channel", ""), a.get("text", ""), a.get("thread_ts", ""), a.get("as", "user")
        ),
    },
}


def tool_result(text: str, is_error: bool = False) -> dict:
    result = {"content": [{"type": "text", "text": text}]}
    if is_error:
        result["isError"] = True
    return result


def handle(message: dict) -> dict | None:
    method = message.get("method")
    msg_id = message.get("id")
    if msg_id is None:
        return None
    if method == "initialize":
        params = message.get("params") or {}
        version = params.get("protocolVersion") or PROTOCOL
        if version not in SUPPORTED:
            version = PROTOCOL
        return {
            "jsonrpc": "2.0",
            "id": msg_id,
            "result": {
                "protocolVersion": version,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "grok-slack", "version": "0.1.0"},
                "instructions": (
                    "只在用户点名频道和正文之后发送。"
                    f"用户权限范围：{USER_SCOPES}。Bot 权限范围：{BOT_SCOPES}。"
                ),
            },
        }
    if method == "ping":
        return {"jsonrpc": "2.0", "id": msg_id, "result": {}}
    if method == "tools/list":
        tools = []
        for name, spec in TOOLS.items():
            tools.append(
                {
                    "name": name,
                    "description": spec["description"],
                    "inputSchema": spec["schema"],
                }
            )
        return {"jsonrpc": "2.0", "id": msg_id, "result": {"tools": tools}}
    if method == "tools/call":
        params = message.get("params") or {}
        name = params.get("name")
        spec = TOOLS.get(name)
        if spec is None:
            return {
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": tool_result(f"没有这个工具：{name}", True),
            }
        try:
            text = spec["fn"](params.get("arguments") or {})
        except Exception as exc:  # noqa: BLE001 — tool boundary, return the message
            eprint(f"tool {name} failed: {exc}")
            return {"jsonrpc": "2.0", "id": msg_id, "result": tool_result(str(exc), True)}
        return {"jsonrpc": "2.0", "id": msg_id, "result": tool_result(text)}
    return {
        "jsonrpc": "2.0",
        "id": msg_id,
        "error": {"code": -32601, "message": f"Method not found: {method}"},
    }


def read_message() -> dict | None:
    line = sys.stdin.buffer.readline()
    if not line:
        return None
    if line.lower().startswith(b"content-length:"):
        length = int(line.split(b":", 1)[1].strip())
        while True:
            header = sys.stdin.buffer.readline()
            if header in (b"\r\n", b"\n", b""):
                break
        body = sys.stdin.buffer.read(length)
        return json.loads(body.decode("utf-8"))
    return json.loads(line.decode("utf-8"))


def write_message(message: dict) -> None:
    raw = json.dumps(message, ensure_ascii=False).encode("utf-8")
    sys.stdout.buffer.write(raw + b"\n")
    sys.stdout.buffer.flush()


def main() -> None:
    while True:
        try:
            incoming = read_message()
        except json.JSONDecodeError as exc:
            eprint(f"bad json: {exc}")
            continue
        if incoming is None:
            return
        outgoing = handle(incoming)
        if outgoing is not None:
            write_message(outgoing)


if __name__ == "__main__":
    main()

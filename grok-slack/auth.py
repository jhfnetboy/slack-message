#!/usr/bin/env python3
"""Open Slack's Allow page and store the tokens in the mode-600 env file.

Does not print the client secret or the tokens. Run this after the Slack app
has the redirect URL and the scopes listed in SCOPES.
"""

from __future__ import annotations

import json
import os
import secrets
import threading
import time
import urllib.parse
import urllib.request
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ENV_PATH = Path.home() / ".config" / "grok-slack" / "env"
REDIRECT = "http://127.0.0.1:8787/slack/callback"
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


def load_env() -> dict[str, str]:
    values: dict[str, str] = {}
    if not ENV_PATH.exists():
        return values
    for raw in ENV_PATH.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        values[key.strip()] = value.strip().strip("'").strip('"')
    return values


def write_env(updates: dict[str, str]) -> None:
    current = load_env()
    current.update(updates)
    order = [
        "SLACK_APP_ID",
        "SLACK_CLIENT_ID",
        "SLACK_TEAM_ID",
        "SLACK_CLIENT_SECRET",
        "SLACK_USER_TOKEN",
        "SLACK_BOT_TOKEN",
    ]
    lines = [
        "# grok-slack credentials. Mode 600. Do not commit. Do not paste into chat.",
        "# SLACK_CLIENT_SECRET comes from Basic Information → Client Secret → Show.",
        "# SLACK_USER_TOKEN and SLACK_BOT_TOKEN are written by auth.py after Allow.",
    ]
    seen = set()
    for key in order:
        lines.append(f"{key}={current.get(key, '')}")
        seen.add(key)
    for key, value in current.items():
        if key not in seen:
            lines.append(f"{key}={value}")
    ENV_PATH.parent.mkdir(mode=0o700, exist_ok=True)
    tmp = ENV_PATH.with_suffix(".env.tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as handle:
        handle.write("\n".join(lines) + "\n")
    os.chmod(tmp, 0o600)
    os.replace(tmp, ENV_PATH)
    os.chmod(ENV_PATH, 0o600)


class Callback(BaseHTTPRequestHandler):
    result: dict = {"state": "", "code": "", "error": ""}

    def do_GET(self) -> None:  # noqa: N802 — stdlib handler name
        parsed = urllib.parse.urlparse(self.path)
        if parsed.path != "/slack/callback":
            self.send_error(404)
            return
        query = urllib.parse.parse_qs(parsed.query)
        if query.get("state", [""])[0] != self.result["state"]:
            self.result["error"] = "state 不一致，授权已丢弃。"
        elif query.get("error"):
            self.result["error"] = query["error"][0]
        else:
            self.result["code"] = query.get("code", [""])[0]
        body = "Slack 授权已回到这台电脑。可以关掉这个页面。".encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt: str, *args) -> None:
        return


def exchange(client_id: str, client_secret: str, code: str) -> dict:
    form = urllib.parse.urlencode(
        {
            "client_id": client_id,
            "client_secret": client_secret,
            "code": code,
            "redirect_uri": REDIRECT,
        }
    ).encode("utf-8")
    request = urllib.request.Request(
        "https://slack.com/api/oauth.v2.access",
        data=form,
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=20) as response:
        body = json.loads(response.read().decode("utf-8"))
    if not body.get("ok"):
        raise RuntimeError(f"换取令牌失败：{body.get('error', 'unknown')}")
    return body


def main() -> int:
    env = load_env()
    client_id = env.get("SLACK_CLIENT_ID", "")
    client_secret = env.get("SLACK_CLIENT_SECRET", "")
    if not client_id or not client_secret:
        print(f"先把 SLACK_CLIENT_SECRET 写进 {ENV_PATH}，文件权限保持 600。")
        print("不要把 Client Secret 贴进对话。")
        return 1
    state = secrets.token_urlsafe(24)
    Callback.result = {"state": state, "code": "", "error": ""}
    server = ThreadingHTTPServer(("127.0.0.1", 8787), Callback)
    query = urllib.parse.urlencode(
        {
            "client_id": client_id,
            "scope": BOT_SCOPES,
            "user_scope": USER_SCOPES,
            "redirect_uri": REDIRECT,
            "state": state,
        }
    )
    url = f"https://slack.com/oauth/v2/authorize?{query}"
    print("正在打开 Slack 授权页。请点 Allow。")
    print(f"如果浏览器没有打开，用这一条：\n{url}")
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    webbrowser.open(url)
    deadline = time.time() + 180
    while not Callback.result["code"] and not Callback.result["error"]:
        if time.time() > deadline:
            Callback.result["error"] = "等待授权超时"
            break
        time.sleep(0.2)
    server.shutdown()
    server.server_close()
    if Callback.result["error"]:
        print(f"授权没有完成：{Callback.result['error']}")
        return 1
    granted = exchange(client_id, client_secret, Callback.result["code"])
    user = granted.get("authed_user") or {}
    updates = {}
    if granted.get("access_token"):
        updates["SLACK_BOT_TOKEN"] = granted["access_token"]
    if user.get("access_token"):
        updates["SLACK_USER_TOKEN"] = user["access_token"]
    if not updates:
        print("Slack 没有返回令牌。检查应用里的 Bot 和 User 权限是否已经保存。")
        return 1
    write_env(updates)
    print(
        "令牌已写入密钥文件，没有打印出来。"
        f" 用户 {user.get('id', '无')}，bot {'有' if 'SLACK_BOT_TOKEN' in updates else '无'}。"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

# slack-message

手动收取和发送 Slack 消息。默认只看 `#pr` 里 @ 自己的消息。不轮询，不自动回复。

这个仓库没有令牌。不要把任何人的 `env` 复制进来，也不要把令牌贴进对话或提交。

## 目录

| 路径 | 作用 |
|---|---|
| `slack-tui/` | Rust 终端界面。收发、持续扫描、本地 SQLite。 |
| `slack-listen/` | 本机监听。登录后把 `#pr` 的新消息写到本机文件，不调用模型。 |
| `skill/slack-msg/` | 手动 skill。收取和发送都走这里。 |
| `grok-slack/auth.py` | 用自己的 Slack App 打开授权页，把令牌写进本机 `env`。 |
| `grok-slack/server.py` | 给 Grok 会话用的 MCP 程序。当前 skill 不依赖它。 |
| `slack-msg.json.example` | 本机发送默认值。复制后改 `default_at`。 |

## 配置

1. 加入 Slack 工作区，进入 `#pr`。网页登录即可。
2. 在 https://api.slack.com/apps 新建自己的 Slack App。Redirect URL 填 `http://127.0.0.1:8787/slack/callback`。
3. User scopes 和 Bot scopes 与 `grok-slack/auth.py` 里的 `USER_SCOPES`、`BOT_SCOPES` 保持一致。
4. 把 `grok-slack/server.py` 和 `grok-slack/auth.py` 复制到 `~/.config/grok-slack/`。新建 `~/.config/grok-slack/env`，权限 `600`，先只写自己的：

```bash
SLACK_APP_ID=
SLACK_CLIENT_ID=
SLACK_CLIENT_SECRET=
SLACK_TEAM_ID=
```

5. 运行 `python3 ~/.config/grok-slack/auth.py`，用自己的账号点 Allow。这一步写入 `SLACK_USER_TOKEN` 和 `SLACK_BOT_TOKEN`。
6. 打开 Socket Mode。生成 App-Level Token，scope 勾 `connections:write`。值必须写在 `SLACK_APP_TOKEN`，`xapp-` 开头。不要写到 `SLACK_USER_TOKEN`。
7. Event Subscriptions 打开，bot events 增加 `message.channels`、`message.groups`、`app_mention`。Slack 要求重新安装时照做。
8. 在 `#pr` 输入 `/invite @你的bot`。bot 不在频道里时，监听连得上，但收不到这个频道的消息。

## 安装

```bash
git clone https://github.com/jhfnetboy/slack-message.git
cd slack-message
mkdir -p ~/.agents/skills
cp -R skill/slack-msg ~/.agents/skills/slack-msg
cp slack-msg.json.example ~/.config/grok-slack/slack-msg.json
# 把 default_at 改成你要默认 @ 的人，例如 jhfnetboy
bash slack-listen/install.sh
```

监听是用户级 LaunchAgent `com.aastar.slack-listen`。日志在 `~/Library/Logs/slack-listen.log`。状态在 `~/.local/state/slack-listen/status.json`。退出登录后它会停。它不消耗模型额度。

可选配置复制 `slack-listen/listen.example.json` 为 `~/.config/grok-slack/listen.json`。不复制时默认只听 `#pr`。

## 终端界面

界面结构来自 [Ratatui 的 event-driven async 模板](https://github.com/ratatui/templates/tree/main/event-driven-async)。令牌仍只读 `~/.config/grok-slack/env`，不进仓库。

```bash
cd slack-tui
cargo build --release
./target/release/slack-tui
./target/release/slack-tui fetch --mention me --limit 10
./target/release/slack-tui send --at David 已装好，这是测试回复
./target/release/slack-tui watch
./target/release/slack-tui --every 30m
```

不带参数打开界面。启动先显示本地库里的消息，再向 Slack 补新的。数据库在 `~/.local/state/slack-tui/slack.db`。第一次会导入 `~/.local/state/slack-msg/mentions.jsonl`。

曼谷时间 13:30–23:00 每 5 分钟扫描。其余时间没有新消息就从 5 分钟退避到 30 分钟。`--every 30m` 或 `--every 90s` 固定间隔。有别人的新消息时播放提示音。`m` 在门铃和轻响之间切换，只影响这次会话。

`j` / `k` 移动，`r` 回复选中的消息并 @ 对方，`i` 发新消息。新消息没写对象时 @ `slack-msg.json` 里的 `default_at`。空正文不发送。不标已读，不加反应。`?` 看全部按键。线程里的新回复会跟在时间线上，`t` 只看当前线程。

`slack-listen/` 仍是原来的登录后监听，这个界面不替换它。

## 使用

在 skill 目录，或写绝对路径：

```bash
python3 ~/.agents/skills/slack-msg/scripts/slack.py
python3 ~/.agents/skills/slack-msg/scripts/slack.py fetch --channel pr --mention me --limit 10
python3 ~/.agents/skills/slack-msg/scripts/slack.py fetch --mention all
python3 ~/.agents/skills/slack-msg/scripts/slack.py send 已装好，这是测试回复
python3 ~/.agents/skills/slack-msg/scripts/slack.py send --at 其他同学 晚上好
python3 ~/.agents/skills/slack-msg/scripts/slack.py watch
```

`watch` 在你启动它的那个终端里运行，不进 AI 对话。第一次先从本地文件显示最近 10 条，不等 Slack。再用最新一条的时间向 Slack 补新消息。之后报这个频道里的新消息，并追加到 `~/.local/state/slack-msg/mentions.jsonl`。曼谷时间 13:30–23:00 每 5 分钟扫描，有新消息时出声。其余时间没有新消息就从 5 分钟退避到 30 分钟。`watch --30` 或 `watch --every 90s` 固定间隔，并关掉退避。在 `回复>` 后输入文字回车，会回复当前最新一条，并 @ 对方。这条回复会立刻写入本地对话。退出再进仍按时间顺序显示别人说的和你回的。空行不发送。Ctrl-C 停止。

- 不带参数：收取 `#pr` 里 @ 自己的消息。时间是清迈时间（UTC+7）。
- `--channel` 换频道。`--mention all` 看该频道最近消息。`--mention 名字` 只看 @ 某个人的。
- `send` 默认发到 `#pr`。没写 `--at` 时，@ `slack-msg.json` 里的 `default_at`。可以重复 `--at`。
- 没有正文不会发送。脚本不标已读，不加反应。

装好后，请用上面的 `send` 给 `jhfnetboy` 回一条消息，确认你这端能发。

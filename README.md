# slack-message

手动收取和发送 Slack 消息。默认只看 `#pr` 里 @ 自己的消息。不轮询，不自动回复。

这个仓库没有令牌。不要把任何人的 `env` 复制进来，也不要把令牌贴进对话或提交。

## 目录

| 路径 | 作用 |
|---|---|
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

`watch` 在你启动它的那个终端里运行，不进 AI 对话。第一次先显示 `#pr` 最近 10 条历史。之后默认每 7 分钟只报 @ 自己的新消息，直接打在这个终端上，并追加到 `~/.local/state/slack-msg/mentions.jsonl`。`watch --30` 改成 30 分钟，`watch --every 90s` 按秒算。Ctrl-C 停止。

- 不带参数：收取 `#pr` 里 @ 自己的消息。时间是清迈时间（UTC+7）。
- `--channel` 换频道。`--mention all` 看该频道最近消息。`--mention 名字` 只看 @ 某个人的。
- `send` 默认发到 `#pr`。没写 `--at` 时，@ `slack-msg.json` 里的 `default_at`。可以重复 `--at`。
- 没有正文不会发送。脚本不标已读，不加反应。

装好后，请用上面的 `send` 给 `jhfnetboy` 回一条消息，确认你这端能发。

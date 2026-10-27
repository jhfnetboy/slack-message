---
name: slack-msg
description: >
  手动收取或发送 Slack 消息。用户说 slack、slack msg、收 Slack、@我、
  发给 David、send 到 #pr 时使用。默认只收 #pr 里 @ 我的消息。不轮询。
disable-model-invocation: true
---

# Slack 消息

只在用户这一轮明确要求收取或发送时运行。不要在对话里轮询，不要自己开下一轮。定时收取是用户在自己的终端里启动的前台命令，不由这个对话启动。

脚本在本 skill 目录。不要把令牌、env 文件或请求头写进对话、日志或仓库。时间是清迈时间（UTC+7）。

## 收取

默认：`#pr` 里 @ 当前用户的消息。

```bash
python3 scripts/slack.py
python3 scripts/slack.py fetch --channel pr --mention me --limit 10
```

可调参数：

- `--channel`：频道名，默认 `pr`
- `--mention me`：只留 @ 我的。`all` 是该频道最近消息。也可以写别人的显示名
- `--limit`：最多几条，默认 10

把输出整理进当前回复。没有符合条件的消息就说没有。不要假装知道 Slack 官方未读数。

用户要定时收取时，告诉他在自己的终端运行，不要在这个对话里挂着：

```bash
python3 ~/.agents/skills/slack-msg/scripts/slack.py watch
```

第一次启动先显示该频道最近 10 条历史，方便看上下文。之后报这个频道里的新消息，包括 @ 你的、你自己发出的，以及频道里的其他消息。直接打在那个终端上，并追加到 `~/.local/state/slack-msg/mentions.jsonl`。在 `回复>` 后输入文字回车，会回复当前最新一条，并立刻写入本地对话。退出再进仍按时间顺序显示别人说的和你回的。空行不发送。曼谷时间 13:30–23:00 每 5 分钟扫描，有新消息时出声。其余时间没有新消息就从 5 分钟退避到 30 分钟。`--30` 或 `--every 90s` 固定间隔，并关掉退避。

## 发送

只有用户给出要发的正文时才发送。默认发到 `#pr`，并 @ David。用户点名其他人时加 `--at`，可以重复。

```bash
python3 scripts/slack.py send --at David 正文
python3 scripts/slack.py send --at David --at 其他同学 正文
python3 scripts/slack.py send --channel pr --at David 正文
```

没写 `--at` 时脚本自己 @ David。发送后把脚本打印的正文和结果显示在当前对话。不要改写成另一句话。

## 不做的事

- 用户没给正文时不发送。
- 不标已读，不加反应。
- 不把监听 `trigger` 指到会启动模型或代发 Slack 的命令。
- 用户没有点名要处理某条消息时，只显示，不继续打开链接或改代码。

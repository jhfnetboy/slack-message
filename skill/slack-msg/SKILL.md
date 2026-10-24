---
name: slack-msg
description: >
  手动收取或发送 Slack 消息。用户说 slack、slack msg、收 Slack、@我、
  send 到 #pr 时使用。默认只收 #pr 里 @ 我的消息。不轮询。
disable-model-invocation: true
---

# Slack 消息

只在用户这一轮明确要求收取或发送时运行。不要设定时扫描，不要后台轮询，不要自己开下一轮。

脚本在本 skill 目录。不要把令牌、env 文件或请求头写进对话、日志或仓库。时间是清迈时间（UTC+7）。

安装和配置看仓库根目录的 `README.md`：https://github.com/jhfnetboy/slack-message

## 收取

默认：`#pr` 里 @ 当前用户的消息。

```bash
python3 scripts/slack.py
python3 scripts/slack.py fetch --channel pr --mention me --limit 10
```

可调参数：

- `--channel`：频道名，默认 `pr`，也可在 `~/.config/grok-slack/slack-msg.json` 改 `default_channel`
- `--mention me`：只留 @ 我的。`all` 是该频道最近消息。也可以写别人的显示名
- `--limit`：最多几条，默认 10

把输出整理进当前回复。没有符合条件的消息就说没有。不要假装知道 Slack 官方未读数。

## 发送

只有用户给出要发的正文时才发送。默认发到 `#pr`。没写 `--at` 时，@ `slack-msg.json` 里的 `default_at`。用户点名其他人时加 `--at`，可以重复。

```bash
python3 scripts/slack.py send 正文
python3 scripts/slack.py send --at jhfnetboy 正文
python3 scripts/slack.py send --at jhfnetboy --at 其他同学 正文
```

发送后把脚本打印的正文和结果显示在当前对话。不要改写成另一句话。

## 不做的事

- 用户没给正文时不发送。
- 不标已读，不加反应。
- 不把监听 `trigger` 指到会启动模型或代发 Slack 的命令。
- 用户没有点名要处理某条消息时，只显示，不继续打开链接或改代码。

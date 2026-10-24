# 交接 2026-10-24

下一棒从这里接。不要猜未读数，不要复制任何人的 `env`，不要在已批准的 iDoris PR 上追加 commit。

## 现在该做什么

1. 用户自己的终端里重新运行 `slack`。正在跑的旧进程不会热更新。
2. 等 David 按仓库装好后，用 skill 回「已装好，收发测试通过」。收到才算收发测试完成。
3. iDoris **#577** 还开着，外审是 `CHANGES_REQUESTED`，不能合并。

## Slack

- 仓库：https://github.com/jhfnetboy/slack-message
- 本地：`/Users/jason/Dev/jhfnetboy/slack-message`
- 正在用的 skill：`~/.agents/skills/slack-msg`
- 命令：`slack`，定义在 `~/.zshrc`，实际是 `python3 ~/.agents/skills/slack-msg/scripts/slack.py watch`
- 最新提交：`5e70d4e`，`main` 已推到 `origin/main`
- 密钥只在 `~/.config/grok-slack/env`，权限应保持 `600`。仓库和交接文档里都没有令牌。
- 本机默认 @ 的人：`~/.config/grok-slack/slack-msg.json` 里的 `default_at` 是 David。

### 行为

- 不带参数：收 `#pr` 里 @ 自己的消息。
- `watch`：在启动它的终端里跑，不进 AI 对话。先从 `~/.local/state/slack-msg/mentions.jsonl` 立刻显示最近 10 条，再用最新时间向 Slack 补新消息。之后默认每 7 分钟只报新的 @ 自己的消息。每条历史下面有分隔线。
- `watch --30`：改成 30 分钟。`watch --every 90s`：按秒。
- Ctrl-C：只打印「已停止。」
- `send`：默认发到 `#pr` 并 @ David。没有正文不会发。
- 文件：`~/.local/state/slack-msg/mentions.jsonl`、`seen.txt`、`channels.json`、`names.json`。
- 监听进程 `com.aastar.slack-listen` 仍在跑，只写 `~/.local/state/slack-listen/events.jsonl`，不调用模型。

### 已经发给 David 的

`#pr` 里已经 @ David：仓库地址、不要复制 env、装好后用 skill 回信、以及后来的更新说明。20:17 他问过只有网页版 Slack 时怎么完成。完整收发测试还没完成。

## iDoris v0.3.1

目标是 current `origin/main`，不是旧栈，不是 v0。外审是仓库外的 clestons。不要启动本机 review watcher。

已合并、行为在 main 上的：#572、#573、#574、#575、#576。对应旧 PR #447、#477、#531、#448、见证测试已处理。

已关闭、不重放旧栈的：

| PR | 原因 |
|---|---|
| #443 | `ProviderGuard::finish` 已记录本地模型失败。断言在 `breaker_wiring_tests.rs` 的 `organic_native_faults_fail_over_to_a_healthy_native_and_record_decided_provider`。 |
| #448 | 第四路折叠已由 #575 进入 main。 |
| #461 | `BreakerKey::Provider` 与 `Connection` 已分开，`admit_pair` 先检查两道闸。旧栈冲突。 |
| #450 | 短数字列表已被形状白名单拒绝。不搬 `MIN_GROUPED_RUN = 3`，它会误伤 1–4 位卡号尾组。 |
| #452 #453 | `12 6222 0212 3456 7894` 已要求从下标 3 命中。不替换当前扫描器。 |
| #445 | 旧栈关闭。缺口改由 #577 移植。 |

仍开着的只有 **#577**：https://github.com/iDoris-ai/iDoris/pull/577

- 分支：`jhfnetboy/v031-port-445`
- head：`5a7237871e74e05281b44202a3f3224e41a4b14e`
- 状态：`CHANGES_REQUESTED`，不能合并。
- 内容：不透明内容把 `any` 收到本地。外审要求只把 allow-list 用在 `content` 的直接子块，避免 `cache_control`、`thinking`、`tool_use.input` 被误钉。修复已推，等新的外审。
- 工作区已删。分支还在，因为 PR 还开着。

`rust` CI 的 `RUSTSEC-2026-0386` 在 main 上同样红，不是这些 PR 引入的。规则集不把它当必需检查。必需的是 clestons 对精确 head 的 APPROVE，加上可合并。

## 清理

- 已删的远端分支：`feat/resilience-06-local-model-lockout`、`feat/inspect-16-homoglyph-folding`、`feat/event-log-32-protocol-finalization`、`fix/review-5-hardening`、`fix/inspect-17-window-segment-floor`、`fix/inspect-19-third-review`、`fix/inspect-20-grouping-recall`、`fix/inspect-13-review-hardening`，以及本轮建过又删的临时分支。
- 已删的工作区：本轮移植用的 v031-port-* 卡片，包括 `v031-port-445`。当前协调者窗口 `tools/slack-listen` 保留。
- 没有删 `jhfnetboy/v031-port-445`。那是 #577 的头。
- 没有批量删除 iDoris 里其他本地旧分支。那些不是这一棒证明过可以丢的分支。
- Mac Mini 这台主机本次清单没有返回，不能说那边的卡片是空的。

## 不要做的事

- 不要把 `env` 或令牌发给 David，不要提交进仓库。
- 不要在对话里挂 `watch`。它属于用户自己的终端。
- 不要合并 #577，除非 clestons 批准的是当时的精确 head。
- 不要重放已关闭的旧栈。

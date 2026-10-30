use std::io::{self, Write};

use clap::{Parser, Subcommand};
use tokio::io::{AsyncBufReadExt, BufReader};

use slack_tui::app::{App, AppArgs};
use slack_tui::config::Config;
use slack_tui::db::Store;
use slack_tui::model::{Message, mention_label, prettify, redact};
use slack_tui::schedule::{ScanMode, parse_interval};
use slack_tui::slack::SlackClient;
use slack_tui::sync::{fetch_matching, send_text, sync_channel};
use slack_tui::timeutil::{bangkok_now, format_clock, format_ts, is_work_hours, play_alert_sound};

#[derive(Parser)]
#[command(
    name = "slack-tui",
    version,
    about = "Slack 终端界面。默认打开 TUI，持续扫描并写入本地数据库。"
)]
struct Cli {
    /// 频道名。默认来自 slack-msg.json，否则是 pr
    #[arg(short, long, global = true)]
    channel: Option<String>,
    /// 固定扫描间隔：30、30m 或 90s。写了就关闭自动退避
    #[arg(long, global = true)]
    every: Option<String>,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// 收取最近消息。默认 #pr 里 @ 你的
    Fetch {
        /// me、all，或某个人的名字
        #[arg(long, default_value = "me")]
        mention: String,
        /// 最多几条，1 到 30
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// 发送一条消息。没写 --at 时 @ 配置里的 default_at
    Send {
        /// 要 @ 的人，可以重复
        #[arg(long)]
        at: Vec<String>,
        /// 正文
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        text: Vec<String>,
    },
    /// 在这个终端持续打印新消息，回车回复最新一条
    Watch,
    /// 向 Slack 扫描一次
    Poll,
}

struct Ctx {
    cfg: Config,
    db: Store,
    slack: SlackClient,
    my_id: String,
    my_name: String,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{}", redact(&error));
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    color_eyre::install().map_err(|error| error.to_string())?;
    let cli = Cli::parse();
    let cfg = Config::load()?;
    let db = Store::open(&cfg.db_path)?;
    let imported = db.import_legacy(&cfg.legacy_dir)?;
    if imported > 0 {
        eprintln!("已从旧记录导入 {imported} 条。");
    }
    let slack = SlackClient::new(cfg.token.clone())?;
    let (my_id, my_name) = slack.auth_test().await?;
    db.save_user(&my_id, &my_name, &my_name, &my_name, false)?;
    let ctx = Ctx {
        cfg,
        db,
        slack,
        my_id,
        my_name,
    };
    let channel = cli
        .channel
        .clone()
        .unwrap_or_else(|| ctx.cfg.default_channel.clone());
    let every = match &cli.every {
        Some(text) => Some(parse_interval(text)?),
        None => None,
    };
    match cli.command {
        None => run_tui(&ctx, &channel, every).await,
        Some(Cmd::Fetch { mention, limit }) => fetch(&ctx, &channel, &mention, limit).await,
        Some(Cmd::Send { at, text }) => send(&ctx, &channel, at, text.join(" ")).await,
        Some(Cmd::Watch) => watch(&ctx, &channel, every).await,
        Some(Cmd::Poll) => poll(&ctx, &channel).await,
    }
}

async fn run_tui(ctx: &Ctx, channel: &str, every: Option<u64>) -> Result<(), String> {
    let (channel_id, channel_name) = ctx.slack.resolve_channel(&ctx.db, channel).await?;
    let messages = ctx.db.recent(&channel_id, &channel_name, 300)?;
    let names = ctx.db.user_names()?;
    let (scan_mode, fixed_secs) = match every {
        Some(seconds) => (ScanMode::Fixed, seconds),
        None => (ScanMode::Adaptive, 300),
    };
    let app = App::new(AppArgs {
        db: ctx.db.clone(),
        slack: ctx.slack.clone(),
        my_id: ctx.my_id.clone(),
        my_name: ctx.my_name.clone(),
        default_at: ctx.cfg.default_at.clone(),
        channel_id,
        channel_name,
        messages,
        names,
        sound: ctx.cfg.sound,
        scan_mode,
        fixed_secs,
        db_path: ctx.cfg.db_path.display().to_string(),
    });
    let terminal = ratatui::init();
    let result = app.run(terminal).await;
    ratatui::restore();
    result
}

async fn fetch(ctx: &Ctx, channel: &str, mention: &str, limit: usize) -> Result<(), String> {
    let limit = limit.clamp(1, 30);
    let (channel_id, channel_name) = ctx.slack.resolve_channel(&ctx.db, channel).await?;
    let wanted = resolve_mention(ctx, mention).await?;
    let (messages, scanned) = fetch_matching(
        &ctx.slack,
        &ctx.db,
        &channel_id,
        &channel_name,
        &ctx.my_id,
        mention,
        &wanted,
        limit,
    )
    .await?;
    let names = ctx.db.user_names()?;
    println!(
        "#{channel_name}  {}  清迈时间  扫描 {scanned} 条",
        mention_label(mention)
    );
    if messages.is_empty() {
        println!("没有符合条件的消息。");
        return Ok(());
    }
    for message in messages {
        println!(
            "{}  {}  {}",
            format_ts(&message.ts),
            message.user_name,
            prettify(&message.text, &names)
        );
    }
    Ok(())
}

async fn send(ctx: &Ctx, channel: &str, mut ats: Vec<String>, text: String) -> Result<(), String> {
    if ats.is_empty() && !ctx.cfg.default_at.is_empty() {
        ats.push(ctx.cfg.default_at.clone());
    }
    let (channel_id, channel_name) = ctx.slack.resolve_channel(&ctx.db, channel).await?;
    let mut mention_ids = Vec::new();
    for name in ats {
        mention_ids.push(ctx.slack.user_id(&ctx.db, &name).await?);
    }
    let message = send_text(
        &ctx.slack,
        &ctx.db,
        &channel_id,
        &channel_name,
        &ctx.my_id,
        &ctx.my_name,
        &text,
        &mention_ids,
        None,
    )
    .await?;
    println!(
        "已发送到 #{channel_name}。身份 {}。ts {}",
        ctx.my_name, message.ts
    );
    println!("{}", message.text);
    Ok(())
}

async fn poll(ctx: &Ctx, channel: &str) -> Result<(), String> {
    let (channel_id, channel_name) = ctx.slack.resolve_channel(&ctx.db, channel).await?;
    let report = sync_channel(&ctx.slack, &ctx.db, &channel_id, &channel_name, &ctx.my_id).await?;
    let stamp = format_clock(&bangkok_now());
    if report.inserted.is_empty() {
        println!("{stamp}  没有新的消息。扫描 {} 条。", report.scanned);
        if report.catching_up {
            println!("还有更早的消息，下次继续补。");
        }
        return Ok(());
    }
    if !report.initial
        && report
            .inserted
            .iter()
            .any(|message| message.user_id != ctx.my_id)
    {
        play_alert_sound(ctx.cfg.sound);
    }
    for message in report.inserted {
        print_message(&message);
    }
    if report.catching_up {
        println!("还有更早的消息，下次继续补。");
    }
    Ok(())
}

async fn watch(ctx: &Ctx, channel: &str, every: Option<u64>) -> Result<(), String> {
    let (channel_id, channel_name) = ctx.slack.resolve_channel(&ctx.db, channel).await?;
    let mode = if every.is_some() {
        ScanMode::Fixed
    } else {
        ScanMode::Adaptive
    };
    let fixed = every.unwrap_or(300);
    let mut interval = fixed;
    emit(&format!(
        "监听已启动。先显示最近 10 条，收取 #{channel_name} 的新消息。结果写入 {}。Ctrl-C 停止。",
        ctx.cfg.db_path.display()
    ));
    if mode == ScanMode::Adaptive {
        emit(
            "扫描规则：曼谷时间 13:30–23:00 每 5 分钟；其他时间没有新消息就从 5 分钟退避到 30 分钟。",
        );
    }
    let cached = ctx.db.recent(&channel_id, &channel_name, 10)?;
    emit(&format!(
        "#{channel_name}  本地对话，最近 {} 条  清迈时间",
        cached.len()
    ));
    if cached.is_empty() {
        emit("本地还没有这个频道的记录，正在向 Slack 取。");
    }
    for message in &cached {
        print_message(message);
    }
    let mut latest = cached.into_iter().next_back();
    match sync_channel(&ctx.slack, &ctx.db, &channel_id, &channel_name, &ctx.my_id).await {
        Ok(report) => {
            for message in report.inserted {
                print_message(&message);
                latest = Some(message);
            }
            if report.catching_up {
                emit("还有更早的消息，下次继续补。");
            }
        }
        Err(error) => emit(&format!("扫描出错：{}", redact(&error))),
    }
    emit("回复最新一条后回车。空行不发送。Ctrl-C 停止。");
    let mut deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(interval);
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                emit("已停止。");
                return Ok(());
            }
            _ = tokio::time::sleep_until(deadline) => {
                match sync_channel(&ctx.slack, &ctx.db, &channel_id, &channel_name, &ctx.my_id).await {
                    Ok(report) => {
                        let had_new = !report.inserted.is_empty();
                        if !report.initial && had_new && report.inserted.iter().any(|message| message.user_id != ctx.my_id) {
                            play_alert_sound(ctx.cfg.sound);
                        }
                        if report.inserted.is_empty() {
                            emit(&format!("{}  没有新的消息。扫描 {} 条。", format_clock(&bangkok_now()), report.scanned));
                        }
                        for message in report.inserted {
                            print_message(&message);
                            latest = Some(message);
                        }
                        if report.catching_up {
                            emit("还有更早的消息，下次继续补。");
                        }
                        interval = next_wait(mode, interval, fixed, had_new);
                    }
                    Err(error) => emit(&format!("扫描出错：{}", redact(&error))),
                }
                deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(interval);
            }
            line = lines.next_line() => {
                match line {
                    Ok(None) => {
                        emit("已停止。");
                        return Ok(());
                    }
                    Ok(Some(text)) => {
                        if text.trim().is_empty() {
                            continue;
                        }
                        let Some(target) = latest.clone() else {
                            emit("还没有可回复的消息。");
                            continue;
                        };
                        let thread = if target.thread_ts.is_empty() { target.ts.clone() } else { target.thread_ts.clone() };
                        let mentions = if target.user_id.is_empty() { Vec::new() } else { vec![target.user_id.clone()] };
                        match send_text(&ctx.slack, &ctx.db, &channel_id, &channel_name, &ctx.my_id, &ctx.my_name, &text, &mentions, Some(&thread)).await {
                            Ok(message) => {
                                print_message(&message);
                                emit("已记入本地数据库。");
                                latest = Some(message);
                            }
                            Err(error) => emit(&format!("没有发送：{}", redact(&error))),
                        }
                    }
                    Err(error) => return Err(redact(&error.to_string())),
                }
            }
        }
    }
}

fn next_wait(mode: ScanMode, current: u64, fixed: u64, had_new: bool) -> u64 {
    slack_tui::schedule::next_interval(mode, is_work_hours(&bangkok_now()), had_new, current, fixed)
}

async fn resolve_mention(ctx: &Ctx, mention: &str) -> Result<String, String> {
    match mention {
        "all" | "" | "me" | "我" => Ok(String::new()),
        name => ctx.slack.user_id(&ctx.db, name).await,
    }
}

fn print_message(message: &Message) {
    let kind = if message.kind.is_empty() {
        String::new()
    } else {
        format!("{}  ", message.kind)
    };
    emit(&format!(
        "{}  {kind}{}  {}",
        format_ts(&message.ts),
        message.user_name,
        message.text
    ));
    emit(&"─".repeat(48));
}

fn emit(line: &str) {
    let mut stdout = io::stdout();
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}

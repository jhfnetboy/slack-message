use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, List, ListItem, Paragraph, Wrap};

use crate::app::{App, InputMode};
use crate::model::{prettify, split_at_cursor};
use crate::schedule::ScanMode;
use crate::timeutil::{format_now, format_ts, is_work_hours};

pub fn draw(frame: &mut ratatui::Frame, app: &mut App) {
    if app.help {
        let body = Layout::vertical([
            Constraint::Length(4),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(frame.area());
        frame.render_widget(header(app), body[0]);
        frame.render_widget(help(app), body[1]);
        frame.render_widget(footer(app), body[2]);
        return;
    }

    let chunks = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(5),
        Constraint::Length(6),
        Constraint::Length(3),
    ])
    .split(frame.area());

    frame.render_widget(header(app), chunks[0]);

    let rows = {
        let visible = app.visible();
        visible
            .into_iter()
            .map(|message| {
                let fresh = app.is_fresh(message);
                let text = prettify(&message.text, &app.names);
                let mark = if fresh { "● " } else { "  " };
                let reply = if !message.thread_ts.is_empty() && message.thread_ts != message.ts {
                    "↳ "
                } else {
                    ""
                };
                let kind = if message.kind.is_empty() {
                    "#".to_string()
                } else {
                    message.kind.clone()
                };
                ListItem::new(Line::from(vec![
                    Span::raw(mark),
                    Span::raw(reply),
                    Span::styled(format!("{} ", format_ts(&message.ts)), Style::new().dim()),
                    Span::styled(format!("{kind} "), kind_style(&kind)),
                    Span::styled(
                        format!("{} ", message.user_name),
                        Style::new().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(text),
                ]))
            })
            .collect::<Vec<_>>()
    };
    let title = format!(
        " 消息 {} 条 · {} · j/k 移动 ",
        rows.len(),
        app.filter.label()
    );
    let list = List::new(rows)
        .block(block(&title))
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(list, chunks[1], &mut app.list_state);
    frame.render_widget(detail(app), chunks[2]);
    frame.render_widget(footer(app), chunks[3]);
}

fn header(app: &App) -> Paragraph<'static> {
    let schedule = match app.scan_mode {
        ScanMode::Fixed => format!("固定 {} 秒", app.fixed_secs),
        ScanMode::Adaptive if is_work_hours(&app.now) => "曼谷 13:30–23:00 每 5 分钟".to_string(),
        ScanMode::Adaptive => "其余时间无消息则 5→30 分钟".to_string(),
    };
    let countdown = if app.scanning {
        "扫描中".to_string()
    } else {
        format_duration(app.countdown_secs)
    };
    let lines = vec![
        Line::from(vec![
            Span::styled(
                format!("#{}  ", app.channel_name),
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{}  ", app.my_name)),
            Span::styled(format_now(&app.now), Style::new().dim()),
            Span::raw(format!(
                "  @你 {}  声音 {}  筛选 {}",
                app.mention_count(),
                app.sound.label(),
                app.filter.label()
            )),
        ]),
        Line::from(vec![
            Span::raw(format!(
                "{schedule}  间隔 {}  下次 {countdown}  ",
                format_duration(app.interval_secs)
            )),
            Span::raw(app.status.clone()),
        ]),
    ];
    Paragraph::new(lines).block(block(" slack-tui "))
}

fn detail(app: &App) -> Paragraph<'static> {
    let Some(message) = app.selected_message() else {
        return Paragraph::new("没有选中的消息。").block(block(" 选中 "));
    };
    let text = prettify(&message.text, &app.names);
    let mut lines = vec![
        Line::from(format!(
            "{}  {}  {}",
            format_ts(&message.ts),
            message.kind,
            message.user_name
        )),
        Line::from(text),
    ];
    if !message.thread_ts.is_empty() && message.thread_ts != message.ts {
        lines.push(Line::from(format!("线程 {}", message.thread_ts)));
    }
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(block(" 选中 "))
}

fn footer(app: &App) -> Paragraph<'static> {
    let (title, text) = match app.input_mode {
        InputMode::Normal if app.help => (" 帮助 ", "Esc 关闭帮助    q 退出".to_string()),
        InputMode::Normal => (
            " 操作 ",
            "j/k 移动  r 回复  i 新消息  t 线程  c 频道  / 搜索  f 筛选  s 刷新  m 声音  ? 帮助  q 退出"
                .to_string(),
        ),
        InputMode::Reply => {
            let who = app
                .selected_message()
                .map(|message| message.user_name)
                .unwrap_or_else(|| "对方".into());
            (" 回复 ", format!("回复 @{who}  {}", painted_input(app)))
        }
        InputMode::Compose => {
            let at = if app.default_at.is_empty() {
                String::new()
            } else {
                format!(" @{}", app.default_at)
            };
            (
                " 新消息 ",
                format!("#{}{at}  {}", app.channel_name, painted_input(app)),
            )
        }
        InputMode::Channel => (" 频道 ", painted_input(app)),
        InputMode::Search => (" 搜索 ", painted_input(app)),
    };
    Paragraph::new(text).block(block(title))
}

fn help(app: &App) -> Paragraph<'static> {
    let text = format!(
        "本地数据库  {}\n\
         启动先读本地，再向 Slack 补这个频道的消息。一次没取完的中间消息，下次继续补。\n\
         线程里的新回复会跟在时间线上。t 只看当前线程，再按一次回到全部。回复进线程并 @ 对方，新消息 @ 配置里的 default_at。\n\
         空正文不会发送。不标已读，不加反应，不把令牌写进界面。\n\
         曼谷时间 13:30–23:00 每 5 分钟扫描并在有别人的新消息时出声。其余时间没有新消息就 5、10、15…直到 30 分钟。\n\
         --every 固定间隔，例如 --every 30m 或 --every 90s。\n\
         j/k 或方向键移动，g/G 到顶到底，Ctrl-u / Ctrl-d 翻页。\n\
         输入时左右方向键移动光标，Home / End 到头尾，Backspace 删前面的字，Delete 删后面的字。\n\
         Esc 取消，Enter 发送，Ctrl-u 清空，Ctrl-w 删掉光标前的一个词。\n\
         筛选在本地记录里进行：全部、@你、你发出，或 / 搜索。● 是这次打开后新到的消息。",
        app.db_path
    );
    Paragraph::new(text)
        .wrap(Wrap { trim: false })
        .block(block(" 帮助 "))
}

fn painted_input(app: &App) -> String {
    let (head, tail) = split_at_cursor(&app.input, app.cursor);
    format!("{head}█{tail}")
}

fn block(title: &str) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title.to_string())
}

fn kind_style(kind: &str) -> Style {
    if kind == "@你" {
        Style::new().add_modifier(Modifier::BOLD)
    } else if kind == "你发出" {
        Style::new().dim()
    } else {
        Style::new()
    }
}

fn format_duration(seconds: u64) -> String {
    if seconds >= 60 && seconds % 60 == 0 {
        format!("{} 分钟", seconds / 60)
    } else if seconds >= 60 {
        format!("{} 分 {} 秒", seconds / 60, seconds % 60)
    } else {
        format!("{seconds} 秒")
    }
}

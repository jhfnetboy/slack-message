use std::collections::{HashMap, HashSet};

use chrono::{DateTime, FixedOffset};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;

use crate::db::Store;
use crate::event::{AppEvent, Event, EventHandler};
use crate::model::{
    Message, backspace_at, char_len, delete_at, delete_word, insert_at, move_cursor, redact,
    thread_root,
};
use crate::schedule::{MIN_SECS, ScanMode, next_interval};
use crate::slack::SlackClient;
use crate::sync::{send_text, sync_channel, sync_thread};
use crate::timeutil::{SoundMode, bangkok_now, is_work_hours, play_alert_sound};
use crate::ui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Reply,
    Compose,
    Channel,
    Search,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Filter {
    All,
    Mentions,
    Mine,
    Search(String),
    Thread(String),
}

impl Filter {
    pub fn label(&self) -> String {
        match self {
            Self::All => "全部".to_string(),
            Self::Mentions => "@你".to_string(),
            Self::Mine => "你发出".to_string(),
            Self::Search(query) => format!("搜索 {query}"),
            Self::Thread(_) => "线程".to_string(),
        }
    }

    fn matches(&self, message: &Message, my_id: &str) -> bool {
        match self {
            Self::All => true,
            Self::Mentions => {
                message.kind == "@你"
                    || (!my_id.is_empty() && message.text.contains(&format!("<@{my_id}>")))
            }
            Self::Mine => {
                message.kind == "你发出" || (!my_id.is_empty() && message.user_id == my_id)
            }
            Self::Search(query) => {
                let query = query.to_lowercase();
                message.text.to_lowercase().contains(&query)
                    || message.user_name.to_lowercase().contains(&query)
            }
            Self::Thread(root) => thread_root(&message.thread_ts, &message.ts) == root,
        }
    }
}

pub struct App {
    pub running: bool,
    pub events: EventHandler,
    pub db: Store,
    pub slack: SlackClient,
    pub my_id: String,
    pub my_name: String,
    pub default_at: String,
    pub channel_id: String,
    pub channel_name: String,
    pub messages: Vec<Message>,
    pub list_state: ListState,
    pub names: HashMap<String, String>,
    pub input: String,
    pub cursor: usize,
    pub input_mode: InputMode,
    pub help: bool,
    pub filter: Filter,
    pub status: String,
    pub sound: SoundMode,
    pub scan_mode: ScanMode,
    pub fixed_secs: u64,
    pub interval_secs: u64,
    pub countdown_secs: u64,
    pub scanning: bool,
    pub generation: u64,
    pub fresh: HashSet<String>,
    pub db_path: String,
    pub now: DateTime<FixedOffset>,
}

pub struct AppArgs {
    pub db: Store,
    pub slack: SlackClient,
    pub my_id: String,
    pub my_name: String,
    pub default_at: String,
    pub channel_id: String,
    pub channel_name: String,
    pub messages: Vec<Message>,
    pub names: HashMap<String, String>,
    pub sound: SoundMode,
    pub scan_mode: ScanMode,
    pub fixed_secs: u64,
    pub db_path: String,
}

impl App {
    pub fn new(args: AppArgs) -> Self {
        let mut list_state = ListState::default();
        if !args.messages.is_empty() {
            list_state.select(Some(args.messages.len() - 1));
        }
        let interval = if args.scan_mode == ScanMode::Fixed {
            args.fixed_secs
        } else {
            MIN_SECS
        };
        Self {
            running: true,
            events: EventHandler::new(),
            db: args.db,
            slack: args.slack,
            my_id: args.my_id,
            my_name: args.my_name,
            default_at: args.default_at,
            channel_id: args.channel_id,
            channel_name: args.channel_name,
            messages: args.messages,
            list_state,
            names: args.names,
            input: String::new(),
            cursor: 0,
            input_mode: InputMode::Normal,
            help: false,
            filter: Filter::All,
            status: "先显示本地记录，再向 Slack 补新消息。".to_string(),
            sound: args.sound,
            scan_mode: args.scan_mode,
            fixed_secs: args.fixed_secs,
            interval_secs: interval,
            countdown_secs: interval,
            scanning: false,
            generation: 0,
            fresh: HashSet::new(),
            db_path: args.db_path,
            now: bangkok_now(),
        }
    }

    pub async fn run(mut self, mut terminal: ratatui::DefaultTerminal) -> Result<(), String> {
        self.spawn_scan();
        while self.running {
            terminal
                .draw(|frame| ui::draw(frame, &mut self))
                .map_err(|error| redact(&error.to_string()))?;
            match self.events.next().await? {
                Event::Tick => self.tick(),
                Event::Crossterm(crossterm::event::Event::Key(key))
                    if key.kind == crossterm::event::KeyEventKind::Press =>
                {
                    self.handle_key(key);
                }
                Event::Crossterm(_) => {}
                Event::App(event) => self.handle_app(event),
            }
        }
        Ok(())
    }

    fn tick(&mut self) {
        self.now = bangkok_now();
        if self.scanning {
            return;
        }
        if self.scan_mode == ScanMode::Adaptive
            && is_work_hours(&self.now)
            && self.interval_secs != MIN_SECS
        {
            self.interval_secs = MIN_SECS;
            self.countdown_secs = self.countdown_secs.min(MIN_SECS);
        }
        if self.countdown_secs > 0 {
            self.countdown_secs -= 1;
        }
        if self.countdown_secs == 0 {
            self.spawn_scan();
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit();
            return;
        }
        if self.input_mode != InputMode::Normal {
            self.handle_input(key);
            return;
        }
        if self.help {
            match key.code {
                KeyCode::Esc | KeyCode::Char('?') => self.help = false,
                KeyCode::Char('q') => self.quit(),
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit(),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Home | KeyCode::Char('g') => self.move_to(0),
            KeyCode::End | KeyCode::Char('G') => self.follow_bottom(),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_by(-10)
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => self.move_by(10),
            KeyCode::Enter | KeyCode::Char('r') => self.begin_reply(),
            KeyCode::Char('i') | KeyCode::Char('n') => self.begin_compose(),
            KeyCode::Char('c') => self.begin_channel(),
            KeyCode::Char('/') => self.begin_search(),
            KeyCode::Char('f') => self.cycle_filter(),
            KeyCode::Char('t') => self.toggle_thread(),
            KeyCode::Char('s') => self.spawn_scan(),
            KeyCode::Char('m') => self.toggle_sound(),
            KeyCode::Char('?') => self.help = true,
            _ => {}
        }
    }

    fn handle_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.reset_input();
                self.input_mode = InputMode::Normal;
                self.status = "已取消。".to_string();
            }
            KeyCode::Enter => self.submit_input(),
            KeyCode::Left => self.cursor = move_cursor(&self.input, self.cursor, -1),
            KeyCode::Right => self.cursor = move_cursor(&self.input, self.cursor, 1),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = char_len(&self.input),
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => self.cursor = 0,
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = char_len(&self.input)
            }
            KeyCode::Backspace => backspace_at(&mut self.input, &mut self.cursor),
            KeyCode::Delete => delete_at(&mut self.input, &mut self.cursor),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.reset_input()
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                delete_word(&mut self.input, &mut self.cursor)
            }
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                insert_at(&mut self.input, &mut self.cursor, ch);
            }
            _ => {}
        }
    }

    fn submit_input(&mut self) {
        let text = self.input.trim().to_string();
        match self.input_mode {
            InputMode::Search => {
                self.filter = if text.is_empty() {
                    Filter::All
                } else {
                    Filter::Search(text)
                };
                self.reset_input();
                self.input_mode = InputMode::Normal;
                self.clamp_selection();
                self.status = format!("筛选：{}", self.filter.label());
            }
            InputMode::Channel => {
                self.reset_input();
                self.input_mode = InputMode::Normal;
                if text.is_empty() {
                    self.status = "没有频道名，没有切换。".to_string();
                    return;
                }
                self.spawn_switch(text);
            }
            InputMode::Reply | InputMode::Compose => {
                if text.is_empty() {
                    self.status = "没有正文，没有发送。".to_string();
                    return;
                }
                let mode = self.input_mode;
                let selected = if mode == InputMode::Reply {
                    self.selected_message()
                } else {
                    None
                };
                if mode == InputMode::Reply && selected.is_none() {
                    self.status = "还没有可回复的消息。".to_string();
                    return;
                }
                self.reset_input();
                self.input_mode = InputMode::Normal;
                self.spawn_send(text, mode, selected);
            }
            InputMode::Normal => {}
        }
    }

    fn handle_app(&mut self, event: AppEvent) {
        match event {
            AppEvent::ScanDone { generation, result } => self.on_scan_done(generation, result),
            AppEvent::ChannelReady { generation, result } => {
                self.on_channel_ready(generation, result)
            }
            AppEvent::SendDone(result) => self.on_send_done(result),
            AppEvent::ThreadDone { generation, result } => self.on_thread_done(generation, result),
        }
    }

    fn on_scan_done(&mut self, generation: u64, result: Result<crate::sync::SyncOut, String>) {
        if generation != self.generation {
            return;
        }
        self.scanning = false;
        self.now = bangkok_now();
        match result {
            Ok(report) => {
                let had_new = !report.inserted.is_empty();
                if !report.initial
                    && report
                        .inserted
                        .iter()
                        .any(|message| message.user_id != self.my_id)
                {
                    play_alert_sound(self.sound);
                }
                if !report.initial {
                    for message in &report.inserted {
                        self.fresh
                            .insert(fresh_key(&message.channel_id, &message.ts));
                    }
                }
                self.reload_messages(false);
                self.interval_secs = next_interval(
                    self.scan_mode,
                    is_work_hours(&self.now),
                    had_new,
                    self.interval_secs,
                    self.fixed_secs,
                );
                self.countdown_secs = self.interval_secs;
                let mut status = if had_new {
                    format!(
                        "新增 {} 条，扫描 {} 条。",
                        report.inserted.len(),
                        report.scanned
                    )
                } else {
                    format!("没有新的消息。扫描 {} 条。", report.scanned)
                };
                if report.catching_up {
                    status.push_str("还有更早的消息，下次继续补。");
                }
                self.status = status;
            }
            Err(error) => {
                self.countdown_secs = self.interval_secs.max(30);
                self.status = format!("扫描出错：{}", redact(&error));
            }
        }
    }

    fn on_channel_ready(&mut self, generation: u64, result: Result<(String, String), String>) {
        if generation != self.generation {
            return;
        }
        match result {
            Ok((channel_id, channel_name)) => {
                self.channel_id = channel_id;
                self.channel_name = channel_name;
                self.fresh.clear();
                self.filter = Filter::All;
                self.reload_messages(true);
                self.status = format!("已切换到 #{}。", self.channel_name);
                self.spawn_scan();
            }
            Err(error) => self.status = format!("没有切换：{}", redact(&error)),
        }
    }

    fn on_send_done(&mut self, result: Result<Message, String>) {
        match result {
            Ok(message) => {
                let _ = self.db.put_message(&message);
                self.fresh
                    .insert(fresh_key(&message.channel_id, &message.ts));
                self.reload_messages(true);
                self.status = "已发送，并写入本地数据库。".to_string();
            }
            Err(error) => self.status = format!("没有发送：{}", redact(&error)),
        }
    }

    fn on_thread_done(&mut self, generation: u64, result: Result<Vec<Message>, String>) {
        if generation != self.generation {
            return;
        }
        match result {
            Ok(messages) => {
                let count = messages.len();
                for message in &messages {
                    self.fresh
                        .insert(fresh_key(&message.channel_id, &message.ts));
                }
                self.reload_messages(false);
                self.status = if count == 0 {
                    "这个线程没有新回复。".to_string()
                } else {
                    format!("线程补了 {count} 条。")
                };
            }
            Err(error) => self.status = format!("线程没有补上：{}", redact(&error)),
        }
    }

    fn reload_messages(&mut self, stick_to_bottom: bool) {
        let selected_ts = self.selected_message().map(|message| message.ts);
        let follow = stick_to_bottom || self.at_bottom();
        if let Ok(names) = self.db.user_names() {
            self.names = names;
        }
        if let Ok(messages) = self.db.recent(&self.channel_id, &self.channel_name, 300) {
            self.messages = messages;
        }
        if follow {
            self.follow_bottom();
        } else if let Some(ts) = selected_ts.as_deref() {
            if !self.select_ts(ts) {
                self.clamp_selection();
            }
        } else {
            self.clamp_selection();
        }
    }

    fn spawn_scan(&mut self) {
        if self.scanning {
            self.status = "正在扫描。".to_string();
            return;
        }
        self.scanning = true;
        self.status = "正在扫描 Slack...".to_string();
        let generation = self.generation;
        let slack = self.slack.clone();
        let db = self.db.clone();
        let channel_id = self.channel_id.clone();
        let channel_name = self.channel_name.clone();
        let my_id = self.my_id.clone();
        let tx = self.events.sender();
        tokio::spawn(async move {
            let result = sync_channel(&slack, &db, &channel_id, &channel_name, &my_id).await;
            let _ = tx.send(Event::App(AppEvent::ScanDone { generation, result }));
        });
    }

    fn spawn_switch(&mut self, name: String) {
        self.generation = self.generation.saturating_add(1);
        self.scanning = false;
        self.status = format!("正在切换到 #{name}...");
        let generation = self.generation;
        let slack = self.slack.clone();
        let db = self.db.clone();
        let tx = self.events.sender();
        tokio::spawn(async move {
            let result = slack.resolve_channel(&db, &name).await;
            let _ = tx.send(Event::App(AppEvent::ChannelReady { generation, result }));
        });
    }

    fn spawn_send(&mut self, body: String, mode: InputMode, selected: Option<Message>) {
        self.status = "正在发送...".to_string();
        let slack = self.slack.clone();
        let db = self.db.clone();
        let channel_id = self.channel_id.clone();
        let channel_name = self.channel_name.clone();
        let my_id = self.my_id.clone();
        let my_name = self.my_name.clone();
        let default_at = self.default_at.clone();
        let tx = self.events.sender();
        tokio::spawn(async move {
            let result: Result<Message, String> = async {
                let thread = match (&mode, &selected) {
                    (InputMode::Reply, Some(message)) => {
                        let ts = if message.thread_ts.is_empty() {
                            message.ts.clone()
                        } else {
                            message.thread_ts.clone()
                        };
                        if ts.is_empty() { None } else { Some(ts) }
                    }
                    _ => None,
                };
                let mention_ids = match mode {
                    InputMode::Reply => selected
                        .map(|message| message.user_id)
                        .filter(|id| !id.is_empty())
                        .into_iter()
                        .collect::<Vec<_>>(),
                    InputMode::Compose if !default_at.is_empty() => {
                        vec![slack.user_id(&db, &default_at).await?]
                    }
                    _ => Vec::new(),
                };
                send_text(
                    &slack,
                    &db,
                    &channel_id,
                    &channel_name,
                    &my_id,
                    &my_name,
                    &body,
                    &mention_ids,
                    thread.as_deref(),
                )
                .await
            }
            .await;
            let _ = tx.send(Event::App(AppEvent::SendDone(result)));
        });
    }

    fn spawn_thread(&mut self, root: String) {
        let generation = self.generation;
        let slack = self.slack.clone();
        let db = self.db.clone();
        let channel_id = self.channel_id.clone();
        let channel_name = self.channel_name.clone();
        let my_id = self.my_id.clone();
        let tx = self.events.sender();
        tokio::spawn(async move {
            let result = sync_thread(&slack, &db, &channel_id, &channel_name, &my_id, &root).await;
            let _ = tx.send(Event::App(AppEvent::ThreadDone { generation, result }));
        });
    }

    fn begin_reply(&mut self) {
        if self.selected_message().is_none() {
            self.status = "还没有可回复的消息。".to_string();
            return;
        }
        self.input_mode = InputMode::Reply;
        self.reset_input();
        self.help = false;
    }

    fn begin_compose(&mut self) {
        self.input_mode = InputMode::Compose;
        self.reset_input();
        self.help = false;
    }

    fn begin_channel(&mut self) {
        self.input_mode = InputMode::Channel;
        self.reset_input();
        self.help = false;
    }

    fn begin_search(&mut self) {
        self.input_mode = InputMode::Search;
        self.reset_input();
        self.help = false;
    }

    fn toggle_thread(&mut self) {
        let Some(message) = self.selected_message() else {
            self.status = "还没有可看的线程。".to_string();
            return;
        };
        let root = thread_root(&message.thread_ts, &message.ts).to_string();
        if matches!(&self.filter, Filter::Thread(current) if current == &root) {
            self.filter = Filter::All;
            self.select_ts(&message.ts);
            self.status = "显示全部消息。".to_string();
            return;
        }
        let ts = message.ts.clone();
        self.filter = Filter::Thread(root.clone());
        if !self.select_ts(&ts) {
            self.clamp_selection();
        }
        self.status = "只看这个线程。再按 t 回到全部。".to_string();
        self.spawn_thread(root);
    }

    fn cycle_filter(&mut self) {
        self.filter = match &self.filter {
            Filter::All => Filter::Mentions,
            Filter::Mentions => Filter::Mine,
            Filter::Mine | Filter::Search(_) | Filter::Thread(_) => Filter::All,
        };
        self.clamp_selection();
        self.status = format!("筛选：{}", self.filter.label());
    }

    fn toggle_sound(&mut self) {
        self.sound = self.sound.toggle();
        play_alert_sound(self.sound);
        self.status = format!("提示音：{}。只在这次会话里生效。", self.sound.label());
    }

    pub fn visible(&self) -> Vec<&Message> {
        self.messages
            .iter()
            .filter(|message| self.filter.matches(message, &self.my_id))
            .collect()
    }

    pub fn selected_message(&self) -> Option<Message> {
        let index = self.list_state.selected()?;
        self.visible().into_iter().nth(index).cloned()
    }

    pub fn mention_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|message| message.kind == "@你")
            .count()
    }

    pub fn is_fresh(&self, message: &Message) -> bool {
        self.fresh
            .contains(&fresh_key(&message.channel_id, &message.ts))
    }

    fn visible_len(&self) -> usize {
        self.messages
            .iter()
            .filter(|message| self.filter.matches(message, &self.my_id))
            .count()
    }

    fn at_bottom(&self) -> bool {
        let len = self.visible_len();
        if len == 0 {
            return true;
        }
        self.list_state
            .selected()
            .map(|index| index + 1 >= len)
            .unwrap_or(true)
    }

    fn move_by(&mut self, delta: isize) {
        let len = self.visible_len();
        if len == 0 {
            self.list_state.select(None);
            return;
        }
        let current = self.list_state.selected().unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, len as isize - 1) as usize;
        self.list_state.select(Some(next));
    }

    fn move_to(&mut self, index: usize) {
        let len = self.visible_len();
        if len == 0 {
            self.list_state.select(None);
            return;
        }
        self.list_state.select(Some(index.min(len - 1)));
    }

    fn select_ts(&mut self, ts: &str) -> bool {
        let Some(index) = self.visible().iter().position(|message| message.ts == ts) else {
            return false;
        };
        self.list_state.select(Some(index));
        true
    }

    fn reset_input(&mut self) {
        self.input.clear();
        self.cursor = 0;
    }

    fn follow_bottom(&mut self) {
        let len = self.visible_len();
        if len == 0 {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(len - 1));
        }
    }

    fn clamp_selection(&mut self) {
        let len = self.visible_len();
        if len == 0 {
            self.list_state.select(None);
            return;
        }
        let index = self.list_state.selected().unwrap_or(len - 1).min(len - 1);
        self.list_state.select(Some(index));
    }

    fn quit(&mut self) {
        self.running = false;
    }
}

fn fresh_key(channel_id: &str, ts: &str) -> String {
    format!("{channel_id}:{ts}")
}

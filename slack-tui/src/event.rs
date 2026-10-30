//! Terminal events.
//!
//! The task shape follows the Ratatui event-driven async template:
//! <https://github.com/ratatui/templates/tree/main/event-driven-async>

use std::time::Duration;

use crossterm::event::Event as CrosstermEvent;
use futures::{FutureExt, StreamExt};
use tokio::sync::mpsc;

use crate::model::Message;
use crate::sync::SyncOut;

pub enum Event {
    Tick,
    Crossterm(CrosstermEvent),
    App(AppEvent),
}

pub enum AppEvent {
    ScanDone {
        generation: u64,
        result: Result<SyncOut, String>,
    },
    ChannelReady {
        generation: u64,
        result: Result<(String, String), String>,
    },
    SendDone(Result<Message, String>),
    ThreadDone {
        generation: u64,
        result: Result<Vec<Message>, String>,
    },
}

pub struct EventHandler {
    sender: mpsc::UnboundedSender<Event>,
    receiver: mpsc::UnboundedReceiver<Event>,
}

impl EventHandler {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        let actor = EventTask {
            sender: sender.clone(),
        };
        tokio::spawn(async move {
            actor.run().await;
        });
        Self { sender, receiver }
    }

    pub async fn next(&mut self) -> Result<Event, String> {
        self.receiver
            .recv()
            .await
            .ok_or_else(|| "事件通道已关闭".to_string())
    }

    pub fn sender(&self) -> mpsc::UnboundedSender<Event> {
        self.sender.clone()
    }
}

struct EventTask {
    sender: mpsc::UnboundedSender<Event>,
}

impl EventTask {
    async fn run(self) {
        let mut reader = crossterm::event::EventStream::new();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tick.tick().await;
        loop {
            let tick_delay = tick.tick();
            let crossterm_event = reader.next().fuse();
            tokio::select! {
                _ = self.sender.closed() => break,
                _ = tick_delay => self.send(Event::Tick),
                Some(Ok(event)) = crossterm_event => self.send(Event::Crossterm(event)),
            }
        }
    }

    fn send(&self, event: Event) {
        let _ = self.sender.send(event);
    }
}

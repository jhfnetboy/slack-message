//! Slack terminal client.
//!
//! The event loop follows the Ratatui event-driven async template:
//! <https://github.com/ratatui/templates/tree/main/event-driven-async>

pub mod app;
pub mod config;
pub mod db;
pub mod event;
pub mod model;
pub mod schedule;
pub mod slack;
pub mod sync;
pub mod timeutil;
pub mod ui;

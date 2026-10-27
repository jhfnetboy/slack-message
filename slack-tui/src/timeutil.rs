use chrono::{DateTime, FixedOffset, TimeZone, Timelike, Utc};
use std::process::Command;

use crate::model::redact;

pub const BANGKOK_OFFSET_SECS: i32 = 7 * 3600;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundMode {
    Doorbell,
    Beep,
}

impl SoundMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Doorbell => "门铃",
            Self::Beep => "轻响",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            Self::Doorbell => Self::Beep,
            Self::Beep => Self::Doorbell,
        }
    }

    pub fn parse(text: &str) -> Self {
        match text.trim().to_lowercase().as_str() {
            "beep" | "pop" | "dudu" | "嘟嘟" | "轻响" => Self::Beep,
            _ => Self::Doorbell,
        }
    }
}

pub fn bangkok_offset() -> FixedOffset {
    FixedOffset::east_opt(BANGKOK_OFFSET_SECS).expect("UTC+7 is a valid offset")
}

pub fn bangkok_now() -> DateTime<FixedOffset> {
    Utc::now().with_timezone(&bangkok_offset())
}

pub fn is_work_hours(moment: &DateTime<FixedOffset>) -> bool {
    let minutes = moment.hour() * 60 + moment.minute();
    (13 * 60 + 30) <= minutes && minutes < (23 * 60)
}

pub fn format_ts(ts: &str) -> String {
    let Ok(seconds) = ts.parse::<f64>() else {
        return ts.to_string();
    };
    let whole = seconds.trunc() as i64;
    let nanos = (seconds.fract() * 1_000_000_000.0) as u32;
    match Utc.timestamp_opt(whole, nanos).single() {
        Some(moment) => moment
            .with_timezone(&bangkok_offset())
            .format("%m-%d %H:%M")
            .to_string(),
        None => ts.to_string(),
    }
}

pub fn format_now(moment: &DateTime<FixedOffset>) -> String {
    moment.format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn format_clock(moment: &DateTime<FixedOffset>) -> String {
    moment.format("%Y-%m-%d %H:%M").to_string()
}

pub fn play_alert_sound(mode: SoundMode) {
    #[cfg(target_os = "macos")]
    {
        let path = match mode {
            SoundMode::Doorbell => "/System/Library/Sounds/Ping.aiff",
            SoundMode::Beep => "/System/Library/Sounds/Pop.aiff",
        };
        if std::path::Path::new(path).is_file() {
            match Command::new("afplay")
                .arg(path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(_) => return,
                Err(error) => {
                    let _ = redact(&error.to_string());
                }
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = mode;
    }
    print!("\u{7}");
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hour: u32, minute: u32) -> DateTime<FixedOffset> {
        bangkok_offset()
            .with_ymd_and_hms(2026, 10, 26, hour, minute, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn work_window_is_half_past_one_until_eleven() {
        assert!(!is_work_hours(&at(13, 29)));
        assert!(is_work_hours(&at(13, 30)));
        assert!(is_work_hours(&at(22, 59)));
        assert!(!is_work_hours(&at(23, 0)));
    }

    #[test]
    fn formats_slack_timestamps_in_bangkok() {
        assert_eq!(format_ts("0"), "01-01 07:00");
    }
}

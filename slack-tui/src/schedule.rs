/// How often the client asks Slack for new messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanMode {
    /// 13:30–23:00 Bangkok: every 5 minutes. Otherwise back off when nothing arrives.
    Adaptive,
    /// Caller fixed the interval. Work hours do not change it.
    Fixed,
}

pub const MIN_SECS: u64 = 5 * 60;
pub const MAX_SECS: u64 = 30 * 60;
pub const STEP_SECS: u64 = 5 * 60;

pub fn next_interval(
    mode: ScanMode,
    in_work_hours: bool,
    had_new: bool,
    current: u64,
    fixed: u64,
) -> u64 {
    if mode == ScanMode::Fixed {
        return fixed.max(30);
    }
    if in_work_hours || had_new {
        return MIN_SECS;
    }
    current.saturating_add(STEP_SECS).min(MAX_SECS)
}

/// `30` and `30m` are minutes. `90s` is seconds. Seconds floor at 30, minutes at 1.
pub fn parse_interval(text: &str) -> Result<u64, String> {
    let text = text.trim().to_lowercase();
    if let Some(number) = text.strip_suffix('s') {
        let seconds: u64 = number
            .parse()
            .map_err(|_| format!("无法识别刷新时间：{text}。用 30、30m 或 90s。"))?;
        return Ok(seconds.max(30));
    }
    if let Some(number) = text.strip_suffix('m') {
        let minutes: u64 = number
            .parse()
            .map_err(|_| format!("无法识别刷新时间：{text}。用 30、30m 或 90s。"))?;
        return Ok(minutes.saturating_mul(60).max(60));
    }
    if let Ok(minutes) = text.parse::<u64>() {
        return Ok(minutes.saturating_mul(60).max(60));
    }
    Err(format!("无法识别刷新时间：{text}。用 30、30m 或 90s。"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_hours_stay_at_five_minutes() {
        assert_eq!(
            next_interval(ScanMode::Adaptive, true, false, 1800, 300),
            MIN_SECS
        );
    }

    #[test]
    fn quiet_hours_back_off_until_thirty_minutes() {
        let mut current = MIN_SECS;
        for expected in [600, 900, 1200, 1500, 1800, 1800] {
            current = next_interval(ScanMode::Adaptive, false, false, current, 300);
            assert_eq!(current, expected);
        }
    }

    #[test]
    fn a_new_message_resets_the_backoff() {
        assert_eq!(
            next_interval(ScanMode::Adaptive, false, true, 1800, 300),
            MIN_SECS
        );
    }

    #[test]
    fn fixed_interval_ignores_the_clock() {
        assert_eq!(next_interval(ScanMode::Fixed, true, true, 300, 90), 90);
    }

    #[test]
    fn parses_minutes_and_seconds() {
        assert_eq!(parse_interval("30").unwrap(), 1800);
        assert_eq!(parse_interval("7m").unwrap(), 420);
        assert_eq!(parse_interval("90s").unwrap(), 90);
        assert_eq!(parse_interval("10s").unwrap(), 30);
        assert!(parse_interval("soon").is_err());
    }
}

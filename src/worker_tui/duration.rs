/// Compact runtime with the two largest units, keeping long runs easy to scan.
pub(crate) fn format_runtime(seconds: i64) -> String {
    let seconds = seconds.max(0);
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        3600..86400 => format!("{}h {:02}m", seconds / 3600, seconds / 60 % 60),
        _ => format!("{}d {:02}h", seconds / 86400, seconds / 3600 % 24),
    }
}

#[cfg(test)]
mod tests {
    use super::format_runtime;

    #[test]
    fn runtimes_use_seconds_minutes_hours_and_days() {
        for (seconds, expected) in [
            (-1, "0s"),
            (0, "0s"),
            (59, "59s"),
            (60, "1m 00s"),
            (64, "1m 04s"),
            (3599, "59m 59s"),
            (3600, "1h 00m"),
            (3661, "1h 01m"),
            (669 * 60 + 51, "11h 09m"),
            (86399, "23h 59m"),
            (86400, "1d 00h"),
            (2110 * 60 + 1, "1d 11h"),
            (100 * 86400 + 3 * 3600, "100d 03h"),
        ] {
            assert_eq!(format_runtime(seconds), expected, "{seconds} seconds");
        }
    }
}

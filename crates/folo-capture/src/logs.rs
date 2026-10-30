//! The agent's log: JSON lines on stderr in the effect engine's format
//! (`ts`, `level`, `logger`, `msg`), which the node writes into its own log
//! as they are (ADR-0034 D8). A line never holds pixels or the launch token
//! (ADR-0037 D9).

use std::io::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

const LOGGER: &str = "folo-capture";

/// What a line is about, in the engine's level names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
    Warning,
    Error,
}

impl Level {
    const fn name(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// Writes one record to stderr.
pub fn log(level: Level, message: &str) {
    let line = format_line(SystemTime::now(), level, message);
    let _ = writeln!(std::io::stderr().lock(), "{line}");
}

fn format_line(time: SystemTime, level: Level, message: &str) -> String {
    json!({
        "ts": timestamp(time),
        "level": level.name(),
        "logger": LOGGER,
        "msg": message,
    })
    .to_string()
}

/// RFC 3339 UTC with milliseconds.
fn timestamp(time: SystemTime) -> String {
    let millis = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let millis = i64::try_from(millis).unwrap_or(i64::MAX);
    let (days, rest) = (millis.div_euclid(86_400_000), millis.rem_euclid(86_400_000));
    let (year, month, day) = civil(days);
    let (hour, rest) = (rest / 3_600_000, rest % 3_600_000);
    let (minute, rest) = (rest / 60_000, rest % 60_000);
    let (second, milli) = (rest / 1000, rest % 1000);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{milli:03}Z")
}

/// The date of a day count since 1970-01-01 (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn records_have_the_engines_fields() {
        let time = UNIX_EPOCH + Duration::from_millis(1_793_015_445_123);
        let line = format_line(time, Level::Warning, "hello \"there\"");
        let record: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(record["ts"], "2026-10-26T11:50:45.123Z");
        assert_eq!(record["level"], "warning");
        assert_eq!(record["logger"], "folo-capture");
        assert_eq!(record["msg"], "hello \"there\"");
    }
}

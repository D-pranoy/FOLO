//! The node's log (ADR-0034 D8, docs/architecture/control-plane.md#supervision):
//! JSON lines in the effect engine's format, to stderr and to `folo.log`,
//! which rotates at 10 MiB keeping four older files. The engine's own lines
//! go in as they are.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use log::{Level, LevelFilter, Log, Metadata, Record};
use serde_json::{Map, Value};

/// The size at which `folo.log` rotates.
const ROTATE_AT: u64 = 10 * 1024 * 1024;
/// Older files kept: `folo.log.1` to `folo.log.4`.
const KEEP: u32 = 4;
/// The logger name of engine lines that aren't JSON.
const ENGINE_LOGGER: &str = "folo-source";

struct Logger {
    level: AtomicUsize,
    file: Mutex<Option<Rotating>>,
}

struct Rotating {
    path: PathBuf,
    file: File,
    size: u64,
}

impl Rotating {
    fn open(path: &Path) -> io::Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            path: path.to_path_buf(),
            file,
            size,
        })
    }

    fn write(&mut self, line: &str) -> io::Result<()> {
        let length = line.len() as u64 + 1;
        if self.size > 0 && self.size + length > ROTATE_AT {
            self.rotate()?;
        }
        self.file.write_all(line.as_bytes())?;
        self.file.write_all(b"\n")?;
        self.size += length;
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        let numbered = |n: u32| PathBuf::from(format!("{}.{n}", self.path.display()));
        let _ = fs::remove_file(numbered(KEEP));
        for n in (1..KEEP).rev() {
            let _ = fs::rename(numbered(n), numbered(n + 1));
        }
        fs::rename(&self.path, numbered(1))?;
        *self = Self::open(&self.path)?;
        Ok(())
    }
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

fn logger() -> &'static Logger {
    LOGGER.get_or_init(|| Logger {
        level: AtomicUsize::new(LevelFilter::Info as usize),
        file: Mutex::new(None),
    })
}

/// Installs the logger, writing to stderr and, once [`open_file`] is
/// called, to the log file. Calling it again only changes the level.
pub fn init(level: LevelFilter) {
    set_level(level);
    let _ = log::set_logger(&LoggerRef);
}

/// Starts writing to `path` as well as stderr.
///
/// # Errors
///
/// When the file can't be opened.
pub fn open_file(path: &Path) -> io::Result<()> {
    let rotating = Rotating::open(path)?;
    *logger().file.lock().unwrap_or_else(PoisonError::into_inner) = Some(rotating);
    Ok(())
}

/// Stops writing to the file, which the node does as it stops.
pub fn close_file() {
    logger()
        .file
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
}

/// Changes the level (`log_level`, which `folo reload` applies).
pub fn set_level(level: LevelFilter) {
    logger().level.store(level as usize, Ordering::Relaxed);
    log::set_max_level(level);
}

/// The level of a `log_level` setting.
#[must_use]
pub fn level_of(setting: &str) -> LevelFilter {
    match setting {
        "error" => LevelFilter::Error,
        "warn" => LevelFilter::Warn,
        "debug" => LevelFilter::Debug,
        "trace" => LevelFilter::Trace,
        _ => LevelFilter::Info,
    }
}

fn current() -> LevelFilter {
    match logger().level.load(Ordering::Relaxed) {
        0 => LevelFilter::Off,
        1 => LevelFilter::Error,
        2 => LevelFilter::Warn,
        3 => LevelFilter::Info,
        4 => LevelFilter::Debug,
        _ => LevelFilter::Trace,
    }
}

/// A line the effect engine wrote: kept as it is when it's a JSON object,
/// filtered by its level; otherwise logged at `error` (ADR-0034 D8).
pub fn engine_line(line: &str) {
    let line = line.trim_end();
    if line.is_empty() {
        return;
    }
    if let Ok(Value::Object(record)) = serde_json::from_str::<Value>(line) {
        let level = match record.get("level").and_then(Value::as_str) {
            Some("critical" | "error") => Level::Error,
            Some("warning") => Level::Warn,
            Some("debug") => Level::Debug,
            _ => Level::Info,
        };
        if level <= current() {
            emit(line);
        }
    } else if Level::Error <= current() {
        emit(&format_line(Level::Error, ENGINE_LOGGER, line));
    }
}

fn emit(line: &str) {
    let _ = writeln!(io::stderr().lock(), "{line}");
    let mut file = logger().file.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(rotating) = file.as_mut()
        && let Err(error) = rotating.write(line)
    {
        let _ = writeln!(
            io::stderr().lock(),
            "the log file can't be written: {error}"
        );
        *file = None;
    }
}

/// One record in the engine's format: `ts`, `level`, `logger`, `msg`.
#[must_use]
pub fn format_line(level: Level, logger: &str, message: &str) -> String {
    let mut record = Map::new();
    record.insert("ts".into(), Value::String(timestamp(SystemTime::now())));
    record.insert("level".into(), Value::String(level_name(level).into()));
    record.insert("logger".into(), Value::String(logger.into()));
    record.insert("msg".into(), Value::String(message.into()));
    Value::Object(record).to_string()
}

const fn level_name(level: Level) -> &'static str {
    match level {
        Level::Error => "error",
        Level::Warn => "warning",
        Level::Info => "info",
        Level::Debug => "debug",
        Level::Trace => "trace",
    }
}

/// UTC, RFC 3339 with milliseconds, as the engine writes it.
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

struct LoggerRef;

impl Log for LoggerRef {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= current()
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            emit(&format_line(
                record.level(),
                record.target(),
                &record.args().to_string(),
            ));
        }
    }

    fn flush(&self) {
        let file = logger().file.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(rotating) = file.as_ref() {
            let _ = rotating.file.sync_data();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamps_are_rfc_3339_utc_with_milliseconds() {
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        let time = UNIX_EPOCH + Duration::from_millis(1_793_015_445_123);
        assert_eq!(timestamp(time), "2026-10-26T11:50:45.123Z");
        let leap = UNIX_EPOCH + Duration::from_hours(264_384); // 2000-02-29
        assert_eq!(timestamp(leap), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn records_have_the_engines_fields() {
        let line = format_line(Level::Warn, "folo_node", "hello \"there\"");
        let record: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(record["level"], "warning");
        assert_eq!(record["logger"], "folo_node");
        assert_eq!(record["msg"], "hello \"there\"");
        assert!(record["ts"].as_str().unwrap().ends_with('Z'));
    }

    #[test]
    fn the_file_rotates_keeping_four() {
        let dir = std::env::temp_dir().join(format!("folo-logs-{}", folo_store::new_id()));
        let path = dir.join("folo.log");
        let mut rotating = Rotating::open(&path).unwrap();
        let line = "x".repeat(1024 * 1024 - 1);
        for _ in 0..60 {
            rotating.write(&line).unwrap();
        }
        let mut names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "folo.log",
                "folo.log.1",
                "folo.log.2",
                "folo.log.3",
                "folo.log.4"
            ]
        );
        for name in &names {
            assert!(fs::metadata(dir.join(name)).unwrap().len() <= ROTATE_AT);
        }
        drop(rotating);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn levels_follow_the_setting() {
        assert_eq!(level_of("warn"), LevelFilter::Warn);
        assert_eq!(level_of("trace"), LevelFilter::Trace);
        assert_eq!(level_of("anything"), LevelFilter::Info);
    }
}

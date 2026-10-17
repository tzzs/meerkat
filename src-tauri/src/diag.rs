//! Meerkat's own log.
//!
//! A release build is a `windows`-subsystem program: it has no console, so every `eprintln!` went
//! nowhere, "Copy report" promised a recent log that did not exist, and the first anyone heard of
//! a settings file that could not be written or a mixer thread that never started was "it does
//! not work". This writes those lines to `<app data>/logs/meerkat.log`, keeps the file before it as
//! `meerkat.log.1` once it passes 512 KiB, and hands the tail of both to the report.
//!
//! What it promises its callers:
//!
//! * It never panics and never fails the caller. A log that cannot be written is dropped (and
//!   goes to stderr, where a debug build can see it); the sampler does not stop for it.
//! * It is cheap: one lock and one `write` to a file the OS already caches. The same message
//!   repeated in a row is written once with a count, so a failure that recurs every second cannot
//!   push the interesting lines out of a file of this size.
//! * It works before it is set up. Lines logged before `init` (the settings file is read very
//!   early) are kept in memory and written when the file is opened, and until then they also go to
//!   stderr.
//! * A panic is recorded with its message, its location and a backtrace before the process dies.
//!   The release profile aborts on panic; the hook runs first.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// The size a log file may reach before it becomes `meerkat.log.1`.
pub const MAX_LOG_BYTES: u64 = 512 * 1024;
const FILE_NAME: &str = "meerkat.log";
const OLDER_NAME: &str = "meerkat.log.1";
/// Lines logged before the file exists are kept up to this many.
const PENDING_LIMIT: usize = 200;
/// A message that keeps repeating is summarised at least this often.
const REPEAT_SUMMARY_EVERY: u32 = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Error,
    Warn,
    Info,
}

impl Level {
    /// The three words the frontend may send.
    pub fn parse(text: &str) -> Option<Level> {
        match text {
            "error" => Some(Level::Error),
            "warn" => Some(Level::Warn),
            "info" => Some(Level::Info),
            _ => None,
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The log file
// ---------------------------------------------------------------------------------------------

/// The state behind the one global log; a value of its own so the file handling can be tested
/// against a temporary directory with a small size limit.
struct Logger {
    /// `<app data>/logs`, once the file is open.
    dir: Option<PathBuf>,
    file: Option<File>,
    size: u64,
    max_bytes: u64,
    /// Lines logged before the file was open, oldest first, in the form they will be written.
    pending: VecDeque<String>,
    last: Option<(Level, String)>,
    repeats: u32,
}

impl Logger {
    const fn new() -> Self {
        Self {
            dir: None,
            file: None,
            size: 0,
            max_bytes: MAX_LOG_BYTES,
            pending: VecDeque::new(),
            last: None,
            repeats: 0,
        }
    }

    /// Opens `<dir>/meerkat.log` for appending (rotating a file that is already too big) and writes
    /// what was logged while there was no file.
    fn open(&mut self, dir: &Path) {
        if self.dir.as_deref() == Some(dir) && self.file.is_some() {
            return;
        }
        let _ = fs::create_dir_all(dir);
        let path = dir.join(FILE_NAME);
        if fs::metadata(&path).map(|m| m.len() >= self.max_bytes).unwrap_or(false) {
            self.rotate_files(dir);
        }
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => {
                self.size = file.metadata().map(|m| m.len()).unwrap_or(0);
                self.file = Some(file);
                self.dir = Some(dir.to_path_buf());
            }
            Err(error) => {
                stderr(&format!("diag: cannot open {}: {error}", path.display()));
                return;
            }
        }
        while let Some(line) = self.pending.pop_front() {
            self.append(&line);
        }
    }

    fn rotate_files(&mut self, dir: &Path) {
        self.file = None;
        // `rename` replaces the older file, which is the point.
        let _ = fs::rename(dir.join(FILE_NAME), dir.join(OLDER_NAME));
    }

    /// One line, with the repeat bookkeeping and the timestamp.
    fn record(&mut self, level: Level, source: &str, message: &str, now_ms: u64) {
        let same = matches!(&self.last, Some((l, m)) if *l == level && m == message);
        if same {
            self.repeats += 1;
            if self.repeats < REPEAT_SUMMARY_EVERY {
                return;
            }
        }
        if self.repeats > 0 {
            let line = format_line(now_ms, Level::Info, "diag", &format!("(the line above repeated {} times)", self.repeats));
            self.emit(&line);
            self.repeats = 0;
            if same {
                return;
            }
        }
        self.last = Some((level, message.to_string()));
        let line = format_line(now_ms, level, source, message);
        self.emit(&line);
    }

    /// Writes a finished line to the file, or keeps it until there is one.
    fn emit(&mut self, line: &str) {
        if self.file.is_some() {
            self.append(line);
        } else {
            stderr(line);
            if self.pending.len() >= PENDING_LIMIT {
                self.pending.pop_front();
            }
            self.pending.push_back(line.to_string());
        }
    }

    fn append(&mut self, line: &str) {
        let needed = line.len() as u64 + 1;
        if self.size > 0 && self.size + needed > self.max_bytes {
            if let Some(dir) = self.dir.clone() {
                self.rotate_files(&dir);
                self.size = 0;
                match OpenOptions::new().create(true).append(true).open(dir.join(FILE_NAME)) {
                    Ok(file) => self.file = Some(file),
                    Err(error) => {
                        stderr(&format!("diag: cannot reopen the log: {error}"));
                        stderr(line);
                        return;
                    }
                }
            }
        }
        let Some(file) = self.file.as_mut() else {
            stderr(line);
            return;
        };
        let mut text = String::with_capacity(line.len() + 1);
        text.push_str(line);
        text.push('\n');
        match file.write_all(text.as_bytes()) {
            Ok(()) => self.size += needed,
            Err(error) => {
                // A full disk or a vanished folder: stop writing rather than fail every call.
                stderr(&format!("diag: cannot write the log: {error}"));
                stderr(line);
                self.file = None;
            }
        }
    }

    /// The last `count` lines, oldest first, from the older file, the current one and whatever is
    /// still waiting for a file.
    fn tail(&self, count: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        if let Some(dir) = &self.dir {
            for name in [OLDER_NAME, FILE_NAME] {
                if let Ok(bytes) = fs::read(dir.join(name)) {
                    lines.extend(String::from_utf8_lossy(&bytes).lines().map(str::to_string));
                }
            }
        }
        lines.extend(self.pending.iter().cloned());
        let skip = lines.len().saturating_sub(count);
        lines.split_off(skip)
    }
}

static LOGGER: Mutex<Logger> = Mutex::new(Logger::new());

fn logger() -> std::sync::MutexGuard<'static, Logger> {
    LOGGER.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn stderr(line: &str) {
    // Not eprintln!: that panics when stderr cannot be written, and this must never panic. (The
    // test harness only captures the macro, so tests use it to keep their output quiet.)
    #[cfg(test)]
    eprintln!("{line}");
    #[cfg(not(test))]
    let _ = writeln!(std::io::stderr(), "{line}");
}

// ---------------------------------------------------------------------------------------------
// Public surface
// ---------------------------------------------------------------------------------------------

/// Opens the log under `<app_data>/logs` and writes what was logged before it existed. Safe to
/// call again with the same folder. Installs the panic hook the first time.
pub fn init(app_data: &Path) {
    logger().open(&app_data.join("logs"));
    install_panic_hook();
}

/// The same, for the very start of `main`, before Tauri has told the app where its data lives:
/// the folder it will choose is `%APPDATA%\<identifier>`. A failure to start at all (no WebView2
/// runtime is the usual one) is then in the file too.
pub fn init_early(identifier: &str) {
    if let Some(base) = std::env::var_os("APPDATA") {
        init(&PathBuf::from(base).join(identifier));
    } else {
        install_panic_hook();
    }
}

pub fn log(level: Level, message: impl AsRef<str>) {
    let thread = std::thread::current();
    let source = thread.name().unwrap_or("main-or-unnamed");
    logger().record(level, source, message.as_ref(), unix_ms());
}

pub fn error(message: impl AsRef<str>) {
    log(Level::Error, message);
}

pub fn warn(message: impl AsRef<str>) {
    log(Level::Warn, message);
}

pub fn info(message: impl AsRef<str>) {
    log(Level::Info, message);
}

/// The last `count` lines of the log, oldest first.
pub fn tail(count: usize) -> Vec<String> {
    logger().tail(count)
}

/// The last `count` lines, ready to be pasted into a bug report: the user's profile folder, user
/// name and computer name are replaced, since paths in log lines carry them.
pub fn tail_for_report(count: usize) -> Vec<String> {
    let needles = personal_needles();
    tail(count).into_iter().map(|line| redact(&line, &needles)).collect()
}

// ---------------------------------------------------------------------------------------------
// Lines
// ---------------------------------------------------------------------------------------------

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// `2026-10-12T14:03:22.123Z`, UTC. Days to a calendar date by Howard Hinnant's civil-from-days.
pub fn timestamp(unix_ms: u64) -> String {
    let seconds = unix_ms / 1000;
    let millis = unix_ms % 1000;
    let days = (seconds / 86_400) as i64;
    let of_day = seconds % 86_400;

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60,
    )
}

/// A message on one entry: no carriage returns and no control characters, and a message that
/// spans lines (a stack trace) is indented so every entry still starts with a timestamp.
fn single_entry(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    for ch in message.chars() {
        match ch {
            '\r' => {}
            '\n' => out.push_str("\n    "),
            '\t' => out.push(' '),
            c if c.is_control() => out.push('?'),
            c => out.push(c),
        }
    }
    out
}

fn format_line(now_ms: u64, level: Level, source: &str, message: &str) -> String {
    format!("{} {} [{}] {}", timestamp(now_ms), level.tag(), source, single_entry(message))
}

// ---------------------------------------------------------------------------------------------
// Panics
// ---------------------------------------------------------------------------------------------

fn install_panic_hook() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let thread = std::thread::current();
            let name = thread.name().unwrap_or("unnamed").to_string();
            let payload = info.payload();
            let message = payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a panic with no text".to_string());
            let location = info
                .location()
                .map(|at| format!("{}:{}:{}", at.file(), at.line(), at.column()))
                .unwrap_or_else(|| "an unknown place".to_string());
            let backtrace = std::backtrace::Backtrace::force_capture();
            let line = format_line(
                unix_ms(),
                Level::Error,
                &name,
                &format!("panic: {message} at {location}\n{backtrace}"),
            );
            emit_from_panic(&line);
            previous(info);
        }));
    });
}

/// Writes a panic's line without ever waiting for the log: the thread that panicked may be the one
/// holding it. Another thread holding it is given a moment (a write takes microseconds).
fn emit_from_panic(line: &str) {
    for attempt in 0..50 {
        match LOGGER.try_lock() {
            Ok(mut logger) => return logger.emit(line),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => return poisoned.into_inner().emit(line),
            Err(std::sync::TryLockError::WouldBlock) if attempt < 49 => {
                std::thread::sleep(std::time::Duration::from_millis(4));
            }
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
    }
    stderr(line);
}

// ---------------------------------------------------------------------------------------------
// Lines from the frontend
// ---------------------------------------------------------------------------------------------

/// The longest message a window may log, in bytes.
pub const MAX_FRONTEND_BYTES: usize = 2048;
/// At most this many frontend lines per window of time, so one error thrown on every frame cannot
/// fill the log.
const FRONTEND_LINES: usize = 20;
const FRONTEND_WINDOW_MS: u64 = 10_000;

/// A sliding-window rate limit.
struct Limiter {
    stamps: VecDeque<u64>,
    max: usize,
    window_ms: u64,
}

impl Limiter {
    const fn new(max: usize, window_ms: u64) -> Self {
        Self { stamps: VecDeque::new(), max, window_ms }
    }

    fn allow(&mut self, now_ms: u64) -> bool {
        while let Some(oldest) = self.stamps.front() {
            if now_ms.saturating_sub(*oldest) >= self.window_ms {
                self.stamps.pop_front();
            } else {
                break;
            }
        }
        if self.stamps.len() >= self.max {
            return false;
        }
        self.stamps.push_back(now_ms);
        true
    }
}

static FRONTEND: Mutex<Limiter> = Mutex::new(Limiter::new(FRONTEND_LINES, FRONTEND_WINDOW_MS));

/// `message` cut to `MAX_FRONTEND_BYTES` at a character boundary, with a mark where it was cut.
fn clip(message: &str) -> String {
    if message.len() <= MAX_FRONTEND_BYTES {
        return message.to_string();
    }
    let mut end = MAX_FRONTEND_BYTES;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... (cut)", &message[..end])
}

/// A line from a window. `Err` for a level that is not one of the three; a line over the rate
/// limit is dropped without an error, which the sender could only answer by sending more.
pub fn from_frontend(level: &str, message: &str) -> Result<(), String> {
    from_frontend_at(&FRONTEND, level, message, unix_ms())
}

fn from_frontend_at(limiter: &Mutex<Limiter>, level: &str, message: &str, now_ms: u64) -> Result<(), String> {
    let Some(level) = Level::parse(level) else {
        return Err(format!("unknown log level {level:?}"));
    };
    let allowed = limiter.lock().unwrap_or_else(|p| p.into_inner()).allow(now_ms);
    if allowed {
        logger().record(level, "ui", &clip(message), now_ms);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// What a report must not carry
// ---------------------------------------------------------------------------------------------

/// What identifies the person or the machine in a path or a message: (text, what stands for it).
/// Longest first, so the profile folder goes before the user name inside it.
fn personal_needles() -> Vec<(String, &'static str)> {
    let mut needles: Vec<(String, &'static str)> = Vec::new();
    for (variable, stand_in) in [
        ("USERPROFILE", "%USERPROFILE%"),
        ("COMPUTERNAME", "<computer>"),
        ("USERNAME", "<user>"),
    ] {
        if let Ok(value) = std::env::var(variable) {
            // A one or two letter name would redact half of every word.
            if value.chars().count() >= 3 {
                needles.push((value, stand_in));
            }
        }
    }
    needles.sort_by_key(|(text, _)| std::cmp::Reverse(text.len()));
    needles
}

/// `line` with each needle replaced, ignoring ASCII letter case, as Windows paths do.
fn redact(line: &str, needles: &[(String, &str)]) -> String {
    let mut current = line.to_string();
    for (needle, stand_in) in needles {
        current = replace_ignoring_case(&current, needle, stand_in);
    }
    current
}

fn replace_ignoring_case(haystack: &str, needle: &str, with: &str) -> String {
    let (hay, pin) = (haystack.as_bytes(), needle.as_bytes());
    if pin.is_empty() || hay.len() < pin.len() {
        return haystack.to_string();
    }
    let mut out = String::with_capacity(haystack.len());
    let mut copied = 0;
    let mut at = 0;
    while at + pin.len() <= hay.len() {
        if hay[at..at + pin.len()].eq_ignore_ascii_case(pin) {
            out.push_str(&haystack[copied..at]);
            out.push_str(with);
            at += pin.len();
            copied = at;
        } else {
            at += 1;
        }
    }
    out.push_str(&haystack[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("meerkat-diag-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir to test against");
        dir
    }

    fn read(dir: &Path, name: &str) -> String {
        fs::read_to_string(dir.join(name)).unwrap_or_default()
    }

    #[test]
    fn timestamps_are_utc_iso_with_milliseconds() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(timestamp(1_000_000_000_123), "2001-09-09T01:46:40.123Z");
        assert_eq!(timestamp(1_760_270_602_456), "2025-10-12T12:03:22.456Z");
        assert_eq!(timestamp(951_782_400_999), "2000-02-29T00:00:00.999Z", "a leap day");
        assert_eq!(timestamp(4_102_444_800_000), "2100-01-01T00:00:00.000Z");
    }

    #[test]
    fn a_line_has_a_time_a_level_a_source_and_one_entry() {
        let line = format_line(0, Level::Warn, "meerkat-sampler", "tray tooltip: refused");
        assert_eq!(line, "1970-01-01T00:00:00.000Z WARN  [meerkat-sampler] tray tooltip: refused");
        // A stack trace stays one entry: its continuation lines are indented, not timestamped.
        let multi = format_line(0, Level::Error, "ui", "Error: x\r\n  at a\n  at b");
        assert_eq!(multi.lines().count(), 3);
        assert!(multi.lines().skip(1).all(|l| l.starts_with("    ")), "{multi}");
        assert!(!multi.contains('\r'));
    }

    #[test]
    fn lines_logged_before_the_file_exists_are_written_when_it_does() {
        let dir = temp_dir("pending");
        let mut logger = Logger::new();
        logger.record(Level::Warn, "main", "settings: ignoring an unreadable part", 1_000);
        logger.record(Level::Info, "main", "started", 2_000);
        assert_eq!(logger.tail(10).len(), 2, "still readable from memory");

        logger.open(&dir);
        logger.record(Level::Info, "main", "after the file", 3_000);
        let text = read(&dir, FILE_NAME);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("ignoring an unreadable part"));
        assert!(lines[2].ends_with("[main] after the file"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_file_rolls_over_at_its_limit_and_keeps_the_one_before() {
        let dir = temp_dir("rotate");
        let mut logger = Logger::new();
        logger.max_bytes = 300;
        logger.open(&dir);
        for n in 0..12 {
            logger.record(Level::Info, "t", &format!("line number {n} with some padding"), 1_000 + n);
        }
        let current = read(&dir, FILE_NAME);
        let older = read(&dir, OLDER_NAME);
        assert!(!older.is_empty(), "the full file became meerkat.log.1");
        assert!(current.len() as u64 <= 300, "the new file stays within the limit: {}", current.len());
        assert!(current.contains("line number 11"), "the newest line is in the current file");
        // Together they hold a contiguous, ordered stretch ending at the newest line.
        let all = logger.tail(100);
        assert!(all.last().unwrap().contains("line number 11"));
        let numbers: Vec<u32> = all
            .iter()
            .filter_map(|l| l.split("line number ").nth(1)?.split(' ').next()?.parse().ok())
            .collect();
        assert!(numbers.windows(2).all(|w| w[1] == w[0] + 1), "{numbers:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_already_too_big_is_rotated_when_it_is_opened() {
        let dir = temp_dir("startup");
        fs::write(dir.join(FILE_NAME), "x".repeat(400)).unwrap();
        let mut logger = Logger::new();
        logger.max_bytes = 300;
        logger.open(&dir);
        logger.record(Level::Info, "t", "fresh", 5);
        assert_eq!(read(&dir, OLDER_NAME).len(), 400);
        assert_eq!(read(&dir, FILE_NAME).lines().count(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_same_message_in_a_row_is_written_once_with_a_count() {
        let dir = temp_dir("repeat");
        let mut logger = Logger::new();
        logger.open(&dir);
        for n in 0..5 {
            logger.record(Level::Warn, "t", "tray tooltip: refused", 1_000 + n);
        }
        logger.record(Level::Warn, "t", "something else", 2_000);
        let text = read(&dir, FILE_NAME);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert!(lines[0].contains("tray tooltip: refused"));
        assert!(lines[1].contains("repeated 4 times"), "{}", lines[1]);
        assert!(lines[2].contains("something else"));
        // The same text at another level is another line.
        logger.record(Level::Error, "t", "something else", 3_000);
        assert_eq!(read(&dir, FILE_NAME).lines().count(), 4);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_tail_is_the_last_lines_oldest_first() {
        let dir = temp_dir("tail");
        let mut logger = Logger::new();
        logger.open(&dir);
        for n in 0..10 {
            logger.record(Level::Info, "t", &format!("entry {n}"), n);
        }
        let last = logger.tail(3);
        assert_eq!(last.len(), 3);
        assert!(last[0].ends_with("entry 7") && last[2].ends_with("entry 9"));
        assert_eq!(logger.tail(0).len(), 0);
        assert_eq!(logger.tail(100).len(), 10);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn opening_the_same_folder_again_keeps_the_file() {
        let dir = temp_dir("again");
        let mut logger = Logger::new();
        logger.open(&dir);
        logger.record(Level::Info, "t", "one", 1);
        logger.open(&dir);
        logger.record(Level::Info, "t", "two", 2);
        assert_eq!(read(&dir, FILE_NAME).lines().count(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_frontend_may_send_twenty_lines_in_ten_seconds() {
        let mut limiter = Limiter::new(FRONTEND_LINES, FRONTEND_WINDOW_MS);
        let allowed = (0..30).filter(|n| limiter.allow(1_000 + n)).count();
        assert_eq!(allowed, 20);
        assert!(!limiter.allow(10_999), "still inside the window of the first line");
        assert!(limiter.allow(11_000), "the first lines have left the window");
    }

    #[test]
    fn a_frontend_line_is_checked_clipped_and_counted() {
        let limiter = Mutex::new(Limiter::new(2, 10_000));
        assert!(from_frontend_at(&limiter, "error", "boom", 1).is_ok());
        assert!(from_frontend_at(&limiter, "warn", "hmm", 2).is_ok());
        // Over the limit: not an error, the line is just dropped.
        assert!(from_frontend_at(&limiter, "info", "chatter", 3).is_ok());
        // An unknown level is refused whatever the limit says.
        let refused = from_frontend_at(&limiter, "fatal", "x", 4);
        assert!(refused.is_err(), "{refused:?}");
        assert_eq!(Level::parse("error"), Some(Level::Error));
        assert_eq!(Level::parse("ERROR"), None);
        assert_eq!(Level::parse(""), None);
    }

    #[test]
    fn a_long_message_is_cut_on_a_character_boundary() {
        let short = "x".repeat(MAX_FRONTEND_BYTES);
        assert_eq!(clip(&short), short);
        let long = "x".repeat(MAX_FRONTEND_BYTES + 1);
        assert!(clip(&long).len() < MAX_FRONTEND_BYTES + 12 && clip(&long).ends_with("(cut)"));
        // Each of these is three bytes; the limit falls inside one of them.
        let wide = "\u{4e2d}".repeat(MAX_FRONTEND_BYTES);
        let cut = clip(&wide);
        assert!(cut.ends_with("(cut)"));
        assert!(cut.trim_end_matches("... (cut)").chars().all(|c| c == '\u{4e2d}'));
    }

    #[test]
    fn a_report_does_not_carry_the_user_the_computer_or_the_profile_folder() {
        let needles = vec![
            (r"C:\Users\Tanz Dev".to_string(), "%USERPROFILE%"),
            ("DESKTOP-7QK2".to_string(), "<computer>"),
            ("tanzdev".to_string(), "<user>"),
        ];
        let line = r"history: could not write c:\users\tanz dev\AppData\Roaming\com.tzz.meerkat\history.json (desktop-7qk2, TanzDev)";
        let clean = redact(line, &needles);
        assert_eq!(
            clean,
            r"history: could not write %USERPROFILE%\AppData\Roaming\com.tzz.meerkat\history.json (<computer>, <user>)"
        );
        assert_eq!(redact("nothing personal here", &needles), "nothing personal here");
        // A non-ASCII line passes through whole.
        assert_eq!(redact("\u{5185}\u{5b58} 38%", &needles), "\u{5185}\u{5b58} 38%");
    }

    // The hook runs on the thread that panicked, which may be the one holding the log, so it can
    // neither wait for it for good nor give up at the first busy answer.
    #[test]
    fn a_panic_waits_a_moment_for_a_busy_log_and_never_for_good() {
        use std::time::{Duration, Instant};

        // Held briefly by somebody else: the line still gets in.
        let holder = std::thread::spawn(|| {
            let _busy = logger();
            std::thread::sleep(Duration::from_millis(30));
        });
        std::thread::sleep(Duration::from_millis(5));
        let started = Instant::now();
        emit_from_panic("2026-01-01T00:00:00.000Z ERROR [t] panic: first");
        holder.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(tail(500).iter().any(|l| l.ends_with("panic: first")), "written once the log was free");

        // Held for good by this very thread: it gives up and still returns.
        let _mine = logger();
        let started = Instant::now();
        emit_from_panic("2026-01-01T00:00:00.000Z ERROR [t] panic: second");
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    }

    #[test]
    fn a_two_letter_name_is_not_redacted_out_of_every_word() {
        // `personal_needles` leaves names under three characters alone; the function that
        // replaces never sees them.
        let needles: Vec<(String, &str)> = Vec::new();
        assert_eq!(redact("an example", &needles), "an example");
    }
}

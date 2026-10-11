//! `env_logger` plus a small in-memory ring of recent lines for the
//! Settings page's log viewer.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;

const KEEP: usize = 300;
/// Longest line kept for the in-app viewer.
const MAX_LINE: usize = 400;

static LINES: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

struct Capture {
    inner: env_logger::Logger,
}

impl log::Log for Capture {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        if !self.inner.matches(record) {
            return;
        }
        self.inner.log(record);
        let t = START
            .get()
            .map(|s| s.elapsed().as_secs_f32())
            .unwrap_or(0.0);
        // One line per record, bounded in length: a multi-line debug dump
        // (iced logs its whole window settings, icon pixels included) would
        // otherwise become a 60 000-line text widget on the Settings page
        // and freeze the window.
        let body = record.args().to_string();
        let first = body.lines().next().unwrap_or("");
        let mut shown: String = first.chars().take(MAX_LINE).collect();
        if shown.len() < first.len() || body.contains('\n') {
            shown.push('…');
        }
        let line = format!(
            "{t:8.2}s {:<5} {}: {shown}",
            record.level(),
            record.target(),
        );
        if let Ok(mut l) = LINES.lock() {
            if l.len() >= KEEP {
                l.pop_front();
            }
            l.push_back(line);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

pub fn init(default_filter: &str) {
    let _ = START.set(Instant::now());
    let inner =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_filter))
            .build();
    let level = inner.filter();
    if log::set_boxed_logger(Box::new(Capture { inner })).is_ok() {
        log::set_max_level(level);
    }
}

pub fn lines() -> Vec<String> {
    LINES
        .lock()
        .map(|l| l.iter().cloned().collect())
        .unwrap_or_default()
}

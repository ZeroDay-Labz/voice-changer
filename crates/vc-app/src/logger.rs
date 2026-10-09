//! `env_logger` plus a small in-memory ring of recent lines for the
//! Settings page's log viewer.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;

const KEEP: usize = 300;

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
        let line = format!(
            "{t:8.2}s {:<5} {}: {}",
            record.level(),
            record.target(),
            record.args()
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

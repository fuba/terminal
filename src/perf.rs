//! Lightweight frame-timing profiler.
//!
//! Each WM_PAINT / WM_PTY_OUTPUT handler builds a [`FrameTimer`], marks the
//! end of each phase, and calls [`FrameTimer::finish`]. Frames slower than the
//! threshold are appended to `%APPDATA%\terminal\perf.log` with a per-phase
//! breakdown. Fast frames only update in-memory counters.
//!
//! Threshold is 16ms by default; override with the `TERMINAL_SLOW_FRAME_MS`
//! environment variable.

use chrono::Local;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_MARKS: usize = 6;
const SUMMARY_EVERY_N_SLOW: u64 = 50;

struct Stats {
    total: u64,
    slow: u64,
    max: Duration,
    last_summary_slow: u64,
}

static STATS: Mutex<Stats> = Mutex::new(Stats {
    total: 0,
    slow: 0,
    max: Duration::ZERO,
    last_summary_slow: 0,
});

pub struct FrameTimer {
    start: Instant,
    marks: [(&'static str, Instant); MAX_MARKS],
    n: usize,
}

impl FrameTimer {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            start: now,
            marks: [("", now); MAX_MARKS],
            n: 0,
        }
    }

    pub fn mark(&mut self, label: &'static str) {
        if self.n < MAX_MARKS {
            self.marks[self.n] = (label, Instant::now());
            self.n += 1;
        }
    }

    pub fn finish(self, kind: &'static str) {
        let end = Instant::now();
        let total = end - self.start;

        let (slow_count, total_count, needs_summary) = {
            let mut s = STATS.lock().unwrap();
            s.total += 1;
            if total > s.max {
                s.max = total;
            }
            if total < threshold() {
                return;
            }
            s.slow += 1;
            let needs = s.slow - s.last_summary_slow >= SUMMARY_EVERY_N_SLOW;
            if needs {
                s.last_summary_slow = s.slow;
            }
            (s.slow, s.total, needs)
        };

        let mut parts = String::new();
        let mut prev = self.start;
        for i in 0..self.n {
            let (label, t) = self.marks[i];
            parts.push_str(&format!(" {}={:.2}ms", label, ms(t - prev)));
            prev = t;
        }
        if self.n > 0 {
            parts.push_str(&format!(" tail={:.2}ms", ms(end - prev)));
        }
        log_line(&format!(
            "[{}] total={:.2}ms{}",
            kind,
            ms(total),
            parts
        ));

        if needs_summary {
            log_line(&format!(
                "[summary] frames={} slow={} ({:.2}%)",
                total_count,
                slow_count,
                slow_count as f64 * 100.0 / total_count as f64
            ));
        }
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn threshold() -> Duration {
    static T: OnceLock<Duration> = OnceLock::new();
    *T.get_or_init(|| {
        std::env::var("TERMINAL_SLOW_FRAME_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(16))
    })
}

fn log_path() -> PathBuf {
    std::env::var("APPDATA")
        .map(|d| PathBuf::from(d).join("terminal").join("perf.log"))
        .unwrap_or_else(|_| PathBuf::from("perf.log"))
}

fn log_file() -> Option<&'static Mutex<std::fs::File>> {
    static F: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();
    F.get_or_init(|| {
        let path = log_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()
            .map(Mutex::new)
    })
    .as_ref()
}

fn log_line(msg: &str) {
    let ts = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    if let Some(m) = log_file() {
        if let Ok(mut f) = m.lock() {
            let _ = writeln!(f, "{} {}", ts, msg);
        }
    }
    eprintln!("{} perf {}", ts, msg);
}

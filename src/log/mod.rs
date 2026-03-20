use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

pub struct SessionLogger {
    file: Option<File>,
    path: PathBuf,
    strip_escapes: bool,
    in_escape: bool,
}

impl SessionLogger {
    pub fn new(path: PathBuf, strip_escapes: bool) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;

        // Write header
        let mut logger = SessionLogger {
            file: Some(file),
            path,
            strip_escapes,
            in_escape: false,
        };
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(logger.file.as_mut().unwrap(), "\n=== Session started: {} ===\n", timestamp);
        Ok(logger)
    }

    pub fn log(&mut self, data: &[u8]) {
        let file = match &mut self.file {
            Some(f) => f,
            None => return,
        };

        if self.strip_escapes {
            // Write only printable characters, stripping escape sequences
            for &b in data {
                if self.in_escape {
                    // Inside escape sequence, skip until end
                    if b.is_ascii_alphabetic() || b == b'~' {
                        self.in_escape = false;
                    }
                    continue;
                }
                match b {
                    0x1B => self.in_escape = true,
                    0x07 => {} // BEL
                    0x08 => {} // BS
                    0x0A => { let _ = file.write_all(b"\n"); }
                    0x0D => {} // CR - skip, LF handles newlines
                    0x20..=0x7E => { let _ = file.write_all(&[b]); }
                    0x80.. => { let _ = file.write_all(&[b]); } // UTF-8 continuation
                    _ => {}
                }
            }
        } else {
            let _ = file.write_all(data);
        }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl Drop for SessionLogger {
    fn drop(&mut self) {
        if let Some(ref mut f) = self.file {
            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            let _ = writeln!(f, "\n=== Session ended: {} ===", timestamp);
        }
    }
}

/// Generate a log file path for a new session
pub fn new_log_path() -> PathBuf {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("logs")))
        .unwrap_or_else(|| PathBuf::from("logs"));
    let _ = std::fs::create_dir_all(&dir);
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    dir.join(format!("session_{}.log", timestamp))
}

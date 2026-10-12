pub mod conpty;
pub mod ssh;
pub mod ssh_config;

use std::io;
use std::path::PathBuf;

/// Abstract PTY backend for a terminal tab.
/// Implementations forward input to a process/session and expose resize.
pub trait PtyBackend: Send {
    fn write(&mut self, data: &[u8]) -> io::Result<()>;
    fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()>;

    fn supports_clipboard_upload(&self) -> bool {
        false
    }

    /// Upload a clipboard image and insert its path at the cursor.
    /// Local PTYs do not handle this; SSH backends can override it.
    fn paste_image(&mut self, _png: Vec<u8>, _bracketed_paste: bool) -> io::Result<bool> {
        Ok(false)
    }

    /// Upload regular files copied from the Windows shell and insert their paths.
    fn paste_files(&mut self, _paths: Vec<PathBuf>, _bracketed_paste: bool) -> io::Result<bool> {
        Ok(false)
    }
}

pub use conpty::ConPty;
pub use ssh::{SshProfile, SshPty};

impl PtyBackend for ConPty {
    fn write(&mut self, data: &[u8]) -> io::Result<()> {
        ConPty::write(self, data)
    }
    fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        ConPty::resize(self, cols, rows).map_err(|e| io::Error::other(e.to_string()))
    }
}

pub mod conpty;
pub mod ssh;
pub mod ssh_config;

use std::io;

/// Abstract PTY backend for a terminal tab.
/// Implementations forward input to a process/session and expose resize.
pub trait PtyBackend: Send {
    fn write(&mut self, data: &[u8]) -> io::Result<()>;
    fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()>;
}

pub use conpty::ConPty;
pub use ssh::{SshPty, SshProfile, SshAuth};

impl PtyBackend for ConPty {
    fn write(&mut self, data: &[u8]) -> io::Result<()> {
        ConPty::write(self, data)
    }
    fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        ConPty::resize(self, cols, rows)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
    }
}

pub mod cell;
pub mod grid;
pub mod handler;
pub mod mouse;
pub mod parser;
pub mod selection;

use cell::{CellAttrs, Color};
use grid::Grid;
use parser::Parser;

#[derive(Clone)]
pub struct Pen {
    pub fg: Color,
    pub bg: Color,
    pub attrs: CellAttrs,
}

impl Default for Pen {
    fn default() -> Self {
        Pen {
            fg: Color::Default,
            bg: Color::Default,
            attrs: CellAttrs::default(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, PartialOrd)]
pub enum MouseMode {
    None,
    Press,        // 1000 - report button press/release
    ButtonMotion, // 1002 - press/release + motion while pressed
    AnyMotion,    // 1003 - all motion events
}

#[derive(Clone, Copy, PartialEq)]
pub enum MouseEncoding {
    Normal, // X10 encoding
    Sgr,    // 1006 - SGR extended
}

pub struct Modes {
    pub cursor_keys_application: bool, // DECCKM mode 1
    pub auto_wrap: bool,               // DECAWM mode 7
    pub origin_mode: bool,             // DECOM mode 6
    pub bracketed_paste: bool,         // mode 2004
    pub mouse_mode: MouseMode,
    pub mouse_encoding: MouseEncoding,
    pub focus_events: bool, // mode 1004
}

impl Default for Modes {
    fn default() -> Self {
        Modes {
            cursor_keys_application: false,
            auto_wrap: true,
            origin_mode: false,
            bracketed_paste: false,
            mouse_mode: MouseMode::None,
            mouse_encoding: MouseEncoding::Normal,
            focus_events: false,
        }
    }
}

pub struct Terminal {
    pub grid: Grid,
    parser: Parser,
    pub pen: Pen,
    pub modes: Modes,
    pub responses: Vec<Vec<u8>>,
    /// OSC 52 writes are UI actions, never data to echo back to the PTY.
    pub clipboard_writes: Vec<String>,
    pub title: String,
    pub images: Vec<crate::image::TerminalImage>,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        Terminal {
            grid: Grid::new(cols, rows),
            parser: Parser::new(),
            pen: Pen::default(),
            modes: Modes::default(),
            responses: Vec::new(),
            clipboard_writes: Vec::new(),
            title: String::new(),
            images: Vec::new(),
        }
    }

    pub fn process(&mut self, data: &[u8]) {
        let actions = self.parser.feed(data);
        for action in actions {
            handler::handle(self, action);
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.grid.resize(cols, rows);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn tmux_clipboard_is_delivered_to_ui_without_echoing_to_pty() {
        let mut terminal = Terminal::new(80, 24);
        let text = "日本語の選択\nsecond line";
        let encoded = base64::engine::general_purpose::STANDARD.encode(text);
        for terminator in ["\x07", "\x1b\\"] {
            let sequence = format!("\x1b]52;c;{encoded}{terminator}");
            // SSH/ConPTY may split the sequence at any byte boundary.
            for byte in sequence.bytes() {
                terminal.process(&[byte]);
            }
            assert_eq!(terminal.clipboard_writes, [text]);
            assert!(terminal.responses.is_empty());
            terminal.clipboard_writes.clear();
        }
    }

    #[test]
    fn clipboard_queries_and_invalid_data_are_not_echoed() {
        let mut terminal = Terminal::new(80, 24);
        terminal.process(b"\x1b]52;c;?\x07\x1b]52;c;invalid!\x07");
        assert!(terminal.clipboard_writes.is_empty());
        assert!(terminal.responses.is_empty());
        terminal.process(b"\x1b[6n");
        assert!(!terminal.responses.is_empty());
    }

    #[test]
    fn tmux_enables_drag_tracking_and_sgr_encoding() {
        let mut terminal = Terminal::new(80, 24);
        terminal.process(b"\x1b[?1002h\x1b[?1006h");
        assert!(terminal.modes.mouse_mode == MouseMode::ButtonMotion);
        assert!(terminal.modes.mouse_encoding == MouseEncoding::Sgr);
        terminal.process(b"\x1b[?1002l\x1b[?1006l");
        assert!(terminal.modes.mouse_mode == MouseMode::None);
        assert!(terminal.modes.mouse_encoding == MouseEncoding::Normal);
    }
}

pub mod cell;
pub mod grid;
pub mod handler;
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
    /// Queued responses to send back to PTY (e.g., DSR replies)
    pub responses: Vec<Vec<u8>>,
    /// Window/tab title set by OSC 0/2
    pub title: String,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        Terminal {
            grid: Grid::new(cols, rows),
            parser: Parser::new(),
            pen: Pen::default(),
            modes: Modes::default(),
            responses: Vec::new(),
            title: String::new(),
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

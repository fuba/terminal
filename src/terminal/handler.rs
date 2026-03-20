use super::cell::{Cell, Color};
use super::parser::Action;
use super::{MouseEncoding, MouseMode, Terminal};

pub fn handle(term: &mut Terminal, action: Action) {
    // Any output resets viewport to live
    term.grid.scroll_viewport_to_bottom();

    match action {
        Action::Print(ch) => print_char(term, ch),
        Action::C0Control(byte) => c0_control(term, byte),
        Action::CsiDispatch {
            params,
            private_marker,
            final_byte,
            ..
        } => csi_dispatch(term, &params, private_marker, final_byte),
        Action::EscDispatch {
            intermediates,
            final_byte,
        } => esc_dispatch(term, &intermediates, final_byte),
        Action::OscDispatch(data) => {
            let s = String::from_utf8_lossy(&data);
            // OSC 0;title ST - Set icon name and window title
            // OSC 2;title ST - Set window title
            if let Some(title) = s.strip_prefix("0;").or_else(|| s.strip_prefix("2;")) {
                term.title = title.to_string();
            }
        }
    }
}

fn print_char(term: &mut Terminal, ch: char) {
    use unicode_width::UnicodeWidthChar;
    let width = ch.width().unwrap_or(1).max(1);

    // Auto-wrap
    if term.grid.cursor.col + width > term.grid.cols {
        if term.modes.auto_wrap {
            term.grid.cursor.col = 0;
            linefeed(term);
        } else {
            term.grid.cursor.col = term.grid.cols - width;
        }
    }

    let row = term.grid.cursor.row;
    let col = term.grid.cursor.col;

    {
        let cell = term.grid.cell_mut(row, col);
        cell.ch = ch;
        cell.fg = term.pen.fg;
        cell.bg = term.pen.bg;
        cell.attrs = term.pen.attrs;
        cell.width = width as u8;
    }

    if width == 2 && col + 1 < term.grid.cols {
        let cell = term.grid.cell_mut(row, col + 1);
        cell.ch = ' ';
        cell.width = 0;
        cell.fg = term.pen.fg;
        cell.bg = term.pen.bg;
        cell.attrs = term.pen.attrs;
    }

    term.grid.cursor.col += width;
}

fn linefeed(term: &mut Terminal) {
    if term.grid.cursor.row + 1 >= term.grid.scroll_bottom {
        term.grid.scroll_up();
    } else {
        term.grid.cursor.row += 1;
    }
}

fn reverse_linefeed(term: &mut Terminal) {
    if term.grid.cursor.row <= term.grid.scroll_top {
        term.grid.scroll_down();
    } else {
        term.grid.cursor.row -= 1;
    }
}

fn c0_control(term: &mut Terminal, byte: u8) {
    match byte {
        0x07 => {} // BEL
        0x08 => {
            if term.grid.cursor.col > 0 {
                term.grid.cursor.col -= 1;
            }
        }
        0x09 => {
            let next_tab = (term.grid.cursor.col / 8 + 1) * 8;
            term.grid.cursor.col = next_tab.min(term.grid.cols - 1);
        }
        0x0A | 0x0B | 0x0C => linefeed(term),
        0x0D => {
            term.grid.cursor.col = 0;
        }
        _ => {}
    }
}

fn param(params: &[i32], idx: usize, default: i32) -> i32 {
    params
        .get(idx)
        .copied()
        .map(|p| if p == 0 { default } else { p })
        .unwrap_or(default)
}

fn csi_dispatch(term: &mut Terminal, params: &[i32], private: Option<u8>, final_byte: u8) {
    match final_byte {
        b'A' => {
            let n = param(params, 0, 1) as usize;
            term.grid.cursor.row = term.grid.cursor.row.saturating_sub(n);
        }
        b'B' | b'e' => {
            let n = param(params, 0, 1) as usize;
            term.grid.cursor.row = (term.grid.cursor.row + n).min(term.grid.rows - 1);
        }
        b'C' | b'a' => {
            let n = param(params, 0, 1) as usize;
            term.grid.cursor.col = (term.grid.cursor.col + n).min(term.grid.cols - 1);
        }
        b'D' => {
            let n = param(params, 0, 1) as usize;
            term.grid.cursor.col = term.grid.cursor.col.saturating_sub(n);
        }
        b'E' => {
            let n = param(params, 0, 1) as usize;
            term.grid.cursor.row = (term.grid.cursor.row + n).min(term.grid.rows - 1);
            term.grid.cursor.col = 0;
        }
        b'F' => {
            let n = param(params, 0, 1) as usize;
            term.grid.cursor.row = term.grid.cursor.row.saturating_sub(n);
            term.grid.cursor.col = 0;
        }
        b'G' | b'`' => {
            let col = (param(params, 0, 1) - 1) as usize;
            term.grid.cursor.col = col.min(term.grid.cols - 1);
        }
        b'H' | b'f' => {
            let row = (param(params, 0, 1) - 1) as usize;
            let col = (param(params, 1, 1) - 1) as usize;
            if term.modes.origin_mode {
                term.grid.cursor.row =
                    (term.grid.scroll_top + row).min(term.grid.scroll_bottom - 1);
            } else {
                term.grid.cursor.row = row.min(term.grid.rows - 1);
            }
            term.grid.cursor.col = col.min(term.grid.cols - 1);
        }
        b'J' => {
            let mode = param(params, 0, 0);
            match mode {
                0 => {
                    let row = term.grid.cursor.row;
                    let col = term.grid.cursor.col;
                    for c in col..term.grid.cols {
                        *term.grid.cell_mut(row, c) = Cell::default();
                    }
                    for r in (row + 1)..term.grid.rows {
                        term.grid.clear_row(r);
                    }
                }
                1 => {
                    let row = term.grid.cursor.row;
                    let col = term.grid.cursor.col;
                    for r in 0..row {
                        term.grid.clear_row(r);
                    }
                    for c in 0..=col.min(term.grid.cols - 1) {
                        *term.grid.cell_mut(row, c) = Cell::default();
                    }
                }
                2 | 3 => {
                    for r in 0..term.grid.rows {
                        term.grid.clear_row(r);
                    }
                }
                _ => {}
            }
        }
        b'K' => {
            let mode = param(params, 0, 0);
            let row = term.grid.cursor.row;
            match mode {
                0 => {
                    for c in term.grid.cursor.col..term.grid.cols {
                        *term.grid.cell_mut(row, c) = Cell::default();
                    }
                }
                1 => {
                    for c in 0..=term.grid.cursor.col.min(term.grid.cols - 1) {
                        *term.grid.cell_mut(row, c) = Cell::default();
                    }
                }
                2 => term.grid.clear_row(row),
                _ => {}
            }
        }
        b'L' => {
            let n = param(params, 0, 1) as usize;
            let row = term.grid.cursor.row;
            let bottom = term.grid.scroll_bottom;
            for _ in 0..n {
                if row < bottom {
                    for r in (row + 1..bottom).rev() {
                        self::swap_rows(&mut term.grid, r, r - 1);
                    }
                    term.grid.clear_row(row);
                }
            }
        }
        b'M' => {
            let n = param(params, 0, 1) as usize;
            let row = term.grid.cursor.row;
            let bottom = term.grid.scroll_bottom;
            for _ in 0..n {
                if row < bottom {
                    for r in row..bottom - 1 {
                        self::swap_rows(&mut term.grid, r, r + 1);
                    }
                    term.grid.clear_row(bottom - 1);
                }
            }
        }
        b'P' => {
            let n = param(params, 0, 1) as usize;
            let row = term.grid.cursor.row;
            let col = term.grid.cursor.col;
            for i in 0..(term.grid.cols - col) {
                let src_col = col + i + n;
                if src_col < term.grid.cols {
                    let cell = term.grid.cell(row, src_col).clone();
                    *term.grid.cell_mut(row, col + i) = cell;
                } else {
                    *term.grid.cell_mut(row, col + i) = Cell::default();
                }
            }
        }
        b'S' => {
            let n = param(params, 0, 1) as usize;
            for _ in 0..n {
                term.grid.scroll_up();
            }
        }
        b'T' => {
            let n = param(params, 0, 1) as usize;
            for _ in 0..n {
                term.grid.scroll_down();
            }
        }
        b'X' => {
            let n = param(params, 0, 1) as usize;
            let row = term.grid.cursor.row;
            for i in 0..n {
                let col = term.grid.cursor.col + i;
                if col < term.grid.cols {
                    *term.grid.cell_mut(row, col) = Cell::default();
                }
            }
        }
        b'@' => {
            let n = param(params, 0, 1) as usize;
            let row = term.grid.cursor.row;
            let col = term.grid.cursor.col;
            for i in (0..(term.grid.cols - col)).rev() {
                let src_col = col + i;
                let dst_col = col + i + n;
                if dst_col < term.grid.cols && src_col < term.grid.cols {
                    let cell = term.grid.cell(row, src_col).clone();
                    *term.grid.cell_mut(row, dst_col) = cell;
                }
            }
            for i in 0..n.min(term.grid.cols - col) {
                *term.grid.cell_mut(row, col + i) = Cell::default();
            }
        }
        b'd' => {
            let row = (param(params, 0, 1) - 1) as usize;
            term.grid.cursor.row = row.min(term.grid.rows - 1);
        }
        b'm' => handle_sgr(term, params),
        b'h' => set_mode(term, params, private, true),
        b'l' => set_mode(term, params, private, false),
        b'n' => {
            // DSR - Device Status Report
            if private.is_none() && param(params, 0, 0) == 6 {
                let response = format!(
                    "\x1b[{};{}R",
                    term.grid.cursor.row + 1,
                    term.grid.cursor.col + 1
                );
                term.responses.push(response.into_bytes());
            }
        }
        b'r' => {
            // DECSTBM - Set Scrolling Region
            if private.is_none() {
                let top = (param(params, 0, 1) - 1) as usize;
                let bottom = param(params, 1, term.grid.rows as i32) as usize;
                term.grid.set_scroll_region(top, bottom);
                // Move cursor to home position
                if term.modes.origin_mode {
                    term.grid.cursor.row = term.grid.scroll_top;
                } else {
                    term.grid.cursor.row = 0;
                }
                term.grid.cursor.col = 0;
            }
        }
        b's' => term.grid.save_cursor(),
        b'u' => term.grid.restore_cursor(),
        b't' => {
            // Window manipulation - mostly ignored, but handle size queries
        }
        b'c' => {
            // DA - Device Attributes
            if private.is_none() || private == Some(b'>') {
                let response = b"\x1b[?62;22c".to_vec();
                term.responses.push(response);
            }
        }
        _ => {}
    }
}

fn set_mode(term: &mut Terminal, params: &[i32], private: Option<u8>, set: bool) {
    if private == Some(b'?') {
        for &p in params {
            match p {
                1 => term.modes.cursor_keys_application = set,
                6 => {
                    term.modes.origin_mode = set;
                    if set {
                        term.grid.cursor.row = term.grid.scroll_top;
                    } else {
                        term.grid.cursor.row = 0;
                    }
                    term.grid.cursor.col = 0;
                }
                7 => term.modes.auto_wrap = set,
                25 => term.grid.cursor.visible = set,
                47 | 1047 => {
                    if set {
                        term.grid.enter_alt_screen();
                    } else {
                        term.grid.exit_alt_screen();
                    }
                }
                1000 => {
                    term.modes.mouse_mode = if set { MouseMode::Press } else { MouseMode::None };
                }
                1002 => {
                    term.modes.mouse_mode =
                        if set { MouseMode::ButtonMotion } else { MouseMode::None };
                }
                1003 => {
                    term.modes.mouse_mode =
                        if set { MouseMode::AnyMotion } else { MouseMode::None };
                }
                1004 => term.modes.focus_events = set,
                1006 => {
                    term.modes.mouse_encoding =
                        if set { MouseEncoding::Sgr } else { MouseEncoding::Normal };
                }
                1049 => {
                    if set {
                        term.grid.save_cursor();
                        term.grid.enter_alt_screen();
                    } else {
                        term.grid.exit_alt_screen();
                        term.grid.restore_cursor();
                    }
                }
                2004 => term.modes.bracketed_paste = set,
                _ => {}
            }
        }
    }
}

fn swap_rows(grid: &mut super::grid::Grid, a: usize, b: usize) {
    if a != b && a < grid.rows && b < grid.rows {
        for c in 0..grid.cols {
            let ca = grid.cell(a, c).clone();
            let cb = grid.cell(b, c).clone();
            *grid.cell_mut(a, c) = cb;
            *grid.cell_mut(b, c) = ca;
        }
    }
}

fn handle_sgr(term: &mut Terminal, params: &[i32]) {
    if params.is_empty() {
        term.pen = super::Pen::default();
        return;
    }
    let mut i = 0;
    while i < params.len() {
        match params[i] {
            0 => term.pen = super::Pen::default(),
            1 => term.pen.attrs.bold = true,
            2 => term.pen.attrs.dim = true,
            3 => term.pen.attrs.italic = true,
            4 => term.pen.attrs.underline = true,
            7 => term.pen.attrs.inverse = true,
            8 => term.pen.attrs.hidden = true,
            9 => term.pen.attrs.strikethrough = true,
            21 => term.pen.attrs.underline = true,
            22 => {
                term.pen.attrs.bold = false;
                term.pen.attrs.dim = false;
            }
            23 => term.pen.attrs.italic = false,
            24 => term.pen.attrs.underline = false,
            27 => term.pen.attrs.inverse = false,
            28 => term.pen.attrs.hidden = false,
            29 => term.pen.attrs.strikethrough = false,
            30..=37 => term.pen.fg = Color::Indexed((params[i] - 30) as u8),
            38 => {
                i += 1;
                if i < params.len() {
                    match params[i] {
                        5 => {
                            i += 1;
                            if i < params.len() {
                                term.pen.fg = Color::Indexed(params[i] as u8);
                            }
                        }
                        2 => {
                            if i + 3 < params.len() {
                                term.pen.fg = Color::Rgb(
                                    params[i + 1] as u8,
                                    params[i + 2] as u8,
                                    params[i + 3] as u8,
                                );
                                i += 3;
                            }
                        }
                        _ => {}
                    }
                }
            }
            39 => term.pen.fg = Color::Default,
            40..=47 => term.pen.bg = Color::Indexed((params[i] - 40) as u8),
            48 => {
                i += 1;
                if i < params.len() {
                    match params[i] {
                        5 => {
                            i += 1;
                            if i < params.len() {
                                term.pen.bg = Color::Indexed(params[i] as u8);
                            }
                        }
                        2 => {
                            if i + 3 < params.len() {
                                term.pen.bg = Color::Rgb(
                                    params[i + 1] as u8,
                                    params[i + 2] as u8,
                                    params[i + 3] as u8,
                                );
                                i += 3;
                            }
                        }
                        _ => {}
                    }
                }
            }
            49 => term.pen.bg = Color::Default,
            90..=97 => term.pen.fg = Color::Indexed((params[i] - 90 + 8) as u8),
            100..=107 => term.pen.bg = Color::Indexed((params[i] - 100 + 8) as u8),
            _ => {}
        }
        i += 1;
    }
}

fn esc_dispatch(term: &mut Terminal, _intermediates: &[u8], final_byte: u8) {
    match final_byte {
        b'7' => term.grid.save_cursor(),
        b'8' => term.grid.restore_cursor(),
        b'D' => linefeed(term),
        b'E' => {
            term.grid.cursor.col = 0;
            linefeed(term);
        }
        b'M' => reverse_linefeed(term),
        b'c' => {
            let cols = term.grid.cols;
            let rows = term.grid.rows;
            *term = Terminal::new(cols, rows);
        }
        _ => {}
    }
}

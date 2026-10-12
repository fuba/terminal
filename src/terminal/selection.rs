use super::grid::Grid;

#[derive(Clone, Default)]
pub struct Selection {
    /// Anchor point (where mouse was pressed) in absolute coordinates
    pub anchor: Option<(usize, usize)>,
    /// Current end point in absolute coordinates
    pub end: Option<(usize, usize)>,
}

impl Selection {
    pub fn start_viewport(&mut self, grid: &Grid, row: usize, col: usize) {
        let (row, col) = Self::screen_position(grid, row, col);
        self.start(row, col);
    }

    pub fn update_viewport(&mut self, grid: &Grid, row: usize, col: usize) {
        let (row, col) = Self::screen_position(grid, row, col);
        self.update(row, col);
    }

    fn screen_position(grid: &Grid, row: usize, col: usize) -> (usize, usize) {
        let row = grid.viewport_to_absolute(row);
        let line = grid.absolute_line(row);
        // Either half of a wide glyph selects the whole character.
        let col = if col > 0 && line.get(col).is_some_and(|cell| cell.width == 0) {
            col - 1
        } else {
            col
        };
        (row, col)
    }

    pub fn start(&mut self, row: usize, col: usize) {
        self.anchor = Some((row, col));
        // A click clears the old selection; only a drag starts a new one.
        self.end = None;
    }

    pub fn update(&mut self, row: usize, col: usize) {
        if self.anchor.is_some() && (self.end.is_some() || self.anchor != Some((row, col))) {
            self.end = Some((row, col));
        }
    }

    pub fn clear(&mut self) {
        self.anchor = None;
        self.end = None;
    }

    pub fn is_active(&self) -> bool {
        self.anchor.is_some() && self.end.is_some()
    }

    /// Returns (start, end) normalized so start <= end in row-major order
    pub fn ordered(&self) -> Option<((usize, usize), (usize, usize))> {
        match (self.anchor, self.end) {
            (Some(a), Some(b)) => {
                if a.0 < b.0 || (a.0 == b.0 && a.1 <= b.1) {
                    Some((a, b))
                } else {
                    Some((b, a))
                }
            }
            _ => None,
        }
    }

    pub fn contains(&self, row: usize, col: usize) -> bool {
        if let Some(((sr, sc), (er, ec))) = self.ordered() {
            if row < sr || row > er {
                return false;
            }
            if sr == er {
                return col >= sc && col <= ec;
            }
            if row == sr {
                return col >= sc;
            }
            if row == er {
                return col <= ec;
            }
            true
        } else {
            false
        }
    }

    /// Extract selected text from grid (including scrollback)
    pub fn extract_text(&self, grid: &Grid) -> String {
        let ((sr, sc), (er, ec)) = match self.ordered() {
            Some(range) => range,
            None => return String::new(),
        };

        let mut result = String::new();
        for row in sr..=er {
            let line = grid.absolute_line(row);
            let col_start = if row == sr { sc } else { 0 };
            let col_end = if row == er {
                ec.min(line.len().saturating_sub(1))
            } else {
                line.len().saturating_sub(1)
            };

            for col in col_start..=col_end {
                if col < line.len() && line[col].width > 0 {
                    result.push(line[col].ch);
                }
            }

            // Trim trailing spaces for non-last lines and add newline
            if row != er {
                let trimmed = result.trim_end_matches(' ');
                result.truncate(trimmed.len());
                result.push('\n');
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_visible_alt_screen_even_when_tmux_requests_mouse_tracking() {
        let mut terminal = crate::terminal::Terminal::new(20, 3);
        terminal.process(b"main screen\x1b[?1049h\x1b[?1002h\x1b[?1006h");
        terminal.process("日本語 | pane".as_bytes());
        assert!(terminal.grid.is_alt_screen);
        let mut selection = Selection::default();
        // Start on the second half of 日, then cross the visible pane border.
        selection.start_viewport(&terminal.grid, 0, 1);
        selection.update_viewport(&terminal.grid, 0, 12);
        assert_eq!(selection.extract_text(&terminal.grid), "日本語 | pane");
        assert!(selection.contains(0, 0));
        assert!(terminal.responses.is_empty());
    }

    #[test]
    fn viewport_selection_keeps_scrollback_position() {
        let mut terminal = crate::terminal::Terminal::new(8, 2);
        terminal.process(b"first\r\nsecond\r\nthird");
        terminal.grid.scroll_viewport_up(1);
        let mut selection = Selection::default();
        selection.start_viewport(&terminal.grid, 0, 0);
        selection.update_viewport(&terminal.grid, 1, 5);
        assert_eq!(selection.extract_text(&terminal.grid), "first\nsecond");
        assert_eq!(terminal.grid.scroll_offset, 1);
    }

    #[test]
    fn click_does_not_swallow_ctrl_c_as_a_copy() {
        let mut selection = Selection::default();
        selection.start(0, 2);
        selection.update(0, 2);
        assert!(!selection.is_active());
        selection.update(0, 4);
        assert!(selection.is_active());
        selection.start(0, 6);
        assert!(!selection.is_active());
    }

    #[test]
    fn reverse_drag_copies_japanese_without_wide_cell_padding() {
        let mut terminal = crate::terminal::Terminal::new(12, 3);
        terminal.process("日本語\r\nabc".as_bytes());
        let mut selection = Selection::default();
        selection.start(1, 2);
        selection.update(0, 0);
        assert_eq!(selection.extract_text(&terminal.grid), "日本語\nabc");
        assert!(selection.contains(0, 0));
        assert!(selection.contains(1, 2));
        assert!(!selection.contains(1, 3));
    }

    #[test]
    fn drag_can_copy_scrollback() {
        let mut terminal = crate::terminal::Terminal::new(6, 2);
        terminal.process(b"first\r\nsecond\r\nthird");
        let mut selection = Selection::default();
        selection.start(0, 0);
        selection.update(1, 5);
        assert_eq!(selection.extract_text(&terminal.grid), "first\nsecond");
    }
}

use super::cell::Cell;

#[derive(Clone)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
    pub visible: bool,
}

impl Default for Cursor {
    fn default() -> Self {
        Cursor { row: 0, col: 0, visible: true }
    }
}

pub struct Grid {
    cells: Vec<Vec<Cell>>,
    pub rows: usize,
    pub cols: usize,
    pub cursor: Cursor,
    saved_cursor: Option<Cursor>,

    // Scrollback
    scrollback: Vec<Vec<Cell>>,
    pub scrollback_limit: usize,
    pub scroll_offset: usize,

    // Scroll region (DECSTBM) - 0-based, bottom is exclusive
    pub scroll_top: usize,
    pub scroll_bottom: usize,

    // Alt screen
    saved_main: Option<SavedScreen>,
    pub is_alt_screen: bool,

    // Cell size hints (set by renderer for image positioning)
    pub cell_width_hint: f32,
    pub cell_height_hint: f32,
}

struct SavedScreen {
    cells: Vec<Vec<Cell>>,
    cursor: Cursor,
    saved_cursor: Option<Cursor>,
    scrollback: Vec<Vec<Cell>>,
}

impl Grid {
    pub fn new(cols: usize, rows: usize) -> Self {
        Grid {
            cells: vec![vec![Cell::default(); cols]; rows],
            rows,
            cols,
            cursor: Cursor::default(),
            saved_cursor: None,
            scrollback: Vec::new(),
            scrollback_limit: 10000,
            scroll_offset: 0,
            scroll_top: 0,
            scroll_bottom: rows,
            saved_main: None,
            is_alt_screen: false,
            cell_width_hint: 0.0,
            cell_height_hint: 0.0,
        }
    }

    pub fn cell(&self, row: usize, col: usize) -> &Cell {
        &self.cells[row][col]
    }

    pub fn cell_mut(&mut self, row: usize, col: usize) -> &mut Cell {
        &mut self.cells[row][col]
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    /// Get a line by absolute index (0 = first scrollback line)
    pub fn absolute_line(&self, abs_row: usize) -> &[Cell] {
        let sb_len = self.scrollback.len();
        if abs_row < sb_len {
            &self.scrollback[abs_row]
        } else {
            let grid_row = abs_row - sb_len;
            if grid_row < self.rows {
                &self.cells[grid_row]
            } else {
                &[]
            }
        }
    }

    /// Get a line for the current viewport (accounting for scroll_offset)
    pub fn viewport_line(&self, viewport_row: usize) -> &[Cell] {
        let sb_len = self.scrollback.len();
        let abs = sb_len as isize - self.scroll_offset as isize + viewport_row as isize;
        if abs < 0 {
            &[]
        } else {
            self.absolute_line(abs as usize)
        }
    }

    /// Convert viewport row to absolute row
    pub fn viewport_to_absolute(&self, viewport_row: usize) -> usize {
        let sb_len = self.scrollback.len();
        (sb_len as isize - self.scroll_offset as isize + viewport_row as isize).max(0) as usize
    }

    pub fn scroll_up(&mut self) {
        self.scroll_up_in_region(self.scroll_top, self.scroll_bottom);
    }

    pub fn scroll_up_in_region(&mut self, top: usize, bottom: usize) {
        if top >= bottom || bottom > self.rows {
            return;
        }

        // If scrolling the full screen (no scroll region), save to scrollback
        if top == 0 && bottom == self.rows && !self.is_alt_screen {
            let row = self.cells.remove(0);
            self.scrollback.push(row);
            if self.scrollback.len() > self.scrollback_limit {
                self.scrollback.remove(0);
            }
            self.cells.push(vec![Cell::default(); self.cols]);
        } else {
            // Scroll within region
            for r in top..bottom - 1 {
                self.cells.swap(r, r + 1);
            }
            let last = bottom - 1;
            self.clear_row(last);
        }
    }

    pub fn scroll_down(&mut self) {
        self.scroll_down_in_region(self.scroll_top, self.scroll_bottom);
    }

    pub fn scroll_down_in_region(&mut self, top: usize, bottom: usize) {
        if top >= bottom || bottom > self.rows {
            return;
        }
        for r in (top + 1..bottom).rev() {
            self.cells.swap(r, r - 1);
        }
        self.clear_row(top);
    }

    pub fn clear_row(&mut self, row: usize) {
        if row < self.rows {
            for col in 0..self.cols {
                self.cells[row][col] = Cell::default();
            }
        }
    }

    pub fn save_cursor(&mut self) {
        self.saved_cursor = Some(self.cursor.clone());
    }

    pub fn restore_cursor(&mut self) {
        if let Some(saved) = self.saved_cursor.take() {
            self.cursor = saved;
        }
    }

    pub fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        if top < bottom && bottom <= self.rows {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
        }
    }

    pub fn reset_scroll_region(&mut self) {
        self.scroll_top = 0;
        self.scroll_bottom = self.rows;
    }

    pub fn enter_alt_screen(&mut self) {
        if self.is_alt_screen {
            return;
        }
        self.saved_main = Some(SavedScreen {
            cells: self.cells.clone(),
            cursor: self.cursor.clone(),
            saved_cursor: self.saved_cursor.clone(),
            scrollback: std::mem::take(&mut self.scrollback),
        });
        self.cells = vec![vec![Cell::default(); self.cols]; self.rows];
        self.cursor = Cursor::default();
        self.saved_cursor = None;
        self.scroll_offset = 0;
        self.is_alt_screen = true;
    }

    pub fn exit_alt_screen(&mut self) {
        if !self.is_alt_screen {
            return;
        }
        if let Some(saved) = self.saved_main.take() {
            self.cells = saved.cells;
            self.cursor = saved.cursor;
            self.saved_cursor = saved.saved_cursor;
            self.scrollback = saved.scrollback;
        }
        self.is_alt_screen = false;
        self.scroll_offset = 0;
    }

    pub fn resize(&mut self, new_cols: usize, new_rows: usize) {
        let mut new_cells = vec![vec![Cell::default(); new_cols]; new_rows];
        let copy_rows = self.rows.min(new_rows);
        let copy_cols = self.cols.min(new_cols);
        for row in 0..copy_rows {
            for col in 0..copy_cols {
                new_cells[row][col] = self.cells[row][col].clone();
            }
        }
        self.cells = new_cells;
        self.rows = new_rows;
        self.cols = new_cols;
        self.cursor.row = self.cursor.row.min(new_rows.saturating_sub(1));
        self.cursor.col = self.cursor.col.min(new_cols.saturating_sub(1));
        self.scroll_bottom = new_rows;
        if self.scroll_top >= new_rows {
            self.scroll_top = 0;
        }
    }

    /// Scroll viewport up (to see older content)
    pub fn scroll_viewport_up(&mut self, lines: usize) {
        let max = self.scrollback.len();
        self.scroll_offset = (self.scroll_offset + lines).min(max);
    }

    /// Scroll viewport down (toward live content)
    pub fn scroll_viewport_down(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
    }

    /// Reset viewport to live (bottom)
    pub fn scroll_viewport_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }
}

use super::grid::Grid;

#[derive(Clone, Default)]
pub struct Selection {
    /// Anchor point (where mouse was pressed) in absolute coordinates
    pub anchor: Option<(usize, usize)>,
    /// Current end point in absolute coordinates
    pub end: Option<(usize, usize)>,
}

impl Selection {
    pub fn start(&mut self, row: usize, col: usize) {
        self.anchor = Some((row, col));
        self.end = Some((row, col));
    }

    pub fn update(&mut self, row: usize, col: usize) {
        self.end = Some((row, col));
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

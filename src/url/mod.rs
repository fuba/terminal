use crate::terminal::grid::Grid;
use regex::Regex;
use std::sync::OnceLock;

fn url_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"https?://[^\s<>"'`\])}]+"#).unwrap())
}

/// Find a URL at the given grid position (row, col).
/// Returns the URL string if found.
pub fn find_url_at(grid: &Grid, row: usize, col: usize) -> Option<String> {
    if row >= grid.rows {
        return None;
    }

    // Build line text and track column mapping
    let mut text = String::new();
    let mut col_to_char: Vec<usize> = Vec::new(); // char_index for each column

    for c in 0..grid.cols {
        let cell = grid.cell(row, c);
        if cell.width > 0 {
            col_to_char.push(text.chars().count());
            text.push(cell.ch);
        } else {
            // Wide char continuation - same char index as previous
            col_to_char.push(if col_to_char.is_empty() {
                0
            } else {
                *col_to_char.last().unwrap()
            });
        }
    }

    if col >= col_to_char.len() {
        return None;
    }
    let char_idx = col_to_char[col];

    // Find URLs and check if char_idx is within any
    let re = url_regex();
    for m in re.find_iter(&text) {
        let match_start = text[..m.start()].chars().count();
        let match_end = text[..m.end()].chars().count();
        if char_idx >= match_start && char_idx < match_end {
            return Some(m.as_str().to_string());
        }
    }

    None
}

/// Find a URL at the given grid position and return (start_col, end_col, url).
pub fn find_url_range_at(grid: &Grid, row: usize, col: usize) -> Option<(usize, usize, String)> {
    if row >= grid.rows {
        return None;
    }

    let mut text = String::new();
    let mut col_to_char: Vec<usize> = Vec::new();
    let mut char_to_col: Vec<usize> = Vec::new();

    for c in 0..grid.cols {
        let cell = grid.cell(row, c);
        if cell.width > 0 {
            col_to_char.push(text.chars().count());
            char_to_col.push(c);
            text.push(cell.ch);
        } else {
            col_to_char.push(if col_to_char.is_empty() {
                0
            } else {
                *col_to_char.last().unwrap()
            });
        }
    }

    if col >= col_to_char.len() {
        return None;
    }
    let char_idx = col_to_char[col];

    let re = url_regex();
    for m in re.find_iter(&text) {
        let match_start = text[..m.start()].chars().count();
        let match_end = text[..m.end()].chars().count();
        if char_idx >= match_start && char_idx < match_end {
            let start_col = if match_start < char_to_col.len() { char_to_col[match_start] } else { 0 };
            let end_col = if match_end <= char_to_col.len() {
                if match_end > 0 && match_end - 1 < char_to_col.len() { char_to_col[match_end - 1] + 1 } else { grid.cols }
            } else {
                grid.cols
            };
            return Some((start_col, end_col, m.as_str().to_string()));
        }
    }

    None
}

/// Open a URL in the default browser using ShellExecute
pub fn open_url(url: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;

    let wide: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        ShellExecuteW(
            None,
            None,
            PCWSTR(wide.as_ptr()),
            None,
            None,
            SW_SHOW,
        );
    }
}

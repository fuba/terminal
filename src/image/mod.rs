pub mod iterm2;
pub mod osc52;
pub mod sixel;

/// Decoded image ready for rendering
pub struct TerminalImage {
    pub data: Vec<u8>, // RGBA pixels
    pub width: u32,
    pub height: u32,
    pub row: usize,      // absolute row position
    pub col: usize,      // column position
    pub cell_cols: usize, // width in cells
    pub cell_rows: usize, // height in cells
}

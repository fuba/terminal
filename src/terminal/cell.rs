#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl Default for Color {
    fn default() -> Self {
        Color::Default
    }
}

#[derive(Clone, Copy, Default)]
pub struct CellAttrs {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub hidden: bool,
    pub strikethrough: bool,
}

#[derive(Clone)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: CellAttrs,
    pub width: u8,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: ' ',
            fg: Color::Default,
            bg: Color::Default,
            attrs: CellAttrs::default(),
            width: 1,
        }
    }
}

pub const ANSI_COLORS: [(u8, u8, u8); 16] = [
    (12, 12, 12),       // 0: Black
    (205, 49, 49),      // 1: Red
    (13, 188, 121),     // 2: Green
    (229, 229, 16),     // 3: Yellow
    (36, 114, 200),     // 4: Blue
    (188, 63, 188),     // 5: Magenta
    (17, 168, 205),     // 6: Cyan
    (204, 204, 204),    // 7: White
    (118, 118, 118),    // 8: Bright Black
    (241, 76, 76),      // 9: Bright Red
    (35, 209, 139),     // 10: Bright Green
    (245, 245, 67),     // 11: Bright Yellow
    (59, 142, 234),     // 12: Bright Blue
    (214, 112, 214),    // 13: Bright Magenta
    (41, 184, 219),     // 14: Bright Cyan
    (242, 242, 242),    // 15: Bright White
];

pub fn color_to_rgb(color: &Color, is_fg: bool) -> (u8, u8, u8) {
    match color {
        Color::Default => {
            if is_fg {
                (204, 204, 204)
            } else {
                (12, 12, 12)
            }
        }
        Color::Indexed(i) => indexed_color(*i),
        Color::Rgb(r, g, b) => (*r, *g, *b),
    }
}

fn indexed_color(index: u8) -> (u8, u8, u8) {
    match index {
        0..=15 => ANSI_COLORS[index as usize],
        16..=231 => {
            let i = index - 16;
            let r = if i / 36 > 0 { (i / 36) * 40 + 55 } else { 0 };
            let g = if (i % 36) / 6 > 0 { ((i % 36) / 6) * 40 + 55 } else { 0 };
            let b = if i % 6 > 0 { (i % 6) * 40 + 55 } else { 0 };
            (r, g, b)
        }
        232..=255 => {
            let v = 8 + (index - 232) * 10;
            (v, v, v)
        }
    }
}

/// Decode Sixel data into RGBA pixels.
/// Input: raw DCS payload after the 'q' introducer.
/// Returns (width, height, rgba_data).
pub fn decode(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut colors: Vec<(u8, u8, u8)> = vec![(0, 0, 0); 256];
    // Default palette: basic 16
    let defaults = [
        (0,0,0),(20,20,80),(80,13,13),(20,80,20),
        (80,20,80),(20,80,80),(80,80,20),(53,53,53),
        (26,26,26),(33,33,100),(100,26,26),(33,100,33),
        (100,33,100),(33,100,100),(100,100,33),(100,100,100),
    ];
    for (i, &(r, g, b)) in defaults.iter().enumerate() {
        colors[i] = ((r * 255 / 100) as u8, (g * 255 / 100) as u8, (b * 255 / 100) as u8);
    }

    let mut pixels: Vec<Vec<(u8, u8, u8, u8)>> = Vec::new(); // rows of pixels
    let mut cur_color: usize = 0;
    let mut x: usize = 0;
    let mut sixel_row: usize = 0;
    let mut max_x: usize = 0;

    // Ensure enough rows
    let ensure_rows = |pixels: &mut Vec<Vec<(u8, u8, u8, u8)>>, row: usize| {
        while pixels.len() <= row {
            pixels.push(Vec::new());
        }
    };

    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        match b {
            b'#' => {
                // Color command
                i += 1;
                let color_idx = parse_num(data, &mut i);
                if i < data.len() && data[i] == b';' {
                    i += 1; // skip ';'
                    let color_type = parse_num(data, &mut i);
                    if i < data.len() && data[i] == b';' { i += 1; }
                    let v1 = parse_num(data, &mut i);
                    if i < data.len() && data[i] == b';' { i += 1; }
                    let v2 = parse_num(data, &mut i);
                    if i < data.len() && data[i] == b';' { i += 1; }
                    let v3 = parse_num(data, &mut i);
                    if color_type == 2 && (color_idx as usize) < colors.len() {
                        // RGB, values 0-100
                        colors[color_idx as usize] = (
                            (v1 * 255 / 100) as u8,
                            (v2 * 255 / 100) as u8,
                            (v3 * 255 / 100) as u8,
                        );
                    }
                }
                cur_color = color_idx as usize;
                continue; // don't increment i again
            }
            b'!' => {
                // Repeat
                i += 1;
                let count = parse_num(data, &mut i) as usize;
                if i < data.len() && data[i] >= 0x3F && data[i] <= 0x7E {
                    let sixel = data[i] - 0x3F;
                    for _ in 0..count {
                        draw_sixel(&mut pixels, &colors, cur_color, sixel, x, sixel_row, &ensure_rows);
                        x += 1;
                    }
                    if x > max_x { max_x = x; }
                }
            }
            b'$' => {
                // Carriage return
                x = 0;
            }
            b'-' => {
                // New line (next sixel row = 6 pixels down)
                sixel_row += 1;
                x = 0;
            }
            0x3F..=0x7E => {
                let sixel = b - 0x3F;
                draw_sixel(&mut pixels, &colors, cur_color, sixel, x, sixel_row, &ensure_rows);
                x += 1;
                if x > max_x { max_x = x; }
            }
            _ => {}
        }
        i += 1;
    }

    if pixels.is_empty() || max_x == 0 {
        return None;
    }

    let height = pixels.len() as u32;
    let width = max_x as u32;
    let mut rgba = vec![0u8; (width * height * 4) as usize];

    for (y, row) in pixels.iter().enumerate() {
        for (x, &(r, g, b, a)) in row.iter().enumerate() {
            if x < width as usize {
                let offset = (y * width as usize + x) * 4;
                rgba[offset] = r;
                rgba[offset + 1] = g;
                rgba[offset + 2] = b;
                rgba[offset + 3] = a;
            }
        }
    }

    Some((width, height, rgba))
}

fn draw_sixel(
    pixels: &mut Vec<Vec<(u8, u8, u8, u8)>>,
    colors: &[(u8, u8, u8)],
    color_idx: usize,
    sixel: u8,
    x: usize,
    sixel_row: usize,
    ensure_rows: &dyn Fn(&mut Vec<Vec<(u8, u8, u8, u8)>>, usize),
) {
    let (r, g, b) = colors.get(color_idx).copied().unwrap_or((255, 255, 255));
    for bit in 0..6u8 {
        if sixel & (1 << bit) != 0 {
            let y = sixel_row * 6 + bit as usize;
            ensure_rows(pixels, y);
            let row = &mut pixels[y];
            while row.len() <= x {
                row.push((0, 0, 0, 0));
            }
            row[x] = (r, g, b, 255);
        }
    }
}

fn parse_num(data: &[u8], i: &mut usize) -> i32 {
    let mut n: i32 = 0;
    while *i < data.len() && data[*i].is_ascii_digit() {
        n = n * 10 + (data[*i] - b'0') as i32;
        *i += 1;
    }
    n
}

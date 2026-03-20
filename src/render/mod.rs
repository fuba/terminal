use crate::terminal::cell::color_to_rgb;
use crate::terminal::selection::Selection;
use crate::terminal::Terminal;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub struct Renderer {
    _factory: ID2D1Factory,
    _dwrite_factory: IDWriteFactory,
    target: Option<ID2D1HwndRenderTarget>,
    text_format: IDWriteTextFormat,
    bold_text_format: IDWriteTextFormat,
    pub cell_width: f32,
    pub cell_height: f32,
    pub bg_rgb: (u8, u8, u8),
    pub fg_rgb: (u8, u8, u8),
}

const TABBAR_PAD: f32 = 4.0;

impl Renderer {
    pub fn new(hwnd: HWND, font_family: &str, font_size: f32, fg: (u8,u8,u8), bg: (u8,u8,u8)) -> windows::core::Result<Self> {
        unsafe {
            let factory: ID2D1Factory =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite_factory: IDWriteFactory =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;

            let wide_family: Vec<u16> = font_family.encode_utf16().chain(std::iter::once(0)).collect();
            let family_pcwstr = windows::core::PCWSTR(wide_family.as_ptr());

            let text_format = dwrite_factory.CreateTextFormat(
                family_pcwstr,
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                font_size,
                w!(""),
            )?;
            let bold_text_format = dwrite_factory.CreateTextFormat(
                family_pcwstr,
                None,
                DWRITE_FONT_WEIGHT_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                font_size,
                w!(""),
            )?;

            let measure_text: [u16; 1] = [b'M' as u16];
            let layout =
                dwrite_factory.CreateTextLayout(&measure_text, &text_format, 1000.0, 1000.0)?;
            let mut metrics = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut metrics)?;
            let cell_width = metrics.width;
            let cell_height = metrics.height;

            let mut rect = RECT::default();
            GetClientRect(hwnd, &mut rect)?;

            let target = factory.CreateHwndRenderTarget(
                &D2D1_RENDER_TARGET_PROPERTIES::default(),
                &D2D1_HWND_RENDER_TARGET_PROPERTIES {
                    hwnd,
                    pixelSize: D2D_SIZE_U {
                        width: (rect.right - rect.left) as u32,
                        height: (rect.bottom - rect.top) as u32,
                    },
                    presentOptions: D2D1_PRESENT_OPTIONS_NONE,
                },
            )?;

            Ok(Renderer {
                _factory: factory,
                _dwrite_factory: dwrite_factory,
                target: Some(target),
                text_format,
                bold_text_format,
                cell_width,
                cell_height,
                bg_rgb: bg,
                fg_rgb: fg,
            })
        }
    }

    pub fn cell_size(&self) -> (f32, f32) {
        (self.cell_width, self.cell_height)
    }

    pub fn tabbar_height(&self) -> f32 {
        self.cell_height + TABBAR_PAD
    }

    pub fn grid_size(&self) -> (usize, usize) {
        if let Some(target) = &self.target {
            let size = unsafe { target.GetSize() };
            let cols = (size.width / self.cell_width).max(1.0) as usize;
            let rows = ((size.height - self.tabbar_height()) / self.cell_height).max(1.0) as usize;
            (cols, rows)
        } else {
            (80, 24)
        }
    }

    /// Returns (plus_x, gear_x, btn_w) for tab bar button hit testing
    pub fn tabbar_buttons(&self) -> (f32, f32, f32) {
        let w = self.dip_width();
        let btn_w = self.tabbar_height();
        let gear_x = w - btn_w;
        let plus_x = gear_x - btn_w;
        (plus_x, gear_x, btn_w)
    }

    /// Check if x is in the close button area of a tab. Returns tab index if so.
    pub fn tab_close_hit(&self, x: f32, tab_count: usize) -> Option<usize> {
        if tab_count == 0 { return None; }
        let tabs_w = self.tabs_area_width();
        let tab_width = (tabs_w / tab_count as f32).min(200.0);
        let close_w = self.tabbar_height() * 0.6;
        for i in 0..tab_count {
            let tab_right = (i + 1) as f32 * tab_width;
            let close_left = tab_right - close_w;
            if x >= close_left && x < tab_right {
                return Some(i);
            }
        }
        None
    }

    pub fn tabs_area_width(&self) -> f32 {
        let w = self.dip_width();
        let btn_w = self.tabbar_height();
        w - btn_w * 2.0
    }

    pub fn dip_width(&self) -> f32 {
        if let Some(target) = &self.target {
            unsafe { target.GetSize().width }
        } else {
            800.0
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) -> windows::core::Result<()> {
        if let Some(target) = &self.target {
            unsafe { target.Resize(&D2D_SIZE_U { width, height })? }
        }
        Ok(())
    }

    pub fn render(
        &self,
        terminal: &Terminal,
        selection: &Selection,
        tab_titles: &[(String, bool)], // (title, is_active)
        hovered_url: Option<(usize, usize, usize)>, // (row, start_col, end_col)
    ) {
        let target = match &self.target {
            Some(t) => t,
            None => return,
        };

        unsafe {
            target.BeginDraw();

            let bg_default = D2D1_COLOR_F {
                r: 12.0 / 255.0, g: 12.0 / 255.0, b: 12.0 / 255.0, a: 1.0,
            };
            target.Clear(Some(&bg_default));

            // --- Tab bar ---
            self.render_tabbar(target, tab_titles);

            // --- Grid ---
            let grid = &terminal.grid;
            let y_off = self.tabbar_height();

            for vp_row in 0..grid.rows {
                let line = grid.viewport_line(vp_row);
                let abs_row = grid.viewport_to_absolute(vp_row);

                for col in 0..grid.cols {
                    if col >= line.len() { break; }
                    let cell = &line[col];
                    if cell.width == 0 { continue; }

                    let x = col as f32 * self.cell_width;
                    let y = y_off + vp_row as f32 * self.cell_height;
                    let w = self.cell_width * cell.width as f32;

                    let selected = selection.contains(abs_row, col);
                    let (fg_rgb, bg_rgb) = if selected {
                        (self.bg_rgb, self.fg_rgb)
                    } else if cell.attrs.inverse {
                        (color_to_rgb(&cell.bg, false), color_to_rgb(&cell.fg, true))
                    } else {
                        (color_to_rgb(&cell.fg, true), color_to_rgb(&cell.bg, false))
                    };

                    if bg_rgb != self.bg_rgb || selected {
                        let bg_c = rgb_color(bg_rgb, 1.0);
                        if let Ok(brush) = target.CreateSolidColorBrush(&bg_c, None) {
                            target.FillRectangle(
                                &D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + self.cell_height },
                                &brush,
                            );
                        }
                    }

                    if cell.ch != ' ' && cell.ch != '\0' {
                        let alpha = if cell.attrs.dim && !selected { 0.5 } else { 1.0 };
                        let fg_c = rgb_color(fg_rgb, alpha);
                        if let Ok(brush) = target.CreateSolidColorBrush(&fg_c, None) {
                            let mut buf = [0u16; 2];
                            let text = cell.ch.encode_utf16(&mut buf);
                            let rect = D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + self.cell_height };
                            let fmt = if cell.attrs.bold { &self.bold_text_format } else { &self.text_format };
                            target.DrawText(text, fmt, &rect, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                        }
                    }

                    if cell.attrs.underline && !selected {
                        let fg_c = rgb_color(fg_rgb, 1.0);
                        if let Ok(brush) = target.CreateSolidColorBrush(&fg_c, None) {
                            let uy = y + self.cell_height - 1.0;
                            target.DrawLine(D2D_POINT_2F { x, y: uy }, D2D_POINT_2F { x: x + w, y: uy }, &brush, 1.0, None);
                        }
                    }
                }
            }

            // --- Cursor ---
            if grid.scroll_offset == 0 && grid.cursor.visible
                && grid.cursor.row < grid.rows && grid.cursor.col < grid.cols
            {
                let cx = grid.cursor.col as f32 * self.cell_width;
                let cy = y_off + grid.cursor.row as f32 * self.cell_height;
                let cc = D2D1_COLOR_F { r: 0.8, g: 0.8, b: 0.8, a: 0.7 };
                if let Ok(brush) = target.CreateSolidColorBrush(&cc, None) {
                    target.FillRectangle(
                        &D2D_RECT_F { left: cx, top: cy, right: cx + self.cell_width, bottom: cy + self.cell_height },
                        &brush,
                    );
                }
            }

            // --- URL hover underline ---
            if let Some((url_row, url_start, url_end)) = hovered_url {
                let ux = url_start as f32 * self.cell_width;
                let uw = (url_end - url_start) as f32 * self.cell_width;
                let uy = y_off + url_row as f32 * self.cell_height + self.cell_height - 1.0;
                let link_c = D2D1_COLOR_F { r: 0.4, g: 0.6, b: 1.0, a: 1.0 };
                if let Ok(brush) = target.CreateSolidColorBrush(&link_c, None) {
                    target.DrawLine(
                        D2D_POINT_2F { x: ux, y: uy },
                        D2D_POINT_2F { x: ux + uw, y: uy },
                        &brush, 1.0, None,
                    );
                }
            }

            // --- Scrollbar ---
            if grid.scroll_offset > 0 {
                let sb_len = grid.scrollback_len();
                let total = sb_len + grid.rows;
                let vp_ratio = grid.rows as f32 / total as f32;
                let pos_ratio = (sb_len - grid.scroll_offset) as f32 / total as f32;
                let bar_h = (grid.rows as f32 * self.cell_height * vp_ratio).max(20.0);
                let bar_y = y_off + grid.rows as f32 * self.cell_height * pos_ratio;
                let bar_x = grid.cols as f32 * self.cell_width - 6.0;
                let sc = D2D1_COLOR_F { r: 0.5, g: 0.5, b: 0.5, a: 0.5 };
                if let Ok(brush) = target.CreateSolidColorBrush(&sc, None) {
                    target.FillRectangle(
                        &D2D_RECT_F { left: bar_x, top: bar_y, right: bar_x + 6.0, bottom: bar_y + bar_h },
                        &brush,
                    );
                }
            }

            let _ = target.EndDraw(None, None);
        }
    }

    unsafe fn render_tabbar(
        &self,
        target: &ID2D1HwndRenderTarget,
        tabs: &[(String, bool)],
    ) {
        let bar_h = self.tabbar_height();
        let size = target.GetSize();

        // Tab bar background
        let bar_bg = D2D1_COLOR_F { r: 0.08, g: 0.08, b: 0.08, a: 1.0 };
        if let Ok(brush) = target.CreateSolidColorBrush(&bar_bg, None) {
            target.FillRectangle(
                &D2D_RECT_F { left: 0.0, top: 0.0, right: size.width, bottom: bar_h },
                &brush,
            );
        }

        // Bottom border
        let border_c = D2D1_COLOR_F { r: 0.25, g: 0.25, b: 0.25, a: 1.0 };
        if let Ok(brush) = target.CreateSolidColorBrush(&border_c, None) {
            target.DrawLine(
                D2D_POINT_2F { x: 0.0, y: bar_h },
                D2D_POINT_2F { x: size.width, y: bar_h },
                &brush, 1.0, None,
            );
        }

        let btn_w = bar_h; // square buttons
        let gear_x = size.width - btn_w;
        let plus_x = gear_x - btn_w;

        // "+" new tab button
        let plus_fg = D2D1_COLOR_F { r: 0.6, g: 0.6, b: 0.6, a: 1.0 };
        if let Ok(brush) = target.CreateSolidColorBrush(&plus_fg, None) {
            let r = D2D_RECT_F { left: plus_x, top: 0.0, right: gear_x, bottom: bar_h };
            let plus: Vec<u16> = "+".encode_utf16().collect();
            target.DrawText(&plus, &self.text_format, &r, &brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        }

        // Gear settings button
        let gear_fg = D2D1_COLOR_F { r: 0.6, g: 0.6, b: 0.6, a: 1.0 };
        if let Ok(brush) = target.CreateSolidColorBrush(&gear_fg, None) {
            let r = D2D_RECT_F { left: gear_x + 2.0, top: 0.0, right: size.width - 2.0, bottom: bar_h };
            let gear: Vec<u16> = "\u{2699}".encode_utf16().collect();
            target.DrawText(&gear, &self.text_format, &r, &brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        }

        if tabs.is_empty() { return; }

        let tabs_area = size.width - btn_w * 2.0;
        let tab_width = (tabs_area / tabs.len() as f32).min(200.0);
        let tab_fg = D2D1_COLOR_F { r: 0.7, g: 0.7, b: 0.7, a: 1.0 };
        let tab_fg_active = D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
        let tab_bg_active = D2D1_COLOR_F { r: 0.15, g: 0.15, b: 0.15, a: 1.0 };
        let accent = D2D1_COLOR_F { r: 0.3, g: 0.6, b: 1.0, a: 1.0 };

        for (i, (title, is_active)) in tabs.iter().enumerate() {
            let x = i as f32 * tab_width;
            let rect = D2D_RECT_F { left: x, top: 0.0, right: x + tab_width, bottom: bar_h };

            if *is_active {
                if let Ok(brush) = target.CreateSolidColorBrush(&tab_bg_active, None) {
                    target.FillRectangle(&rect, &brush);
                }
                // Accent underline for active tab
                if let Ok(brush) = target.CreateSolidColorBrush(&accent, None) {
                    target.FillRectangle(
                        &D2D_RECT_F { left: x, top: bar_h - 2.0, right: x + tab_width, bottom: bar_h },
                        &brush,
                    );
                }
            }

            let close_w = bar_h * 0.6;
            let fg = if *is_active { &tab_fg_active } else { &tab_fg };

            // Tab title (leave room for close button)
            if let Ok(brush) = target.CreateSolidColorBrush(fg, None) {
                let wide: Vec<u16> = title.encode_utf16().collect();
                let text_rect = D2D_RECT_F {
                    left: x + 8.0,
                    top: TABBAR_PAD / 2.0,
                    right: x + tab_width - close_w - 4.0,
                    bottom: bar_h - TABBAR_PAD / 2.0,
                };
                target.DrawText(
                    &wide, &self.text_format, &text_rect, &brush,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            // Close "x" button
            let close_fg = D2D1_COLOR_F { r: 0.5, g: 0.5, b: 0.5, a: 1.0 };
            if let Ok(brush) = target.CreateSolidColorBrush(&close_fg, None) {
                let cx = x + tab_width - close_w;
                let close_rect = D2D_RECT_F {
                    left: cx, top: TABBAR_PAD / 2.0,
                    right: cx + close_w, bottom: bar_h - TABBAR_PAD / 2.0,
                };
                let xmark: Vec<u16> = "\u{00D7}".encode_utf16().collect(); // ×
                target.DrawText(
                    &xmark, &self.text_format, &close_rect, &brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
                );
            }
        }
    }
}

fn rgb_color(rgb: (u8, u8, u8), a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: rgb.0 as f32 / 255.0,
        g: rgb.1 as f32 / 255.0,
        b: rgb.2 as f32 / 255.0,
        a,
    }
}

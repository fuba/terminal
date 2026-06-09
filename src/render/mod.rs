use crate::terminal::cell::{color_to_rgb, Color};
use crate::terminal::selection::Selection;
use crate::terminal::Terminal;
use std::collections::HashMap;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;
use windows::Win32::UI::WindowsAndMessaging::*;

pub struct Renderer {
    factory: ID2D1Factory,
    _dwrite_factory: IDWriteFactory,
    hwnd: HWND,
    target: Option<ID2D1HwndRenderTarget>,
    target_size: D2D_SIZE_U,
    text_format: IDWriteTextFormat,
    bold_text_format: IDWriteTextFormat,
    pub cell_width: f32,
    pub cell_height: f32,
    pub bg_rgb: (u8, u8, u8),
    pub fg_rgb: (u8, u8, u8),
    brushes: BrushCache,
    bitmaps: BitmapCache,
}

const TABBAR_PAD: f32 = 4.0;
/// HRESULT returned by Direct2D when the device is lost (TDR, sleep,
/// driver update, mode change). The render target must be recreated.
const D2DERR_RECREATE_TARGET: windows::core::HRESULT =
    windows::core::HRESULT(0x8899000Cu32 as i32);
const BRUSH_CACHE_MAX: usize = 256;
const BITMAP_CACHE_MAX: usize = 64;

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
            let target_size = D2D_SIZE_U {
                width: (rect.right - rect.left) as u32,
                height: (rect.bottom - rect.top) as u32,
            };

            let target = factory.CreateHwndRenderTarget(
                &D2D1_RENDER_TARGET_PROPERTIES::default(),
                &D2D1_HWND_RENDER_TARGET_PROPERTIES {
                    hwnd,
                    pixelSize: target_size,
                    presentOptions: D2D1_PRESENT_OPTIONS_NONE,
                },
            )?;

            Ok(Renderer {
                factory,
                _dwrite_factory: dwrite_factory,
                hwnd,
                target: Some(target),
                target_size,
                text_format,
                bold_text_format,
                cell_width,
                cell_height,
                bg_rgb: bg,
                fg_rgb: fg,
                brushes: BrushCache::new(),
                bitmaps: BitmapCache::new(),
            })
        }
    }

    /// Recreate the HWND render target (e.g. after device loss). Brush and
    /// bitmap caches are tied to a specific render target, so they are
    /// invalidated whenever we need to rebuild the target.
    fn ensure_target(&mut self) -> bool {
        if self.target.is_some() {
            return true;
        }
        self.brushes.clear();
        self.bitmaps.clear();
        unsafe {
            match self.factory.CreateHwndRenderTarget(
                &D2D1_RENDER_TARGET_PROPERTIES::default(),
                &D2D1_HWND_RENDER_TARGET_PROPERTIES {
                    hwnd: self.hwnd,
                    pixelSize: self.target_size,
                    presentOptions: D2D1_PRESENT_OPTIONS_NONE,
                },
            ) {
                Ok(t) => {
                    self.target = Some(t);
                    true
                }
                Err(_) => false,
            }
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

    /// Returns (plus_x, dropdown_x, gear_x) — plus_x==dropdown_x (combined button)
    pub fn tabbar_buttons(&self) -> (f32, f32, f32) {
        let w = self.dip_width();
        let bar_h = self.tabbar_height();
        let btn_w = bar_h;
        let gear_x = w - btn_w;
        let plus_x = gear_x - btn_w * 1.4;
        (plus_x, plus_x, gear_x)
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
        self.tabbar_buttons().0 // plus_x is the right edge of the tabs area
    }

    pub fn dip_width(&self) -> f32 {
        if let Some(target) = &self.target {
            unsafe { target.GetSize().width }
        } else {
            800.0
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) -> windows::core::Result<()> {
        self.target_size = D2D_SIZE_U { width, height };
        if let Some(target) = &self.target {
            unsafe {
                if let Err(e) = target.Resize(&self.target_size) {
                    if e.code() == D2DERR_RECREATE_TARGET {
                        self.target = None;
                    } else {
                        return Err(e);
                    }
                }
            }
        }
        Ok(())
    }

    pub fn render(
        &mut self,
        terminal: &Terminal,
        selection: &Selection,
        tab_titles: &[(String, bool)],
        hovered_url: Option<(usize, usize, usize)>,
        ime_composition: &str,
    ) {
        if !self.ensure_target() {
            return;
        }

        // Snapshot Copy fields so we can borrow-split self into target + caches.
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let bg_default = self.bg_rgb;
        let fg_default = self.fg_rgb;
        let tabbar_h = cell_height + TABBAR_PAD;

        let target = self.target.as_ref().unwrap();
        let text_format = &self.text_format;
        let bold_text_format = &self.bold_text_format;
        let brushes = &mut self.brushes;
        let bitmaps = &mut self.bitmaps;

        let resolve_fg = |color: &Color| -> (u8, u8, u8) {
            match color {
                Color::Default => fg_default,
                _ => color_to_rgb(color, true),
            }
        };
        let resolve_bg = |color: &Color| -> (u8, u8, u8) {
            match color {
                Color::Default => bg_default,
                _ => color_to_rgb(color, false),
            }
        };

        unsafe {
            target.BeginDraw();
            target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
            let clear_c = D2D1_COLOR_F {
                r: bg_default.0 as f32 / 255.0,
                g: bg_default.1 as f32 / 255.0,
                b: bg_default.2 as f32 / 255.0,
                a: 1.0,
            };
            target.Clear(Some(&clear_c));

            render_tabbar(target, brushes, text_format, cell_height, tab_titles);

            let grid = &terminal.grid;
            let y_off = tabbar_h;

            let mut text_buf: Vec<u16> = Vec::with_capacity(grid.cols.saturating_mul(2));

            for vp_row in 0..grid.rows {
                let line = grid.viewport_line(vp_row);
                let abs_row = grid.viewport_to_absolute(vp_row);
                let y = y_off + vp_row as f32 * cell_height;
                let line_len = line.len();

                // === Pass 1: backgrounds (batch consecutive same-color cells) ===
                let mut bg_start: usize = 0;
                let mut bg_end: usize = 0;
                let mut bg_color: Option<(u8, u8, u8)> = None;

                for col in 0..grid.cols {
                    if col >= line_len { break; }
                    let cell = &line[col];
                    if cell.width == 0 { continue; }

                    let selected = selection.contains(abs_row, col);
                    let bg = if selected {
                        fg_default
                    } else if cell.attrs.inverse {
                        resolve_fg(&cell.fg)
                    } else {
                        resolve_bg(&cell.bg)
                    };
                    let needs_bg = bg != bg_default || selected;
                    let next_end = col + cell.width as usize;

                    if needs_bg {
                        if bg_color == Some(bg) && bg_end == col {
                            bg_end = next_end;
                        } else {
                            if let Some(prev) = bg_color {
                                draw_bg_run(target, brushes, bg_start, bg_end, y, cell_width, cell_height, prev);
                            }
                            bg_start = col;
                            bg_end = next_end;
                            bg_color = Some(bg);
                        }
                    } else if let Some(prev) = bg_color.take() {
                        draw_bg_run(target, brushes, bg_start, bg_end, y, cell_width, cell_height, prev);
                    }
                }
                if let Some(prev) = bg_color {
                    draw_bg_run(target, brushes, bg_start, bg_end, y, cell_width, cell_height, prev);
                }

                // === Pass 2: foreground text (batch by fg/bold/dim; width=2 cells render alone) ===
                let mut tr_start: usize = 0;
                let mut tr_end: usize = 0;
                let mut tr_attrs: Option<((u8, u8, u8), bool, bool)> = None;
                let mut tr_has_visible = false;
                text_buf.clear();

                for col in 0..grid.cols {
                    if col >= line_len { break; }
                    let cell = &line[col];
                    if cell.width == 0 { continue; }

                    let selected = selection.contains(abs_row, col);
                    let fg = if selected {
                        bg_default
                    } else if cell.attrs.inverse {
                        resolve_bg(&cell.bg)
                    } else {
                        resolve_fg(&cell.fg)
                    };
                    let bold = cell.attrs.bold;
                    let dim = cell.attrs.dim && !selected;
                    let has_text = cell.ch != ' ' && cell.ch != '\0';
                    let drawable_ch = if cell.ch == '\0' { ' ' } else { cell.ch };
                    let next_end = col + cell.width as usize;

                    let box_glyph = is_box_glyph(cell.ch);
                    if cell.width >= 2 || box_glyph {
                        // Wide char or block-element glyph: flush current run and render
                        // this cell standalone. Block elements bypass DirectWrite entirely
                        // (see draw_box_glyph) so adjacent cells join without sub-pixel gaps.
                        if let Some((rfg, rbold, rdim)) = tr_attrs.take() {
                            if tr_has_visible {
                                let fmt = if rbold { bold_text_format } else { text_format };
                                draw_text_run(target, brushes, &text_buf, tr_start, tr_end, y, cell_width, cell_height, rfg, rdim, fmt);
                            }
                        }
                        text_buf.clear();
                        tr_has_visible = false;

                        if has_text {
                            let alpha = if dim { 0.5 } else { 1.0 };
                            let x = col as f32 * cell_width;
                            if box_glyph {
                                draw_box_glyph(target, brushes, drawable_ch, x, y, cell_width, cell_height, fg, alpha);
                            } else {
                                let mut buf = [0u16; 2];
                                let s = drawable_ch.encode_utf16(&mut buf);
                                let c = rgb_color(fg, alpha);
                                if let Some(brush) = brushes.get(target, &c) {
                                    let rect = D2D_RECT_F {
                                        left: x, top: y,
                                        right: x + cell.width as f32 * cell_width,
                                        bottom: y + cell_height,
                                    };
                                    let fmt = if bold { bold_text_format } else { text_format };
                                    target.DrawText(s, fmt, &rect, &brush,
                                        D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                                }
                            }
                        }
                    } else {
                        // width == 1
                        let attrs = (fg, bold, dim);
                        if tr_attrs == Some(attrs) && tr_end == col {
                            let mut buf = [0u16; 2];
                            let s = drawable_ch.encode_utf16(&mut buf);
                            text_buf.extend_from_slice(s);
                            tr_end = next_end;
                            if has_text { tr_has_visible = true; }
                        } else {
                            if let Some((rfg, rbold, rdim)) = tr_attrs {
                                if tr_has_visible {
                                    let fmt = if rbold { bold_text_format } else { text_format };
                                    draw_text_run(target, brushes, &text_buf, tr_start, tr_end, y, cell_width, cell_height, rfg, rdim, fmt);
                                }
                            }
                            text_buf.clear();
                            let mut buf = [0u16; 2];
                            let s = drawable_ch.encode_utf16(&mut buf);
                            text_buf.extend_from_slice(s);
                            tr_start = col;
                            tr_end = next_end;
                            tr_attrs = Some(attrs);
                            tr_has_visible = has_text;
                        }
                    }
                }
                if let Some((rfg, rbold, rdim)) = tr_attrs {
                    if tr_has_visible {
                        let fmt = if rbold { bold_text_format } else { text_format };
                        draw_text_run(target, brushes, &text_buf, tr_start, tr_end, y, cell_width, cell_height, rfg, rdim, fmt);
                    }
                }

                // === Pass 3: underlines (batch by color) ===
                let mut ul_start: usize = 0;
                let mut ul_end: usize = 0;
                let mut ul_color: Option<(u8, u8, u8)> = None;

                for col in 0..grid.cols {
                    if col >= line_len { break; }
                    let cell = &line[col];
                    if cell.width == 0 { continue; }

                    let selected = selection.contains(abs_row, col);
                    if cell.attrs.underline && !selected {
                        let fg = if cell.attrs.inverse {
                            resolve_bg(&cell.bg)
                        } else {
                            resolve_fg(&cell.fg)
                        };
                        let next_end = col + cell.width as usize;
                        if ul_color == Some(fg) && ul_end == col {
                            ul_end = next_end;
                        } else {
                            if let Some(prev) = ul_color {
                                draw_underline_run(target, brushes, ul_start, ul_end, y, cell_width, cell_height, prev);
                            }
                            ul_start = col;
                            ul_end = next_end;
                            ul_color = Some(fg);
                        }
                    } else if let Some(prev) = ul_color.take() {
                        draw_underline_run(target, brushes, ul_start, ul_end, y, cell_width, cell_height, prev);
                    }
                }
                if let Some(prev) = ul_color {
                    draw_underline_run(target, brushes, ul_start, ul_end, y, cell_width, cell_height, prev);
                }
            }

            // --- Images (cached bitmaps; key on pixel buffer pointer) ---
            for img in &terminal.images {
                let sb_len = grid.scrollback_len();
                let vp_start = sb_len as isize - grid.scroll_offset as isize;
                let img_vp_row = img.row as isize - vp_start;
                if img_vp_row < 0 || img_vp_row >= grid.rows as isize {
                    continue;
                }
                let ix = img.col as f32 * cell_width;
                let iy = y_off + img_vp_row as f32 * cell_height;
                let iw = img.cell_cols as f32 * cell_width;
                let ih = img.cell_rows as f32 * cell_height;
                let dest = D2D_RECT_F { left: ix, top: iy, right: ix + iw, bottom: iy + ih };

                if let Some(bitmap) = bitmaps.get(target, img) {
                    target.DrawBitmap(
                        &bitmap,
                        Some(&dest),
                        1.0,
                        D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                        None,
                    );
                }
            }

            // --- IME composition (inline) ---
            let ime_advance = if !ime_composition.is_empty()
                && grid.cursor.row < grid.rows
            {
                use unicode_width::UnicodeWidthChar;
                let cx = grid.cursor.col as f32 * cell_width;
                let cy = y_off + grid.cursor.row as f32 * cell_height;
                let comp_cells: usize = ime_composition.chars().map(|c| c.width().unwrap_or(1).max(1)).sum();
                let comp_w = comp_cells as f32 * cell_width;
                let bg_c = D2D1_COLOR_F { r: 0.2, g: 0.3, b: 0.5, a: 1.0 };
                if let Some(brush) = brushes.get(target, &bg_c) {
                    target.FillRectangle(
                        &D2D_RECT_F { left: cx, top: cy, right: cx + comp_w, bottom: cy + cell_height },
                        &brush,
                    );
                }
                let fg_c = D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
                if let Some(brush) = brushes.get(target, &fg_c) {
                    let wide: Vec<u16> = ime_composition.encode_utf16().collect();
                    let r = D2D_RECT_F { left: cx, top: cy, right: cx + comp_w, bottom: cy + cell_height };
                    target.DrawText(&wide, text_format, &r, &brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                    let uy = cy + cell_height - 1.0;
                    target.DrawLine(
                        D2D_POINT_2F { x: cx, y: uy },
                        D2D_POINT_2F { x: cx + comp_w, y: uy },
                        &brush, 1.0, None,
                    );
                }
                comp_cells
            } else { 0 };

            // --- Cursor ---
            if grid.scroll_offset == 0 && grid.cursor.visible
                && grid.cursor.row < grid.rows && grid.cursor.col < grid.cols
            {
                let cx = (grid.cursor.col + ime_advance) as f32 * cell_width;
                let cy = y_off + grid.cursor.row as f32 * cell_height;
                let cc = D2D1_COLOR_F { r: 0.8, g: 0.8, b: 0.8, a: 0.7 };
                if let Some(brush) = brushes.get(target, &cc) {
                    target.FillRectangle(
                        &D2D_RECT_F { left: cx, top: cy, right: cx + cell_width, bottom: cy + cell_height },
                        &brush,
                    );
                }
            }

            // --- URL hover underline ---
            if let Some((url_row, url_start, url_end)) = hovered_url {
                let ux = url_start as f32 * cell_width;
                let uw = (url_end - url_start) as f32 * cell_width;
                let uy = y_off + url_row as f32 * cell_height + cell_height - 1.0;
                let link_c = D2D1_COLOR_F { r: 0.4, g: 0.6, b: 1.0, a: 1.0 };
                if let Some(brush) = brushes.get(target, &link_c) {
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
                let bar_h = (grid.rows as f32 * cell_height * vp_ratio).max(20.0);
                let bar_y = y_off + grid.rows as f32 * cell_height * pos_ratio;
                let bar_x = grid.cols as f32 * cell_width - 6.0;
                let sc = D2D1_COLOR_F { r: 0.5, g: 0.5, b: 0.5, a: 0.5 };
                if let Some(brush) = brushes.get(target, &sc) {
                    target.FillRectangle(
                        &D2D_RECT_F { left: bar_x, top: bar_y, right: bar_x + 6.0, bottom: bar_y + bar_h },
                        &brush,
                    );
                }
            }

            if let Err(e) = target.EndDraw(None, None) {
                if e.code() == D2DERR_RECREATE_TARGET {
                    // Device lost — drop target so the next render recreates it.
                    // Caches get cleared on the next ensure_target before BeginDraw.
                    self.target = None;
                }
            }
        }
    }
}

unsafe fn render_tabbar(
    target: &ID2D1HwndRenderTarget,
    brushes: &mut BrushCache,
    text_format: &IDWriteTextFormat,
    cell_height: f32,
    tabs: &[(String, bool)],
) {
    let bar_h = cell_height + TABBAR_PAD;
    let size = target.GetSize();

    // Tab bar background
    let bar_bg = D2D1_COLOR_F { r: 0.08, g: 0.08, b: 0.08, a: 1.0 };
    if let Some(brush) = brushes.get(target, &bar_bg) {
        target.FillRectangle(
            &D2D_RECT_F { left: 0.0, top: 0.0, right: size.width, bottom: bar_h },
            &brush,
        );
    }

    // Bottom border
    let border_c = D2D1_COLOR_F { r: 0.25, g: 0.25, b: 0.25, a: 1.0 };
    if let Some(brush) = brushes.get(target, &border_c) {
        target.DrawLine(
            D2D_POINT_2F { x: 0.0, y: bar_h },
            D2D_POINT_2F { x: size.width, y: bar_h },
            &brush, 1.0, None,
        );
    }

    let btn_w = bar_h;
    let gear_x = size.width - btn_w;
    let plus_x = gear_x - btn_w * 1.4;

    let btn_fg = D2D1_COLOR_F { r: 0.6, g: 0.6, b: 0.6, a: 1.0 };
    if let Some(brush) = brushes.get(target, &btn_fg) {
        let r = D2D_RECT_F { left: plus_x, top: 0.0, right: gear_x, bottom: bar_h };
        let plus: Vec<u16> = "+ \u{25BE}".encode_utf16().collect();
        target.DrawText(&plus, text_format, &r, &brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);

        let r2 = D2D_RECT_F { left: gear_x + 2.0, top: 0.0, right: size.width - 2.0, bottom: bar_h };
        let gear: Vec<u16> = "\u{2699}".encode_utf16().collect();
        target.DrawText(&gear, text_format, &r2, &brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
    }

    if tabs.is_empty() { return; }

    let tabs_area = plus_x;
    let tab_width = (tabs_area / tabs.len() as f32).min(200.0);
    let tab_fg = D2D1_COLOR_F { r: 0.7, g: 0.7, b: 0.7, a: 1.0 };
    let tab_fg_active = D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
    let tab_bg_active = D2D1_COLOR_F { r: 0.15, g: 0.15, b: 0.15, a: 1.0 };
    let accent = D2D1_COLOR_F { r: 0.3, g: 0.6, b: 1.0, a: 1.0 };
    let close_fg = D2D1_COLOR_F { r: 0.5, g: 0.5, b: 0.5, a: 1.0 };

    for (i, (title, is_active)) in tabs.iter().enumerate() {
        let x = i as f32 * tab_width;
        let rect = D2D_RECT_F { left: x, top: 0.0, right: x + tab_width, bottom: bar_h };

        if *is_active {
            if let Some(brush) = brushes.get(target, &tab_bg_active) {
                target.FillRectangle(&rect, &brush);
            }
            if let Some(brush) = brushes.get(target, &accent) {
                target.FillRectangle(
                    &D2D_RECT_F { left: x, top: bar_h - 2.0, right: x + tab_width, bottom: bar_h },
                    &brush,
                );
            }
        }

        let close_w = bar_h * 0.6;
        let fg = if *is_active { &tab_fg_active } else { &tab_fg };

        if let Some(brush) = brushes.get(target, fg) {
            let wide: Vec<u16> = title.encode_utf16().collect();
            let text_rect = D2D_RECT_F {
                left: x + 8.0,
                top: TABBAR_PAD / 2.0,
                right: x + tab_width - close_w - 4.0,
                bottom: bar_h - TABBAR_PAD / 2.0,
            };
            target.DrawText(
                &wide, text_format, &text_rect, &brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL,
            );
        }

        if let Some(brush) = brushes.get(target, &close_fg) {
            let cx = x + tab_width - close_w;
            let close_rect = D2D_RECT_F {
                left: cx, top: TABBAR_PAD / 2.0,
                right: cx + close_w, bottom: bar_h - TABBAR_PAD / 2.0,
            };
            let xmark: Vec<u16> = "\u{00D7}".encode_utf16().collect();
            target.DrawText(
                &xmark, text_format, &close_rect, &brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }
}

unsafe fn draw_bg_run(
    target: &ID2D1HwndRenderTarget,
    brushes: &mut BrushCache,
    start: usize, end: usize, y: f32,
    cell_width: f32, cell_height: f32,
    color: (u8, u8, u8),
) {
    let bx = start as f32 * cell_width;
    let bw = (end - start) as f32 * cell_width;
    let c = rgb_color(color, 1.0);
    if let Some(brush) = brushes.get(target, &c) {
        target.FillRectangle(
            &D2D_RECT_F { left: bx, top: y, right: bx + bw, bottom: y + cell_height },
            &brush,
        );
    }
}

unsafe fn draw_text_run(
    target: &ID2D1HwndRenderTarget,
    brushes: &mut BrushCache,
    text: &[u16],
    start: usize, end: usize, y: f32,
    cell_width: f32, cell_height: f32,
    fg: (u8, u8, u8), dim: bool,
    fmt: &IDWriteTextFormat,
) {
    if text.is_empty() { return; }
    let alpha = if dim { 0.5 } else { 1.0 };
    let c = rgb_color(fg, alpha);
    if let Some(brush) = brushes.get(target, &c) {
        let x = start as f32 * cell_width;
        let w = (end - start) as f32 * cell_width;
        let rect = D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + cell_height };
        target.DrawText(text, fmt, &rect, &brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
    }
}

/// Box Drawing (U+2500-U+257F) and Block Elements (U+2580-U+259F).
/// These are rendered as primitives instead of going through DirectWrite so
/// adjacent cells join without the sub-pixel gaps that the font's glyph
/// advances introduce.
fn is_box_glyph(ch: char) -> bool {
    matches!(ch, '\u{2500}'..='\u{259F}')
}

unsafe fn draw_box_glyph(
    target: &ID2D1HwndRenderTarget,
    brushes: &mut BrushCache,
    ch: char,
    x: f32, y: f32,
    cw: f32, h: f32,
    fg: (u8, u8, u8), alpha: f32,
) {
    if (ch as u32) < 0x2580 {
        draw_box_drawing(target, brushes, ch, x, y, cw, h, fg, alpha);
        return;
    }

    // Shaded blocks: full-cell fill with reduced alpha.
    let shade = match ch {
        '\u{2591}' => Some(0.25), // ░ LIGHT SHADE
        '\u{2592}' => Some(0.50), // ▒ MEDIUM SHADE
        '\u{2593}' => Some(0.75), // ▓ DARK SHADE
        _ => None,
    };
    if let Some(s) = shade {
        let c = rgb_color(fg, alpha * s);
        if let Some(brush) = brushes.get(target, &c) {
            target.FillRectangle(
                &D2D_RECT_F { left: x, top: y, right: x + cw, bottom: y + h },
                &brush,
            );
        }
        return;
    }

    let c = rgb_color(fg, alpha);
    let brush = match brushes.get(target, &c) {
        Some(b) => b,
        None => return,
    };
    let fill = |l: f32, t: f32, r: f32, b: f32| {
        target.FillRectangle(
            &D2D_RECT_F { left: x + l, top: y + t, right: x + r, bottom: y + b },
            &brush,
        );
    };

    let half_w = cw * 0.5;
    let half_h = h * 0.5;

    match ch {
        // Upper / lower N/8 blocks
        '\u{2580}' => fill(0.0, 0.0, cw, h * 4.0 / 8.0),         // ▀ upper half
        '\u{2581}' => fill(0.0, h * 7.0 / 8.0, cw, h),           // ▁ lower 1/8
        '\u{2582}' => fill(0.0, h * 6.0 / 8.0, cw, h),           // ▂ lower 2/8
        '\u{2583}' => fill(0.0, h * 5.0 / 8.0, cw, h),           // ▃ lower 3/8
        '\u{2584}' => fill(0.0, h * 4.0 / 8.0, cw, h),           // ▄ lower half
        '\u{2585}' => fill(0.0, h * 3.0 / 8.0, cw, h),           // ▅ lower 5/8
        '\u{2586}' => fill(0.0, h * 2.0 / 8.0, cw, h),           // ▆ lower 6/8
        '\u{2587}' => fill(0.0, h * 1.0 / 8.0, cw, h),           // ▇ lower 7/8
        '\u{2588}' => fill(0.0, 0.0, cw, h),                     // █ full block
        '\u{2589}' => fill(0.0, 0.0, cw * 7.0 / 8.0, h),         // ▉ left 7/8
        '\u{258A}' => fill(0.0, 0.0, cw * 6.0 / 8.0, h),         // ▊ left 6/8
        '\u{258B}' => fill(0.0, 0.0, cw * 5.0 / 8.0, h),         // ▋ left 5/8
        '\u{258C}' => fill(0.0, 0.0, cw * 4.0 / 8.0, h),         // ▌ left half
        '\u{258D}' => fill(0.0, 0.0, cw * 3.0 / 8.0, h),         // ▍ left 3/8
        '\u{258E}' => fill(0.0, 0.0, cw * 2.0 / 8.0, h),         // ▎ left 2/8
        '\u{258F}' => fill(0.0, 0.0, cw * 1.0 / 8.0, h),         // ▏ left 1/8
        '\u{2590}' => fill(cw * 4.0 / 8.0, 0.0, cw, h),          // ▐ right half
        '\u{2594}' => fill(0.0, 0.0, cw, h * 1.0 / 8.0),         // ▔ upper 1/8
        '\u{2595}' => fill(cw * 7.0 / 8.0, 0.0, cw, h),          // ▕ right 1/8

        // Quadrants
        '\u{2596}' => fill(0.0, half_h, half_w, h),              // ▖ lower-left
        '\u{2597}' => fill(half_w, half_h, cw, h),               // ▗ lower-right
        '\u{2598}' => fill(0.0, 0.0, half_w, half_h),            // ▘ upper-left
        '\u{2599}' => {                                          // ▙ UL + LL + LR
            fill(0.0, 0.0, half_w, half_h);
            fill(0.0, half_h, cw, h);
        }
        '\u{259A}' => {                                          // ▚ UL + LR
            fill(0.0, 0.0, half_w, half_h);
            fill(half_w, half_h, cw, h);
        }
        '\u{259B}' => {                                          // ▛ UL + UR + LL
            fill(0.0, 0.0, cw, half_h);
            fill(0.0, half_h, half_w, h);
        }
        '\u{259C}' => {                                          // ▜ UL + UR + LR
            fill(0.0, 0.0, cw, half_h);
            fill(half_w, half_h, cw, h);
        }
        '\u{259D}' => fill(half_w, 0.0, cw, half_h),             // ▝ upper-right
        '\u{259E}' => {                                          // ▞ UR + LL
            fill(half_w, 0.0, cw, half_h);
            fill(0.0, half_h, half_w, h);
        }
        '\u{259F}' => {                                          // ▟ UR + LL + LR
            fill(half_w, 0.0, cw, half_h);
            fill(0.0, half_h, cw, h);
        }
        _ => {}
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum LW { None, Light, Heavy, Double }

#[derive(Copy, Clone)]
struct BoxEdges { u: LW, r: LW, d: LW, l: LW }

/// Lookup table mapping each line-based Box Drawing char to the line weight
/// on each of its four sides. Arcs (256D-2570), diagonals (2571-2573), and
/// dashed lines (2504-250B, 254C-254F) are handled separately.
fn box_edges(ch: char) -> Option<BoxEdges> {
    use LW::*;
    const fn e(u: LW, r: LW, d: LW, l: LW) -> BoxEdges { BoxEdges { u, r, d, l } }
    Some(match ch {
        '\u{2500}' => e(None,  Light, None,  Light),  // ─
        '\u{2501}' => e(None,  Heavy, None,  Heavy),  // ━
        '\u{2502}' => e(Light, None,  Light, None),   // │
        '\u{2503}' => e(Heavy, None,  Heavy, None),   // ┃
        '\u{250C}' => e(None,  Light, Light, None),   // ┌
        '\u{250D}' => e(None,  Heavy, Light, None),   // ┍
        '\u{250E}' => e(None,  Light, Heavy, None),   // ┎
        '\u{250F}' => e(None,  Heavy, Heavy, None),   // ┏
        '\u{2510}' => e(None,  None,  Light, Light),  // ┐
        '\u{2511}' => e(None,  None,  Light, Heavy),  // ┑
        '\u{2512}' => e(None,  None,  Heavy, Light),  // ┒
        '\u{2513}' => e(None,  None,  Heavy, Heavy),  // ┓
        '\u{2514}' => e(Light, Light, None,  None),   // └
        '\u{2515}' => e(Light, Heavy, None,  None),   // ┕
        '\u{2516}' => e(Heavy, Light, None,  None),   // ┖
        '\u{2517}' => e(Heavy, Heavy, None,  None),   // ┗
        '\u{2518}' => e(Light, None,  None,  Light),  // ┘
        '\u{2519}' => e(Light, None,  None,  Heavy),  // ┙
        '\u{251A}' => e(Heavy, None,  None,  Light),  // ┚
        '\u{251B}' => e(Heavy, None,  None,  Heavy),  // ┛
        '\u{251C}' => e(Light, Light, Light, None),   // ├
        '\u{251D}' => e(Light, Heavy, Light, None),   // ┝
        '\u{251E}' => e(Heavy, Light, Light, None),   // ┞
        '\u{251F}' => e(Light, Light, Heavy, None),   // ┟
        '\u{2520}' => e(Heavy, Light, Heavy, None),   // ┠
        '\u{2521}' => e(Heavy, Heavy, Light, None),   // ┡
        '\u{2522}' => e(Light, Heavy, Heavy, None),   // ┢
        '\u{2523}' => e(Heavy, Heavy, Heavy, None),   // ┣
        '\u{2524}' => e(Light, None,  Light, Light),  // ┤
        '\u{2525}' => e(Light, None,  Light, Heavy),  // ┥
        '\u{2526}' => e(Heavy, None,  Light, Light),  // ┦
        '\u{2527}' => e(Light, None,  Heavy, Light),  // ┧
        '\u{2528}' => e(Heavy, None,  Heavy, Light),  // ┨
        '\u{2529}' => e(Heavy, None,  Light, Heavy),  // ┩
        '\u{252A}' => e(Light, None,  Heavy, Heavy),  // ┪
        '\u{252B}' => e(Heavy, None,  Heavy, Heavy),  // ┫
        '\u{252C}' => e(None,  Light, Light, Light),  // ┬
        '\u{252D}' => e(None,  Light, Light, Heavy),  // ┭
        '\u{252E}' => e(None,  Heavy, Light, Light),  // ┮
        '\u{252F}' => e(None,  Heavy, Light, Heavy),  // ┯
        '\u{2530}' => e(None,  Light, Heavy, Light),  // ┰
        '\u{2531}' => e(None,  Light, Heavy, Heavy),  // ┱
        '\u{2532}' => e(None,  Heavy, Heavy, Light),  // ┲
        '\u{2533}' => e(None,  Heavy, Heavy, Heavy),  // ┳
        '\u{2534}' => e(Light, Light, None,  Light),  // ┴
        '\u{2535}' => e(Light, Light, None,  Heavy),  // ┵
        '\u{2536}' => e(Light, Heavy, None,  Light),  // ┶
        '\u{2537}' => e(Light, Heavy, None,  Heavy),  // ┷
        '\u{2538}' => e(Heavy, Light, None,  Light),  // ┸
        '\u{2539}' => e(Heavy, Light, None,  Heavy),  // ┹
        '\u{253A}' => e(Heavy, Heavy, None,  Light),  // ┺
        '\u{253B}' => e(Heavy, Heavy, None,  Heavy),  // ┻
        '\u{253C}' => e(Light, Light, Light, Light),  // ┼
        '\u{253D}' => e(Light, Light, Light, Heavy),  // ┽
        '\u{253E}' => e(Light, Heavy, Light, Light),  // ┾
        '\u{253F}' => e(Light, Heavy, Light, Heavy),  // ┿
        '\u{2540}' => e(Heavy, Light, Light, Light),  // ╀
        '\u{2541}' => e(Light, Light, Heavy, Light),  // ╁
        '\u{2542}' => e(Heavy, Light, Heavy, Light),  // ╂
        '\u{2543}' => e(Heavy, Light, Light, Heavy),  // ╃
        '\u{2544}' => e(Heavy, Heavy, Light, Light),  // ╄
        '\u{2545}' => e(Light, Light, Heavy, Heavy),  // ╅
        '\u{2546}' => e(Light, Heavy, Heavy, Light),  // ╆
        '\u{2547}' => e(Heavy, Heavy, Light, Heavy),  // ╇
        '\u{2548}' => e(Light, Heavy, Heavy, Heavy),  // ╈
        '\u{2549}' => e(Heavy, Light, Heavy, Heavy),  // ╉
        '\u{254A}' => e(Heavy, Heavy, Heavy, Light),  // ╊
        '\u{254B}' => e(Heavy, Heavy, Heavy, Heavy),  // ╋
        '\u{2550}' => e(None,   Double, None,   Double),  // ═
        '\u{2551}' => e(Double, None,   Double, None),    // ║
        '\u{2552}' => e(None,   Double, Light,  None),    // ╒
        '\u{2553}' => e(None,   Light,  Double, None),    // ╓
        '\u{2554}' => e(None,   Double, Double, None),    // ╔
        '\u{2555}' => e(None,   None,   Light,  Double),  // ╕
        '\u{2556}' => e(None,   None,   Double, Light),   // ╖
        '\u{2557}' => e(None,   None,   Double, Double),  // ╗
        '\u{2558}' => e(Light,  Double, None,   None),    // ╘
        '\u{2559}' => e(Double, Light,  None,   None),    // ╙
        '\u{255A}' => e(Double, Double, None,   None),    // ╚
        '\u{255B}' => e(Light,  None,   None,   Double),  // ╛
        '\u{255C}' => e(Double, None,   None,   Light),   // ╜
        '\u{255D}' => e(Double, None,   None,   Double),  // ╝
        '\u{255E}' => e(Light,  Double, Light,  None),    // ╞
        '\u{255F}' => e(Double, Light,  Double, None),    // ╟
        '\u{2560}' => e(Double, Double, Double, None),    // ╠
        '\u{2561}' => e(Light,  None,   Light,  Double),  // ╡
        '\u{2562}' => e(Double, None,   Double, Light),   // ╢
        '\u{2563}' => e(Double, None,   Double, Double),  // ╣
        '\u{2564}' => e(None,   Double, Light,  Double),  // ╤
        '\u{2565}' => e(None,   Light,  Double, Light),   // ╥
        '\u{2566}' => e(None,   Double, Double, Double),  // ╦
        '\u{2567}' => e(Light,  Double, None,   Double),  // ╧
        '\u{2568}' => e(Double, Light,  None,   Light),   // ╨
        '\u{2569}' => e(Double, Double, None,   Double),  // ╩
        '\u{256A}' => e(Light,  Double, Light,  Double),  // ╪
        '\u{256B}' => e(Double, Light,  Double, Light),   // ╫
        '\u{256C}' => e(Double, Double, Double, Double),  // ╬
        '\u{2574}' => e(None,  None,  None,  Light),  // ╴
        '\u{2575}' => e(Light, None,  None,  None),   // ╵
        '\u{2576}' => e(None,  Light, None,  None),   // ╶
        '\u{2577}' => e(None,  None,  Light, None),   // ╷
        '\u{2578}' => e(None,  None,  None,  Heavy),  // ╸
        '\u{2579}' => e(Heavy, None,  None,  None),   // ╹
        '\u{257A}' => e(None,  Heavy, None,  None),   // ╺
        '\u{257B}' => e(None,  None,  Heavy, None),   // ╻
        '\u{257C}' => e(None,  Heavy, None,  Light),  // ╼
        '\u{257D}' => e(Light, None,  Heavy, None),   // ╽
        '\u{257E}' => e(None,  Light, None,  Heavy),  // ╾
        '\u{257F}' => e(Heavy, None,  Light, None),   // ╿
        _ => return Option::None,
    })
}

/// Dashed line spec: (dash_count, vertical, heavy).
fn box_dash_spec(ch: char) -> Option<(usize, bool, bool)> {
    Some(match ch {
        '\u{2504}' => (3, false, false), // ┄ light triple-dash horizontal
        '\u{2505}' => (3, false, true),  // ┅ heavy triple-dash horizontal
        '\u{2506}' => (3, true,  false), // ┆ light triple-dash vertical
        '\u{2507}' => (3, true,  true),  // ┇ heavy triple-dash vertical
        '\u{2508}' => (4, false, false), // ┈ light quadruple-dash horizontal
        '\u{2509}' => (4, false, true),  // ┉ heavy quadruple-dash horizontal
        '\u{250A}' => (4, true,  false), // ┊ light quadruple-dash vertical
        '\u{250B}' => (4, true,  true),  // ┋ heavy quadruple-dash vertical
        '\u{254C}' => (2, false, false), // ╌ light double-dash horizontal
        '\u{254D}' => (2, false, true),  // ╍ heavy double-dash horizontal
        '\u{254E}' => (2, true,  false), // ╎ light double-dash vertical
        '\u{254F}' => (2, true,  true),  // ╏ heavy double-dash vertical
        _ => return None,
    })
}

unsafe fn draw_box_drawing(
    target: &ID2D1HwndRenderTarget,
    brushes: &mut BrushCache,
    ch: char,
    x: f32, y: f32,
    cw: f32, h: f32,
    fg: (u8, u8, u8), alpha: f32,
) {
    let color = rgb_color(fg, alpha);
    let brush = match brushes.get(target, &color) {
        Some(b) => b,
        None => return,
    };

    // Line thickness: ~7.5% of cell height for light, ~2x that for heavy.
    let thin = (h * 0.075).max(1.0).round();
    let thick = (thin * 2.0).max(2.0).round();

    if let Some((n, vertical, heavy)) = box_dash_spec(ch) {
        let t = if heavy { thick } else { thin };
        draw_box_dash(target, &brush, n, vertical, t, x, y, cw, h);
        return;
    }

    if matches!(ch, '\u{256D}'..='\u{2570}') {
        // Arcs benefit from anti-aliasing — temporarily switch from the
        // global aliased mode so the curve doesn't look like a staircase.
        target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        draw_box_arc(target, &brush, ch, x, y, cw, h, thin);
        target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
        return;
    }

    if matches!(ch, '\u{2571}'..='\u{2573}') {
        target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        draw_box_diagonal(target, &brush, ch, x, y, cw, h, thin);
        target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
        return;
    }

    if let Some(edges) = box_edges(ch) {
        draw_box_lines(target, &brush, edges, x, y, cw, h, thin, thick);
    }
}

unsafe fn fill_rect(
    target: &ID2D1HwndRenderTarget,
    brush: &ID2D1SolidColorBrush,
    l: f32, t: f32, r: f32, b: f32,
) {
    target.FillRectangle(
        &D2D_RECT_F { left: l, top: t, right: r, bottom: b },
        brush,
    );
}

unsafe fn draw_box_lines(
    target: &ID2D1HwndRenderTarget,
    brush: &ID2D1SolidColorBrush,
    edges: BoxEdges,
    x: f32, y: f32, cw: f32, h: f32,
    thin: f32, thick: f32,
) {
    let cx = x + cw * 0.5;
    let cy = y + h * 0.5;
    let half_thin = thin * 0.5;
    let half_thick = thick * 0.5;
    // Double-line offset: each parallel line's center sits `dbl` from the
    // cell axis, giving a gap of `dbl - half_thin` between strokes.
    let dbl = thin;
    // Each half-stroke is extended past center by `pad` so perpendicular
    // strokes overlap into a clean junction with no gap at the cross.
    let pad = half_thick;

    // RIGHT half
    match edges.r {
        LW::None  => {}
        LW::Light => fill_rect(target, brush, cx - pad, cy - half_thin,  x + cw, cy + half_thin),
        LW::Heavy => fill_rect(target, brush, cx - pad, cy - half_thick, x + cw, cy + half_thick),
        LW::Double => {
            fill_rect(target, brush, cx - pad, cy - dbl - half_thin, x + cw, cy - dbl + half_thin);
            fill_rect(target, brush, cx - pad, cy + dbl - half_thin, x + cw, cy + dbl + half_thin);
        }
    }
    // LEFT half
    match edges.l {
        LW::None  => {}
        LW::Light => fill_rect(target, brush, x, cy - half_thin,  cx + pad, cy + half_thin),
        LW::Heavy => fill_rect(target, brush, x, cy - half_thick, cx + pad, cy + half_thick),
        LW::Double => {
            fill_rect(target, brush, x, cy - dbl - half_thin, cx + pad, cy - dbl + half_thin);
            fill_rect(target, brush, x, cy + dbl - half_thin, cx + pad, cy + dbl + half_thin);
        }
    }
    // DOWN half
    match edges.d {
        LW::None  => {}
        LW::Light => fill_rect(target, brush, cx - half_thin,  cy - pad, cx + half_thin,  y + h),
        LW::Heavy => fill_rect(target, brush, cx - half_thick, cy - pad, cx + half_thick, y + h),
        LW::Double => {
            fill_rect(target, brush, cx - dbl - half_thin, cy - pad, cx - dbl + half_thin, y + h);
            fill_rect(target, brush, cx + dbl - half_thin, cy - pad, cx + dbl + half_thin, y + h);
        }
    }
    // UP half
    match edges.u {
        LW::None  => {}
        LW::Light => fill_rect(target, brush, cx - half_thin,  y, cx + half_thin,  cy + pad),
        LW::Heavy => fill_rect(target, brush, cx - half_thick, y, cx + half_thick, cy + pad),
        LW::Double => {
            fill_rect(target, brush, cx - dbl - half_thin, y, cx - dbl + half_thin, cy + pad);
            fill_rect(target, brush, cx + dbl - half_thin, y, cx + dbl + half_thin, cy + pad);
        }
    }
}

unsafe fn draw_box_arc(
    target: &ID2D1HwndRenderTarget,
    brush: &ID2D1SolidColorBrush,
    ch: char,
    x: f32, y: f32, cw: f32, h: f32,
    thickness: f32,
) {
    use std::f32::consts::PI;
    // For each arc, the ellipse is centered on one of the cell's corners
    // with semi-axes (cw/2, h/2). The arc sweeps the quarter that lies
    // inside the cell; its endpoints are the two relevant edge midpoints.
    let (ecx, ecy, a_start, a_end) = match ch {
        '\u{256D}' => (x + cw, y + h, PI,        1.5 * PI), // ╭ down+right (top-left round)
        '\u{256E}' => (x,      y + h, 1.5 * PI,  2.0 * PI), // ╮ down+left  (top-right round)
        '\u{256F}' => (x,      y,     0.0,       0.5 * PI), // ╯ up+left    (bottom-right round)
        '\u{2570}' => (x + cw, y,     0.5 * PI,  PI),       // ╰ up+right   (bottom-left round)
        _ => return,
    };
    let rx = cw * 0.5;
    let ry = h * 0.5;

    const SEGS: usize = 16;
    let step = (a_end - a_start) / SEGS as f32;
    let mut prev = D2D_POINT_2F {
        x: ecx + rx * a_start.cos(),
        y: ecy + ry * a_start.sin(),
    };
    for i in 1..=SEGS {
        let a = a_start + step * i as f32;
        let next = D2D_POINT_2F {
            x: ecx + rx * a.cos(),
            y: ecy + ry * a.sin(),
        };
        target.DrawLine(prev, next, brush, thickness, None);
        prev = next;
    }
}

unsafe fn draw_box_diagonal(
    target: &ID2D1HwndRenderTarget,
    brush: &ID2D1SolidColorBrush,
    ch: char,
    x: f32, y: f32, cw: f32, h: f32,
    thickness: f32,
) {
    let tl = D2D_POINT_2F { x, y };
    let tr = D2D_POINT_2F { x: x + cw, y };
    let bl = D2D_POINT_2F { x, y: y + h };
    let br = D2D_POINT_2F { x: x + cw, y: y + h };
    match ch {
        '\u{2571}' => { target.DrawLine(bl, tr, brush, thickness, None); } // ╱
        '\u{2572}' => { target.DrawLine(tl, br, brush, thickness, None); } // ╲
        '\u{2573}' => {                                                    // ╳
            target.DrawLine(bl, tr, brush, thickness, None);
            target.DrawLine(tl, br, brush, thickness, None);
        }
        _ => {}
    }
}

unsafe fn draw_box_dash(
    target: &ID2D1HwndRenderTarget,
    brush: &ID2D1SolidColorBrush,
    n: usize, vertical: bool, thickness: f32,
    x: f32, y: f32, cw: f32, h: f32,
) {
    // N dashes with N-1 equal gaps fill the cell from edge to edge.
    let segs = (2 * n - 1) as f32;
    let half_t = thickness * 0.5;
    if vertical {
        let cx = x + cw * 0.5;
        let seg = h / segs;
        for i in 0..n {
            let dy = y + (i as f32 * 2.0) * seg;
            fill_rect(target, brush, cx - half_t, dy, cx + half_t, dy + seg);
        }
    } else {
        let cy = y + h * 0.5;
        let seg = cw / segs;
        for i in 0..n {
            let dx = x + (i as f32 * 2.0) * seg;
            fill_rect(target, brush, dx, cy - half_t, dx + seg, cy + half_t);
        }
    }
}

unsafe fn draw_underline_run(
    target: &ID2D1HwndRenderTarget,
    brushes: &mut BrushCache,
    start: usize, end: usize, y: f32,
    cell_width: f32, cell_height: f32,
    color: (u8, u8, u8),
) {
    let x1 = start as f32 * cell_width;
    let x2 = end as f32 * cell_width;
    let uy = y + cell_height - 1.0;
    let c = rgb_color(color, 1.0);
    if let Some(brush) = brushes.get(target, &c) {
        target.DrawLine(
            D2D_POINT_2F { x: x1, y: uy },
            D2D_POINT_2F { x: x2, y: uy },
            &brush, 1.0, None,
        );
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

// === Brush cache ===
// SolidColorBrush creation in tight per-cell loops was the dominant cost
// in the original renderer. We cache them by quantized RGBA so repeated
// colors within and across frames reuse the same brush instance.

struct BrushCache {
    map: HashMap<u32, ID2D1SolidColorBrush>,
}

impl BrushCache {
    fn new() -> Self {
        Self { map: HashMap::with_capacity(64) }
    }

    fn clear(&mut self) {
        self.map.clear();
    }

    unsafe fn get(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        c: &D2D1_COLOR_F,
    ) -> Option<ID2D1SolidColorBrush> {
        let key = brush_key(c);
        if let Some(b) = self.map.get(&key) {
            return Some(b.clone());
        }
        if self.map.len() >= BRUSH_CACHE_MAX {
            // Pessimistic eviction: dump everything and rebuild. With ~256 entries
            // and typical terminal palettes well under that, this almost never fires.
            self.map.clear();
        }
        match target.CreateSolidColorBrush(c, None) {
            Ok(b) => {
                self.map.insert(key, b.clone());
                Some(b)
            }
            Err(_) => None,
        }
    }
}

fn brush_key(c: &D2D1_COLOR_F) -> u32 {
    let r = (c.r.clamp(0.0, 1.0) * 255.0).round() as u32;
    let g = (c.g.clamp(0.0, 1.0) * 255.0).round() as u32;
    let b = (c.b.clamp(0.0, 1.0) * 255.0).round() as u32;
    let a = (c.a.clamp(0.0, 1.0) * 255.0).round() as u32;
    (r << 24) | (g << 16) | (b << 8) | a
}

// === Bitmap cache ===
// Keyed by the pixel buffer pointer (stable as long as the Vec isn't
// reallocated). Images stored in `terminal.images` are append-only, so the
// pointer for an existing entry remains valid for its lifetime.

struct BitmapCache {
    map: HashMap<usize, (ID2D1Bitmap, u32, u32)>,
}

impl BitmapCache {
    fn new() -> Self {
        Self { map: HashMap::with_capacity(16) }
    }

    fn clear(&mut self) {
        self.map.clear();
    }

    unsafe fn get(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        image: &crate::image::TerminalImage,
    ) -> Option<ID2D1Bitmap> {
        let key = image.data.as_ptr() as usize;
        if let Some((bmp, w, h)) = self.map.get(&key) {
            if *w == image.width && *h == image.height {
                return Some(bmp.clone());
            }
        }
        if self.map.len() >= BITMAP_CACHE_MAX {
            self.map.clear();
        }
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_R8G8B8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        let size = D2D_SIZE_U { width: image.width, height: image.height };
        match target.CreateBitmap(
            size,
            Some(image.data.as_ptr() as *const _),
            image.width * 4,
            &props,
        ) {
            Ok(bmp) => {
                self.map.insert(key, (bmp.clone(), image.width, image.height));
                Some(bmp)
            }
            Err(_) => None,
        }
    }
}

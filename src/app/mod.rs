mod settings;

use crate::config::Config;
use crate::keys::{Action, KeybindingEngine};
use crate::pty::ConPty;
use crate::render::Renderer;
use crate::terminal::selection::Selection;
use crate::terminal::{MouseEncoding, MouseMode, Terminal};
use crate::url;
use std::io::Read;
use std::sync::mpsc;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::System::Memory::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_PTY_OUTPUT: u32 = WM_USER + 1;
const HOTKEY_ID: i32 = 1;

struct Tab {
    terminal: Terminal,
    pty: ConPty,
    rx: mpsc::Receiver<Vec<u8>>,
    selection: Selection,
    high_surrogate: Option<u16>,
    mouse_pressed: bool,
    logger: Option<crate::log::SessionLogger>,
}

struct AppState {
    tabs: Vec<Tab>,
    active_tab: usize,
    renderer: Renderer,
    keys: KeybindingEngine,
    config: Config,
    hwnd: HWND,
    hovered_url: Option<(usize, usize, usize, String)>, // (row, start_col, end_col, url)
    suppress_char: bool, // suppress next WM_CHAR after keybinding consumed WM_KEYDOWN
    dock_height: bool,
    undocked_rect: Option<RECT>, // saved window rect before docking
}

impl AppState {
    fn active(&self) -> &Tab {
        &self.tabs[self.active_tab]
    }
    fn active_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active_tab]
    }

    fn new_tab(&mut self) -> windows::core::Result<()> {
        let (cols, rows) = self.renderer.grid_size();
        let mut terminal = Terminal::new(cols, rows);
        terminal.grid.cell_width_hint = self.renderer.cell_width;
        terminal.grid.cell_height_hint = self.renderer.cell_height;
        let (pty, reader) = ConPty::spawn(&self.config.shell, cols as u16, rows as u16)?;
        let (tx, rx) = mpsc::channel::<Vec<u8>>();

        let hwnd_raw = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            reader_thread(reader, tx, HWND(hwnd_raw as *mut _));
        });

        self.tabs.push(Tab {
            terminal,
            pty,
            rx,
            selection: Selection::default(),
            high_surrogate: None,
            mouse_pressed: false,
            logger: None,
        });
        self.active_tab = self.tabs.len() - 1;
        Ok(())
    }

    fn close_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.tabs.remove(index);
            if self.tabs.is_empty() {
                unsafe { PostQuitMessage(0); }
                return;
            }
            if self.active_tab >= self.tabs.len() {
                self.active_tab = self.tabs.len() - 1;
            }
        }
    }

    fn switch_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active_tab = index;
        }
    }

    fn tab_titles(&self) -> Vec<(String, bool)> {
        self.tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                let title = if tab.terminal.title.is_empty() {
                    format!("Tab {}", i + 1)
                } else {
                    tab.terminal.title.clone()
                };
                (title, i == self.active_tab)
            })
            .collect()
    }

    fn update_window_title(&self) {
        let tab = &self.tabs[self.active_tab];
        let title = if tab.terminal.title.is_empty() {
            "Terminal".to_string()
        } else {
            format!("{} - Terminal", tab.terminal.title)
        };
        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let _ = SetWindowTextW(self.hwnd, windows::core::PCWSTR(wide.as_ptr()));
        }
    }
}

pub fn run() -> windows::core::Result<()> {
    let config = Config::load();

    unsafe {
        let instance = GetModuleHandleW(None)?;

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: HINSTANCE(instance.0),
            hCursor: LoadCursorW(None, IDC_IBEAM)?,
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            lpszClassName: w!("TerminalWindow"),
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("TerminalWindow"),
            w!("Terminal"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT, CW_USEDEFAULT,
            (config.columns * 9).max(400) as i32,   // approximate width
            (config.rows * 20 + 40).max(300) as i32, // approximate height
            None, None, HINSTANCE(instance.0), None,
        )?;

        let fg = Config::parse_color(&config.fg_color);
        let bg = Config::parse_color(&config.bg_color);
        let renderer = Renderer::new(hwnd, &config.font_family, config.font_size, fg, bg)?;

        // Apply opacity (layered window)
        if config.opacity < 100 {
            let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style | WS_EX_LAYERED.0 as isize);
            let alpha = (config.opacity as f32 * 2.55) as u8;
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA);
        }
        let (cols, rows) = renderer.grid_size();

        let mut terminal = Terminal::new(cols, rows);
        terminal.grid.cell_width_hint = renderer.cell_width;
        terminal.grid.cell_height_hint = renderer.cell_height;
        let (pty, reader) = ConPty::spawn(&config.shell, cols as u16, rows as u16)?;
        let (tx, rx) = mpsc::channel::<Vec<u8>>();

        let state = Box::new(AppState {
            tabs: vec![Tab {
                terminal,
                pty,
                rx,
                selection: Selection::default(),
                high_surrogate: None,
                mouse_pressed: false,
                logger: None,
            }],
            active_tab: 0,
            renderer,
            keys: KeybindingEngine::new(),
            config: config.clone(),
            hwnd,
            hovered_url: None,
            suppress_char: false,
            dock_height: false,
            undocked_rect: None,
        });
        let state_ptr = Box::into_raw(state);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);

        // Global hotkey
        if config.hotkey.enabled {
            let mods = config.hotkey.modifier_flags();
            let vk = config.hotkey.virtual_key();
            if let Err(e) = RegisterHotKey(hwnd, HOTKEY_ID, HOT_KEY_MODIFIERS(mods), vk) {
                eprintln!("Failed to register hotkey: {e}");
            }
        }

        let _ = ShowWindow(hwnd, SW_SHOW);

        // Start reader thread for first tab
        let hwnd_raw = hwnd.0 as isize;
        std::thread::spawn(move || {
            reader_thread(reader, tx, HWND(hwnd_raw as *mut _));
        });

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        if config.hotkey.enabled {
            let _ = UnregisterHotKey(hwnd, HOTKEY_ID);
        }
        let _ = Box::from_raw(state_ptr);
        Ok(())
    }
}

fn reader_thread(mut reader: std::fs::File, tx: mpsc::Sender<Vec<u8>>, hwnd: HWND) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if tx.send(buf[..n].to_vec()).is_err() { break; }
                unsafe { let _ = PostMessageW(hwnd, WM_PTY_OUTPUT, WPARAM(0), LPARAM(0)); }
            }
        }
    }
}

fn pixel_to_cell(state: &AppState, x: f32, y: f32) -> (usize, usize) {
    let grid_y = y - state.renderer.tabbar_height();
    let col = (x / state.renderer.cell_width).max(0.0) as usize;
    let row = (grid_y / state.renderer.cell_height).max(0.0) as usize;
    (
        row.min(state.active().terminal.grid.rows.saturating_sub(1)),
        col.min(state.active().terminal.grid.cols.saturating_sub(1)),
    )
}

fn dip_coords(hwnd: HWND, lparam: LPARAM) -> (f32, f32) {
    let px = (lparam.0 & 0xFFFF) as i16 as f32;
    let py = ((lparam.0 >> 16) & 0xFFFF) as i16 as f32;
    let dpi = unsafe { GetDpiForWindow(hwnd) } as f32;
    let scale = dpi / 96.0;
    (px / scale, py / scale)
}

fn get_key_state(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) & 0x8000u16 as i16 != 0 }
}

fn copy_to_clipboard(hwnd: HWND, text: &str) {
    if text.is_empty() { return; }
    unsafe {
        if OpenClipboard(hwnd).is_ok() {
            let _ = EmptyClipboard();
            let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let size = wide.len() * 2;
            if let Ok(hmem) = GlobalAlloc(GMEM_MOVEABLE, size) {
                let ptr = GlobalLock(hmem);
                if !ptr.is_null() {
                    std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr as *mut u16, wide.len());
                    let _ = GlobalUnlock(hmem);
                    let _ = SetClipboardData(13, HANDLE(hmem.0));
                }
            }
            let _ = CloseClipboard();
        }
    }
}

fn paste_from_clipboard(hwnd: HWND) -> Option<String> {
    unsafe {
        if OpenClipboard(hwnd).is_err() { return None; }
        let result = GetClipboardData(13).ok().and_then(|handle| {
            let hmem = HGLOBAL(handle.0);
            let ptr = GlobalLock(hmem) as *const u16;
            if ptr.is_null() { return None; }
            let mut len = 0;
            while *ptr.add(len) != 0 { len += 1; }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            let _ = GlobalUnlock(hmem);
            Some(text)
        });
        let _ = CloseClipboard();
        result
    }
}

fn do_paste(tab: &mut Tab, text: &str) {
    if tab.terminal.modes.bracketed_paste {
        let _ = tab.pty.write(b"\x1b[200~");
        let _ = tab.pty.write(text.as_bytes());
        let _ = tab.pty.write(b"\x1b[201~");
    } else {
        let _ = tab.pty.write(text.as_bytes());
    }
}

fn send_mouse_event(tab: &mut Tab, button: u8, row: usize, col: usize, pressed: bool, motion: bool) {
    if tab.terminal.modes.mouse_mode == MouseMode::None { return; }
    let mut cb = button;
    if motion { cb += 32; }
    if get_key_state(VK_SHIFT) { cb += 4; }
    if get_key_state(VK_MENU) { cb += 8; }
    if get_key_state(VK_CONTROL) { cb += 16; }
    let cx = col + 1;
    let cy = row + 1;
    match tab.terminal.modes.mouse_encoding {
        MouseEncoding::Sgr => {
            let s = if pressed { 'M' } else { 'm' };
            let seq = format!("\x1b[<{};{};{}{}", cb, cx, cy, s);
            let _ = tab.pty.write(seq.as_bytes());
        }
        MouseEncoding::Normal => {
            if cx <= 223 && cy <= 223 {
                let _ = tab.pty.write(&[b'\x1b', b'[', b'M', cb + 32, cx as u8 + 32, cy as u8 + 32]);
            }
        }
    }
}

fn get_work_area() -> RECT {
    unsafe {
        let mut rc = RECT::default();
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA, 0,
            Some(&mut rc as *mut RECT as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        rc
    }
}

fn apply_dock_height(hwnd: HWND) {
    let wa = get_work_area();
    unsafe {
        let mut wr = RECT::default();
        let _ = GetWindowRect(hwnd, &mut wr);
        let _ = SetWindowPos(
            hwnd, None,
            wr.left, wa.top,
            wr.right - wr.left, wa.bottom - wa.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

fn handle_action(state: &mut AppState, action: Action, hwnd: HWND) {
    match action {
        Action::NewTab => { let _ = state.new_tab(); }
        Action::CloseTab => { let idx = state.active_tab; state.close_tab(idx); }
        Action::NextTab => {
            let next = (state.active_tab + 1) % state.tabs.len();
            state.switch_tab(next);
        }
        Action::PrevTab => {
            let prev = if state.active_tab == 0 { state.tabs.len() - 1 } else { state.active_tab - 1 };
            state.switch_tab(prev);
        }
        Action::SelectTab(i) => { state.switch_tab(i); }
        Action::Copy => {
            if state.active().selection.is_active() {
                let text = state.active().selection.extract_text(&state.active().terminal.grid);
                copy_to_clipboard(hwnd, &text);
                state.active_mut().selection.clear();
            } else {
                // No selection → send ^C (SIGINT) to PTY
                let _ = state.active_mut().pty.write(&[0x03]);
            }
        }
        Action::Paste => {
            if let Some(text) = paste_from_clipboard(hwnd) {
                do_paste(state.active_mut(), &text);
            }
        }
        Action::ScrollPageUp => {
            let rows = state.active().terminal.grid.rows;
            state.active_mut().terminal.grid.scroll_viewport_up(rows / 2);
        }
        Action::ScrollPageDown => {
            let rows = state.active().terminal.grid.rows;
            state.active_mut().terminal.grid.scroll_viewport_down(rows / 2);
        }
        Action::ScrollToTop => {
            let max = state.active().terminal.grid.scrollback_len();
            state.active_mut().terminal.grid.scroll_viewport_up(max);
        }
        Action::ScrollToBottom => {
            state.active_mut().terminal.grid.scroll_viewport_to_bottom();
        }
        Action::ToggleLog => {
            let tab = state.active_mut();
            if tab.logger.is_some() {
                tab.logger = None; // stop logging
            } else {
                let path = crate::log::new_log_path();
                if let Ok(logger) = crate::log::SessionLogger::new(path, true) {
                    tab.logger = Some(logger);
                }
            }
        }
        Action::ToggleDockHeight => {
            state.dock_height = !state.dock_height;
            if state.dock_height {
                // Save current rect and snap to full height
                unsafe {
                    let mut rc = RECT::default();
                    let _ = GetWindowRect(hwnd, &mut rc);
                    state.undocked_rect = Some(rc);
                }
                apply_dock_height(hwnd);
            } else {
                // Restore saved rect
                if let Some(rc) = state.undocked_rect.take() {
                    unsafe {
                        let _ = SetWindowPos(
                            hwnd, None,
                            rc.left, rc.top,
                            rc.right - rc.left, rc.bottom - rc.top,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                }
            }
        }
        Action::OpenConfig => {
            if let Some(new_config) = settings::show_settings(hwnd, &state.config) {
                state.config = new_config;
            }
        }
    }
    unsafe { let _ = InvalidateRect(hwnd, None, false); }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
    if state_ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let state = &mut *state_ptr;
    if state.tabs.is_empty() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }

    match msg {
        WM_PAINT => {
            let titles = state.tab_titles();
            let url_hover = state.hovered_url.as_ref().map(|(r, s, e, _)| (*r, *s, *e));
            state.renderer.render(&state.active().terminal, &state.active().selection, &titles, url_hover);
            let _ = ValidateRect(hwnd, None);
            LRESULT(0)
        }
        WM_SIZE => {
            let width = (lparam.0 & 0xFFFF) as u32;
            let height = ((lparam.0 >> 16) & 0xFFFF) as u32;
            if width > 0 && height > 0 {
                let _ = state.renderer.resize(width, height);
                let (cols, rows) = state.renderer.grid_size();
                for tab in &mut state.tabs {
                    if cols != tab.terminal.grid.cols || rows != tab.terminal.grid.rows {
                        tab.terminal.resize(cols, rows);
                        let _ = tab.pty.resize(cols as u16, rows as u16);
                    }
                }
            }
            LRESULT(0)
        }
        WM_CHAR => {
            if state.suppress_char {
                state.suppress_char = false;
                return LRESULT(0);
            }
            let ch = wparam.0 as u16;
            let tab = state.active_mut();
            let c = if (0xD800..=0xDBFF).contains(&ch) {
                tab.high_surrogate = Some(ch);
                return LRESULT(0);
            } else if (0xDC00..=0xDFFF).contains(&ch) {
                if let Some(high) = tab.high_surrogate.take() {
                    let cp = 0x10000 + ((high as u32 - 0xD800) << 10) + (ch as u32 - 0xDC00);
                    char::from_u32(cp).unwrap_or('?')
                } else { return LRESULT(0); }
            } else {
                tab.high_surrogate = None;
                char::from_u32(ch as u32).unwrap_or('?')
            };

            tab.terminal.grid.scroll_viewport_to_bottom();
            tab.selection.clear();
            let mut buf = [0u8; 4];
            let s = c.encode_utf8(&mut buf);
            let _ = tab.pty.write(s.as_bytes());
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = VIRTUAL_KEY(wparam.0 as u16);

            // Check keybindings first
            if let Some(action) = state.keys.match_key(vk) {
                handle_action(state, action, hwnd);
                state.suppress_char = true; // prevent WM_CHAR from sending control char
                return LRESULT(0);
            }

            let tab = state.active_mut();
            let app_mode = tab.terminal.modes.cursor_keys_application;
            let seq: Option<&[u8]> = match vk {
                VK_UP => Some(if app_mode { b"\x1bOA" } else { b"\x1b[A" }),
                VK_DOWN => Some(if app_mode { b"\x1bOB" } else { b"\x1b[B" }),
                VK_RIGHT => Some(if app_mode { b"\x1bOC" } else { b"\x1b[C" }),
                VK_LEFT => Some(if app_mode { b"\x1bOD" } else { b"\x1b[D" }),
                VK_HOME => Some(if app_mode { b"\x1bOH" } else { b"\x1b[H" }),
                VK_END => Some(if app_mode { b"\x1bOF" } else { b"\x1b[F" }),
                VK_BACK => Some(b"\x7f"),
                VK_RETURN => Some(b"\r"),
                VK_TAB => Some(b"\t"),
                VK_ESCAPE => Some(b"\x1b"),
                VK_INSERT => Some(b"\x1b[2~"),
                VK_DELETE => Some(b"\x1b[3~"),
                VK_PRIOR => Some(b"\x1b[5~"),
                VK_NEXT => Some(b"\x1b[6~"),
                VK_F1 => Some(b"\x1bOP"),
                VK_F2 => Some(b"\x1bOQ"),
                VK_F3 => Some(b"\x1bOR"),
                VK_F4 => Some(b"\x1bOS"),
                VK_F5 => Some(b"\x1b[15~"),
                VK_F6 => Some(b"\x1b[17~"),
                VK_F7 => Some(b"\x1b[18~"),
                VK_F8 => Some(b"\x1b[19~"),
                VK_F9 => Some(b"\x1b[20~"),
                VK_F10 => Some(b"\x1b[21~"),
                VK_F11 => Some(b"\x1b[23~"),
                VK_F12 => Some(b"\x1b[24~"),
                _ => None,
            };
            if let Some(seq) = seq {
                tab.terminal.grid.scroll_viewport_to_bottom();
                let _ = tab.pty.write(seq);
                state.suppress_char = true; // prevent duplicate from WM_CHAR
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = dip_coords(hwnd, lparam);
            let tabbar_h = state.renderer.tabbar_height();

            if y < tabbar_h {
                // Tab bar click - check buttons first
                let (plus_x, gear_x, _btn_w) = state.renderer.tabbar_buttons();
                if x >= gear_x {
                    // Gear button → open settings
                    handle_action(state, Action::OpenConfig, hwnd);
                } else if x >= plus_x {
                    // "+" button → new tab
                    handle_action(state, Action::NewTab, hwnd);
                } else {
                    // Tab area - check close button first
                    let tab_count = state.tabs.len();
                    if let Some(idx) = state.renderer.tab_close_hit(x, tab_count) {
                        state.close_tab(idx);
                        let _ = InvalidateRect(hwnd, None, false);
                    } else if tab_count > 0 {
                        let tabs_w = state.renderer.tabs_area_width();
                        let tab_width = (tabs_w / tab_count as f32).min(200.0);
                        let clicked = (x / tab_width) as usize;
                        if clicked < tab_count {
                            state.switch_tab(clicked);
                            let _ = InvalidateRect(hwnd, None, false);
                        }
                    }
                }
                return LRESULT(0);
            }

            let (row, col) = pixel_to_cell(state, x, y);
            let tab = state.active_mut();

            // Ctrl+Click = open URL
            if get_key_state(VK_CONTROL) {
                if let Some(found_url) = url::find_url_at(&tab.terminal.grid, row, col) {
                    url::open_url(&found_url);
                    return LRESULT(0);
                }
            }

            if tab.terminal.modes.mouse_mode != MouseMode::None {
                send_mouse_event(tab, 0, row, col, true, false);
            } else {
                let abs_row = tab.terminal.grid.viewport_to_absolute(row);
                tab.selection.start(abs_row, col);
                tab.mouse_pressed = true;
                SetCapture(hwnd);
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = dip_coords(hwnd, lparam);
            let (row, col) = pixel_to_cell(state, x, y);
            let tab = state.active_mut();

            if tab.terminal.modes.mouse_mode != MouseMode::None {
                let mode = tab.terminal.modes.mouse_mode;
                if tab.mouse_pressed && mode >= MouseMode::ButtonMotion {
                    send_mouse_event(tab, 0, row, col, true, true);
                } else if mode == MouseMode::AnyMotion {
                    send_mouse_event(tab, 3, row, col, true, true);
                }
            } else if tab.mouse_pressed {
                let abs_row = tab.terminal.grid.viewport_to_absolute(row);
                tab.selection.update(abs_row, col);
                let _ = InvalidateRect(hwnd, None, false);
            }

            // Ctrl+hover URL detection
            let old_hovered = state.hovered_url.clone();
            if get_key_state(VK_CONTROL) {
                let tab = state.active();
                if let Some((start_col, end_col, found_url)) =
                    url::find_url_range_at(&tab.terminal.grid, row, col)
                {
                    state.hovered_url = Some((row, start_col, end_col, found_url));
                    if let Ok(cursor) = LoadCursorW(None, IDC_HAND) {
                        SetCursor(cursor);
                    }
                } else {
                    state.hovered_url = None;
                }
            } else {
                state.hovered_url = None;
            }
            if state.hovered_url != old_hovered {
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (x, y) = dip_coords(hwnd, lparam);
            let (row, col) = pixel_to_cell(state, x, y);
            let tab = state.active_mut();
            if tab.terminal.modes.mouse_mode != MouseMode::None {
                send_mouse_event(tab, 0, row, col, false, false);
            }
            tab.mouse_pressed = false;
            let _ = ReleaseCapture();
            LRESULT(0)
        }
        WM_RBUTTONDOWN => {
            if let Some(text) = paste_from_clipboard(hwnd) {
                do_paste(state.active_mut(), &text);
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) as i16) as i32;
            let lines = (delta.abs() / 120).max(1) as usize * 3;
            let tab = state.active_mut();

            if tab.terminal.modes.mouse_mode != MouseMode::None {
                let (x, y) = dip_coords(hwnd, lparam);
                let (row, col) = pixel_to_cell(state, x, y);
                let tab = state.active_mut();
                let button = if delta > 0 { 64 } else { 65 };
                send_mouse_event(tab, button, row, col, true, false);
            } else if delta > 0 {
                tab.terminal.grid.scroll_viewport_up(lines);
                let _ = InvalidateRect(hwnd, None, false);
            } else {
                tab.terminal.grid.scroll_viewport_down(lines);
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_MBUTTONDOWN | WM_MBUTTONUP | WM_RBUTTONUP => {
            let (x, y) = dip_coords(hwnd, lparam);
            let (row, col) = pixel_to_cell(state, x, y);
            let tab = state.active_mut();
            if tab.terminal.modes.mouse_mode != MouseMode::None {
                let button = if msg == WM_MBUTTONDOWN || msg == WM_MBUTTONUP { 1 } else { 2 };
                let pressed = msg == WM_MBUTTONDOWN;
                send_mouse_event(tab, button, row, col, pressed, false);
            }
            LRESULT(0)
        }
        WM_KEYUP => {
            let vk = VIRTUAL_KEY(wparam.0 as u16);
            if vk == VK_CONTROL || vk == VK_LCONTROL || vk == VK_RCONTROL {
                if state.hovered_url.is_some() {
                    state.hovered_url = None;
                    let _ = InvalidateRect(hwnd, None, false);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        m if m == settings::WM_SETTINGS_LIVE => {
            // Apply config changes in real-time
            let cfg_ptr = lparam.0 as *mut Config;
            if !cfg_ptr.is_null() {
                let new_cfg = *Box::from_raw(cfg_ptr);
                // Apply opacity
                if new_cfg.opacity < 100 {
                    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as isize);
                    let alpha = (new_cfg.opacity as f32 * 2.55) as u8;
                    let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA);
                } else {
                    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex & !(WS_EX_LAYERED.0 as isize));
                }
                // Apply colors
                let fg = Config::parse_color(&new_cfg.fg_color);
                let bg = Config::parse_color(&new_cfg.bg_color);
                state.renderer.bg_rgb = bg;
                state.renderer.fg_rgb = fg;
                // Update config
                state.config = new_cfg;
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_HOTKEY if wparam.0 as i32 == HOTKEY_ID => {
            // Toggle window visibility
            if IsWindowVisible(hwnd).as_bool() {
                ShowWindow(hwnd, SW_HIDE);
            } else {
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
            }
            LRESULT(0)
        }
        m if m == WM_PTY_OUTPUT => {
            // Process output for ALL tabs
            for tab in &mut state.tabs {
                while let Ok(data) = tab.rx.try_recv() {
                    if let Some(ref mut logger) = tab.logger {
                        logger.log(&data);
                    }
                    tab.terminal.process(&data);
                }
                for response in tab.terminal.responses.drain(..) {
                    let _ = tab.pty.write(&response);
                }
            }
            // Update window title from active tab
            state.update_window_title();
            // Render
            let titles = state.tab_titles();
            let url_hover = state.hovered_url.as_ref().map(|(r, s, e, _)| (*r, *s, *e));
            state.renderer.render(&state.active().terminal, &state.active().selection, &titles, url_hover);
            let _ = ValidateRect(hwnd, None);
            LRESULT(0)
        }
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
            if state.dock_height {
                apply_dock_height(hwnd);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

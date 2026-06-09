mod settings;
pub mod shell_integration;
mod ssh_picker;
mod tailscale;
mod window_state;

use crate::config::Config;
use crate::keys::{Action, KeybindingEngine};
use crate::pty::{ConPty, PtyBackend, SshPty, SshProfile};
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
    pty: Box<dyn PtyBackend>,
    rx: mpsc::Receiver<Vec<u8>>,
    selection: Selection,
    high_surrogate: Option<u16>,
    mouse_pressed: bool,
    logger: Option<crate::log::SessionLogger>,
    /// Current IME composition string being typed (shown inline)
    ime_composition: String,
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
    undocked_rect: Option<RECT>,
    menu_items: Vec<MenuItem>,
    /// True between WM_ENTERSIZEMOVE and WM_EXITSIZEMOVE — used to defer
    /// expensive grid/PTY resize while the user is dragging the edge.
    in_sizemove: bool,
    /// Set when WM_SIZE fired during an active sizemove drag; the deferred
    /// grid resize runs in WM_EXITSIZEMOVE.
    pending_resize: bool,
}

#[derive(Clone)]
struct MenuItem {
    label: String,
    fav_key: Option<String>, // None for separators/headers
    action: MenuAction,
}

#[derive(Clone)]
enum MenuAction {
    None,
    NewLocalTab,
    Shell(String),
    SshProfileName(String),
    Tailscale { host: String },
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
            pty: Box::new(pty),
            rx,
            selection: Selection::default(),
            high_surrogate: None,
            mouse_pressed: false,
            logger: None,
            ime_composition: String::new(),
        });
        self.active_tab = self.tabs.len() - 1;
        Ok(())
    }

    fn new_ssh_tab(&mut self, profile: SshProfile) -> windows::core::Result<()> {
        let (cols, rows) = self.renderer.grid_size();
        let mut terminal = Terminal::new(cols, rows);
        terminal.grid.cell_width_hint = self.renderer.cell_width;
        terminal.grid.cell_height_hint = self.renderer.cell_height;
        terminal.title = profile.name.clone();
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let ssh = SshPty::spawn(profile, cols as u16, rows as u16, tx, self.hwnd)
            .map_err(|e| windows::core::Error::from_hresult(
                windows::core::HRESULT::from_win32(e.raw_os_error().unwrap_or(0) as u32),
            ))?;

        self.tabs.push(Tab {
            terminal,
            pty: Box::new(ssh),
            rx,
            selection: Selection::default(),
            high_surrogate: None,
            mouse_pressed: false,
            logger: None,
            ime_composition: String::new(),
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

pub fn run(initial_cwd: Option<String>) -> windows::core::Result<()> {
    let config = Config::load();

    unsafe {
        let instance = GetModuleHandleW(None)?;

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: HINSTANCE(instance.0),
            hCursor: LoadCursorW(None, IDC_IBEAM)?,
            hIcon: LoadIconW(HINSTANCE(instance.0), windows::core::PCWSTR(1 as *const u16)).unwrap_or_default(),
            hIconSm: LoadIconW(HINSTANCE(instance.0), windows::core::PCWSTR(1 as *const u16)).unwrap_or_default(),
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
        let (pty, reader) = ConPty::spawn_with_cwd(
            &config.shell,
            initial_cwd.as_deref(),
            cols as u16,
            rows as u16,
        )?;
        let (tx, rx) = mpsc::channel::<Vec<u8>>();

        let state = Box::new(AppState {
            tabs: vec![Tab {
                terminal,
                pty: Box::new(pty),
                rx,
                selection: Selection::default(),
                high_surrogate: None,
                mouse_pressed: false,
                logger: None,
                ime_composition: String::new(),
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
            menu_items: Vec::new(),
            in_sizemove: false,
            pending_resize: false,
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

        // If there is no saved position for the current monitor, size the window
        // from config.columns/rows. If there is a saved position, restore it
        // (user's remembered size takes priority over columns/rows setting).
        let cursor_monitor = unsafe {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFOEXW {
                monitorInfo: MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
                    ..Default::default()
                },
                ..Default::default()
            };
            if GetMonitorInfoW(mon, &mut info.monitorInfo as *mut _ as *mut MONITORINFO).as_bool() {
                let len = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
                Some(String::from_utf16_lossy(&info.szDevice[..len]))
            } else { None }
        };
        let has_saved = cursor_monitor
            .as_ref()
            .map(|n| config.window_positions.contains_key(n))
            .unwrap_or(false);

        if has_saved {
            window_state::restore_position(hwnd, &config);
        } else {
            resize_window_to_grid(hwnd, &(*state_ptr).renderer, config.columns, config.rows);
            window_state::restore_position(hwnd, &config); // centers on current monitor
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

/// Build escape sequence for a navigation/function/special key with xterm-style
/// modifier encoding. Modifier code = 1 + (Shift?1:0) + (Alt?2:0) + (Ctrl?4:0).
/// When no modifier is held, falls back to the unmodified sequence (CSI or SS3
/// for arrow keys per `app_mode`).
fn build_key_seq(
    vk: VIRTUAL_KEY,
    shift: bool,
    alt: bool,
    ctrl: bool,
    app_mode: bool,
) -> Option<Vec<u8>> {
    let m = 1u8 + (shift as u8) + 2 * (alt as u8) + 4 * (ctrl as u8);
    let has_mod = m > 1;
    // CSI nav key with optional modifier: ESC [ 1 ; m <final>
    let cursor = |final_byte: u8, ss3: u8| -> Vec<u8> {
        if has_mod {
            format!("\x1b[1;{}{}", m, final_byte as char).into_bytes()
        } else if app_mode {
            vec![0x1b, b'O', ss3]
        } else {
            vec![0x1b, b'[', final_byte]
        }
    };
    // CSI tilde key: ESC [ <n> [; m] ~
    let tilde = |n: u8| -> Vec<u8> {
        if has_mod {
            format!("\x1b[{};{}~", n, m).into_bytes()
        } else {
            format!("\x1b[{}~", n).into_bytes()
        }
    };
    // F1..F4: ESC O <P|Q|R|S> unmodified, ESC [ 1 ; m <P|Q|R|S> modified
    let f1_4 = |final_byte: u8| -> Vec<u8> {
        if has_mod {
            format!("\x1b[1;{}{}", m, final_byte as char).into_bytes()
        } else {
            vec![0x1b, b'O', final_byte]
        }
    };
    match vk {
        VK_UP => Some(cursor(b'A', b'A')),
        VK_DOWN => Some(cursor(b'B', b'B')),
        VK_RIGHT => Some(cursor(b'C', b'C')),
        VK_LEFT => Some(cursor(b'D', b'D')),
        VK_HOME => Some(cursor(b'H', b'H')),
        VK_END => Some(cursor(b'F', b'F')),
        VK_INSERT => Some(tilde(2)),
        VK_DELETE => Some(tilde(3)),
        VK_PRIOR => Some(tilde(5)),
        VK_NEXT => Some(tilde(6)),
        VK_F1 => Some(f1_4(b'P')),
        VK_F2 => Some(f1_4(b'Q')),
        VK_F3 => Some(f1_4(b'R')),
        VK_F4 => Some(f1_4(b'S')),
        VK_F5 => Some(tilde(15)),
        VK_F6 => Some(tilde(17)),
        VK_F7 => Some(tilde(18)),
        VK_F8 => Some(tilde(19)),
        VK_F9 => Some(tilde(20)),
        VK_F10 => Some(tilde(21)),
        VK_F11 => Some(tilde(23)),
        VK_F12 => Some(tilde(24)),
        VK_BACK => Some(if alt { b"\x1b\x7f".to_vec() } else { b"\x7f".to_vec() }),
        VK_RETURN => Some(if alt { b"\x1b\r".to_vec() } else { b"\r".to_vec() }),
        VK_TAB => Some(if shift { b"\x1b[Z".to_vec() }
                       else if alt { b"\x1b\t".to_vec() }
                       else { b"\t".to_vec() }),
        VK_ESCAPE => Some(if alt { b"\x1b\x1b".to_vec() } else { b"\x1b".to_vec() }),
        _ => None,
    }
}

/// Whether a VK is expected to produce a WM_CHAR after WM_KEYDOWN.
/// Navigation / function keys do not, so suppressing the next WM_CHAR for them
/// would eat the user's *next* typed character.
fn vk_produces_char(vk: VIRTUAL_KEY) -> bool {
    !matches!(
        vk,
        VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN
        | VK_HOME | VK_END | VK_PRIOR | VK_NEXT
        | VK_INSERT | VK_DELETE
        | VK_F1 | VK_F2 | VK_F3 | VK_F4 | VK_F5 | VK_F6
        | VK_F7 | VK_F8 | VK_F9 | VK_F10 | VK_F11 | VK_F12
        | VK_F13 | VK_F14 | VK_F15 | VK_F16
        | VK_F17 | VK_F18 | VK_F19 | VK_F20 | VK_F21 | VK_F22 | VK_F23 | VK_F24
        | VK_SHIFT | VK_CONTROL | VK_MENU
        | VK_LSHIFT | VK_RSHIFT | VK_LCONTROL | VK_RCONTROL | VK_LMENU | VK_RMENU
        | VK_LWIN | VK_RWIN | VK_APPS
        | VK_CAPITAL | VK_NUMLOCK | VK_SCROLL
        | VK_PAUSE | VK_SNAPSHOT
    )
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

fn position_ime_window(hwnd: HWND, state: &AppState) {
    use windows::Win32::UI::Input::Ime::*;
    unsafe {
        let himc = ImmGetContext(hwnd);
        if himc.0.is_null() {
            return;
        }
        // Cursor position in DIPs
        let tab = &state.tabs[state.active_tab];
        let row = tab.terminal.grid.cursor.row;
        let col = tab.terminal.grid.cursor.col + tab.ime_composition.chars().count();
        let cw = state.renderer.cell_width;
        let ch = state.renderer.cell_height;
        let bar_h = state.renderer.tabbar_height();
        let dpi = GetDpiForWindow(hwnd) as f32;
        let scale = dpi / 96.0;
        let x = (col as f32 * cw * scale) as i32;
        let y = ((bar_h + row as f32 * ch) * scale) as i32;
        let cf = COMPOSITIONFORM {
            dwStyle: CFS_POINT,
            ptCurrentPos: POINT { x, y },
            rcArea: RECT::default(),
        };
        let _ = ImmSetCompositionWindow(himc, &cf);
        // Also set candidate window
        let cf2 = CANDIDATEFORM {
            dwIndex: 0,
            dwStyle: CFS_CANDIDATEPOS,
            ptCurrentPos: POINT { x, y: y + (ch * scale) as i32 },
            rcArea: RECT::default(),
        };
        let _ = ImmSetCandidateWindow(himc, &cf2);
        let _ = ImmReleaseContext(hwnd, himc);
    }
}

fn ime_read_string(
    himc: windows::Win32::UI::Input::Ime::HIMC,
    flag: u32,
) -> Option<String> {
    use windows::Win32::UI::Input::Ime::*;
    unsafe {
        let len = ImmGetCompositionStringW(himc, IME_COMPOSITION_STRING(flag), None, 0);
        if len <= 0 {
            return None;
        }
        let mut buf = vec![0u16; (len as usize) / 2];
        ImmGetCompositionStringW(
            himc,
            IME_COMPOSITION_STRING(flag),
            Some(buf.as_mut_ptr() as *mut _),
            len as u32,
        );
        Some(String::from_utf16_lossy(&buf))
    }
}

fn collect_ssh_profiles(state: &AppState) -> Vec<SshProfile> {
    let mut profiles = crate::pty::ssh_config::load_profiles();
    let existing: std::collections::HashSet<String> = profiles.iter().map(|p| p.name.clone()).collect();
    for p in &state.config.ssh_profiles {
        if !existing.contains(&p.name) {
            profiles.push(p.clone());
        }
    }
    profiles
}

fn detect_shells() -> Vec<(String, String)> {
    // (display_name, command)
    let mut shells = Vec::new();
    shells.push(("PowerShell".into(), "powershell.exe".into()));
    if which("pwsh.exe") {
        shells.push(("PowerShell 7".into(), "pwsh.exe".into()));
    }
    shells.push(("Command Prompt".into(), "cmd.exe".into()));
    if which("wsl.exe") {
        shells.push(("WSL".into(), "wsl.exe".into()));
    }
    let git_bash = r"C:\Program Files\Git\bin\bash.exe";
    if std::path::Path::new(git_bash).exists() {
        shells.push(("Git Bash".into(), git_bash.into()));
    }
    shells
}

fn which(cmd: &str) -> bool {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            if dir.join(cmd).exists() {
                return true;
            }
        }
    }
    false
}

fn build_menu_items(state: &AppState) -> Vec<MenuItem> {
    let mut items = Vec::new();
    // Default new tab
    items.push(MenuItem {
        label: "New Tab".into(),
        fav_key: None,
        action: MenuAction::NewLocalTab,
    });
    // Shells
    for (name, cmd) in detect_shells() {
        items.push(MenuItem {
            label: name,
            fav_key: Some(format!("shell:{}", cmd)),
            action: MenuAction::Shell(cmd),
        });
    }
    // Bookmarks
    for b in &state.config.bookmarks {
        let action = if let Some(shell) = &b.shell {
            MenuAction::Shell(shell.clone())
        } else if let Some(ssh_name) = &b.ssh {
            MenuAction::SshProfileName(ssh_name.clone())
        } else {
            MenuAction::None
        };
        items.push(MenuItem {
            label: b.name.clone(),
            fav_key: Some(format!("bookmark:{}", b.name)),
            action,
        });
    }
    // SSH profiles
    for p in collect_ssh_profiles(state) {
        items.push(MenuItem {
            label: format!("{}  [{}@{}]", p.name, p.user, p.host),
            fav_key: Some(format!("ssh:{}", p.name)),
            action: MenuAction::SshProfileName(p.name),
        });
    }
    // Tailscale
    for peer in tailscale::list_peers() {
        let suffix = if peer.online { "" } else { " (offline)" };
        items.push(MenuItem {
            label: format!("{}  {}{}", peer.host, peer.ip, suffix),
            fav_key: Some(format!("tailscale:{}", peer.host)),
            action: MenuAction::Tailscale { host: peer.host.clone() },
        });
    }
    items
}

fn show_full_menu(state: &mut AppState, hwnd: HWND, dip_x: f32, dip_y: f32) {
    let items = build_menu_items(state);
    state.menu_items = items.clone();

    let favs: Vec<(usize, &MenuItem)> = items.iter().enumerate()
        .filter(|(_, i)| {
            i.fav_key.as_ref()
                .map(|k| state.config.favorites.contains(k))
                .unwrap_or(false)
        }).collect();

    unsafe {
        let menu = match CreatePopupMenu() {
            Ok(m) => m,
            Err(_) => return,
        };

        // Favorites first
        if !favs.is_empty() {
            add_header(menu, "★ Favorites");
            for (i, item) in &favs {
                add_item(menu, *i + 1, &format!("  {}", item.label));
            }
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
        }

        // Iterate items and add with section headers
        let shells = detect_shells();
        let shell_count = shells.len();
        let bookmarks_count = state.config.bookmarks.len();
        let ssh_count = collect_ssh_profiles(state).len();

        // Item 0: "New Tab" (no header before)
        add_item(menu, 1, &items[0].label);

        let mut idx = 1;
        if shell_count > 0 {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
            add_header(menu, "Shells");
            for _ in 0..shell_count {
                add_item(menu, idx + 1, &format!("  {}", items[idx].label));
                idx += 1;
            }
        }
        if bookmarks_count > 0 {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
            add_header(menu, "Bookmarks");
            for _ in 0..bookmarks_count {
                add_item(menu, idx + 1, &format!("  {}", items[idx].label));
                idx += 1;
            }
        }
        if ssh_count > 0 {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
            add_header(menu, "SSH");
            for _ in 0..ssh_count {
                add_item(menu, idx + 1, &format!("  {}", items[idx].label));
                idx += 1;
            }
        }
        // Tailscale (everything remaining)
        let remaining = items.len().saturating_sub(idx);
        if remaining > 0 {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
            add_header(menu, "Tailscale");
            for _ in 0..remaining {
                add_item(menu, idx + 1, &format!("  {}", items[idx].label));
                idx += 1;
            }
        }

        // Show favorites indicator via check mark
        for (i, item) in items.iter().enumerate() {
            if let Some(key) = &item.fav_key {
                if state.config.favorites.contains(key) {
                    let _ = CheckMenuItem(menu, (i + 1) as u32, MF_BYCOMMAND.0 | MF_CHECKED.0);
                }
            }
        }

        // Show menu
        let dpi = GetDpiForWindow(hwnd) as f32;
        let scale = dpi / 96.0;
        let client_x = (dip_x * scale) as i32;
        let client_y = (dip_y * scale) as i32;
        let mut pt = POINT { x: client_x, y: client_y };
        let _ = ClientToScreen(hwnd, &mut pt);

        let selection = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN,
            pt.x, pt.y, 0, hwnd, None,
        );
        let _ = DestroyMenu(menu);

        let cmd = selection.0;
        if cmd > 0 {
            let item_idx = (cmd - 1) as usize;
            if let Some(item) = items.get(item_idx).cloned() {
                execute_menu_action(state, &item.action);
            }
        }
        state.menu_items.clear();
        let _ = InvalidateRect(hwnd, None, false);
    }
}

unsafe fn add_header(menu: HMENU, text: &str) {
    let label = format!("{}\0", text);
    let w: Vec<u16> = label.encode_utf16().collect();
    let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, windows::core::PCWSTR(w.as_ptr()));
}

unsafe fn add_item(menu: HMENU, id: usize, text: &str) {
    let label = format!("{}\0", text);
    let w: Vec<u16> = label.encode_utf16().collect();
    let _ = AppendMenuW(menu, MF_STRING, id, windows::core::PCWSTR(w.as_ptr()));
}

fn execute_menu_action(state: &mut AppState, action: &MenuAction) {
    match action {
        MenuAction::None => {}
        MenuAction::NewLocalTab => { let _ = state.new_tab(); }
        MenuAction::Shell(cmd) => { let _ = spawn_shell_tab(state, cmd); }
        MenuAction::SshProfileName(name) => {
            if let Some(profile) = collect_ssh_profiles(state).iter().find(|p| &p.name == name).cloned() {
                let _ = state.new_ssh_tab(profile);
            }
        }
        MenuAction::Tailscale { host } => {
            // Try matching SSH profile first
            let existing = collect_ssh_profiles(state).iter()
                .find(|p| &p.name == host || &p.host == host)
                .cloned();
            let profile = existing.unwrap_or_else(|| SshProfile {
                name: host.clone(),
                host: host.clone(),
                port: 22,
                user: std::env::var("USERNAME").unwrap_or_else(|_| "root".into()),
                auth: "key".into(),
                password: None,
                key_path: default_key_path(),
            });
            let _ = state.new_ssh_tab(profile);
        }
    }
}

fn default_key_path() -> Option<String> {
    let home = std::env::var("USERPROFILE").ok()?;
    for name in &["id_ed25519", "id_rsa", "id_ecdsa"] {
        let p = std::path::PathBuf::from(&home).join(".ssh").join(name);
        if p.exists() {
            return Some(p.to_string_lossy().into_owned());
        }
    }
    None
}

fn spawn_shell_tab(state: &mut AppState, shell: &str) -> windows::core::Result<()> {
    let (cols, rows) = state.renderer.grid_size();
    let mut terminal = Terminal::new(cols, rows);
    terminal.grid.cell_width_hint = state.renderer.cell_width;
    terminal.grid.cell_height_hint = state.renderer.cell_height;
    let (pty, reader) = ConPty::spawn(shell, cols as u16, rows as u16)?;
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let hwnd_raw = state.hwnd.0 as isize;
    std::thread::spawn(move || {
        reader_thread(reader, tx, HWND(hwnd_raw as *mut _));
    });
    state.tabs.push(Tab {
        terminal,
        pty: Box::new(pty),
        rx,
        selection: Selection::default(),
        high_surrogate: None,
        mouse_pressed: false,
        logger: None,
        ime_composition: String::new(),
    });
    state.active_tab = state.tabs.len() - 1;
    Ok(())
}

/// Apply the renderer's current viewport size to each tab's grid and PTY.
/// Called immediately from WM_SIZE for programmatic/snap resizes, and
/// deferred to WM_EXITSIZEMOVE for interactive drag resizes.
fn apply_grid_resize(state: &mut AppState) {
    let (cols, rows) = state.renderer.grid_size();
    for tab in &mut state.tabs {
        if cols != tab.terminal.grid.cols || rows != tab.terminal.grid.rows {
            tab.terminal.resize(cols, rows);
            let _ = tab.pty.resize(cols as u16, rows as u16);
        }
    }
}

fn resize_window_to_grid(hwnd: HWND, renderer: &Renderer, cols: u32, rows: u32) {
    unsafe {
        let dpi = GetDpiForWindow(hwnd) as f32;
        let scale = dpi / 96.0;
        // Calculate client area in physical pixels
        let client_w = (cols as f32 * renderer.cell_width * scale) as i32;
        let tabbar_h_dips = renderer.tabbar_height();
        let client_h = ((rows as f32 * renderer.cell_height + tabbar_h_dips) * scale) as i32;

        // Adjust for non-client area (title bar, borders)
        let mut rect = RECT { left: 0, top: 0, right: client_w, bottom: client_h };
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let _ = AdjustWindowRectExForDpi(
            &mut rect,
            WINDOW_STYLE(style),
            false,
            WINDOW_EX_STYLE(ex_style),
            dpi as u32,
        );
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;

        let _ = SetWindowPos(
            hwnd, None, 0, 0, width, height,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOMOVE,
        );
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
        Action::MoveTabLeft => {
            if state.active_tab > 0 {
                let i = state.active_tab;
                state.tabs.swap(i, i - 1);
                state.active_tab = i - 1;
            }
        }
        Action::MoveTabRight => {
            if state.active_tab + 1 < state.tabs.len() {
                let i = state.active_tab;
                state.tabs.swap(i, i + 1);
                state.active_tab = i + 1;
            }
        }
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
        Action::OpenMenu => {
            let (plus_x, _, _) = state.renderer.tabbar_buttons();
            let y = state.renderer.tabbar_height();
            show_full_menu(state, hwnd, plus_x, y);
        }
        Action::SshPicker => {
            // Merge profiles from ~/.ssh/config and TOML config
            let mut profiles = crate::pty::ssh_config::load_profiles();
            let existing_names: std::collections::HashSet<String> =
                profiles.iter().map(|p| p.name.clone()).collect();
            for p in &state.config.ssh_profiles {
                if !existing_names.contains(&p.name) {
                    profiles.push(p.clone());
                }
            }
            if let Some(profile) = ssh_picker::pick_profile(hwnd, &profiles) {
                if let Err(e) = state.new_ssh_tab(profile) {
                    eprintln!("SSH tab creation failed: {}", e);
                }
            }
        }
        Action::TestSixel => {
            // Load test_image.png and feed sixel data directly to terminal (bypass ConPTY)
            let img_path = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join("test_image.png")))
                .unwrap_or_else(|| std::path::PathBuf::from("test_image.png"));
            // Try current dir too
            let img_path = if img_path.exists() { img_path } else { std::path::PathBuf::from("test_image.png") };
            if let Ok(img_data) = std::fs::read(&img_path) {
                if let Ok(img) = image::load_from_memory(&img_data) {
                    let rgba = img.to_rgba8();
                    let w = rgba.width();
                    let h = rgba.height();
                    let tab = state.active_mut();
                    let row = tab.terminal.grid.cursor.row;
                    let col = tab.terminal.grid.cursor.col;
                    let cw = tab.terminal.grid.cell_width_hint;
                    let ch = tab.terminal.grid.cell_height_hint;
                    let cell_cols = if cw > 0.0 { ((w as f32) / cw).ceil() as usize } else { 1 };
                    let cell_rows = if ch > 0.0 { ((h as f32) / ch).ceil() as usize } else { 1 };
                    let abs_row = tab.terminal.grid.scrollback_len() + row;
                    tab.terminal.images.push(crate::image::TerminalImage {
                        data: rgba.into_raw(),
                        width: w,
                        height: h,
                        row: abs_row,
                        col,
                        cell_cols,
                        cell_rows,
                    });
                    // Move cursor past the image
                    for _ in 0..cell_rows {
                        tab.terminal.grid.cursor.row += 1;
                        if tab.terminal.grid.cursor.row >= tab.terminal.grid.rows {
                            tab.terminal.grid.scroll_up();
                            tab.terminal.grid.cursor.row = tab.terminal.grid.rows - 1;
                        }
                    }
                    tab.terminal.grid.cursor.col = 0;
                }
            }
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
            let t = crate::perf::FrameTimer::new();
            let titles = state.tab_titles();
            let url_hover = state.hovered_url.as_ref().map(|(r, s, e, _)| (*r, *s, *e));
            let tab = &state.tabs[state.active_tab];
            state.renderer.render(&tab.terminal, &tab.selection, &titles, url_hover, &tab.ime_composition);
            let _ = ValidateRect(hwnd, None);
            t.finish("paint");
            LRESULT(0)
        }
        WM_SIZE => {
            let width = (lparam.0 & 0xFFFF) as u32;
            let height = ((lparam.0 >> 16) & 0xFFFF) as u32;
            if width > 0 && height > 0 {
                // Always resize the D2D target so the viewport tracks the
                // window during an interactive drag — keeps the existing
                // grid drawn crisp instead of stretched by Windows.
                let _ = state.renderer.resize(width, height);
                if state.in_sizemove {
                    // Defer the expensive grid + PTY resize until the drag
                    // ends. While dragging, render continues with the old
                    // grid dimensions; the new edges show background color.
                    state.pending_resize = true;
                } else {
                    apply_grid_resize(state);
                }
            }
            LRESULT(0)
        }
        WM_ENTERSIZEMOVE => {
            state.in_sizemove = true;
            LRESULT(0)
        }
        WM_MENURBUTTONUP => {
            // Right-click on menu item: toggle favorite status and close menu
            let pos = wparam.0 as i32;
            let menu_handle = HMENU(lparam.0 as *mut _);
            let cmd_id = GetMenuItemID(menu_handle, pos);
            if cmd_id > 0 {
                let item_idx = (cmd_id - 1) as usize;
                if let Some(item) = state.menu_items.get(item_idx).cloned() {
                    if let Some(key) = item.fav_key {
                        if let Some(p) = state.config.favorites.iter().position(|f| f == &key) {
                            state.config.favorites.remove(p);
                        } else {
                            state.config.favorites.push(key);
                        }
                        let _ = state.config.save();
                    }
                }
                let _ = EndMenu();
            }
            LRESULT(0)
        }
        WM_IME_STARTCOMPOSITION => {
            // Set composition window position to cursor, so the candidate
            // popup appears next to where the user is typing.
            position_ime_window(hwnd, state);
            LRESULT(0) // suppress default floating composition box
        }
        WM_IME_COMPOSITION => {
            use windows::Win32::UI::Input::Ime::*;
            let flags = lparam.0 as u32;
            let himc = ImmGetContext(hwnd);
            if !himc.0.is_null() {
                if flags & GCS_RESULTSTR.0 != 0 {
                    // Final committed string — send to PTY
                    if let Some(s) = ime_read_string(himc, GCS_RESULTSTR.0) {
                        let tab = state.active_mut();
                        tab.ime_composition.clear();
                        tab.terminal.grid.scroll_viewport_to_bottom();
                        let _ = tab.pty.write(s.as_bytes());
                    }
                }
                if flags & GCS_COMPSTR.0 != 0 {
                    // Composition in progress — store for inline rendering
                    let tab = state.active_mut();
                    tab.ime_composition = ime_read_string(himc, GCS_COMPSTR.0).unwrap_or_default();
                    let _ = InvalidateRect(hwnd, None, false);
                }
                let _ = ImmReleaseContext(hwnd, himc);
            }
            position_ime_window(hwnd, state);
            LRESULT(0)
        }
        WM_IME_ENDCOMPOSITION => {
            let tab = state.active_mut();
            if !tab.ime_composition.is_empty() {
                tab.ime_composition.clear();
                let _ = InvalidateRect(hwnd, None, false);
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
        WM_SYSKEYDOWN => {
            // Alt+key combos go through WM_SYSKEYDOWN. Run keybinding match.
            let vk = VIRTUAL_KEY(wparam.0 as u16);
            // Reset stale suppress flag from a prior keydown that produced no WM_CHAR.
            state.suppress_char = false;
            if let Some(action) = state.keys.match_key(vk) {
                handle_action(state, action, hwnd);
                if vk_produces_char(vk) {
                    state.suppress_char = true;
                }
                return LRESULT(0);
            }
            // Preserve Alt+F4 = system close.
            if vk == VK_F4 {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            // Alt+nav/function/special: emit xterm-style modifier sequence.
            let shift = get_key_state(VK_SHIFT);
            let ctrl = get_key_state(VK_CONTROL);
            let tab = state.active_mut();
            let app_mode = tab.terminal.modes.cursor_keys_application;
            if let Some(seq) = build_key_seq(vk, shift, true, ctrl, app_mode) {
                tab.terminal.grid.scroll_viewport_to_bottom();
                let _ = tab.pty.write(&seq);
                if vk_produces_char(vk) {
                    state.suppress_char = true;
                }
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_SYSCHAR => {
            // Suppress default ding for Alt+letter combos that we handled
            if state.suppress_char {
                state.suppress_char = false;
                return LRESULT(0);
            }
            // Alt+printable: send ESC + char (M-x style) to PTY.
            let ch = wparam.0 as u32;
            if let Some(c) = char::from_u32(ch) {
                if !c.is_control() {
                    let tab = state.active_mut();
                    tab.terminal.grid.scroll_viewport_to_bottom();
                    tab.selection.clear();
                    let mut buf = [0u8; 5];
                    buf[0] = 0x1b;
                    let s = c.encode_utf8(&mut buf[1..]);
                    let total = 1 + s.len();
                    let _ = tab.pty.write(&buf[..total]);
                    return LRESULT(0);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_KEYDOWN => {
            let vk = VIRTUAL_KEY(wparam.0 as u16);

            // Reset stale suppress flag from a prior keydown that produced no WM_CHAR
            // (e.g. arrow / function / nav keys). Without this, the next typed char
            // gets eaten because suppress_char never got consumed.
            state.suppress_char = false;

            // Check keybindings first
            if let Some(action) = state.keys.match_key(vk) {
                handle_action(state, action, hwnd);
                if vk_produces_char(vk) {
                    state.suppress_char = true; // prevent WM_CHAR from sending control char
                }
                return LRESULT(0);
            }

            let shift = get_key_state(VK_SHIFT);
            let ctrl = get_key_state(VK_CONTROL);
            let tab = state.active_mut();
            let app_mode = tab.terminal.modes.cursor_keys_application;
            if let Some(seq) = build_key_seq(vk, shift, false, ctrl, app_mode) {
                tab.terminal.grid.scroll_viewport_to_bottom();
                let _ = tab.pty.write(&seq);
                if vk_produces_char(vk) {
                    state.suppress_char = true; // prevent duplicate from WM_CHAR
                }
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = dip_coords(hwnd, lparam);
            let tabbar_h = state.renderer.tabbar_height();

            if y < tabbar_h {
                let (plus_x, _dropdown_x, gear_x) = state.renderer.tabbar_buttons();
                if x >= gear_x {
                    handle_action(state, Action::OpenConfig, hwnd);
                } else if x >= plus_x {
                    // + button → show full menu (shells / bookmarks / ssh / tailscale)
                    show_full_menu(state, hwnd, x, tabbar_h);
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

                // Apply columns/rows by resizing the window
                if new_cfg.columns != state.config.columns || new_cfg.rows != state.config.rows {
                    resize_window_to_grid(hwnd, &state.renderer, new_cfg.columns, new_cfg.rows);
                }

                // Update config
                state.config = new_cfg;
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_HOTKEY if wparam.0 as i32 == HOTKEY_ID => {
            if IsWindowVisible(hwnd).as_bool() {
                // Save position before hiding
                window_state::save_position(hwnd, &mut state.config, state.dock_height);
                let _ = ShowWindow(hwnd, SW_HIDE);
            } else {
                // Restore position for the monitor under cursor
                window_state::restore_position(hwnd, &state.config);
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_EXITSIZEMOVE => {
            state.in_sizemove = false;
            if state.pending_resize {
                state.pending_resize = false;
                apply_grid_resize(state);
                let _ = InvalidateRect(hwnd, None, false);
            }
            // User finished moving/resizing the window — save position
            window_state::save_position(hwnd, &mut state.config, state.dock_height);
            LRESULT(0)
        }
        m if m == WM_PTY_OUTPUT => {
            // The reader thread posts WM_PTY_OUTPUT once per 4KB chunk, so a
            // full-screen TUI redraw arrives as many small messages in quick
            // succession. We drain data inline (cheap) but defer painting to
            // WM_PAINT, which Windows coalesces — multiple invalidates between
            // paints collapse into a single render.
            let t = crate::perf::FrameTimer::new();
            let mut had_data = false;
            for tab in &mut state.tabs {
                while let Ok(data) = tab.rx.try_recv() {
                    had_data = true;
                    if let Some(ref mut logger) = tab.logger {
                        logger.log(&data);
                    }
                    tab.terminal.process(&data);
                }
                for response in tab.terminal.responses.drain(..) {
                    let _ = tab.pty.write(&response);
                }
            }
            if had_data {
                state.update_window_title();
                let _ = InvalidateRect(hwnd, None, false);
            }
            t.finish("pty");
            LRESULT(0)
        }
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
            if state.dock_height {
                apply_dock_height(hwnd);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_DESTROY => {
            // Save position before closing
            window_state::save_position(hwnd, &mut state.config, state.dock_height);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

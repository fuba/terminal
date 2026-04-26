use crate::config::Config;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::UI::Controls::Dialogs::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const ID_SHELL: i32 = 101;
const ID_FONT: i32 = 102;
const ID_FONTSIZE: i32 = 103;
const ID_COLS: i32 = 104;
const ID_ROWS: i32 = 105;
const ID_OPACITY: i32 = 106;
const ID_FG_BTN: i32 = 107;
const ID_BG_BTN: i32 = 108;
const ID_SCROLLBACK: i32 = 109;
const ID_HK_MOD: i32 = 110;
const ID_HK_KEY: i32 = 111;
const ID_HK_ENABLED: i32 = 112;
const ID_SAVE: i32 = 200;
const ID_CANCEL: i32 = 201;
const ID_HELP: i32 = 202;
// Real-time change notifications
const ID_APPLY_LIVE: i32 = 300;

/// Message sent to parent to apply live changes
pub const WM_SETTINGS_LIVE: u32 = WM_USER + 100;

fn center_on_parent(parent: HWND, w: i32, h: i32) -> (i32, i32) {
    unsafe {
        let mut r = RECT::default();
        if GetWindowRect(parent, &mut r).is_ok() {
            let pw = r.right - r.left;
            let ph = r.bottom - r.top;
            let x = r.left + (pw - w) / 2;
            let y = r.top + (ph - h) / 2;
            return (x.max(0), y.max(0));
        }
    }
    (CW_USEDEFAULT, CW_USEDEFAULT)
}

struct SettingsState {
    config: Config,
    saved: bool,
    parent: HWND,
    fg_color: COLORREF,
    bg_color: COLORREF,
    custom_colors: [COLORREF; 16],
}

pub fn show_settings(parent: HWND, config: &Config) -> Option<Config> {
    unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(settings_proc),
            hInstance: HINSTANCE(instance.0),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(6 as *mut _),
            lpszClassName: w!("TerminalSettings"),
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let fg = parse_hex_to_colorref(&config.fg_color);
        let bg = parse_hex_to_colorref(&config.bg_color);

        let state = Box::new(SettingsState {
            config: config.clone(),
            saved: false,
            parent,
            fg_color: fg,
            bg_color: bg,
            custom_colors: [COLORREF(0); 16],
        });
        let state_ptr = Box::into_raw(state);

        let (x, y) = center_on_parent(parent, 460, 580);
        let hwnd = CreateWindowExW(
            WS_EX_DLGMODALFRAME | WS_EX_TOPMOST,
            w!("TerminalSettings"), w!("Settings"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            x, y, 460, 580,
            parent, None, HINSTANCE(instance.0),
            Some(state_ptr as *const _),
        );

        if hwnd.is_err() {
            let _ = Box::from_raw(state_ptr);
            return None;
        }
        let hwnd = hwnd.unwrap();
        EnableWindow(parent, false);
        create_controls(hwnd, HINSTANCE(instance.0), config);

        let mut msg = MSG::default();
        loop {
            let ret = PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE);
            if !IsWindow(hwnd).as_bool() { break; }
            if ret.as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            } else {
                let _ = WaitMessage();
            }
        }

        EnableWindow(parent, true);
        let _ = SetForegroundWindow(parent);

        let state = Box::from_raw(state_ptr);
        if state.saved { Some(state.config) } else { None }
    }
}

unsafe fn create_controls(hwnd: HWND, inst: HINSTANCE, cfg: &Config) {
    let xl = 20i32;
    let xi = 160i32;
    let wi = 250i32;
    let h = 24i32;
    let mut y = 16i32;
    let g = 32i32;

    // Shell - ComboBox
    label(hwnd, inst, "Shell:", xl, y, 130, h);
    let cb = combobox(hwnd, inst, xi, y, wi, 200, ID_SHELL);
    let shells = ["powershell.exe", "pwsh.exe", "cmd.exe", "wsl.exe",
                  "bash.exe", "C:\\Program Files\\Git\\bin\\bash.exe"];
    for s in &shells {
        add_combo_item(cb, s);
    }
    set_combo_text(cb, &cfg.shell);
    y += g;

    // Font - ComboBox
    label(hwnd, inst, "Font:", xl, y, 130, h);
    let fb = combobox(hwnd, inst, xi, y, wi, 300, ID_FONT);
    let fonts = ["Consolas", "Cascadia Mono", "Cascadia Code",
                 "Courier New", "Lucida Console", "MS Gothic",
                 "BIZ UDGothic", "HackGen Console", "JetBrains Mono",
                 "Fira Code", "Source Code Pro", "IBM Plex Mono"];
    for f in &fonts {
        add_combo_item(fb, f);
    }
    set_combo_text(fb, &cfg.font_family);
    y += g;

    // Font size
    label(hwnd, inst, "Font Size:", xl, y, 130, h);
    edit(hwnd, inst, &cfg.font_size.to_string(), xi, y, 60, h, ID_FONTSIZE);
    y += g;

    // Columns
    label(hwnd, inst, "Columns:", xl, y, 130, h);
    edit(hwnd, inst, &cfg.columns.to_string(), xi, y, 60, h, ID_COLS);
    y += g;

    // Rows
    label(hwnd, inst, "Rows:", xl, y, 130, h);
    edit(hwnd, inst, &cfg.rows.to_string(), xi, y, 60, h, ID_ROWS);
    y += g;

    // Opacity
    label(hwnd, inst, "Opacity (0-100):", xl, y, 130, h);
    edit(hwnd, inst, &cfg.opacity.to_string(), xi, y, 60, h, ID_OPACITY);
    y += g;

    // FG Color button
    label(hwnd, inst, "FG Color:", xl, y, 130, h);
    button(hwnd, inst, &cfg.fg_color, xi, y, 120, h, ID_FG_BTN);
    y += g;

    // BG Color button
    label(hwnd, inst, "BG Color:", xl, y, 130, h);
    button(hwnd, inst, &cfg.bg_color, xi, y, 120, h, ID_BG_BTN);
    y += g;

    // Scrollback
    label(hwnd, inst, "Scrollback:", xl, y, 130, h);
    edit(hwnd, inst, &cfg.scrollback_limit.to_string(), xi, y, 80, h, ID_SCROLLBACK);
    y += g;

    // Hotkey
    label(hwnd, inst, "Hotkey Modifiers:", xl, y, 130, h);
    edit(hwnd, inst, &cfg.hotkey.modifiers, xi, y, 140, h, ID_HK_MOD);
    y += g;

    label(hwnd, inst, "Hotkey Key:", xl, y, 130, h);
    edit(hwnd, inst, &cfg.hotkey.key, xi, y, 80, h, ID_HK_KEY);
    y += g;

    checkbox(hwnd, inst, "Hotkey Enabled", xi, y, 140, h, ID_HK_ENABLED, cfg.hotkey.enabled);
    y += g;

    // Apply Live button
    button(hwnd, inst, "Apply Live", xi, y, 100, 28, ID_APPLY_LIVE);
    y += g + 8;

    // Save / Cancel / Help
    button(hwnd, inst, "Keys…", 20, y, 90, 32, ID_HELP);
    button(hwnd, inst, "Save", 240, y, 90, 32, ID_SAVE);
    button(hwnd, inst, "Cancel", 340, y, 90, 32, ID_CANCEL);
}

fn show_keybindings_help(parent: HWND) {
    let text = "\
Tabs\n\
  Ctrl+T              New tab (default shell)\n\
  Ctrl+W              Close tab\n\
  Ctrl+\u{2192} / Ctrl+\u{2190}        Next / previous tab\n\
  Ctrl+Shift+\u{2192}/\u{2190}      Move tab right / left\n\
  Ctrl+1 \u{2026} Ctrl+9       Select tab 1\u{2013}9\n\
  Alt+O               Open new-tab menu (\u{2191}\u{2193}/Enter to pick)\n\
\n\
Clipboard\n\
  Ctrl+C              Copy selection (or send ^C)\n\
  Ctrl+V              Paste\n\
  Ctrl+Click on URL   Open URL in browser\n\
\n\
Scrollback\n\
  Mouse wheel         Scroll history\n\
  Ctrl+\u{2191} / Ctrl+\u{2193}        Page up / down\n\
  Ctrl+Home / Ctrl+End  Jump to top / bottom\n\
\n\
Window\n\
  Alt+Shift+V         Toggle visibility (global hotkey, configurable)\n\
  F11                 Dock to full screen height\n\
  Ctrl+,              Open Settings\n\
  Ctrl+L              Toggle session log\n\
\n\
Tab bar buttons\n\
  [+ \u{25BE}]                Open new-tab menu\n\
  [\u{2699}]                  Settings\n\
  [\u{00D7}] on a tab         Close that tab\n\
  Right-click menu item   Toggle favorite (\u{2605} pinned to top)\n\
\0";
    let wide: Vec<u16> = text.encode_utf16().collect();
    let title: Vec<u16> = "Keybindings\0".encode_utf16().collect();
    unsafe {
        MessageBoxW(
            parent,
            windows::core::PCWSTR(wide.as_ptr()),
            windows::core::PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

unsafe fn label(hwnd: HWND, inst: HINSTANCE, text: &str, x: i32, y: i32, w: i32, h: i32) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let _ = CreateWindowExW(
        WINDOW_EX_STYLE::default(), w!("STATIC"),
        windows::core::PCWSTR(wide.as_ptr()),
        WS_CHILD | WS_VISIBLE, x, y, w, h, hwnd, None, inst, None,
    );
}

unsafe fn edit(hwnd: HWND, inst: HINSTANCE, text: &str, x: i32, y: i32, w: i32, h: i32, id: i32) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let _ = CreateWindowExW(
        WS_EX_CLIENTEDGE, w!("EDIT"),
        windows::core::PCWSTR(wide.as_ptr()),
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(0x0080),
        x, y, w, h, hwnd, HMENU(id as *mut _), inst, None,
    );
}

unsafe fn button(hwnd: HWND, inst: HINSTANCE, text: &str, x: i32, y: i32, w: i32, h: i32, id: i32) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let _ = CreateWindowExW(
        WINDOW_EX_STYLE::default(), w!("BUTTON"),
        windows::core::PCWSTR(wide.as_ptr()),
        WS_CHILD | WS_VISIBLE, x, y, w, h, hwnd,
        HMENU(id as *mut _), inst, None,
    );
}

unsafe fn combobox(hwnd: HWND, inst: HINSTANCE, x: i32, y: i32, w: i32, drop_h: i32, id: i32) -> HWND {
    // CBS_DROPDOWN = 0x0002, CBS_AUTOHSCROLL = 0x0040
    CreateWindowExW(
        WS_EX_CLIENTEDGE, w!("COMBOBOX"), w!(""),
        WS_CHILD | WS_VISIBLE | WS_VSCROLL | WINDOW_STYLE(0x0042),
        x, y, w, drop_h, hwnd, HMENU(id as *mut _), inst, None,
    ).unwrap_or(HWND::default())
}

unsafe fn add_combo_item(cb: HWND, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    SendMessageW(cb, 0x0143, WPARAM(0), LPARAM(wide.as_ptr() as isize)); // CB_ADDSTRING
}

unsafe fn set_combo_text(cb: HWND, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    SetWindowTextW(cb, windows::core::PCWSTR(wide.as_ptr()));
}

unsafe fn checkbox(hwnd: HWND, inst: HINSTANCE, text: &str, x: i32, y: i32, w: i32, h: i32, id: i32, checked: bool) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    if let Ok(cb) = CreateWindowExW(
        WINDOW_EX_STYLE::default(), w!("BUTTON"),
        windows::core::PCWSTR(wide.as_ptr()),
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(0x0003),
        x, y, w, h, hwnd, HMENU(id as *mut _), inst, None,
    ) {
        if checked { SendMessageW(cb, 0x00F1, WPARAM(1), LPARAM(0)); }
    }
}

fn get_text(hwnd: HWND, id: i32) -> String {
    unsafe {
        if let Ok(ctrl) = GetDlgItem(hwnd, id) {
            let len = GetWindowTextLengthW(ctrl) as usize;
            if len == 0 { return String::new(); }
            let mut buf = vec![0u16; len + 1];
            GetWindowTextW(ctrl, &mut buf);
            String::from_utf16_lossy(&buf[..len])
        } else { String::new() }
    }
}

fn get_check(hwnd: HWND, id: i32) -> bool {
    unsafe {
        if let Ok(ctrl) = GetDlgItem(hwnd, id) {
            SendMessageW(ctrl, 0x00F0, WPARAM(0), LPARAM(0)).0 == 1
        } else { false }
    }
}

fn read_config_from_controls(hwnd: HWND, state: &SettingsState) -> Config {
    Config {
        shell: get_text(hwnd, ID_SHELL),
        font_family: get_text(hwnd, ID_FONT),
        font_size: get_text(hwnd, ID_FONTSIZE).parse().unwrap_or(16.0),
        columns: get_text(hwnd, ID_COLS).parse().unwrap_or(120),
        rows: get_text(hwnd, ID_ROWS).parse().unwrap_or(30),
        opacity: get_text(hwnd, ID_OPACITY).parse::<u8>().unwrap_or(100).min(100),
        fg_color: colorref_to_hex(state.fg_color),
        bg_color: colorref_to_hex(state.bg_color),
        scrollback_limit: get_text(hwnd, ID_SCROLLBACK).parse().unwrap_or(10000),
        hotkey: crate::config::HotkeyConfig {
            modifiers: get_text(hwnd, ID_HK_MOD),
            key: get_text(hwnd, ID_HK_KEY),
            enabled: get_check(hwnd, ID_HK_ENABLED),
        },
        ssh_profiles: state.config.ssh_profiles.clone(),
        bookmarks: state.config.bookmarks.clone(),
        favorites: state.config.favorites.clone(),
        window_positions: state.config.window_positions.clone(),
    }
}

fn choose_color(hwnd: HWND, current: COLORREF, custom: &mut [COLORREF; 16]) -> Option<COLORREF> {
    unsafe {
        let mut cc = CHOOSECOLORW {
            lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
            hwndOwner: hwnd,
            rgbResult: current,
            lpCustColors: custom.as_mut_ptr(),
            Flags: CC_FULLOPEN | CC_RGBINIT,
            ..std::mem::zeroed()
        };
        if ChooseColorW(&mut cc).as_bool() {
            Some(cc.rgbResult)
        } else {
            None
        }
    }
}

fn parse_hex_to_colorref(hex: &str) -> COLORREF {
    let (r, g, b) = Config::parse_color(hex);
    COLORREF(r as u32 | ((g as u32) << 8) | ((b as u32) << 16))
}

fn colorref_to_hex(c: COLORREF) -> String {
    let r = c.0 & 0xFF;
    let g = (c.0 >> 8) & 0xFF;
    let b = (c.0 >> 16) & 0xFF;
    format!("#{:02X}{:02X}{:02X}", r, g, b)
}

fn update_color_button(hwnd: HWND, id: i32, color: COLORREF) {
    let hex = colorref_to_hex(color);
    let wide: Vec<u16> = hex.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if let Ok(btn) = GetDlgItem(hwnd, id) {
            SetWindowTextW(btn, windows::core::PCWSTR(wide.as_ptr()));
        }
    }
}

unsafe extern "system" fn settings_proc(
    hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let sp = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SettingsState;
            if sp.is_null() { return DefWindowProcW(hwnd, msg, wparam, lparam); }
            let state = &mut *sp;

            match id {
                ID_FG_BTN => {
                    if let Some(c) = choose_color(hwnd, state.fg_color, &mut state.custom_colors) {
                        state.fg_color = c;
                        update_color_button(hwnd, ID_FG_BTN, c);
                    }
                }
                ID_BG_BTN => {
                    if let Some(c) = choose_color(hwnd, state.bg_color, &mut state.custom_colors) {
                        state.bg_color = c;
                        update_color_button(hwnd, ID_BG_BTN, c);
                    }
                }
                ID_APPLY_LIVE => {
                    state.config = read_config_from_controls(hwnd, state);
                    // Send live config to parent
                    let cfg_box = Box::new(state.config.clone());
                    let ptr = Box::into_raw(cfg_box);
                    let _ = PostMessageW(state.parent, WM_SETTINGS_LIVE, WPARAM(0), LPARAM(ptr as isize));
                }
                ID_SAVE => {
                    state.config = read_config_from_controls(hwnd, state);
                    if let Err(e) = state.config.save() {
                        let msg = format!("Save failed: {}\0", e);
                        let w: Vec<u16> = msg.encode_utf16().collect();
                        MessageBoxW(hwnd, windows::core::PCWSTR(w.as_ptr()), w!("Error"), MB_OK | MB_ICONERROR);
                    }
                    // Apply live too
                    let cfg_box = Box::new(state.config.clone());
                    let ptr = Box::into_raw(cfg_box);
                    let _ = PostMessageW(state.parent, WM_SETTINGS_LIVE, WPARAM(0), LPARAM(ptr as isize));
                    state.saved = true;
                    DestroyWindow(hwnd);
                }
                ID_CANCEL => { let _ = DestroyWindow(hwnd); }
                ID_HELP => { show_keybindings_help(hwnd); }
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => { let _ = DestroyWindow(hwnd); LRESULT(0) }
        WM_DESTROY => {
            // Do NOT call PostQuitMessage - that would kill the main app.
            // The settings message loop breaks via IsWindow check.
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

//! A separate Win32 UI thread owns the transfer and its short-lived secrets.
use crate::{pty::SshProfile, tkqr};
use qrcode::{Color, EcLevel, QrCode};
use std::{
    cell::{Cell, RefCell},
    io::Read,
    time::{Duration, Instant},
};
use windows::{
    core::{w, PCWSTR, PWSTR},
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{LibraryLoader::*, RemoteDesktop::*},
        UI::{Controls::Dialogs::*, HiDpi::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    },
};
use zeroize::Zeroizing;

const CANCEL: i32 = 2;
const CANDIDATE: i32 = 100;
const NAME: i32 = 101;
const BROWSE: i32 = 102;
const GENERATE: i32 = 103;
const TOGGLE: i32 = 104;
const PAUSE: i32 = 105;
const PREV: i32 = 106;
const NEXT: i32 = 107;
const STATUS: i32 = 108;
const SECRET: i32 = 109;

struct WindowState {
    state: RefCell<State>,
    invalidated: Cell<bool>,
}

struct State {
    paths: Vec<String>,
    transfer: Option<tkqr::Transfer>,
    codes: Vec<QrCode>,
    created: Instant,
    advanced: Instant,
    index: usize,
    passphrase: bool,
    paused: bool,
    font: HFONT,
    displayed_second: u64,
}

pub(super) fn show(profiles: Vec<SshProfile>) {
    let mut paths = Vec::new();
    for p in profiles {
        if let Some(path) = p.key_path {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    for path in crate::pty::ssh_config::identity_files() {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    // No application-state pointers or secrets cross the thread boundary.
    let _ = std::thread::Builder::new()
        .name("key-transfer-ui".into())
        .spawn(move || {
            // SAFETY: all HWNDs, state pointers and GDI objects below belong to this
            // thread; the boxed state outlives the window and message loop.
            unsafe {
                run(paths);
            }
        });
}

unsafe fn run(paths: Vec<String>) {
    let Ok(module) = GetModuleHandleW(None) else {
        return;
    };
    let instance = HINSTANCE(module.0);
    let class = w!("TerminalKeyTransfer");
    RegisterClassW(&WNDCLASSW {
        lpfnWndProc: Some(proc),
        hInstance: instance,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as *mut _),
        lpszClassName: class,
        ..Default::default()
    });
    let mut state = Box::new(WindowState {
        state: RefCell::new(State {
            paths,
            transfer: None,
            codes: Vec::new(),
            created: Instant::now(),
            advanced: Instant::now(),
            index: 0,
            passphrase: false,
            paused: false,
            font: HFONT::default(),
            displayed_second: 0,
        }),
        invalidated: Cell::new(false),
    });
    let dpi = GetDpiForSystem() as i32;
    let width = (780 * dpi / 96).min(GetSystemMetrics(SM_CXSCREEN));
    let height = (930 * dpi / 96).min(GetSystemMetrics(SM_CYSCREEN) - 60);
    let Ok(hwnd) = CreateWindowExW(
        WS_EX_CONTROLPARENT,
        class,
        w!("秘密鍵をiPhoneへ転送 — TKQR1"),
        WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        width,
        height,
        None,
        None,
        instance,
        Some((&mut *state as *mut WindowState).cast()),
    ) else {
        return;
    };
    for (id, class, label, style) in [
        (200, w!("STATIC"), "秘密鍵ファイル (&K):", WINDOW_STYLE(0)),
        (CANDIDATE, w!("COMBOBOX"), "", WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32)),
        (BROWSE, w!("BUTTON"), "ファイルを選択 (&B)…", WS_TABSTOP),
        (201, w!("STATIC"), "表示名 (&N):", WINDOW_STYLE(0)),
        (NAME, w!("EDIT"), "SSH key", WS_TABSTOP | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32)),
        (GENERATE, w!("BUTTON"), "生成 / 再生成 (&G)", WS_TABSTOP),
        (202, w!("STATIC"), "秘密鍵を暗号化して転送します。元の鍵のパスフレーズとは別の24文字をiPhoneへ入力してください。", WINDOW_STYLE(0)),
        (STATUS, w!("STATIC"), "鍵を選び、表示名を入力して「生成」を押してください。", WINDOW_STYLE(0)),
        (SECRET, w!("STATIC"), "", WINDOW_STYLE(1)),
        (TOGGLE, w!("BUTTON"), "パスフレーズ / QR (&T)", WS_TABSTOP),
        (PAUSE, w!("BUTTON"), "一時停止 / 再開 (&P)", WS_TABSTOP),
        (PREV, w!("BUTTON"), "前へ (&L)", WS_TABSTOP),
        (NEXT, w!("BUTTON"), "次へ (&R)", WS_TABSTOP),
        (CANCEL, w!("BUTTON"), "閉じる", WS_TABSTOP),
    ] {
        let label = wide(label);
        let _ = CreateWindowExW(WINDOW_EX_STYLE::default(), class, PCWSTR(label.as_ptr()), WS_CHILD | WS_VISIBLE | style,
            0,0,0,0,hwnd, HMENU(id as *mut _),instance,None);
    }
    SendDlgItemMessageW(hwnd, NAME, 0x00c5, WPARAM(128), LPARAM(0));
    if let Ok(secret) = GetDlgItem(hwnd, SECRET) {
        let _ = ShowWindow(secret, SW_HIDE);
    }
    refresh_paths(hwnd, &state.state.borrow());
    set_font(hwnd, &mut state.state.borrow_mut());
    layout(hwnd);
    if WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION).is_err() {
        error(
            hwnd,
            "OSロック通知を登録できないため、転送を開始できません。",
        );
        let _ = DestroyWindow(hwnd);
        return;
    }
    if SetTimer(hwnd, 1, 50, None) == 0 {
        error(hwnd, "有効期限タイマーを開始できません。");
        let _ = WTSUnRegisterSessionNotification(hwnd);
        let _ = DestroyWindow(hwnd);
        return;
    }
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetFocus(GetDlgItem(hwnd, CANDIDATE).unwrap_or(hwnd));
    #[cfg(test)]
    if tests::EXERCISE.get() {
        tests::exercise(hwnd, &state);
    }
    let mut msg = MSG::default();
    while IsWindow(hwnd).as_bool() {
        if GetMessageW(&mut msg, None, 0, 0).0 <= 0 {
            break;
        }
        if !IsDialogMessageW(hwnd, &msg).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    if IsWindow(hwnd).as_bool() {
        let _ = DestroyWindow(hwnd);
    }
    let _ = DeleteObject(state.state.borrow().font);
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
unsafe fn set(hwnd: HWND, id: i32, s: &str) {
    // This temporary can contain the transfer passphrase. Wipe UTF-16 too.
    let s = Zeroizing::new(wide(s));
    if let Ok(c) = GetDlgItem(hwnd, id) {
        let _ = SetWindowTextW(c, PCWSTR(s.as_ptr()));
    }
}
unsafe fn text(hwnd: HWND, id: i32) -> String {
    let Ok(c) = GetDlgItem(hwnd, id) else {
        return String::new();
    };
    let mut buf = vec![0; GetWindowTextLengthW(c) as usize + 1];
    let n = GetWindowTextW(c, &mut buf);
    String::from_utf16_lossy(&buf[..n as usize])
}
unsafe fn error(hwnd: HWND, message: &str) {
    let s = wide(message);
    MessageBoxW(
        hwnd,
        PCWSTR(s.as_ptr()),
        w!("秘密鍵転送"),
        MB_OK | MB_ICONERROR,
    );
}
unsafe fn refresh_paths(hwnd: HWND, state: &State) {
    SendDlgItemMessageW(hwnd, CANDIDATE, CB_RESETCONTENT, WPARAM(0), LPARAM(0));
    for path in &state.paths {
        let s = wide(path);
        SendDlgItemMessageW(
            hwnd,
            CANDIDATE,
            CB_ADDSTRING,
            WPARAM(0),
            LPARAM(s.as_ptr() as isize),
        );
    }
    if state.paths.len() == 1 {
        SendDlgItemMessageW(hwnd, CANDIDATE, CB_SETCURSEL, WPARAM(0), LPARAM(0));
    }
}
unsafe fn clear(hwnd: HWND, s: &mut State, status: &str) {
    if let Ok(secret) = GetDlgItem(hwnd, SECRET) {
        let _ = ShowWindow(secret, SW_HIDE);
    }
    set(hwnd, SECRET, "");
    s.transfer = None;
    s.codes.clear();
    s.passphrase = false;
    s.paused = true;
    set(hwnd, STATUS, status);
    let _ = InvalidateRect(hwnd, None, true);
}
unsafe fn update(hwnd: HWND, s: &State) {
    if let Some(t) = &s.transfer {
        let remaining = 300u64.saturating_sub(s.created.elapsed().as_secs());
        set(
            hwnd,
            STATUS,
            &format!(
                "{} / {}　残り {} 秒　{}",
                s.index + 1,
                t.frames.len(),
                remaining,
                if s.passphrase {
                    "転送用パスフレーズ（QRは非表示）"
                } else if s.paused {
                    "QR 一時停止"
                } else {
                    "QR 表示中"
                }
            ),
        );
        let _ = InvalidateRect(hwnd, None, true);
    }
}
unsafe fn generate(hwnd: HWND, s: &mut State) -> Result<(), &'static str> {
    clear(hwnd, s, "転送を準備しています…");
    let index = SendDlgItemMessageW(hwnd, CANDIDATE, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0;
    let path = s
        .paths
        .get(usize::try_from(index).map_err(|_| "鍵ファイルを選択してください。")?)
        .ok_or("鍵ファイルを選択してください。")?;
    let name = text(hwnd, NAME);
    let confirmation = wide(&format!("秘密鍵ファイル:\n{path}\n\nこの鍵を持つ人はあなたとして認証できる可能性があります。iPhoneへ転送するQRを作成しますか？\n\nQRは撮影後も復号できます。周囲に注意してください。"));
    if MessageBoxW(
        hwnd,
        PCWSTR(confirmation.as_ptr()),
        w!("秘密鍵の転送を確認"),
        MB_YESNO | MB_DEFBUTTON2 | MB_ICONWARNING,
    ) != IDYES
    {
        return Ok(());
    }
    let file = std::fs::File::open(crate::pty::ssh::expand_path(path))
        .map_err(|_| "鍵ファイルを開けません。");
    let mut key = Zeroizing::new(Vec::with_capacity(tkqr::MAX_KEY + 1));
    file?
        .take((tkqr::MAX_KEY + 1) as u64)
        .read_to_end(&mut key)
        .map_err(|_| "鍵ファイルを読み込めません。")?;
    let format = tkqr::key_format(&key)?;
    let created = Instant::now();
    let transfer = tkqr::create(&name, format, &key)?;
    let codes = transfer
        .frames
        .iter()
        .map(|f| QrCode::with_error_correction_level(f.as_bytes(), EcLevel::M))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "QRを生成できません。")?;
    s.transfer = Some(transfer);
    s.codes = codes;
    s.created = created;
    s.advanced = Instant::now();
    s.index = 0;
    s.passphrase = false;
    s.paused = false;
    update(hwnd, s);
    Ok(())
}

unsafe fn set_font(hwnd: HWND, s: &mut State) {
    let dpi = GetDpiForWindow(hwnd) as i32;
    let font = CreateFontW(
        -16 * dpi / 96,
        0,
        0,
        0,
        400,
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        0,
        0,
        0,
        0,
        w!("Segoe UI"),
    );
    for id in [
        200, CANDIDATE, BROWSE, 201, NAME, GENERATE, 202, STATUS, SECRET, TOGGLE, PAUSE, PREV,
        NEXT, CANCEL,
    ] {
        SendDlgItemMessageW(hwnd, id, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
    }
    let old = std::mem::replace(&mut s.font, font);
    if !old.is_invalid() {
        let _ = DeleteObject(old);
    }
}
unsafe fn layout(hwnd: HWND) {
    let mut r = RECT::default();
    let _ = GetClientRect(hwnd, &mut r);
    let scale = GetDpiForWindow(hwnd) as i32;
    let d = |v| v * scale / 96;
    let width = r.right;
    let height = r.bottom;
    for (id, x, y, w, h) in [
        (200, d(16), d(12), d(160), d(22)),
        (CANDIDATE, d(16), d(38), width - d(216), d(260)),
        (BROWSE, width - d(190), d(38), d(174), d(30)),
        (201, d(16), d(78), d(110), d(24)),
        (NAME, d(126), d(76), width - d(326), d(30)),
        (GENERATE, width - d(190), d(76), d(174), d(30)),
        (202, d(16), d(116), width - d(32), d(48)),
        (STATUS, d(16), d(170), width - d(32), d(48)),
        (SECRET, d(16), height / 2, width - d(32), d(40)),
        (TOGGLE, d(16), height - d(90), d(224), d(30)),
        (PAUSE, d(250), height - d(90), d(200), d(30)),
        (PREV, d(16), height - d(48), d(110), d(30)),
        (NEXT, d(136), height - d(48), d(110), d(30)),
        (CANCEL, width - d(126), height - d(48), d(110), d(30)),
    ] {
        if let Ok(c) = GetDlgItem(hwnd, id) {
            let _ = MoveWindow(c, x, y, w, h, true);
        }
    }
    let _ = InvalidateRect(hwnd, None, true);
}
unsafe fn paint(hwnd: HWND, s: &State) {
    let mut ps = PAINTSTRUCT::default();
    let dc = BeginPaint(hwnd, &mut ps);
    if !s.passphrase && s.created.elapsed() < Duration::from_secs(300) {
        if let Some(code) = s.codes.get(s.index) {
            let mut r = RECT::default();
            let _ = GetClientRect(hwnd, &mut r);
            let dpi = GetDpiForWindow(hwnd) as i32;
            let top = 222 * dpi / 96;
            let bottom = r.bottom - 102 * dpi / 96;
            let modules = code.width() as i32 + 8;
            let unit = ((r.right - 32 * dpi / 96).min(bottom - top) / modules).max(0);
            if unit > 0 {
                let left = (r.right - modules * unit) / 2;
                let y = top + (bottom - top - modules * unit) / 2;
                let white = GetStockObject(WHITE_BRUSH);
                let black = GetStockObject(BLACK_BRUSH);
                FillRect(
                    dc,
                    &RECT {
                        left,
                        top: y,
                        right: left + modules * unit,
                        bottom: y + modules * unit,
                    },
                    HBRUSH(white.0),
                );
                for row in 0..code.width() {
                    for col in 0..code.width() {
                        if code[(col, row)] == Color::Dark {
                            let x = left + (col as i32 + 4) * unit;
                            let y = y + (row as i32 + 4) * unit;
                            FillRect(
                                dc,
                                &RECT {
                                    left: x,
                                    top: y,
                                    right: x + unit,
                                    bottom: y + unit,
                                },
                                HBRUSH(black.0),
                            );
                        }
                    }
                }
            }
        }
    }
    let _ = EndPaint(hwnd, &ps);
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(lp.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    // SAFETY: the window lives only inside run(); its boxed state is stable.
    let window = &*ptr;
    if matches!(msg, WM_WTSSESSION_CHANGE | WM_QUERYENDSESSION | WM_DESTROY) {
        window.invalidated.set(true);
    }
    if msg == WM_DESTROY {
        let _ = KillTimer(hwnd, 1);
        let _ = WTSUnRegisterSessionNotification(hwnd);
    }
    // Native modal dialogs and synchronous control notifications can reenter
    // this procedure. Never create aliased mutable references to State.
    let Ok(mut borrowed) = window.state.try_borrow_mut() else {
        return DefWindowProcW(hwnd, msg, wp, lp);
    };
    let s = &mut *borrowed;
    match msg {
        WM_COMMAND => match (wp.0 & 0xffff) as i32 {
            BROWSE => {
                clear(hwnd, s, "鍵を選び直しています。");
                let mut path = [0u16; 32768];
                let mut file = OPENFILENAMEW {
                    lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
                    hwndOwner: hwnd,
                    lpstrFile: PWSTR(path.as_mut_ptr()),
                    nMaxFile: path.len() as u32,
                    lpstrTitle: w!("転送する秘密鍵を1つ選択"),
                    Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
                    ..Default::default()
                };
                if GetOpenFileNameW(&mut file).as_bool() {
                    let path = String::from_utf16_lossy(
                        &path[..path.iter().position(|c| *c == 0).unwrap_or(path.len())],
                    );
                    if !s.paths.contains(&path) {
                        s.paths.push(path.clone());
                    }
                    refresh_paths(hwnd, s);
                    let i = s.paths.iter().position(|p| *p == path).unwrap();
                    SendDlgItemMessageW(hwnd, CANDIDATE, CB_SETCURSEL, WPARAM(i), LPARAM(0));
                }
            }
            CANDIDATE if (wp.0 >> 16) as u32 == CBN_SELCHANGE => {
                clear(hwnd, s, "鍵が変更されました。「生成」を押してください。")
            }
            NAME if (wp.0 >> 16) as u32 == EN_CHANGE => clear(
                hwnd,
                s,
                "表示名が変更されました。「生成」を押してください。",
            ),
            GENERATE => {
                if let Err(e) = generate(hwnd, s) {
                    clear(hwnd, s, "転送を生成できませんでした。");
                    error(hwnd, e);
                }
            }
            TOGGLE => {
                s.passphrase = !s.passphrase;
                set(hwnd, SECRET, "");
                if let Ok(secret) = GetDlgItem(hwnd, SECRET) {
                    let _ = ShowWindow(secret, SW_HIDE);
                }
                // Erase the old QR synchronously before revealing any password.
                // Reentrant WM_PAINT uses DefWindowProc while State is borrowed.
                let _ = RedrawWindow(hwnd, None, None, RDW_INVALIDATE | RDW_ERASE | RDW_UPDATENOW);
                if s.passphrase {
                    if let Some(t) = &s.transfer {
                        set(hwnd, SECRET, &t.passphrase);
                        if let Ok(secret) = GetDlgItem(hwnd, SECRET) {
                            let _ = ShowWindow(secret, SW_SHOW);
                        }
                    }
                }
                s.advanced = Instant::now();
                update(hwnd, s);
            }
            PAUSE => {
                s.paused = !s.paused;
                s.advanced = Instant::now();
                update(hwnd, s);
            }
            PREV | NEXT if !s.codes.is_empty() => {
                s.paused = true;
                s.index = (s.index
                    + if wp.0 as i32 == PREV {
                        s.codes.len() - 1
                    } else {
                        1
                    })
                    % s.codes.len();
                update(hwnd, s);
            }
            CANCEL => {
                let _ = DestroyWindow(hwnd);
            }
            _ => {}
        },
        WM_TIMER => {
            if s.transfer.is_some() {
                if s.created.elapsed() >= Duration::from_secs(300) {
                    clear(hwnd, s, "5分経過しました。再生成するか閉じてください。");
                } else {
                    let mut changed = s.displayed_second != s.created.elapsed().as_secs();
                    s.displayed_second = s.created.elapsed().as_secs();
                    if !s.paused
                        && !s.passphrase
                        && s.codes.len() > 1
                        && s.advanced.elapsed() >= Duration::from_millis(350)
                    {
                        s.index = (s.index + 1) % s.codes.len();
                        s.advanced = Instant::now();
                        changed = true;
                    }
                    if changed {
                        update(hwnd, s);
                    }
                }
            }
        }
        WM_WTSSESSION_CHANGE | WM_QUERYENDSESSION => {
            clear(
                hwnd,
                s,
                "セッション状態が変わりました。再生成してください。",
            );
            return LRESULT(1);
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
        }
        WM_DESTROY => {
            clear(hwnd, s, "");
            let _ = KillTimer(hwnd, 1);
            let _ = WTSUnRegisterSessionNotification(hwnd);
        }
        WM_SIZE => layout(hwnd),
        WM_DPICHANGED => {
            let r = &*(lp.0 as *const RECT);
            let _ = SetWindowPos(
                hwnd,
                None,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            set_font(hwnd, s);
            layout(hwnd);
        }
        WM_GETMINMAXINFO => {
            let dpi = GetDpiForWindow(hwnd) as i32;
            let info = &mut *(lp.0 as *mut MINMAXINFO);
            info.ptMinTrackSize = POINT {
                x: 560 * dpi / 96,
                y: 600 * dpi / 96,
            };
        }
        WM_PAINT => {
            if window.invalidated.get() {
                clear(hwnd, s, "再生成してください。");
            }
            paint(hwnd, s);
            return LRESULT(0);
        }
        _ => {
            drop(borrowed);
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
    }
    if window.invalidated.replace(false) {
        clear(
            hwnd,
            s,
            "セッション状態が変わりました。再生成してください。",
        );
    }
    LRESULT(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    thread_local! { pub(super) static EXERCISE: Cell<bool> = const { Cell::new(false) }; }

    unsafe fn seed(hwnd: HWND, window: &WindowState) {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../docs/test-vectors/tkqr1-multi.json")).unwrap();
        let frames: Vec<String> = fixture["frames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let codes = frames
            .iter()
            .map(|s| QrCode::with_error_correction_level(s.as_bytes(), EcLevel::M).unwrap())
            .collect();
        let mut s = window.state.borrow_mut();
        s.transfer = Some(tkqr::Transfer {
            passphrase: Zeroizing::new(fixture["passphrase"].as_str().unwrap().to_string()),
            frames,
        });
        s.codes = codes;
        s.created = Instant::now();
        s.advanced = Instant::now();
        s.index = 0;
        s.paused = false;
        update(hwnd, &s);
    }

    pub(super) unsafe fn exercise(hwnd: HWND, window: &WindowState) {
        seed(hwnd, window);
        let secret = GetDlgItem(hwnd, SECRET).unwrap();
        assert!(!IsWindowVisible(secret).as_bool());
        SendMessageW(hwnd, WM_COMMAND, WPARAM(TOGGLE as usize), LPARAM(0));
        assert!(IsWindowVisible(secret).as_bool());
        assert_eq!(GetWindowTextLengthW(secret), 24);
        SendMessageW(hwnd, WM_COMMAND, WPARAM(TOGGLE as usize), LPARAM(0));
        assert!(!IsWindowVisible(secret).as_bool());
        assert_eq!(GetWindowTextLengthW(secret), 0);
        SendMessageW(hwnd, WM_COMMAND, WPARAM(NEXT as usize), LPARAM(0));
        assert_eq!(window.state.borrow().index, 1);
        assert!(window.state.borrow().paused);
        SendMessageW(hwnd, WM_COMMAND, WPARAM(PREV as usize), LPARAM(0));
        assert_eq!(window.state.borrow().index, 0);
        let _ = SetWindowPos(hwnd, None, 0, 0, 620, 730, SWP_NOMOVE | SWP_NOZORDER);
        let _ = RedrawWindow(hwnd, None, None, RDW_INVALIDATE | RDW_ERASE | RDW_UPDATENOW);
        // Save only this publicly documented fixture's rendered UI for visual QA.
        if let Some(path) = std::env::var_os("TKQR_TEST_SCREENSHOT") {
            screenshot(hwnd, std::path::Path::new(&path));
        }
        SendMessageW(
            hwnd,
            WM_WTSSESSION_CHANGE,
            WPARAM(WTS_SESSION_LOCK as usize),
            LPARAM(0),
        );
        assert!(window.state.borrow().transfer.is_none());
        assert!(window.state.borrow().codes.is_empty());
        assert_eq!(GetWindowTextLengthW(secret), 0);
        seed(hwnd, window);
        window.state.borrow_mut().created = Instant::now() - Duration::from_secs(301);
        SendMessageW(hwnd, WM_TIMER, WPARAM(1), LPARAM(0));
        assert!(window.state.borrow().transfer.is_none());
        seed(hwnd, window);
        SendMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        assert!(!IsWindow(hwnd).as_bool());
        assert!(window.state.borrow().transfer.is_none());
    }

    unsafe fn screenshot(hwnd: HWND, path: &std::path::Path) {
        let mut r = RECT::default();
        GetClientRect(hwnd, &mut r).unwrap();
        let dc = GetDC(hwnd);
        let memory = CreateCompatibleDC(dc);
        let bitmap = CreateCompatibleBitmap(dc, r.right, r.bottom);
        let old = SelectObject(memory, bitmap);
        BitBlt(memory, 0, 0, r.right, r.bottom, dc, 0, 0, SRCCOPY).unwrap();
        SelectObject(memory, old);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: r.right,
                biHeight: -r.bottom,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (r.right * r.bottom * 4) as usize];
        assert_ne!(
            GetDIBits(
                memory,
                bitmap,
                0,
                r.bottom as u32,
                Some(pixels.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS
            ),
            0
        );
        for p in pixels.chunks_exact_mut(4) {
            p.swap(0, 2);
            p[3] = 255;
        }
        ::image::save_buffer(
            path,
            &pixels,
            r.right as u32,
            r.bottom as u32,
            ::image::ColorType::Rgba8,
        )
        .unwrap();
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(memory);
        ReleaseDC(hwnd, dc);
    }

    #[test]
    fn native_dialog_hides_secrets_on_toggle_lock_expiry_and_close() {
        EXERCISE.set(true);
        // SAFETY: this test owns its UI thread and all test-only native windows.
        unsafe {
            run(vec!["public synthetic fixture (not a real key)".into()]);
        }
        EXERCISE.set(false);
    }
}

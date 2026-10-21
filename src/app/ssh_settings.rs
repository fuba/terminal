use crate::config::Config;
use crate::pty::SshProfile;
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::UI::Controls::Dialogs::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const PROFILE: i32 = 100;
const NAME: i32 = 101;
const HOST: i32 = 102;
const USER: i32 = 103;
const PORT: i32 = 104;
const KEY: i32 = 105;
const BROWSE: i32 = 106;
const SAVE: i32 = 1;
const CANCEL: i32 = 2;

struct State {
    profiles: Vec<SshProfile>,
    config: Config,
    result: Option<Config>,
}

pub(super) fn upsert_profile(profiles: &mut Vec<SshProfile>, profile: SshProfile) {
    if let Some(existing) = profiles.iter_mut().find(|p| p.name == profile.name) {
        *existing = profile;
    } else {
        profiles.push(profile);
    }
}

pub(super) fn merge_profiles(
    mut imported: Vec<SshProfile>,
    saved: &[SshProfile],
) -> Vec<SshProfile> {
    for profile in saved {
        upsert_profile(&mut imported, profile.clone());
    }
    imported
}

fn make_profile(values: [String; 5]) -> Result<SshProfile, &'static str> {
    let [name, host, user, port, key] = values.map(|v| v.trim().to_string());
    if name.is_empty() || host.is_empty() || user.is_empty() || key.is_empty() {
        return Err("Enter a name, host, user name, and private key file.");
    }
    let port = port
        .parse::<u16>()
        .ok()
        .filter(|p| *p != 0)
        .ok_or("Port must be a number from 1 to 65535.")?;
    Ok(SshProfile {
        name,
        host,
        user,
        port,
        auth: "key".into(),
        password: None,
        key_path: Some(key),
    })
}

pub(super) fn show_error(parent: HWND, message: &str) {
    let wide: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    // SAFETY: the UTF-16 buffer remains alive for this synchronous dialog.
    unsafe {
        MessageBoxW(
            parent,
            PCWSTR(wide.as_ptr()),
            w!("SSH Settings"),
            MB_OK | MB_ICONERROR,
        );
    }
}

pub(super) fn show(parent: HWND, profiles: &[SshProfile], config: &Config) -> Option<Config> {
    // SAFETY: state is owned by this modal loop and outlives the window procedure.
    unsafe {
        let instance = HINSTANCE(GetModuleHandleW(None).ok()?.0);
        let class = w!("TerminalSshSettings");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(6 as *mut _),
            lpszClassName: class,
            ..Default::default()
        });
        let mut state = Box::new(State {
            profiles: profiles.to_vec(),
            config: config.clone(),
            result: None,
        });
        let mut rect = RECT::default();
        let _ = GetWindowRect(parent, &mut rect);
        let hwnd = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            class,
            w!("SSH Settings"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
            (rect.left + (rect.right - rect.left - 560) / 2).max(0),
            (rect.top + (rect.bottom - rect.top - 400) / 2).max(0),
            560,
            400,
            parent,
            None,
            instance,
            Some((&mut *state as *mut State).cast()),
        )
        .ok()?;
        controls(hwnd, instance, profiles);
        let _ = EnableWindow(parent, false);
        let _ = ShowWindow(hwnd, SW_SHOW);
        if let Ok(combo) = GetDlgItem(hwnd, PROFILE) {
            let _ = SetFocus(combo);
        }
        let mut msg = MSG::default();
        while IsWindow(hwnd).as_bool() {
            let result = GetMessageW(&mut msg, None, 0, 0).0;
            if result <= 0 {
                let _ = DestroyWindow(hwnd);
                if result == 0 {
                    PostQuitMessage(msg.wParam.0 as i32);
                }
                break;
            }
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let _ = EnableWindow(parent, true);
        let _ = SetForegroundWindow(parent);
        state.result.take()
    }
}

unsafe fn control(
    parent: HWND,
    instance: HINSTANCE,
    class: PCWSTR,
    text: &str,
    id: i32,
    rect: (i32, i32, i32, i32),
    style: WINDOW_STYLE,
) -> HWND {
    let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        class,
        PCWSTR(text.as_ptr()),
        WS_CHILD | WS_VISIBLE | style,
        rect.0,
        rect.1,
        rect.2,
        rect.3,
        parent,
        HMENU(id as *mut _),
        instance,
        None,
    )
    .unwrap_or_default()
}

unsafe fn controls(hwnd: HWND, instance: HINSTANCE, profiles: &[SshProfile]) {
    control(
        hwnd,
        instance,
        w!("STATIC"),
        "Connection:",
        0,
        (16, 18, 110, 24),
        WINDOW_STYLE(0),
    );
    let combo = control(
        hwnd,
        instance,
        w!("COMBOBOX"),
        "",
        PROFILE,
        (136, 16, 390, 260),
        WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(3),
    );
    for name in std::iter::once("New connection…").chain(profiles.iter().map(|p| p.name.as_str()))
    {
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        SendMessageW(
            combo,
            CB_ADDSTRING,
            WPARAM(0),
            LPARAM(name.as_ptr() as isize),
        );
    }
    SendMessageW(combo, CB_SETCURSEL, WPARAM(0), LPARAM(0));
    for (i, (label, id, value)) in [
        ("Name:", NAME, ""),
        ("Host:", HOST, ""),
        ("User name:", USER, ""),
        ("Port:", PORT, "22"),
        ("Private key:", KEY, "~/.ssh/id_ed25519"),
    ]
    .iter()
    .enumerate()
    {
        let y = 58 + i as i32 * 40;
        control(
            hwnd,
            instance,
            w!("STATIC"),
            label,
            0,
            (16, y + 3, 110, 24),
            WINDOW_STYLE(0),
        );
        control(
            hwnd,
            instance,
            w!("EDIT"),
            value,
            *id,
            (136, y, if *id == KEY { 292 } else { 390 }, 26),
            WS_TABSTOP | WS_BORDER | WINDOW_STYLE(0x80),
        );
    }
    control(
        hwnd,
        instance,
        w!("BUTTON"),
        "Browse…",
        BROWSE,
        (436, 218, 90, 26),
        WS_TABSTOP,
    );
    control(hwnd, instance, w!("STATIC"), "Save adds or replaces an app profile with this name.\nApp profiles take priority over imported SSH settings.",
        0, (16, 260, 510, 40), WINDOW_STYLE(0));
    control(
        hwnd,
        instance,
        w!("BUTTON"),
        "Save",
        SAVE,
        (336, 310, 90, 30),
        WS_TABSTOP | WINDOW_STYLE(1),
    );
    control(
        hwnd,
        instance,
        w!("BUTTON"),
        "Cancel",
        CANCEL,
        (436, 310, 90, 30),
        WS_TABSTOP,
    );
}

unsafe fn text(hwnd: HWND, id: i32) -> String {
    let Ok(control) = GetDlgItem(hwnd, id) else {
        return String::new();
    };
    let mut buf = vec![0u16; GetWindowTextLengthW(control) as usize + 1];
    let count = GetWindowTextW(control, &mut buf);
    String::from_utf16_lossy(&buf[..count as usize])
}

unsafe fn set_text(hwnd: HWND, id: i32, value: &str) {
    if let Ok(control) = GetDlgItem(hwnd, id) {
        let wide: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
        let _ = SetWindowTextW(control, PCWSTR(wide.as_ptr()));
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(lp.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if msg == WM_COMMAND && !ptr.is_null() {
        let state = &mut *ptr;
        match (wp.0 & 0xffff) as i32 {
            PROFILE if (wp.0 >> 16) as u32 == CBN_SELCHANGE => {
                let index = GetDlgItem(hwnd, PROFILE)
                    .map(|c| SendMessageW(c, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0)
                    .unwrap_or(-1);
                if let Some(p) = index
                    .checked_sub(1)
                    .and_then(|i| usize::try_from(i).ok())
                    .and_then(|i| state.profiles.get(i))
                {
                    for (id, value) in [
                        (NAME, p.name.as_str()),
                        (HOST, p.host.as_str()),
                        (USER, p.user.as_str()),
                        (KEY, p.key_path.as_deref().unwrap_or("")),
                    ] {
                        set_text(hwnd, id, value);
                    }
                    set_text(hwnd, PORT, &p.port.to_string());
                } else {
                    for id in [NAME, HOST, USER] {
                        set_text(hwnd, id, "");
                    }
                    set_text(hwnd, PORT, "22");
                    set_text(hwnd, KEY, "~/.ssh/id_ed25519");
                }
            }
            BROWSE => {
                let mut path = [0u16; 32768];
                let mut file = OPENFILENAMEW {
                    lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
                    hwndOwner: hwnd,
                    lpstrFile: PWSTR(path.as_mut_ptr()),
                    nMaxFile: path.len() as u32,
                    lpstrTitle: w!("Select private key"),
                    Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
                    ..Default::default()
                };
                if GetOpenFileNameW(&mut file).as_bool() {
                    let len = path.iter().position(|c| *c == 0).unwrap_or(path.len());
                    set_text(hwnd, KEY, &String::from_utf16_lossy(&path[..len]));
                }
            }
            SAVE => match make_profile([NAME, HOST, USER, PORT, KEY].map(|id| text(hwnd, id))) {
                Ok(profile) => {
                    let key_path =
                        crate::pty::ssh::expand_path(profile.key_path.as_deref().unwrap_or(""));
                    if let Err(error) = russh_keys::load_secret_key(&key_path, None) {
                        show_error(hwnd, &format!("Could not load private key: {error}"));
                        return LRESULT(0);
                    }
                    let mut config = state.config.clone();
                    upsert_profile(&mut config.ssh_profiles, profile);
                    match config.save() {
                        Ok(()) => {
                            state.result = Some(config);
                            let _ = DestroyWindow(hwnd);
                        }
                        Err(error) => {
                            show_error(hwnd, &format!("Could not save SSH settings: {error}"))
                        }
                    }
                }
                Err(error) => show_error(hwnd, error),
            },
            CANCEL => {
                let _ = DestroyWindow(hwnd);
            }
            _ => {}
        }
        return LRESULT(0);
    }
    if msg == WM_CLOSE {
        let _ = DestroyWindow(hwnd);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, user: &str) -> SshProfile {
        make_profile([name, "host", user, "22", "~/.ssh/id_rsa"].map(str::to_string)).unwrap()
    }

    #[test]
    fn saved_profiles_override_imports_and_keep_unrelated_hosts() {
        let merged = merge_profiles(
            vec![profile("puma2", "eccee"), profile("other", "ubuntu")],
            &[profile("puma2", "ec"), profile("tuner", "ec")],
        );
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].user, "ec");
        assert_eq!(merged[1].user, "ubuntu");
        assert_eq!(merged[2].name, "tuner");
    }

    #[test]
    fn rejects_empty_fields_and_invalid_ports() {
        for port in ["0", "65536", "-1", "ssh", ""] {
            assert!(make_profile(["host", "host", "ec", port, "key"].map(str::to_string)).is_err());
        }
        for index in [0, 1, 2, 4] {
            let mut values = ["host", "host", "ec", "22", "key"].map(str::to_string);
            values[index] = " ".into();
            assert!(make_profile(values).is_err());
        }
    }

    #[test]
    fn resaving_replaces_profile_and_roundtrips_through_config() {
        let mut config = crate::config::Config::default();
        upsert_profile(&mut config.ssh_profiles, profile("tuner", "wrong"));
        upsert_profile(&mut config.ssh_profiles, profile("tuner", "ec"));
        let loaded: crate::config::Config =
            toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
        assert_eq!(loaded.ssh_profiles.len(), 1);
        assert_eq!(loaded.ssh_profiles[0].user, "ec");
        assert_eq!(
            loaded.ssh_profiles[0].key_path.as_deref(),
            Some("~/.ssh/id_rsa")
        );
    }
}

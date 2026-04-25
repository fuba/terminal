use crate::pty::SshProfile;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const ID_LIST: i32 = 101;
const ID_CONNECT: i32 = 200;
const ID_CANCEL: i32 = 201;

struct PickerState {
    profiles: Vec<SshProfile>,
    selected: Option<usize>,
}

pub fn pick_profile(parent: HWND, profiles: &[SshProfile]) -> Option<SshProfile> {
    if profiles.is_empty() {
        unsafe {
            MessageBoxW(
                parent,
                w!("No SSH profiles configured.\n\nAdd profiles to config.toml under [[ssh_profiles]]"),
                w!("SSH"),
                MB_OK | MB_ICONINFORMATION,
            );
        }
        return None;
    }

    unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(picker_proc),
            hInstance: HINSTANCE(instance.0),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(6 as *mut _),
            lpszClassName: w!("TerminalSshPicker"),
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let state = Box::new(PickerState {
            profiles: profiles.to_vec(),
            selected: None,
        });
        let state_ptr = Box::into_raw(state);

        let (x, y) = {
            let mut r = RECT::default();
            if GetWindowRect(parent, &mut r).is_ok() {
                let pw = r.right - r.left;
                let ph = r.bottom - r.top;
                (r.left + (pw - 420) / 2, r.top + (ph - 340) / 2)
            } else {
                (CW_USEDEFAULT, CW_USEDEFAULT)
            }
        };
        let hwnd = CreateWindowExW(
            WS_EX_DLGMODALFRAME | WS_EX_TOPMOST,
            w!("TerminalSshPicker"),
            w!("Connect SSH"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            x, y, 420, 340,
            parent, None, HINSTANCE(instance.0),
            Some(state_ptr as *const _),
        );

        if hwnd.is_err() {
            let _ = Box::from_raw(state_ptr);
            return None;
        }
        let hwnd = hwnd.unwrap();
        EnableWindow(parent, false);
        create_controls(hwnd, HINSTANCE(instance.0), profiles);

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
        state.selected.and_then(|i| state.profiles.get(i).cloned())
    }
}

unsafe fn create_controls(hwnd: HWND, inst: HINSTANCE, profiles: &[SshProfile]) {
    // LISTBOX
    let list = CreateWindowExW(
        WS_EX_CLIENTEDGE, w!("LISTBOX"), w!(""),
        WS_CHILD | WS_VISIBLE | WS_VSCROLL | WINDOW_STYLE(0x00100000), // LBS_NOTIFY
        16, 16, 370, 230, hwnd,
        HMENU(ID_LIST as *mut _), inst, None,
    );
    if let Ok(list) = list {
        for p in profiles {
            let text = format!("{} — {}@{}:{}", p.name, p.user, p.host, p.port);
            let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            SendMessageW(list, 0x0180, WPARAM(0), LPARAM(wide.as_ptr() as isize)); // LB_ADDSTRING
        }
        SendMessageW(list, 0x0186, WPARAM(0), LPARAM(0)); // LB_SETCURSEL
    }

    // Connect button
    let wide: Vec<u16> = "Connect\0".encode_utf16().collect();
    let _ = CreateWindowExW(
        WINDOW_EX_STYLE::default(), w!("BUTTON"),
        windows::core::PCWSTR(wide.as_ptr()),
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(0x0001), // BS_DEFPUSHBUTTON
        196, 256, 90, 32, hwnd,
        HMENU(ID_CONNECT as *mut _), inst, None,
    );

    // Cancel button
    let wide: Vec<u16> = "Cancel\0".encode_utf16().collect();
    let _ = CreateWindowExW(
        WINDOW_EX_STYLE::default(), w!("BUTTON"),
        windows::core::PCWSTR(wide.as_ptr()),
        WS_CHILD | WS_VISIBLE, 296, 256, 90, 32, hwnd,
        HMENU(ID_CANCEL as *mut _), inst, None,
    );
}

unsafe fn get_selected_index(hwnd: HWND) -> Option<usize> {
    let list = GetDlgItem(hwnd, ID_LIST).ok()?;
    let result = SendMessageW(list, 0x0188, WPARAM(0), LPARAM(0)); // LB_GETCURSEL
    let idx = result.0;
    if idx < 0 { None } else { Some(idx as usize) }
}

unsafe extern "system" fn picker_proc(
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
            let notify = (wparam.0 >> 16) as u32;
            let sp = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PickerState;
            if sp.is_null() { return DefWindowProcW(hwnd, msg, wparam, lparam); }
            let state = &mut *sp;

            match id {
                ID_LIST if notify == 2 => {
                    // LBN_DBLCLK - double click connects
                    state.selected = get_selected_index(hwnd);
                    let _ = DestroyWindow(hwnd);
                }
                ID_CONNECT => {
                    state.selected = get_selected_index(hwnd);
                    let _ = DestroyWindow(hwnd);
                }
                ID_CANCEL => { let _ = DestroyWindow(hwnd); }
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => { let _ = DestroyWindow(hwnd); LRESULT(0) }
        WM_DESTROY => { LRESULT(0) }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

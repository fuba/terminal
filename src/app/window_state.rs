use crate::config::{Config, WindowPosition};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Returns the stable device name of the monitor under the cursor (e.g. "\\.\DISPLAY1")
fn monitor_name_for_cursor() -> Option<String> {
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        monitor_name(mon)
    }
}

/// Returns the stable device name of the monitor containing the given window
fn monitor_name_for_window(hwnd: HWND) -> Option<String> {
    unsafe {
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        monitor_name(mon)
    }
}

unsafe fn monitor_name(mon: HMONITOR) -> Option<String> {
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
    } else {
        None
    }
}

/// Get the work area (excludes taskbar) for the monitor containing the given point
unsafe fn monitor_work_area_for_point(pt: POINT) -> RECT {
    let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(mon, &mut info).as_bool() {
        info.rcWork
    } else {
        RECT { left: 0, top: 0, right: 800, bottom: 600 }
    }
}

/// Save the current window rect for the current monitor.
/// Only saves if the window is not minimized or docked.
pub fn save_position(hwnd: HWND, config: &mut Config, docked: bool) {
    if docked {
        return; // don't save while docked
    }
    unsafe {
        let mut placement = WINDOWPLACEMENT::default();
        placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        if GetWindowPlacement(hwnd, &mut placement).is_err() {
            return;
        }
        if placement.showCmd == SW_SHOWMINIMIZED.0 as u32 {
            return; // don't save minimized state
        }
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return;
        }
        if let Some(name) = monitor_name_for_window(hwnd) {
            let pos = WindowPosition {
                x: rect.left,
                y: rect.top,
                width: rect.right - rect.left,
                height: rect.bottom - rect.top,
            };
            config.window_positions.insert(name, pos);
            let _ = config.save(); // best-effort persist
        }
    }
}

/// Restore the window position for the monitor currently under the cursor.
/// If no saved position exists for that monitor, uses the default placement.
pub fn restore_position(hwnd: HWND, config: &Config) {
    unsafe {
        let monitor_key = match monitor_name_for_cursor() {
            Some(n) => n,
            None => return,
        };

        let pos = match config.window_positions.get(&monitor_key) {
            Some(p) => p.clone(),
            None => {
                // No saved position for this monitor; move window to its center as default
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let work = monitor_work_area_for_point(pt);
                let width = 900;
                let height = 600;
                let x = work.left + ((work.right - work.left) - width) / 2;
                let y = work.top + ((work.bottom - work.top) - height) / 2;
                let _ = SetWindowPos(
                    hwnd, None, x, y, width, height,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                return;
            }
        };

        // Validate position is within some visible monitor (in case display was disconnected)
        let pt = POINT { x: pos.x + pos.width / 2, y: pos.y + pos.height / 2 };
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONULL);
        let mut rect = RECT {
            left: pos.x, top: pos.y,
            right: pos.x + pos.width, bottom: pos.y + pos.height,
        };
        if mon.is_invalid() {
            // Target monitor no longer exists — fall back to cursor monitor work area
            let cursor_pt = {
                let mut p = POINT::default();
                let _ = GetCursorPos(&mut p);
                p
            };
            let work = monitor_work_area_for_point(cursor_pt);
            let width = pos.width.min(work.right - work.left);
            let height = pos.height.min(work.bottom - work.top);
            let x = work.left + ((work.right - work.left) - width) / 2;
            let y = work.top + ((work.bottom - work.top) - height) / 2;
            rect = RECT { left: x, top: y, right: x + width, bottom: y + height };
        }

        let _ = SetWindowPos(
            hwnd, None,
            rect.left, rect.top,
            rect.right - rect.left, rect.bottom - rect.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

// In release builds, use the Windows subsystem (no console window).
// In debug builds, keep the console for development output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod crash;
mod image;
mod keys;
mod log;
mod perf;
mod pty;
mod render;
mod terminal;
mod url;

pub const BUILD_VERSION: &str = env!("TERMINAL_BUILD_VERSION");

fn main() {
    crash::install();

    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("--install-menu") => {
            match app::shell_integration::install() {
                Ok(()) => show_info("Context menu installed.\n\nRight-click a folder (or empty space inside one) and choose 'Open in Terminal'."),
                Err(e) => show_error(&format!("Failed to install: {}", e)),
            }
            return;
        }
        Some("--uninstall-menu") => {
            match app::shell_integration::uninstall() {
                Ok(()) => show_info("Context menu removed."),
                Err(e) => show_error(&format!("Failed to uninstall: {}", e)),
            }
            return;
        }
        _ => {}
    }

    // An explicit directory (for example from the Explorer context menu) wins.
    // Launches without one, such as from the taskbar, start in the user's home.
    let cwd = args
        .get(1)
        .filter(|p| std::path::Path::new(p).is_dir())
        .cloned()
        .or_else(|| {
            std::env::var("USERPROFILE")
                .ok()
                .filter(|p| std::path::Path::new(p).is_dir())
        });

    if let Err(e) = app::run(cwd) {
        eprintln!("Fatal error: {e}");
        std::process::exit(1);
    }
}

fn show_info(msg: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};
    let m: Vec<u16> = format!("{}\0", msg).encode_utf16().collect();
    let t: Vec<u16> = "Terminal\0".encode_utf16().collect();
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(m.as_ptr()),
            PCWSTR(t.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

fn show_error(msg: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let m: Vec<u16> = format!("{}\0", msg).encode_utf16().collect();
    let t: Vec<u16> = "Terminal\0".encode_utf16().collect();
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(m.as_ptr()),
            PCWSTR(t.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

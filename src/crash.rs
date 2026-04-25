use std::io::Write;
use std::path::PathBuf;

/// Install panic hook and Windows unhandled exception filter.
/// Writes crash info to %APPDATA%\terminal\crash.log
pub fn install() {
    // Rust panic hook
    std::panic::set_hook(Box::new(|info| {
        let mut msg = String::new();
        msg.push_str(&format!(
            "\n==== Panic: {} ====\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        ));
        msg.push_str(&format!("{}\n", info));
        msg.push_str("\nBacktrace:\n");
        let bt = std::backtrace::Backtrace::force_capture();
        msg.push_str(&format!("{}\n", bt));
        write_crash_log(&msg);

        // Also show message box
        show_crash_dialog(&format!("Terminal crashed:\n\n{}\n\nSee {} for details.",
            info, log_path().display()));
    }));

    // Windows unhandled exception filter
    unsafe {
        use windows::Win32::System::Diagnostics::Debug::SetUnhandledExceptionFilter;
        SetUnhandledExceptionFilter(Some(exception_filter));
    }
}

fn log_path() -> PathBuf {
    std::env::var("APPDATA")
        .map(|d| PathBuf::from(d).join("terminal").join("crash.log"))
        .unwrap_or_else(|_| PathBuf::from("crash.log"))
}

fn write_crash_log(msg: &str) {
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(msg.as_bytes());
    }
    // Also print to stderr
    eprintln!("{}", msg);
}

fn show_crash_dialog(msg: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::*;
    let wide: Vec<u16> = msg.encode_utf16().chain(std::iter::once(0)).collect();
    let title: Vec<u16> = "Terminal — Crash\0".encode_utf16().collect();
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(wide.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

unsafe extern "system" fn exception_filter(
    ep: *const windows::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
) -> i32 {
    let code = if !ep.is_null() && !(*ep).ExceptionRecord.is_null() {
        (*(*ep).ExceptionRecord).ExceptionCode.0
    } else {
        0
    };
    let addr = if !ep.is_null() && !(*ep).ExceptionRecord.is_null() {
        (*(*ep).ExceptionRecord).ExceptionAddress as usize
    } else {
        0
    };

    let msg = format!(
        "\n==== Unhandled exception: {} ====\nCode: 0x{:08X} ({})\nAddress: 0x{:X}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        code as u32, exception_name(code as u32), addr,
    );
    write_crash_log(&msg);
    show_crash_dialog(&format!(
        "Terminal crashed:\n\nException: 0x{:08X} ({})\nAddress: 0x{:X}\n\nSee {} for details.",
        code as u32, exception_name(code as u32), addr, log_path().display()
    ));

    // Return EXCEPTION_EXECUTE_HANDLER (1) to terminate the process
    1
}

fn exception_name(code: u32) -> &'static str {
    match code {
        0xC0000005 => "Access violation",
        0xC0000094 => "Integer divide by zero",
        0xC0000095 => "Integer overflow",
        0xC00000FD => "Stack overflow",
        0xC0000006 => "In-page error",
        0xC0000017 => "No memory",
        0xC000001D => "Illegal instruction",
        0xC0000025 => "Noncontinuable exception",
        0xC0000026 => "Invalid disposition",
        0xC000008C => "Array bounds exceeded",
        0xC000008D => "Float denormal operand",
        0xC000008E => "Float divide by zero",
        0xC0000090 => "Float invalid operation",
        0xC0000091 => "Float overflow",
        0xC0000093 => "Float underflow",
        0x80000003 => "Breakpoint",
        0x80000002 => "Datatype misalignment",
        _ => "Unknown",
    }
}

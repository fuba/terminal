use std::io::{self, Write};
use std::os::windows::io::FromRawHandle;
use windows::core::PWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::System::Console::*;
use windows::Win32::System::Pipes::*;
use windows::Win32::System::Threading::*;

const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x00020016;

pub struct ConPty {
    hpc: HPCON,
    writer: std::fs::File,
    process_handle: HANDLE,
    thread_handle: HANDLE,
}

// HPCON is a handle (just an integer), safe to send across threads
unsafe impl Send for ConPty {}

impl ConPty {
    pub fn spawn(cmd: &str, cols: u16, rows: u16) -> windows::core::Result<(Self, std::fs::File)> {
        Self::spawn_with_cwd(cmd, None, cols, rows)
    }

    pub fn spawn_with_cwd(cmd: &str, cwd: Option<&str>, cols: u16, rows: u16) -> windows::core::Result<(Self, std::fs::File)> {
        unsafe {
            // Create pipes for PTY I/O
            let mut pipe_in_read = HANDLE::default();
            let mut pipe_in_write = HANDLE::default();
            let mut pipe_out_read = HANDLE::default();
            let mut pipe_out_write = HANDLE::default();

            CreatePipe(&mut pipe_in_read, &mut pipe_in_write, None, 0)?;
            CreatePipe(&mut pipe_out_read, &mut pipe_out_write, None, 0)?;

            // Create pseudo console
            let size = COORD {
                X: cols as i16,
                Y: rows as i16,
            };
            let hpc = CreatePseudoConsole(size, pipe_in_read, pipe_out_write, 0)?;
            // Close pipe ends now owned by the pseudo console
            let _ = CloseHandle(pipe_in_read);
            let _ = CloseHandle(pipe_out_write);

            // Initialize process thread attribute list
            let mut attr_size: usize = 0;
            let _ = InitializeProcThreadAttributeList(
                LPPROC_THREAD_ATTRIBUTE_LIST(std::ptr::null_mut()),
                1,
                0,
                &mut attr_size,
            );

            let mut attr_list_buf = vec![0u8; attr_size];
            let attr_list = LPPROC_THREAD_ATTRIBUTE_LIST(attr_list_buf.as_mut_ptr() as _);
            InitializeProcThreadAttributeList(attr_list, 1, 0, &mut attr_size)?;

            // Pass HPCON value directly as lpValue (C API pattern)
            UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                Some(hpc.0 as *const std::ffi::c_void),
                std::mem::size_of::<HPCON>(),
                None,
                None,
            )?;

            // Create process
            let si = STARTUPINFOEXW {
                StartupInfo: STARTUPINFOW {
                    cb: std::mem::size_of::<STARTUPINFOEXW>() as u32,
                    ..Default::default()
                },
                lpAttributeList: attr_list,
            };

            let mut pi = PROCESS_INFORMATION::default();
            let mut cmd_line: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
            let cwd_wide: Option<Vec<u16>> = cwd.map(|d|
                d.encode_utf16().chain(std::iter::once(0)).collect()
            );
            let cwd_pcwstr = cwd_wide
                .as_ref()
                .map(|w| windows::core::PCWSTR(w.as_ptr()))
                .unwrap_or(windows::core::PCWSTR::null());

            CreateProcessW(
                None,
                PWSTR(cmd_line.as_mut_ptr()),
                None,
                None,
                false,
                EXTENDED_STARTUPINFO_PRESENT,
                None,
                cwd_pcwstr,
                &si.StartupInfo,
                &mut pi,
            )?;

            DeleteProcThreadAttributeList(attr_list);
            // Wrap pipe handles as File for standard Read/Write traits
            let writer =
                std::fs::File::from_raw_handle(pipe_in_write.0 as *mut std::ffi::c_void);
            let reader =
                std::fs::File::from_raw_handle(pipe_out_read.0 as *mut std::ffi::c_void);

            Ok((
                ConPty {
                    hpc,
                    writer,
                    process_handle: pi.hProcess,
                    thread_handle: pi.hThread,
                },
                reader,
            ))
        }
    }

    pub fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.writer.write_all(data)
    }

    pub fn resize(&self, cols: u16, rows: u16) -> windows::core::Result<()> {
        let size = COORD {
            X: cols as i16,
            Y: rows as i16,
        };
        unsafe { ResizePseudoConsole(self.hpc, size) }
    }
}

impl Drop for ConPty {
    fn drop(&mut self) {
        unsafe {
            ClosePseudoConsole(self.hpc);
            let _ = CloseHandle(self.process_handle);
            let _ = CloseHandle(self.thread_handle);
        }
    }
}

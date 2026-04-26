use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::*;

const HKCU: HKEY = HKEY(0x80000001u32 as *mut _);

const MENU_NAME: &str = "Open in Terminal";
// HKCU base paths for both folder and folder-background
const KEY_FOLDER: &str = r"Software\Classes\Directory\shell\Terminal";
const KEY_FOLDER_BG: &str = r"Software\Classes\Directory\Background\shell\Terminal";

pub fn install() -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("get current_exe: {}", e))?
        .to_string_lossy()
        .into_owned();
    let cmd = format!("\"{}\" \"%V\"", exe);

    write_menu_keys(KEY_FOLDER, &exe, &cmd)?;
    write_menu_keys(KEY_FOLDER_BG, &exe, &cmd)?;
    Ok(())
}

pub fn uninstall() -> Result<(), String> {
    delete_tree(HKCU, KEY_FOLDER)?;
    delete_tree(HKCU, KEY_FOLDER_BG)?;
    Ok(())
}

fn write_menu_keys(base: &str, icon: &str, command: &str) -> Result<(), String> {
    create_key(HKCU, base)?;
    set_value(HKCU, base, "", MENU_NAME)?;
    set_value(HKCU, base, "Icon", icon)?;
    let cmd_key = format!("{}\\command", base);
    create_key(HKCU, &cmd_key)?;
    set_value(HKCU, &cmd_key, "", command)?;
    Ok(())
}

fn create_key(root: HKEY, path: &str) -> Result<(), String> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut hkey = HKEY::default();
    unsafe {
        let result = RegCreateKeyExW(
            root,
            PCWSTR(wide.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        );
        if result != ERROR_SUCCESS {
            return Err(format!("RegCreateKeyExW({}) -> {:?}", path, result));
        }
        let _ = RegCloseKey(hkey);
    }
    Ok(())
}

fn set_value(root: HKEY, path: &str, name: &str, value: &str) -> Result<(), String> {
    let wide_path: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let wide_name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let wide_value: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let value_bytes = unsafe {
        std::slice::from_raw_parts(
            wide_value.as_ptr() as *const u8,
            wide_value.len() * 2,
        )
    };
    unsafe {
        let mut hkey = HKEY::default();
        let r = RegOpenKeyExW(
            root,
            PCWSTR(wide_path.as_ptr()),
            0,
            KEY_WRITE,
            &mut hkey,
        );
        if r != ERROR_SUCCESS {
            return Err(format!("RegOpenKeyExW({}) -> {:?}", path, r));
        }
        let r = RegSetValueExW(
            hkey,
            PCWSTR(wide_name.as_ptr()),
            0,
            REG_SZ,
            Some(value_bytes),
        );
        let _ = RegCloseKey(hkey);
        if r != ERROR_SUCCESS {
            return Err(format!("RegSetValueExW({}/{}) -> {:?}", path, name, r));
        }
    }
    Ok(())
}

fn delete_tree(root: HKEY, path: &str) -> Result<(), String> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let r = RegDeleteTreeW(root, PCWSTR(wide.as_ptr()));
        if r != ERROR_SUCCESS && r.0 != 2 {
            // ignore "not found" (ERROR_FILE_NOT_FOUND = 2)
            return Err(format!("RegDeleteTreeW({}) -> {:?}", path, r));
        }
    }
    Ok(())
}

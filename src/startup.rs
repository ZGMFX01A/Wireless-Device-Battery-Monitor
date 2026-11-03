//! Only manage the preview's own HKCU value after an explicit preference change.
use windows_sys::Win32::{Foundation::*, System::Registry::*};
const NAME: &str = "WirelessDeviceBatteryMonitor-GPUI";
const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}
fn command() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let s = exe.to_str().ok_or("Executable path is not valid Unicode")?;
    Ok(format!("\"{s}\""))
}
fn value(n: &str) -> Result<Option<String>, String> {
    let mut b = [0u16; 32768];
    let mut len = (b.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide(RUN).as_ptr(),
            wide(n).as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            b.as_mut_ptr().cast(),
            &mut len,
        )
    };
    match rc {
        ERROR_FILE_NOT_FOUND => Ok(None),
        0 => Ok(Some(
            String::from_utf16_lossy(&b[..(len as usize / 2).min(b.len())])
                .trim_end_matches('\0')
                .to_owned(),
        )),
        x => Err(format!("Cannot read Run key: {x}")),
    }
}
pub fn enabled() -> Result<bool, String> {
    Ok(value(NAME)?.as_deref() == Some(&command()?))
}
pub fn set(on: bool) -> Result<(), String> {
    if on && value("MouseBatteryMonitor")?.is_some() {
        return Err("原版已设置自启动。请先在原版关闭；新版不会更改旧版 Run 项。".into());
    }
    let existing = value(NAME)?;
    let cmd = command()?;
    if existing.as_ref().is_some_and(|v| v != &cmd) {
        return Err("Another GPUI installation owns this startup entry".into());
    }
    if !on && existing.is_none() {
        return Ok(());
    }
    let rc = if on {
        let s = wide(&cmd);
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                wide(RUN).as_ptr(),
                wide(NAME).as_ptr(),
                REG_SZ,
                s.as_ptr().cast(),
                (s.len() * 2) as u32,
            )
        }
    } else {
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(RUN).as_ptr(), wide(NAME).as_ptr()) }
    };
    if rc != 0 {
        return Err(format!("Run key write failed: {rc}"));
    }
    if enabled()? != on {
        return Err("startup verification failed".into());
    }
    Ok(())
}

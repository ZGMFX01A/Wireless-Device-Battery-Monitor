//! Update HKCU startup only on an explicit preference change, migrating legacy startup on enable.
use windows_sys::Win32::{Foundation::*, System::Registry::*};
const NAME: &str = "WirelessDeviceBatteryMonitor-GPUI";
const LEGACY_NAME: &str = "MouseBatteryMonitor";
const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}
fn command() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let s = exe.to_str().ok_or("Executable path is not valid Unicode")?;
    Ok(format!("\"{s}\""))
}
fn value_at(run: &str, n: &str) -> Result<Option<String>, String> {
    let mut b = [0u16; 32768];
    let mut len = (b.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide(run).as_ptr(),
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
    Ok(value_at(RUN, NAME)?.as_deref() == Some(&command()?))
}
pub fn set(on: bool) -> Result<(), String> {
    set_at(RUN, &command()?, on)
}
fn set_at(run: &str, cmd: &str, on: bool) -> Result<(), String> {
    set_with_writer(run, cmd, on, |name, value| write_at(run, name, value))
}
fn set_with_writer(
    run: &str,
    cmd: &str,
    on: bool,
    mut write: impl FnMut(&str, Option<&str>) -> Result<(), String>,
) -> Result<(), String> {
    let existing = value_at(run, NAME)?;
    if existing.as_deref().is_some_and(|v| v != cmd) {
        return Err("Another GPUI installation owns this startup entry".into());
    }
    if !on && existing.is_none() {
        return Ok(());
    }
    let legacy = if on {
        value_at(run, LEGACY_NAME)?
    } else {
        None
    };
    let desired = on.then_some(cmd);
    let result = (|| {
        write(NAME, desired)?;
        if value_at(run, NAME)?.as_deref() != desired {
            return Err("startup verification failed".into());
        }
        // Keep the old entry until the replacement is verified. Deleting it is
        // the final operation, so a failure can restore our previous entry.
        if legacy.is_some() {
            write(LEGACY_NAME, None)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        return match write(NAME, existing.as_deref()) {
            Ok(()) => Err(error),
            Err(rollback) => Err(format!("{error}; startup rollback failed: {rollback}")),
        };
    }
    Ok(())
}
fn write_at(run: &str, name: &str, value: Option<&str>) -> Result<(), String> {
    let rc = if let Some(cmd) = value {
        let s = wide(cmd);
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                wide(run).as_ptr(),
                wide(name).as_ptr(),
                REG_SZ,
                s.as_ptr().cast(),
                (s.len() * 2) as u32,
            )
        }
    } else {
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(run).as_ptr(), wide(name).as_ptr()) }
    };
    if rc != ERROR_SUCCESS && !(value.is_none() && rc == ERROR_FILE_NOT_FOUND) {
        return Err(format!("Run key write failed: {rc}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestRun(String);
    impl TestRun {
        fn new() -> Self {
            // Each test has a private HKCU key; never change the real Run key.
            let dir = tempfile::tempdir().unwrap();
            Self(format!(
                r"Software\MouseBatteryStartupTest-{}",
                dir.path().file_name().unwrap().to_str().unwrap()
            ))
        }

        fn put(&self, name: &str, cmd: &str) {
            let s = wide(cmd);
            assert_eq!(
                unsafe {
                    RegSetKeyValueW(
                        HKEY_CURRENT_USER,
                        wide(&self.0).as_ptr(),
                        wide(name).as_ptr(),
                        REG_SZ,
                        s.as_ptr().cast(),
                        (s.len() * 2) as u32,
                    )
                },
                ERROR_SUCCESS
            );
        }

        fn get(&self, name: &str) -> Option<String> {
            value_at(&self.0, name).unwrap()
        }
    }
    impl Drop for TestRun {
        fn drop(&mut self) {
            unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide(&self.0).as_ptr()) };
        }
    }

    const CMD: &str = r#""C:\新版 程序\battery.exe""#;

    #[test]
    fn deleted_legacy_executable_does_not_block_startup() {
        let run = TestRun::new();
        run.put(LEGACY_NAME, r#""C:\deleted-legacy\battery.exe" --hidden"#);
        run.put("UnrelatedApp", "keep.exe");
        let result = set_at(&run.0, CMD, true);
        assert!(result.is_ok(), "legacy takeover failed: {result:?}");
        assert_eq!(run.get(NAME).as_deref(), Some(CMD));
        assert_eq!(run.get(LEGACY_NAME), None);
        assert_eq!(run.get("UnrelatedApp").as_deref(), Some("keep.exe"));
    }

    #[test]
    fn disable_does_not_change_legacy_startup() {
        let run = TestRun::new();
        run.put(NAME, CMD);
        run.put(LEGACY_NAME, "legacy.exe");
        set_at(&run.0, CMD, false).unwrap();
        set_at(&run.0, CMD, false).unwrap();
        assert_eq!(run.get(NAME), None);
        assert_eq!(run.get(LEGACY_NAME).as_deref(), Some("legacy.exe"));
    }

    #[test]
    fn another_installation_is_preserved() {
        let run = TestRun::new();
        run.put(NAME, "another.exe");
        run.put(LEGACY_NAME, "legacy.exe");
        assert!(set_at(&run.0, CMD, true).is_err());
        assert_eq!(run.get(NAME).as_deref(), Some("another.exe"));
        assert_eq!(run.get(LEGACY_NAME).as_deref(), Some("legacy.exe"));
    }

    #[test]
    fn enable_without_legacy_is_idempotent() {
        let run = TestRun::new();
        set_at(&run.0, CMD, true).unwrap();
        set_at(&run.0, CMD, true).unwrap();
        assert_eq!(run.get(NAME).as_deref(), Some(CMD));
        set_at(&run.0, CMD, false).unwrap();
        assert_eq!(run.get(NAME), None);
    }

    #[test]
    fn migration_failure_restores_previous_startup() {
        for previous in [None, Some(CMD)] {
            let run = TestRun::new();
            if let Some(previous) = previous {
                run.put(NAME, previous);
            }
            run.put(LEGACY_NAME, "legacy.exe");
            let result = set_with_writer(&run.0, CMD, true, |name, value| {
                if name == LEGACY_NAME {
                    // The replacement must already be verified before cleanup.
                    assert_eq!(run.get(NAME).as_deref(), Some(CMD));
                    return Err("legacy deletion denied".into());
                }
                write_at(&run.0, name, value)
            });
            assert_eq!(result.unwrap_err(), "legacy deletion denied");
            assert_eq!(run.get(NAME).as_deref(), previous);
            assert_eq!(run.get(LEGACY_NAME).as_deref(), Some("legacy.exe"));
        }
    }

    #[test]
    fn replacement_must_verify_before_legacy_is_removed() {
        let run = TestRun::new();
        run.put(LEGACY_NAME, "legacy.exe");
        let result = set_with_writer(&run.0, CMD, true, |name, value| {
            assert_eq!(name, NAME, "must not remove legacy before verification");
            // Simulate a successful write that does not persist the new value.
            if value.is_none() {
                write_at(&run.0, name, value)?;
            }
            Ok(())
        });
        assert_eq!(result.unwrap_err(), "startup verification failed");
        assert_eq!(run.get(NAME), None);
        assert_eq!(run.get(LEGACY_NAME).as_deref(), Some("legacy.exe"));
    }
}

//! Same-volume GPUI-only portable update. The helper runs from a fresh per-transaction directory.
//! Never takes a user-provided destination, never writes the old Python executable or HKCU Run.
use crate::update::{Envelope, KEY, Manifest, validate, verified};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
    Storage::FileSystem::{CopyFileW, ReplaceFileW},
    System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        QueryFullProcessImageNameW, WaitForSingleObject,
    },
};
#[cfg(test)]
const APP: &str = "mouse-battery-gpui.exe";
const HELPER: &str = "gpui-swap-helper.exe";
const PREFIX: &str = ".gpui-update-";
type R<T> = Result<T, String>;
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn wide(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain([0]).collect()
}
fn sha(p: &Path) -> R<String> {
    let mut r = fs::File::open(p).map_err(err)?;
    let mut h = Sha256::new();
    let mut b = [0u8; 65536];
    loop {
        let n = r.read(&mut b).map_err(err)?;
        if n == 0 {
            break;
        }
        h.update(&b[..n]);
    }
    Ok(hex::encode(h.finalize()))
}
fn signed(p: &Path) -> R<Manifest> {
    let file = fs::File::open(p).map_err(err)?;
    if file.metadata().map_err(err)?.len() > 65536 {
        return Err("oversized signature envelope".into());
    }
    let e: Envelope = serde_json::from_reader(file).map_err(err)?;
    let m = validate(
        &e,
        KEY.ok_or("No trusted GPUI release key")?,
        env!("APP_VERSION"),
    )?;
    if m.version == env!("APP_VERSION") {
        return Err("update must be newer".into());
    }
    Ok(m)
}
fn signal(child: &mut Child, expected: &'static str, timeout: Duration) -> R<()> {
    let stdout = child.stdout.take().ok_or("missing health channel")?;
    let (tx, rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut s = String::new();
        let r = BufReader::new(stdout).take(128).read_line(&mut s);
        let _ = tx.send(r.is_ok() && s.trim() == expected);
    });
    let start = Instant::now();
    loop {
        if child.try_wait().map_err(err)?.is_some() {
            return Err("process exited before health confirmation".into());
        }
        match rx.recv_timeout(Duration::from_millis(40)) {
            Ok(true) => return Ok(()),
            Ok(false) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("invalid health handshake".into());
            }
            _ => (),
        }
        if start.elapsed() >= timeout {
            return Err("health check timed out".into());
        }
    }
}
pub fn prepare(src: &Path, m: &Manifest) -> R<ChildStdin> {
    let old = std::env::current_exe()
        .map_err(err)?
        .canonicalize()
        .map_err(err)?;
    let dir = old.parent().ok_or("missing installation directory")?;
    let meta = src.with_file_name("signed.json");
    let validated = signed(&meta)?;
    if validated.sha256 != m.sha256 {
        return Err("signed candidate changed".into());
    }
    verified(src, &validated)?;
    // Check the verified image before asking the old application to shut down.
    preflight(src, &validated.version)?;
    let job = tempfile::Builder::new()
        .prefix(PREFIX)
        .tempdir_in(dir)
        .map_err(err)?;
    let helper = job.path().join(HELPER);
    fs::copy(&old, &helper).map_err(err)?;
    fs::copy(src, job.path().join("candidate.exe")).map_err(err)?;
    fs::copy(meta, job.path().join("signed.json")).map_err(err)?;
    verified(&job.path().join("candidate.exe"), &validated)?;
    let path = job.path().to_path_buf();
    let mut child = Command::new(&helper)
        .arg("--gpui-update-worker")
        .env("GPUI_UPDATE_PARENT", std::process::id().to_string())
        .current_dir(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(err)?;
    if let Err(e) = signal(&mut child, "GPUI-ARMED/1", Duration::from_secs(10)) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e);
    }
    let _ = job.keep();
    child.stdin.take().ok_or("missing transfer channel".into())
}

fn preflight(path: &Path, version: &str) -> R<()> {
    for (arg, expected) in [("--version", Some(version)), ("--smoke-test", None)] {
        let mut child = Command::new(path)
            .arg(arg)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(err)?;
        let stdout = child.stdout.take().ok_or("missing preflight pipe")?;
        let (tx, rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut output = String::new();
            let result = stdout
                .take(1024)
                .read_to_string(&mut output)
                .map(|_| output);
            let _ = tx.send(result);
        });
        let result: R<()> = (|| {
            let output = rx
                .recv_timeout(Duration::from_secs(10))
                .map_err(|_| "candidate preflight timed out")?
                .map_err(err)?;
            let end = Instant::now() + Duration::from_secs(2);
            loop {
                if let Some(code) = child.try_wait().map_err(err)? {
                    if !code.success() || expected.is_some_and(|v| output.trim() != v) {
                        return Err("candidate version/smoke preflight failed".into());
                    }
                    return Ok(());
                }
                if Instant::now() >= end {
                    return Err("candidate preflight did not exit".into());
                }
                thread::sleep(Duration::from_millis(20));
            }
        })();
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        result?;
    }
    Ok(())
}
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn parent() -> R<(Handle, PathBuf)> {
    let id = std::env::var("GPUI_UPDATE_PARENT")
        .map_err(err)?
        .parse::<u32>()
        .map_err(err)?;
    let h = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            id,
        )
    };
    if h.is_null() {
        return Err("parent unavailable".into());
    }
    let h = Handle(h);
    let mut b = vec![0u16; 32768];
    let mut n = b.len() as u32;
    if unsafe { QueryFullProcessImageNameW(h.0, 0, b.as_mut_ptr(), &mut n) } == 0 {
        return Err(err(std::io::Error::last_os_error()));
    }
    let actual = PathBuf::from(std::ffi::OsString::from_wide(&b[..n as usize]))
        .canonicalize()
        .map_err(err)?;
    Ok((h, actual))
}
fn validate_target(job: &Path, target: &Path, helper: &Path) -> R<()> {
    let installation = job
        .parent()
        .ok_or("missing installation directory")?
        .canonicalize()
        .map_err(err)?;
    let parent = target
        .parent()
        .ok_or("missing parent image directory")?
        .canonicalize()
        .map_err(err)?;
    if parent != installation
        || target
            .extension()
            .is_none_or(|e| !e.eq_ignore_ascii_case("exe"))
    {
        return Err("parent image is outside this installation".into());
    }
    if sha(target)? != sha(helper)? {
        return Err("previous executable mismatch".into());
    }
    Ok(())
}
fn swap(target: &Path, source: &Path, backup: &Path) -> R<()> {
    if backup.exists() {
        return Err("Backup already exists; refusing overwrite".into());
    }
    let ok = unsafe {
        ReplaceFileW(
            wide(target).as_ptr(),
            wide(source).as_ptr(),
            wide(backup).as_ptr(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok != 0 {
        Ok(())
    } else {
        Err(err(std::io::Error::last_os_error()))
    }
}
fn transition(
    job: &Path,
    target: &Path,
    old_hash: &str,
    new_hash: &str,
    launch: impl FnOnce(&Path) -> R<()>,
) -> R<()> {
    transition_using(job, target, old_hash, new_hash, swap, launch)
}

// ReplaceFile can fail AFTER moving the target aside (ERROR 1177). Restore a
// missing path with fail-if-exists semantics; never overwrite a concurrent file.
fn restore_missing(target: &Path, backup: &Path, old_hash: &str) -> R<()> {
    if target.exists() {
        return Ok(());
    }
    if sha(backup)? != old_hash {
        return Err("recovery backup hash mismatch".into());
    }
    if unsafe { CopyFileW(wide(backup).as_ptr(), wide(target).as_ptr(), 1) } == 0 {
        return Err(err(std::io::Error::last_os_error()));
    }
    Ok(())
}

fn transition_using(
    job: &Path,
    target: &Path,
    old_hash: &str,
    new_hash: &str,
    mut replace: impl FnMut(&Path, &Path, &Path) -> R<()>,
    launch: impl FnOnce(&Path) -> R<()>,
) -> R<()> {
    let candidate = job.join("candidate.exe");
    let backup = job.join("previous.exe");
    if sha(target)? != old_hash || sha(&candidate)? != new_hash {
        return Err("image changed before transaction".into());
    }
    fs::write(job.join("stage.txt"), "prepared").map_err(err)?;
    if let Err(e) = replace(target, &candidate, &backup) {
        if !target.exists() && backup.exists() {
            restore_missing(target, &backup, old_hash)?;
            fs::write(job.join("stage.txt"), "rolled-back").map_err(err)?;
        }
        return Err(e);
    }
    if let Err(e) = launch(target) {
        if sha(target)? != new_hash || sha(&backup)? != old_hash {
            return Err(format!(
                "{e}; concurrent change; refusing rollback, backup retained"
            ));
        }
        if let Err(rollback) = replace(target, &backup, &job.join("rejected.exe")) {
            restore_missing(target, &backup, old_hash)?;
            if sha(target)? != old_hash {
                return Err(format!("{e}; rollback: {rollback}"));
            }
        }
        fs::write(job.join("stage.txt"), "rolled-back").map_err(err)?;
        return Err(e);
    }
    fs::write(job.join("stage.txt"), "confirmed").map_err(err)?;
    Ok(())
}
fn launch(p: &Path) -> R<()> {
    let mut child = Command::new(p)
        .arg("--gpui-health-start")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(err)?;
    if let Err(e) = signal(&mut child, "GPUI-HEALTH/1", Duration::from_secs(15)) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e);
    }
    thread::sleep(Duration::from_millis(800));
    if child.try_wait().map_err(err)?.is_some() {
        Err("candidate exited after initialization".into())
    } else {
        Ok(())
    }
}
pub fn helper() -> R<()> {
    let me = std::env::current_exe()
        .map_err(err)?
        .canonicalize()
        .map_err(err)?;
    let job = me.parent().ok_or("missing job directory")?;
    if me.file_name().and_then(|x| x.to_str()) != Some(HELPER)
        || !job
            .file_name()
            .and_then(|x| x.to_str())
            .is_some_and(|x| x.starts_with(PREFIX))
    {
        return Err("not a GPUI update transaction".into());
    }
    let (h, target) = parent()?;
    validate_target(job, &target, &me)?;
    let old = sha(&me)?;
    let man = signed(&job.join("signed.json"))?;
    verified(&job.join("candidate.exe"), &man)?;
    println!("GPUI-ARMED/1");
    std::io::stdout().flush().map_err(err)?;
    let mut ans = String::new();
    BufReader::new(std::io::stdin().take(64))
        .read_line(&mut ans)
        .map_err(err)?;
    if ans.is_empty() {
        // The parent cancelled while preparation was running. EOF is a normal
        // abort, not an error dialog in an otherwise invisible helper process.
        return Ok(());
    }
    if ans.trim() != "GPUI-DEPART/1" {
        return Err("handoff not authorized".into());
    }
    if unsafe { WaitForSingleObject(h.0, 30000) } != WAIT_OBJECT_0 {
        return Err("previous process is still active; installation unchanged".into());
    }
    let res = transition(job, &target, &old, &man.sha256.to_ascii_lowercase(), launch);
    if res.is_err() {
        let _ = crate::update::record_failed(&man);
        // Also restart after a failed FIRST replacement, not only health rollback.
        if sha(&target).ok().as_deref() == Some(&old) {
            Command::new(&target)
                .env("WDBM_UPDATE_ROLLED_BACK", "1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(err)?;
        }
    }
    res
}
#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf, String, String) {
        let d = tempfile::tempdir().unwrap();
        let t = d.path().join(APP);
        let j = d.path().join("job");
        fs::create_dir(&j).unwrap();
        fs::write(&t, b"old GPUI").unwrap();
        fs::write(j.join("candidate.exe"), b"new GPUI").unwrap();
        let a = sha(&t).unwrap();
        let b = sha(&j.join("candidate.exe")).unwrap();
        (d, t, j, a, b)
    }
    #[test]
    fn health_failure_restores_original_bytes() {
        let (_d, t, j, a, b) = setup();
        assert!(transition(&j, &t, &a, &b, |_| Err("crash".into())).is_err());
        assert_eq!(sha(&t).unwrap(), a);
        assert_eq!(sha(&j.join("rejected.exe")).unwrap(), b);
    }
    #[test]
    fn success_keeps_old_bytes() {
        let (_d, t, j, a, b) = setup();
        transition(&j, &t, &a, &b, |_| Ok(())).unwrap();
        assert_eq!(sha(&t).unwrap(), b);
        assert_eq!(sha(&j.join("previous.exe")).unwrap(), a)
    }
    #[test]
    fn hash_change_rejected_before_swap() {
        let (_d, t, j, a, b) = setup();
        fs::write(j.join("candidate.exe"), "different").unwrap();
        assert!(transition(&j, &t, &a, &b, |_| panic!()).is_err());
        assert_eq!(sha(&t).unwrap(), a);
    }
    #[test]
    fn concurrent_change_never_overwritten() {
        let (_d, t, j, a, b) = setup();
        assert!(
            transition(&j, &t, &a, &b, |p| {
                fs::write(p, "concurrent change").unwrap();
                Err("changed".into())
            })
            .is_err()
        );
        assert_eq!(fs::read_to_string(t).unwrap(), "concurrent change");
        assert_eq!(sha(&j.join("previous.exe")).unwrap(), a);
    }
    #[test]
    fn partial_replace_failure_restores_missing_executable() {
        let (_d, t, j, a, b) = setup();
        let result = transition_using(
            &j,
            &t,
            &a,
            &b,
            |target, _, backup| {
                fs::rename(target, backup).unwrap();
                Err("simulated ERROR_UNABLE_TO_MOVE_REPLACEMENT_2".into())
            },
            |_| panic!("failed replacement must not launch candidate"),
        );
        assert!(result.is_err());
        assert_eq!(sha(&t).unwrap(), a);
        assert_eq!(sha(&j.join("previous.exe")).unwrap(), a);
    }
    #[test]
    fn restore_does_not_overwrite_a_concurrent_target() {
        let (_d, t, j, a, _) = setup();
        let backup = j.join("previous.exe");
        fs::write(&backup, b"old GPUI").unwrap();
        fs::write(&t, b"concurrent file").unwrap();
        restore_missing(&t, &backup, &a).unwrap();
        assert_eq!(fs::read(t).unwrap(), b"concurrent file");
    }
}

#[cfg(test)]
mod portable_name_tests {
    use super::*;
    #[test]
    fn release_name_and_user_rename_are_safe_with_exact_parent_identity() {
        let d = tempfile::tempdir().unwrap();
        let job = d.path().join(".gpui-update-test");
        fs::create_dir(&job).unwrap();
        let helper = job.join(HELPER);
        fs::write(&helper, b"same executable").unwrap();
        for name in [
            "WirelessDeviceBatteryMonitor-v2.8.1.exe",
            "我的电量工具.exe",
            "mouse-battery-gpui.exe",
        ] {
            let target = d.path().join(name);
            fs::write(&target, b"same executable").unwrap();
            assert!(validate_target(&job, &target, &helper).is_ok());
            fs::write(&target, b"different executable").unwrap();
            assert!(validate_target(&job, &target, &helper).is_err());
        }
        let foreign = tempfile::tempdir().unwrap();
        let outside = foreign.path().join("app.exe");
        fs::write(&outside, b"same executable").unwrap();
        assert!(validate_target(&job, &outside, &helper).is_err());
    }
}

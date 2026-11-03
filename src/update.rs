//! An isolated, signed side-by-side update channel. Never installs Python release assets.
use crate::model::State;
use ed25519_dalek::{Signature, VerifyingKey};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
const ROOT: &str = "https://github.com/ZGMFX01A/Wireless-Device-Battery-Monitor/releases";
pub(crate) const KEY: Option<&str> = option_env!("GPUI_UPDATE_PUBLIC_KEY");
const MAX: u64 = 96 * 1024 * 1024;
#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    Current,
    Available(String),
    Downloading(u64, u64),
    Ready(String),
    Cancelled,
    Error(String),
}
#[derive(Serialize, Deserialize)]
pub struct Envelope {
    pub manifest: String,
    pub signature: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub size: u64,
    pub sha256: String,
    pub target: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub notes_en: String,
}
pub(crate) fn validate(e: &Envelope, key: &str, current: &str) -> Result<Manifest, String> {
    let pk: [u8; 32] = hex::decode(key)
        .map_err(|_| "invalid release key")?
        .try_into()
        .map_err(|_| "invalid release key length")?;
    let sig = Signature::from_slice(
        &hex::decode(&e.signature).map_err(|_| "invalid signature encoding")?,
    )
    .map_err(|_| "invalid signature size")?;
    VerifyingKey::from_bytes(&pk)
        .map_err(|_| "invalid key")?
        .verify_strict(e.manifest.as_bytes(), &sig)
        .map_err(|_| "invalid update signature")?;
    let m: Manifest = serde_json::from_str(&e.manifest).map_err(|_| "invalid signed manifest")?;
    let v = Version::parse(&m.version).map_err(|_| "invalid version")?;
    let base = Version::parse(current).map_err(|_| "invalid local version")?;
    if v < base
        || m.target != "x86_64-pc-windows-msvc"
        || m.size == 0
        || m.size > MAX
        || hex::decode(&m.sha256).map_or(true, |b| b.len() != 32)
        || !v.pre.is_empty()
        || !v.build.is_empty()
    {
        return Err("incompatible or invalid release".into());
    }
    Ok(m)
}
fn verify_bytes(
    mut r: impl Read,
    mut w: impl Write,
    m: &Manifest,
    cancel: &AtomicBool,
    progress: impl Fn(u64),
) -> Result<(), String> {
    let mut h = Sha256::new();
    let mut n = 0;
    let mut b = [0u8; 65536];
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        let a = r.read(&mut b).map_err(|e| e.to_string())?;
        if a == 0 {
            break;
        }
        n += a as u64;
        if n > m.size {
            return Err("oversized update".into());
        }
        h.update(&b[..a]);
        w.write_all(&b[..a]).map_err(|e| e.to_string())?;
        progress(n)
    }
    if n != m.size || !hex::encode(h.finalize()).eq_ignore_ascii_case(&m.sha256) {
        return Err("update hash or size mismatch".into());
    }
    Ok(())
}
fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .https_only(true)
        .user_agent("mouse-battery-gpui")
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(|e| e.to_string())
}
fn base() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .map(|p| {
            PathBuf::from(p)
                .join("WirelessDeviceBatteryMonitor-GPUI")
                .join("updates")
        })
        .ok_or("LOCALAPPDATA unavailable".into())
}
pub(crate) fn verified(p: &Path, m: &Manifest) -> Result<(), String> {
    verify_bytes(
        fs::File::open(p).map_err(|e| e.to_string())?,
        std::io::sink(),
        m,
        &AtomicBool::new(false),
        |_| {},
    )
}
/// Verified decoded metadata together with the signed bytes used by the independent swap helper.
type SignedManifestCache = Arc<Mutex<Option<(Manifest, Vec<u8>)>>>;
pub struct Updates {
    state: Arc<Mutex<State>>,
    busy: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    manifest: SignedManifestCache,
    staged: Arc<Mutex<Option<(PathBuf, Manifest)>>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    worker: Option<thread::JoinHandle<()>>,
    demo: bool,
}
impl Updates {
    pub fn new(
        state: Arc<Mutex<State>>,
        demo: bool,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            state,
            demo,
            busy: Default::default(),
            cancel: Default::default(),
            manifest: Default::default(),
            staged: Default::default(),
            wake: Arc::new(wake),
            worker: None,
        }
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release)
    }
    pub fn run(&mut self, download: bool) {
        if self.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        self.cancel.store(false, Ordering::Release);
        let (st, busy, c, m, ready, w, demo) = (
            self.state.clone(),
            self.busy.clone(),
            self.cancel.clone(),
            self.manifest.clone(),
            self.staged.clone(),
            self.wake.clone(),
            self.demo,
        );
        self.worker = Some(thread::spawn(move || {
            let post = |s| {
                st.lock().unwrap().update = s;
                w()
            };
            let result = (|| {
                if demo {
                    return Err("预览模式不访问更新服务 / Preview is offline".into());
                }
                let key=KEY.ok_or("此开发版未配置 GPUI 发布公钥，不会安装旧版更新。 / No GPUI release key configured.")?;
                let client = client()?;
                if !download {
                    post(Status::Checking);
                    let mut data = Vec::new();
                    client
                        .get(format!("{ROOT}/latest/download/gpui-windows.json"))
                        .send()
                        .and_then(|r| r.error_for_status())
                        .map_err(|e| e.to_string())?
                        .take(65537)
                        .read_to_end(&mut data)
                        .map_err(|e| e.to_string())?;
                    if data.len() > 65536 {
                        return Err("oversized manifest".into());
                    }
                    let env: Envelope = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
                    let v = validate(&env, key, env!("CARGO_PKG_VERSION"))?;
                    if v.version == env!("CARGO_PKG_VERSION") {
                        return Ok(Status::Current);
                    }
                    {
                        let mut state = st.lock().unwrap();
                        state.release_size = v.size;
                        state.release_notes =
                            if crate::view::english(&state.prefs) && !v.notes_en.is_empty() {
                                v.notes_en.clone()
                            } else {
                                v.notes.clone()
                            };
                        state.bytes_per_sec = 0;
                    }
                    let version = v.version.clone();
                    *m.lock().unwrap() = Some((v, data));
                    return Ok(Status::Available(version));
                }
                let (man, envelope) = m
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or("check for a signed release first")?;
                let dir = base()?.join(&man.version);
                fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let exe = dir.join("mouse-battery-gpui.exe");
                if !exe.exists() {
                    let mut temp =
                        tempfile::NamedTempFile::new_in(&dir).map_err(|e| e.to_string())?;
                    let response = client
                        .get(format!(
                            "{ROOT}/download/gpui-v{}/mouse-battery-gpui.exe",
                            man.version
                        ))
                        .send()
                        .and_then(|r| r.error_for_status())
                        .map_err(|e| e.to_string())?;
                    let last = std::cell::Cell::new(0);
                    let started = std::time::Instant::now();
                    verify_bytes(response, &mut temp, &man, &c, |n| {
                        if n - last.get() >= 262144 || n == man.size {
                            st.lock().unwrap().bytes_per_sec =
                                (n as f64 / started.elapsed().as_secs_f64().max(0.001)) as u64;
                            post(Status::Downloading(n, man.size));
                            last.set(n);
                        }
                    })?;
                    temp.as_file().sync_all().map_err(|e| e.to_string())?;
                    temp.persist_noclobber(&exe)
                        .map_err(|e| e.error.to_string())?;
                }
                verified(&exe, &man)?;
                let mut tmp = tempfile::NamedTempFile::new_in(&dir).map_err(|e| e.to_string())?;
                tmp.write_all(&envelope)
                    .and_then(|_| tmp.as_file().sync_all())
                    .map_err(|e| e.to_string())?;
                tmp.persist(dir.join("signed.json"))
                    .map_err(|e| e.error.to_string())?;
                let v = man.version.clone();
                *ready.lock().unwrap() = Some((exe, man));
                Ok(Status::Ready(v))
            })();
            let final_status = if c.load(Ordering::Acquire) {
                Status::Cancelled
            } else {
                result.unwrap_or_else(Status::Error)
            };
            busy.store(false, Ordering::Release);
            post(final_status);
        }));
    }
    /// User can inspect/start the verified side-by-side candidate after exiting. Never run an arbitrary URL or overwrite an executable.
    pub fn install_input(&self) -> Result<(PathBuf, Manifest), String> {
        if self.demo || KEY.is_none() || self.busy.load(Ordering::Acquire) {
            return Err("No installable signed build".into());
        }
        self.staged
            .lock()
            .unwrap()
            .clone()
            .ok_or("No verified GPUI update".into())
    }
    pub fn open_dir(&self) -> Result<(), String> {
        let (p, m) = self
            .staged
            .lock()
            .unwrap()
            .clone()
            .ok_or("no verified update")?;
        verified(&p, &m)?;
        std::process::Command::new(
            std::env::var_os("SYSTEMROOT")
                .map(PathBuf::from)
                .ok_or("SYSTEMROOT missing")?
                .join("explorer.exe"),
        )
        .arg(p.parent().ok_or("invalid update folder")?)
        .spawn()
        .map_err(|e| e.to_string())?;
        Ok(())
    }
}
impl Drop for Updates {
    fn drop(&mut self) {
        self.cancel(); /* No install transaction is in progress; unfinished downloads are unique temp files, never executable candidates. */
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    fn signed() -> (Envelope, String, Manifest) {
        let k = SigningKey::from_bytes(&[11u8; 32]);
        let m = Manifest {
            target: "x86_64-pc-windows-msvc".into(),
            notes: String::new(),
            notes_en: String::new(),
            version: "1.2.3".into(),
            size: 3,
            sha256: hex::encode(Sha256::digest(b"abc")),
        };
        let s = serde_json::to_string(&m).unwrap();
        (
            Envelope {
                signature: hex::encode(k.sign(s.as_bytes()).to_bytes()),
                manifest: s,
            },
            hex::encode(k.verifying_key().as_bytes()),
            m,
        )
    }
    #[test]
    fn verify_and_reject_tamper() {
        let (mut e, k, _) = signed();
        assert!(validate(&e, &k, "0.1.0").is_ok());
        assert!(validate(&e, &k, "2.0.0").is_err());
        e.manifest.push(' ');
        assert!(validate(&e, &k, "0.1.0").is_err());
    }
    #[test]
    fn transfers() {
        let (_, _, m) = signed();
        for (b, ok) in [
            (&b"abc"[..], true),
            (&b"ab"[..], false),
            (&b"xyz"[..], false),
            (&b"abcd"[..], false),
        ] {
            assert_eq!(
                verify_bytes(b, std::io::sink(), &m, &AtomicBool::new(false), |_| {}).is_ok(),
                ok
            )
        }
        assert!(
            verify_bytes(
                &b"abc"[..],
                std::io::sink(),
                &m,
                &AtomicBool::new(true),
                |_| {}
            )
            .is_err()
        )
    }
}

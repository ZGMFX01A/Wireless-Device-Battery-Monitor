//! Signed portable-EXE updates. Transport, scheduling and file replacement are separate.
mod network;
pub mod schedule;
use crate::model::State;
use ed25519_dalek::{Signature, VerifyingKey};
pub use network::DEFAULT_MIRROR;
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
};
const ROOT: &str = "https://github.com/ZGMFX01A/Wireless-Device-Battery-Monitor/releases";
const LATEST: &str =
    "https://api.github.com/repos/ZGMFX01A/Wireless-Device-Battery-Monitor/releases/latest";
pub(crate) const KEY: Option<&str> = option_env!("GPUI_UPDATE_PUBLIC_KEY");
fn asset_url(version: &str) -> String {
    format!("{ROOT}/download/v{version}/WirelessDeviceBatteryMonitor-v{version}.exe")
}

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
fn release_envelope(data: &[u8]) -> Result<Vec<u8>, String> {
    #[derive(Deserialize)]
    struct Release {
        body: String,
    }
    let release: Release = serde_json::from_slice(data).map_err(|e| e.to_string())?;
    let body = release.body.replace("\r\n", "\n");
    let (_, content) = body
        .split_once("<!-- gpui-update-manifest\n")
        .ok_or("release has no signed update metadata")?;
    let (envelope, _) = content
        .split_once("\n-->")
        .ok_or("incomplete signed update metadata")?;
    if envelope.len() > 65536 {
        return Err("oversized manifest".into());
    }
    Ok(envelope.as_bytes().to_vec())
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
type SignedManifestCache = Arc<Mutex<Option<(Manifest, Vec<u8>, usize)>>>;

fn signed_release(data: &[u8], key: &str, current: &str) -> Result<(Manifest, Vec<u8>), String> {
    let envelope = release_envelope(data)?;
    let e: Envelope = serde_json::from_slice(&envelope).map_err(|e| e.to_string())?;
    let manifest = validate(&e, key, current)?;
    let release: serde_json::Value = serde_json::from_slice(data).map_err(|e| e.to_string())?;
    if release["tag_name"].as_str() != Some(&format!("v{}", manifest.version))
        || release["draft"].as_bool() == Some(true)
        || release["prerelease"].as_bool() == Some(true)
    {
        return Err("Release identity does not match its signed metadata".into());
    }
    Ok((manifest, envelope))
}

pub(crate) fn record_failed(man: &Manifest) -> Result<(), String> {
    let dir = base()?.join(&man.version);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(
        dir.join(format!("failed-{}", man.sha256.to_ascii_lowercase())),
        b"Launch/replace failed; manual retry allowed",
    )
    .map_err(|e| e.to_string())
}
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
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
        self.staged.lock().unwrap().take();
        self.state.lock().unwrap().update = Status::Cancelled;
        (self.wake)();
    }
    pub fn run(&mut self, download: bool) {
        self.start(download, false);
    }
    pub fn automatic_check(&mut self) {
        self.start(false, true);
    }
    fn start(&mut self, download: bool, automatic: bool) {
        if self.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        self.cancel.store(false, Ordering::Release);
        self.staged.lock().unwrap().take();
        if !download {
            self.manifest.lock().unwrap().take();
        }
        let mirrors = {
            let mut state = self.state.lock().unwrap();
            state.bytes_per_sec = 0;
            state.update = if download {
                Status::Downloading(0, state.release_size)
            } else {
                Status::Checking
            };
            if !download {
                state.release_notes.clear();
                state.release_size = 0;
            }
            state.prefs.update_mirrors.clone()
        };
        (self.wake)();
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
            let result = (|| -> Result<Status, String> {
                if demo {
                    return Err("Preview is offline".into());
                }
                let key = KEY.ok_or("Automatic updates are unavailable in this build; download from the releases page")?;
                let transport = network::Transport::new(&mirrors)?;
                network::run(async {
                    if !download {
                        let ((man, envelope), route) = transport
                            .metadata(LATEST, &c, |data| {
                                signed_release(data, key, env!("APP_VERSION"))
                            })
                            .await?;
                        if man.version == env!("APP_VERSION") {
                            return Ok(Status::Current);
                        }
                        if automatic
                            && base()?
                                .join(&man.version)
                                .join(format!("failed-{}", man.sha256.to_ascii_lowercase()))
                                .is_file()
                        {
                            return Err("This update previously failed to start; automatic installation paused. Manual retry is available.".into());
                        }
                        {
                            let mut state = st.lock().unwrap();
                            state.release_size = man.size;
                            state.release_notes =
                                if crate::view::english(&state.prefs) && !man.notes_en.is_empty() {
                                    man.notes_en.clone()
                                } else {
                                    man.notes.clone()
                                };
                        }
                        let version = man.version.clone();
                        *m.lock().unwrap() = Some((man, envelope, route));
                        return Ok(Status::Available(version));
                    }
                    let (man, envelope, preferred) = m
                        .lock()
                        .unwrap()
                        .clone()
                        .ok_or("check for a signed release first")?;
                    let dir = base()?.join(&man.version);
                    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                    let exe = dir.join("mouse-battery-gpui.exe");
                    if exe.exists() && verified(&exe, &man).is_err() {
                        let quarantine = tempfile::Builder::new()
                            .prefix("rejected-")
                            .tempdir_in(&dir)
                            .map_err(|e| e.to_string())?;
                        fs::rename(&exe, quarantine.path().join("candidate.bin"))
                            .map_err(|e| e.to_string())?;
                        let _ = quarantine.keep();
                    }
                    if c.load(Ordering::Acquire) {
                        return Err("cancelled".into());
                    }
                    if !exe.exists() {
                        let part = dir.join(format!("{}.part", man.sha256.to_ascii_lowercase()));
                        let previous = std::cell::Cell::new(part.metadata().map_or(0, |m| m.len()));
                        let received = std::cell::Cell::new(0u64);
                        let started = std::time::Instant::now();
                        transport
                            .download(
                                &asset_url(&man.version),
                                preferred,
                                &part,
                                network::Expected {
                                    size: man.size,
                                    sha256: &man.sha256,
                                },
                                &c,
                                |n| {
                                    received.set(
                                        received
                                            .get()
                                            .saturating_add(n.saturating_sub(previous.replace(n))),
                                    );
                                    let mut state = st.lock().unwrap();
                                    state.bytes_per_sec = (received.get() as f64
                                        / started.elapsed().as_secs_f64().max(0.001))
                                        as u64;
                                    if !c.load(Ordering::Acquire) {
                                        state.update = Status::Downloading(n, man.size);
                                    }
                                    drop(state);
                                    w();
                                },
                            )
                            .await?;
                        if c.load(Ordering::Acquire) {
                            return Err("cancelled".into());
                        }
                        verified(&part, &man)?;
                        fs::rename(&part, &exe).map_err(|e| e.to_string())?;
                    }
                    verified(&exe, &man)?;
                    let mut tmp =
                        tempfile::NamedTempFile::new_in(&dir).map_err(|e| e.to_string())?;
                    tmp.write_all(&envelope)
                        .and_then(|_| tmp.as_file().sync_all())
                        .map_err(|e| e.to_string())?;
                    tmp.persist(dir.join("signed.json"))
                        .map_err(|e| e.error.to_string())?;
                    let version = man.version.clone();
                    *ready.lock().unwrap() = Some((exe, man));
                    Ok(Status::Ready(version))
                })
            })();
            {
                let mut state = st.lock().unwrap();
                state.update = if c.load(Ordering::Acquire) {
                    Status::Cancelled
                } else {
                    result.unwrap_or_else(Status::Error)
                };
                if !matches!(state.update, Status::Ready(_)) {
                    ready.lock().unwrap().take();
                }
                busy.store(false, Ordering::Release);
            }
            w();
        }));
    }
    /// User can inspect/start the verified side-by-side candidate after exiting. Never run an arbitrary URL or overwrite an executable.
    pub fn install_input(&self) -> Result<(PathBuf, Manifest), String> {
        if self.demo
            || KEY.is_none()
            || self.busy.load(Ordering::Acquire)
            || self.cancel.load(Ordering::Acquire)
            || !matches!(self.state.lock().unwrap().update, Status::Ready(_))
        {
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
    fn release_body_preserves_the_signed_envelope() {
        let (envelope, key, expected) = signed();
        let body = format!(
            "User release notes\n\n<!-- gpui-update-manifest\n{}\n-->\n",
            serde_json::to_string(&envelope).unwrap()
        );
        let response = serde_json::to_vec(&serde_json::json!({ "body": body })).unwrap();
        let data = release_envelope(&response).unwrap();
        let mut decoded: Envelope = serde_json::from_slice(&data).unwrap();
        assert_eq!(
            validate(&decoded, &key, "0.1.0").unwrap().version,
            expected.version
        );
        decoded.manifest.push(' ');
        assert!(validate(&decoded, &key, "0.1.0").is_err());
    }
    #[test]
    fn release_body_requires_complete_bounded_metadata() {
        for body in [
            "Release notes only".to_string(),
            "<!-- gpui-update-manifest\n{}".to_string(),
            format!("<!-- gpui-update-manifest\n{}\n-->", "x".repeat(65537)),
        ] {
            let response = serde_json::to_vec(&serde_json::json!({ "body": body })).unwrap();
            assert!(release_envelope(&response).is_err());
        }
    }
    #[test]
    fn release_tag_and_signature_must_describe_the_same_stable_release() {
        let (envelope, key, _) = signed();
        let body = format!(
            "Notes\r\n<!-- gpui-update-manifest\r\n{}\r\n-->\r\n",
            serde_json::to_string(&envelope).unwrap()
        );
        let mut release =
            serde_json::json!({"body":body,"tag_name":"v1.2.3","draft":false,"prerelease":false});
        assert!(signed_release(&serde_json::to_vec(&release).unwrap(), &key, "1.0.0").is_ok());
        release["tag_name"] = "v1.2.4".into();
        assert!(signed_release(&serde_json::to_vec(&release).unwrap(), &key, "1.0.0").is_err());
        release["tag_name"] = "v1.2.3".into();
        release["prerelease"] = true.into();
        assert!(signed_release(&serde_json::to_vec(&release).unwrap(), &key, "1.0.0").is_err());
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

#[cfg(test)]
mod formal_release_tests {
    use super::*;
    #[test]
    fn asset_contract_matches_the_public_release_filename() {
        assert_eq!(
            asset_url("2.8.1"),
            format!("{ROOT}/download/v2.8.1/WirelessDeviceBatteryMonitor-v2.8.1.exe")
        );
    }
    #[test]
    fn cancel_clears_previous_installable_candidate() {
        let state = Arc::new(Mutex::new(State::demo()));
        let u = Updates::new(state, true, || {});
        *u.staged.lock().unwrap() = Some((
            PathBuf::from("not-run.exe"),
            Manifest {
                version: "1.0.0".into(),
                size: 1,
                sha256: "00".repeat(32),
                target: "x86_64-pc-windows-msvc".into(),
                notes: String::new(),
                notes_en: String::new(),
            },
        ));
        u.cancel();
        assert!(u.staged.lock().unwrap().is_none());
        assert!(u.install_input().is_err());
    }
}

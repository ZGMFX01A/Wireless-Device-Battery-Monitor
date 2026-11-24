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
const RELEASE_BY_TAG_PREFIX: &str =
    "https://api.github.com/repos/ZGMFX01A/Wireless-Device-Battery-Monitor/releases/tags/v";
const NOTES_FILE_LIMIT: u64 = 256 * 1024;
pub(crate) const KEY: Option<&str> = option_env!("GPUI_UPDATE_PUBLIC_KEY");
fn asset_url(version: &str) -> String {
    format!("{ROOT}/download/v{version}/WirelessDeviceBatteryMonitor-v{version}.exe")
}

const MAX: u64 = 96 * 1024 * 1024;
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Failure {
    Check,
    Download,
    Install,
    Handoff,
    Preview,
    Unavailable,
    AutomaticInstallPaused,
}
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
    Error(Failure),
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
/// Verified decoded metadata together with the signed bytes and parsed bilingual notes.
type SignedManifestCache = Arc<Mutex<Option<(Manifest, Vec<u8>, usize, String, String)>>>;

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

fn is_placeholder_notes(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.is_empty()
        || trimmed == "详情请查看该版本的发布说明。"
        || trimmed == "See the release notes for details."
}

fn clean_notes_section(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    let mut start = 0;
    let mut end = lines.len();
    while start < end && (lines[start].trim().is_empty() || lines[start].trim() == "---") {
        start += 1;
    }
    while end > start && (lines[end - 1].trim().is_empty() || lines[end - 1].trim() == "---") {
        end -= 1;
    }
    lines[start..end].join("\n")
}

fn strip_manifest_comment(body: &str) -> String {
    let normalized = body.replace("\r\n", "\n");
    if let Some((before, rest)) = normalized.split_once("<!-- gpui-update-manifest\n")
        && let Some((_, after)) = rest.split_once("\n-->")
    {
        format!("{before}{after}")
    } else {
        normalized
    }
}

/// 从 GitHub Release 正文中解析中英双语发布说明，剥离更新清单注释并处理缺省回退。
pub(crate) fn extract_release_notes(
    body: &str,
    fallback_zh: &str,
    fallback_en: &str,
) -> (String, String) {
    let stripped = strip_manifest_comment(body);
    let mut zh_buf = Vec::new();
    let mut en_buf = Vec::new();
    enum LangSection {
        None,
        Zh,
        En,
    }
    let mut current = LangSection::None;
    let mut saw_marker = false;
    for line in stripped.lines() {
        let trimmed = line.trim();
        if trimmed == "<!-- lang:zh -->" {
            current = LangSection::Zh;
            saw_marker = true;
            continue;
        }
        if trimmed == "<!-- lang:en -->" {
            current = LangSection::En;
            saw_marker = true;
            continue;
        }
        match current {
            LangSection::Zh => zh_buf.push(line),
            LangSection::En => en_buf.push(line),
            LangSection::None => {}
        }
    }
    let mut zh = if saw_marker {
        clean_notes_section(&zh_buf.join("\n"))
    } else {
        clean_notes_section(&stripped)
    };
    let mut en = if saw_marker {
        clean_notes_section(&en_buf.join("\n"))
    } else {
        zh.clone()
    };
    if is_placeholder_notes(&zh) {
        zh.clear();
    }
    if is_placeholder_notes(&en) {
        en.clear();
    }
    if zh.is_empty() && !is_placeholder_notes(fallback_zh) {
        zh = clean_notes_section(fallback_zh);
    }
    if en.is_empty() && !is_placeholder_notes(fallback_en) {
        en = clean_notes_section(fallback_en);
    }
    if zh.is_empty() && !en.is_empty() {
        zh = en.clone();
    }
    if en.is_empty() && !zh.is_empty() {
        en = zh.clone();
    }
    (zh, en)
}

fn signed_release_with_notes(
    data: &[u8],
    key: &str,
    current: &str,
) -> Result<(Manifest, Vec<u8>, String, String), String> {
    let (manifest, envelope) = signed_release(data, key, current)?;
    let release: serde_json::Value = serde_json::from_slice(data).map_err(|e| e.to_string())?;
    let body = release["body"].as_str().unwrap_or("");
    let (notes_zh, notes_en) = extract_release_notes(body, &manifest.notes, &manifest.notes_en);
    Ok((manifest, envelope, notes_zh, notes_en))
}

fn release_notes_without_key(
    data: &[u8],
    expected_version: &str,
) -> Result<crate::model::InstalledReleaseNotes, String> {
    let release: serde_json::Value = serde_json::from_slice(data).map_err(|e| e.to_string())?;
    if release["tag_name"].as_str() != Some(&format!("v{expected_version}"))
        || release["draft"].as_bool() == Some(true)
        || release["prerelease"].as_bool() == Some(true)
    {
        return Err("Release identity does not match expected version".into());
    }
    let body = release["body"].as_str().unwrap_or("");
    let (notes_zh, notes_en) = extract_release_notes(body, "", "");
    Ok(crate::model::InstalledReleaseNotes {
        version: expected_version.to_string(),
        notes_zh,
        notes_en,
    })
}

/// 比较两个语义化版本号，仅当 `current` 严格大于 `previous` 时返回 `true`。
pub(crate) fn is_newer_version(current: &str, previous: &str) -> bool {
    match (Version::parse(current), Version::parse(previous)) {
        (Ok(cur), Ok(prev)) => cur > prev,
        _ => false,
    }
}

/// 检查更新缓存目录中是否存在指定版本的升级落盘记录。
pub(crate) fn has_installed_update_record(version: &str) -> bool {
    let Ok(root) = base() else {
        return false;
    };
    let dir = root.join(version);
    dir.join("notes.json").is_file() || dir.join("signed.json").is_file()
}

fn write_notes_file_to_dir(
    dir: &Path,
    notes: &crate::model::InstalledReleaseNotes,
) -> Result<(), String> {
    if !notes.has_content() {
        return Ok(());
    }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut tmp, notes).map_err(|e| e.to_string())?;
    tmp.as_file().sync_all().map_err(|e| e.to_string())?;
    tmp.persist(dir.join("notes.json"))
        .map_err(|e| e.error.to_string())?;
    Ok(())
}

fn load_installed_release_notes_from_dir(
    dir: &Path,
    version: &str,
) -> Option<crate::model::InstalledReleaseNotes> {
    let notes_path = dir.join("notes.json");
    if let Ok(meta) = fs::metadata(&notes_path)
        && meta.len() <= NOTES_FILE_LIMIT
        && let Ok(bytes) = fs::read(&notes_path)
        && let Ok(notes) = serde_json::from_slice::<crate::model::InstalledReleaseNotes>(&bytes)
        && notes.version == version
        && notes.has_content()
    {
        return Some(notes);
    }
    let signed_path = dir.join("signed.json");
    let meta = fs::metadata(&signed_path).ok()?;
    if meta.len() > 65536 {
        return None;
    }
    let envelope: Envelope = serde_json::from_slice(&fs::read(&signed_path).ok()?).ok()?;
    let manifest: Manifest = serde_json::from_str(&envelope.manifest).ok()?;
    if manifest.version != version {
        return None;
    }
    let (notes_zh, notes_en) = extract_release_notes("", &manifest.notes, &manifest.notes_en);
    let notes = crate::model::InstalledReleaseNotes {
        version: version.to_string(),
        notes_zh,
        notes_en,
    };
    notes.has_content().then_some(notes)
}

/// 从本地更新缓存加载指定版本的双语更新说明（优先 `notes.json`，其次过滤占位符后的 `signed.json`）。
pub(crate) fn load_installed_release_notes(
    version: &str,
) -> Option<crate::model::InstalledReleaseNotes> {
    let dir = base().ok()?.join(version);
    load_installed_release_notes_from_dir(&dir, version)
}

/// 在后台静默补拉指定已安装版本的 GitHub Release 说明（用于本地无有效缓存或从旧版首次升级场景）。
pub(crate) fn fetch_installed_release_notes_async(
    version: String,
    state: Arc<Mutex<State>>,
    wake: impl Fn() + Send + Sync + 'static,
) {
    let mirrors = state.lock().unwrap().prefs.update_mirrors.clone();
    thread::spawn(move || {
        let Ok(transport) = network::Transport::new(&mirrors) else {
            return;
        };
        let url = format!("{RELEASE_BY_TAG_PREFIX}{version}");
        let cancel = AtomicBool::new(false);
        let version_ref = version.as_str();
        let fetched = network::run(async {
            let (notes, _) = transport
                .metadata(&url, &cancel, |data| {
                    if let Some(key) = KEY {
                        let (man, _, notes_zh, notes_en) =
                            signed_release_with_notes(data, key, version_ref)?;
                        if man.version != version_ref {
                            return Err("release version mismatch".into());
                        }
                        Ok(crate::model::InstalledReleaseNotes {
                            version: man.version,
                            notes_zh,
                            notes_en,
                        })
                    } else {
                        release_notes_without_key(data, version_ref)
                    }
                })
                .await?;
            Ok(notes)
        });
        let Ok(notes) = fetched else {
            // 静默补拉失败不影响正常使用，弹窗中仍提供手动跳转发布页按钮。
            return;
        };
        if !notes.has_content() {
            return;
        }
        if let Ok(dir) = base().map(|root| root.join(&version)) {
            let _ = write_notes_file_to_dir(&dir, &notes);
        }
        let mut updated = false;
        {
            let mut st = state.lock().unwrap();
            if st.prefs.last_seen_release_notes_version != version
                && st
                    .installed_release_notes
                    .as_ref()
                    .is_some_and(|existing| existing.version == version)
            {
                st.installed_release_notes = Some(notes);
                updated = true;
            }
        }
        if updated {
            wake();
        }
    });
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
                state.release_notes_en.clear();
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
                    return Ok(Status::Error(Failure::Preview));
                }
                let Some(key) = KEY else {
                    return Ok(Status::Error(Failure::Unavailable));
                };
                let transport = network::Transport::new(&mirrors)?;
                network::run(async {
                    if !download {
                        let ((man, envelope, notes_zh, notes_en), route) = transport
                            .metadata(LATEST, &c, |data| {
                                signed_release_with_notes(data, key, env!("APP_VERSION"))
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
                            return Ok(Status::Error(Failure::AutomaticInstallPaused));
                        }
                        {
                            let mut state = st.lock().unwrap();
                            state.release_size = man.size;
                            state.release_notes = notes_zh.clone();
                            state.release_notes_en = notes_en.clone();
                        }
                        let version = man.version.clone();
                        *m.lock().unwrap() = Some((man, envelope, route, notes_zh, notes_en));
                        return Ok(Status::Available(version));
                    }
                    let (man, envelope, preferred, notes_zh, notes_en) = m
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
                    let notes = crate::model::InstalledReleaseNotes {
                        version: man.version.clone(),
                        notes_zh,
                        notes_en,
                    };
                    if let Err(e) = write_notes_file_to_dir(&dir, &notes) {
                        eprintln!("Failed to save release notes: {e}");
                    }
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
                    result.unwrap_or_else(|e| {
                        eprintln!("Update failed: {e}");
                        Status::Error(if download {
                            Failure::Download
                        } else {
                            Failure::Check
                        })
                    })
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

    #[test]
    fn extract_release_notes_parses_bilingual_body_and_strips_manifest() {
        let body = concat!(
            "<!-- lang:zh -->\r\n\r\n",
            "## 新增功能\r\n\r\n",
            "- **兼容模式**：支持更多设备。\r\n\r\n",
            "---\r\n\r\n",
            "<!-- lang:en -->\r\n\r\n",
            "## New Features\r\n\r\n",
            "- **Compatible mode**: Supports more devices.\r\n\r\n",
            "<!-- gpui-update-manifest\r\n",
            "{\"manifest\":\"{}\",\"signature\":\"00\"}\r\n",
            "-->\r\n"
        );
        let (zh, en) = extract_release_notes(
            body,
            "详情请查看该版本的发布说明。",
            "See the release notes for details.",
        );
        assert!(zh.contains("## 新增功能"));
        assert!(!zh.contains("New Features"));
        assert!(!zh.contains("gpui-update-manifest"));
        assert!(en.contains("## New Features"));
        assert!(!en.contains("新增功能"));
        assert!(!en.contains("gpui-update-manifest"));

        // 无语言标记时同时填充中英两份，且过滤占位符回退。
        let (single_zh, single_en) = extract_release_notes(
            "## Fixes\n- Fixed bug\n<!-- gpui-update-manifest\n{}\n-->\n",
            "详情请查看该版本的发布说明。",
            "See the release notes for details.",
        );
        assert_eq!(single_zh, "## Fixes\n- Fixed bug");
        assert_eq!(single_en, "## Fixes\n- Fixed bug");

        let (empty_zh, empty_en) = extract_release_notes(
            "",
            "详情请查看该版本的发布说明。",
            "See the release notes for details.",
        );
        assert!(empty_zh.is_empty());
        assert!(empty_en.is_empty());
    }

    #[test]
    fn installed_release_notes_round_trip_and_version_checks() {
        assert!(is_newer_version("3.1.1", "3.1.0"));
        assert!(!is_newer_version("3.1.0", "3.1.0"));
        assert!(!is_newer_version("3.0.9", "3.1.0"));

        let dir = tempfile::tempdir().unwrap();
        // signed.json 仅含占位符时不返回有效内容。
        let m = Manifest {
            version: "3.1.0".into(),
            size: 3,
            sha256: "00".repeat(32),
            target: "x86_64-pc-windows-msvc".into(),
            notes: "详情请查看该版本的发布说明。".into(),
            notes_en: "See the release notes for details.".into(),
        };
        let envelope = Envelope {
            manifest: serde_json::to_string(&m).unwrap(),
            signature: "00".repeat(64),
        };
        fs::write(
            dir.path().join("signed.json"),
            serde_json::to_vec(&envelope).unwrap(),
        )
        .unwrap();
        assert!(load_installed_release_notes_from_dir(dir.path(), "3.1.0").is_none());

        // 写入 notes.json 后按精确版本读取，不匹配的版本拒绝加载。
        let notes = crate::model::InstalledReleaseNotes {
            version: "3.1.0".into(),
            notes_zh: "## 新增\n- 功能 A".into(),
            notes_en: "## Added\n- Feature A".into(),
        };
        write_notes_file_to_dir(dir.path(), &notes).unwrap();
        assert_eq!(
            load_installed_release_notes_from_dir(dir.path(), "3.1.0"),
            Some(notes)
        );
        assert!(load_installed_release_notes_from_dir(dir.path(), "3.2.0").is_none());
    }
}

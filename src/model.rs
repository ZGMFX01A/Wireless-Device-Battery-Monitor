// SPDX-FileCopyrightText: 2026 ZGMFX01A
// SPDX-License-Identifier: LicenseRef-Wireless-Device-Battery-Monitor
// See LICENSE in the repository root for the full terms.

use battery_protocol::{Charge, Descriptor, EngineMode, Kind, Reading, hid::DiscoveryWarning};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    path::{Path, PathBuf},
};
pub const POLL_INTERVALS: [u64; 7] = [5, 10, 15, 30, 60, 120, 300];
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 30;
/// 普通模式下，标记需要切回兼容模式才能识别的离线绑定。
pub const PROBED_COMPATIBLE_ONLY_ERROR: &str = "compatible-mode-only";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Priority {
    #[default]
    Mouse,
    Keyboard,
    Lowest,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Style {
    #[default]
    Capsule,
    Vertical,
    Number,
    Circle,
}
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    Auto,
    Light,
    Dark,
}
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    Auto,
    Zh,
    En,
    Ru,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeyboardConnection {
    pub endpoint_id: String,
    #[serde(default)]
    pub serial: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedDeviceIdentity {
    name: String,
    identity: battery_protocol::DeviceIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    generic_name: Option<battery_protocol::GenericName>,
    endpoint_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    probed_route: Option<battery_protocol::ProbedRoute>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Prefs {
    pub priority: Priority,
    pub style: Style,
    pub theme: Theme,
    pub lang: Language,
    pub glass: u8,
    /// 设备识别模式，旧偏好文件缺省为普通模式。
    pub engine_mode: EngineMode,
    /// 兼容模式下是否允许向同源方案商的未知 PID 设备发送只读探测命令（默认关闭）。
    #[serde(default)]
    pub probe_unknown_devices: bool,
    pub auto_update: bool,
    /// Empty means GitHub/system proxy only; defaults provide an automatic fallback.
    pub update_mirrors: Vec<String>,
    pub low: u8,
    pub poll_interval_secs: u64,
    pub keyboard: Option<String>,
    /// `keyboard` remains the logical key for names and settings; this is its current transport.
    pub keyboard_connection: Option<KeyboardConnection>,
    pub bluetooth: BTreeSet<String>,
    pub aliases: BTreeMap<String, String>,
    pub bound_names: BTreeMap<String, String>,
    pub bound_identity: BTreeMap<String, CachedDeviceIdentity>,
}
impl Default for Prefs {
    fn default() -> Self {
        Self {
            priority: Priority::Mouse,
            style: Default::default(),
            theme: Default::default(),
            lang: Default::default(),
            glass: 25,
            engine_mode: EngineMode::default(),
            probe_unknown_devices: false,
            auto_update: false,
            update_mirrors: vec![crate::update::DEFAULT_MIRROR.into()],
            low: 20,
            poll_interval_secs: DEFAULT_POLL_INTERVAL_SECS,
            keyboard: None,
            keyboard_connection: None,
            bluetooth: Default::default(),
            aliases: Default::default(),
            bound_names: Default::default(),
            bound_identity: Default::default(),
        }
    }
}
impl Prefs {
    pub fn keyboard_endpoint(&self) -> Option<&str> {
        self.keyboard.as_deref().map(|id| {
            self.keyboard_connection
                .as_ref()
                .map_or(id, |c| c.endpoint_id.as_str())
        })
    }
    pub fn keyboard_row(&self, mut d: Descriptor) -> Descriptor {
        if d.kind == Kind::Keyboard && self.keyboard_endpoint() == Some(d.id.as_str()) {
            d.id = self.keyboard.clone().unwrap();
        }
        d
    }
    pub fn poll_ms(&self) -> u64 {
        if POLL_INTERVALS.contains(&self.poll_interval_secs) {
            self.poll_interval_secs * 1000
        } else {
            DEFAULT_POLL_INTERVAL_SECS * 1000
        }
    }
    pub fn bound(&self, d: &Descriptor) -> bool {
        match d.kind {
            Kind::Mouse | Kind::Audio | Kind::Gamepad => true,
            Kind::Keyboard => {
                self.keyboard.as_ref() == Some(&d.id)
                    || self.keyboard_endpoint() == Some(d.id.as_str())
            }
            Kind::Bluetooth => self.bluetooth.contains(&d.id),
        }
    }
    fn name_key<'a>(&'a self, d: &'a Descriptor) -> &'a str {
        if d.kind == Kind::Keyboard && self.keyboard_endpoint() == Some(d.id.as_str()) {
            self.keyboard.as_deref().unwrap()
        } else {
            &d.id
        }
    }
    fn cached_exact_identity(&self, d: &Descriptor) -> Option<&CachedDeviceIdentity> {
        d.generic_name.as_ref()?;
        let key = self.name_key(d);
        if !self.bound(d) {
            return None;
        }
        let expected_endpoint = if d.kind == Kind::Keyboard {
            self.keyboard_endpoint().unwrap_or(&d.id)
        } else {
            &d.id
        };
        self.bound_identity.get(key).filter(|cached| {
            cached.identity.model_key.is_some()
                && cached.generic_name.is_none()
                && cached.endpoint_id == expected_endpoint
        })
    }
    pub fn name(&self, d: &Descriptor) -> String {
        self.aliases
            .get(self.name_key(d))
            .map(String::as_str)
            .or_else(|| {
                self.cached_exact_identity(d)
                    .map(|cached| cached.name.as_str())
            })
            .map_or_else(
                || crate::device_display::display_name(d, crate::view::locale(self)),
                str::to_owned,
            )
    }
    /// 在有效回包到达前，使用绑定端点已确认的型号身份和图标。
    pub fn retain_bound_identity(&self, d: &mut Descriptor) {
        if let Some(cached) = self.cached_exact_identity(d) {
            d.name.clone_from(&cached.name);
            d.identity = Some(cached.identity.clone());
            d.generic_name = None;
        }
    }
    fn uses_generic_name(&self, d: &Descriptor) -> bool {
        d.generic_name.is_some()
            && !self.aliases.contains_key(self.name_key(d))
            && self.cached_exact_identity(d).is_none()
    }
    fn identity_binding(&self, d: &Descriptor) -> Option<&str> {
        match d.kind {
            Kind::Keyboard if self.keyboard_endpoint() == Some(d.id.as_str()) => {
                self.keyboard.as_deref()
            }
            Kind::Bluetooth => self.bluetooth.get(&d.id).map(String::as_str),
            _ => None,
        }
    }
    pub fn remember_identity(&mut self, d: &Descriptor) -> bool {
        self.remember_identity_with_route(d, None)
    }
    pub fn remember_identity_with_route(
        &mut self,
        d: &Descriptor,
        probed_route: Option<battery_protocol::ProbedRoute>,
    ) -> bool {
        let Some(key) = self.identity_binding(d).map(str::to_owned) else {
            return false;
        };
        let Some(identity) = &d.identity else {
            return false;
        };
        if identity.model_key.is_none() && self.cached_exact_identity(d).is_some() {
            return false;
        }
        let snapshot = CachedDeviceIdentity {
            name: d.name.clone(),
            identity: identity.clone(),
            generic_name: d.generic_name.clone(),
            endpoint_id: d.id.clone(),
            probed_route,
        };
        if self.bound_identity.get(&key) == Some(&snapshot) {
            return false;
        }
        self.bound_names.insert(key.clone(), d.name.clone());
        self.bound_identity.insert(key, snapshot);
        true
    }
    pub fn bound_probed_routes(&self) -> Vec<(String, battery_protocol::ProbedRoute)> {
        self.bound_identity
            .iter()
            .filter_map(|(key, cached)| {
                let bound = self.bluetooth.contains(key)
                    || self.keyboard_endpoint() == Some(cached.endpoint_id.as_str())
                    || self.keyboard.as_ref() == Some(key);
                bound.then_some((cached.endpoint_id.clone(), cached.probed_route?))
            })
            .collect()
    }
    pub fn has_probed_only_binding(&self, id: &str) -> bool {
        self.bound_identity
            .get(id)
            .or_else(|| {
                (self.keyboard_endpoint() == Some(id))
                    .then(|| {
                        self.keyboard
                            .as_ref()
                            .and_then(|key| self.bound_identity.get(key))
                    })
                    .flatten()
            })
            .is_some_and(|cached| cached.probed_route.is_some())
    }
    pub fn unknown_probe_enabled(&self) -> bool {
        self.engine_mode == EngineMode::Compatible && self.probe_unknown_devices
    }
    pub fn keyboard_requires_compatible_mode(&self) -> bool {
        !self.unknown_probe_enabled()
            && self
                .keyboard
                .as_deref()
                .is_some_and(|id| self.has_probed_only_binding(id))
    }
    pub(crate) fn keyboard_serial(&self) -> Option<&str> {
        let endpoint = self.keyboard_endpoint()?;
        self.keyboard_connection
            .as_ref()
            .and_then(|c| c.serial.as_deref())
            .or_else(|| {
                endpoint
                    .split_once(":serial:")
                    .map(|(_, serial)| serial)
                    .filter(|serial| !serial.is_empty() && !serial.chars().all(|c| c == '0'))
            })
    }
    pub(crate) fn clear_replaced_keyboard_identity(&mut self, endpoint: &str) {
        if self.keyboard_endpoint() != Some(endpoint)
            && let Some(key) = &self.keyboard
        {
            self.bound_identity.remove(key);
        }
    }
    pub(crate) fn reconcile_replaced_keyboard_identity(&mut self, connection: &KeyboardConnection) {
        if self.keyboard_endpoint() == Some(connection.endpoint_id.as_str()) {
            return;
        }
        let same_serial = self
            .keyboard_serial()
            .is_some_and(|serial| connection.serial.as_deref() == Some(serial));
        if same_serial
            && let Some(key) = &self.keyboard
            && let Some(cached) = self.bound_identity.get_mut(key)
            && cached.probed_route.is_none()
        {
            cached.endpoint_id.clone_from(&connection.endpoint_id);
        } else {
            self.clear_replaced_keyboard_identity(&connection.endpoint_id);
        }
    }
    pub(crate) fn clear_probed_route(&mut self, d: &Descriptor) -> bool {
        let Some(key) = self.identity_binding(d).map(str::to_owned) else {
            return false;
        };
        self.bound_identity
            .get_mut(&key)
            .is_some_and(|cached| cached.probed_route.take().is_some())
    }
    pub fn clear_identity(&mut self, d: &Descriptor) -> bool {
        let Some(key) = self.identity_binding(d).map(str::to_owned) else {
            return false;
        };
        let had_snapshot = self.bound_identity.remove(&key).is_some();
        let changed_name = self.bound_names.get(&key) != Some(&d.name);
        if changed_name {
            self.bound_names.insert(key, d.name.clone());
        }
        had_snapshot || changed_name
    }
    pub fn preserve_missing(&self, rows: &mut Vec<Row>, bt: bool) {
        let ids: Vec<_> = if bt {
            self.bluetooth.iter().collect()
        } else {
            self.keyboard.iter().collect()
        };
        for id in ids {
            if !rows.iter().any(|r| &r.device.id == id) {
                let cached = self.bound_identity.get(id);
                rows.push(Row {
                    device: Descriptor {
                        id: id.clone(),
                        name: cached
                            .map(|d| d.name.clone())
                            .or_else(|| self.bound_names.get(id).cloned())
                            .unwrap_or_else(|| {
                                if bt {
                                    "Bluetooth".into()
                                } else {
                                    "Keyboard".into()
                                }
                            }),
                        kind: if bt { Kind::Bluetooth } else { Kind::Keyboard },
                        legacy_ids: vec![],
                        identity: cached.map(|d| d.identity.clone()),
                        generic_name: cached.and_then(|d| d.generic_name.clone()),
                    },
                    battery: None,
                    error: Some(
                        if !self.unknown_probe_enabled()
                            && cached.and_then(|c| c.probed_route).is_some()
                        {
                            PROBED_COMPATIBLE_ONLY_ERROR
                        } else {
                            "offline"
                        }
                        .into(),
                    ),
                    sampled: 0,
                    held: false,
                })
            }
        }
    }
    pub fn path() -> Result<PathBuf, String> {
        std::env::var_os("APPDATA")
            .map(|p| {
                PathBuf::from(p)
                    .join("WirelessDeviceBatteryMonitor-GPUI")
                    .join("preferences-v2.json")
            })
            .ok_or("APPDATA unavailable".into())
    }
    pub fn resolve(&mut self, list: &[Descriptor]) -> bool {
        // Carry forward 0.1.2 HID references only when exactly one current candidate
        // owns that old key. A model name is not physical identity.
        let mut remapped = false;
        if let Some(old) = self.keyboard.clone()
            && self.keyboard_connection.is_none()
            && let Some(d) = exact(list, std::slice::from_ref(&old), Some(Kind::Keyboard))
            && d.id != old
        {
            self.keyboard = Some(d.id.clone());
            let previous = self
                .bound_names
                .remove(&old)
                .unwrap_or_else(|| d.name.clone());
            self.bound_names.entry(d.id.clone()).or_insert(previous);
            if let Some(snapshot) = self.bound_identity.remove(&old) {
                self.bound_identity.entry(d.id.clone()).or_insert(snapshot);
            }
            remapped = true;
        }
        for (old, alias) in self.aliases.clone() {
            if old.starts_with("hid:")
                && !(self.keyboard_connection.is_some() && self.keyboard.as_ref() == Some(&old))
                && let Some(d) = exact(list, std::slice::from_ref(&old), None)
                && d.id != old
            {
                self.aliases.entry(d.id.clone()).or_insert(alias);
                self.aliases.remove(&old);
                remapped = true;
            }
        }
        remapped
    }
    pub fn load(p: &Path) -> Result<Self, String> {
        let read = || -> std::io::Result<Vec<u8>> {
            let mut bytes = Vec::new();
            std::fs::File::open(p)?
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > 4 * 1024 * 1024 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "preferences too large",
                ));
            }
            Ok(bytes)
        };
        match read() {
            Ok(b) => {
                let v: Self =
                    serde_json::from_slice(&b).map_err(|e| format!("Invalid preferences: {e}"))?;
                if v.glass > 100
                    || ![0, 10, 20, 30].contains(&v.low)
                    || !POLL_INTERVALS.contains(&v.poll_interval_secs)
                {
                    return Err("invalid preference range".into());
                }
                Ok(v)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Fork on first save; 0.1.2 must retain bindings in its original ID space.
                if p.file_name().and_then(|v| v.to_str()) == Some("preferences-v2.json") {
                    Self::load(&p.with_file_name("preferences.json"))
                } else {
                    Ok(Self::default())
                }
            }
            Err(e) => Err(e.to_string()),
        }
    }
    pub fn save(&self, p: &Path) -> Result<(), String> {
        let dir = p.parent().ok_or("invalid data path")?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let mut t = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
        t.write_all(&serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?)
            .and_then(|_| t.as_file().sync_all())
            .map_err(|e| e.to_string())?;
        t.persist(p).map_err(|e| e.error.to_string())?;
        Ok(())
    }
}
#[cfg(test)]
#[test]
fn cached_keyboard_identity_tracks_the_logical_binding_and_survives_offline_restart() {
    use battery_protocol::{
        BatteryCapability, ConnectionKind, DeviceCategory, DeviceIdentity, ModelSource,
    };
    let mut prefs = Prefs {
        keyboard: Some("logical".into()),
        keyboard_connection: Some(KeyboardConnection {
            endpoint_id: "current".into(),
            serial: Some("physical".into()),
        }),
        ..Default::default()
    };
    prefs
        .aliases
        .insert("logical".into(), "Desk keyboard".into());
    let device = Descriptor {
        id: "current".into(),
        name: "AULA MINI60HE MAX".into(),
        kind: Kind::Keyboard,
        legacy_ids: vec![],
        identity: Some(DeviceIdentity {
            model_key: Some("aula-mini60he-max".into()),
            category: DeviceCategory::Keyboard,
            connection: ConnectionKind::Rf24,
            battery: BatteryCapability::Present,
            model_source: Some(ModelSource::ProtocolIdentity),
        }),
        generic_name: None,
    };
    assert_eq!(prefs.name(&device), "Desk keyboard");
    assert!(prefs.remember_identity(&device));
    assert!(!prefs.remember_identity(&device));
    let mut prefs: Prefs = serde_json::from_slice(&serde_json::to_vec(&prefs).unwrap()).unwrap();
    let mut offline = Vec::new();
    prefs.preserve_missing(&mut offline, false);
    let row = &offline[0];
    assert_eq!(row.device.id, "logical");
    assert_eq!(row.device.name, device.name);
    assert_eq!(row.device.identity, device.identity);
    assert_eq!(prefs.name(&row.device), "Desk keyboard");
    assert!(row.battery.is_none());
    assert!(prefs.change(&Command::Unbind("logical".into()), &[]));
    assert!(prefs.bound_identity.is_empty());
}

#[cfg(test)]
#[test]
fn old_preferences_and_descriptors_default_missing_identity_fields() {
    let prefs: Prefs = serde_json::from_value(serde_json::json!({
        "keyboard": "old", "bound_names": {"old": "Previous name"}
    }))
    .unwrap();
    assert!(prefs.bound_identity.is_empty());
    let mut rows = Vec::new();
    prefs.preserve_missing(&mut rows, false);
    assert_eq!(rows[0].device.name, "Previous name");
    assert!(rows[0].device.identity.is_none());
    let old: Descriptor = serde_json::from_value(serde_json::json!({
        "id": "old", "name": "Previous name", "kind": "Keyboard"
    }))
    .unwrap();
    assert!(old.identity.is_none());
    assert!(old.generic_name.is_none());
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Row {
    pub device: Descriptor,
    pub battery: Option<Reading>,
    pub error: Option<String>,
    pub sampled: u64,
    /// The displayed value comes from an earlier successful sample.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub held: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceJob {
    pub id: u64,
    pub kind: Kind,
    pub bind: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Receipt {
    pub id: u64,
    pub result: Result<(), String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Notice {
    HidScanComplete,
    BluetoothScanComplete,
    DiscoveryWarnings(Vec<DiscoveryWarning>),
    HidScanFailed,
    BluetoothScanFailed,
    RefreshFailed,
    Suspended,
    SettingsSaveFailed,
    KeyboardReconnected,
    KeyboardReconnectFailed,
    PreviewHardwareUnavailable,
    AutostartStatusFailed,
    UpdateFailed(crate::update::Failure),
    UpdateFolderFailed,
    OperationFailed,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manual {
    pub id: u64,
    pub hid: bool,
    pub bt: bool,
    pub error: Option<Notice>,
}
impl Manual {
    pub fn completed(&self) -> bool {
        self.hid && self.bt
    }
    pub fn finish(&mut self, id: u64, bt: bool, error: Option<Notice>) {
        if self.id == id {
            if bt {
                self.bt = true
            } else {
                self.hid = true
            }
            if error.is_some() {
                self.error = error
            }
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct State {
    pub prefs: Prefs,
    pub rows: Vec<Row>,
    pub candidates: Vec<Descriptor>,
    /// 兼容模式下最近代际的脱敏探测诊断摘要。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub probe_diagnostics: Vec<battery_protocol::hid::probe::ProbeDiagnosticEntry>,
    pub manual: Manual,
    pub keyboard_reply: Option<Receipt>,
    pub bluetooth_reply: Option<Receipt>,
    pub release_notes: String,
    pub release_size: u64,
    pub bytes_per_sec: u64,
    pub note: Option<Notice>,
    pub demo: bool,
    pub autostart: bool,
    pub update: crate::update::Status,
}
impl State {
    fn numbered_generic_name<'a>(
        &self,
        device: &Descriptor,
        peers: impl Iterator<Item = &'a Descriptor>,
    ) -> String {
        let name = self.prefs.name(device);
        if self.prefs.engine_mode != EngineMode::Compatible || !self.prefs.uses_generic_name(device)
        {
            return name;
        }
        let previous = peers
            .filter(|other| {
                other.id < device.id
                    && self.prefs.uses_generic_name(other)
                    && self.prefs.name(other) == name
            })
            .count();
        if previous == 0 {
            name
        } else {
            format!("{name} {}", previous + 1)
        }
    }
    /// 以稳定设备 ID 为序，为重名的通用名称添加编号。
    pub fn row_name(&self, row: &Row) -> String {
        self.numbered_generic_name(
            &row.device,
            self.rows
                .iter()
                .map(|other| &other.device)
                .filter(|other| self.prefs.bound(other)),
        )
    }
    /// 在绑定弹窗中按同品类候选稳定编号，区分重名的通用设备。
    pub fn candidate_name(&self, device: &Descriptor) -> String {
        self.numbered_generic_name(
            device,
            self.candidates
                .iter()
                .filter(|other| other.kind == device.kind),
        )
    }

    /// Reconcile against the latest preferences, not a worker's pre-I/O snapshot.
    pub fn prune_unbound(&mut self) {
        self.rows.retain(|r| self.prefs.bound(&r.device));
    }
    /// Suppress only *painting* an invisible timestamp. Destructure exhaustively:
    /// adding a receipt or a new visible field must not silently drop it.
    pub fn same_except_sample_time(&self, o: &Self) -> bool {
        let State {
            prefs,
            rows,
            candidates,
            probe_diagnostics,
            manual,
            keyboard_reply,
            bluetooth_reply,
            release_notes,
            release_size,
            bytes_per_sec,
            note,
            demo,
            autostart,
            update,
        } = self;
        prefs == &o.prefs
            && candidates == &o.candidates
            && probe_diagnostics == &o.probe_diagnostics
            && manual == &o.manual
            && keyboard_reply == &o.keyboard_reply
            && bluetooth_reply == &o.bluetooth_reply
            && release_notes == &o.release_notes
            && release_size == &o.release_size
            && bytes_per_sec == &o.bytes_per_sec
            && note == &o.note
            && demo == &o.demo
            && autostart == &o.autostart
            && update == &o.update
            && rows.len() == o.rows.len()
            && rows.iter().zip(&o.rows).all(|(r, b)| {
                let Row {
                    device,
                    battery,
                    error,
                    held,
                    sampled: _,
                } = r;
                device == &b.device && battery == &b.battery && error == &b.error && held == &b.held
            })
    }
    pub fn reply(&self, k: Kind) -> Option<&Receipt> {
        if k == Kind::Bluetooth {
            self.bluetooth_reply.as_ref()
        } else {
            self.keyboard_reply.as_ref()
        }
    }
    pub fn finish(&mut self, j: &DeviceJob, result: Result<(), String>) {
        if let Err(e) = &result {
            eprintln!("{:?} device request failed: {e}", j.kind);
        }
        let dst = if j.kind == Kind::Bluetooth {
            &mut self.bluetooth_reply
        } else {
            &mut self.keyboard_reply
        };
        *dst = Some(Receipt { id: j.id, result });
    }
    pub fn demo_reconcile(&mut self) {
        if !self.demo {
            return;
        }
        self.rows.retain(|r| self.prefs.bound(&r.device));
        for d in self.candidates.clone() {
            if self.prefs.bound(&d) && !self.rows.iter().any(|r| r.device.id == d.id) {
                let source = if d.kind == Kind::Bluetooth {
                    battery_protocol::Source::Ble
                } else {
                    battery_protocol::Source::Weikav
                };
                self.rows.push(Row {
                    device: d,
                    battery: Reading::new(35, Charge::Charging, source),
                    error: None,
                    sampled: 0,
                    held: false,
                })
            }
        }
    }
    pub fn demo_request(&mut self, j: &DeviceJob) {
        let list = self.candidates.clone();
        let res = if let Some(id) = &j.bind {
            if list.iter().any(|d| d.id == *id && d.kind == j.kind)
                && self.prefs.change(&Command::Bind(id.clone()), &list)
            {
                Ok(())
            } else {
                Err("Unknown preview device".into())
            }
        } else {
            Ok(())
        };
        self.demo_reconcile();
        self.finish(j, res)
    }
    pub fn selected(&self) -> Option<&Row> {
        self.rows
            .iter()
            .filter(|r| {
                self.prefs.bound(&r.device)
                    && r.battery.is_some_and(|b| b.percentage_known)
                    && r.error.is_none()
            })
            .min_by_key(|r| {
                let rank = match self.prefs.priority {
                    Priority::Lowest => 0,
                    Priority::Mouse => match r.device.kind {
                        Kind::Mouse => 0,
                        Kind::Keyboard => 1,
                        Kind::Bluetooth | Kind::Audio | Kind::Gamepad => 2,
                    },
                    Priority::Keyboard => match r.device.kind {
                        Kind::Keyboard => 0,
                        Kind::Mouse => 1,
                        Kind::Bluetooth | Kind::Audio | Kind::Gamepad => 2,
                    },
                };
                (rank, r.battery.unwrap().display)
            })
    }
    pub fn demo() -> Self {
        let mut s = Self {
            demo: true,
            ..Default::default()
        };
        for (name, kind, pct, charge) in [
            ("演示鼠标", Kind::Mouse, 72, Charge::Discharging),
            ("演示键盘", Kind::Keyboard, 35, Charge::Charging),
        ] {
            s.rows.push(Row {
                device: Descriptor {
                    id: name.into(),
                    name: name.into(),
                    kind,
                    legacy_ids: Vec::new(),
                    identity: None,
                    generic_name: None,
                },
                battery: Reading::new(pct, charge, battery_protocol::Source::Rog),
                error: None,
                sampled: 0,
                held: false,
            });
        }
        s.rows[0].battery.as_mut().unwrap().source = battery_protocol::Source::Hidpp(0x1004);
        s.prefs.keyboard = Some(s.rows[1].device.id.clone());
        s.candidates = s.rows.iter().map(|r| r.device.clone()).collect();
        for (n, k) in [
            ("NUT75", Kind::Keyboard),
            ("Bluetooth Headset", Kind::Bluetooth),
        ] {
            s.candidates.push(Descriptor {
                id: format!("demo:{n}"),
                name: n.into(),
                kind: k,
                legacy_ids: Vec::new(),
                identity: None,
                generic_name: None,
            });
        }
        s
    }
}
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Flyout,
    Settings,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    Sync(u64, bool),
    Refresh,
    Scan,
    DiscoverBt,
    Bind(String),
    Unbind(String),
    Glass(i8),
    Theme,
    Language,
    Priority,
    Style,
    SetStyle(Style),
    SetLanguage(Language),
    SetTheme(Theme),
    /// 切换设备识别模式。
    SetEngineMode(EngineMode),
    SetUnknownProbe(bool),
    SetPriority(Priority),
    SetLow(u8),
    SetPollInterval(u64),
    DeviceRequest(DeviceJob),
    Low,
    Alias(String, String),
    Presented(u64),
    Hidden(u64),
    Ready,
    QuitGui,
    Autostart,
    AutoUpdate,
    CheckUpdate,
    DownloadUpdate,
    CancelUpdate,
    OpenUpdate,
    InstallUpdate,
    Switched(Mode, u64),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    State(Box<State>),
    Open(Mode, u64, Option<[i32; 2]>),
    Hide(u64),
    Exit,
}
impl Prefs {
    pub fn change(&mut self, c: &Command, list: &[Descriptor]) -> bool {
        match c {
            Command::SetLanguage(v) => self.lang = v.clone(),
            Command::SetTheme(v) => self.theme = v.clone(),
            Command::SetEngineMode(v) => {
                self.engine_mode = *v;
                self.probe_unknown_devices = false;
            }
            Command::SetUnknownProbe(v) => self.probe_unknown_devices = *v,
            Command::SetPriority(v) => self.priority = v.clone(),
            Command::SetLow(v) if [0, 10, 20, 30].contains(v) => self.low = *v,
            Command::SetPollInterval(v) if POLL_INTERVALS.contains(v) => {
                self.poll_interval_secs = *v
            }
            Command::SetStyle(v) => self.style = v.clone(),
            Command::AutoUpdate => self.auto_update = !self.auto_update,
            Command::Glass(x) => self.glass = (self.glass as i16 + *x as i16).clamp(0, 100) as u8,
            Command::Low => {
                self.low = match self.low {
                    0 => 10,
                    10 => 20,
                    20 => 30,
                    _ => 0,
                }
            }
            Command::Theme => {
                self.theme = match self.theme {
                    Theme::Auto => Theme::Light,
                    Theme::Light => Theme::Dark,
                    Theme::Dark => Theme::Auto,
                }
            }
            Command::Language => {
                self.lang = match self.lang {
                    Language::Auto => Language::Zh,
                    Language::Zh => Language::En,
                    Language::En => Language::Ru,
                    Language::Ru => Language::Auto,
                }
            }
            Command::Style => {
                self.style = match self.style {
                    Style::Capsule => Style::Vertical,
                    Style::Vertical => Style::Number,
                    Style::Number => Style::Circle,
                    Style::Circle => Style::Capsule,
                }
            }
            Command::Priority => {
                self.priority = match self.priority {
                    Priority::Mouse => Priority::Keyboard,
                    Priority::Keyboard => Priority::Lowest,
                    Priority::Lowest => Priority::Mouse,
                }
            }
            Command::Bind(id) => {
                let Some(d) = list.iter().find(|d| d.id == *id) else {
                    return false;
                };
                match d.kind {
                    Kind::Bluetooth => {
                        self.bluetooth.insert(id.clone());
                    }
                    Kind::Keyboard => {
                        self.clear_replaced_keyboard_identity(id);
                        if let Some(old) = self.keyboard.replace(id.clone()).filter(|old| old != id)
                        {
                            self.bound_identity.remove(&old);
                        }
                        self.keyboard_connection = None;
                    }
                    _ => return false,
                }
                self.bound_names.insert(id.clone(), d.name.clone());
            }
            Command::Unbind(id) => {
                if self.keyboard.as_ref() == Some(id) {
                    self.keyboard = None;
                    self.keyboard_connection = None;
                }
                self.bluetooth.remove(id);
                self.bound_names.remove(id);
                self.bound_identity.remove(id);
            }
            Command::Alias(id, name)
                if (list.iter().any(|d| d.id == *id) || self.bound_names.contains_key(id))
                    && name.encode_utf16().count() <= 32 =>
            {
                if name.trim().is_empty() {
                    self.aliases.remove(id);
                } else {
                    self.aliases.insert(id.clone(), name.trim().into());
                }
            }
            _ => return false,
        }
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn probed_binding(kind: Kind) -> (Prefs, Descriptor, battery_protocol::ProbedRoute) {
        use battery_protocol::{
            BatteryCapability, ConnectionKind, DeviceCategory, DeviceIdentity, ProbeVariantId,
        };
        let keyboard = kind == Kind::Keyboard;
        let device = Descriptor {
            id: "transport".into(),
            name: "Receiver".into(),
            kind,
            legacy_ids: vec![],
            identity: Some(DeviceIdentity {
                model_key: None,
                category: if keyboard {
                    DeviceCategory::Keyboard
                } else {
                    DeviceCategory::Unknown
                },
                connection: if keyboard {
                    ConnectionKind::Rf24
                } else {
                    ConnectionKind::Bluetooth
                },
                battery: BatteryCapability::Present,
                model_source: None,
            }),
            generic_name: None,
        };
        let mut prefs = Prefs::default();
        if keyboard {
            prefs.keyboard = Some("logical".into());
            prefs.keyboard_connection = Some(KeyboardConnection {
                endpoint_id: device.id.clone(),
                serial: None,
            });
        } else {
            prefs.bluetooth.insert(device.id.clone());
        }
        let route = battery_protocol::ProbedRoute {
            variant: if keyboard {
                ProbeVariantId::AulaHfdWireless56
            } else {
                ProbeVariantId::RongYuanBle77
            },
            vid: 0x0c45,
            pid: 0x1234,
            usage_page: 0xff00,
            usage: 1,
        };
        (prefs, device, route)
    }

    #[test]
    fn probed_binding_round_trips_and_legacy_snapshot_omits_the_route() {
        let (mut prefs, device, route) = probed_binding(Kind::Keyboard);
        assert!(prefs.remember_identity(&device));
        let old_json = serde_json::to_value(&prefs).unwrap();
        assert!(
            old_json["bound_identity"]["logical"]
                .get("probed_route")
                .is_none()
        );
        let old: Prefs = serde_json::from_value(old_json).unwrap();
        assert!(old.bound_probed_routes().is_empty());
        assert!(prefs.remember_identity_with_route(&device, Some(route)));
        let json = serde_json::to_value(&prefs).unwrap();
        assert_eq!(
            json["bound_identity"]["logical"]["probed_route"],
            serde_json::to_value(route).unwrap()
        );
        let mut restored: Prefs = serde_json::from_value(json).unwrap();
        assert_eq!(
            restored.bound_probed_routes(),
            vec![(device.id.clone(), route)]
        );
        assert!(restored.has_probed_only_binding("logical"));
        assert!(restored.has_probed_only_binding(&device.id));
        assert!(restored.clear_identity(&device));
        assert!(restored.bound_probed_routes().is_empty());
        assert!(restored.remember_identity_with_route(&device, Some(route)));
        assert!(restored.change(&Command::Unbind("logical".into()), &[]));
        assert!(restored.bound_identity.is_empty());
    }

    #[test]
    fn offline_probed_bindings_keep_cached_routes_in_both_modes() {
        let legacy: Prefs = serde_json::from_str(r#"{"engine_mode":"Compatible"}"#).unwrap();
        assert!(!legacy.unknown_probe_enabled());
        for kind in [Kind::Keyboard, Kind::Bluetooth] {
            let (mut prefs, device, route) = probed_binding(kind);
            assert!(prefs.remember_identity_with_route(&device, Some(route)));
            let identity = prefs.bound_identity.clone();
            for (mode, probe, error) in [
                (EngineMode::Normal, false, PROBED_COMPATIBLE_ONLY_ERROR),
                (EngineMode::Normal, true, PROBED_COMPATIBLE_ONLY_ERROR),
                (EngineMode::Compatible, false, PROBED_COMPATIBLE_ONLY_ERROR),
                (EngineMode::Compatible, true, "offline"),
            ] {
                prefs.engine_mode = mode;
                prefs.probe_unknown_devices = probe;
                let mut rows = Vec::new();
                prefs.preserve_missing(&mut rows, kind == Kind::Bluetooth);
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].error.as_deref(), Some(error));
                assert_eq!(rows[0].device.identity, device.identity);
                assert_eq!(prefs.bound_identity, identity);
            }
        }
    }

    #[test]
    fn replacing_probed_keyboard_transport_discards_the_old_route() {
        let (mut prefs, device, route) = probed_binding(Kind::Keyboard);
        assert!(prefs.remember_identity_with_route(&device, Some(route)));
        prefs.clear_replaced_keyboard_identity(&device.id);
        assert!(prefs.has_probed_only_binding("logical"));
        prefs.clear_replaced_keyboard_identity("replacement");
        assert!(prefs.bound_identity.is_empty());
    }

    #[test]
    fn replacing_keyboard_endpoint_checks_endpoint_id_and_candidate_numbering() {
        use battery_protocol::{
            BatteryCapability, ConnectionKind, DeviceCategory, DeviceIdentity, GenericName,
            ModelSource,
        };
        let exact = Descriptor {
            id: "old-ep".into(),
            name: "Exact Keyboard".into(),
            kind: Kind::Keyboard,
            legacy_ids: vec![],
            identity: Some(DeviceIdentity {
                model_key: Some("exact_kb".into()),
                category: DeviceCategory::Keyboard,
                connection: ConnectionKind::Rf24,
                battery: BatteryCapability::Present,
                model_source: Some(ModelSource::CatalogRule),
            }),
            generic_name: None,
        };
        let mut generic_new = Descriptor {
            id: "new-ep".into(),
            name: "2.4G Receiver".into(),
            kind: Kind::Keyboard,
            legacy_ids: vec![],
            identity: Some(DeviceIdentity {
                model_key: None,
                category: DeviceCategory::Keyboard,
                connection: ConnectionKind::Rf24,
                battery: BatteryCapability::Present,
                model_source: None,
            }),
            generic_name: Some(GenericName::Receiver24g {
                brand: None,
                channel: Some(DeviceCategory::Keyboard),
            }),
        };
        let mut prefs = Prefs {
            lang: Language::En,
            keyboard: Some("logical".into()),
            keyboard_connection: Some(KeyboardConnection {
                endpoint_id: exact.id.clone(),
                serial: None,
            }),
            ..Default::default()
        };
        assert!(prefs.remember_identity(&exact));
        // 即使尚未清除缓存，不同端点 ID 也不得继承旧端点的精确身份。
        assert_ne!(prefs.name(&generic_new), "Exact Keyboard");
        generic_new.id = "logical".into();
        prefs.keyboard_connection = Some(KeyboardConnection {
            endpoint_id: "new-ep".into(),
            serial: None,
        });
        assert_ne!(prefs.name(&generic_new), "Exact Keyboard");

        // 无序多通用候选在兼容模式下按品类与稳定 ID 编号，普通模式保持原名。
        let bt = |id: &str| Descriptor {
            id: id.into(),
            name: "BLE".into(),
            kind: Kind::Bluetooth,
            legacy_ids: vec![],
            identity: Some(DeviceIdentity {
                model_key: None,
                category: DeviceCategory::Unknown,
                connection: ConnectionKind::Bluetooth,
                battery: BatteryCapability::Present,
                model_source: None,
            }),
            generic_name: Some(GenericName::UnknownDevice),
        };
        let mut st = State {
            prefs: Prefs {
                lang: Language::En,
                engine_mode: EngineMode::Compatible,
                ..Default::default()
            },
            candidates: vec![bt("bt-b"), bt("bt-a")],
            ..Default::default()
        };
        assert_eq!(st.candidate_name(&st.candidates[1]), "Bluetooth device");
        assert_eq!(st.candidate_name(&st.candidates[0]), "Bluetooth device 2");
        st.prefs.engine_mode = EngineMode::Normal;
        assert_eq!(st.candidate_name(&st.candidates[0]), "Bluetooth device");
    }

    #[test]
    fn russian_preferences_round_trip_and_legacy_language_imports() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.json");
        let mut prefs = Prefs::default();
        for lang in [Language::Auto, Language::Zh, Language::En, Language::Ru] {
            assert!(prefs.change(&Command::SetLanguage(lang.clone()), &[]));
            prefs.save(&path).unwrap();
            assert_eq!(Prefs::load(&path).unwrap().lang, lang);
        }
        let command: Command = serde_json::from_str(r#"{"SetLanguage":"Ru"}"#).unwrap();
        assert!(prefs.change(&command, &[]));
        assert_eq!(prefs.lang, Language::Ru);
        assert!(prefs.change(&Command::Language, &[]));
        assert_eq!(prefs.lang, Language::Auto);
    }
    #[test]
    fn keyboard_rebind_and_unbind_clear_transport_but_bluetooth_bind_does_not() {
        let mut state = State::demo();
        let id = state.prefs.keyboard.clone().unwrap();
        let connection = KeyboardConnection {
            endpoint_id: "new endpoint".into(),
            serial: Some("unique".into()),
        };
        state.prefs.keyboard_connection = Some(connection.clone());
        let bt = Descriptor {
            id: "headset".into(),
            name: "Headset".into(),
            kind: Kind::Bluetooth,
            legacy_ids: vec![],
            identity: None,
            generic_name: None,
        };
        assert!(state.prefs.change(&Command::Bind(bt.id.clone()), &[bt]));
        assert_eq!(state.prefs.keyboard_connection, Some(connection.clone()));
        assert!(state.prefs.change(&Command::Unbind(id.clone()), &[]));
        assert!(state.prefs.keyboard_connection.is_none());
        let keyboard = state.rows[1].device.clone();
        state.prefs.keyboard_connection = Some(connection);
        assert!(
            state
                .prefs
                .change(&Command::Bind(keyboard.id.clone()), &[keyboard])
        );
        assert!(state.prefs.keyboard_connection.is_none());
        assert_eq!(state.prefs.keyboard.as_deref(), Some(id.as_str()));
    }
    #[test]
    fn select() {
        let mut s = State::demo();
        assert_eq!(s.selected().unwrap().battery.unwrap().raw, 72);
        s.prefs.priority = Priority::Lowest;
        assert_eq!(s.selected().unwrap().battery.unwrap().raw, 35);
        s.rows[1].error = Some("sleep".into());
        assert_eq!(s.selected().unwrap().battery.unwrap().raw, 72)
    }
    #[test]
    fn disk() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("config");
        let mut p = Prefs::default();
        p.change(&Command::Glass(5), &[]);
        p.save(&path).unwrap();
        assert_eq!(Prefs::load(&path).unwrap(), p);
        p.change(&Command::Glass(5), &[]);
        p.save(&path).unwrap();
        assert_eq!(Prefs::load(&path).unwrap().glass, 35);
        std::fs::write(&path, "broken").unwrap();
        assert!(Prefs::load(&path).is_err());
    }
    #[test]
    fn legacy_preferences_default_to_thirty_second_polling() {
        let prefs: Prefs = serde_json::from_str(r#"{"low":20,"glass":25}"#).unwrap();
        assert_eq!(
            serde_json::to_value(prefs).unwrap()["poll_interval_secs"],
            30
        );
    }
    #[test]
    fn polling_interval_is_persisted_and_invalid_values_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.json");
        for seconds in [5, 10, 15, 30, 60, 120, 300] {
            std::fs::write(&path, format!(r#"{{"poll_interval_secs":{seconds}}}"#)).unwrap();
            let prefs = Prefs::load(&path).unwrap();
            prefs.save(&path).unwrap();
            let saved: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(saved["poll_interval_secs"], seconds);
        }
        for seconds in [0, 1, 301, 3600] {
            std::fs::write(&path, format!(r#"{{"poll_interval_secs":{seconds}}}"#)).unwrap();
            assert!(Prefs::load(&path).is_err());
        }
    }
    #[test]
    fn polling_command_validates_before_changing_preferences() {
        let mut prefs = Prefs::default();
        for seconds in POLL_INTERVALS {
            let command: Command =
                serde_json::from_value(serde_json::json!({"SetPollInterval":seconds})).unwrap();
            assert!(prefs.change(&command, &[]));
            assert_eq!(prefs.poll_ms(), seconds * 1000);
        }
        for seconds in [0, 1, 301, u64::MAX] {
            let old = prefs.clone();
            assert!(!prefs.change(&Command::SetPollInterval(seconds), &[]));
            assert_eq!(prefs, old);
        }
    }
    #[test]
    fn refuse_unknown() {
        assert!(!Prefs::default().change(&Command::Bind("other".into()), &[]))
    }
    #[test]
    fn legacy_migration_field_in_saved_preferences_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("preferences-v2.json");
        std::fs::write(
            &file,
            r#"{"glass":40,"migration":{"keyboard":["old"],"aliases":{"a":"b"}}}"#,
        )
        .unwrap();
        let loaded = Prefs::load(&file).unwrap();
        assert_eq!(loaded.glass, 40);
    }
}

fn exact<'a>(
    list: &'a [Descriptor],
    keys: &[String],
    kind: Option<Kind>,
) -> Option<&'a Descriptor> {
    let mut matches = list.iter().filter(|d| {
        kind.is_none_or(|k| d.kind == k)
            && keys.iter().any(|k| {
                d.id.eq_ignore_ascii_case(k)
                    || d.legacy_ids.iter().any(|v| v.eq_ignore_ascii_case(k))
            })
    });
    let d = matches.next()?;
    matches.next().is_none().then_some(d)
}
#[cfg(test)]
mod parity_tests {
    use super::*;
    #[test]
    fn requests_match_worker_and_generation() {
        let mut s = State::demo();
        let j = DeviceJob {
            id: 12,
            kind: Kind::Bluetooth,
            bind: Some("demo:Bluetooth Headset".into()),
        };
        s.demo_request(&j);
        assert_eq!(s.reply(Kind::Bluetooth).unwrap().id, 12);
        assert!(s.reply(Kind::Keyboard).is_none());
        assert!(
            s.rows
                .iter()
                .any(|r| r.device.id == "demo:Bluetooth Headset")
        );
        s.demo_request(&DeviceJob {
            id: 13,
            kind: Kind::Keyboard,
            bind: j.bind,
        });
        assert!(s.reply(Kind::Keyboard).unwrap().result.is_err())
    }
    #[test]
    fn paused_manual_completion_requires_both_matching_workers() {
        let mut m = Manual {
            id: 11,
            ..Manual::default()
        };
        m.finish(11, false, None);
        m.finish(10, true, None);
        assert!(!m.completed());
        m.finish(11, true, Some(Notice::RefreshFailed));
        assert!(m.completed());
        assert!(m.error.is_some())
    }
    #[test]
    fn alias_limits_are_utf16() {
        let s = State::demo();
        let mut p = Prefs::default();
        let id = s.candidates[0].id.clone();
        assert!(p.change(&Command::Alias(id.clone(), "🖱".repeat(16)), &s.candidates));
        assert!(!p.change(&Command::Alias(id, "🖱".repeat(17)), &s.candidates));
    }
}

#[cfg(test)]
mod audit_identity {
    use super::*;
    #[test]
    fn prior_profile_is_read_only_and_corrupt_new_profile_never_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let prior = dir.path().join("preferences.json");
        let next = dir.path().join("preferences-v2.json");
        let old = Prefs {
            glass: 45,
            keyboard: Some("hid:old".into()),
            ..Prefs::default()
        };
        old.save(&prior).unwrap();
        let original = std::fs::read(&prior).unwrap();
        let mut p = Prefs::load(&next).unwrap();
        assert_eq!(p.glass, 45);
        assert_eq!(p.keyboard, old.keyboard);
        assert!(!next.exists());
        p.keyboard = Some("hid:new".into());
        p.glass = 70;
        p.save(&next).unwrap();
        assert_eq!(Prefs::load(&next).unwrap().glass, 70);
        assert_eq!(Prefs::load(&prior).unwrap().glass, 45);
        std::fs::write(&next, b"!").unwrap();
        assert!(Prefs::load(&next).is_err());
        assert_eq!(std::fs::read(prior).unwrap(), original);
    }
    #[test]
    fn migration_leaves_ambiguous_keys_untouched() {
        let mut p = Prefs::default();
        let old = "hid:old".to_owned();
        p.keyboard = Some(old.clone());
        p.aliases.insert(old.clone(), "Mine".into());
        let mk = |id: &str| Descriptor {
            id: id.into(),
            name: "NUT75".into(),
            kind: Kind::Keyboard,
            legacy_ids: vec![old.clone()],
            identity: None,
            generic_name: None,
        };
        assert!(!p.resolve(&[mk("a"), mk("b")]));
        assert_eq!(p.keyboard.as_ref(), Some(&old));
        assert!(p.resolve(&[mk("a")]));
        assert_eq!(p.keyboard.as_deref(), Some("a"));
        assert_eq!(p.aliases["a"], "Mine");
        assert!(!p.resolve(&[mk("a")]));
    }
}

#[cfg(test)]
mod formal_review_tests {
    use super::*;
    #[test]
    fn unbound_rows_cannot_take_over_the_tray() {
        let mut s = State::demo();
        s.prefs.priority = Priority::Lowest;
        s.prefs.keyboard = None;
        assert_eq!(s.selected().unwrap().device.kind, Kind::Mouse);
    }
    #[test]
    fn oversized_preferences_are_rejected_before_deserialization() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("preferences-v2.json");
        let bytes = format!(r#"{{"future_field":"{}"}}"#, "a".repeat(4 * 1024 * 1024));
        std::fs::write(&file, bytes).unwrap();
        assert!(Prefs::load(&file).is_err());
    }
}

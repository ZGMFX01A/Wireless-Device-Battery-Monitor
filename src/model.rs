use battery_protocol::{Charge, Descriptor, Kind, Reading};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::{Path, PathBuf},
};
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
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Prefs {
    pub priority: Priority,
    pub style: Style,
    pub theme: Theme,
    pub lang: Language,
    pub glass: u8,
    pub auto_update: bool,
    pub low: u8,
    pub keyboard: Option<String>,
    pub bluetooth: BTreeSet<String>,
    pub aliases: BTreeMap<String, String>,
    pub bound_names: BTreeMap<String, String>,
    pub migration: LegacyDevices,
}
impl Default for Prefs {
    fn default() -> Self {
        Self {
            priority: Priority::Mouse,
            style: Default::default(),
            theme: Default::default(),
            lang: Default::default(),
            glass: 25,
            auto_update: false,
            low: 20,
            keyboard: None,
            bluetooth: Default::default(),
            aliases: Default::default(),
            bound_names: Default::default(),
            migration: Default::default(),
        }
    }
}
impl Prefs {
    pub fn bound(&self, d: &Descriptor) -> bool {
        match d.kind {
            Kind::Mouse => true,
            Kind::Keyboard => self.keyboard.as_ref() == Some(&d.id),
            Kind::Bluetooth => self.bluetooth.contains(&d.id),
        }
    }
    pub fn name<'a>(&'a self, d: &'a Descriptor) -> &'a str {
        self.aliases
            .get(&d.id)
            .map(String::as_str)
            .unwrap_or(&d.name)
    }
    pub fn preserve_missing(&self, rows: &mut Vec<Row>, bt: bool) {
        let ids: Vec<_> = if bt {
            self.bluetooth.iter().collect()
        } else {
            self.keyboard.iter().collect()
        };
        for id in ids {
            if !rows.iter().any(|r| &r.device.id == id) {
                rows.push(Row {
                    device: Descriptor {
                        id: id.clone(),
                        name: self.bound_names.get(id).cloned().unwrap_or_else(|| {
                            if bt {
                                "Bluetooth".into()
                            } else {
                                "Keyboard".into()
                            }
                        }),
                        kind: if bt { Kind::Bluetooth } else { Kind::Keyboard },
                        legacy_ids: vec![],
                    },
                    battery: None,
                    error: Some("offline".into()),
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
    /// Read-only import: do not map bindings by name, and never touch the legacy Run entry.
    pub fn import_appearance(&mut self, path: &Path) -> Result<(), String> {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        if file.metadata().map_err(|e| e.to_string())?.len() > 4 * 1024 * 1024 {
            return Err("configuration is too large".into());
        }
        let d: serde_json::Value = serde_json::from_reader(file).map_err(|e| e.to_string())?;
        let o = d.as_object().ok_or("invalid legacy settings")?;
        let text = |k: &str| o.get(k).and_then(|v| v.as_str());
        if let Some(x) = o
            .get("glass_transparency")
            .and_then(|v| v.as_u64())
            .filter(|v| *v <= 100)
        {
            self.glass = x as u8
        }
        if let Some(x) = o
            .get("low_battery_notify")
            .and_then(|v| v.as_u64())
            .filter(|v| [0, 10, 20, 30].contains(v))
        {
            self.low = x as u8
        }
        self.priority = match text("tray_icon_priority") {
            Some("mouse_first") => Priority::Mouse,
            Some("keyboard_first") => Priority::Keyboard,
            Some("lowest_battery_first") => Priority::Lowest,
            _ => self.priority.clone(),
        };
        self.theme = match text("theme_mode") {
            Some("dark") => Theme::Dark,
            Some("light") => Theme::Light,
            Some("auto") => Theme::Auto,
            _ => self.theme.clone(),
        };
        self.lang = match text("ui_language") {
            Some("en-US") => Language::En,
            Some("zh-CN") => Language::Zh,
            Some("auto") => Language::Auto,
            _ => self.lang.clone(),
        };
        self.style = match text("tray_icon_style") {
            Some("number") => Style::Number,
            Some("vertical") => Style::Vertical,
            Some("capsule") => Style::Capsule,
            Some("circle") => Style::Circle,
            _ => self.style.clone(),
        };
        Ok(())
    }
    /// Opt-in exact-identity import. Preserve unmatched entries for devices currently asleep.
    pub fn import_devices(&mut self, path: &Path, list: &[Descriptor]) -> Result<usize, String> {
        let b = std::fs::File::open(path).map_err(|e| e.to_string())?;
        if b.metadata().map_err(|e| e.to_string())?.len() > 4 * 1024 * 1024 {
            return Err("preferences too large".into());
        }
        let v: serde_json::Value = serde_json::from_reader(b).map_err(|e| e.to_string())?;
        self.migration = LegacyDevices::parse(&v);
        self.resolve(list);
        Ok(self.migration.remaining())
    }
    pub fn resolve(&mut self, list: &[Descriptor]) -> bool {
        // Carry forward 0.1.2 HID references only when exactly one current candidate
        // owns that old key. A model name is not physical identity.
        let mut remapped = false;
        if let Some(old) = self.keyboard.clone()
            && let Some(d) = exact(list, std::slice::from_ref(&old), Some(Kind::Keyboard))
            && d.id != old
        {
            self.keyboard = Some(d.id.clone());
            let previous = self
                .bound_names
                .remove(&old)
                .unwrap_or_else(|| d.name.clone());
            self.bound_names.entry(d.id.clone()).or_insert(previous);
            remapped = true;
        }
        for (old, alias) in self.aliases.clone() {
            if old.starts_with("hid:")
                && let Some(d) = exact(list, std::slice::from_ref(&old), None)
                && d.id != old
            {
                self.aliases.entry(d.id.clone()).or_insert(alias);
                self.aliases.remove(&old);
                remapped = true;
            }
        }
        let mut mig = std::mem::take(&mut self.migration);
        let before = mig.remaining();
        if let Some(ref k) = mig.keyboard
            && let Some(d) = exact(list, k, Some(Kind::Keyboard))
        {
            if self.keyboard.is_none() {
                self.keyboard = Some(d.id.clone());
                self.bound_names.insert(d.id.clone(), d.name.clone());
            }
            mig.keyboard = None;
        }
        mig.bluetooth.retain(|keys| {
            if let Some(d) = exact(list, keys, Some(Kind::Bluetooth)) {
                self.bluetooth.insert(d.id.clone());
                self.bound_names.insert(d.id.clone(), d.name.clone());
                false
            } else {
                true
            }
        });
        mig.aliases.retain(|old, n| {
            if let Some(d) = exact(list, std::slice::from_ref(old), None) {
                self.aliases
                    .entry(d.id.clone())
                    .or_insert_with(|| n.clone());
                false
            } else {
                true
            }
        });
        let changed = remapped || before != mig.remaining();
        self.migration = mig;
        changed
    }
    pub fn load(p: &Path) -> Result<Self, String> {
        match std::fs::read(p) {
            Ok(b) => {
                let v: Self =
                    serde_json::from_slice(&b).map_err(|e| format!("Invalid preferences: {e}"))?;
                if v.glass > 100 || ![0, 10, 20, 30].contains(&v.low) {
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
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Row {
    pub device: Descriptor,
    pub battery: Option<Reading>,
    pub error: Option<String>,
    pub sampled: u64,
    /// Last accepted reading remains valid; the most recent packet was rejected.
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
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manual {
    pub id: u64,
    pub hid: bool,
    pub bt: bool,
    pub error: Option<String>,
}
impl Manual {
    pub fn completed(&self) -> bool {
        self.hid && self.bt
    }
    pub fn finish(&mut self, id: u64, bt: bool, error: Option<String>) {
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
    pub manual: Manual,
    pub keyboard_reply: Option<Receipt>,
    pub bluetooth_reply: Option<Receipt>,
    pub release_notes: String,
    pub release_size: u64,
    pub bytes_per_sec: u64,
    pub note: String,
    pub demo: bool,
    pub autostart: bool,
    pub update: crate::update::Status,
}
impl State {
    /// Suppress only *painting* an invisible timestamp. Destructure exhaustively:
    /// adding a receipt or a new visible field must not silently drop it.
    pub fn same_except_sample_time(&self, o: &Self) -> bool {
        let State {
            prefs,
            rows,
            candidates,
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
            .filter(|r| r.battery.is_some() && r.error.is_none())
            .min_by_key(|r| {
                let rank = match self.prefs.priority {
                    Priority::Lowest => 0,
                    Priority::Mouse => match r.device.kind {
                        Kind::Mouse => 0,
                        Kind::Keyboard => 1,
                        Kind::Bluetooth => 2,
                    },
                    Priority::Keyboard => match r.device.kind {
                        Kind::Keyboard => 0,
                        Kind::Mouse => 1,
                        Kind::Bluetooth => 2,
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
    SetPriority(Priority),
    SetLow(u8),
    DeviceRequest(DeviceJob),
    ImportAppearance,
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
            Command::SetPriority(v) => self.priority = v.clone(),
            Command::SetLow(v) if [0, 10, 20, 30].contains(v) => self.low = *v,
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
                    Language::En => Language::Auto,
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
            Command::Bind(id) => match list.iter().find(|d| d.id == *id).map(|d| d.kind) {
                Some(Kind::Bluetooth) => {
                    self.bound_names.insert(
                        id.clone(),
                        list.iter().find(|d| d.id == *id).unwrap().name.clone(),
                    );
                    self.bluetooth.insert(id.clone());
                }
                Some(Kind::Keyboard) => {
                    self.bound_names.insert(
                        id.clone(),
                        list.iter().find(|d| d.id == *id).unwrap().name.clone(),
                    );
                    self.keyboard = Some(id.clone());
                    self.migration.keyboard = None;
                }
                _ => return false,
            },
            Command::Unbind(id) => {
                if self.keyboard.as_ref() == Some(id) {
                    self.keyboard = None
                }
                self.bluetooth.remove(id);
                self.bound_names.remove(id);
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
    fn refuse_unknown() {
        assert!(!Prefs::default().change(&Command::Bind("other".into()), &[]))
    }
}

#[cfg(test)]
mod import_tests {
    use super::*;
    #[test]
    fn only_import_display_fields() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("legacy.json");
        let bytes=br#"{"theme_mode":"dark","glass_transparency":70,"tray_icon_priority":"lowest_battery_first","autostart":true,"keyboard_binding":{"device_id":"untrusted"},"auto_update":true}"#;
        std::fs::write(&f, bytes).unwrap();
        let mut p = Prefs::default();
        p.import_appearance(&f).unwrap();
        assert_eq!(p.glass, 70);
        assert_eq!(p.priority, Priority::Lowest);
        assert!(p.keyboard.is_none());
        assert!(!p.auto_update);
        assert_eq!(std::fs::read(f).unwrap(), bytes);
    }
}

#[derive(Default, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct LegacyDevices {
    keyboard: Option<Vec<String>>,
    bluetooth: Vec<Vec<String>>,
    aliases: BTreeMap<String, String>,
}
impl LegacyDevices {
    pub fn remaining(&self) -> usize {
        self.keyboard.is_some() as usize + self.bluetooth.len() + self.aliases.len()
    }
    fn parse(v: &serde_json::Value) -> Self {
        fn keys(v: &serde_json::Value) -> Vec<String> {
            let mut out = vec![];
            if let Some(ep) = v["device_id"].as_str().filter(|v| !v.is_empty()) {
                out.push(ep.to_lowercase())
            }
            if let Some(c) = v["container_id"].as_str() {
                let g = c
                    .trim_matches(['{', '}'])
                    .replace('-', "")
                    .to_ascii_lowercase();
                if g.len() == 32
                    && g.bytes().all(|c| c.is_ascii_hexdigit())
                    && g.bytes().any(|c| c != b'0')
                {
                    out.push(format!("container:{g}"));
                }
            }
            let a = &v["bluetooth_address"];
            if let Some(a) = a
                .as_u64()
                .or_else(|| {
                    a.as_str()
                        .and_then(|s| u64::from_str_radix(&s.replace([':', '-'], ""), 16).ok())
                })
                .filter(|a| *a > 0 && *a <= 0xffffffffffff)
            {
                out.push(format!("address:{a:012x}"))
            }
            out
        }
        let k = keys(&v["keyboard_binding"]);
        Self {
            keyboard: (!k.is_empty()).then_some(k),
            bluetooth: v["bluetooth_bindings"]
                .as_array()
                .into_iter()
                .flatten()
                .map(keys)
                .filter(|k| !k.is_empty())
                .collect(),
            aliases: v["device_custom_names"]
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(k, v)| {
                    v.as_str()
                        .filter(|s| !s.trim().is_empty() && s.encode_utf16().count() <= 32)
                        .map(|s| (k.to_lowercase(), s.to_owned()))
                })
                .collect(),
        }
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
        m.finish(11, true, Some("timeout".into()));
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
    #[test]
    fn exact_import_never_guesses_name_and_preserves_legacy_bytes() {
        let mut d = Descriptor {
            id: "new".into(),
            kind: Kind::Keyboard,
            name: "Same".into(),
            legacy_ids: vec!["old_path".into()],
        };
        let temp = tempfile::tempdir().unwrap();
        let f = temp.path().join("old.json");
        let bytes=br#"{"keyboard_binding":{"device_id":"OLD_PATH"},"device_custom_names":{"old_path":"Work"}}"#;
        std::fs::write(&f, bytes).unwrap();
        let mut p = Prefs::default();
        assert_eq!(p.import_devices(&f, &[]).unwrap(), 2);
        let mut other = d.clone();
        other.id = "other".into();
        assert!(!p.resolve(&[d.clone(), other]));
        assert!(p.resolve(std::slice::from_ref(&d)));
        assert_eq!(p.name(&d), "Work");
        let mut rows = vec![];
        p.preserve_missing(&mut rows, false);
        assert_eq!(rows[0].device.name, "Same");
        assert!(rows[0].battery.is_none());
        d.legacy_ids.clear();
        assert_eq!(std::fs::read(f).unwrap(), bytes)
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
        };
        assert!(!p.resolve(&[mk("a"), mk("b")]));
        assert_eq!(p.keyboard.as_ref(), Some(&old));
        assert!(p.resolve(&[mk("a")]));
        assert_eq!(p.keyboard.as_deref(), Some("a"));
        assert_eq!(p.aliases["a"], "Mine");
        assert!(!p.resolve(&[mk("a")]));
    }
}

use crate::model::*;
use battery_protocol::{Charge, Descriptor, Kind, Reading, bluetooth, hid};
use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const SCAN: u8 = 1;
const REFRESH: u8 = 2;
const PREFS: u8 = 4;
const JOB: u8 = 64;
const SYNC: u8 = 128;
const RESUME: u8 = 8;
const STOP: u8 = 16;
const SUSPEND: u8 = 32;
#[derive(Default)]
pub struct Signal {
    bits: Mutex<u8>,
    device_scan: Mutex<Option<Instant>>,
    manual: Mutex<Option<(u64, bool)>>,
    jobs: Mutex<std::collections::VecDeque<DeviceJob>>,
    cv: Condvar,
}
impl Signal {
    fn send(&self, b: u8) {
        let mut f = self.bits.lock().unwrap();
        if b == RESUME {
            *f &= !SUSPEND
        } else if b == SUSPEND {
            *f &= !RESUME
        }
        *f |= b;
        self.cv.notify_one()
    }
    fn take(&self, d: Duration) -> u8 {
        let f = self.bits.lock().unwrap();
        let (mut f, _) = self.cv.wait_timeout_while(f, d, |s| *s == 0).unwrap();
        std::mem::take(&mut *f)
    }
}
#[derive(Default)]
pub struct Signals {
    cycle: Mutex<PowerCycle>,
    hid: Signal,
    bt: Signal,
}
/// One recovery per wake cycle. A late user-input notification after automatic resume
/// is not a second wake. A fresh suspend always rearms recovery, even inside 10 s.
#[derive(Default)]
struct PowerCycle {
    last: Option<Instant>,
    automatic: bool,
}
impl PowerCycle {
    fn event(&mut self, code: u32, now: Instant) -> Option<u8> {
        match code {
            4 => {
                *self = Self::default();
                return Some(SUSPEND);
            }
            7 if !self.automatic => (),
            18 => self.automatic = true,
            _ => return None,
        }
        if self
            .last
            .is_some_and(|t| now.saturating_duration_since(t) < Duration::from_secs(10))
        {
            return None;
        }
        self.last = Some(now);
        Some(RESUME)
    }
}
impl Signals {
    pub fn power(&self, code: u32) {
        let mut cycle = self.cycle.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(signal) = cycle.event(code, Instant::now()) {
            self.both(signal);
        }
    }
    fn both(&self, b: u8) {
        self.hid.send(b);
        self.bt.send(b)
    }
}
pub struct Service {
    pub state: Arc<Mutex<State>>,
    pub signals: Arc<Signals>,
    workers: Vec<JoinHandle<()>>,
    demo: bool,
}
fn stamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
#[derive(Debug)]
struct Slot {
    row: Row,
    fail: u32,
    due: u64,
    interval_ms: u64,
    rebaseline: bool,
}
impl Slot {
    fn new(d: Descriptor, t: u64) -> Self {
        Self {
            row: Row {
                device: d,
                battery: None,
                error: None,
                sampled: 0,
                held: false,
            },
            fail: 0,
            due: t,
            interval_ms: DEFAULT_POLL_INTERVAL_SECS * 1000,
            rebaseline: true,
        }
    }
    fn set_interval(&mut self, interval_ms: u64, now: u64) {
        if self.interval_ms != interval_ms {
            self.interval_ms = interval_ms;
            self.due = now;
        }
    }
    fn apply(&mut self, r: Result<Reading, String>, t: u64, manual: bool) {
        if let Ok(r) = r
            && !manual
            && !self.rebaseline
            && self.row.device.kind == Kind::Mouse
            && self.row.sampled > 0
            && stamp().saturating_sub(self.row.sampled) <= 300
            && self.row.error.is_none()
            && let Some(prev) = self.row.battery
        {
            let max = if matches!(r.charge, Charge::Charging | Charge::Full) {
                60
            } else if r.source == battery_protocol::Source::Rog {
                50
            } else {
                40
            };
            if prev.raw.abs_diff(r.raw) > max {
                // A rejected packet is neither a new reading nor a disconnect. Advance the
                // deadline; returning with due in the past would poll hardware every 25 ms.
                self.row.held = true;
                self.retry(t);
                return;
            }
        }
        match r {
            Ok(r) => {
                self.row.held = false;
                self.rebaseline = false;
                self.row.battery = Some(r);
                self.row.sampled = stamp();
                self.row.error = None;
                self.fail = 0;
                self.due = t + self.interval_ms;
            }
            Err(e) => {
                self.rebaseline = true;
                self.row.error = Some(e);
                self.row.held = false;
                // No fresh device response means offline/unknown, never a
                // current percentage (or a cached zero) for any transport.
                self.row.battery = None;
                self.retry(t);
            }
        }
    }
    fn retry(&mut self, t: u64) {
        self.fail = self.fail.saturating_add(1);
        // Offline devices stay in the same polling cycle as online devices.
        self.due = t + self.interval_ms;
    }
}
enum Target {
    H(hid::Target, Option<hid::Session>),
    B(bluetooth::BtTarget),
}
fn transport_bound(prefs: &Prefs, d: &Descriptor) -> bool {
    if d.kind == Kind::Keyboard {
        prefs.keyboard_endpoint() == Some(d.id.as_str())
    } else {
        prefs.bound(d)
    }
}
fn schedule_device_scan(pending: &mut Option<Instant>, now: Instant) {
    pending.get_or_insert(now + Duration::from_millis(300));
}
fn next_poll(slots: &HashMap<String, Slot>, discovery: Option<u64>, now: u64) -> u64 {
    slots
        .values()
        .map(|s| s.due)
        .chain(discovery)
        .min()
        .unwrap_or(now + 3_600_000)
}
fn stale_bluetooth(slots: &HashMap<String, Slot>, prefs: &Prefs) -> bool {
    slots
        .values()
        .any(|s| s.row.device.kind == Kind::Bluetooth && prefs.bound(&s.row.device) && s.fail >= 3)
}
fn stale_keyboard(slots: &HashMap<String, Slot>, prefs: &Prefs) -> bool {
    prefs
        .keyboard_endpoint()
        .and_then(|id| slots.get(id))
        .is_some_and(|s| s.fail >= 3)
}
fn missing_devices<'a>(
    prefs: &Prefs,
    devices: impl Iterator<Item = &'a Descriptor> + Clone,
    bt: bool,
) -> (bool, bool) {
    if bt {
        return (
            prefs.bluetooth.iter().any(|id| {
                !devices
                    .clone()
                    .any(|d| d.kind == Kind::Bluetooth && d.id == *id)
            }),
            false,
        );
    }
    let mouse = !devices.clone().any(|d| d.kind == Kind::Mouse);
    let keyboard = prefs.keyboard_endpoint().is_some_and(|id| {
        !devices
            .clone()
            .any(|d| d.kind == Kind::Keyboard && d.id == id)
    });
    (mouse, keyboard)
}
fn restore_keyboard<T>(
    id: Option<&str>,
    devices: &mut Vec<T>,
    info: impl Fn(&T) -> &Descriptor,
    discover: impl FnOnce(&str) -> Result<Option<T>, String>,
) -> Result<(), String> {
    let Some(id) = id else {
        return Ok(());
    };
    if devices
        .iter()
        .any(|t| info(t).id == id && info(t).kind == Kind::Keyboard)
    {
        return Ok(());
    }
    let target =
        discover(id)?.ok_or("接收器存在，但键盘电量暂不可读 / Keyboard battery unavailable")?;
    if info(&target).id != id || info(&target).kind != Kind::Keyboard {
        return Err("Keyboard identity changed; refresh did not change the binding".into());
    }
    // Scoped discovery has just queried this keyboard. Poll it before other
    // HID targets as well, rather than waiting behind unrelated mouse reads.
    devices.insert(0, target);
    Ok(())
}
impl Target {
    fn data(&self) -> &Descriptor {
        match self {
            Self::H(t, _) => &t.info,
            Self::B(t) => &t.info,
        }
    }
    fn read(&mut self, ms: u64, reset: bool) -> Result<Reading, String> {
        match self {
            Self::H(t, s) => {
                if s.is_none() {
                    *s = Some(t.open()?)
                }
                let h = s.as_mut().unwrap();
                if reset {
                    h.reset_filter()
                }
                let r = h.read(ms)?;
                if let Some(name) = h.name_once() {
                    t.info.name = name;
                }
                Ok(r)
            }
            Self::B(t) => t.read(),
        }
    }
    fn reopen_one(&mut self) -> bool {
        let Self::H(t, session) = self else {
            return false;
        };
        *session = None;
        if let Ok(s) = t.open() {
            *session = Some(s);
            return true;
        }
        if let Ok(Some(fresh)) = t.rediscover()
            && let Ok(s) = fresh.open()
        {
            *t = fresh;
            *session = Some(s);
            return true;
        }
        false
    }
    fn close(&mut self) {
        if let Self::H(_, s) = self {
            *s = None
        }
    }
}
fn run(state: Arc<Mutex<State>>, sig: Arc<Signals>, bt: bool, wake: Arc<dyn Fn() + Send + Sync>) {
    let _apartment = if bt {
        match bluetooth::Apartment::enter() {
            Ok(a) => Some(a),
            Err(e) => {
                // Preserve the request/reply contract even on systems without a usable WinRT apartment.
                state.lock().unwrap().note = e.clone();
                wake();
                loop {
                    let f = sig.bt.take(Duration::from_secs(3600));
                    if f & STOP != 0 {
                        return;
                    }
                    let j = std::mem::take(&mut *sig.bt.jobs.lock().unwrap());
                    let sync = sig.bt.manual.lock().unwrap().take();
                    let mut st = state.lock().unwrap();
                    if let Some((id, _)) = sync {
                        st.manual.finish(id, true, Some(e.clone()));
                    }
                    for j in j {
                        st.finish(&j, Err(e.clone()));
                    }
                    st.note = e.clone();
                    drop(st);
                    wake();
                }
            }
        }
    } else {
        None
    };
    let signal = if bt { &sig.bt } else { &sig.hid };
    let start = Instant::now();
    let clock = || start.elapsed().as_millis() as u64;
    let mut devices: Vec<Target> = Vec::new();
    let mut slots: HashMap<String, Slot> = HashMap::new();
    let mut delay = Duration::ZERO;
    let mut suspended = false;
    let mut recover_only = false;
    let mut recovered = std::collections::HashMap::<String, u64>::new();
    let mut failed_reopens = std::collections::HashSet::new();
    let mut last_fallback = None::<u64>;
    let mut first = true;
    let mut interval_ms = state.lock().unwrap().prefs.poll_ms();
    let mut discovery_at = interval_ms;
    loop {
        let mut flags = signal.take(delay);
        {
            let mut pending = signal.device_scan.lock().unwrap();
            if pending.is_some_and(|due| Instant::now() >= due) {
                *pending = None;
                flags |= SCAN;
            }
        }
        if flags & STOP != 0 {
            break;
        }
        if flags & SUSPEND != 0 {
            suspended = true
        }
        if flags & RESUME != 0 {
            suspended = false;
            recovered.clear();
            failed_reopens.clear();
            last_fallback = None;
            // Reopen cached handles on their owner, not in the power callback. Avoid waking every unbound endpoint.
            for t in &mut devices {
                t.close();
            }
            let t = clock();
            for slot in slots.values_mut() {
                slot.rebaseline = true;
                slot.fail = 0;
                slot.due = t + 1000;
                slot.row.held = false;
                slot.row.error = Some("等待唤醒后读数 / Resuming".into());
            }
            if devices.is_empty() && !bt {
                flags |= SCAN;
            }
        }
        let sync = if flags & SYNC != 0 {
            signal.manual.lock().unwrap().take()
        } else {
            None
        };
        let mut manual_err = None;
        if let Some((_, scan)) = sync {
            flags |= REFRESH;
            if scan {
                flags |= SCAN
            }
        }
        if suspended {
            if let Some((id, _)) = sync {
                state
                    .lock()
                    .unwrap()
                    .manual
                    .finish(id, bt, Some("System suspended".into()));
                wake();
            }
            if flags & JOB != 0 {
                let mut pending = signal.jobs.lock().unwrap();
                let mut st = state.lock().unwrap();
                for j in pending.drain(..) {
                    st.finish(&j, Err("System suspended; retry after wake".into()));
                }
                drop(st);
                wake();
            }
            delay = Duration::from_secs(3600);
            continue;
        }
        let new_interval = state.lock().unwrap().prefs.poll_ms();
        if interval_ms != new_interval {
            interval_ms = new_interval;
            let now = clock();
            for slot in slots.values_mut() {
                slot.set_interval(interval_ms, now);
            }
            discovery_at = now;
        }
        if first {
            if !bt || !state.lock().unwrap().prefs.bluetooth.is_empty() {
                flags |= SCAN
            }
            first = false
        }
        let job = if flags & JOB != 0 {
            let mut q = signal.jobs.lock().unwrap();
            let j = q.pop_front();
            if !q.is_empty() {
                signal.send(JOB)
            }
            j
        } else {
            None
        };
        if job.as_ref().is_some_and(|j| j.bind.is_none()) {
            flags |= SCAN;
        }
        if !bt && flags & REFRESH != 0 && state.lock().unwrap().prefs.keyboard.is_some() {
            flags |= SCAN;
        }
        let mut recover_keyboard = false;
        if clock() >= discovery_at {
            // An existing descriptor can contain obsolete Windows endpoint IDs.
            // Repeatedly querying that descriptor is not discovery/recovery.
            let stale_bt = bt
                && stale_bluetooth(&slots, &state.lock().unwrap().prefs)
                && last_fallback.is_none_or(|last| clock().saturating_sub(last) >= 60_000);
            let missing = missing_devices(
                &state.lock().unwrap().prefs,
                devices.iter().map(Target::data),
                bt,
            );
            let stale_key = !bt && stale_keyboard(&slots, &state.lock().unwrap().prefs);
            if (missing.0 || missing.1 || stale_bt || stale_key) && flags & SCAN == 0 {
                recover_only = true;
                flags |= SCAN;
                if stale_bt {
                    last_fallback = Some(clock());
                }
            }
            recover_keyboard = missing.1;
            discovery_at = clock() + interval_ms;
        }
        let mut keyboard_inventory = None;
        if flags & SCAN != 0 {
            let mut warnings = Vec::new();
            let list: Result<Vec<Target>, String> = if bt {
                bluetooth::discover().map(|ds| ds.into_iter().map(Target::B).collect())
            } else {
                hid::discover_inventory().map(|found| {
                    warnings = found.warnings;
                    keyboard_inventory = Some(Ok(found.keyboards));
                    found
                        .targets
                        .into_iter()
                        .map(|d| Target::H(d, None))
                        .collect()
                })
            };
            let scan_result = list.as_ref().map(|_| ()).map_err(Clone::clone);
            match list {
                Ok(v) => {
                    if recover_only {
                        let mut old = std::mem::take(&mut devices);
                        devices = v
                            .into_iter()
                            .map(|new| {
                                if let Some(i) =
                                    old.iter().position(|t| t.data().id == new.data().id)
                                    && slots.get(&new.data().id).is_some_and(|s| s.fail < 3)
                                {
                                    old.swap_remove(i)
                                } else {
                                    new
                                }
                            })
                            .collect();
                        // Recovery enumeration can be incomplete during login. Keep
                        // existing paths, including healthy devices omitted by this scan.
                        for old_device in old {
                            if !devices.iter().any(|d| d.data().id == old_device.data().id) {
                                devices.push(old_device);
                            }
                        }
                        slots.retain(|id, _| devices.iter().any(|t| t.data().id == *id));
                    } else {
                        devices = v;
                        slots.clear();
                        failed_reopens.clear();
                        recovered.clear();
                        last_fallback = None;
                    }
                    if let Some(Ok(present)) = keyboard_inventory.as_ref() {
                        // Retaining an old session does not make its receiver present.
                        // Missing keyboards must keep discovery on the polling deadline.
                        devices.retain(|t| {
                            t.data().kind != Kind::Keyboard
                                || present.iter().any(|p| p.id == t.data().id)
                        });
                        slots.retain(|id, s| {
                            s.row.device.kind != Kind::Keyboard
                                || present.iter().any(|p| p.id == *id)
                        });
                    }
                    let kinds = |d: &Descriptor| (d.kind == Kind::Bluetooth) == bt;
                    let mut st = state.lock().unwrap();
                    st.candidates.retain(|d| !kinds(d));
                    st.candidates
                        .extend(devices.iter().map(|t| t.data().clone()));
                    let candidates = st.candidates.clone();
                    let mut pref = st.prefs.clone();
                    if pref.resolve(&candidates) {
                        match Prefs::path().and_then(|path| pref.save(&path)) {
                            Ok(()) => st.prefs = pref,
                            Err(e) => {
                                manual_err = Some(e.clone());
                                st.note = e;
                            }
                        }
                    }
                    st.note = if let Some(e) = manual_err.as_ref() {
                        e.clone()
                    } else if warnings.is_empty() {
                        if bt {
                            "蓝牙扫描完成"
                        } else {
                            "接收器扫描完成"
                        }
                        .into()
                    } else {
                        warnings.sort();
                        warnings.dedup();
                        let m = warnings
                            .iter()
                            .take(3)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("；");
                        manual_err = Some(m.clone());
                        m
                    };
                }
                Err(e) => {
                    keyboard_inventory = Some(Err(e.clone()));
                    manual_err = Some(e.clone());
                    state.lock().unwrap().note = e;
                }
            }
            discovery_at = clock() + interval_ms;
            if let Some(j) = job.as_ref().filter(|j| j.bind.is_none()) {
                state.lock().unwrap().finish(j, scan_result);
            }
            wake();
        }
        if let Some(ref j) = job
            && let Some(id) = j.bind.as_ref()
        {
            let out = (|| -> Result<(), String> {
                let t = devices
                    .iter_mut()
                    .find(|t| t.data().id == *id && t.data().kind == j.kind)
                    .ok_or("Device is no longer available")?;
                let sample = t.read(clock(), true)?;
                let mut st = state.lock().unwrap();
                let mut p = st.prefs.clone();
                if !p.change(&Command::Bind(id.clone()), &st.candidates) {
                    return Err("Device identity changed".into());
                }
                p.save(&Prefs::path()?)?;
                st.prefs = p;
                let slot = slots
                    .entry(id.clone())
                    .or_insert_with(|| Slot::new(t.data().clone(), clock()));
                slot.set_interval(interval_ms, clock());
                slot.apply(Ok(sample), clock(), true);
                Ok(())
            })();
            state.lock().unwrap().finish(j, out);
            wake();
        }
        let mut prefs = state.lock().unwrap().prefs.clone();
        recover_only = false;
        let force = flags & REFRESH != 0;
        let mut changed = flags != 0 || recover_keyboard;
        let mut keyboard_blocked = false;
        let mut recovered_sample = None;
        if !bt
            && prefs.keyboard.is_some()
            && let Some(inventory) = keyboard_inventory.as_ref()
        {
            // The complete fresh inventory, never retained sessions or handshake successes,
            // decides whether replacing an absent endpoint is unambiguous.
            let recovery = crate::keyboard::probe(
                &prefs,
                inventory.as_deref().map_err(Clone::clone),
                |connection| {
                    let target = devices
                        .iter_mut()
                        .find(|t| t.data().id == connection.endpoint_id)
                        .ok_or("no responsive keyboard control endpoint")?;
                    target.read(clock(), true)
                },
            );
            let outcome = match recovery {
                Ok(Some((connection, sample))) => {
                    let endpoint = connection.endpoint_id.clone();
                    let mut st = state.lock().unwrap();
                    match crate::keyboard::commit(&mut st, &prefs, connection, |p| {
                        Prefs::path().and_then(|path| p.save(&path))
                    }) {
                        Ok(true) => {
                            prefs = st.prefs.clone();
                            slots.retain(|_, s| s.row.device.kind != Kind::Keyboard);
                            let target = devices.iter().find(|t| t.data().id == endpoint).unwrap();
                            let mut slot = Slot::new(target.data().clone(), clock());
                            slot.set_interval(interval_ms, clock());
                            slot.apply(Ok(sample), clock(), true);
                            slots.insert(endpoint, slot);
                            recovered_sample = prefs.keyboard_endpoint().map(str::to_owned);
                            st.note = "键盘已自动恢复连接 / Keyboard reconnected".into();
                            Ok(())
                        }
                        Ok(false) => {
                            prefs = st.prefs.clone();
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                }
                Ok(None) => restore_keyboard(
                    prefs.keyboard_endpoint(),
                    &mut devices,
                    Target::data,
                    |_| Ok(None),
                ),
                Err(e) => Err(e),
            };
            if let Err(e) = outcome {
                // A failed enumeration forbids replacement, but an already-open
                // endpoint may still provide a fresh valid reading.
                keyboard_blocked = inventory.is_ok();
                if keyboard_blocked {
                    slots.retain(|_, s| s.row.device.kind != Kind::Keyboard);
                }
                state.lock().unwrap().note = e.clone();
                manual_err.get_or_insert(e);
            }
        }
        let due_at = clock();
        let batch = bt.then(|| {
            bluetooth::ReadBatch::prepare(devices.iter().filter_map(|d| match d {
                Target::B(t)
                    if prefs.bound(&t.info)
                        && (force || slots.get(&t.info.id).is_none_or(|s| s.due <= due_at)) =>
                {
                    Some(t)
                }
                _ => None,
            }))
        });
        for t in &mut devices {
            let d = t.data().clone();
            if !transport_bound(&prefs, &d) || (keyboard_blocked && d.kind == Kind::Keyboard) {
                slots.remove(&d.id);
                t.close();
                continue;
            }
            if recovered_sample.as_deref() == Some(d.id.as_str()) {
                continue;
            }
            let slot = slots
                .entry(d.id.clone())
                .or_insert_with(|| Slot::new(d.clone(), due_at));
            slot.set_interval(interval_ms, due_at);
            if !force && slot.due > due_at {
                continue;
            }
            if *signal.bits.lock().unwrap() & (STOP | SUSPEND) != 0 {
                break;
            }
            let r = match (&*t, batch.as_ref()) {
                (Target::B(dev), Some(batch)) => dev.read_in(batch),
                _ => t.read(clock(), force),
            };
            if r.is_ok()
                && d.kind == Kind::Keyboard
                && let Some(Ok(inventory)) = keyboard_inventory.as_ref()
                && let Some(presence) = inventory.iter().find(|p| p.id == d.id)
                && (prefs.keyboard_connection.is_none()
                    || (prefs
                        .keyboard_connection
                        .as_ref()
                        .is_some_and(|c| c.serial.is_none())
                        && presence.serial.is_some()))
            {
                let connection = KeyboardConnection {
                    endpoint_id: d.id.clone(),
                    serial: presence.serial.clone(),
                };
                let mut st = state.lock().unwrap();
                match crate::keyboard::commit(&mut st, &prefs, connection, |p| {
                    Prefs::path().and_then(|path| p.save(&path))
                }) {
                    Ok(_) => prefs = st.prefs.clone(),
                    Err(e) => {
                        st.note = e.clone();
                        manual_err.get_or_insert(e);
                    }
                }
            }
            if sync.is_some()
                && let Err(e) = &r
            {
                manual_err = Some(e.clone())
            }
            slot.row.device = t.data().clone();
            slot.apply(r, clock(), force);
            let key = t.data().id.clone();
            if slot.fail == 0 {
                failed_reopens.remove(&key);
            }
            let now = clock();
            if !bt
                && slot.fail >= 3
                && recovered
                    .get(&key)
                    .is_none_or(|prior| now.saturating_sub(*prior) >= 60_000)
            {
                recovered.insert(key.clone(), now);
                if t.reopen_one() {
                    slot.rebaseline = true;
                    failed_reopens.remove(&key);
                } else {
                    failed_reopens.insert(key);
                }
            }
            changed = true;
        }
        let mut known = devices.iter().filter(|d| transport_bound(&prefs, d.data()));
        if !bt
            && !recover_only
            && known.clone().next().is_some()
            && known.all(|d| failed_reopens.contains(&d.data().id))
            && last_fallback.is_none_or(|t| clock().saturating_sub(t) >= 60_000)
        {
            last_fallback = Some(clock());
            recover_only = true;
            signal.send(SCAN);
        }
        if changed {
            let mut st = state.lock().unwrap();
            publish_rows(&mut st, &slots, bt);
            if let Some((id, _)) = sync {
                st.manual.finish(id, bt, manual_err);
            }
            st.rows.sort_by(|a, b| a.device.id.cmp(&b.device.id));
            drop(st);
            wake();
        }
        let missing = missing_devices(&prefs, devices.iter().map(Target::data), bt);
        let discovery = (missing.0
            || missing.1
            || (bt && stale_bluetooth(&slots, &prefs))
            || (!bt && stale_keyboard(&slots, &prefs)))
        .then_some(discovery_at);
        let next = next_poll(&slots, discovery, clock());
        delay = Duration::from_millis(next.saturating_sub(clock()).max(25));
        if let Some(due) = *signal.device_scan.lock().unwrap() {
            delay = delay.min(due.saturating_duration_since(Instant::now()));
        }
    }
}
fn publish_rows(st: &mut State, slots: &HashMap<String, Slot>, bt: bool) {
    st.rows.retain(|r| (r.device.kind == Kind::Bluetooth) != bt);
    st.rows.extend(slots.iter().filter_map(|(endpoint, slot)| {
        if slot.row.device.kind == Kind::Keyboard
            && st.prefs.keyboard_endpoint() != Some(endpoint.as_str())
        {
            return None;
        }
        let mut row = slot.row.clone();
        row.device = st.prefs.keyboard_row(row.device);
        Some(row)
    }));
    let current = st.prefs.clone();
    current.preserve_missing(&mut st.rows, bt);
    st.prune_unbound();
}
impl Service {
    pub fn start(p: Prefs, demo: bool, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let w: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let state = Arc::new(Mutex::new(if demo {
            State::demo()
        } else {
            State {
                prefs: p,
                ..Default::default()
            }
        }));
        let signals = Arc::new(Signals::default());
        let workers = if demo {
            vec![]
        } else {
            [false, true]
                .into_iter()
                .map(|bt| {
                    let (s, n, w) = (state.clone(), signals.clone(), w.clone());
                    thread::spawn(move || run(s, n, bt, w))
                })
                .collect()
        };
        Self {
            state,
            signals,
            workers,
            demo,
        }
    }
    pub fn device_changed(&self) {
        if !self.demo {
            schedule_device_scan(
                &mut self.signals.hid.device_scan.lock().unwrap(),
                Instant::now(),
            );
            self.signals.hid.send(PREFS);
        }
    }
    pub fn command(&self, c: Command) -> Result<(), String> {
        match c {
            Command::Sync(id, scan) => {
                self.state.lock().unwrap().manual = Manual {
                    id,
                    hid: self.demo,
                    bt: self.demo,
                    error: None,
                };
                if !self.demo {
                    for sig in [&self.signals.hid, &self.signals.bt] {
                        *sig.manual.lock().unwrap() = Some((id, scan));
                        sig.send(SYNC)
                    }
                }
            }
            Command::DeviceRequest(j) => {
                if !matches!(j.kind, Kind::Keyboard | Kind::Bluetooth) {
                    return Err("invalid device class".into());
                }
                if self.demo {
                    self.state.lock().unwrap().demo_request(&j)
                } else {
                    let sig = if j.kind == Kind::Bluetooth {
                        &self.signals.bt
                    } else {
                        &self.signals.hid
                    };
                    let mut q = sig.jobs.lock().unwrap();
                    if q.len() >= 8 {
                        self.state
                            .lock()
                            .unwrap()
                            .finish(&j, Err("Busy; retry".into()));
                    } else {
                        q.push_back(j);
                        drop(q);
                        sig.send(JOB);
                    }
                }
            }
            Command::ImportAppearance => {
                if self.demo {
                    return Err("预览模式不读取旧配置 / Preview does not import preferences".into());
                }
                let target = Prefs::path()?;
                let old = target
                    .parent()
                    .and_then(|p| p.parent())
                    .ok_or("invalid profile path")?
                    .join("WirelessDeviceBatteryMonitor")
                    .join("config.json");
                use std::io::Read;
                let mut bytes = Vec::new();
                std::fs::File::open(&old)
                    .map_err(|e| e.to_string())?
                    .take(4 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() > 4 * 1024 * 1024 {
                    return Err("legacy preferences too large".into());
                }
                let snapshot: serde_json::Value =
                    serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                let mut st = self.state.lock().unwrap();
                let mut prefs = st.prefs.clone();
                prefs.import_appearance_value(&snapshot)?;
                let pending = prefs.import_devices_value(&snapshot, &st.candidates)?;
                prefs.save(&target)?;
                st.prefs = prefs;
                st.prune_unbound();
                st.note = format!(
                    "已导入偏好与匹配的设备；待匹配 {pending} 项 / Import complete; {pending} pending"
                );
                drop(st);
                self.signals.hid.send(SCAN);
                self.signals.bt.send(SCAN);
            }
            Command::Autostart => {
                let mut st = self.state.lock().unwrap();
                if !self.demo {
                    crate::startup::set(!st.autostart)?;
                }
                st.autostart = !st.autostart;
            }
            // A raw Bind has no successful I/O receipt. Only DeviceRequest may commit a binding.
            Command::Bind(_) => {
                return Err("Device binding requires a successful battery read".into());
            }
            Command::Refresh => self.signals.both(REFRESH),
            Command::Scan => self.signals.hid.send(SCAN),
            Command::DiscoverBt => self.signals.bt.send(SCAN),
            _ => {
                let mut s = self.state.lock().unwrap();
                let mut p = s.prefs.clone();
                if !p.change(&c, &s.candidates) {
                    return Err("invalid command or device".into());
                }
                if !self.demo {
                    p.save(&Prefs::path()?)?
                }
                s.prefs = p;
                s.prune_unbound();
                s.demo_reconcile();
                drop(s);
                self.signals.both(PREFS)
            }
        }
        Ok(())
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.signals.both(STOP);
        for j in self.workers.drain(..) {
            let _ = j.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires the original 0C45:FEFA receiver; uses a subprocess with isolated APPDATA"]
    fn live_old_binding_recovers_without_touching_the_user_profile() {
        const TEST_NAME: &str =
            "service::tests::live_old_binding_recovers_without_touching_the_user_profile";
        if std::env::var_os("WDBM_LIVE_PROFILE").is_none() {
            let dir = tempfile::tempdir().unwrap();
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([TEST_NAME, "--exact", "--ignored", "--nocapture"])
                .env("APPDATA", dir.path())
                .env("WDBM_LIVE_PROFILE", dir.path())
                .status()
                .unwrap();
            assert!(status.success(), "isolated hardware recovery failed");
            return;
        }
        let profile = std::path::PathBuf::from(std::env::var_os("WDBM_LIVE_PROFILE").unwrap());
        assert_eq!(std::env::var_os("APPDATA").unwrap(), profile.as_os_str());
        const OLD: &str = "hid:0c45:fefa:Keyboard:container:799872a0d2b611f1b1e1806e6f6e6963";
        let mut prefs = Prefs {
            keyboard: Some(OLD.into()),
            poll_interval_secs: 5,
            ..Default::default()
        };
        prefs.aliases.insert(OLD.into(), "Isolated keyboard".into());
        let svc = Service::start(prefs, false, || {});
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let state = svc.state.lock().unwrap().clone();
            if let Some(row) = state
                .rows
                .iter()
                .find(|r| r.device.id == OLD && r.battery.is_some() && r.error.is_none())
            {
                let loaded = Prefs::load(&Prefs::path().unwrap()).unwrap();
                assert_eq!(loaded.keyboard.as_deref(), Some(OLD));
                assert_ne!(loaded.keyboard_endpoint(), Some(OLD));
                assert_eq!(loaded.aliases[OLD], "Isolated keyboard");
                println!(
                    "LIVE RECOVERY: logical={OLD}; endpoint={}; battery={}%; alias preserved; isolated profile persisted",
                    loaded.keyboard_endpoint().unwrap(),
                    row.battery.unwrap().raw
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "live recovery timed out: {}",
                state.note
            );
            thread::sleep(Duration::from_millis(100));
        }
    }
    #[test]
    fn notification_burst_coalesces_without_postponing_the_first_scan() {
        let now = Instant::now();
        let mut pending = None;
        schedule_device_scan(&mut pending, now);
        for ms in [10, 100, 200, 290] {
            schedule_device_scan(&mut pending, now + Duration::from_millis(ms));
        }
        assert_eq!(pending, Some(now + Duration::from_millis(300)));
    }
    #[test]
    fn failed_keyboard_is_rediscovered_even_while_the_mouse_is_healthy() {
        let state = State::demo();
        let mut keyboard = Slot::new(state.rows[1].device.clone(), 0);
        for t in [0, 30_000, 60_000] {
            keyboard.apply(Err("stale endpoint".into()), t, false);
        }
        let slots = HashMap::from([(keyboard.row.device.id.clone(), keyboard)]);
        assert!(stale_keyboard(&slots, &state.prefs));
        assert_eq!(
            missing_devices(&state.prefs, state.rows.iter().map(|r| &r.device), false),
            (false, false)
        );
    }
    fn slot() -> Slot {
        Slot::new(
            Descriptor {
                id: "a".into(),
                name: "a".into(),
                kind: Kind::Mouse,
                legacy_ids: Vec::new(),
            },
            0,
        )
    }
    #[test]
    fn offline_retries_keep_the_configured_interval() {
        let mut s = slot();
        for (t, want) in [(0, 30000), (30000, 60000), (60000, 90000), (90000, 120000)] {
            s.apply(Err("offline".into()), t, false);
            assert_eq!(s.due, want);
            assert_eq!(s.row.sampled, 0)
        }
    }
    #[test]
    fn startup_without_targets_retries_discovery_before_hour_long_idle() {
        assert_eq!(next_poll(&HashMap::new(), Some(15_000), 0), 15_000);
    }
    #[test]
    fn stale_bluetooth_descriptor_is_rediscovered_even_when_its_id_is_present() {
        let mut state = State::demo();
        let mut s = slot();
        s.row.device.kind = Kind::Bluetooth;
        state.prefs.bluetooth.insert(s.row.device.id.clone());
        let id = s.row.device.id.clone();
        s.fail = 3;
        let slots = HashMap::from([(id.clone(), s)]);
        assert_eq!(
            missing_devices(&state.prefs, slots.values().map(|s| &s.row.device), true),
            (false, false)
        );
        assert!(stale_bluetooth(&slots, &state.prefs));
        state.prefs.bluetooth.remove(&id);
        assert!(!stale_bluetooth(&slots, &state.prefs));
    }
    #[test]
    fn missing_keyboard_discovery_is_not_delayed_by_healthy_mouse() {
        let mut mouse = slot();
        mouse.due = 60_000;
        let slots = HashMap::from([("a".into(), mouse)]);
        assert_eq!(next_poll(&slots, Some(15_000), 0), 15_000);
    }
    #[test]
    fn offline_target_is_retried_within_thirty_seconds() {
        let mut mouse = slot();
        for t in [0, 1000, 3000, 5000, 35_000, 65_000] {
            mouse.apply(Err("receiver not ready at login".into()), t, false);
            assert!(
                mouse.due <= t + 30_000,
                "retry delayed until {} after failure at {t}",
                mouse.due
            );
        }
    }
    #[test]
    fn every_device_polls_online_and_offline_at_each_supported_interval() {
        for kind in [Kind::Mouse, Kind::Keyboard, Kind::Bluetooth] {
            for seconds in POLL_INTERVALS {
                let mut s = slot();
                s.row.device.kind = kind;
                s.set_interval(seconds * 1000, 0);
                for i in 0..12 {
                    let now = i * seconds * 1000;
                    s.apply(Err("offline".into()), now, false);
                    assert_eq!(s.due, now + seconds * 1000);
                }
                let now = s.due;
                s.apply(
                    Ok(
                        Reading::new(73, Charge::Discharging, battery_protocol::Source::Razer)
                            .unwrap(),
                    ),
                    now,
                    false,
                );
                assert_eq!(s.due, now + seconds * 1000);
                assert!(s.row.error.is_none());
                assert_eq!(s.row.battery.unwrap().raw, 73);
            }
        }
    }
    #[test]
    fn changing_interval_wakes_existing_slots_immediately() {
        let mut s = slot();
        s.set_interval(300_000, 0);
        s.apply(Err("offline".into()), 0, false);
        assert_eq!(s.due, 300_000);
        s.set_interval(5000, 1000);
        assert_eq!(s.due, 1000);
        s.apply(Err("offline".into()), 1000, false);
        assert_eq!(s.due, 6000);
        s.set_interval(5000, 2000);
        assert_eq!(
            s.due, 6000,
            "unrelated preference updates must not trigger extra reads"
        );
    }
    #[test]
    fn missing_devices_are_retried_without_losing_healthy_peers() {
        let mut state = State::demo();
        let mut devices = vec![state.rows[0].device.clone()];
        assert_eq!(
            missing_devices(&state.prefs, devices.iter(), false),
            (false, true)
        );
        restore_keyboard(
            state.prefs.keyboard.as_deref(),
            &mut devices,
            |d| d,
            |_| Ok(Some(state.rows[1].device.clone())),
        )
        .unwrap();
        assert_eq!(
            missing_devices(&state.prefs, devices.iter(), false),
            (false, false)
        );
        assert_eq!(
            missing_devices(&state.prefs, std::iter::empty(), false),
            (true, true)
        );
        state.prefs.keyboard = None;
        assert_eq!(
            missing_devices(&state.prefs, devices[1..].iter(), false),
            (false, false)
        );
        assert_eq!(
            missing_devices(&state.prefs, devices.iter(), true),
            (false, false)
        );
        state.prefs.bluetooth.insert("missing-headset".into());
        assert_eq!(
            missing_devices(&state.prefs, devices.iter(), true),
            (true, false)
        );
    }
    #[test]
    fn refresh_restores_bound_keyboard_absent_from_initial_scan() {
        let state = State::demo();
        let keyboard = state.rows[1].device.clone();
        let mut devices = vec![state.rows[0].device.clone()];
        let mut requested = None;
        restore_keyboard(
            state.prefs.keyboard.as_deref(),
            &mut devices,
            |d| d,
            |id| {
                requested = Some(id.to_owned());
                Ok(Some(keyboard.clone()))
            },
        )
        .unwrap();
        assert_eq!(
            requested.as_deref(),
            state.prefs.keyboard.as_deref(),
            "refresh must query the bound keyboard even when it was asleep during discovery"
        );
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0], keyboard);
        assert_eq!(devices[1], state.rows[0].device);
    }

    #[test]
    fn refresh_does_not_probe_unbound_or_already_known_keyboard() {
        let state = State::demo();
        let mut devices = state
            .rows
            .iter()
            .map(|r| r.device.clone())
            .collect::<Vec<_>>();
        restore_keyboard(
            None,
            &mut devices,
            |d| d,
            |_| panic!("no binding to recover"),
        )
        .unwrap();
        restore_keyboard(
            state.prefs.keyboard.as_deref(),
            &mut devices,
            |d| d,
            |_| panic!("known keyboard is read in the normal forced pass"),
        )
        .unwrap();
    }

    #[test]
    fn refresh_reports_missing_keyboard_and_rejects_other_identities() {
        let state = State::demo();
        let original = vec![state.rows[0].device.clone()];
        for result in [
            Ok(None),
            Err("discovery failed".to_owned()),
            Ok(Some(state.rows[0].device.clone())),
        ] {
            let mut devices = original.clone();
            assert!(
                restore_keyboard(
                    state.prefs.keyboard.as_deref(),
                    &mut devices,
                    |d| d,
                    |_| result
                )
                .is_err()
            );
            assert_eq!(devices, original);
        }
    }
    #[test]
    fn sleeping_keyboard_switches_tray_to_remaining_mouse() {
        let mut state = State::demo();
        state.prefs.priority = Priority::Lowest;
        state.rows[0].battery.as_mut().unwrap().raw = 49;
        state.rows[0].battery.as_mut().unwrap().display = 49;
        let mut keyboard = Slot::new(state.rows[1].device.clone(), 0);
        keyboard.apply(
            Ok(Reading::new(35, Charge::Discharging, battery_protocol::Source::Weikav).unwrap()),
            0,
            false,
        );
        state.rows[1] = keyboard.row.clone();
        assert_eq!(state.selected().unwrap().device.kind, Kind::Keyboard);
        keyboard.apply(Err("no keyboard response".into()), 60_000, false);
        state.rows[1] = keyboard.row.clone();
        assert_eq!(state.selected().unwrap().battery.unwrap().display, 49);
        assert!(state.rows[1].error.is_some());
        assert!(state.rows[1].battery.is_none());
        keyboard.apply(
            Ok(Reading::new(73, Charge::Discharging, battery_protocol::Source::Weikav).unwrap()),
            61_000,
            false,
        );
        state.rows[1] = keyboard.row;
        assert!(state.rows[1].error.is_none());
        state.prefs.priority = Priority::Keyboard;
        assert_eq!(state.selected().unwrap().battery.unwrap().display, 73);
        state.rows[1].battery =
            Reading::new(0, Charge::Discharging, battery_protocol::Source::Weikav);
        assert_eq!(
            state.selected().unwrap().battery.unwrap().display,
            0,
            "a genuine online 0% is still valid"
        );
    }
    #[test]
    fn stable() {
        let mut s = slot();
        s.apply(
            Ok(Reading::new(99, Charge::Charging, battery_protocol::Source::Razer).unwrap()),
            0,
            false,
        );
        assert_eq!(s.due, 30000);
        s.apply(
            Ok(Reading::new(99, Charge::Full, battery_protocol::Source::Razer).unwrap()),
            30000,
            false,
        );
        assert_eq!(s.due, 60000)
    }
    #[test]
    fn demo_never_starts_hardware() {
        let s = Service::start(Prefs::default(), true, || ());
        assert!(s.workers.is_empty());
        assert!(s.state.lock().unwrap().demo)
    }

    #[test]
    fn audit_rejected_jump_preserves_online_value_and_moves_deadline() {
        let mut s = slot();
        let read =
            |n| Ok(Reading::new(n, Charge::Discharging, battery_protocol::Source::Razer).unwrap());
        s.apply(read(76), 0, false);
        let old = s.row.clone();
        s.apply(read(2), 60000, false);
        assert!(s.row.error.is_none());
        assert_eq!(s.row.battery, old.battery);
        assert_eq!(s.row.sampled, old.sampled);
        assert!(s.due > 60000);
        let mut st = State::default();
        st.rows.push(s.row.clone());
        assert!(st.selected().is_some());
        s.apply(read(2), 60001, true);
        assert_eq!(s.row.battery.unwrap().raw, 2);
    }
}

#[cfg(test)]
mod audit_power {
    use super::*;

    #[test]
    fn resume_sequences_and_late_user_event() {
        let now = Instant::now();
        let mut p = PowerCycle::default();
        let at = |n| now + Duration::from_secs(n);
        assert_eq!(p.event(4, at(0)), Some(SUSPEND));
        assert_eq!(p.event(7, at(1)), Some(RESUME));
        assert_eq!(p.event(18, at(2)), None);
        assert_eq!(p.event(7, at(70)), None);
        assert_eq!(p.event(4, at(71)), Some(SUSPEND));
        assert_eq!(p.event(18, at(72)), Some(RESUME));
        assert_eq!(p.event(7, at(500)), None);
        assert_eq!(p.event(4, at(501)), Some(SUSPEND));
        assert_eq!(p.event(7, at(502)), Some(RESUME));
        assert_eq!(p.event(7, at(503)), None);
        assert_eq!(p.event(999, at(504)), None);
    }
    #[test]
    fn both_workers_wake_if_only_event_7_arrives() {
        let s = Signals::default();
        s.power(4);
        s.power(7);
        for w in [&s.bt, &s.hid] {
            assert_eq!(w.take(Duration::ZERO), RESUME);
        }
        s.power(18);
        for w in [&s.bt, &s.hid] {
            assert_eq!(w.take(Duration::ZERO), 0);
        }
    }
    #[test]
    fn charging_pad_states_follow_the_configured_interval() {
        let mut state = State::demo();
        let mut s = Slot::new(state.rows.remove(0).device, 0);
        for (flags, t, due) in [
            (0x80, 0, 30_000),
            (0x81, 30_000, 60_000),
            (1, 60_000, 90_000),
        ] {
            s.apply(
                Ok(battery_protocol::codec::hidpp_reading(0x1001, &[0x0e, 0xe3, flags]).unwrap()),
                t,
                false,
            );
            assert_eq!(s.due, due);
        }
    }
    #[test]
    fn offline_bluetooth_never_exposes_last_cached_percentage() {
        let mut st = State::demo();
        let mut d = st.rows.remove(0).device;
        d.kind = Kind::Bluetooth;
        let mut s = Slot::new(d, 0);
        s.apply(
            Ok(Reading::new(76, Charge::Unknown, battery_protocol::Source::Classic).unwrap()),
            0,
            false,
        );
        let accepted = s.row.sampled;
        s.apply(Err("not connected".into()), 60_000, false);
        assert!(s.row.battery.is_none());
        assert_eq!(s.row.sampled, accepted);
        assert!(s.row.error.is_some());
    }
}

#[cfg(test)]
mod formal_review_tests {
    use super::*;
    #[test]
    fn bare_bind_cannot_bypass_a_successful_device_read() {
        let svc = Service::start(Prefs::default(), true, || {});
        let id = "demo:Bluetooth Headset".to_owned();
        assert!(svc.command(Command::Bind(id.clone())).is_err());
        assert!(!svc.state.lock().unwrap().prefs.bluetooth.contains(&id));
    }
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    #[test]
    fn recovered_physical_slot_publishes_once_under_the_stable_logical_key() {
        let mut state = State::demo();
        let logical = state.prefs.keyboard.clone().unwrap();
        let endpoint = "hid:0c45:fefa:Keyboard:container:new";
        state.prefs.keyboard_connection = Some(KeyboardConnection {
            endpoint_id: endpoint.into(),
            serial: None,
        });
        let mut target = state.rows[1].device.clone();
        target.id = endpoint.into();
        let mut current = Slot::new(target, 0);
        current.apply(
            Ok(Reading::new(80, Charge::Discharging, battery_protocol::Source::Weikav).unwrap()),
            0,
            false,
        );
        let old = Slot::new(state.rows[1].device.clone(), 0);
        let slots = HashMap::from([(endpoint.into(), current), (logical.clone(), old)]);
        publish_rows(&mut state, &slots, false);
        let rows: Vec<_> = state
            .rows
            .iter()
            .filter(|r| r.device.kind == Kind::Keyboard)
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].device.id, logical);
        assert_eq!(rows[0].battery.unwrap().raw, 80);
        assert!(state.prefs.change(&Command::Unbind(logical), &[]));
        publish_rows(&mut state, &slots, false);
        assert!(state.rows.iter().all(|r| r.device.kind != Kind::Keyboard));
    }
    #[test]
    fn late_worker_result_cannot_resurrect_unbound_keyboard() {
        let mut st = State::demo();
        let row = st.rows[1].clone();
        let mut slots = HashMap::new();
        slots.insert(
            row.device.id.clone(),
            Slot {
                row,
                fail: 0,
                due: 60000,
                interval_ms: 30_000,
                rebaseline: false,
            },
        );
        st.prefs.keyboard = None;
        publish_rows(&mut st, &slots, false);
        assert!(!st.rows.iter().any(|r| r.device.kind == Kind::Keyboard));
    }
    #[test]
    fn publishing_hid_does_not_drop_bound_bluetooth() {
        let mut st = State::demo();
        st.demo_request(&DeviceJob {
            id: 1,
            kind: Kind::Bluetooth,
            bind: Some("demo:Bluetooth Headset".into()),
        });
        publish_rows(&mut st, &HashMap::new(), false);
        assert!(st.rows.iter().any(|r| r.device.kind == Kind::Bluetooth));
    }
}

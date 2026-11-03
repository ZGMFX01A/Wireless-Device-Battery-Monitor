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
    probes: Vec<u64>,
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
            probes: vec![t + 1000, t + 3000, t + 5000],
            rebaseline: true,
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
                self.probes.clear();
                self.due = t + if self.row.device.kind == Kind::Mouse
                    && r.charge == Charge::Charging
                    && r.raw != 100
                {
                    30000
                } else {
                    60000
                }
            }
            Err(e) => {
                self.rebaseline = true;
                self.row.error = Some(e);
                self.row.held = false;
                // Do not publish a cached Bluetooth percentage as a current HFP/GATT value.
                if self.row.device.kind == Kind::Bluetooth {
                    self.row.battery = None;
                }
                self.retry(t);
            }
        }
    }
    fn retry(&mut self, t: u64) {
        self.fail = self.fail.saturating_add(1);
        self.probes.retain(|x| *x > t);
        self.due = if self.probes.is_empty() {
            t + (60000u64 << self.fail.saturating_sub(1).min(3)).min(300000)
        } else {
            self.probes.remove(0)
        };
    }
}
enum Target {
    H(hid::Target, Option<hid::Session>),
    B(bluetooth::BtTarget),
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
    loop {
        let mut flags = signal.take(delay);
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
                slot.probes = vec![t + 3000, t + 5000];
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
            if scan && !bt {
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
        if flags & SCAN != 0 {
            let mut warnings = Vec::new();
            let list: Result<Vec<Target>, String> = if bt {
                bluetooth::discover().map(|ds| ds.into_iter().map(Target::B).collect())
            } else {
                hid::discover_diagnosed().map(|(ds, notes)| {
                    warnings = notes;
                    ds.into_iter().map(|d| Target::H(d, None)).collect()
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
                        // A transient empty enumeration must not discard the last known
                        // failed path (otherwise it could never be retried without USB events).
                        for old_device in old {
                            if slots
                                .get(&old_device.data().id)
                                .is_some_and(|s| s.fail >= 3)
                                && !devices.iter().any(|d| d.data().id == old_device.data().id)
                            {
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
                    manual_err = Some(e.clone());
                    state.lock().unwrap().note = e;
                }
            }
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
                slot.apply(Ok(sample), clock(), true);
                Ok(())
            })();
            state.lock().unwrap().finish(j, out);
            wake();
        }
        let prefs = state.lock().unwrap().prefs.clone();
        recover_only = false;
        let force = flags & REFRESH != 0;
        let mut changed = flags != 0;
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
            if !prefs.bound(&d) {
                slots.remove(&d.id);
                t.close();
                continue;
            }
            let slot = slots
                .entry(d.id.clone())
                .or_insert_with(|| Slot::new(d, due_at));
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
                    slot.due = now + 1000;
                    slot.probes = vec![now + 3000, now + 5000];
                    failed_reopens.remove(&key);
                } else {
                    failed_reopens.insert(key);
                }
            }
            changed = true;
        }
        let mut known = devices.iter().filter(|d| prefs.bound(d.data()));
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
            st.rows.retain(|r| (r.device.kind == Kind::Bluetooth) != bt);
            st.rows.extend(slots.values().map(|s| s.row.clone()));
            let current = st.prefs.clone();
            current.preserve_missing(&mut st.rows, bt);
            if let Some((id, _)) = sync {
                st.manual.finish(id, bt, manual_err);
            }
            st.rows.sort_by(|a, b| a.device.id.cmp(&b.device.id));
            drop(st);
            wake();
        }
        let next = slots
            .values()
            .map(|s| s.due)
            .min()
            .unwrap_or(clock() + 3600000);
        delay = Duration::from_millis(next.saturating_sub(clock()).max(25));
    }
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
                let mut prefs = self.state.lock().unwrap().prefs.clone();
                prefs.import_appearance(&old)?;
                let list = self.state.lock().unwrap().candidates.clone();
                let pending = prefs.import_devices(&old, &list)?;
                prefs.save(&target)?;
                let mut st = self.state.lock().unwrap();
                st.prefs = prefs;
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
    fn retry() {
        let mut s = slot();
        for (t, want) in [(0, 1000), (1000, 3000), (3100, 5000), (6600, 306600)] {
            s.apply(Err("offline".into()), t, false);
            assert_eq!(s.due, want);
            assert_eq!(s.row.sampled, 0)
        }
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
        assert_eq!(s.due, 90000)
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
    fn charging_pad_bit_selects_slow_poll_even_at_half_capacity() {
        let mut state = State::demo();
        let mut s = Slot::new(state.rows.remove(0).device, 0);
        for (flags, t, due) in [
            (0x80, 0, 30_000),
            (0x81, 30_000, 90_000),
            (1, 90_000, 150_000),
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

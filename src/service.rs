// SPDX-FileCopyrightText: 2026 ZGMFX01A
// SPDX-License-Identifier: LicenseRef-Wireless-Device-Battery-Monitor
// See LICENSE in the repository root for the full terms.

use crate::model::*;
use battery_protocol::{
    BusScope, Charge, Descriptor, ENABLED_VARIANTS, EngineMode, GenericName, Kind, ProbedRoute,
    Reading, Source, bluetooth,
    codec::KYSONA_SLOT_UNPAIRED,
    hid::{
        self, DiscoveryWarning, KeyboardPresence,
        probe::{
            ProbeBatchResult, ProbeCache, ProbeControl, ProbeDiagnosticEntry, ProbeJob,
            ProbeScanOptions, ProbeWorker,
        },
    },
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Condvar, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const SCAN: u16 = 1;
const REFRESH: u16 = 2;
const PREFS: u16 = 4;
const JOB: u16 = 64;
const SYNC: u16 = 128;
const RESUME: u16 = 8;
const STOP: u16 = 16;
const SUSPEND: u16 = 32;
// 探测结果只唤醒所属总线的轮询线程。
const PROBE_RESULT: u16 = 256;
// 手动扫描等待多批探测的总预算。
const MANUAL_SCAN_MAX_WAIT: Duration = Duration::from_millis(4500);
#[derive(Default)]
pub struct Signal {
    bits: Mutex<u16>,
    device_scan: Mutex<Option<Instant>>,
    manual: Mutex<Option<(u64, bool)>>,
    jobs: Mutex<std::collections::VecDeque<DeviceJob>>,
    cv: Condvar,
}
impl Signal {
    fn send(&self, b: u16) {
        let mut f = self.bits.lock().unwrap();
        if b == RESUME {
            *f &= !SUSPEND
        } else if b == SUSPEND {
            *f &= !RESUME
        }
        *f |= b;
        self.cv.notify_one()
    }
    fn reset(&self) -> std::collections::VecDeque<DeviceJob> {
        *self.bits.lock().unwrap() = 0;
        *self.device_scan.lock().unwrap() = None;
        *self.manual.lock().unwrap() = None;
        std::mem::take(&mut *self.jobs.lock().unwrap())
    }
    fn take(&self, d: Duration) -> u16 {
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
    probe_results_hid: Mutex<VecDeque<ProbeBatchResult>>,
    probe_results_bt: Mutex<VecDeque<ProbeBatchResult>>,
    probe_control: Mutex<Option<ProbeControl>>,
}
/// One recovery per wake cycle. A late user-input notification after automatic resume
/// is not a second wake. A fresh suspend always rearms recovery, even inside 10 s.
#[derive(Default)]
struct PowerCycle {
    last: Option<Instant>,
    automatic: bool,
    suspended: bool,
}
impl PowerCycle {
    fn event(&mut self, code: u32, now: Instant) -> Option<u16> {
        match code {
            4 => {
                *self = Self {
                    suspended: true,
                    ..Self::default()
                };
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
        self.suspended = false;
        Some(RESUME)
    }
}
impl Signals {
    pub fn power(&self, code: u32) {
        let mut cycle = self.cycle.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(signal) = cycle.event(code, Instant::now()) {
            if let Some(control) = self.probe_control.lock().unwrap().as_ref() {
                control.set_suspended(cycle.suspended);
            }
            self.both(signal);
        }
    }
    fn both(&self, b: u16) {
        self.hid.send(b);
        self.bt.send(b)
    }
    fn probe_results(&self, scope: BusScope) -> &Mutex<VecDeque<ProbeBatchResult>> {
        match scope {
            BusScope::Usb => &self.probe_results_hid,
            BusScope::Bluetooth => &self.probe_results_bt,
        }
    }
    fn forward_probe_result(&self, batch: ProbeBatchResult) {
        let scope = batch.bus_scope;
        self.probe_results(scope).lock().unwrap().push_back(batch);
        match scope {
            BusScope::Usb => self.hid.send(PROBE_RESULT),
            BusScope::Bluetooth => self.bt.send(PROBE_RESULT),
        }
    }
}

struct ProbeRuntime {
    worker: ProbeWorker,
    forwarder: JoinHandle<()>,
}

impl ProbeRuntime {
    fn start(signals: &Arc<Signals>) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let worker = ProbeWorker::start(tx)?;
        let cycle = signals.cycle.lock().unwrap();
        let control = worker.control();
        control.set_suspended(cycle.suspended);
        let forward_signals = signals.clone();
        let forwarder = thread::Builder::new()
            .name("battery-probe-results".into())
            .spawn(move || {
                for batch in rx {
                    forward_signals.forward_probe_result(batch);
                }
            })?;
        *signals.probe_control.lock().unwrap() = Some(control);
        Ok(Self { worker, forwarder })
    }

    fn stop(self) {
        // 先停止生产者并释放发送端，转发线程随后读完队列退出。
        let _ = self.worker.stop();
        let _ = self.forwarder.join();
    }
}

#[derive(Default)]
struct Workers(
    Mutex<Vec<JoinHandle<Vec<Target>>>>,
    Mutex<Option<ProbeRuntime>>,
);

impl Workers {
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.0.lock().unwrap().is_empty()
    }
    fn probe_runtime_active(&self) -> bool {
        self.1.lock().unwrap().is_some()
    }
}

pub struct Service {
    pub state: Arc<Mutex<State>>,
    pub signals: Arc<Signals>,
    workers: Workers,
    wake: Arc<dyn Fn() + Send + Sync>,
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
fn retains_last_peripheral_sample(source: Source) -> bool {
    matches!(
        source,
        Source::Atk(_)
            | Source::Kysona24g
            | Source::ArdorFury
            | Source::KysonaBluetooth
            | Source::MchoseQhw
            | Source::MchoseMouse
            | Source::AulaLegacy
            | Source::AulaYb
            | Source::AulaVision
            | Source::AulaYxt
            | Source::AulaXxwd
            | Source::AulaBytechMagnetic
            | Source::AulaHfd
            | Source::AulaSparkLink
            | Source::AulaHx
            | Source::AulaJm
            | Source::AulaRlw
            | Source::AulaBytech
            | Source::MchoseAudio
            | Source::HyperX
            | Source::TurtleBeach
            | Source::Scyrox
    )
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
    fn clear_sample(&mut self) {
        self.row.battery = None;
        self.row.error = None;
        self.row.sampled = 0;
        self.row.held = false;
        self.rebaseline = true;
    }
    fn apply(&mut self, r: Result<Reading, String>, t: u64, manual: bool) {
        if let Ok(r) = r
            && !retains_last_peripheral_sample(r.source)
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
                self.row.held = e != KYSONA_SLOT_UNPAIRED && self.has_peripheral_sample();
                self.row.error = Some(e);
                if !self.row.held {
                    self.row.battery = None;
                }
                self.retry(t);
            }
        }
    }
    fn has_peripheral_sample(&self) -> bool {
        self.row
            .battery
            .is_some_and(|reading| retains_last_peripheral_sample(reading.source))
    }
    fn retry(&mut self, t: u64) {
        self.fail = self.fail.saturating_add(1);
        // Offline devices stay in the same polling cycle as online devices.
        self.due = t + self.interval_ms;
    }
}
enum Target {
    H(hid::Target, Option<Box<hid::Session>>),
    B(bluetooth::BtTarget),
}
struct TargetSample {
    reading: Result<Reading, String>,
    association_changed: bool,
}
fn transport_bound(prefs: &Prefs, d: &Descriptor) -> bool {
    if d.kind == Kind::Keyboard {
        prefs.keyboard_endpoint() == Some(d.id.as_str())
    } else {
        prefs.bound(d)
    }
}

fn merge_bluetooth_discovery<T>(
    standard: Result<Vec<T>, String>,
    private: Result<Vec<T>, String>,
) -> (Result<Vec<T>, String>, Option<String>) {
    match (standard, private) {
        (Ok(mut standard), Ok(private)) => {
            standard.extend(private);
            (Ok(standard), None)
        }
        (Ok(standard), Err(error)) => (Ok(standard), Some(error)),
        (Err(error), Ok(private)) if private.is_empty() => (Err(error), None),
        (Err(error), Ok(private)) => (Ok(private), Some(error)),
        (Err(standard), Err(private)) => (Err(format!("{standard}; {private}")), None),
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
    !prefs.keyboard_requires_compatible_mode()
        && prefs
            .keyboard_endpoint()
            .and_then(|id| slots.get(id))
            .is_some_and(|s| s.fail >= 3)
}
fn keyboard_recovery_allowed(prefs: &Prefs) -> bool {
    prefs.keyboard.is_some() && !prefs.keyboard_requires_compatible_mode()
}
fn missing_devices<'a>(
    prefs: &Prefs,
    devices: impl Iterator<Item = &'a Descriptor> + Clone,
    bt: bool,
) -> (bool, bool) {
    if bt {
        return (
            prefs.bluetooth.iter().any(|id| {
                (prefs.unknown_probe_enabled() || !prefs.has_probed_only_binding(id))
                    && !devices
                        .clone()
                        .any(|d| d.kind == Kind::Bluetooth && d.id == *id)
            }),
            false,
        );
    }
    let mouse = !devices.clone().any(|d| d.kind == Kind::Mouse);
    let keyboard = keyboard_recovery_allowed(prefs)
        && prefs.keyboard_endpoint().is_some_and(|id| {
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
    fn probed_route(&self) -> Option<ProbedRoute> {
        match self {
            Self::H(t, _) => t.probed_route(),
            Self::B(_) => None,
        }
    }

    fn data(&self) -> &Descriptor {
        match self {
            Self::H(t, _) => &t.info,
            Self::B(t) => &t.info,
        }
    }
    fn same_endpoint(&self, prior: &Self) -> bool {
        match (self, prior) {
            (Self::H(t, _), Self::H(old, _)) => t.same_endpoint(old),
            (Self::B(t), Self::B(old)) => t.same_endpoint(old),
            _ => false,
        }
    }
    fn retain_identity(&mut self, prior: &Self) {
        match (self, prior) {
            (Self::H(t, _), Self::H(old, _)) => t.retain_identity(old),
            (Self::B(t), Self::B(old)) if t.same_endpoint(old) => {
                t.info.identity.clone_from(&old.info.identity);
            }
            _ => (),
        }
    }
    fn association_changed_from(&self, prior: &Self) -> bool {
        match (self, prior) {
            (Self::H(t, _), Self::H(old, _)) => t.association_changed_from(old),
            (Self::B(t), Self::B(old)) => !t.same_endpoint(old),
            _ => true,
        }
    }
    fn read(&mut self, ms: u64, reset: bool) -> TargetSample {
        let mut association_changed = false;
        let reading = match self {
            Self::H(t, session) => (|| {
                if session.is_none() {
                    *session = Some(Box::new(t.open()?));
                }
                let h = session.as_mut().unwrap();
                if reset {
                    h.reset_filter();
                    h.reset_identity_query();
                }
                let reading = h.read(ms);
                if let Some(update) = h.take_identity_update() {
                    association_changed = t.apply_identity_update(update);
                }
                if reading.is_ok()
                    && let Some(name) = h.name_once()
                {
                    association_changed = t.apply_legacy_name(name);
                }
                reading
            })(),
            Self::B(t) => t.read(),
        };
        TargetSample {
            reading,
            association_changed,
        }
    }
    /// The result reports identity/sample transitions for reopened HID targets.
    fn reopen_one(&mut self) -> Option<TargetTransition> {
        let rediscovered = {
            let Self::H(t, session) = self else {
                return None;
            };
            *session = None;
            if let Ok(s) = t.open() {
                *session = Some(Box::new(s));
                return Some(TargetTransition::unchanged());
            }
            let fresh = t.rediscover().ok()??;
            let s = fresh.open().ok()?;
            Self::H(fresh, Some(Box::new(s)))
        };
        Some(replace_with_reopened(self, rediscovered))
    }
    fn close(&mut self) {
        if let Self::H(_, s) = self {
            *s = None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TargetTransition {
    identity_updated: bool,
    clear_sample: bool,
}

impl TargetTransition {
    const fn unchanged() -> Self {
        Self {
            identity_updated: false,
            clear_sample: false,
        }
    }
}

trait EndpointReconcile {
    fn descriptor(&self) -> &Descriptor;
    fn same_endpoint(&self, prior: &Self) -> bool;
    fn retain_identity(&mut self, prior: &Self);
    fn association_changed_from(&self, prior: &Self) -> bool;
}

impl EndpointReconcile for Target {
    fn descriptor(&self) -> &Descriptor {
        self.data()
    }

    fn same_endpoint(&self, prior: &Self) -> bool {
        Self::same_endpoint(self, prior)
    }

    fn retain_identity(&mut self, prior: &Self) {
        Self::retain_identity(self, prior);
    }

    fn association_changed_from(&self, prior: &Self) -> bool {
        Self::association_changed_from(self, prior)
    }
}

fn reconcile_same_endpoint<T: EndpointReconcile>(new: &mut T, prior: &T) -> TargetTransition {
    new.retain_identity(prior);
    let clear_sample = new.association_changed_from(prior);
    let identity_updated = clear_sample || new.descriptor() != prior.descriptor();
    TargetTransition {
        identity_updated,
        clear_sample,
    }
}

fn merge_discovered_target<T: EndpointReconcile>(
    mut new: T,
    old: &mut Vec<T>,
    slots: &mut HashMap<String, Slot>,
    recover_only: bool,
) -> T {
    if let Some(i) = old.iter().position(|prior| new.same_endpoint(prior)) {
        let mut prior = old.swap_remove(i);
        let transition = reconcile_same_endpoint(&mut new, &prior);
        if recover_only
            && !transition.identity_updated
            && slots.get(&new.descriptor().id).is_some_and(|s| s.fail < 3)
        {
            // 保留已打开句柄时仍同步首次观测到的无描述符变化证据（如目录外 395A:000F）。
            prior.retain_identity(&new);
            return prior;
        }
        if transition.clear_sample {
            slots.remove(&new.descriptor().id);
        } else if transition.identity_updated
            && let Some(slot) = slots.get_mut(&new.descriptor().id)
        {
            slot.row.device = new.descriptor().clone();
        }
    } else {
        slots.remove(&new.descriptor().id);
    }
    new
}

fn replace_with_reopened<T: EndpointReconcile>(
    current: &mut T,
    mut candidate: T,
) -> TargetTransition {
    let transition = if candidate.same_endpoint(current) {
        reconcile_same_endpoint(&mut candidate, current)
    } else {
        TargetTransition {
            identity_updated: true,
            clear_sample: true,
        }
    };
    *current = candidate;
    transition
}

fn apply_reopened_target(device: &Descriptor, slot: &mut Slot, transition: TargetTransition) {
    slot.rebaseline = true;
    if transition.clear_sample {
        slot.clear_sample();
    }
    if transition.identity_updated {
        slot.row.device = device.clone();
    }
}

fn has_exact_identity(d: &Descriptor) -> bool {
    d.generic_name.is_none() && d.identity.as_ref().is_some_and(|i| i.model_key.is_some())
}

fn restore_exact_identity<T: EndpointReconcile>(target: &mut T, prior: &[T]) -> bool {
    let Some(old) = prior.iter().find(|old| target.same_endpoint(old)) else {
        return false;
    };
    if target.descriptor().generic_name.is_some() && !has_exact_identity(target.descriptor()) {
        target.retain_identity(old);
    }
    has_exact_identity(target.descriptor())
}

struct ManualProbeWindow {
    sync_id: Option<u64>,
    job: Option<DeviceJob>,
    deadline: Instant,
    warnings: Vec<DiscoveryWarning>,
    partial_error: Option<Notice>,
    scan_result: Result<(), String>,
}

impl ManualProbeWindow {
    fn new(now: Instant) -> Self {
        Self {
            sync_id: None,
            job: None,
            deadline: now + MANUAL_SCAN_MAX_WAIT,
            warnings: Vec::new(),
            partial_error: None,
            scan_result: Ok(()),
        }
    }

    fn waiting(&self, now: Instant, in_flight: bool, pending: bool) -> bool {
        now < self.deadline && (in_flight || pending)
    }

    fn finish(mut self, st: &mut State, bt: bool) {
        if let Some(id) = self.sync_id {
            self.warnings.sort();
            self.warnings.dedup();
            self.warnings.truncate(3);
            let notice = self.partial_error.or_else(|| {
                (!self.warnings.is_empty()).then_some(Notice::DiscoveryWarnings(self.warnings))
            });
            st.manual.finish(id, bt, notice);
        }
        if let Some(job) = self.job {
            st.finish(&job, self.scan_result);
        }
    }

    fn cancel(self, st: &mut State, bt: bool, notice: Notice, error: &str) {
        if let Some(id) = self.sync_id {
            st.manual.finish(id, bt, Some(notice));
        }
        if let Some(job) = self.job {
            st.finish(&job, Err(error.into()));
        }
    }
}

type KeyboardInventory = Result<Vec<KeyboardPresence>, String>;

fn keyboard_recovery_inventory<'a>(
    prefs: &Prefs,
    unpaired: &[Descriptor],
    inventory: Option<&'a KeyboardInventory>,
) -> Option<&'a KeyboardInventory> {
    inventory.filter(|_| {
        keyboard_recovery_allowed(prefs)
            && !unpaired
                .iter()
                .any(|d| prefs.keyboard_endpoint() == Some(d.id.as_str()))
    })
}

struct CompatibleScan {
    scope: BusScope,
    control: Option<ProbeControl>,
    cache: ProbeCache,
    scan_generation: u64,
    probe_in_flight: Option<u64>,
    manual_probe: Option<ManualProbeWindow>,
    keyboard_inventory: Option<KeyboardInventory>,
    seeded_routes: HashMap<String, ProbedRoute>,
}

/// 两个发现线程各自发布当前总线的诊断，保留另一总线的摘要。
fn publish_probe_diagnostics(st: &mut State, scope: BusScope, entries: Vec<ProbeDiagnosticEntry>) {
    if st.prefs.unknown_probe_enabled() {
        st.probe_diagnostics
            .retain(|entry| entry.bus_scope != scope);
        st.probe_diagnostics.extend(entries);
        st.probe_diagnostics
            .sort_unstable_by_key(ProbeDiagnosticEntry::sort_key);
    }
}

impl CompatibleScan {
    fn new(scope: BusScope, control: Option<ProbeControl>) -> Self {
        Self {
            scope,
            control,
            cache: ProbeCache::default(),
            scan_generation: 0,
            probe_in_flight: None,
            manual_probe: None,
            keyboard_inventory: None,
            seeded_routes: HashMap::new(),
        }
    }

    fn advance(&mut self, signals: &Signals) {
        self.invalidate_batch(signals);
        self.keyboard_inventory = None;
    }

    fn invalidate_batch(&mut self, signals: &Signals) {
        self.scan_generation += 1;
        if let Some(control) = &self.control {
            control.set_generation(self.scope, self.scan_generation);
        }
        self.probe_in_flight = None;
        signals.probe_results(self.scope).lock().unwrap().clear();
    }

    fn seed(&mut self, prefs: &Prefs, reset: bool) {
        let routes: HashMap<_, _> = prefs
            .bound_probed_routes()
            .into_iter()
            .filter(|(id, _)| prefs.bluetooth.contains(id) == (self.scope == BusScope::Bluetooth))
            .collect();
        // core 按总线清理种子。绑定换路或移除时同步淘汰旧快照。
        if reset
            || self
                .seeded_routes
                .iter()
                .any(|(id, route)| routes.get(id) != Some(route))
        {
            self.cache.reset(self.scope);
        }
        for (id, route) in &routes {
            self.cache.seed_bound_route(id.clone(), *route);
        }
        self.seeded_routes = routes;
    }

    fn options(&mut self) -> ProbeScanOptions<'_> {
        ProbeScanOptions {
            mode: EngineMode::Compatible,
            cache: &mut self.cache,
            generation: self.scan_generation,
            enabled: ENABLED_VARIANTS,
        }
    }

    fn submit(&mut self, job: Option<ProbeJob>) {
        if self.probe_in_flight.is_none()
            && let (Some(control), Some(job)) = (&self.control, job)
            && control.submit(job)
        {
            self.probe_in_flight = Some(self.scan_generation);
        }
    }

    fn commit(&mut self, batch: &ProbeBatchResult) -> bool {
        self.commit_current(batch.generation, |cache| cache.commit(batch))
    }

    fn commit_current(
        &mut self,
        generation: u64,
        commit: impl FnOnce(&mut ProbeCache) -> bool,
    ) -> bool {
        if self.probe_in_flight != Some(generation) || generation != self.scan_generation {
            return false;
        }
        self.probe_in_flight = None;
        commit(&mut self.cache)
    }

    fn waiting(&self, now: Instant) -> bool {
        self.manual_probe.as_ref().is_some_and(|window| {
            window.waiting(
                now,
                self.probe_in_flight.is_some(),
                self.control.is_some() && self.cache.has_pending(self.scope),
            )
        })
    }

    fn awaiting_generation(&self, now: Instant) -> bool {
        self.probe_in_flight.is_some() || self.waiting(now)
    }

    fn budget_expired(&self, now: Instant) -> bool {
        self.manual_probe
            .as_ref()
            .is_some_and(|window| now >= window.deadline)
    }

    fn take_ready_window(&mut self, signals: &Signals, now: Instant) -> Option<ManualProbeWindow> {
        if self.waiting(now) {
            return None;
        }
        if self.budget_expired(now) && self.probe_in_flight.is_some() {
            // 预算用完后作废未返回的批次，已提交的缓存留给后续退避扫描。
            self.invalidate_batch(signals);
        }
        self.manual_probe.take()
    }

    fn next_probe(&self, now: Instant) -> Option<Instant> {
        if self.probe_in_flight.is_some() || self.control.is_none() {
            return None;
        }
        self.cache.next_probe_after_in(self.scope, now)
    }
}

fn probe_wake_delay(
    now: Instant,
    probe_due: Option<Instant>,
    deadline: Option<Instant>,
) -> Option<Duration> {
    let retry = probe_due.map(|due| {
        due.saturating_duration_since(now)
            .max(Duration::from_millis(25))
    });
    retry
        .into_iter()
        .chain(deadline.map(|due| due.saturating_duration_since(now)))
        .min()
}

fn merge_keyboard_presence(inventory: &mut Vec<KeyboardPresence>, added: Vec<KeyboardPresence>) {
    for presence in added {
        if let Some(prior) = inventory.iter_mut().find(|p| p.id == presence.id) {
            if prior.serial.is_none() {
                prior.serial = presence.serial;
            }
        } else {
            inventory.push(presence);
        }
    }
}

fn prune_absent_keyboards(
    devices: &mut Vec<Target>,
    slots: &mut HashMap<String, Slot>,
    present: &[KeyboardPresence],
) {
    devices
        .retain(|t| t.data().kind != Kind::Keyboard || present.iter().any(|p| p.id == t.data().id));
    slots
        .retain(|id, s| s.row.device.kind != Kind::Keyboard || present.iter().any(|p| p.id == *id));
}

fn run(
    state: Arc<Mutex<State>>,
    sig: Arc<Signals>,
    bt: bool,
    mode: EngineMode,
    unknown_probe_enabled: bool,
    wake: Arc<dyn Fn() + Send + Sync>,
    mut prior_exact: Vec<Target>,
) -> Vec<Target> {
    let (_apartment, bluetooth_initialization_error) = if bt {
        match bluetooth::Apartment::enter() {
            Ok(a) => (Some(a), None),
            Err(e) => (None, Some(e)),
        }
    } else {
        (None, None)
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
    let scope = if bt {
        BusScope::Bluetooth
    } else {
        BusScope::Usb
    };
    let mut compatible = unknown_probe_enabled
        .then(|| sig.probe_control.lock().unwrap().clone())
        .map(|control| CompatibleScan::new(scope, control));
    let mut discovery_at = interval_ms;
    let mut unpaired = Vec::<Descriptor>::new();
    loop {
        let mut flags = signal.take(delay);
        let hotplug = {
            let mut pending = signal.device_scan.lock().unwrap();
            let due = pending.is_some_and(|due| Instant::now() >= due);
            if due {
                *pending = None;
                flags |= SCAN;
            }
            due
        };
        if flags & STOP != 0 {
            if let Some(window) = compatible.as_mut().and_then(|p| p.manual_probe.take()) {
                window.cancel(
                    &mut state.lock().unwrap(),
                    bt,
                    Notice::OperationFailed,
                    "Recognition mode changed",
                );
            }
            break;
        }
        if flags & SUSPEND != 0 {
            suspended = true;
            if let Some(p) = compatible.as_mut() {
                p.advance(&sig);
                if let Some(window) = p.manual_probe.take() {
                    window.cancel(
                        &mut state.lock().unwrap(),
                        bt,
                        Notice::Suspended,
                        "System suspended; retry after wake",
                    );
                    flags |= JOB;
                    wake();
                }
            }
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
                slot.row.held = slot.has_peripheral_sample();
                slot.row.error = Some("Resuming".into());
            }
            if devices.is_empty() && !bt {
                flags |= SCAN;
            }
            if compatible.is_some() {
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
                    .finish(id, bt, Some(Notice::Suspended));
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
        let job = if flags & JOB != 0
            && !compatible
                .as_ref()
                .is_some_and(|p| p.manual_probe.as_ref().is_some_and(|w| w.job.is_some()))
        {
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
        if flags & REFRESH != 0 {
            let st = state.lock().unwrap();
            if compatible.is_some()
                || (!bt
                    && st.prefs.keyboard.is_some()
                    && !st.prefs.keyboard_requires_compatible_mode())
            {
                flags |= SCAN;
            }
        }
        // 主动重扫和热插拔清除旧消歧结果，周期退避仍复用当前缓存。
        let reset_probe_cache = hotplug || (flags & SCAN != 0 && !recover_only);
        let mut keyboard_inventory = None;
        let mut probe_updated = false;
        if let Some(p) = compatible.as_mut()
            && flags & PROBE_RESULT != 0
            && flags & SCAN == 0
        {
            let batches = std::mem::take(&mut *sig.probe_results(scope).lock().unwrap());
            for batch in batches {
                if !p.commit(&batch) {
                    continue;
                }
                probe_updated = true;
                if bt
                    && !batch.targets.is_empty()
                    && let Some(window) = p.manual_probe.as_mut()
                {
                    // 标准蓝牙枚举失败时，异步私有蓝牙结果仍可完成发现。
                    window.scan_result = Ok(());
                }
                if let Some(Ok(present)) = p.keyboard_inventory.as_mut() {
                    merge_keyboard_presence(present, batch.probed_keyboards);
                }
                let prefs = &state.lock().unwrap().prefs;
                for target in batch.targets {
                    let target = merge_discovered_target(
                        Target::H(target, None),
                        &mut devices,
                        &mut slots,
                        true,
                    );
                    let d = target.data();
                    if transport_bound(prefs, d) {
                        let slot = slots
                            .entry(d.id.clone())
                            .or_insert_with(|| Slot::new(d.clone(), clock()));
                        slot.row.device = d.clone();
                    }
                    devices.retain(|prior| prior.data().id != target.data().id);
                    devices.push(target);
                }
            }
        }
        let mut continuation = false;
        if let Some(p) = compatible.as_mut()
            && flags & SCAN == 0
        {
            let now = Instant::now();
            if p.next_probe(now).is_some_and(|due| due <= now) && (!probe_updated || p.waiting(now))
            {
                continuation = p.waiting(now);
                flags |= SCAN;
                recover_only = true;
            }
            if flags & SCAN == 0 && p.probe_in_flight.is_none() && !p.waiting(now) {
                keyboard_inventory = p.keyboard_inventory.take();
                if let Some(Ok(present)) = keyboard_inventory.as_ref() {
                    prune_absent_keyboards(&mut devices, &mut slots, present);
                }
            }
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
            let probe_pending = compatible
                .as_ref()
                .is_some_and(|p| p.awaiting_generation(Instant::now()));
            if (missing.0 || missing.1 || stale_bt || stale_key)
                && flags & SCAN == 0
                && !probe_pending
            {
                recover_only = true;
                flags |= SCAN;
                if stale_bt {
                    last_fallback = Some(clock());
                }
            }
            recover_keyboard = missing.1;
            discovery_at = clock() + interval_ms;
        }
        if flags & SCAN != 0 {
            if let Some(p) = compatible.as_mut() {
                if !continuation {
                    p.advance(&sig);
                    p.seed(
                        &state.lock().unwrap().prefs,
                        reset_probe_cache || flags & (REFRESH | RESUME) != 0,
                    );
                }
                let scan_sync = sync.filter(|(_, scan)| *scan).map(|(id, _)| id);
                let scan_job = job.as_ref().filter(|j| j.bind.is_none());
                if scan_sync.is_some() || scan_job.is_some() {
                    let window = p
                        .manual_probe
                        .get_or_insert_with(|| ManualProbeWindow::new(Instant::now()));
                    if let Some(id) = scan_sync {
                        window.sync_id = Some(id);
                    }
                    if let Some(job) = scan_job {
                        window.job = Some(job.clone());
                    }
                }
            }
            let mut warnings = Vec::new();
            let list: Result<Vec<Target>, String> = if bt {
                let standard = if continuation {
                    Ok(Vec::new())
                } else {
                    match bluetooth_initialization_error.as_ref() {
                        Some(error) => Err(error.clone()),
                        None => {
                            bluetooth::discover().map(|ds| ds.into_iter().map(Target::B).collect())
                        }
                    }
                };
                let private = if let Some(p) = compatible.as_mut() {
                    match hid::discover_private_bluetooth_compatible(p.options()) {
                        Ok((targets, job)) => {
                            p.submit(job);
                            Ok(targets)
                        }
                        Err(error) => {
                            p.cache.reset(scope);
                            Err(error)
                        }
                    }
                } else {
                    hid::discover_private_bluetooth_in_mode(mode)
                }
                .map(|ds| ds.into_iter().map(|t| Target::H(t, None)).collect());
                let (targets, partial_error) = merge_bluetooth_discovery(standard, private);
                if let Some(error) = partial_error {
                    recover_only = true;
                    eprintln!("Bluetooth discovery incomplete: {error}");
                    manual_err.get_or_insert(Notice::BluetoothScanFailed);
                }
                targets
            } else {
                let found = if let Some(p) = compatible.as_mut() {
                    match hid::discover_inventory_compatible(p.options()) {
                        Ok((found, job)) => {
                            p.submit(job);
                            Ok(found)
                        }
                        Err(error) => {
                            p.cache.reset(scope);
                            Err(error)
                        }
                    }
                } else {
                    hid::discover_inventory_with_diagnostics_in_mode(mode)
                };
                found.map(|found| {
                    warnings = found.warnings;
                    keyboard_inventory = Some(Ok(found.keyboards));
                    unpaired = found.unpaired;
                    found
                        .targets
                        .into_iter()
                        .map(|d| Target::H(d, None))
                        .collect()
                })
            };
            let scan_result = list.as_ref().map(|_| ()).map_err(Clone::clone);
            if let Some(window) = compatible.as_mut().and_then(|p| p.manual_probe.as_mut()) {
                window.warnings.extend(warnings.iter().copied());
                if let Err(error) = &scan_result {
                    window.scan_result = Err(error.clone());
                }
            }
            let keep_pending_keyboard = !bt
                && compatible.as_ref().is_some_and(|p| {
                    p.probe_in_flight.is_some()
                        || (scan_result.is_ok() && p.waiting(Instant::now()))
                });
            match list {
                Ok(v) => {
                    let mut old = std::mem::take(&mut devices);
                    devices = v
                        .into_iter()
                        .map(|new| merge_discovered_target(new, &mut old, &mut slots, recover_only))
                        .collect();
                    if recover_only || keep_pending_keyboard {
                        // Recovery enumeration can be incomplete during login. Keep
                        // existing paths, including healthy devices omitted by this scan.
                        for old_device in old {
                            if (recover_only || old_device.data().kind == Kind::Keyboard)
                                && !devices.iter().any(|d| d.data().id == old_device.data().id)
                                && !unpaired.iter().any(|d| d.id == old_device.data().id)
                            {
                                devices.push(old_device);
                            }
                        }
                    }
                    if recover_only {
                        slots.retain(|id, _| devices.iter().any(|t| t.data().id == *id));
                    } else {
                        slots.retain(|id, slot| {
                            (slot.has_peripheral_sample()
                                || (keep_pending_keyboard
                                    && slot.row.device.kind == Kind::Keyboard))
                                && devices.iter().any(|t| t.data().id == *id)
                        });
                        for slot in slots.values_mut() {
                            slot.due = clock();
                            slot.fail = 0;
                        }
                        failed_reopens.clear();
                        recovered.clear();
                        last_fallback = None;
                    }
                    if !keep_pending_keyboard && let Some(Ok(present)) = keyboard_inventory.as_ref()
                    {
                        // Retaining an old session does not make its receiver present.
                        // Missing keyboards must keep discovery on the polling deadline.
                        prune_absent_keyboards(&mut devices, &mut slots, present);
                    }
                    let mut st = state.lock().unwrap();
                    // 只继承本轮仍能确认同一端点的精确身份，旧会话和读数随线程退出。
                    if !prior_exact.is_empty() {
                        for target in &mut devices {
                            if restore_exact_identity(target, &prior_exact)
                                && transport_bound(&st.prefs, target.data())
                            {
                                let d = target.data();
                                slots
                                    .entry(d.id.clone())
                                    .or_insert_with(|| Slot::new(d.clone(), clock()));
                            }
                        }
                        prior_exact.clear();
                    }
                    update_candidates(&mut st, &devices, bt);
                    for target in &devices {
                        let d = target.data();
                        if let Some(slot) = slots.get_mut(&d.id) {
                            slot.row.device = d.clone();
                        }
                    }
                    let candidates = st.candidates.clone();
                    let mut pref = st.prefs.clone();
                    if pref.resolve(&candidates) {
                        match Prefs::path().and_then(|path| pref.save(&path)) {
                            Ok(()) => st.prefs = pref,
                            Err(e) => {
                                eprintln!("Failed to save discovered device preferences: {e}");
                                manual_err = Some(Notice::SettingsSaveFailed);
                            }
                        }
                    }
                    st.note = Some(if let Some(e) = manual_err.as_ref() {
                        e.clone()
                    } else if warnings.is_empty() {
                        if bt {
                            Notice::BluetoothScanComplete
                        } else {
                            Notice::HidScanComplete
                        }
                    } else {
                        warnings.sort();
                        warnings.dedup();
                        warnings.truncate(3);
                        let m = Notice::DiscoveryWarnings(warnings);
                        manual_err = Some(m.clone());
                        m
                    });
                    publish_device_rows(&mut st, &slots, &devices, &unpaired, &[], bt);
                }
                Err(e) => {
                    keyboard_inventory = Some(Err(e.clone()));
                    let notice = if bt {
                        eprintln!("Bluetooth scan failed: {e}");
                        Notice::BluetoothScanFailed
                    } else {
                        eprintln!("Receiver scan failed: {e}");
                        Notice::HidScanFailed
                    };
                    manual_err = Some(notice.clone());
                    state.lock().unwrap().note = Some(notice);
                }
            }
            discovery_at = clock() + interval_ms;
            if let Some(p) = compatible.as_mut() {
                if keep_pending_keyboard {
                    p.keyboard_inventory = keyboard_inventory.take();
                } else {
                    p.keyboard_inventory = None;
                }
            } else if let Some(j) = job.as_ref().filter(|j| j.bind.is_none()) {
                state.lock().unwrap().finish(j, scan_result);
            }
        }
        if let Some(p) = compatible.as_ref()
            && (flags & SCAN != 0 || probe_updated)
        {
            let entries = p.cache.diagnostics(scope);
            publish_probe_diagnostics(&mut state.lock().unwrap(), scope, entries);
        }
        if flags & SCAN != 0 && compatible.as_ref().is_none_or(|p| p.manual_probe.is_none()) {
            wake();
        }
        if let Some(ref j) = job
            && let Some(id) = j.bind.as_ref()
        {
            let out = (|| -> Result<(), String> {
                let before = state.lock().unwrap().prefs.clone();
                let t = devices
                    .iter_mut()
                    .find(|t| t.data().id == *id && t.data().kind == j.kind)
                    .ok_or("Device is no longer available")?;
                let sample = t.read(clock(), true);
                let mut st = state.lock().unwrap();
                let slot = slots
                    .entry(id.clone())
                    .or_insert_with(|| Slot::new(t.data().clone(), clock()));
                slot.set_interval(interval_ms, clock());
                let error = sample.reading.as_ref().err().cloned();
                let probed_route = t.probed_route();
                let association_replaced = generic_association_changed(mode, t.data(), &sample);
                apply_target_sample(slot, t.data(), sample, clock(), true);
                update_candidates(&mut st, &devices, bt);
                let outcome = (|| {
                    if let Some(error) = error {
                        return Err(error);
                    }
                    if !same_binding(&before, &st.prefs, j.kind, id) {
                        return Err("Device binding changed while reading".into());
                    }
                    let mut p = st.prefs.clone();
                    if !p.change(&Command::Bind(id.clone()), &st.candidates) {
                        return Err("Device identity changed".into());
                    }
                    if association_replaced {
                        p.clear_identity(&slot.row.device);
                    }
                    if probed_route.is_none() {
                        p.clear_probed_route(&slot.row.device);
                    }
                    p.remember_identity_with_route(&slot.row.device, probed_route);
                    p.save(&Prefs::path()?)?;
                    st.prefs = p;
                    Ok(())
                })();
                publish_device_rows(&mut st, &slots, &devices, &unpaired, &[], bt);
                outcome
            })();
            state.lock().unwrap().finish(j, out);
            wake();
        }
        let mut prefs = state.lock().unwrap().prefs.clone();
        recover_only = false;
        let force = flags & REFRESH != 0;
        let mut changed = flags != 0 || recover_keyboard || keyboard_inventory.is_some();
        let mut keyboard_blocked = false;
        let mut recovered_sample = None;
        let mut changed_associations = Vec::new();
        if !bt
            && let Some(inventory) =
                keyboard_recovery_inventory(&prefs, &unpaired, keyboard_inventory.as_ref())
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
                    let sample = target.read(clock(), true);
                    if generic_association_changed(mode, target.data(), &sample) {
                        changed_associations.push(target.data().clone());
                    }
                    sample.reading
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
                            st.note = Some(Notice::KeyboardReconnected);
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
                // 兼容模式的已确认路由在设备离线时保留，等待下次枚举恢复。
                if !prefs
                    .keyboard_endpoint()
                    .is_some_and(|id| prefs.has_probed_only_binding(id))
                {
                    eprintln!("Keyboard reconnect failed: {e}");
                    state.lock().unwrap().note = Some(Notice::KeyboardReconnectFailed);
                    manual_err.get_or_insert(Notice::KeyboardReconnectFailed);
                }
            }
        }
        let due_at = clock();
        let batch = (bt && _apartment.is_some()).then(|| {
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
            if compatible
                .as_ref()
                .is_some_and(|p| p.budget_expired(Instant::now()))
            {
                break;
            }
            let sample = match (&mut *t, batch.as_ref()) {
                (Target::B(dev), Some(batch)) => TargetSample {
                    reading: dev.read_in(batch),
                    association_changed: false,
                },
                _ => t.read(clock(), force),
            };
            if sample.reading.is_ok()
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
                        eprintln!("Failed to save restored keyboard preferences: {e}");
                        st.note = Some(Notice::SettingsSaveFailed);
                        manual_err.get_or_insert(Notice::SettingsSaveFailed);
                    }
                }
            }
            if (sync.is_some()
                || compatible
                    .as_ref()
                    .is_some_and(|p| p.manual_probe.as_ref().is_some_and(|w| w.sync_id.is_some())))
                && let Err(e) = &sample.reading
            {
                eprintln!("Battery refresh failed: {e}");
                manual_err = Some(Notice::RefreshFailed);
            }
            if generic_association_changed(mode, t.data(), &sample) {
                changed_associations.push(t.data().clone());
            }
            apply_target_sample(slot, t.data(), sample, clock(), force);
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
                if let Some(transition) = t.reopen_one() {
                    apply_reopened_target(t.data(), slot, transition);
                    failed_reopens.remove(&key);
                } else {
                    failed_reopens.insert(key);
                }
            }
            changed = true;
        }
        let mut category_pending = false;
        for target in &mut devices {
            if let Target::B(t) = target {
                if t.poll_category() {
                    if let Some(slot) = slots.get_mut(&t.info.id) {
                        slot.row.device = t.info.clone();
                    }
                    changed = true;
                }
                category_pending |= t.category_pending();
            }
        }
        let mut known = devices.iter().filter(|d| transport_bound(&prefs, d.data()));
        if !bt
            && !recover_only
            && !compatible
                .as_ref()
                .is_some_and(|p| p.awaiting_generation(Instant::now()))
            && known.clone().next().is_some()
            && known.all(|d| failed_reopens.contains(&d.data().id))
            && last_fallback.is_none_or(|t| clock().saturating_sub(t) >= 60_000)
        {
            last_fallback = Some(clock());
            recover_only = true;
            signal.send(SCAN);
        }
        let mut defer_scan_wake = false;
        if let Some(p) = compatible.as_mut() {
            let waiting = p.waiting(Instant::now());
            if let Some(window) = p.manual_probe.as_mut() {
                if let Some(notice) = &manual_err
                    && !matches!(notice, Notice::DiscoveryWarnings(_))
                {
                    window.partial_error = Some(notice.clone());
                }
                defer_scan_wake = waiting && flags & (SCAN | PROBE_RESULT) != 0;
            }
            if !waiting && p.manual_probe.is_some() {
                changed = true;
            }
        }
        if changed {
            let mut st = state.lock().unwrap();
            update_candidates(&mut st, &devices, bt);
            publish_device_rows(
                &mut st,
                &slots,
                &devices,
                &unpaired,
                &changed_associations,
                bt,
            );
            if let Some((id, scan)) = sync
                && !(scan && compatible.is_some())
            {
                st.manual.finish(id, bt, manual_err);
            }
            if let Some(window) = compatible
                .as_mut()
                .and_then(|p| p.take_ready_window(&sig, Instant::now()))
            {
                let resume_jobs = window.job.is_some();
                window.finish(&mut st, bt);
                if resume_jobs {
                    signal.send(JOB);
                }
                defer_scan_wake = false;
            }
            drop(st);
            if !defer_scan_wake {
                wake();
            }
        }
        let missing = missing_devices(&prefs, devices.iter().map(Target::data), bt);
        let discovery = (missing.0
            || missing.1
            || (bt && stale_bluetooth(&slots, &prefs))
            || (!bt && stale_keyboard(&slots, &prefs)))
        .then_some(discovery_at);
        let next = next_poll(&slots, discovery, clock());
        delay = Duration::from_millis(next.saturating_sub(clock()).max(25));
        if category_pending {
            delay = delay.min(Duration::from_millis(100));
        }
        if let Some(due) = *signal.device_scan.lock().unwrap() {
            delay = delay.min(due.saturating_duration_since(Instant::now()));
        }
        if let Some(p) = compatible.as_ref() {
            let now = Instant::now();
            if let Some(wait) = probe_wake_delay(
                now,
                p.next_probe(now),
                p.manual_probe.as_ref().map(|w| w.deadline),
            ) {
                delay = delay.min(wait);
            }
        }
    }
    prior_exact.extend(devices.into_iter().filter_map(|mut target| {
        if slots.contains_key(&target.data().id) && has_exact_identity(target.data()) {
            target.close();
            Some(target)
        } else {
            None
        }
    }));
    prior_exact
}

fn generic_association_changed(mode: EngineMode, d: &Descriptor, sample: &TargetSample) -> bool {
    mode == EngineMode::Compatible
        && sample.association_changed
        && matches!(
            d.generic_name,
            Some(GenericName::Receiver24g { brand: None, .. })
        )
}

fn apply_target_sample(
    slot: &mut Slot,
    device: &Descriptor,
    sample: TargetSample,
    now: u64,
    manual: bool,
) {
    if sample.association_changed {
        slot.clear_sample();
    }
    slot.row.device = device.clone();
    slot.apply(sample.reading, now, manual);
}
fn same_binding(before: &Prefs, current: &Prefs, kind: Kind, id: &str) -> bool {
    match kind {
        Kind::Keyboard => {
            before.keyboard == current.keyboard
                && before.keyboard_connection == current.keyboard_connection
        }
        Kind::Bluetooth => before.bluetooth.contains(id) == current.bluetooth.contains(id),
        _ => false,
    }
}
fn update_candidates(st: &mut State, devices: &[Target], bt: bool) {
    st.candidates.retain(|d| (d.kind == Kind::Bluetooth) != bt);
    st.candidates
        .extend(devices.iter().map(|t| t.data().clone()));
    st.candidates.sort_by(|a, b| a.id.cmp(&b.id));
}
#[cfg(test)]
fn remember_slot_identities(
    prefs: &mut Prefs,
    slots: &HashMap<String, Slot>,
    unpaired: &[Descriptor],
    changed_associations: &[Descriptor],
) -> bool {
    remember_slot_identities_with_routes(prefs, slots, &[], unpaired, changed_associations)
}

fn remember_slot_identities_with_routes(
    prefs: &mut Prefs,
    slots: &HashMap<String, Slot>,
    devices: &[Target],
    unpaired: &[Descriptor],
    changed_associations: &[Descriptor],
) -> bool {
    let mut changed = false;
    for device in unpaired.iter().chain(changed_associations) {
        changed |= prefs.clear_identity(device);
    }
    for slot in slots.values() {
        let row = &slot.row;
        let target = devices
            .iter()
            .find(|target| target.data().id == row.device.id);
        let route = target.and_then(Target::probed_route);
        if row.error.as_deref() == Some(KYSONA_SLOT_UNPAIRED) {
            changed |= prefs.clear_identity(&row.device);
            continue;
        }
        // 有现行目录目标时，淘汰同端点旧的探测路由，离线占位不参与。
        if target.is_some() && route.is_none() {
            changed |= prefs.clear_probed_route(&row.device);
        }
        if route.is_some()
            || row.battery.is_some()
            || (row.device.kind == Kind::Bluetooth
                && row
                    .device
                    .identity
                    .as_ref()
                    .is_some_and(|i| i.category != battery_protocol::DeviceCategory::Unknown))
        {
            if route.is_some() {
                changed |= prefs.remember_identity_with_route(&row.device, route);
            } else {
                changed |= prefs.remember_identity(&row.device);
            }
        }
    }
    changed
}

fn publish_device_rows(
    st: &mut State,
    slots: &HashMap<String, Slot>,
    devices: &[Target],
    unpaired: &[Descriptor],
    changed_associations: &[Descriptor],
    bt: bool,
) {
    let mut prefs = st.prefs.clone();
    let changed = remember_slot_identities_with_routes(
        &mut prefs,
        slots,
        devices,
        unpaired,
        changed_associations,
    );
    if changed {
        match Prefs::path().and_then(|path| prefs.save(&path)) {
            Ok(()) => st.prefs = prefs,
            Err(error) => {
                eprintln!("Failed to save device identity: {error}");
                st.note = Some(Notice::SettingsSaveFailed);
            }
        }
    }
    publish_rows(st, slots, bt);
    for device in unpaired.iter().filter(|d| transport_bound(&st.prefs, d)) {
        let device = st.prefs.keyboard_row(device.clone());
        if let Some(row) = st.rows.iter_mut().find(|r| r.device.id == device.id) {
            row.device = device;
            row.error = Some(KYSONA_SLOT_UNPAIRED.into());
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
        st.prefs.retain_bound_identity(&mut row.device);
        row.device = st.prefs.keyboard_row(row.device);
        Some(row)
    }));
    let current = st.prefs.clone();
    current.preserve_missing(&mut st.rows, bt);
    st.prune_unbound();
    st.rows.sort_by(|a, b| a.device.id.cmp(&b.device.id));
}
impl Service {
    pub fn start(p: Prefs, demo: bool, wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self::start_with_signals(p, demo, Signals::default(), wake)
    }

    fn start_with_signals(
        p: Prefs,
        demo: bool,
        signals: Signals,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let w: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let state = Arc::new(Mutex::new(if demo {
            let mut state = State::demo();
            state.prefs.engine_mode = p.engine_mode;
            state.prefs.probe_unknown_devices = p.probe_unknown_devices;
            state
        } else {
            State {
                prefs: p,
                ..Default::default()
            }
        }));
        let service = Self {
            state,
            signals: Arc::new(signals),
            workers: Workers::default(),
            wake: w,
            demo,
        };
        if !demo {
            *service.workers.0.lock().unwrap() = service.spawn_workers([vec![], vec![]]);
        }
        service
    }

    /// 以挂起状态启动真实线程，避免首次扫描依赖本机 HID 或蓝牙设备。
    pub(crate) fn smoke_test_compatible(prefs: Prefs) -> bool {
        for enabled in [false, true] {
            let mut prefs = prefs.clone();
            prefs.probe_unknown_devices = enabled;
            let signals = Signals::default();
            signals.power(4);
            let service = Self::start_with_signals(prefs, false, signals, || {});
            let worker_count = service.workers.0.lock().unwrap().len();
            if worker_count != 2 || service.workers.probe_runtime_active() != enabled {
                return false;
            }
        }

        let demo = Self::start(prefs, true, || {});
        let st = demo.state.lock().unwrap();
        st.prefs.engine_mode == EngineMode::Compatible && st.rows.len() == 2
    }

    fn spawn_workers(&self, prior_exact: [Vec<Target>; 2]) -> Vec<JoinHandle<Vec<Target>>> {
        let (mode, unknown_probe_enabled) = {
            let st = self.state.lock().unwrap();
            (st.prefs.engine_mode, st.prefs.unknown_probe_enabled())
        };
        if unknown_probe_enabled {
            match ProbeRuntime::start(&self.signals) {
                Ok(runtime) => *self.workers.1.lock().unwrap() = Some(runtime),
                Err(error) => eprintln!("Failed to start device probe worker: {error}"),
            }
        }
        [false, true]
            .into_iter()
            .zip(prior_exact)
            .map(|(bt, prior)| {
                let (state, signals, wake) =
                    (self.state.clone(), self.signals.clone(), self.wake.clone());
                thread::spawn(move || {
                    run(state, signals, bt, mode, unknown_probe_enabled, wake, prior)
                })
            })
            .collect()
    }

    fn stop_workers(&self, workers: &mut Vec<JoinHandle<Vec<Target>>>) -> [Vec<Target>; 2] {
        self.signals.both(STOP);
        if let Some(runtime) = self.workers.1.lock().unwrap().take() {
            self.signals.probe_control.lock().unwrap().take();
            runtime.stop();
        }
        let mut snapshots = [vec![], vec![]];
        for (index, worker) in workers.drain(..).enumerate() {
            if let Ok(exact) = worker.join() {
                snapshots[index] = exact;
            }
        }
        for scope in [BusScope::Usb, BusScope::Bluetooth] {
            self.signals.probe_results(scope).lock().unwrap().clear();
        }
        snapshots
    }

    /// 停止旧线程后，用当前模式重启发现与轮询，保持外部订阅的状态和信号引用。
    pub fn restart_workers(&self) {
        if self.demo {
            return;
        }
        let mut workers = self.workers.0.lock().unwrap();
        let snapshots = self.stop_workers(&mut workers);
        let pending_jobs = {
            let cycle = self.signals.cycle.lock().unwrap();
            let hid_jobs = self.signals.hid.reset();
            let bt_jobs = self.signals.bt.reset();
            if cycle.suspended {
                self.signals.both(SUSPEND);
            }
            hid_jobs.into_iter().chain(bt_jobs)
        };
        {
            let mut st = self.state.lock().unwrap();
            st.probe_diagnostics.clear();
            // 模式切换取消旧请求，向仍在等待回执的界面结束对应请求。
            if st.manual.id != 0 && !st.manual.completed() {
                let id = st.manual.id;
                st.manual.finish(id, false, Some(Notice::OperationFailed));
                st.manual.finish(id, true, None);
            }
            for job in pending_jobs {
                st.finish(&job, Err("Recognition mode changed".into()));
            }
        }
        *workers = self.spawn_workers(snapshots);
    }
    pub fn device_changed(&self) {
        if !self.demo {
            let now = Instant::now();
            let schedule = |signal: &Signal| {
                schedule_device_scan(&mut signal.device_scan.lock().unwrap(), now);
                signal.send(PREFS);
            };
            schedule(&self.signals.hid);
            if self.state.lock().unwrap().prefs.unknown_probe_enabled() {
                schedule(&self.signals.bt);
            }
        }
    }
    pub fn command(&self, c: Command) -> Result<(), String> {
        match c {
            Command::SetEngineMode(mode) => {
                let mut st = self.state.lock().unwrap();
                if st.prefs.engine_mode == mode {
                    return Ok(());
                }
                let mut prefs = st.prefs.clone();
                prefs.change(&Command::SetEngineMode(mode), &[]);
                if !self.demo {
                    prefs.save(&Prefs::path()?)?;
                }
                st.prefs = prefs;
                st.probe_diagnostics.clear();
                drop(st);
                if !self.demo {
                    self.restart_workers();
                }
            }
            Command::SetUnknownProbe(enabled) => {
                let mut st = self.state.lock().unwrap();
                if st.prefs.probe_unknown_devices == enabled {
                    return Ok(());
                }
                let mut prefs = st.prefs.clone();
                prefs.change(&Command::SetUnknownProbe(enabled), &[]);
                if !self.demo {
                    prefs.save(&Prefs::path()?)?;
                }
                let compatible = prefs.engine_mode == EngineMode::Compatible;
                st.prefs = prefs;
                if !enabled {
                    st.probe_diagnostics.clear();
                }
                drop(st);
                if compatible {
                    self.restart_workers();
                }
            }
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
                if !matches!(c, Command::SetLanguage(_) | Command::Language) {
                    self.signals.both(PREFS)
                }
            }
        }
        Ok(())
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.stop_workers(&mut self.workers.0.lock().unwrap());
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn test_probed_route(variant: battery_protocol::ProbeVariantId) -> ProbedRoute {
        ProbedRoute {
            variant,
            vid: 0x0c45,
            pid: 0x1234,
            usage_page: 0xff00,
            usage: 1,
        }
    }

    #[test]
    fn probe_diagnostics_sync_by_bus_and_round_trip_with_state_json() {
        use battery_protocol::{
            ProbeVariantId,
            hid::probe::{ProbeDiagnosticReason, ProbeDiagnosticState, format_probe_diagnostics},
        };

        let mut st: State = serde_json::from_str("{}").unwrap();
        let mut entry = ProbeDiagnosticEntry {
            bus_scope: BusScope::Usb,
            vid: 0x0c45,
            pid: 0x9999,
            usage_page: 0xffb0,
            usage: 1,
            variant: ProbeVariantId::SparkLinkV2Wireless,
            state: ProbeDiagnosticState::Rejected,
            reason: Some(ProbeDiagnosticReason::ConfirmedWired),
        };
        publish_probe_diagnostics(&mut st, BusScope::Usb, vec![entry.clone()]);
        assert!(st.probe_diagnostics.is_empty());
        assert!(
            serde_json::to_value(&st)
                .unwrap()
                .get("probe_diagnostics")
                .is_none()
        );

        st.prefs.engine_mode = EngineMode::Compatible;
        publish_probe_diagnostics(&mut st, BusScope::Usb, vec![entry.clone()]);
        assert!(st.probe_diagnostics.is_empty());
        st.prefs.probe_unknown_devices = true;
        let bt = ProbeDiagnosticEntry {
            bus_scope: BusScope::Bluetooth,
            vid: 0x3151,
            pid: 0xee09,
            usage_page: 0xff55,
            usage: 0x0202,
            variant: ProbeVariantId::RongYuanBle77,
            ..entry.clone()
        };
        publish_probe_diagnostics(&mut st, BusScope::Bluetooth, vec![bt.clone()]);
        publish_probe_diagnostics(&mut st, BusScope::Usb, vec![entry.clone()]);
        entry.state = ProbeDiagnosticState::Confirmed;
        entry.reason = None;
        publish_probe_diagnostics(&mut st, BusScope::Usb, vec![entry.clone()]);
        assert_eq!(st.probe_diagnostics, [entry, bt.clone()]);
        let json = serde_json::to_string(&st).unwrap();
        let decoded: State = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, st);
        let poisoned = serde_json::json!({
            "bus_scope": "Usb",
            "vid": 0x0c45,
            "pid": 0x9999,
            "usage_page": 0xffb0,
            "usage": 1,
            "variant": "SparkLinkV2Wireless",
            "state": r"C:\Users\AliceUser",
            "reason": "SN-SECRET-987654"
        });
        assert!(serde_json::from_value::<ProbeDiagnosticEntry>(poisoned).is_err());
        let report = format_probe_diagnostics(&decoded.probe_diagnostics);
        assert!(report.contains("[Usb] 0c45:9999"));
        assert!(report.contains("[Bluetooth] 3151:ee09"));
        for secret in ["AliceUser", "SN-SECRET-987654", "\\"] {
            assert!(!report.contains(secret));
        }
        publish_probe_diagnostics(&mut st, BusScope::Usb, Vec::new());
        assert_eq!(st.probe_diagnostics, [bt]);

        let service = Service::start(st.prefs.clone(), true, || {});
        *service.state.lock().unwrap() = st;
        service
            .command(Command::SetEngineMode(EngineMode::Normal))
            .unwrap();
        assert!(service.state.lock().unwrap().probe_diagnostics.is_empty());
    }

    #[test]
    fn probe_runtime_is_compatible_only_and_stops_on_restart_and_drop() {
        let mut service = Service::start(Prefs::default(), true, || {});
        service.demo = false;
        service.signals.power(4);
        service.restart_workers();
        assert!(!service.workers.probe_runtime_active());
        let control = || {
            service
                .signals
                .probe_control
                .lock()
                .unwrap()
                .clone()
                .unwrap()
        };

        {
            let mut st = service.state.lock().unwrap();
            st.prefs.engine_mode = EngineMode::Compatible;
            st.prefs.probe_unknown_devices = true;
        }
        service.restart_workers();
        assert!(service.workers.probe_runtime_active());
        let first = control();
        let scope = BusScope::Usb;
        first.set_generation(scope, 99);
        assert!(first.should_abort(scope, 99));
        first.set_suspended(false);
        assert!(!first.should_abort(scope, 99));

        service.state.lock().unwrap().prefs.engine_mode = EngineMode::Normal;
        service.restart_workers();
        assert!(!service.workers.probe_runtime_active());
        assert!(first.should_abort(scope, 99));
        assert!(service.signals.probe_control.lock().unwrap().is_none());

        service.state.lock().unwrap().prefs.engine_mode = EngineMode::Compatible;
        service.restart_workers();
        let last = control();
        let signals = service.signals.clone();
        last.set_generation(scope, 100);
        last.set_suspended(false);
        assert!(!last.should_abort(scope, 100));
        drop(service);
        assert!(last.should_abort(scope, 100));
        assert!(signals.probe_control.lock().unwrap().is_none());
    }

    #[test]
    fn probe_result_bit_survives_each_independent_signal() {
        let signal = Signal::default();
        for bit in [SCAN, REFRESH, PREFS, JOB, SYNC, STOP, SUSPEND, RESUME] {
            assert_eq!(bit & PROBE_RESULT, 0);
            signal.send(PROBE_RESULT);
            signal.send(bit);
            assert_eq!(signal.take(Duration::ZERO), PROBE_RESULT | bit);
        }
    }

    #[test]
    fn stale_probe_generation_never_reaches_cache_commit() {
        let signals = Signals::default();
        let control = ProbeControl::default();
        let mut scan = CompatibleScan::new(BusScope::Usb, Some(control.clone()));
        scan.advance(&signals);
        scan.probe_in_flight = Some(1);
        scan.advance(&signals);
        scan.probe_in_flight = Some(2);
        assert!(control.should_abort(BusScope::Usb, 1));
        assert!(!scan.commit_current(1, |_| panic!("stale batch reached the cache")));
        assert_eq!(scan.probe_in_flight, Some(2));
        assert!(scan.commit_current(2, |_| true));
        assert!(!scan.commit_current(2, |_| panic!("duplicate batch reached the cache")));
    }

    #[test]
    fn normal_mode_missing_probed_bindings_skip_recovery_and_keep_routes() {
        use battery_protocol::ProbeVariantId::{AulaHfdWireless56, RongYuanBle77};
        let mut prefs = Prefs {
            keyboard: Some("key".into()),
            ..Default::default()
        };
        let keyboard = phase1_receiver("key", Kind::Keyboard, None);
        let bt = phase1_receiver("bt", Kind::Bluetooth, None);
        prefs.bluetooth.insert(bt.id.clone());
        assert!(
            prefs.remember_identity_with_route(
                &keyboard,
                Some(test_probed_route(AulaHfdWireless56))
            )
        );
        assert!(prefs.remember_identity_with_route(&bt, Some(test_probed_route(RongYuanBle77))));
        let saved = prefs.bound_probed_routes();
        let mut stale = Slot::new(keyboard.clone(), 0);
        stale.fail = 3;
        let slots = HashMap::from([(keyboard.id.clone(), stale)]);
        let inventory = Err("枚举失败".into());
        assert!(keyboard_recovery_inventory(&prefs, &[], Some(&inventory)).is_none());
        assert!(!stale_keyboard(&slots, &prefs));
        let mouse = phase1_receiver("mouse", Kind::Mouse, Some("known"));
        assert_eq!(
            missing_devices(&prefs, std::iter::once(&mouse), false),
            (false, false)
        );
        assert_eq!(
            missing_devices(&prefs, std::iter::empty(), true),
            (false, false)
        );
        prefs.bluetooth.insert("ordinary-bt".into());
        assert_eq!(
            missing_devices(&prefs, std::iter::empty(), true),
            (true, false)
        );
        let mut st = State {
            prefs,
            ..Default::default()
        };
        publish_rows(&mut st, &HashMap::new(), false);
        publish_rows(&mut st, &HashMap::new(), true);
        assert_eq!(
            st.rows
                .iter()
                .filter(|row| row.error.as_deref() == Some(PROBED_COMPATIBLE_ONLY_ERROR))
                .count(),
            2
        );
        assert_ne!(st.note, Some(Notice::KeyboardReconnectFailed));
        assert_eq!(st.prefs.bound_probed_routes(), saved);
        st.prefs.engine_mode = EngineMode::Compatible;
        assert!(keyboard_recovery_inventory(&st.prefs, &[], Some(&inventory)).is_none());
        assert!(!stale_keyboard(&slots, &st.prefs));
        assert_eq!(
            missing_devices(&st.prefs, std::iter::once(&mouse), false),
            (false, false)
        );
        st.prefs.bluetooth.remove("ordinary-bt");
        assert_eq!(
            missing_devices(&st.prefs, std::iter::empty(), true),
            (false, false)
        );
        publish_rows(&mut st, &HashMap::new(), false);
        publish_rows(&mut st, &HashMap::new(), true);
        assert_eq!(
            st.rows
                .iter()
                .filter(|row| row.error.as_deref() == Some(PROBED_COMPATIBLE_ONLY_ERROR))
                .count(),
            2
        );
        assert_ne!(st.note, Some(Notice::KeyboardReconnectFailed));
        assert_eq!(st.prefs.bound_probed_routes(), saved);
        st.prefs.probe_unknown_devices = true;
        assert_eq!(
            keyboard_recovery_inventory(&st.prefs, &[], Some(&inventory)),
            Some(&inventory)
        );
        assert!(stale_keyboard(&slots, &st.prefs));
        assert_eq!(
            missing_devices(&st.prefs, std::iter::empty(), false),
            (true, true)
        );
    }

    #[test]
    fn compatible_restart_seeds_the_bound_transport_and_schedules_probe_retries() {
        let mut prefs = Prefs {
            engine_mode: EngineMode::Compatible,
            keyboard: Some("logical".into()),
            keyboard_connection: Some(KeyboardConnection {
                endpoint_id: "physical".into(),
                serial: None,
            }),
            ..Default::default()
        };
        let keyboard = phase1_receiver("physical", Kind::Keyboard, None);
        let route = test_probed_route(battery_protocol::ProbeVariantId::AulaHfdWireless56);
        assert!(prefs.remember_identity_with_route(&keyboard, Some(route)));
        let mut scan = CompatibleScan::new(BusScope::Usb, Some(ProbeControl::default()));
        scan.seed(&prefs, true);
        assert_eq!(
            scan.seeded_routes,
            HashMap::from([(keyboard.id.clone(), route)])
        );
        let now = Instant::now();
        // 只预热路由，等 core 枚举端点后才会有下一次探测时间。
        assert_eq!(scan.next_probe(now), None);
        assert_eq!(
            probe_wake_delay(now, Some(now + Duration::from_secs(3)), None),
            Some(Duration::from_secs(3))
        );
        assert_eq!(
            probe_wake_delay(now, Some(now), None),
            Some(Duration::from_millis(25))
        );
        assert!(prefs.clear_identity(&keyboard));
        scan.seed(&prefs, false);
        assert!(scan.seeded_routes.is_empty());
    }

    #[test]
    fn manual_probe_waits_for_pending_batches_and_finishes_both_receipts_at_deadline() {
        let now = Instant::now();
        let signals = Signals::default();
        for (elapsed, in_flight, pending) in [
            (Duration::from_secs(1), false, false),
            (MANUAL_SCAN_MAX_WAIT, true, true),
        ] {
            let mut window = ManualProbeWindow::new(now);
            window.sync_id = Some(7);
            window.job = Some(DeviceJob {
                id: 8,
                kind: Kind::Keyboard,
                bind: None,
            });
            let mut st = State {
                manual: Manual {
                    id: 7,
                    bt: true,
                    ..Default::default()
                },
                ..Default::default()
            };
            assert_eq!(
                window.deadline.duration_since(now),
                Duration::from_millis(4500)
            );
            let before = window.deadline - Duration::from_millis(1);
            assert!(window.waiting(before, false, true));
            assert!(window.waiting(before, true, false));
            assert!(!st.manual.completed());
            assert!(st.reply(Kind::Keyboard).is_none());
            assert_eq!(
                probe_wake_delay(before, Some(before), Some(window.deadline)),
                Some(Duration::from_millis(1))
            );
            assert!(!window.waiting(now + elapsed, in_flight, pending));
            let control = ProbeControl::default();
            let mut scan = CompatibleScan::new(BusScope::Usb, Some(control.clone()));
            scan.advance(&signals);
            scan.manual_probe = Some(window);
            scan.probe_in_flight = Some(1);
            assert!(scan.take_ready_window(&signals, before).is_none());
            if !in_flight {
                assert!(scan.commit_current(1, |_| true));
            }
            scan.take_ready_window(&signals, now + elapsed)
                .unwrap()
                .finish(&mut st, false);
            assert_eq!(scan.probe_in_flight, None);
            assert_eq!(control.should_abort(BusScope::Usb, 1), in_flight);
            assert!(st.manual.completed());
            assert_eq!(
                st.reply(Kind::Keyboard),
                Some(&Receipt {
                    id: 8,
                    result: Ok(())
                })
            );
        }
    }
    fn phase1_receiver(id: &str, kind: Kind, key: Option<&str>) -> Descriptor {
        let category = if kind == Kind::Keyboard {
            battery_protocol::DeviceCategory::Keyboard
        } else {
            battery_protocol::DeviceCategory::Mouse
        };
        Descriptor {
            id: id.into(),
            name: key.unwrap_or("2.4G Receiver").into(),
            kind,
            legacy_ids: vec![],
            identity: Some(battery_protocol::DeviceIdentity {
                model_key: key.map(str::to_owned),
                category,
                connection: battery_protocol::ConnectionKind::Rf24,
                battery: battery_protocol::BatteryCapability::Present,
                model_source: key.map(|_| battery_protocol::ModelSource::ProtocolIdentity),
            }),
            generic_name: key.is_none().then_some(GenericName::Receiver24g {
                brand: None,
                channel: Some(category),
            }),
        }
    }

    #[test]
    fn compatible_association_replaces_bound_cache_and_keeps_only_the_new_sample() {
        use crate::device_display::{DeviceIcon, device_icon_in_mode};
        let old = phase1_receiver("keyboard", Kind::Keyboard, Some("AULA F87 Pro V2"));
        let new = phase1_receiver("keyboard", Kind::Keyboard, None);
        let mut st = State {
            prefs: Prefs {
                keyboard: Some(old.id.clone()),
                lang: Language::En,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(st.prefs.remember_identity(&old));
        let cached = st.prefs.bound_identity.clone();
        let mut slot = Slot::new(old.clone(), 0);
        let last = Reading::new(93, Charge::Discharging, Source::AulaHfd).unwrap();
        slot.apply(Ok(last), 10, false);
        slot.row.sampled = 1;
        let slots = HashMap::from([(old.id.clone(), Slot::new(new.clone(), 0))]);
        for mode in [EngineMode::Normal, EngineMode::Compatible] {
            st.prefs.engine_mode = mode;
            assert!(!st.prefs.remember_identity(&new));
            assert_eq!(st.prefs.name(&new), old.name);
            assert_eq!(st.prefs.bound_identity, cached);
            publish_rows(&mut st, &slots, false);
            assert_eq!(st.rows[0].device, old);
        }

        let reading = Reading::new(37, Charge::Discharging, Source::AulaHfd).unwrap();
        let sample = TargetSample {
            reading: Ok(reading),
            association_changed: true,
        };
        assert!(!generic_association_changed(
            EngineMode::Normal,
            &new,
            &sample
        ));
        assert!(generic_association_changed(
            EngineMode::Compatible,
            &new,
            &sample
        ));
        apply_target_sample(&mut slot, &new, sample, 20, false);
        assert_eq!(slot.row.battery, Some(reading));
        assert!(slot.row.sampled > 1);
        assert!(!slot.row.held);
        let slots = HashMap::from([(new.id.clone(), slot)]);
        assert!(remember_slot_identities(
            &mut st.prefs,
            &slots,
            &[],
            std::slice::from_ref(&new)
        ));
        publish_rows(&mut st, &slots, false);
        assert_eq!(st.rows[0].device, new);
        assert_ne!(st.prefs.bound_identity, cached);
        assert_eq!(st.row_name(&st.rows[0]), "2.4G receiver (Keyboard channel)");
        assert_eq!(
            device_icon_in_mode(&st.rows[0].device, EngineMode::Compatible),
            Some(DeviceIcon::Keyboard)
        );
    }

    #[test]
    fn compatible_generic_names_are_numbered_by_id_across_display_surfaces() {
        let mut st = State {
            prefs: Prefs {
                lang: Language::En,
                ..Default::default()
            },
            ..Default::default()
        };
        st.prefs.aliases.insert("b".into(), "Desk mouse".into());
        let slots = ["e", "c", "a", "b", "d"]
            .into_iter()
            .map(|id| {
                let d = phase1_receiver(id, Kind::Mouse, (id == "d").then_some("Exact Mouse"));
                (id.into(), Slot::new(d, 0))
            })
            .collect();
        publish_rows(&mut st, &slots, false);
        assert_eq!(
            st.rows
                .iter()
                .map(|row| row.device.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "c", "d", "e"]
        );
        let base = st.prefs.name(&st.rows[0].device);
        for (mode, expected) in [
            (
                EngineMode::Normal,
                vec![
                    base.clone(),
                    "Desk mouse".into(),
                    base.clone(),
                    "Exact Mouse".into(),
                    base.clone(),
                ],
            ),
            (
                EngineMode::Compatible,
                vec![
                    base.clone(),
                    "Desk mouse".into(),
                    format!("{base} 2"),
                    "Exact Mouse".into(),
                    format!("{base} 3"),
                ],
            ),
        ] {
            st.prefs.engine_mode = mode;
            assert_eq!(
                st.rows.iter().map(|r| st.row_name(r)).collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[test]
    fn restart_snapshot_inherits_only_same_endpoint_without_fresh_evidence() {
        let mut pending = FakeEndpoint::new("new-worker", "2.4G Receiver", None, None);
        pending.info.generic_name = Some(GenericName::Receiver24g {
            brand: Some("AULA".into()),
            channel: None,
        });
        let known = FakeEndpoint::new(
            "old-worker",
            "AULA F87 Pro V2",
            Some(0x2710),
            Some("aula_f87_pro_v2_z"),
        );
        let prior = [known.clone()];
        let mut different = pending.clone();
        different.path = "another collection";
        assert!(!restore_exact_identity(&mut different, &prior));
        let mut observed = pending.clone();
        observed.observed = Some(battery_protocol::IdentityEvidence::PeripheralIds {
            vid: 0x38a6,
            pid: 0xee01,
        });
        assert!(!restore_exact_identity(&mut observed, &prior));
        assert!(restore_exact_identity(&mut pending, &prior));
        assert_eq!(pending.info, known.info);
        assert_eq!(pending.observed, known.observed);
    }

    #[test]
    fn engine_mode_command_restarts_workers_in_place_and_keeps_preferences() {
        const NAME: &str =
            "service::tests::engine_mode_command_restarts_workers_in_place_and_keeps_preferences";
        const PROFILE: &str = "WDBM_PHASE1_TEST_PROFILE";
        if std::env::var_os(PROFILE).is_none() {
            let dir = tempfile::tempdir().unwrap();
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([NAME, "--exact"])
                .env("APPDATA", dir.path())
                .env(PROFILE, dir.path())
                .output()
                .unwrap();
            assert!(
                status.status.success(),
                "{}",
                String::from_utf8_lossy(&status.stdout)
            );
            return;
        }
        assert_eq!(std::env::var_os(PROFILE), std::env::var_os("APPDATA"));
        let mut service = Service::start(Prefs::default(), true, || {});
        service.demo = false;
        let state = service.state.clone();
        let signals = service.signals.clone();
        let sleeping = phase1_receiver("bound-keyboard", Kind::Keyboard, Some("Known keyboard"));
        let mut prefs: Prefs = serde_json::from_str(
            r#"{"keyboard":"bound-keyboard","low":30,"aliases":{"mouse":"Desk Mouse"}}"#,
        )
        .unwrap();
        assert_eq!(prefs.engine_mode, EngineMode::Normal);
        prefs.bluetooth.insert("bound-headset".into());
        assert!(prefs.remember_identity(&sleeping));
        *state.lock().unwrap() = State {
            prefs: prefs.clone(),
            autostart: true,
            update: crate::update::Status::Current,
            ..Default::default()
        };
        signals.power(4);

        // 旧线程以退出计数证明切换是 join，后续两代由生产路径创建。
        let exited = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        *service.workers.0.lock().unwrap() = [false, true]
            .into_iter()
            .map(|bt| {
                let signals = signals.clone();
                let exited = exited.clone();
                thread::spawn(move || {
                    let sig = if bt { &signals.bt } else { &signals.hid };
                    while sig.take(Duration::from_secs(60)) & STOP == 0 {}
                    exited.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Vec::new()
                })
            })
            .collect();
        let thread_ids = || {
            service
                .workers
                .0
                .lock()
                .unwrap()
                .iter()
                .map(|w| w.thread().id())
                .collect::<Vec<_>>()
        };
        for mode in [EngineMode::Compatible, EngineMode::Normal] {
            let old_ids = thread_ids();
            *signals.hid.manual.lock().unwrap() = Some((7, true));
            signals.bt.jobs.lock().unwrap().push_back(DeviceJob {
                id: 8,
                kind: Kind::Bluetooth,
                bind: None,
            });
            *signals.hid.device_scan.lock().unwrap() =
                Some(Instant::now() + Duration::from_secs(60));
            service.command(Command::SetEngineMode(mode)).unwrap();
            assert_eq!(exited.load(std::sync::atomic::Ordering::SeqCst), 2);
            assert!(Arc::ptr_eq(&state, &service.state));
            assert!(Arc::ptr_eq(&signals, &service.signals));
            assert_eq!(thread_ids().len(), 2);
            assert!(thread_ids().iter().all(|id| !old_ids.contains(id)));
            assert!(signals.hid.manual.lock().unwrap().is_none());
            assert!(signals.hid.device_scan.lock().unwrap().is_none());
            assert!(signals.bt.jobs.lock().unwrap().is_empty());
            let ids = thread_ids();
            service.command(Command::SetEngineMode(mode)).unwrap();
            assert_eq!(thread_ids(), ids);
            prefs.engine_mode = mode;
            let mut st = state.lock().unwrap();
            assert!(st.autostart);
            assert_eq!(st.update, crate::update::Status::Current);
            assert_eq!(st.prefs, prefs);
            assert_eq!(Prefs::load(&Prefs::path().unwrap()).unwrap(), prefs);
            publish_rows(&mut st, &HashMap::new(), false);
            let row = st.rows.iter().find(|r| r.device.id == sleeping.id).unwrap();
            assert_eq!(st.row_name(row), sleeping.name);
            assert_eq!(row.device.identity, sleeping.identity);
        }

        let route = test_probed_route(battery_protocol::ProbeVariantId::AulaHfdWireless56);
        {
            let mut st = state.lock().unwrap();
            assert!(
                st.prefs
                    .remember_identity_with_route(&sleeping, Some(route))
            );
        }
        service
            .command(Command::SetEngineMode(EngineMode::Compatible))
            .unwrap();
        assert!(!service.workers.probe_runtime_active());
        {
            let mut st = state.lock().unwrap();
            publish_rows(&mut st, &HashMap::new(), false);
            assert_eq!(
                st.prefs.bound_probed_routes(),
                [(sleeping.id.clone(), route)]
            );
            let row = st.rows.iter().find(|r| r.device.id == sleeping.id).unwrap();
            assert_eq!(row.error.as_deref(), Some(PROBED_COMPATIBLE_ONLY_ERROR));
            assert_ne!(st.note, Some(Notice::KeyboardReconnectFailed));
        }

        let diagnostic = ProbeDiagnosticEntry {
            bus_scope: BusScope::Usb,
            vid: route.vid,
            pid: route.pid,
            usage_page: route.usage_page,
            usage: route.usage,
            variant: route.variant,
            state: battery_protocol::hid::probe::ProbeDiagnosticState::Confirmed,
            reason: None,
        };
        for stop in [
            Command::SetUnknownProbe(false),
            Command::SetEngineMode(EngineMode::Normal),
        ] {
            if st_mode_is_normal(&state) {
                service
                    .command(Command::SetEngineMode(EngineMode::Compatible))
                    .unwrap();
                assert!(!service.workers.probe_runtime_active());
                assert!(!state.lock().unwrap().prefs.probe_unknown_devices);
            }
            service.command(Command::SetUnknownProbe(true)).unwrap();
            assert!(service.workers.probe_runtime_active());
            assert!(
                Prefs::load(&Prefs::path().unwrap())
                    .unwrap()
                    .unknown_probe_enabled()
            );
            let ids = thread_ids();
            service.command(Command::SetUnknownProbe(true)).unwrap();
            assert_eq!(thread_ids(), ids);
            {
                let mut st = state.lock().unwrap();
                publish_probe_diagnostics(&mut st, BusScope::Usb, vec![diagnostic.clone()]);
                assert_eq!(st.probe_diagnostics, std::slice::from_ref(&diagnostic));
            }
            service.command(stop).unwrap();
            assert!(!service.workers.probe_runtime_active());
            assert!(signals.probe_control.lock().unwrap().is_none());
            let st = state.lock().unwrap();
            assert!(st.probe_diagnostics.is_empty());
            assert!(!st.prefs.unknown_probe_enabled());
            assert!(!st.prefs.probe_unknown_devices);
            assert_eq!(
                st.prefs.bound_probed_routes(),
                [(sleeping.id.clone(), route)]
            );
            assert_eq!(Prefs::load(&Prefs::path().unwrap()).unwrap(), st.prefs);
        }
        // 切回普通模式后再次进入兼容模式，不得绕过二次确认自动开启未知探测。
        service
            .command(Command::SetEngineMode(EngineMode::Compatible))
            .unwrap();
        assert!(!service.workers.probe_runtime_active());
        assert!(!state.lock().unwrap().prefs.probe_unknown_devices);
        let ids = thread_ids();
        service.command(Command::SetUnknownProbe(false)).unwrap();
        assert_eq!(thread_ids(), ids);
        assert!(!service.workers.probe_runtime_active());
        assert!(
            !Prefs::load(&Prefs::path().unwrap())
                .unwrap()
                .probe_unknown_devices
        );
    }

    fn st_mode_is_normal(state: &Arc<Mutex<State>>) -> bool {
        state.lock().unwrap().prefs.engine_mode == EngineMode::Normal
    }

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
                "live recovery timed out: {:?}",
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
                identity: None,
                generic_name: None,
            },
            0,
        )
    }
    #[test]
    fn changed_association_never_holds_the_previous_peripheral_battery() {
        let mut slot = slot();
        let reading = Reading::new(74, Charge::Unknown, Source::Kysona24g).unwrap();
        slot.apply(Ok(reading), 0, false);
        let mut device = slot.row.device.clone();
        device.name = "2.4G Receiver".into();
        apply_target_sample(
            &mut slot,
            &device,
            TargetSample {
                reading: Err("new association has no reading".into()),
                association_changed: true,
            },
            1000,
            false,
        );
        assert_eq!(slot.row.device, device);
        assert!(!slot.row.held);
        assert_eq!(slot.row.battery, None);
        assert_eq!(slot.row.sampled, 0);
    }

    #[test]
    fn an_unbind_during_a_probe_does_not_pass_the_binding_check() {
        let mut before = Prefs::default();
        before.bluetooth.extend(["target".into(), "other".into()]);
        let mut after = before.clone();
        after.bluetooth.remove("other");
        assert!(same_binding(&before, &after, Kind::Bluetooth, "target"));
        after.bluetooth.remove("target");
        assert!(!same_binding(&before, &after, Kind::Bluetooth, "target"));
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
    fn peripheral_disconnect_preserves_the_last_reading_until_a_fresh_zero() {
        let mut state = State::demo();
        let mut mouse = Slot::new(state.rows[0].device.clone(), 0);
        let read = |v| Reading::new(v, Charge::Unknown, Source::Kysona24g).unwrap();
        mouse.apply(Ok(read(85)), 0, false);
        let sample_time = mouse.row.sampled;
        mouse.apply(Err("device asleep".into()), 1000, false);
        assert_eq!(mouse.row.battery, Some(read(85)));
        assert_eq!(mouse.row.sampled, sample_time);
        assert!(mouse.row.held && mouse.row.error.is_some());
        state.rows[0] = mouse.row.clone();
        assert_ne!(state.selected().unwrap().device.kind, Kind::Mouse);
        mouse.apply(Ok(read(0)), 2000, false);
        assert_eq!(mouse.row.battery, Some(read(0)));
        assert!(!mouse.row.held && mouse.row.error.is_none());
        mouse.apply(Ok(read(85)), 3000, false);
        mouse.apply(Ok(read(0)), 4000, false);
        assert_eq!(mouse.row.battery, Some(read(0)));
        mouse.apply(Err(KYSONA_SLOT_UNPAIRED.into()), 5000, false);
        assert!(mouse.row.battery.is_none() && !mouse.row.held);
    }

    #[test]
    fn aula_protocols_preserve_offline_samples_and_resume_with_fresh_zero() {
        for source in [
            Source::Atk(battery_protocol::catalog::atk::Provider::BitMouse),
            Source::Atk(battery_protocol::catalog::atk::Provider::Bolutek),
            Source::Atk(battery_protocol::catalog::atk::Provider::KeyLinker),
            Source::AulaLegacy,
            Source::AulaYb,
            Source::AulaVision,
            Source::AulaYxt,
            Source::AulaXxwd,
            Source::AulaBytechMagnetic,
            Source::AulaHfd,
            Source::AulaBytech,
            Source::AulaSparkLink,
            Source::AulaHx,
            Source::AulaJm,
            Source::AulaRlw,
        ] {
            let mut state = State::demo();
            let mut slot = Slot::new(state.rows[0].device.clone(), 0);
            let read = |level| Reading::new(level, Charge::Unknown, source).unwrap();
            slot.apply(Ok(read(73)), 100, false);
            let sampled = slot.row.sampled;
            slot.apply(Err("device offline".into()), 1000, false);
            assert_eq!(slot.row.battery, Some(read(73)));
            assert_eq!(slot.row.sampled, sampled);
            assert!(slot.row.held);
            state.rows = vec![slot.row.clone()];
            assert!(state.selected().is_none());
            slot.apply(Ok(read(0)), 2000, false);
            assert_eq!(slot.row.battery, Some(read(0)));
            assert!(!slot.row.held && slot.row.error.is_none());
            state.rows = vec![slot.row];
            assert_eq!(state.selected().unwrap().battery, Some(read(0)));
        }
    }

    #[test]
    fn ardor_fury_keeps_sleeping_samples_and_accepts_fresh_battery_tiers() {
        let device = Descriptor {
            id: "hid:1d57:fa60:Mouse:test".into(),
            name: "ARDOR GAMING Fury Wireless".into(),
            kind: Kind::Mouse,
            legacy_ids: vec![],
            identity: None,
            generic_name: None,
        };
        let mut slot = Slot::new(device, 0);
        let read = |v| Reading::new(v, Charge::Unknown, Source::ArdorFury).unwrap();
        slot.apply(Ok(read(100)), 0, false);
        slot.apply(Ok(read(25)), 1000, false);
        assert_eq!(slot.row.battery, Some(read(25)));
        let sampled = slot.row.sampled;
        slot.apply(
            Err("Peripheral status notification timed out".into()),
            2000,
            false,
        );
        assert_eq!(slot.row.battery, Some(read(25)));
        assert_eq!(slot.row.sampled, sampled);
        assert!(slot.row.held && slot.row.error.is_some());
        slot.apply(Ok(read(75)), 3000, false);
        assert_eq!(slot.row.battery, Some(read(75)));
        assert!(!slot.row.held && slot.row.error.is_none());
    }
    #[test]
    fn private_bluetooth_survives_standard_initialization_and_scan_failures() {
        for error in [
            "WinRT apartment initialization failed",
            "Classic and LE enumeration failed",
        ] {
            let (result, warning) =
                merge_bluetooth_discovery(Err(error.into()), Ok(vec!["private HID"]));
            assert_eq!(result.unwrap(), ["private HID"]);
            assert_eq!(warning.as_deref(), Some(error));
            let (result, _) = merge_bluetooth_discovery::<&str>(Err(error.into()), Ok(vec![]));
            assert_eq!(result.unwrap_err(), error);
        }
        let (result, warning) =
            merge_bluetooth_discovery(Ok(vec!["standard"]), Err("HID enumeration failed".into()));
        assert_eq!(result.unwrap(), ["standard"]);
        assert_eq!(warning.as_deref(), Some("HID enumeration failed"));
        let (result, warning) =
            merge_bluetooth_discovery(Ok(vec!["standard"]), Ok(vec!["private HID"]));
        assert_eq!(result.unwrap(), ["standard", "private HID"]);
        assert!(warning.is_none());
        let (result, _) =
            merge_bluetooth_discovery::<&str>(Err("standard".into()), Err("private".into()));
        let error = result.unwrap_err();
        assert!(error.contains("standard") && error.contains("private"));
    }

    #[test]
    fn private_bluetooth_uses_the_bluetooth_binding_and_retains_sleeping_samples() {
        let mut state = State::demo();
        let device = Descriptor {
            id: "hid:3151:5027:Bluetooth:path:private".into(),
            name: "RY Bluetooth Device".into(),
            kind: Kind::Bluetooth,
            legacy_ids: vec![],
            identity: None,
            generic_name: None,
        };
        assert!(!transport_bound(&state.prefs, &device));
        state.prefs.bluetooth.insert(device.id.clone());
        assert!(transport_bound(&state.prefs, &device));
        let mut slot = Slot::new(device, 0);
        let reading = |pct| Reading::new(pct, Charge::Unknown, Source::KysonaBluetooth).unwrap();
        slot.apply(Ok(reading(85)), 0, false);
        slot.apply(Err("KYSONA Bluetooth device is asleep".into()), 1000, false);
        assert_eq!(slot.row.battery, Some(reading(85)));
        assert!(slot.row.held && slot.row.error.is_some());
        slot.apply(Ok(reading(0)), 2000, false);
        assert_eq!(slot.row.battery, Some(reading(0)));
        assert!(!slot.row.held && slot.row.error.is_none());
    }

    #[test]
    fn scyrox_mouse_retains_last_sample_while_asleep() {
        let device = Descriptor {
            id: "hid:395a:0002:Mouse:test".into(),
            name: "Scyrox V8S".into(),
            kind: Kind::Mouse,
            legacy_ids: vec![],
            identity: Some(battery_protocol::DeviceIdentity {
                model_key: Some("scyrox:395a:000d".into()),
                category: battery_protocol::DeviceCategory::Mouse,
                connection: battery_protocol::ConnectionKind::Rf24,
                battery: battery_protocol::BatteryCapability::Present,
                model_source: Some(battery_protocol::ModelSource::ProtocolIdentity),
            }),
            generic_name: None,
        };
        let mut slot = Slot::new(device, 0);
        let sample = Reading::new(73, Charge::Discharging, Source::Scyrox).unwrap();
        slot.apply(Ok(sample), 0, false);
        slot.apply(Err("Scyrox mouse is offline".into()), 1000, false);
        assert_eq!(slot.row.battery, Some(sample));
        assert!(slot.row.held && slot.row.error.is_some());
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct FakeEndpoint {
        path: &'static str,
        info: Descriptor,
        observed: Option<battery_protocol::IdentityEvidence>,
        tag: &'static str,
    }

    impl FakeEndpoint {
        fn new(tag: &'static str, name: &str, pid: Option<u16>, model_key: Option<&str>) -> Self {
            Self {
                path: r"\\?\hid#vid_395a&pid_0002#1",
                info: Descriptor {
                    id: "hid:395a:0002:Mouse:test".into(),
                    name: name.into(),
                    kind: Kind::Mouse,
                    legacy_ids: vec![],
                    identity: Some(battery_protocol::DeviceIdentity {
                        model_key: model_key.map(str::to_owned),
                        category: battery_protocol::DeviceCategory::Mouse,
                        connection: battery_protocol::ConnectionKind::Rf24,
                        battery: battery_protocol::BatteryCapability::Present,
                        model_source: model_key
                            .map(|_| battery_protocol::ModelSource::ProtocolIdentity),
                    }),
                    generic_name: None,
                },
                observed: pid.map(|pid| battery_protocol::IdentityEvidence::PeripheralIds {
                    vid: 0x395a,
                    pid,
                }),
                tag,
            }
        }
    }

    impl EndpointReconcile for FakeEndpoint {
        fn descriptor(&self) -> &Descriptor {
            &self.info
        }

        fn same_endpoint(&self, prior: &Self) -> bool {
            self.info.id == prior.info.id && self.path == prior.path
        }

        fn retain_identity(&mut self, prior: &Self) {
            if !self.same_endpoint(prior) || self.observed.is_some() {
                return;
            }
            self.observed = prior.observed.clone();
            if self.observed.is_some() {
                self.info = prior.info.clone();
            }
        }

        fn association_changed_from(&self, prior: &Self) -> bool {
            if !self.same_endpoint(prior) {
                return true;
            }
            fn model(d: &Descriptor) -> Option<&str> {
                d.identity
                    .as_ref()
                    .and_then(|identity| identity.model_key.as_deref())
            }
            let replaced = self.observed.is_some()
                && prior.observed.is_some()
                && self.observed != prior.observed;
            replaced || (model(&prior.info).is_some() && model(&prior.info) != model(&self.info))
        }
    }

    #[test]
    fn recovery_scan_and_reopen_reconcile_first_model_and_switched_models_uniformly() {
        let generic = FakeEndpoint::new("prior-generic", "Scyrox 2.4G Receiver", None, None);
        let first_v8s = FakeEndpoint::new(
            "fresh-v8s",
            "Scyrox V8S",
            Some(0x000d),
            Some("scyrox:395a:000d"),
        );
        let switched_v6s = FakeEndpoint::new(
            "fresh-v6s",
            "Scyrox V6S",
            Some(0x000b),
            Some("scyrox:395a:000b"),
        );
        let uncatalogued_0f =
            FakeEndpoint::new("fresh-0f", "Scyrox 2.4G Receiver", Some(0x000f), None);
        let reading = Reading::new(73, Charge::Discharging, Source::Scyrox).unwrap();

        // 1) recover_only 下且 fail < 3：旧端点无型号，新探针首次确认 V8S -> 采纳新目标、刷新名称且保留电量。
        let mut slots = HashMap::new();
        let mut slot = Slot::new(generic.info.clone(), 0);
        slot.apply(Ok(reading), 0, false);
        slot.fail = 1;
        slots.insert(generic.info.id.clone(), slot);
        let mut old = vec![generic.clone()];
        let merged = merge_discovered_target(first_v8s.clone(), &mut old, &mut slots, true);
        assert_eq!(merged.tag, "fresh-v8s");
        let kept_slot = slots.get(&generic.info.id).unwrap();
        assert_eq!(kept_slot.row.battery, Some(reading));
        assert_eq!(kept_slot.row.device.name, "Scyrox V8S");

        // 2) recover_only 下且 fail < 3：已识别为 V8S，重扫探测到 V6S -> 采纳新目标并清除旧 V8S 缓存。
        let prior_v8s = FakeEndpoint::new(
            "prior-v8s",
            "Scyrox V8S",
            Some(0x000d),
            Some("scyrox:395a:000d"),
        );
        let mut slot = Slot::new(prior_v8s.info.clone(), 0);
        slot.apply(Ok(reading), 0, false);
        slot.fail = 2;
        slots.insert(prior_v8s.info.id.clone(), slot);
        let mut old = vec![prior_v8s.clone()];
        let merged = merge_discovered_target(switched_v6s.clone(), &mut old, &mut slots, true);
        assert_eq!(merged.tag, "fresh-v6s");
        assert!(!slots.contains_key(&prior_v8s.info.id));

        // 3) recover_only 下且 fail < 3：单端点未重测无新证据 -> 保留原端点句柄与身份。
        let sleeping_rescan = FakeEndpoint::new("rescan-sleep", "Scyrox 2.4G Receiver", None, None);
        let mut slot = Slot::new(prior_v8s.info.clone(), 0);
        slot.apply(Ok(reading), 0, false);
        slot.fail = 1;
        slots.insert(prior_v8s.info.id.clone(), slot);
        let mut old = vec![prior_v8s.clone()];
        let merged = merge_discovered_target(sleeping_rescan, &mut old, &mut slots, true);
        assert_eq!(merged.tag, "prior-v8s");
        assert_eq!(
            slots.get(&prior_v8s.info.id).unwrap().row.battery,
            Some(reading)
        );

        // 4) 自动重开：同路径 V8S -> V6S 与 V8S -> 目录外 395A:000F 均须清除旧电量并刷新设备信息；
        //    无型号 -> 首获 V8S 则刷新设备信息且保留缓存电量。
        for (mut current, fresh, expect_name, expect_cleared) in [
            (generic.clone(), first_v8s.clone(), "Scyrox V8S", false),
            (prior_v8s.clone(), switched_v6s, "Scyrox V6S", true),
            (
                prior_v8s,
                uncatalogued_0f.clone(),
                "Scyrox 2.4G Receiver",
                true,
            ),
        ] {
            let mut slot = Slot::new(current.info.clone(), 0);
            slot.apply(Ok(reading), 0, false);
            let transition = replace_with_reopened(&mut current, fresh);
            apply_reopened_target(current.descriptor(), &mut slot, transition);
            assert_eq!(slot.row.device.name, expect_name);
            assert_eq!(slot.row.battery.is_none(), expect_cleared);
        }

        // 5) recover_only 下：无有效 ID -> 首次观测到 395A:000F（复用旧句柄并同步新证据）->
        //    后续再观测到 V8S 时，必须判定关联发生变更并清除旧鼠标缓存。
        let mut slot = Slot::new(generic.info.clone(), 0);
        slot.apply(Ok(reading), 0, false);
        slot.fail = 1;
        slots.insert(generic.info.id.clone(), slot);
        let mut old = vec![generic];
        let after_0f = merge_discovered_target(uncatalogued_0f, &mut old, &mut slots, true);
        assert_eq!(after_0f.tag, "prior-generic");
        assert!(after_0f.observed.is_some());
        assert_eq!(
            slots.get(&after_0f.info.id).unwrap().row.battery,
            Some(reading)
        );

        let mut old = vec![after_0f];
        let after_v8s = merge_discovered_target(first_v8s, &mut old, &mut slots, true);
        assert_eq!(after_v8s.tag, "fresh-v8s");
        assert!(!slots.contains_key(&after_v8s.info.id));
    }

    #[test]
    fn audio_is_monitored_and_retains_its_sample_while_asleep() {
        for (id, name, source) in [
            (
                "hid:3837:6011:Audio:test",
                "MCHOSE V9 PRO 2",
                Source::MchoseAudio,
            ),
            (
                "hid:03f0:06be:Audio:test",
                "HyperX Cloud III S Wireless",
                Source::HyperX,
            ),
            (
                "hid:10f5:2256:Audio:test",
                "Turtle Beach Stealth 700P Gen 3",
                Source::TurtleBeach,
            ),
        ] {
            let mut state = State::demo();
            let device = Descriptor {
                id: id.into(),
                name: name.into(),
                kind: Kind::Audio,
                legacy_ids: vec![],
                identity: None,
                generic_name: None,
            };
            assert!(state.prefs.bound(&device));
            let encoded = serde_json::to_vec(&device).unwrap();
            assert_eq!(
                serde_json::from_slice::<Descriptor>(&encoded).unwrap(),
                device
            );
            let mut slot = Slot::new(device, 0);
            let reading = Reading::new(73, Charge::Charging, source).unwrap();
            slot.apply(Ok(reading), 0, false);
            state.rows = vec![slot.row.clone()];
            assert_eq!(state.selected().unwrap().battery, Some(reading));
            slot.apply(Err("audio device is asleep".into()), 1000, false);
            assert_eq!(slot.row.battery, Some(reading));
            assert!(slot.row.held);
            state.rows = vec![slot.row];
            assert!(state.selected().is_none());
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

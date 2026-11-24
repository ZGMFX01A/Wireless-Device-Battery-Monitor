// SPDX-FileCopyrightText: 2026 ZGMFX01A
// SPDX-License-Identifier: LicenseRef-Wireless-Device-Battery-Monitor
// See LICENSE in the repository root for the full terms.

use crate::{
    icon,
    model::*,
    service::{Service, Signals},
    view::{
        locale,
        strings::{Locale, label},
    },
};
use battery_protocol::Charge;
use std::{
    collections::BTreeMap,
    ffi::c_void,
    io::{BufRead, BufReader, Write},
    process::{Child, Command as Process, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{self, Sender},
    },
    thread,
    time::{Duration, Instant},
};
use tray_icon::{
    MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};
use windows_sys::Win32::{
    Devices::DeviceAndDriverInstallation::*,
    Foundation::*,
    System::{Power::*, Threading::*},
    UI::{Shell::*, WindowsAndMessaging::*},
};
const WAKE: u32 = WM_APP + 73;
const REAP: usize = 73;
struct UpdateTimer(usize);
impl Drop for UpdateTimer {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                KillTimer(std::ptr::null_mut(), self.0);
            }
        }
    }
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}
pub enum AppError {
    AlreadyRunning,
    Startup(String),
    UpdateWorker(String),
    SmokeTest,
    UnknownOption,
}
impl From<String> for AppError {
    fn from(error: String) -> Self {
        Self::Startup(error)
    }
}
pub fn report(error: AppError) {
    let key = match error {
        AppError::AlreadyRunning => "app.already_running",
        AppError::Startup(e) => {
            eprintln!("Application startup failed: {e}");
            "app.start_failed"
        }
        AppError::UpdateWorker(e) => {
            eprintln!("Update worker failed: {e}");
            "update.install_failed"
        }
        AppError::SmokeTest => "app.smoke_failed",
        AppError::UnknownOption => "app.unknown_option",
    };
    let prefs = Prefs::path()
        .and_then(|path| Prefs::load(&path))
        .unwrap_or_default();
    let locale = locale(&prefs);
    let message = label(locale, key);
    eprintln!("{message}");
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            wide(message).as_ptr(),
            wide(label(locale, "app.header_title")).as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}
struct Lease(HANDLE);
impl Lease {
    fn new() -> Result<Self, AppError> {
        unsafe {
            let h = CreateMutexW(
                std::ptr::null(),
                0,
                wide("Local\\MouseBattery_Main_SingleInstance").as_ptr(),
            );
            if h.is_null() {
                return Err(std::io::Error::last_os_error().to_string().into());
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(h);
                return Err(AppError::AlreadyRunning);
            }
            Ok(Self(h))
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct Power {
    registration: *mut c_void,
    ctx: *const Signals,
}
unsafe extern "system" fn on_power(c: *const c_void, e: u32, _: *const c_void) -> u32 {
    if !c.is_null() && matches!(e, 4 | 7 | 18) {
        unsafe { &*c.cast::<Signals>() }.power(e)
    }
    0
}
impl Power {
    fn new(s: Arc<Signals>) -> Self {
        let ctx = Arc::into_raw(s);
        let mut p = DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(on_power),
            Context: ctx.cast_mut().cast(),
        };
        let mut registration = std::ptr::null_mut();
        let rc = unsafe {
            PowerRegisterSuspendResumeNotification(
                2,
                (&mut p as *mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS).cast(),
                &mut registration,
            )
        };
        if rc != 0 {
            eprintln!("power notifications unavailable: {rc}")
        }
        Self { registration, ctx }
    }
}
impl Drop for Power {
    fn drop(&mut self) {
        unsafe {
            if self.registration.is_null()
                || PowerUnregisterSuspendResumeNotification(self.registration as isize) == 0
            {
                drop(Arc::from_raw(self.ctx));
            }
        }
    }
}
enum Msg {
    Data,
    DeviceChanged,
    Open(Mode, Option<[i32; 2]>),
    Gui(u64, Command),
    Gone(u64),
    Action(Command),
    UpdateHandoff(Result<std::process::ChildStdin, String>),
    Install(bool),
    Quit,
}
#[derive(Clone)]
struct Bus(Sender<Msg>, u32);
impl Bus {
    fn post(&self, m: Msg) {
        if self.0.send(m).is_ok() {
            unsafe {
                PostThreadMessageW(self.1, WAKE, 0, 0);
            }
        }
    }
}
struct DeviceNotifications {
    registration: HCMNOTIFICATION,
    context: *mut Bus,
}
unsafe extern "system" fn on_device_change(
    _: HCMNOTIFICATION,
    context: *const c_void,
    action: CM_NOTIFY_ACTION,
    _: *const CM_NOTIFY_EVENT_DATA,
    _: u32,
) -> u32 {
    if matches!(
        action,
        CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL | CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL
    ) {
        // The allocation remains alive until unregister has waited for callbacks.
        unsafe { &*context.cast::<Bus>() }.post(Msg::DeviceChanged);
    }
    0
}
impl DeviceNotifications {
    fn new(bus: Bus) -> Option<Self> {
        let filter = CM_NOTIFY_FILTER {
            cbSize: std::mem::size_of::<CM_NOTIFY_FILTER>() as u32,
            FilterType: CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
            u: CM_NOTIFY_FILTER_0 {
                DeviceInterface: CM_NOTIFY_FILTER_0_0 {
                    ClassGuid: windows_sys::core::GUID {
                        data1: 0x4d1e55b2,
                        data2: 0xf16f,
                        data3: 0x11cf,
                        data4: [0x88, 0xcb, 0x00, 0x11, 0x11, 0x00, 0x00, 0x30],
                    },
                },
            },
            ..Default::default()
        };
        let context = Box::into_raw(Box::new(bus));
        let mut registration = std::ptr::null_mut();
        let rc = unsafe {
            CM_Register_Notification(
                &filter,
                context.cast(),
                Some(on_device_change),
                &mut registration,
            )
        };
        if rc != CR_SUCCESS {
            unsafe {
                drop(Box::from_raw(context));
            }
            eprintln!("HID device notifications unavailable: {rc}; polling remains active");
            return None;
        }
        Some(Self {
            registration,
            context,
        })
    }
}
impl Drop for DeviceNotifications {
    fn drop(&mut self) {
        let rc = unsafe { CM_Unregister_Notification(self.registration) };
        if rc == CR_SUCCESS {
            unsafe {
                drop(Box::from_raw(self.context));
            }
        } else {
            // Do not free a callback context that the OS could still access.
            eprintln!("HID notification cleanup failed: {rc}; retaining callback context");
        }
    }
}
#[derive(Default)]
struct Mail {
    state: Option<Box<State>>,
    control: Option<Event>,
    closed: bool,
}
type BoxedMail = Arc<(Mutex<Mail>, Condvar)>;
fn enqueue(mail: &BoxedMail, e: Event) {
    let (m, c) = &**mail;
    let mut g = m.lock().unwrap();
    match e {
        _ if g.closed => return,
        Event::State(s) => g.state = Some(s),
        Event::Exit => {
            g.state = None;
            g.closed = true;
            g.control = Some(Event::Exit)
        }
        e => g.control = Some(e),
    }
    c.notify_one()
}

fn write_mail(wr: BoxedMail, mut stdin: impl Write) {
    loop {
        let (m, c) = &*wr;
        let mut g = c
            .wait_while(m.lock().unwrap(), |m| {
                !m.closed && m.state.is_none() && m.control.is_none()
            })
            .unwrap();
        let st = g.state.take().map(Event::State);
        let ev = g.control.take();
        let end = g.closed;
        drop(g);
        for e in st.into_iter().chain(ev) {
            if serde_json::to_writer(&mut stdin, &e).is_err()
                || stdin.write_all(b"\n").is_err()
                || stdin.flush().is_err()
            {
                return;
            }
        }
        if end {
            break;
        }
    }
}
struct WindowProcess {
    ready: bool,
    ready_timer: usize,
    id: u64,
    child: Child,
    mail: BoxedMail,
    hidden: Option<Instant>,
    closing: bool,
    generation: u64,
    mode: Mode,
    visible: bool,
}
impl WindowProcess {
    fn spawn(id: u64, bus: Bus) -> Result<Self, String> {
        let mut child = Process::new(std::env::current_exe().map_err(|e| e.to_string())?)
            .arg("--gui-child")
            .env("RTSSHooksCompatibility", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| e.to_string())?;
        if std::env::args().any(|x| x == "--exercise-tray") {
            println!("GUI_PID {}", child.id());
            let _ = std::io::stdout().flush();
        }
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mail: BoxedMail = Arc::new(Default::default());
        let wr = mail.clone();
        thread::spawn(move || write_mail(wr, stdin));
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(cmd) = serde_json::from_str(&line) {
                    bus.post(Msg::Gui(id, cmd));
                }
            }
            bus.post(Msg::Gone(id));
        });
        Ok(Self {
            id,
            ready: false,
            ready_timer: unsafe { SetTimer(std::ptr::null_mut(), 0, 10_000, None) },
            child,
            mail,
            hidden: None,
            closing: false,
            generation: 0,
            mode: Mode::Flyout,
            visible: false,
        })
    }
    fn send(&self, e: Event) {
        enqueue(&self.mail, e);
    }
    fn stop_ready_timer(&mut self) {
        if self.ready_timer != 0 {
            unsafe {
                KillTimer(std::ptr::null_mut(), self.ready_timer);
            }
            self.ready_timer = 0;
        }
    }
    fn open(&mut self, mode: Mode, at: Option<[i32; 2]>) {
        self.hidden = None;
        self.generation = self.generation.wrapping_add(1);
        self.mode = mode;
        self.visible = true;
        self.send(Event::Open(mode, self.generation, at))
    }
    fn exit(&mut self) {
        if !self.closing {
            self.closing = true;
            self.stop_ready_timer();
            self.ready_timer = unsafe { SetTimer(std::ptr::null_mut(), 0, 2000, None) };
        }
        self.send(Event::Exit)
    }
}
impl Drop for WindowProcess {
    fn drop(&mut self) {
        self.exit();
        self.stop_ready_timer();
        let until = Instant::now() + Duration::from_secs(2);
        while self.child.try_wait().ok().flatten().is_none() && Instant::now() < until {
            thread::sleep(Duration::from_millis(10))
        }
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
struct MenuKey {
    locale: Locale,
    labels: Vec<String>,
}
#[derive(Debug, PartialEq, Eq)]
struct TrayView {
    icon: (Option<u8>, bool, Style, icon::Appearance),
    tip: crate::tooltip::Content,
    menu: MenuKey,
    taskbar: crate::taskbar::Content,
}
impl TrayView {
    fn of(s: &State, appearance: icon::Appearance) -> Self {
        let r = s.selected().and_then(|r| r.battery);
        let labels = s
            .rows
            .iter()
            .take(24)
            .map(|r| {
                format!(
                    "{}: {}",
                    s.row_name(r),
                    r.battery
                        .and_then(|b| b.percentage())
                        .map_or_else(|| "—".to_owned(), |p| format!("{p}%"))
                )
            })
            .collect();
        Self {
            icon: (
                r.map(|r| r.display),
                r.is_some_and(|b| matches!(b.charge, Charge::Charging | Charge::Full)),
                s.prefs.style.clone(),
                appearance,
            ),
            tip: crate::tooltip::Content::of(s),
            menu: MenuKey {
                locale: locale(&s.prefs),
                labels,
            },
            taskbar: crate::taskbar::Content::of(s, appearance.dark),
        }
    }
}
fn menu(key: &MenuKey) -> Menu {
    let locale = key.locale;
    let m = Menu::new();
    let mut rows = key
        .labels
        .iter()
        .enumerate()
        .map(|(i, label)| MenuItem::with_id(format!("d{i}"), label, false, None))
        .collect::<Vec<_>>();
    for (id, key) in [
        ("refresh", "tray.menu.refresh_now"),
        ("settings", "tray.settings"),
    ] {
        rows.push(MenuItem::with_id(id, label(locale, key), true, None))
    }
    for i in &rows {
        let _ = m.append(i);
    }
    let styles = tray_icon::menu::Submenu::new(label(locale, "tray.style"), true);
    for (id, key) in [
        ("capsule", "settings.tray_style.capsule"),
        ("vertical", "settings.tray_style.vertical"),
        ("number", "settings.tray_style.number"),
        ("circle", "settings.tray_style.circle"),
    ] {
        let _ = styles.append(&MenuItem::with_id(id, label(locale, key), true, None));
    }
    let _ = m.append(&styles);
    let _ = m.append(&tray_icon::menu::PredefinedMenuItem::separator());
    let _ = m.append(&MenuItem::with_id(
        "quit",
        label(locale, "tray.quit"),
        true,
        None,
    ));
    m
}
fn notify_level(old: Option<u8>, value: u8, threshold: u8) -> (Option<u8>, bool) {
    if value == 0 || threshold == 0 {
        return (old, false);
    }
    let last = old.unwrap_or(101);
    if value > threshold {
        return (if last <= threshold { Some(value) } else { old }, false);
    }
    if last > threshold || (value < last && (value <= 5 || last - value >= 5)) {
        (Some(value), true)
    } else {
        (old, false)
    }
}
fn migrate_alerts(old: &mut BTreeMap<String, u8>, known: &[battery_protocol::Descriptor]) -> bool {
    let mut changed = false;
    let old_keys: Vec<_> = old
        .keys()
        .filter(|k| k.starts_with("hid:") && !known.iter().any(|d| &d.id == *k))
        .cloned()
        .collect();
    for key in old_keys {
        let mut ds = known.iter().filter(|d| d.legacy_ids.contains(&key));
        if let Some(d) = ds.next()
            && ds.next().is_none()
            && let Some(v) = old.remove(&key)
        {
            old.entry(d.id.clone()).or_insert(v);
            changed = true;
        }
    }
    changed
}
fn read_alerts(dir: &std::path::Path) -> std::io::Result<BTreeMap<String, u8>> {
    let read = |name: &str| -> std::io::Result<BTreeMap<String, u8>> {
        let bytes = std::fs::read(dir.join(name))?;
        serde_json::from_slice(&bytes).map_err(std::io::Error::other)
    };
    match read("notifications-v2.json") {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => read("notifications.json"),
        x => x,
    }
}
fn save_alerts(levels: &BTreeMap<String, u8>) -> Result<(), String> {
    let p = Prefs::path()?;
    let dir = p.parent().ok_or("invalid path")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut t = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut t, levels).map_err(|e| e.to_string())?;
    t.as_file().sync_all().map_err(|e| e.to_string())?;
    t.persist(dir.join("notifications-v2.json"))
        .map_err(|e| e.error.to_string())?;
    Ok(())
}
fn balloon(tray: &tray_icon::TrayIcon, title: &str, text: &str) {
    let mut n = NOTIFYICONDATAW::default();
    n.cbSize = std::mem::size_of_val(&n) as _;
    n.hWnd = tray.window_handle() as _;
    let mut found = false;
    for id in 0..64 {
        let ni = NOTIFYICONIDENTIFIER {
            cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: n.hWnd,
            uID: id,
            ..Default::default()
        };
        let mut rect = RECT::default();
        if unsafe { Shell_NotifyIconGetRect(&ni, &mut rect) } == 0 {
            n.uID = id;
            found = true;
            break;
        }
    }
    if !found {
        return;
    }
    n.uFlags = NIF_INFO;
    n.dwInfoFlags = NIIF_INFO;
    for (dst, src) in n.szInfoTitle.iter_mut().take(63).zip(title.encode_utf16()) {
        *dst = src
    }
    for (dst, src) in n.szInfo.iter_mut().take(255).zip(text.encode_utf16()) {
        *dst = src
    }
    unsafe {
        Shell_NotifyIconW(NIM_MODIFY, &n);
    }
}
pub fn run(demo: bool) -> Result<(), AppError> {
    let _lease = if demo { None } else { Some(Lease::new()?) };
    let p = if demo {
        Prefs::default()
    } else {
        Prefs::load(&Prefs::path()?)?
    };
    let tid = unsafe { GetCurrentThreadId() };
    let mut msg = MSG::default();
    unsafe {
        PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
    }
    let (tx, rx) = mpsc::channel();
    let bus = Bus(tx, tid);
    let b = bus.clone();
    let svc = Service::start(p, demo, move || b.post(Msg::Data));
    if !demo {
        match crate::startup::enabled() {
            Ok(v) => svc.state.lock().unwrap().autostart = v,
            Err(e) => {
                eprintln!("Unable to read launch-at-startup status: {e}");
                svc.state.lock().unwrap().note = Some(Notice::AutostartStatusFailed);
            }
        };
    }
    let wb = bus.clone();
    let mut updater =
        crate::update::Updates::new(svc.state.clone(), demo, move || wb.post(Msg::Data));
    let recovering = std::env::args().any(|s| s == "--gpui-health-start")
        || std::env::var_os("WDBM_UPDATE_ROLLED_BACK").is_some();
    let mut update_schedule = crate::update::schedule::Schedule::new(
        !demo && svc.state.lock().unwrap().prefs.auto_update,
        recovering,
        Instant::now(),
    );
    let update_timer = UpdateTimer(if demo {
        0
    } else {
        unsafe { SetTimer(std::ptr::null_mut(), 0, 60_000, None) }
    });
    let _power = (!demo).then(|| Power::new(svc.signals.clone()));
    let _device_notifications = if demo {
        None
    } else {
        DeviceNotifications::new(bus.clone())
    };
    let s = svc.state.lock().unwrap().clone();
    let initial = TrayView::of(&s, icon::appearance());
    let mut tooltip = crate::tooltip::Tooltip::new(initial.tip.clone())?;
    let tb_bus = bus.clone();
    let mut taskbar = crate::taskbar::TaskbarWidget::new(
        initial.taskbar.clone(),
        tooltip.target(),
        move |action| match action {
            crate::taskbar::Action::OpenFlyout(at) => {
                tb_bus.post(Msg::Open(Mode::Flyout, Some(at)))
            }
            crate::taskbar::Action::OpenSettings => tb_bus.post(Msg::Open(Mode::Settings, None)),
            crate::taskbar::Action::Refresh => tb_bus.post(Msg::Action(Command::Refresh)),
            crate::taskbar::Action::SetPosition(pos) => {
                tb_bus.post(Msg::Action(Command::SetTaskbarPosition(pos)))
            }
            crate::taskbar::Action::SetLayout(layout) => {
                tb_bus.post(Msg::Action(Command::SetTaskbarLayout(layout)))
            }
            crate::taskbar::Action::SetBackdrop(on) => {
                tb_bus.post(Msg::Action(Command::SetTaskbarBackdrop(on)))
            }
            crate::taskbar::Action::ClearPinned => {
                tb_bus.post(Msg::Action(Command::ClearTaskbarDevices))
            }
            crate::taskbar::Action::Quit => tb_bus.post(Msg::Quit),
        },
    )?;
    let tray = TrayIconBuilder::new()
        .with_icon(icon::draw(
            initial.icon.0,
            initial.icon.1,
            &initial.icon.2,
            initial.icon.3,
        )?)
        .with_menu(Box::new(menu(&initial.menu)))
        .with_menu_on_left_click(false)
        .build()
        .map_err(|e| e.to_string())?;
    let b = bus.clone();
    let tip_target = tooltip.target();
    TrayIconEvent::set_event_handler(Some(move |e| {
        tip_target.event(&e);
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            rect,
            ..
        } = e
        {
            b.post(Msg::Open(
                Mode::Flyout,
                Some([
                    rect.position.x as i32 + (rect.size.width / 2) as i32,
                    rect.position.y as i32,
                ]),
            ))
        }
    }));
    let b = bus.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| match e.id.0.as_str() {
        "quit" => b.post(Msg::Quit),
        "settings" => b.post(Msg::Open(Mode::Settings, None)),
        "capsule" => b.post(Msg::Action(Command::SetStyle(Style::Capsule))),
        "vertical" => b.post(Msg::Action(Command::SetStyle(Style::Vertical))),
        "number" => b.post(Msg::Action(Command::SetStyle(Style::Number))),
        "circle" => b.post(Msg::Action(Command::SetStyle(Style::Circle))),
        "refresh" => b.post(Msg::Action(Command::Refresh)),
        _ => (),
    }));
    let mut gui: Option<WindowProcess> = None;
    let mut serial = 0;
    let mut pending = None;
    let mut last = None;
    let mut tray_view = initial;
    let mut alert: BTreeMap<String, u8> = if demo {
        Default::default()
    } else {
        Prefs::path()?
            .parent()
            .and_then(|p| read_alerts(p).ok())
            .unwrap_or_default()
    };
    let mut timer = 0;
    bus.post(Msg::Data);
    if demo && std::env::args().any(|x| x == "--exercise-tray") {
        let b = bus.clone();
        thread::spawn(move || {
            for l in std::io::stdin().lock().lines().map_while(Result::ok) {
                match l.trim() {
                    "settings" => b.post(Msg::Open(Mode::Settings, None)),
                    "flyout" => b.post(Msg::Open(Mode::Flyout, None)),
                    "quit" => break,
                    _ => (),
                }
            }
            b.post(Msg::Quit);
        });
    }
    if !demo && std::env::args().any(|s| s == "--gpui-health-start") {
        println!("GPUI-HEALTH/1");
        let _ = std::io::stdout().flush();
    }
    let probe_mode = demo && std::env::args().any(|s| s == "--exercise-tray");
    let mut handing_off = false;
    let mut automatic_handoff = false;
    'pump: loop {
        let rc = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if rc <= 0 {
            break;
        }
        if msg.message == WM_TIMER && update_timer.0 != 0 && msg.wParam == update_timer.0 {
            bus.post(Msg::Data);
        }
        if msg.message == WM_TIMER
            && msg.hwnd.is_null()
            && gui.as_ref().is_some_and(|g| {
                (!g.ready || g.closing) && g.ready_timer != 0 && g.ready_timer == msg.wParam
            })
        {
            let stalled_start = gui.as_ref().is_some_and(|g| !g.closing);
            if let Some(mut g) = gui.take() {
                g.stop_ready_timer();
                let _ = g.child.kill();
                g.closing = true;
                drop(g);
            }
            if stalled_start {
                balloon(
                    &tray,
                    "GPUI",
                    label(
                        locale(&svc.state.lock().unwrap().prefs),
                        "window.start_timeout",
                    ),
                );
            }
            if let Some((mode, a)) = pending.take() {
                bus.post(Msg::Open(mode, a));
            }
        }
        if msg.message == WM_TIMER && msg.hwnd.is_null() && timer != 0 && msg.wParam == timer {
            unsafe {
                KillTimer(std::ptr::null_mut(), timer);
            }
            timer = 0;
            if let Some(g) = gui.as_mut()
                && g.hidden
                    .is_some_and(|t| t.elapsed() >= Duration::from_secs(30))
            {
                g.exit();
            }
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        while let Ok(m) = rx.try_recv() {
            match m {
                Msg::Quit => break 'pump,
                Msg::DeviceChanged => svc.device_changed(),
                Msg::Data => {
                    let s = svc.state.lock().unwrap().clone();
                    // Keep independent projections: a new timestamp may matter to a visible
                    // card, never to a tray bitmap or HMENU. Receipts must not be discarded.
                    let next = TrayView::of(&s, icon::appearance());
                    if next.icon != tray_view.icon {
                        let (p, c, style, a) = &next.icon;
                        tray.set_icon(Some(icon::draw(*p, *c, style, *a)?))
                            .map_err(|e| e.to_string())?;
                    }
                    if next.tip != tray_view.tip {
                        tooltip.update(next.tip.clone());
                    }
                    if next.menu != tray_view.menu {
                        tray.set_menu(Some(Box::new(menu(&next.menu))));
                    }
                    if next.taskbar != tray_view.taskbar {
                        taskbar.update(next.taskbar.clone());
                    }
                    tray_view = next;
                    use crate::update::schedule::Action;
                    match update_schedule.tick(
                        !demo && s.prefs.auto_update,
                        updater.is_busy(),
                        &s.update,
                        Instant::now(),
                    ) {
                        Some(Action::Check) => updater.automatic_check(),
                        Some(Action::Download) => updater.run(true),
                        Some(Action::Install) => bus.post(Msg::Install(true)),
                        Some(Action::Cancel) => updater.cancel(),
                        None => (),
                    }
                    if last.as_ref() == Some(&s) {
                        continue;
                    }
                    let mut changed = migrate_alerts(&mut alert, &s.candidates);
                    for r in &s.rows {
                        if let Some(v) = r.battery {
                            if r.error.is_some()
                                || matches!(v.charge, Charge::Charging | Charge::Full)
                            {
                                continue;
                            }
                            let old = alert.get(&r.device.id).copied();
                            let (next, notify) = notify_level(old, v.raw, s.prefs.low);
                            if next != old {
                                if let Some(x) = next {
                                    alert.insert(r.device.id.clone(), x);
                                }
                                changed = true;
                            }
                            if notify && !demo {
                                balloon(
                                    &tray,
                                    label(locale(&s.prefs), "settings.low_battery.title"),
                                    &format!("{} · {}%", s.row_name(r), v.raw),
                                );
                            }
                        }
                    }
                    if changed
                        && !demo
                        && let Err(e) = save_alerts(&alert)
                    {
                        eprintln!("notification persistence: {e}");
                    }
                    if let Some(g) = &gui
                        && g.visible
                        && !g.closing
                    {
                        g.send(Event::State(Box::new(s.clone())))
                    }
                    last = Some(s)
                }
                Msg::Open(mode, a) => {
                    if probe_mode {
                        eprintln!(
                            "intent {:?}, before {:?}",
                            mode,
                            gui.as_ref()
                                .map(|g| (g.id, g.visible, g.mode, g.generation))
                        );
                    }
                    if timer != 0 {
                        unsafe {
                            KillTimer(std::ptr::null_mut(), timer);
                        }
                        timer = 0
                    }
                    if gui.as_ref().is_some_and(|g| g.closing) {
                        pending = Some((mode, a));
                        continue;
                    }
                    if gui.is_none() {
                        serial += 1;
                        let g = match WindowProcess::spawn(serial, bus.clone()) {
                            Ok(g) => g,
                            Err(e) => {
                                eprintln!("Could not start the window: {e}");
                                balloon(
                                    &tray,
                                    "GPUI",
                                    label(
                                        locale(&svc.state.lock().unwrap().prefs),
                                        "window.start_failed",
                                    ),
                                );
                                continue;
                            }
                        };
                        gui = Some(g)
                    }
                    let g = gui.as_mut().unwrap();
                    if mode == Mode::Flyout && g.visible && g.mode == mode {
                        g.generation = g.generation.wrapping_add(1);
                        g.visible = false;
                        g.send(Event::Hide(g.generation));
                    } else {
                        g.send(Event::State(Box::new(svc.state.lock().unwrap().clone())));
                        g.open(mode, a)
                    }
                }
                Msg::Gone(id) => {
                    if gui.as_ref().is_some_and(|g| g.id == id) {
                        if let Some(mut g) = gui.take() {
                            let unexpected = !g.closing
                                && (!g.ready
                                    || g.child
                                        .try_wait()
                                        .ok()
                                        .flatten()
                                        .is_some_and(|s| !s.success()));
                            drop(g);
                            if unexpected {
                                balloon(
                                    &tray,
                                    "GPUI",
                                    label(
                                        locale(&svc.state.lock().unwrap().prefs),
                                        "window.exited",
                                    ),
                                );
                            }
                        }
                        if let Some((mode, a)) = pending.take() {
                            bus.post(Msg::Open(mode, a))
                        }
                    }
                }
                Msg::Gui(id, c) => {
                    let Some(g) = gui.as_mut().filter(|g| g.id == id) else {
                        continue;
                    };
                    if probe_mode {
                        eprintln!("gui {} {:?}", id, c);
                    }
                    match c {
                        Command::Presented(generation) => {
                            if probe_mode && generation == g.generation {
                                println!("PRESENTED {id} {generation}");
                                let _ = std::io::stdout().flush();
                            }
                        }
                        Command::Hidden(generation) if generation == g.generation => {
                            g.visible = false;
                            g.hidden = Some(Instant::now());
                            if timer != 0 {
                                unsafe {
                                    KillTimer(std::ptr::null_mut(), timer);
                                }
                            }
                            timer = unsafe { SetTimer(std::ptr::null_mut(), REAP, 30_050, None) }
                        }
                        Command::Switched(mode, v) if v == g.generation => g.mode = mode,
                        Command::Ready => {
                            g.ready = true;
                            if !g.closing {
                                g.stop_ready_timer();
                            }
                        }
                        Command::Switched(_, _) | Command::Hidden(_) => (),
                        Command::QuitGui => g.exit(),
                        c => bus.post(Msg::Action(c)),
                    }
                }
                Msg::Action(Command::InstallUpdate) => {
                    update_schedule.manual(Instant::now());
                    bus.post(Msg::Install(false));
                }
                Msg::Install(automatic)
                    if !handing_off
                        && (!automatic || svc.state.lock().unwrap().prefs.auto_update) =>
                {
                    match updater.install_input() {
                        Ok((path, man)) => {
                            handing_off = true;
                            automatic_handoff = automatic;
                            let b = bus.clone();
                            thread::spawn(move || {
                                b.post(Msg::UpdateHandoff(crate::swap::prepare(&path, &man)))
                            });
                        }
                        Err(e) => {
                            eprintln!("Update installation failed: {e}");
                            let mut state = svc.state.lock().unwrap();
                            state.note =
                                Some(Notice::UpdateFailed(crate::update::Failure::Install));
                            state.update =
                                crate::update::Status::Error(crate::update::Failure::Install);
                            bus.post(Msg::Data);
                        }
                    }
                }
                Msg::UpdateHandoff(Ok(mut pipe)) => {
                    if updater.install_input().is_err()
                        || (automatic_handoff && !svc.state.lock().unwrap().prefs.auto_update)
                    {
                        // Closing the pipe sends EOF, so an armed helper cannot commit.
                        handing_off = false;
                        continue;
                    }
                    let error = match writeln!(pipe, "GPUI-DEPART/1").and_then(|_| pipe.flush()) {
                        Ok(()) => break 'pump,
                        Err(e) => e,
                    };
                    eprintln!("Update handoff failed: {error}");
                    handing_off = false;
                    svc.state.lock().unwrap().note =
                        Some(Notice::UpdateFailed(crate::update::Failure::Handoff));
                    bus.post(Msg::Data);
                }
                Msg::UpdateHandoff(Err(e)) => {
                    handing_off = false;
                    eprintln!("Update handoff failed: {e}");
                    let mut state = svc.state.lock().unwrap();
                    state.note = Some(Notice::UpdateFailed(crate::update::Failure::Handoff));
                    state.update = crate::update::Status::Error(crate::update::Failure::Handoff);
                    bus.post(Msg::Data);
                }
                Msg::Install(_) => (),
                Msg::Action(Command::CheckUpdate) => {
                    update_schedule.manual(Instant::now());
                    updater.run(false);
                }
                Msg::Action(Command::DownloadUpdate) => {
                    update_schedule.manual(Instant::now());
                    updater.run(true);
                }
                Msg::Action(Command::CancelUpdate) => {
                    update_schedule.manual(Instant::now());
                    updater.cancel();
                }
                Msg::Action(Command::OpenUpdate) => {
                    if let Err(e) = updater.open_dir() {
                        eprintln!("Unable to open the update folder: {e}");
                        svc.state.lock().unwrap().note = Some(Notice::UpdateFolderFailed);
                        bus.post(Msg::Data)
                    }
                }
                Msg::Action(c) => {
                    if let Err(e) = svc.command(c) {
                        eprintln!("Application command failed: {e}");
                        svc.state.lock().unwrap().note = Some(Notice::OperationFailed);
                    }
                    bus.post(Msg::Data)
                }
            }
        }
    }
    drop(gui);
    Ok(())
}
#[cfg(test)]
mod alert_tests {
    use super::*;
    #[test]
    fn thresholds() {
        let mut last = None;
        let mut seen = Vec::new();
        for v in [92, 20, 20, 19, 16, 15, 6, 5, 4, 0, 42, 18] {
            let (n, alert) = notify_level(last, v, 20);
            last = n;
            if alert {
                seen.push(v)
            }
        }
        assert_eq!(seen, [20, 15, 6, 5, 4, 18]);
    }
}

#[cfg(test)]
mod audit_tray {
    use super::*;

    #[test]
    fn auto_tooltip_is_independent_of_taskbar_theme() {
        let mut state = State::demo();
        state.prefs.theme = Theme::Auto;
        let light_taskbar = TrayView::of(
            &state,
            icon::Appearance {
                dark: false,
                size: 48,
            },
        );
        let dark_taskbar = TrayView::of(
            &state,
            icon::Appearance {
                dark: true,
                size: 48,
            },
        );
        assert_ne!(light_taskbar.icon, dark_taskbar.icon);
        assert_eq!(light_taskbar.tip, dark_taskbar.tip);
    }

    #[test]
    fn switching_between_english_and_russian_invalidates_the_native_tray_menu() {
        let appearance = icon::Appearance {
            dark: false,
            size: 48,
        };
        let mut state = State::demo();
        state.prefs.lang = Language::En;
        let before = TrayView::of(&state, appearance);
        state.prefs.lang = Language::Ru;
        let after = TrayView::of(&state, appearance);
        assert_ne!(before.menu, after.menu);
        assert_ne!(before.tip, after.tip);
        assert_eq!(before.icon, after.icon);
    }

    #[test]
    fn icon_only_identity_change_invalidates_the_tray_tooltip() {
        let appearance = icon::Appearance {
            dark: false,
            size: 48,
        };
        let mut state = State::demo();
        let before = TrayView::of(&state, appearance);
        state.rows[0].device.identity = Some(battery_protocol::DeviceIdentity {
            model_key: None,
            category: battery_protocol::DeviceCategory::Unknown,
            connection: battery_protocol::ConnectionKind::Rf24,
            battery: battery_protocol::BatteryCapability::Present,
            model_source: None,
        });
        let after = TrayView::of(&state, appearance);
        assert_eq!(before.icon, after.icon);
        assert_eq!(before.menu, after.menu);
        assert_ne!(before.tip, after.tip);
        assert_ne!(before, after);
    }

    #[test]
    fn quit_is_the_last_menu_item_in_all_three_languages() {
        for locale in [Locale::Zh, Locale::En, Locale::Ru] {
            let m = menu(&MenuKey {
                locale,
                labels: vec!["Mouse: 69%".into()],
            });
            assert_eq!(m.items().last().unwrap().id().0, "quit");
        }
    }

    #[test]
    fn tray_keys_do_not_swallow_receipts_or_rebuild_for_timestamp() {
        let a = icon::Appearance {
            dark: false,
            size: 48,
        };
        let mut st = State::demo();
        st.prefs.lang = Language::En;
        let v = TrayView::of(&st, a);
        let base = st.clone();
        st.rows[0].sampled = 500;
        assert_eq!(v, TrayView::of(&st, a));
        assert!(base.same_except_sample_time(&st));
        st.manual.id = 123;
        assert!(!base.same_except_sample_time(&st));
        assert_eq!(v, TrayView::of(&st, a));
        st.prefs
            .aliases
            .insert(st.rows[0].device.id.clone(), "Alias".into());
        let named = TrayView::of(&st, a);
        assert_eq!(named.icon, v.icon);
        assert_ne!(named.menu, v.menu);
        assert_ne!(named.tip, v.tip);
        let themed = TrayView::of(
            &st,
            icon::Appearance {
                dark: true,
                size: 48,
            },
        );
        assert_ne!(themed.icon, named.icon);
        assert_eq!(themed.menu, named.menu);

        let before_pin = TrayView::of(&st, a);
        let id = st.rows[0].device.id.clone();
        assert!(
            st.prefs
                .change(&Command::ToggleTaskbarDevice(id), &st.candidates)
        );
        let after_pin = TrayView::of(&st, a);
        assert_eq!(after_pin.icon, before_pin.icon);
        assert_eq!(after_pin.menu, before_pin.menu);
        assert_ne!(after_pin.taskbar, before_pin.taskbar);
        assert_eq!(after_pin.taskbar.items.len(), 1);
    }
    #[test]
    fn unread_pipe_does_not_block_state_or_exit_producer() {
        use std::sync::mpsc::channel;
        let (reader, writer) = std::io::pipe().unwrap();
        let mail: BoxedMail = Default::default();
        let producer = mail.clone();
        let wr = mail.clone();
        let (started, write_started) = channel();
        struct Ping<W>(W, Option<Sender<()>>);
        impl<W: Write> Write for Ping<W> {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                if let Some(s) = self.1.take() {
                    s.send(()).unwrap();
                }
                self.0.write(b)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.0.flush()
            }
        }
        let worker = thread::spawn(move || write_mail(wr, Ping(writer, Some(started))));
        let big = State {
            release_notes: "x".repeat(4 << 20),
            ..Default::default()
        };
        enqueue(&mail, Event::State(Box::new(big)));
        write_started.recv_timeout(Duration::from_secs(3)).unwrap();
        let (sent, done) = channel();
        let handle = thread::spawn(move || {
            for i in 0..1000 {
                enqueue(
                    &producer,
                    Event::State(Box::new(State {
                        release_size: i,
                        ..Default::default()
                    })),
                );
            }
            enqueue(&producer, Event::Exit);
            sent.send(()).unwrap();
        });
        let response = done.recv_timeout(Duration::from_secs(2));
        drop(reader);
        worker.join().unwrap();
        handle.join().unwrap();
        assert!(response.is_ok());
        let m = mail.0.lock().unwrap();
        assert!(m.closed && m.state.is_none());
    }
}

#[cfg(test)]
mod profile_rollback_tests {
    use super::*;
    #[test]
    fn alert_history_forks_only_on_missing_file() {
        let d = tempfile::tempdir().unwrap();
        let old = d.path().join("notifications.json");
        let new = d.path().join("notifications-v2.json");
        std::fs::write(&old, br#"{"a":4}"#).unwrap();
        assert_eq!(read_alerts(d.path()).unwrap()["a"], 4);
        std::fs::write(&new, br#"{"a":2}"#).unwrap();
        assert_eq!(read_alerts(d.path()).unwrap()["a"], 2);
        std::fs::write(&new, b"oops").unwrap();
        assert!(read_alerts(d.path()).is_err());
        assert_eq!(std::fs::read(old).unwrap(), br#"{"a":4}"#);
    }
    #[test]
    fn a_valid_zero_is_not_a_new_notification() {
        assert!(!notify_level(None, 0, 20).1);
        assert!(!notify_level(Some(1), 0, 20).1);
        assert!(!notify_level(None, 1, 0).1);
        assert!(notify_level(None, 1, 20).1);
    }
}

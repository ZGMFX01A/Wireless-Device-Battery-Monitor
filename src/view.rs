//! Only this process starts GPUI/Direct3D. No HID, GATT, updater or web engine here.
use crate::model::Style;
use crate::model::*;
use battery_protocol::{Charge, Kind};
use gpui::{MouseButton as Button, prelude::*, *};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::io::{BufRead, Write};
use windows_sys::Win32::{
    Foundation::{HWND, POINT},
    Graphics::Gdi::*,
    UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::*},
};
fn hwnd(w: &Window) -> HWND {
    match HasWindowHandle::window_handle(w).map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get() as HWND,
        _ => std::ptr::null_mut(),
    }
}
fn send(c: &Command) {
    let mut o = std::io::stdout().lock();
    let _ = serde_json::to_writer(&mut o, c);
    let _ = o.write_all(b"\n");
    let _ = o.flush();
}
fn place(w: &Window, m: Mode, a: Option<[i32; 2]>, show: bool) {
    let h = hwnd(w);
    if h.is_null() {
        return;
    }
    unsafe {
        let mut p = POINT::default();
        if let Some([x, y]) = a {
            p = POINT { x, y }
        } else {
            GetCursorPos(&mut p);
        }
        let mon = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as _,
            ..Default::default()
        };
        if GetMonitorInfoW(mon, &mut mi) == 0 {
            return;
        }
        let r = mi.rcWork;
        let mut dx = 0u32;
        let mut dy = 0u32;
        let dpi = if windows_sys::Win32::UI::HiDpi::GetDpiForMonitor(
            mon,
            windows_sys::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI,
            &mut dx,
            &mut dy,
        ) == 0
        {
            dx
        } else {
            GetDpiForWindow(h).max(96)
        };
        let (bw, bh) = if m == Mode::Flyout {
            (380, 540)
        } else {
            (540, 680)
        };
        let ww = ((bw * dpi / 96) as i32).min(r.right - r.left);
        let hh = ((bh * dpi / 96) as i32).min(r.bottom - r.top);
        // SetWindowPos takes the outer rectangle; GPUI can add a non-client frame after first show.
        // Target the logical CLIENT size on every reopen, including mode changes.
        let (mut client, mut outer) = (
            windows_sys::Win32::Foundation::RECT::default(),
            windows_sys::Win32::Foundation::RECT::default(),
        );
        GetClientRect(h, &mut client);
        GetWindowRect(h, &mut outer);
        let ww = (ww + (outer.right - outer.left - client.right).max(0)).min(r.right - r.left);
        let hh = (hh + (outer.bottom - outer.top - client.bottom).max(0)).min(r.bottom - r.top);
        let gap = 12;
        let (x, y) = if m == Mode::Settings {
            ((r.left + r.right - ww) / 2, (r.top + r.bottom - hh) / 2)
        } else {
            (
                (p.x - ww / 2).clamp(r.left, (r.right - ww).max(r.left)),
                (if p.y < r.top + gap {
                    r.top + gap
                } else {
                    p.y - hh - gap
                })
                .clamp(r.top, (r.bottom - hh).max(r.top)),
            )
        };
        SetWindowPos(
            h,
            if m == Mode::Flyout {
                HWND_TOPMOST
            } else {
                HWND_NOTOPMOST
            },
            x,
            y,
            ww,
            hh,
            if show { SWP_SHOWWINDOW } else { SWP_NOACTIVATE },
        );
        if show {
            ShowWindow(h, SW_SHOW);
            SetForegroundWindow(h);
        }
        // The synchronous WM_SIZE above can arrive inside WindowHandle::update. Re-dispatch the
        // actual client extent outside that GPUI borrow so both its scene and hit tests see it.
        let mut actual = windows_sys::Win32::Foundation::RECT::default();
        if GetClientRect(h, &mut actual) != 0 {
            let lp = (actual.right as u16 as usize) | ((actual.bottom as u16 as usize) << 16);
            PostMessageW(h, WM_SIZE, SIZE_RESTORED as usize, lp as isize);
        }
    }
}
fn system_dark() -> bool {
    use windows_sys::Win32::System::Registry::*;
    let mut v = 1u32;
    let mut n = 4;
    let key: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
        .encode_utf16()
        .collect();
    let name: Vec<u16> = "AppsUseLightTheme\0".encode_utf16().collect();
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut v as *mut u32).cast(),
            &mut n,
        ) == 0
            && v == 0
    }
}
pub fn english(p: &Prefs) -> bool {
    match p.lang {
        Language::Zh => false,
        Language::En => true,
        Language::Auto => unsafe {
            windows_sys::Win32::Globalization::GetUserDefaultUILanguage() & 0x3ff != 4
        },
    }
}
struct Panel {
    state: State,
    mode: Mode,
    generation: u64,
    visible: bool,
    managed: bool,
    has_focus: bool,
    dark_auto: bool,
    last_material: Option<bool>,
    live: bool,
    pending_rows: Option<Vec<Row>>,
    settings_open: bool,
    scroll: [ScrollHandle; 2],
    drag_scroll: Option<f32>,
    last_scroll: (i32, i32),
    popup: bool,
    modal: Option<Dialog>,
    focus: FocusHandle,
    edit_blur: Option<Subscription>,
    manual: Option<(u64, bool)>,
    toast: Option<(String, u64)>,
    editor: Option<(String, Entity<crate::entry::TextInput>)>,
    _sub: Option<Subscription>,
}
impl Panel {
    fn hide(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if !self.managed {
            cx.quit();
            return;
        }
        if self.visible {
            self.visible = false;
            self.has_focus = false;
            unsafe {
                ShowWindow(hwnd(w), SW_HIDE);
            }
            send(&Command::Hidden(self.generation));
        }
    }
    fn tr(&self, zh: &'static str, en: &'static str) -> &'static str {
        if english(&self.state.prefs) { en } else { zh }
    }
    fn issue(&mut self, c: Command, cx: &mut Context<Self>) {
        if self.managed {
            send(&c)
        } else {
            if let Command::DeviceRequest(ref j) = c {
                self.state.demo_request(j);
                self.reconcile(cx);
                cx.notify();
                return;
            }
            if let Command::Sync(id, scan) = c {
                let mut st = self.state.clone();
                st.manual = Manual {
                    id,
                    hid: true,
                    bt: true,
                    error: None,
                };
                self.manual = Some((id, scan));
                self.accept(st, cx);
                return;
            }
            if matches!(c, Command::CheckUpdate) {
                self.state.update = crate::update::Status::Error(
                    self.tr(
                        "预览模式不访问更新服务",
                        "Preview does not contact the update service",
                    )
                    .into(),
                );
                cx.notify();
                return;
            }
            if matches!(c, Command::Autostart) {
                self.state.autostart = !self.state.autostart;
                cx.notify();
                return;
            }
            let list = self.state.candidates.clone();
            if !self.state.prefs.change(&c, &list) {
                self.state.note = self
                    .tr("预览模式不访问外设", "Preview does not access hardware")
                    .to_owned()
            }
        }
        if !self.managed {
            self.state.demo_reconcile()
        }
        cx.notify()
    }
    fn t(&self, k: &str) -> &'static str {
        strings::label(english(&self.state.prefs), k)
    }
    fn accept(&mut self, mut st: State, cx: &mut Context<Self>) {
        let completed = self
            .manual
            .is_some_and(|(id, _)| st.manual.id == id && st.manual.completed());
        let members_changed = st.prefs.keyboard != self.state.prefs.keyboard
            || st.prefs.bluetooth != self.state.prefs.bluetooth;
        if !self.live && !completed && !members_changed {
            self.pending_rows = Some(std::mem::replace(
                &mut st.rows,
                std::mem::take(&mut self.state.rows),
            ));
        } else {
            self.pending_rows = None;
        }
        let note = (st.note != self.state.note && !st.note.is_empty()).then(|| st.note.clone());
        self.state = st;
        if completed {
            let (_, scan) = self.manual.take().unwrap();
            let note = self.state.manual.error.clone().unwrap_or_else(|| {
                self.t(if scan {
                    "toast.scan_completed"
                } else {
                    "toast.refresh_completed"
                })
                .into()
            });
            self.toast(note, cx)
        } else if let Some(n) = note {
            self.toast(n, cx)
        }
    }
}
#[path = "ui/assets.rs"]
mod assets;
#[path = "ui/dialogs.rs"]
mod dialogs;
#[path = "ui/material.rs"]
mod material;
#[path = "ui/panels.rs"]
mod panels;
#[path = "ui/skin.rs"]
mod skin;
#[path = "ui/strings.rs"]
mod strings;
#[derive(Clone)]
enum Dialog {
    Pick {
        kind: Kind,
        id: u64,
        chosen: Option<String>,
        binding: bool,
    },
    Remove(String, Kind, String),
    Update,
}
/// Reader never waits on the drawing thread. One latest snapshot plus one latest
/// window intent per burst; exit is terminal. Send a wake token, not another copy.
#[derive(Default)]
struct UiInbox {
    state: Option<Box<State>>,
    intent: Option<Event>,
    exiting: bool,
}
impl UiInbox {
    fn push(&mut self, e: Event) {
        if self.exiting {
            return;
        }
        match e {
            Event::State(s) => self.state = Some(s),
            Event::Exit => {
                self.state = None;
                self.intent = None;
                self.exiting = true;
            }
            e => self.intent = Some(e),
        }
    }
    fn drain(&mut self) -> Vec<Event> {
        if self.exiting {
            return vec![Event::Exit];
        }
        self.state
            .take()
            .map(Event::State)
            .into_iter()
            .chain(self.intent.take())
            .collect()
    }
}
pub fn run(managed: bool, initial: Mode) {
    let (tx, rx) = async_channel::bounded::<()>(1);
    let inbox = std::sync::Arc::new(std::sync::Mutex::new(UiInbox::default()));
    if managed {
        let wr = inbox.clone();
        std::thread::spawn(move || {
            let enqueue = |e| {
                wr.lock().unwrap().push(e);
                !matches!(tx.try_send(()), Err(async_channel::TrySendError::Closed(_)))
            };
            for l in std::io::stdin().lock().lines().map_while(Result::ok) {
                if let Ok(e) = serde_json::from_str(&l) {
                    let end = matches!(e, Event::Exit);
                    if !enqueue(e) || end {
                        return;
                    }
                }
            }
            enqueue(Event::Exit);
        });
    }
    Application::new()
        .with_assets(assets::Icons)
        .run(move |cx| {
            crate::entry::register(cx);
            cx.on_window_closed(|cx| cx.quit()).detach();
            let bounds = Bounds::centered(None, size(px(380.), px(540.)), cx);
            let h = cx.open_window(
                WindowOptions {
                    titlebar: None,
                    show: !managed,
                    focus: !managed,
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_background: WindowBackgroundAppearance::Blurred,
                    is_resizable: false,
                    ..Default::default()
                },
                |w, cx| {
                    cx.new(|cx| {
                        material::apply(hwnd(w), system_dark());
                        w.set_window_title("无线设备电量 · GPUI");
                        let sub = cx.observe_window_activation(w, |s: &mut Panel, w, cx| {
                            if w.is_window_active() {
                                s.has_focus = true
                            } else if s.managed && s.has_focus && s.mode == Mode::Flyout {
                                s.hide(w, cx)
                            }
                        });
                        let focus = cx.focus_handle();
                        w.focus(&focus);
                        Panel {
                            state: if managed {
                                Default::default()
                            } else {
                                State::demo()
                            },
                            mode: initial,
                            generation: 0,
                            visible: !managed,
                            has_focus: false,
                            managed,
                            dark_auto: system_dark(),
                            last_material: None,
                            live: true,
                            pending_rows: None,
                            settings_open: initial == Mode::Settings,
                            scroll: Default::default(),
                            drag_scroll: None,
                            last_scroll: (0, 0),
                            popup: false,
                            modal: None,
                            focus,
                            edit_blur: None,
                            manual: None,
                            toast: None,
                            editor: None,
                            _sub: Some(sub),
                        }
                    })
                },
            );
            let h = match h {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("GPUI window: {e}");
                    cx.quit();
                    return;
                }
            };
            if managed {
                send(&Command::Ready);
                cx.spawn(async move |cx| {
                    'reader: while rx.recv().await.is_ok() {
                        let events = inbox.lock().unwrap().drain();
                        for ev in events {
                            if matches!(ev, Event::Exit) {
                                let _ = cx.update(|app| app.quit());
                                break 'reader;
                            }
                            if h.update(cx, |s, w, cx| {
                                match ev {
                                    Event::State(st) => {
                                        let repaint = (s.visible && !s.settings_open && s.live)
                                            || !s.state.same_except_sample_time(&st);
                                        let before = s.skin().dark;
                                        s.accept(*st, cx);
                                        if s.skin().dark != before {
                                            material::apply(hwnd(w), s.skin().dark);
                                        }
                                        s.reconcile(cx);
                                        if !repaint {
                                            return;
                                        }
                                    }
                                    Event::Open(m, g, a) => {
                                        s.generation = g;
                                        s.mode = m;
                                        s.settings_open = m == Mode::Settings;
                                        s.modal = None;
                                        s.popup = false;
                                        s.editor = None;
                                        s.edit_blur = None;
                                        w.focus(&s.focus);
                                        s.dark_auto = system_dark();
                                        s.visible = true;
                                        s.has_focus = false;
                                        place(w, m, a, true);
                                        material::apply(hwnd(w), s.skin().dark);
                                        w.activate_window();
                                        // Windows may apply GPUI's initial hidden-window placement during the first ShowWindow.
                                        // Correct the actual client bounds after that transition, without stealing focus.
                                        let weak = cx.weak_entity();
                                        w.on_next_frame(move |w, cx| {
                                            let _ = weak.update(cx, |s, _| {
                                                if s.visible && s.generation == g {
                                                    place(w, m, a, false);
                                                    send(&Command::Presented(g));
                                                }
                                            });
                                        });
                                    }
                                    Event::Hide(g) if g >= s.generation => {
                                        s.generation = g;
                                        if s.visible {
                                            s.hide(w, cx)
                                        } else {
                                            send(&Command::Hidden(g));
                                        }
                                    }
                                    _ => (),
                                }
                                cx.notify()
                            })
                            .is_err()
                            {
                                break 'reader;
                            }
                        }
                    }
                })
                .detach()
            } else {
                cx.activate(true);
            }
        })
}

#[cfg(test)]
mod audit_child_mail {
    use super::UiInbox;
    use crate::model::{Event, Mode, State};
    #[test]
    fn last_state_and_intent_before_terminal_exit() {
        let mut q = UiInbox::default();
        for i in 0..10000 {
            q.push(Event::State(Box::new(State {
                release_size: i,
                ..Default::default()
            })));
        }
        q.push(Event::Hide(1));
        q.push(Event::Open(Mode::Settings, 2, None));
        let mut v = q.drain().into_iter();
        assert!(matches!(v.next(),Some(Event::State(s)) if s.release_size==9999));
        assert!(matches!(
            v.next(),
            Some(Event::Open(Mode::Settings, 2, None))
        ));
        assert!(v.next().is_none());
        q.push(Event::Exit);
        q.push(Event::State(Box::default()));
        assert!(matches!(&q.drain()[..], [Event::Exit]));
    }
}

//! A small, nonactivating tray tooltip. It lives on the host message-pump thread
//! and uses GDI only; hovering never starts the GPUI child or touches devices.
use crate::{
    model::{State, Theme},
    view::{english, skin::BatteryTone},
};
use battery_protocol::Charge;
use std::{cell::RefCell, collections::BTreeMap, marker::PhantomData, rc::Rc};
use tray_icon::TrayIconEvent;
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{HiDpi::*, WindowsAndMessaging::*},
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Device {
    name: String,
    value: String,
    tone: BatteryTone,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    rows: Vec<Device>,
    dark: bool,
    en: bool,
}
impl Content {
    pub fn of(state: &State, system_dark: bool) -> Self {
        let en = english(&state.prefs);
        Self {
            rows: state
                .rows
                .iter()
                .filter(|r| state.prefs.bound(&r.device))
                .map(|r| {
                    let online = r.error.is_none();
                    let battery = r.battery.filter(|_| online);
                    Device {
                        name: state
                            .prefs
                            .name(&r.device)
                            .chars()
                            .take(180)
                            .map(|c| if c.is_control() { ' ' } else { c })
                            .collect(),
                        value: if !online {
                            if en { "Offline" } else { "离线" }.into()
                        } else {
                            battery.map_or_else(
                                || "—".into(),
                                |b| {
                                    format!(
                                        "{}%{}",
                                        b.display,
                                        if b.charge == Charge::Charging {
                                            " ⚡"
                                        } else {
                                            ""
                                        }
                                    )
                                },
                            )
                        },
                        tone: BatteryTone::for_sample(
                            battery.is_some(),
                            battery.is_some_and(|b| {
                                matches!(b.charge, Charge::Charging | Charge::Full)
                            }),
                            battery.map_or(0, |b| b.display),
                        ),
                    }
                })
                .collect(),
            dark: match state.prefs.theme {
                Theme::Auto => system_dark,
                Theme::Light => false,
                Theme::Dark => true,
            },
            en,
        }
    }
    fn accessible_text(&self) -> String {
        if self.rows.is_empty() {
            return if self.en {
                "No devices"
            } else {
                "暂无设备"
            }
            .into();
        }
        self.rows
            .iter()
            .map(|r| format!("{}: {}", r.name, r.value))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

const ENTER: u32 = WM_APP + 1;
const HIDE: u32 = WM_APP + 2;
const HOVER_TIMER: usize = 1;
const CLASS: &str = "MouseBatteryTooltip";
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}
fn rgb(v: u32) -> COLORREF {
    (v & 0xff00) | ((v >> 16) & 0xff) | ((v & 0xff) << 16)
}
#[derive(Clone)]
struct Popup {
    content: Content,
    anchor: RECT,
    dpi: u32,
    count: usize,
    hovering: bool,
}
thread_local! {
    // Window messages can reenter the host. Never keep a borrow across a
    // window-management call; painting takes an owned snapshot instead.
    static POPUPS: RefCell<BTreeMap<isize, Popup>> = const { RefCell::new(BTreeMap::new()) };
}
pub struct Tooltip {
    hwnd: HWND,
    _thread: PhantomData<Rc<()>>,
}
#[derive(Clone, Copy)]
pub struct Target(usize);
impl Target {
    pub fn event(self, event: &TrayIconEvent) {
        // Hide synchronously before tray-icon enters TrackPopupMenu's nested
        // message loop on right click. Messages carry coordinates, never pointers.
        unsafe {
            match event {
                TrayIconEvent::Enter { rect, .. } => {
                    let r = RECT {
                        left: rect.position.x as i32,
                        top: rect.position.y as i32,
                        right: rect.position.x as i32 + rect.size.width as i32,
                        bottom: rect.position.y as i32 + rect.size.height as i32,
                    };
                    SendMessageW(
                        self.0 as HWND,
                        ENTER,
                        pack(r.left, r.top) as WPARAM,
                        pack(r.right, r.bottom) as LPARAM,
                    );
                }
                TrayIconEvent::Leave { .. }
                | TrayIconEvent::Click { .. }
                | TrayIconEvent::DoubleClick { .. } => {
                    SendMessageW(self.0 as HWND, HIDE, 0, 0);
                }
                _ => (),
            }
        }
    }
}
impl Tooltip {
    pub fn new(content: Content) -> Result<Self, String> {
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let class = wide(CLASS);
            let wc = WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                class.as_ptr(),
                wide(&content.accessible_text()).as_ptr(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                return Err(std::io::Error::last_os_error().to_string());
            }
            POPUPS.with_borrow_mut(|p| {
                p.insert(
                    hwnd as isize,
                    Popup {
                        content,
                        anchor: RECT::default(),
                        dpi: 96,
                        count: 0,
                        hovering: false,
                    },
                );
            });
            Ok(Self {
                hwnd,
                _thread: PhantomData,
            })
        }
    }
    pub fn target(&self) -> Target {
        Target(self.hwnd as usize)
    }
    pub fn update(&mut self, content: Content) {
        unsafe {
            SetWindowTextW(self.hwnd, wide(&content.accessible_text()).as_ptr());
        }
        POPUPS.with_borrow_mut(|p| {
            if let Some(p) = p.get_mut(&(self.hwnd as isize)) {
                p.content = content;
            }
        });
        if unsafe { IsWindowVisible(self.hwnd) } != 0 {
            show(self.hwnd);
        }
    }
}
impl Drop for Tooltip {
    fn drop(&mut self) {
        hide(self.hwnd);
        POPUPS.with_borrow_mut(|p| {
            p.remove(&(self.hwnd as isize));
        });
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}
fn hide(hwnd: HWND) {
    POPUPS.with_borrow_mut(|all| {
        if let Some(p) = all.get_mut(&(hwnd as isize)) {
            p.hovering = false;
        }
    });
    unsafe {
        KillTimer(hwnd, HOVER_TIMER);
        ShowWindow(hwnd, SW_HIDE);
    }
}
fn snapshot(hwnd: HWND) -> Option<Popup> {
    POPUPS.with_borrow(|p| p.get(&(hwnd as isize)).cloned())
}
fn px(dip: i32, dpi: u32) -> i32 {
    ((dip as i64 * dpi as i64 + 48) / 96) as i32
}
fn pack(x: i32, y: i32) -> u64 {
    x as u32 as u64 | ((y as u32 as u64) << 32)
}
fn unpack(value: u64) -> (i32, i32) {
    (value as u32 as i32, (value >> 32) as u32 as i32)
}
struct Font(HFONT);
impl Font {
    fn new(size: i32, weight: u32, dpi: u32) -> Self {
        Self(unsafe {
            CreateFontW(
                -px(size, dpi),
                0,
                0,
                0,
                weight as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                CLEARTYPE_QUALITY as u32,
                DEFAULT_PITCH as u32,
                wide("Segoe UI").as_ptr(),
            )
        })
    }
}
impl Drop for Font {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.0);
        }
    }
}

fn show(hwnd: HWND) {
    let Some(mut p) = snapshot(hwnd) else {
        return;
    };
    unsafe {
        let mon = MonitorFromRect(&p.anchor, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(mon, &mut info) == 0 {
            hide(hwnd);
            return;
        }
        let (mut xdpi, mut ydpi) = (96, 96);
        if GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut xdpi, &mut ydpi) == 0 {
            p.dpi = xdpi;
        }
        let work = info.rcWork;
        let width = px(360, p.dpi).min(work.right - work.left);
        let count = visible_rows(&p, work.bottom - work.top);
        p.count = count;
        let summary = p.content.rows.len() > count;
        let height = px(
            24 + 44 * count.max(1) as i32 + if summary { 24 } else { 0 },
            p.dpi,
        )
        .min(work.bottom - work.top);
        let (x, y) = position(p.anchor, work, width, height, px(10, p.dpi));
        POPUPS.with_borrow_mut(|all| {
            all.insert(hwnd as isize, p);
        });
        let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, px(14, xdpi), px(14, xdpi));
        if !region.is_null() && SetWindowRgn(hwnd, region, 0) == 0 {
            DeleteObject(region);
        }
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        InvalidateRect(hwnd, std::ptr::null(), 0);
    }
}
fn visible_rows(p: &Popup, available: i32) -> usize {
    let max = ((available - px(48, p.dpi)) / px(44, p.dpi)).clamp(1, 10) as usize;
    p.content.rows.len().min(max)
}
fn position(anchor: RECT, work: RECT, width: i32, height: i32, gap: i32) -> (i32, i32) {
    let left = i64::from(work.left);
    let top = i64::from(work.top);
    let right = i64::from(work.right);
    let bottom = i64::from(work.bottom);
    let x = ((i64::from(anchor.left) + i64::from(anchor.right) - i64::from(width)) / 2)
        .clamp(left, (right - i64::from(width)).max(left));
    let above = i64::from(anchor.top) - i64::from(gap) - i64::from(height);
    let y = if above >= top {
        above
    } else {
        i64::from(anchor.bottom) + i64::from(gap)
    };
    (
        x as i32,
        y.clamp(top, (bottom - i64::from(height)).max(top)) as i32,
    )
}
fn draw_text(dc: HDC, text: &str, rect: &mut RECT, color: u32, flags: u32) {
    let text = wide(text);
    unsafe {
        SetTextColor(dc, rgb(color));
        DrawTextW(
            dc,
            text.as_ptr(),
            (text.len() - 1) as i32,
            rect,
            flags | DT_NOPREFIX,
        );
    }
}
fn paint(hwnd: HWND, dc: HDC) {
    let Some(p) = snapshot(hwnd) else {
        return;
    };
    unsafe {
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client);
        let dark = p.content.dark;
        let background = if dark { 0x272c33 } else { 0xf9fafb };
        let ink = if dark { 0xf1f5f9 } else { 0x182331 };
        let line = if dark { 0x414952 } else { 0xdce2e8 };
        let brush = GetStockObject(DC_BRUSH);
        let pen = GetStockObject(DC_PEN);
        let old_brush = SelectObject(dc, brush);
        let old_pen = SelectObject(dc, pen);
        SetDCBrushColor(dc, rgb(background));
        SetDCPenColor(dc, rgb(line));
        RoundRect(
            dc,
            0,
            0,
            client.right,
            client.bottom,
            px(14, p.dpi),
            px(14, p.dpi),
        );
        SelectObject(dc, old_pen);
        SelectObject(dc, old_brush);
        SetBkMode(dc, TRANSPARENT as i32);
        let name_font = Font::new(13, FW_SEMIBOLD, p.dpi);
        let value_font = Font::new(16, FW_BOLD, p.dpi);
        let old_font = SelectObject(dc, name_font.0);
        let pad = px(14, p.dpi);
        let count = p.count;
        if p.content.rows.is_empty() {
            draw_text(
                dc,
                &p.content.accessible_text(),
                &mut client,
                ink,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
        }
        for (i, row) in p.content.rows.iter().take(count).enumerate() {
            let top = px(12 + 44 * i as i32, p.dpi);
            let mut value = RECT {
                left: client.right - px(100, p.dpi),
                top,
                right: client.right - pad,
                bottom: top + px(40, p.dpi),
            };
            SelectObject(dc, value_font.0);
            let mut measured = RECT::default();
            draw_text(
                dc,
                &row.value,
                &mut measured,
                ink,
                DT_CALCRECT | DT_SINGLELINE,
            );
            value.left = value.right - (measured.right - measured.left);
            draw_text(
                dc,
                &row.value,
                &mut value,
                row.tone.palette(dark).0,
                DT_RIGHT | DT_VCENTER | DT_SINGLELINE,
            );
            SelectObject(dc, name_font.0);
            let mut name = RECT {
                left: pad,
                top: 0,
                right: value.left - px(18, p.dpi),
                bottom: 0,
            };
            draw_text(dc, &row.name, &mut name, ink, DT_CALCRECT | DT_WORDBREAK);
            let height = (name.bottom - name.top).min(px(36, p.dpi));
            name.right = value.left - px(18, p.dpi);
            name.top = top + (px(40, p.dpi) - height) / 2;
            name.bottom = name.top + height;
            draw_text(
                dc,
                &row.name,
                &mut name,
                ink,
                DT_WORDBREAK | DT_END_ELLIPSIS,
            );
        }
        if p.content.rows.len() > count {
            let remaining = p.content.rows.len() - count;
            let text = if p.content.en {
                format!("+{remaining} more devices")
            } else {
                format!("另有 {remaining} 个设备")
            };
            let mut rect = RECT {
                left: pad,
                top: px(12 + 44 * count as i32, p.dpi),
                right: client.right - pad,
                bottom: client.bottom,
            };
            draw_text(
                dc,
                &text,
                &mut rect,
                BatteryTone::Offline.palette(dark).0,
                DT_SINGLELINE,
            );
        }
        SelectObject(dc, old_font);
    }
}
unsafe extern "system" fn procedure(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        ENTER => {
            let (left, top) = unpack(w as u64);
            let (right, bottom) = unpack(l as u64);
            let rect = RECT {
                left,
                top,
                right,
                bottom,
            };
            POPUPS.with_borrow_mut(|p| {
                if let Some(p) = p.get_mut(&(hwnd as isize)) {
                    p.anchor = rect;
                    p.hovering = true;
                }
            });
            unsafe {
                SetTimer(hwnd, HOVER_TIMER, 450, None);
            }
            0
        }
        HIDE => {
            hide(hwnd);
            0
        }
        WM_TIMER if w == HOVER_TIMER => {
            let mut cursor = POINT::default();
            if let Some(p) = snapshot(hwnd)
                && p.hovering
                && unsafe { GetCursorPos(&mut cursor) } != 0
                && (p.anchor.left..p.anchor.right).contains(&cursor.x)
                && (p.anchor.top..p.anchor.bottom).contains(&cursor.y)
            {
                if unsafe { IsWindowVisible(hwnd) } == 0 {
                    show(hwnd);
                }
                unsafe {
                    SetTimer(hwnd, HOVER_TIMER, 100, None);
                }
            } else {
                hide(hwnd);
            }
            0
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = unsafe { BeginPaint(hwnd, &mut ps) };
            paint(hwnd, dc);
            unsafe {
                EndPaint(hwnd, &ps);
            }
            0
        }
        WM_PRINTCLIENT => {
            paint(hwnd, w as HDC);
            0
        }
        WM_ERASEBKGND => 1,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}

#[cfg(test)]
#[allow(dead_code)]
#[path = "../examples/probe/native.rs"]
mod native_probe;

#[cfg(test)]
mod tests {
    use super::*;
    fn capture(hwnd: HWND, path: &std::path::Path) -> image::RgbImage {
        #[link(name = "dwmapi")]
        unsafe extern "system" {
            fn DwmFlush() -> i32;
        }
        // PrintWindow can otherwise return a cached pre-update DWM frame.
        unsafe {
            UpdateWindow(hwnd);
            GdiFlush();
            DwmFlush();
        }
        native_probe::capture(hwnd, path).unwrap()
    }
    #[test]
    fn device_names_and_live_battery_states() {
        let mut state = State::demo();
        state
            .prefs
            .aliases
            .insert(state.rows[0].device.id.clone(), "主力鼠标".into());
        let c = Content::of(&state, true);
        assert_eq!(c.rows[0].name, "主力鼠标");
        assert_eq!(c.rows[0].value, "72%");
        assert_eq!(c.rows[0].tone, BatteryTone::Normal);
        assert_eq!(c.rows[1].tone, BatteryTone::Charging);
        state.rows[0].battery.as_mut().unwrap().display = 49;
        assert_eq!(Content::of(&state, true).rows[0].tone, BatteryTone::Warning);
        state.rows[0].battery.as_mut().unwrap().display = 0;
        let c = Content::of(&state, true);
        assert_eq!(c.rows[0].value, "0%");
        assert_eq!(c.rows[0].tone, BatteryTone::Critical);
        state.rows[0].error = Some("disconnected".into());
        state.prefs.lang = crate::model::Language::En;
        let c = Content::of(&state, true);
        assert_eq!(c.rows[0].value, "Offline");
        assert_eq!(c.rows[0].tone, BatteryTone::Offline);
    }
    #[test]
    fn popup_stays_on_work_area_for_each_taskbar_edge() {
        let work = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        for (x, y) in [(-1920, 500), (-20, 500), (-900, 0), (-900, 1080)] {
            let anchor = RECT {
                left: x,
                top: y,
                right: x + 20,
                bottom: y + 20,
            };
            let (x, y) = position(anchor, work, 360, 120, 10);
            assert!(x >= work.left && x + 360 <= work.right);
            assert!(y >= work.top && y + 120 <= work.bottom);
        }
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop"]
    fn native_render_and_hover_lifecycle() {
        native_probe::dpi_aware();
        let temporary = tempfile::tempdir().unwrap();
        let out = std::env::var_os("TOOLTIP_EVIDENCE_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| temporary.path().into());
        std::fs::create_dir_all(&out).unwrap();
        let mut state = State::demo();
        state.rows[0].device.name = "2.4G Dongle".into();
        state.rows[0].battery.as_mut().unwrap().display = 69;
        state.rows[1].device.name = "Basilisk V3 Pro 35K Wireless".into();
        state.rows[1].battery.as_mut().unwrap().display = 49;
        state.rows[1].battery.as_mut().unwrap().charge = Charge::Discharging;
        let mut third = state.rows[0].clone();
        third.device.id = "low-test".into();
        third.device.name = "备用鼠标".into();
        third.battery.as_mut().unwrap().display = 12;
        state.rows.push(third);
        state.prefs.theme = Theme::Dark;
        let mut tooltip = Tooltip::new(Content::of(&state, true)).unwrap();
        let hwnd = tooltip.hwnd;
        let foreground = unsafe { GetForegroundWindow() };
        let mut cursor = POINT::default();
        assert_ne!(unsafe { GetCursorPos(&mut cursor) }, 0);
        let rect = tray_icon::Rect {
            position: ((cursor.x - 1) as f64, (cursor.y - 1) as f64).into(),
            size: (3, 3).into(),
        };
        tooltip.target().event(&TrayIconEvent::Enter {
            id: "tooltip-test".into(),
            position: (cursor.x as f64, cursor.y as f64).into(),
            rect,
        });
        assert_eq!(
            unsafe { IsWindowVisible(hwnd) },
            0,
            "must wait for hover delay"
        );
        unsafe {
            SendMessageW(hwnd, WM_TIMER, HOVER_TIMER, 0);
        }
        assert_ne!(unsafe { IsWindowVisible(hwnd) }, 0);
        assert_eq!(unsafe { GetForegroundWindow() }, foreground);
        assert_eq!(snapshot(hwnd).unwrap().count, 3);
        let dark = capture(hwnd, &out.join("tooltip-dark.png"));
        state.prefs.theme = Theme::Light;
        tooltip.update(Content::of(&state, false));
        let light = capture(hwnd, &out.join("tooltip-light.png"));
        assert!(dark != light, "theme must change the rendered tooltip");
        state.rows[0].battery.as_mut().unwrap().display = 10;
        tooltip.update(Content::of(&state, false));
        let updated = capture(hwnd, &out.join("tooltip-updated.png"));
        assert!(updated != light, "visible numbers and colors must update");
        state.rows[0].error = Some("disconnected".into());
        state.rows[1].device.name = "A".repeat(180);
        tooltip.update(Content::of(&state, false));
        capture(hwnd, &out.join("tooltip-offline-long-name.png"));
        assert_eq!(unsafe { GetForegroundWindow() }, foreground);
        tooltip.target().event(&TrayIconEvent::Click {
            id: "tooltip-test".into(),
            position: (cursor.x as f64, cursor.y as f64).into(),
            rect,
            button: tray_icon::MouseButton::Right,
            button_state: tray_icon::MouseButtonState::Up,
        });
        unsafe {
            // KillTimer does not remove already queued WM_TIMER messages.
            SendMessageW(hwnd, WM_TIMER, HOVER_TIMER, 0);
        }
        assert_eq!(
            unsafe { IsWindowVisible(hwnd) },
            0,
            "click/leave must cancel queued hover"
        );
        drop(tooltip);
        assert_eq!(unsafe { IsWindow(hwnd) }, 0);
        assert!(snapshot(hwnd).is_none());
    }
}

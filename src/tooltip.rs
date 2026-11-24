// SPDX-FileCopyrightText: 2026 ZGMFX01A
// SPDX-License-Identifier: LicenseRef-Wireless-Device-Battery-Monitor
// See LICENSE in the repository root for the full terms.

//! A small, nonactivating tray tooltip. It lives on the host message-pump thread
//! and uses GDI only; hovering never starts the GPUI child or touches devices.
use crate::{
    device_display::{DeviceIcon, device_icon_in_mode, icon_accessible_name},
    model::{State, Theme},
    view::{
        apps_use_dark_theme, locale,
        skin::{BatteryTone, Skin},
        strings::{Locale, label},
    },
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
    icon: Option<DeviceIcon>,
    value: String,
    charge: Option<Charge>,
    tone: BatteryTone,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    rows: Vec<Device>,
    theme: Theme,
    locale: Locale,
}
impl Content {
    pub fn of(state: &State) -> Self {
        let locale = locale(&state.prefs);
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
                            .row_name(r)
                            .chars()
                            .take(180)
                            .map(|c| if c.is_control() { ' ' } else { c })
                            .collect(),
                        icon: device_icon_in_mode(&r.device, state.prefs.engine_mode),
                        value: if !online {
                            label(locale, "tray.offline").into()
                        } else {
                            battery
                                .and_then(|b| b.percentage())
                                .map_or_else(|| "—".into(), |p| format!("{p}%"))
                        },
                        charge: battery.map(|b| b.charge),
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
            theme: state.prefs.theme.clone(),
            locale,
        }
    }
    fn accessible_text(&self) -> String {
        if self.rows.is_empty() {
            return label(self.locale, "tray.no_devices").into();
        }
        self.rows
            .iter()
            .map(|r| {
                let status = match r.charge {
                    Some(Charge::Charging) => label(self.locale, "device.charging_short"),
                    Some(Charge::Full) => label(self.locale, "device.full"),
                    _ => "",
                };
                let device_type = r.icon.map_or_else(String::new, |icon| {
                    let name = icon_accessible_name(self.locale, icon);
                    if r.name.contains(name) {
                        String::new()
                    } else {
                        format!(" ({name})")
                    }
                });
                if status.is_empty() {
                    format!("{}{device_type}: {}", r.name, r.value)
                } else {
                    format!("{}{device_type}: {}, {status}", r.name, r.value)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
#[test]
fn charging_without_percentage_is_online_without_a_zero_or_tray_selection() {
    use battery_protocol::{Reading, Source, catalog::atk::Provider};
    let mut state = State::demo();
    state.rows.truncate(1);
    let reading = Reading::charging_without_percentage(Source::Atk(Provider::Hfd));
    state.rows[0].battery = Some(reading);
    state.rows[0].error = None;
    let content = Content::of(&state);
    assert_eq!(content.rows[0].value, "—");
    assert_eq!(content.rows[0].charge, Some(Charge::Charging));
    assert!(state.selected().is_none());
    state.rows[0].battery = Reading::new(0, Charge::Discharging, Source::Atk(Provider::Hfd));
    assert_eq!(Content::of(&state).rows[0].value, "0%");
    assert!(state.selected().is_some());
}

const ENTER: u32 = WM_APP + 1;
const HIDE: u32 = WM_APP + 2;
const HOVER_TIMER: usize = 1;
const CLASS: &str = "MouseBatteryTooltip";
const ROW_HEIGHT: i32 = 31;
const SUMMARY_HEIGHT: i32 = 24;
const INSET_X: i32 = 13;
const INSET_Y: i32 = 6;
const GLYPH_SIZE: i32 = 14;
const LABEL_OFFSET: i32 = INSET_X + GLYPH_SIZE + 10;
const VALUE_GAP: i32 = 10;
const CORNER_DIAMETER: i32 = 20;
const TEXT_SIZE: i32 = 13;
const BATTERY_TEXT_SIZE: i32 = 15;
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
        let layout_changed = POPUPS.with_borrow_mut(|p| {
            if let Some(p) = p.get_mut(&(self.hwnd as isize)) {
                let layout_changed = p.content.locale != content.locale
                    || p.content.rows.len() != content.rows.len()
                    || p.content
                        .rows
                        .iter()
                        .zip(&content.rows)
                        .any(|(before, after)| before.name != after.name);
                p.content = content;
                layout_changed
            } else {
                false
            }
        });
        if unsafe { IsWindowVisible(self.hwnd) } != 0 {
            if layout_changed {
                show(self.hwnd);
            } else {
                unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
            }
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
    fn new(family: &str, size: i32, weight: u32, dpi: u32) -> Self {
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
                wide(family).as_ptr(),
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

struct TooltipFonts {
    name: Font,
    status: Font,
    battery: Font,
}
impl TooltipFonts {
    fn new(dpi: u32) -> Self {
        Self {
            name: Font::new("Segoe UI", TEXT_SIZE, FW_NORMAL, dpi),
            status: Font::new("Segoe UI", TEXT_SIZE, FW_SEMIBOLD, dpi),
            battery: Font::new("Consolas", BATTERY_TEXT_SIZE, FW_BOLD, dpi),
        }
    }
    fn value_width(&self, dc: HDC, locale: Locale) -> i32 {
        unsafe {
            let old_font = SelectObject(dc, self.status.0);
            let status = text_width(dc, label(locale, "tray.offline")).max(text_width(dc, "—"));
            SelectObject(dc, self.battery.0);
            let battery = text_width(dc, "100%").max(text_width(dc, "88%"));
            SelectObject(dc, old_font);
            status.max(battery)
        }
    }
}

fn text_width(dc: HDC, text: &str) -> i32 {
    let text = wide(text);
    let mut extent = SIZE::default();
    unsafe {
        GetTextExtentPoint32W(dc, text.as_ptr(), (text.len() - 1) as i32, &mut extent);
    }
    extent.cx
}

fn preferred_width(hwnd: HWND, p: &Popup) -> i32 {
    unsafe {
        let dc = GetDC(hwnd);
        if dc.is_null() {
            return px(234, p.dpi);
        }
        let fonts = TooltipFonts::new(p.dpi);
        let old_font = SelectObject(dc, fonts.name.0);
        let name = p
            .content
            .rows
            .iter()
            .take(p.count)
            .map(|row| text_width(dc, &row.name))
            .max()
            .unwrap_or_default();
        let value = fonts.value_width(dc, p.content.locale);
        SelectObject(dc, old_font);
        ReleaseDC(hwnd, dc);
        (px(LABEL_OFFSET, p.dpi) + name + px(VALUE_GAP, p.dpi) + value + px(INSET_X, p.dpi))
            .clamp(px(160, p.dpi), px(272, p.dpi))
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
        let available = work.bottom - work.top;
        p.count = visible_rows(&p, available);
        let summary = p.content.rows.len() > p.count;
        let width = preferred_width(hwnd, &p).min(work.right - work.left);
        let height = (2 * px(INSET_Y, p.dpi)
            + px(ROW_HEIGHT, p.dpi) * p.count.max(1) as i32
            + if summary {
                px(SUMMARY_HEIGHT, p.dpi)
            } else {
                0
            })
        .min(available);
        let (x, y) = position(p.anchor, work, width, height, px(10, p.dpi));
        let diameter = px(CORNER_DIAMETER, p.dpi);
        POPUPS.with_borrow_mut(|all| {
            all.insert(hwnd as isize, p);
        });
        let region = CreateRoundRectRgn(0, 0, width, height, diameter, diameter);
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
    let space = available - 2 * px(INSET_Y, p.dpi);
    let row_height = px(ROW_HEIGHT, p.dpi);
    let count = p.content.rows.len();
    let capacity = (space / row_height).clamp(1, 10) as usize;
    if count <= capacity {
        count
    } else {
        ((space - px(SUMMARY_HEIGHT, p.dpi)) / row_height).clamp(1, 10) as usize
    }
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

fn draw_device_icon(dc: HDC, icon: DeviceIcon, left: i32, top: i32, size: i32) {
    let x = |offset| left + (size * offset + GLYPH_SIZE / 2) / GLYPH_SIZE;
    let y = |offset| top + (size * offset + GLYPH_SIZE / 2) / GLYPH_SIZE;
    let stroke = |points: &[(i32, i32)]| {
        let points: Vec<_> = points
            .iter()
            .map(|&(a, b)| POINT { x: x(a), y: y(b) })
            .collect();
        unsafe { Polyline(dc, points.as_ptr(), points.len() as i32) };
    };
    unsafe {
        match icon {
            DeviceIcon::Mouse => {
                RoundRect(dc, x(3), y(0), x(12), y(14), x(6) - left, y(6) - top);
                stroke(&[(7, 3), (7, 6)]);
            }
            DeviceIcon::Keyboard => {
                RoundRect(dc, x(0), y(2), x(14), y(12), x(3) - left, y(3) - top);
                for a in [3, 6, 9] {
                    stroke(&[(a, 5), (a + 1, 5)]);
                }
                stroke(&[(4, 8), (10, 8)]);
            }
            DeviceIcon::Bluetooth => {
                stroke(&[(3, 3), (11, 10), (7, 13), (7, 1), (11, 4), (3, 11)]);
            }
            DeviceIcon::Headset => {
                stroke(&[(1, 8), (1, 4), (3, 1), (7, 0), (11, 1), (13, 4), (13, 8)]);
                RoundRect(dc, x(0), y(7), x(4), y(13), x(2) - left, y(2) - top);
                RoundRect(dc, x(10), y(7), x(14), y(13), x(2) - left, y(2) - top);
            }
            DeviceIcon::Gamepad => {
                stroke(&[
                    (1, 12),
                    (0, 10),
                    (2, 3),
                    (5, 2),
                    (9, 2),
                    (12, 3),
                    (14, 10),
                    (13, 12),
                    (10, 9),
                    (4, 9),
                    (1, 12),
                ]);
                stroke(&[(3, 5), (3, 8)]);
                stroke(&[(2, 6), (5, 6)]);
                stroke(&[(10, 5), (11, 5)]);
                stroke(&[(11, 7), (12, 7)]);
            }
            DeviceIcon::Receiver24g => {
                RoundRect(dc, x(1), y(1), x(13), y(6), x(2) - left, y(2) - top);
                Rectangle(dc, x(3), y(5), x(11), y(13));
                stroke(&[(5, 8), (5, 11)]);
                stroke(&[(9, 8), (9, 11)]);
            }
        }
    }
}
fn paint(hwnd: HWND, dc: HDC) {
    let Some(p) = snapshot(hwnd) else {
        return;
    };
    unsafe {
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client);
        let dark = match p.content.theme {
            Theme::Auto => apps_use_dark_theme(),
            Theme::Light => false,
            Theme::Dark => true,
        };
        let skin = Skin { dark, glass: 0 };
        let ink = skin.text_rgb();
        let edge = if dark { 0x3c4553 } else { 0xd6dfe9 };
        let divider = if dark { 0x333c4b } else { 0xe1e8f0 };
        let brush = GetStockObject(DC_BRUSH);
        let pen = GetStockObject(DC_PEN);
        let old_brush = SelectObject(dc, brush);
        let old_pen = SelectObject(dc, pen);
        SetDCBrushColor(dc, rgb(skin.dialog_rgb()));
        SetDCPenColor(dc, rgb(edge));
        RoundRect(
            dc,
            0,
            0,
            client.right,
            client.bottom,
            px(CORNER_DIAMETER, p.dpi),
            px(CORNER_DIAMETER, p.dpi),
        );
        SelectObject(dc, GetStockObject(NULL_BRUSH));
        SetBkMode(dc, TRANSPARENT as i32);
        let fonts = TooltipFonts::new(p.dpi);
        let old_font = SelectObject(dc, fonts.name.0);
        let reserve = fonts.value_width(dc, p.content.locale);
        let inset = px(INSET_X, p.dpi);
        let label_left = px(LABEL_OFFSET, p.dpi);
        let right = client.right - inset;
        let gap = px(VALUE_GAP, p.dpi);
        let name_room = (right - label_left - gap - reserve).max(0);
        let row_height = px(ROW_HEIGHT, p.dpi);
        let inset_y = px(INSET_Y, p.dpi);
        let glyph = px(GLYPH_SIZE, p.dpi);
        if p.content.rows.is_empty() {
            draw_text(
                dc,
                &p.content.accessible_text(),
                &mut client,
                ink,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
        }
        for (i, row) in p.content.rows.iter().take(p.count).enumerate() {
            let top = inset_y + row_height * i as i32;
            if i != 0 {
                SetDCPenColor(dc, rgb(divider));
                MoveToEx(dc, label_left, top, std::ptr::null_mut());
                LineTo(dc, right, top);
            }
            SetDCPenColor(dc, rgb(skin.dim_rgb()));
            if let Some(icon) = row.icon {
                draw_device_icon(dc, icon, inset, top + (row_height - glyph) / 2, glyph);
            }

            let name_right = label_left + text_width(dc, &row.name).min(name_room);
            let mut name = RECT {
                left: label_left,
                top,
                right: name_right,
                bottom: top + row_height,
            };
            draw_text(
                dc,
                &row.name,
                &mut name,
                ink,
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
            let mut value = RECT {
                left: name_right + gap,
                top,
                right,
                bottom: top + row_height,
            };
            let value_font = if row.charge.is_some() {
                &fonts.battery
            } else {
                &fonts.status
            };
            SelectObject(dc, value_font.0);
            draw_text(
                dc,
                &row.value,
                &mut value,
                row.tone.palette(dark).0,
                DT_VCENTER | DT_SINGLELINE,
            );
            SelectObject(dc, fonts.name.0);
        }
        if p.content.rows.len() > p.count {
            let remaining = p.content.rows.len() - p.count;
            let text =
                label(p.content.locale, "tray.more").replace("{count}", &remaining.to_string());
            let top = inset_y + row_height * p.count as i32;
            SetDCPenColor(dc, rgb(divider));
            MoveToEx(dc, label_left, top, std::ptr::null_mut());
            LineTo(dc, right, top);
            let mut rect = RECT {
                left: label_left,
                top,
                right,
                bottom: top + px(SUMMARY_HEIGHT, p.dpi),
            };
            draw_text(
                dc,
                &text,
                &mut rect,
                BatteryTone::Offline.palette(dark).0,
                DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
            );
        }
        SelectObject(dc, old_font);
        SelectObject(dc, old_pen);
        SelectObject(dc, old_brush);
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
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            if unsafe { IsWindowVisible(hwnd) } != 0 {
                unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            }
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
    use battery_protocol::Kind;
    use battery_protocol::{BatteryCapability, ConnectionKind, DeviceCategory, DeviceIdentity};

    fn identity(category: DeviceCategory, connection: ConnectionKind) -> DeviceIdentity {
        DeviceIdentity {
            model_key: None,
            category,
            connection,
            battery: BatteryCapability::Present,
            model_source: None,
        }
    }

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
    fn capture_change(
        hwnd: HWND,
        path: &std::path::Path,
        before: &image::RgbImage,
    ) -> image::RgbImage {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            let current = capture(hwnd, path);
            if &current != before || std::time::Instant::now() >= deadline {
                return current;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }
    #[test]
    fn russian_tooltip_replaces_english_even_when_percentages_do_not_change() {
        let mut state = State::demo();
        state.prefs.lang = crate::model::Language::En;
        let en = Content::of(&state);
        state.prefs.lang = crate::model::Language::Ru;
        let ru = Content::of(&state);
        assert_ne!(en, ru);
        assert!(ru.accessible_text().contains("Заряжается"));
        state.rows[0].error = Some("offline".into());
        assert!(Content::of(&state).accessible_text().contains("Не в сети"));
        state.rows.clear();
        assert_eq!(Content::of(&state).accessible_text(), "Нет устройств");
    }
    #[test]
    fn device_names_and_live_battery_states() {
        let mut state = State::demo();
        state
            .prefs
            .aliases
            .insert(state.rows[0].device.id.clone(), "主力鼠标".into());
        let c = Content::of(&state);
        assert_eq!(c.rows[0].name, "主力鼠标");
        assert_eq!(c.rows[0].icon, Some(DeviceIcon::Mouse));
        assert_eq!(c.rows[0].value, "72%");
        assert_eq!(c.rows[0].tone, BatteryTone::Normal);
        assert_eq!(c.rows[1].icon, Some(DeviceIcon::Keyboard));
        assert_eq!(c.rows[1].value, "35%");
        assert_eq!(c.rows[1].tone, BatteryTone::Charging);
        state.rows[0].battery.as_mut().unwrap().display = 49;
        assert_eq!(Content::of(&state).rows[0].tone, BatteryTone::Warning);
        state.rows[0].battery.as_mut().unwrap().display = 0;
        let c = Content::of(&state);
        assert_eq!(c.rows[0].value, "0%");
        assert_eq!(c.rows[0].tone, BatteryTone::Critical);
        state.rows[0].error = Some("disconnected".into());
        state.prefs.lang = crate::model::Language::En;
        let c = Content::of(&state);
        assert_eq!(c.rows[0].value, "Offline");
        assert_eq!(c.rows[0].tone, BatteryTone::Offline);
        assert!(c.accessible_text().contains("35%, Charging"));
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
    fn all_device_glyphs_remain_distinct_at_96_144_and_192_dpi() {
        let icons = [
            DeviceIcon::Mouse,
            DeviceIcon::Keyboard,
            DeviceIcon::Headset,
            DeviceIcon::Bluetooth,
            DeviceIcon::Receiver24g,
        ];
        for (dpi, expected_size) in [(96, 14), (144, 21), (192, 28)] {
            let size = px(GLYPH_SIZE, dpi);
            assert_eq!(size, expected_size);
            let margin = px(2, dpi);
            let gap = px(4, dpi);
            let step = size + gap;
            let width = margin * 2 + size * icons.len() as i32 + gap * (icons.len() as i32 - 1);
            let height = size + margin * 2;
            let masks = unsafe {
                let screen = GetDC(std::ptr::null_mut());
                assert!(!screen.is_null());
                let memory = CreateCompatibleDC(screen);
                assert!(!memory.is_null());
                let bitmap = CreateCompatibleBitmap(screen, width, height);
                assert!(!bitmap.is_null());
                let old_bitmap = SelectObject(memory, bitmap);
                let background = CreateSolidBrush(rgb(0xffffff));
                assert!(!background.is_null());
                let canvas = RECT {
                    left: 0,
                    top: 0,
                    right: width,
                    bottom: height,
                };
                assert_ne!(FillRect(memory, &canvas, background), 0);
                DeleteObject(background);
                let old_pen = SelectObject(memory, GetStockObject(DC_PEN));
                let old_brush = SelectObject(memory, GetStockObject(NULL_BRUSH));
                SetDCPenColor(memory, rgb(0x303030));
                for (i, icon) in icons.iter().copied().enumerate() {
                    draw_device_icon(memory, icon, margin + i as i32 * step, margin, size);
                }
                GdiFlush();
                let masks = icons
                    .iter()
                    .enumerate()
                    .map(|(i, _)| {
                        let left = margin + i as i32 * step;
                        let mut points = Vec::new();
                        for y in 0..size {
                            for x in 0..size {
                                if GetPixel(memory, left + x, margin + y) != rgb(0xffffff) {
                                    points.push((x, y));
                                }
                            }
                        }
                        points
                    })
                    .collect::<Vec<_>>();
                SelectObject(memory, old_brush);
                SelectObject(memory, old_pen);
                SelectObject(memory, old_bitmap);
                DeleteObject(bitmap);
                DeleteDC(memory);
                ReleaseDC(std::ptr::null_mut(), screen);
                masks
            };
            assert!(masks.iter().all(|mask| mask.len() >= 4));
            for (i, mask) in masks.iter().enumerate() {
                for other in masks.iter().skip(i + 1) {
                    assert_ne!(mask, other, "icons overlap at {dpi} DPI");
                }
            }
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
        third.device.name = "Galaxy Buds4 Pro".into();
        third.device.kind = Kind::Bluetooth;
        third.device.identity = Some(identity(DeviceCategory::Unknown, ConnectionKind::Bluetooth));
        state.prefs.bluetooth.insert(third.device.id.clone());
        third.battery.as_mut().unwrap().display = 12;
        state.rows.push(third);
        let mut headset = state.rows[0].clone();
        headset.device.id = "headset-test".into();
        headset.device.name = "Studio Headset".into();
        headset.device.kind = Kind::Audio;
        headset.device.identity =
            Some(identity(DeviceCategory::Headset, ConnectionKind::Bluetooth));
        state.rows.push(headset);
        let mut receiver = state.rows[0].clone();
        receiver.device.id = "receiver-test".into();
        receiver.device.name = "Shared receiver".into();
        receiver.device.identity = Some(identity(DeviceCategory::Unknown, ConnectionKind::Rf24));
        state.rows.push(receiver);
        state.prefs.theme = Theme::Auto;
        let mut tooltip = Tooltip::new(Content::of(&state)).unwrap();
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
        assert_eq!(snapshot(hwnd).unwrap().count, 5);
        let auto = capture(hwnd, &out.join("tooltip-five-icons-auto.png"));
        let app_dark = apps_use_dark_theme();
        let [r, g, b] = auto.get_pixel(auto.width() / 2, 4).0;
        let background = u32::from_be_bytes([0, r, g, b]);
        let expected = Skin {
            dark: app_dark,
            glass: 0,
        }
        .dialog_rgb();
        assert_eq!(background, expected, "auto theme must match Windows apps");

        let mut icon_before = RECT::default();
        assert_ne!(unsafe { GetWindowRect(hwnd, &mut icon_before) }, 0);
        state.rows[0].device.identity =
            Some(identity(DeviceCategory::Unknown, ConnectionKind::Rf24));
        tooltip.update(Content::of(&state));
        let receiver_icon = capture_change(hwnd, &out.join("tooltip-icon-only.png"), &auto);
        assert_ne!(
            receiver_icon, auto,
            "icon-only changes must redraw immediately"
        );
        let mut icon_after = RECT::default();
        assert_ne!(unsafe { GetWindowRect(hwnd, &mut icon_after) }, 0);
        assert_eq!(
            (
                icon_before.left,
                icon_before.top,
                icon_before.right,
                icon_before.bottom
            ),
            (
                icon_after.left,
                icon_after.top,
                icon_after.right,
                icon_after.bottom
            ),
            "icon-only changes must not recompute tooltip layout"
        );

        state.prefs.theme = Theme::Dark;
        tooltip.update(Content::of(&state));
        let dark_path = out.join("tooltip-dark.png");
        let dark = if app_dark {
            capture(hwnd, &dark_path)
        } else {
            capture_change(hwnd, &dark_path, &auto)
        };
        state.prefs.theme = Theme::Light;
        tooltip.update(Content::of(&state));
        let light = capture_change(hwnd, &out.join("tooltip-light.png"), &dark);
        assert!(dark != light, "theme must change the rendered tooltip");
        let mut initial_bounds = RECT::default();
        assert_ne!(unsafe { GetWindowRect(hwnd, &mut initial_bounds) }, 0);
        let battery = state.rows[0].battery.as_mut().unwrap();
        battery.display = 100;
        battery.charge = Charge::Full;
        tooltip.update(Content::of(&state));
        let updated = capture_change(hwnd, &out.join("tooltip-updated.png"), &light);
        assert!(updated != light, "visible numbers and colors must update");
        let mut updated_bounds = RECT::default();
        assert_ne!(unsafe { GetWindowRect(hwnd, &mut updated_bounds) }, 0);
        let edges = |r: RECT| (r.left, r.top, r.right, r.bottom);
        assert_eq!(
            edges(initial_bounds),
            edges(updated_bounds),
            "battery updates must not resize or move the popup"
        );
        state.rows[0].error = Some("disconnected".into());
        state.rows[1].device.name = "A".repeat(180);
        tooltip.update(Content::of(&state));
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

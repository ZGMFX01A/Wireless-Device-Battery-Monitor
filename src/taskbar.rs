// SPDX-FileCopyrightText: 2026 ZGMFX01A
// SPDX-License-Identifier: LicenseRef-Wireless-Device-Battery-Monitor
// See LICENSE in the repository root for the full terms.

//! 纯 Win32 分层窗口实现的任务栏多设备电量小组件，避免在后台常驻 GPUI/D3D 上下文。
use crate::{
    device_display::DeviceIcon,
    model::{State, TaskbarLayout, TaskbarPosition},
    tooltip::Target as TooltipTarget,
    view::{
        locale,
        strings::{Locale, label},
    },
};
use battery_protocol::{Charge, Kind};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    marker::PhantomData,
    rc::Rc,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::HWND as WinHwnd,
    System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    },
    UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationCondition,
        TreeScope_Descendants, TreeScope_Element, UIA_BoundingRectanglePropertyId,
        UIA_ButtonControlTypeId, UIA_CheckBoxControlTypeId, UIA_ComboBoxControlTypeId,
        UIA_ControlTypePropertyId, UIA_EditControlTypeId, UIA_HyperlinkControlTypeId,
        UIA_ImageControlTypeId, UIA_IsOffscreenPropertyId, UIA_ListItemControlTypeId,
        UIA_MenuItemControlTypeId, UIA_SplitButtonControlTypeId, UIA_TabItemControlTypeId,
        UIA_TextControlTypeId,
    },
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentProcessId},
    UI::{HiDpi::*, WindowsAndMessaging::*},
};

const CLASS: &str = "MouseBatteryTaskbarWidget";
/// 轮询任务栏可见性与全屏状态的定时器 ID。
const SYNC_TIMER: usize = 1;
/// 任务栏可见性与全屏状态的轮询间隔（毫秒）。
const SYNC_INTERVAL_MS: u32 = 1000;
/// 水平换位平滑动画的帧定时器 ID。
const ANIM_TIMER: usize = 2;
/// 水平换位动画帧间隔（约 60fps，毫秒）。
const ANIM_INTERVAL_MS: u32 = 16;
/// 障碍物扫描通知的合并定时器 ID，避免同一轮任务栏变化引起反复定位。
const COALESCE_TIMER: usize = 3;
/// 障碍物扫描更新合并延迟（毫秒）。
const COALESCE_DELAY_MS: u32 = 40;
/// 任务栏安全换位的缓出动画时长。
const MOVE_ANIM_DURATION: Duration = Duration::from_millis(150);
/// 后台扫描线程通知 UI 线程障碍物已更新的自定义窗口消息。
const WM_APP_OBSTACLES: u32 = WM_APP + 11;
/// 后台障碍物扫描周期。
const SCAN_INTERVAL: Duration = Duration::from_millis(1200);

const CMD_OPEN: u32 = 1001;
const CMD_REFRESH: u32 = 1002;
const CMD_POS_TRAY_LEFT: u32 = 1010;
const CMD_POS_TASKBAR_LEFT: u32 = 1011;
const CMD_LAYOUT_STACKED: u32 = 1020;
const CMD_LAYOUT_INLINE: u32 = 1021;
const CMD_BACKDROP: u32 = 1030;
const CMD_CLEAR: u32 = 1040;
const CMD_SETTINGS: u32 = 1041;
const CMD_QUIT: u32 = 1042;

/// 单个固定在任务栏展示的在线设备条目。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskbarItem {
    pub id: String,
    pub name: String,
    pub icon: DeviceIcon,
    /// 用户自定义的 24-bit RGB 图标颜色（`None` 时沿用电池状态默认配色）。
    pub icon_color: Option<u32>,
    pub percent: Option<u8>,
    pub charge: Charge,
    pub low_threshold: u8,
}

/// 任务栏多设备小组件的完整视图状态投影。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    pub items: Vec<TaskbarItem>,
    pub position: TaskbarPosition,
    pub layout: TaskbarLayout,
    pub backdrop: bool,
    pub dark_taskbar: bool,
    pub locale: Locale,
}

impl Content {
    /// 从当前应用状态提取任务栏展示快照，离线或休眠设备自动隐藏。
    pub fn of(state: &State, dark_taskbar: bool) -> Self {
        let items = state
            .prefs
            .taskbar_devices
            .iter()
            .filter_map(|id| {
                let row = state.rows.iter().find(|r| &r.device.id == id)?;
                if !state.prefs.bound(&row.device) || row.error.is_some() {
                    return None;
                }
                let battery = row.battery?;
                let icon = state
                    .prefs
                    .device_icon(&row.device)
                    .unwrap_or(match row.device.kind {
                        Kind::Mouse => DeviceIcon::Mouse,
                        Kind::Keyboard => DeviceIcon::Keyboard,
                        Kind::Audio => DeviceIcon::Headset,
                        Kind::Gamepad => DeviceIcon::Gamepad,
                        Kind::Bluetooth => DeviceIcon::Bluetooth,
                    });
                Some(TaskbarItem {
                    id: row.device.id.clone(),
                    name: state.row_name(row),
                    icon,
                    icon_color: state.prefs.device_icon_color(&row.device),
                    percent: battery.percentage(),
                    charge: battery.charge,
                    low_threshold: state.prefs.low,
                })
            })
            .collect();
        Self {
            items,
            position: state.prefs.taskbar_position.clone(),
            layout: state.prefs.taskbar_layout.clone(),
            backdrop: state.prefs.taskbar_backdrop,
            dark_taskbar,
            locale: locale(&state.prefs),
        }
    }
}

/// 任务栏小组件触发的用户动作。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    OpenFlyout([i32; 2]),
    OpenSettings,
    Refresh,
    SetPosition(TaskbarPosition),
    SetLayout(TaskbarLayout),
    SetBackdrop(bool),
    ClearPinned,
    Quit,
}

type ActionCallback = Rc<RefCell<Box<dyn FnMut(Action)>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SyncReason {
    Reset,
    ContentChanged { anchor_reset: bool },
    Obstacles,
    Poll,
    DisplayChanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MoveAnimation {
    start_x: i32,
    target_x: i32,
    current_x: i32,
    y: i32,
    started_at: Instant,
    duration: Duration,
}

impl MoveAnimation {
    fn sample(&self, now: Instant) -> (i32, bool) {
        let elapsed = now.saturating_duration_since(self.started_at);
        if self.duration.is_zero() || elapsed >= self.duration {
            (self.target_x, true)
        } else {
            let progress = elapsed.as_secs_f64() / self.duration.as_secs_f64();
            (interpolate_x(self.start_x, self.target_x, progress), false)
        }
    }

    fn retarget(&mut self, next_target_x: i32, y: i32, now: Instant) {
        let (cur_x, _) = self.sample(now);
        self.start_x = cur_x;
        self.current_x = cur_x;
        self.target_x = next_target_x;
        self.y = y;
        self.started_at = now;
    }
}

/// 复用的 32bpp top-down DIB 分层表面，避免每次换图重建 GDI 位图或先调 SetWindowPos 导致闪烁。
struct LayeredSurface {
    mem_dc: HDC,
    dib: HBITMAP,
    old_bmp: HGDIOBJ,
    bits: *mut u32,
    width: i32,
    height: i32,
}

impl LayeredSurface {
    fn new() -> Self {
        Self {
            mem_dc: std::ptr::null_mut(),
            dib: std::ptr::null_mut(),
            old_bmp: std::ptr::null_mut(),
            bits: std::ptr::null_mut(),
            width: 0,
            height: 0,
        }
    }

    fn release_dib(&mut self) {
        unsafe {
            if !self.mem_dc.is_null() && !self.old_bmp.is_null() {
                SelectObject(self.mem_dc, self.old_bmp);
                self.old_bmp = std::ptr::null_mut();
            }
            if !self.dib.is_null() {
                DeleteObject(self.dib);
                self.dib = std::ptr::null_mut();
            }
        }
        self.bits = std::ptr::null_mut();
        self.width = 0;
        self.height = 0;
    }

    fn upload(&mut self, canvas: &Canvas) -> bool {
        unsafe {
            if self.mem_dc.is_null() {
                self.mem_dc = CreateCompatibleDC(std::ptr::null_mut());
                if self.mem_dc.is_null() {
                    return false;
                }
            }
            if self.dib.is_null() || self.width != canvas.width || self.height != canvas.height {
                self.release_dib();
                let bmi = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: canvas.width,
                        // 负高度指定 top-down DIB，与 Canvas 行序一致。
                        biHeight: -canvas.height,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut raw_bits: *mut std::ffi::c_void = std::ptr::null_mut();
                let dib = CreateDIBSection(
                    self.mem_dc,
                    &bmi,
                    DIB_RGB_COLORS,
                    &mut raw_bits,
                    std::ptr::null_mut(),
                    0,
                );
                if dib.is_null() || raw_bits.is_null() {
                    if !dib.is_null() {
                        DeleteObject(dib);
                    }
                    return false;
                }
                self.old_bmp = SelectObject(self.mem_dc, dib);
                self.dib = dib;
                self.bits = raw_bits.cast::<u32>();
                self.width = canvas.width;
                self.height = canvas.height;
            }
            let count = (canvas.width as usize) * (canvas.height as usize);
            std::ptr::copy_nonoverlapping(canvas.pixels.as_ptr(), self.bits, count);
            true
        }
    }

    fn commit(&self, hwnd: HWND, dst: Option<POINT>, size: Option<SIZE>) -> bool {
        if self.mem_dc.is_null() || self.dib.is_null() {
            return false;
        }
        unsafe {
            let screen_dc = GetDC(std::ptr::null_mut());
            if screen_dc.is_null() {
                return false;
            }
            let src_pos = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let ok = UpdateLayeredWindow(
                hwnd,
                screen_dc,
                dst.as_ref().map_or(std::ptr::null(), |p| p as *const POINT),
                size.as_ref().map_or(std::ptr::null(), |s| s as *const SIZE),
                self.mem_dc,
                &src_pos,
                0,
                &blend,
                ULW_ALPHA,
            );
            ReleaseDC(std::ptr::null_mut(), screen_dc);
            ok != 0
        }
    }
}

impl Drop for LayeredSurface {
    fn drop(&mut self) {
        self.release_dib();
        if !self.mem_dc.is_null() {
            unsafe {
                DeleteDC(self.mem_dc);
            }
            self.mem_dc = std::ptr::null_mut();
        }
    }
}

struct WidgetState {
    content: Content,
    dpi: u32,
    tb_hwnd: isize,
    tb_rect: RECT,
    width: i32,
    height: i32,
    current_rect: Option<RECT>,
    target_rect: Option<RECT>,
    surface_committed: bool,
    obstacles_applied: bool,
    last_obstacle_rev: u64,
    animation: Option<MoveAnimation>,
    surface: Option<LayeredSurface>,
    tooltip: TooltipTarget,
    callback: ActionCallback,
    taskbar_created_msg: u32,
    scanner: ObstacleScanner,
}

#[derive(Default)]
struct ScannerShared {
    active: bool,
    stop: bool,
    wake: bool,
    notify_pending: bool,
    tb_hwnd: isize,
    tb_rect: RECT,
    dpi: u32,
    spans: Vec<(i32, i32)>,
    scanned: bool,
    revision: u64,
}

#[derive(Clone, Debug, Default)]
struct ScannerSnapshot {
    spans: Vec<(i32, i32)>,
    revision: u64,
    scanned: bool,
}

struct ObstacleScanner {
    shared: Arc<(Mutex<ScannerShared>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

impl ObstacleScanner {
    fn start(widget_hwnd: HWND, initial_active: bool) -> Self {
        let shared = Arc::new((
            Mutex::new(ScannerShared {
                active: initial_active,
                dpi: 96,
                ..Default::default()
            }),
            Condvar::new(),
        ));
        let worker_shared = shared.clone();
        let target_hwnd = widget_hwnd as isize;
        let worker = thread::Builder::new()
            .name("taskbar-obstacle-scanner".into())
            .spawn(move || scanner_loop(target_hwnd, worker_shared))
            .ok();
        Self { shared, worker }
    }

    fn set_active(&self, active: bool) {
        let (lock, cv) = &*self.shared;
        let mut g = lock.lock().unwrap();
        if g.active != active {
            g.active = active;
            if active {
                g.wake = true;
            }
            cv.notify_all();
        }
    }

    /// 非阻塞读取后台扫描的最近一份障碍物区间；任务栏句柄/矩形/DPI 变化时在后台触发重扫。
    fn latest_spans(&self, tb_hwnd: HWND, tb_rect: RECT, dpi: u32) -> ScannerSnapshot {
        let (lock, cv) = &*self.shared;
        let mut g = lock.lock().unwrap();
        let tb_changed =
            g.tb_hwnd != tb_hwnd as isize || !same_rect(g.tb_rect, tb_rect) || g.dpi != dpi;
        if tb_changed {
            g.tb_hwnd = tb_hwnd as isize;
            g.tb_rect = tb_rect;
            g.dpi = dpi;
            g.spans.clear();
            g.scanned = false;
            g.active = true;
            g.wake = true;
            cv.notify_all();
        } else if !g.scanned {
            g.active = true;
            g.wake = true;
            cv.notify_all();
        }
        ScannerSnapshot {
            spans: g.spans.clone(),
            revision: g.revision,
            scanned: g.scanned,
        }
    }

    fn acknowledge_notify(&self) {
        let (lock, _) = &*self.shared;
        let mut g = lock.lock().unwrap();
        g.notify_pending = false;
    }

    fn wake(&self) {
        let (lock, cv) = &*self.shared;
        let mut g = lock.lock().unwrap();
        g.active = true;
        g.wake = true;
        cv.notify_all();
    }
}

impl Drop for ObstacleScanner {
    fn drop(&mut self) {
        {
            let (lock, cv) = &*self.shared;
            let mut g = lock.lock().unwrap();
            g.stop = true;
            g.wake = true;
            cv.notify_all();
        }
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

struct ComGuard(bool);
impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

fn scanner_loop(widget_hwnd: isize, shared: Arc<(Mutex<ScannerShared>, Condvar)>) {
    let com_ok = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() };
    let _com = ComGuard(com_ok);
    let automation: Option<IUIAutomation> = if com_ok {
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok() }
    } else {
        None
    };
    let uia_cache: Option<(IUIAutomationCondition, IUIAutomationCacheRequest)> =
        automation.as_ref().and_then(|uia| unsafe {
            let cond = uia.CreateTrueCondition().ok()?;
            let req = uia.CreateCacheRequest().ok()?;
            req.AddProperty(UIA_ControlTypePropertyId).ok()?;
            req.AddProperty(UIA_BoundingRectanglePropertyId).ok()?;
            req.AddProperty(UIA_IsOffscreenPropertyId).ok()?;
            req.SetTreeScope(TreeScope_Element).ok()?;
            Some((cond, req))
        });

    loop {
        let (tb_hwnd, tb_rect, dpi) = {
            let (lock, cv) = &*shared;
            let mut g = lock.lock().unwrap();
            loop {
                if g.stop {
                    return;
                }
                if g.active && g.tb_hwnd != 0 {
                    if g.wake {
                        g.wake = false;
                        break (g.tb_hwnd as HWND, g.tb_rect, g.dpi);
                    }
                    let (next, timeout) = cv.wait_timeout(g, SCAN_INTERVAL).unwrap();
                    g = next;
                    if g.stop {
                        return;
                    }
                    if (timeout.timed_out() || g.wake) && g.active && g.tb_hwnd != 0 {
                        g.wake = false;
                        break (g.tb_hwnd as HWND, g.tb_rect, g.dpi);
                    }
                } else {
                    g = cv.wait(g).unwrap();
                }
            }
        };

        let mut spans = Vec::new();
        if let (Some(uia), Some((cond, req))) = (&automation, &uia_cache) {
            scan_uia_taskbar_spans(uia, cond, req, tb_hwnd, tb_rect, dpi, &mut spans);
        }
        scan_external_taskbar_windows(tb_hwnd, tb_rect, widget_hwnd as HWND, dpi, &mut spans);
        spans.sort_unstable();
        spans.dedup();

        let notify_widget = {
            let (lock, _) = &*shared;
            let mut g = lock.lock().unwrap();
            let stale =
                g.tb_hwnd != tb_hwnd as isize || !same_rect(g.tb_rect, tb_rect) || g.dpi != dpi;
            if stale {
                // 扫描期间任务栏几何已变更，丢弃过期结果并在下一轮立即重扫。
                if !g.stop && g.active {
                    g.wake = true;
                }
                false
            } else {
                let updated = !g.scanned || g.spans != spans;
                if updated {
                    g.spans = spans;
                    g.scanned = true;
                    g.revision = g.revision.wrapping_add(1);
                }
                let should_post = updated && g.active && !g.stop && !g.notify_pending;
                if should_post {
                    g.notify_pending = true;
                }
                should_post
            }
        };
        if notify_widget {
            unsafe {
                PostMessageW(widget_hwnd as HWND, WM_APP_OBSTACLES, 0, 0);
            }
        }
    }
}

fn scan_uia_taskbar_spans(
    automation: &IUIAutomation,
    cond: &IUIAutomationCondition,
    cache_req: &IUIAutomationCacheRequest,
    tb_hwnd: HWND,
    tb_rect: RECT,
    dpi: u32,
    out: &mut Vec<(i32, i32)>,
) {
    // 先扫描顶层任务栏，再对关键任务栏/桥接子 HWND 补扫，避免空矩形中间容器在部分任务栏模式下阻断遍历。
    scan_uia_hwnd_spans(automation, cond, cache_req, tb_hwnd, tb_rect, dpi, out);

    unsafe extern "system" fn collect_child_hwnds(
        hwnd: HWND,
        lparam: LPARAM,
    ) -> windows_sys::core::BOOL {
        let list = unsafe { &mut *(lparam as *mut Vec<HWND>) };
        if unsafe { IsWindowVisible(hwnd) } != 0 {
            let cls = window_class_name(hwnd);
            if matches!(
                cls.as_str(),
                "MSTaskListWClass"
                    | "ToolbarWindow32"
                    | "TrayNotifyWnd"
                    | "Windows.UI.Composition.DesktopWindowContentBridge"
            ) {
                list.push(hwnd);
            }
        }
        1
    }

    let mut sub_hwnds = Vec::new();
    unsafe {
        EnumChildWindows(
            tb_hwnd,
            Some(collect_child_hwnds),
            (&mut sub_hwnds as *mut Vec<HWND>) as LPARAM,
        );
    }
    for sub in sub_hwnds {
        scan_uia_hwnd_spans(automation, cond, cache_req, sub, tb_rect, dpi, out);
    }
}

fn scan_uia_hwnd_spans(
    automation: &IUIAutomation,
    cond: &IUIAutomationCondition,
    cache_req: &IUIAutomationCacheRequest,
    target_hwnd: HWND,
    tb_rect: RECT,
    dpi: u32,
    out: &mut Vec<(i32, i32)>,
) {
    let win_hwnd = WinHwnd(target_hwnd.cast());
    let Ok(root) = (unsafe { automation.ElementFromHandle(win_hwnd) }) else {
        return;
    };
    let Ok(arr) = (unsafe { root.FindAllBuildCache(TreeScope_Descendants, cond, cache_req) })
    else {
        return;
    };
    let len = unsafe { arr.Length().unwrap_or(0) };
    let tb_width = (tb_rect.right - tb_rect.left).max(1);
    let max_item_width = (tb_width * 45 / 100).max(px(360, dpi));

    for i in 0..len {
        let Ok(el) = (unsafe { arr.GetElement(i) }) else {
            continue;
        };
        if let Ok(offscreen) = unsafe { el.CachedIsOffscreen() }
            && offscreen.as_bool()
        {
            continue;
        }
        let Ok(rect) = (unsafe { el.CachedBoundingRectangle() }) else {
            continue;
        };
        if rect.right <= rect.left
            || rect.bottom <= rect.top
            || rect.bottom <= tb_rect.top
            || rect.top >= tb_rect.bottom
        {
            continue;
        }
        let w = rect.right - rect.left;
        if w <= 0 || w > max_item_width {
            continue;
        }
        let Ok(ctrl_type) = (unsafe { el.CachedControlType() }) else {
            continue;
        };
        let is_leaf = [
            UIA_ButtonControlTypeId,
            UIA_EditControlTypeId,
            UIA_CheckBoxControlTypeId,
            UIA_ComboBoxControlTypeId,
            UIA_SplitButtonControlTypeId,
            UIA_MenuItemControlTypeId,
            UIA_HyperlinkControlTypeId,
            UIA_TabItemControlTypeId,
            UIA_ListItemControlTypeId,
            UIA_ImageControlTypeId,
            UIA_TextControlTypeId,
        ]
        .contains(&ctrl_type);
        if is_leaf {
            out.push((rect.left, rect.right));
        }
    }
}

fn is_window_cloaked(hwnd: HWND) -> bool {
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmGetWindowAttribute(
            hwnd: HWND,
            dwAttribute: u32,
            pvAttribute: *mut std::ffi::c_void,
            cbAttribute: u32,
        ) -> i32;
    }
    const DWMWA_CLOAKED: u32 = 14;
    let mut cloaked: u32 = 0;
    let hr = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    };
    hr == 0 && cloaked != 0
}

fn scan_external_taskbar_windows(
    tb_hwnd: HWND,
    tb_rect: RECT,
    self_hwnd: HWND,
    dpi: u32,
    out: &mut Vec<(i32, i32)>,
) {
    struct TopEnumCtx<'a> {
        tb_hwnd: HWND,
        self_hwnd: HWND,
        current_pid: u32,
        tb_rect: RECT,
        tb_width: i32,
        max_height: i32,
        out: &'a mut Vec<(i32, i32)>,
    }
    unsafe extern "system" fn enum_top_cb(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
        let ctx = unsafe { &mut *(lparam as *mut TopEnumCtx<'_>) };
        if hwnd == ctx.self_hwnd || hwnd == ctx.tb_hwnd || unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        if unsafe { IsIconic(hwnd) } != 0 || is_window_cloaked(hwnd) {
            return 1;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == 0 || pid == ctx.current_pid {
            return 1;
        }
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
            return 1;
        }
        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;
        if w <= 0 || h <= 0 || w >= ctx.tb_width * 60 / 100 || h > ctx.max_height {
            return 1;
        }
        let overlap_y =
            (rect.bottom.min(ctx.tb_rect.bottom) - rect.top.max(ctx.tb_rect.top)).max(0);
        if overlap_y * 100 >= h * 40 {
            let cls = window_class_name(hwnd);
            if !matches!(
                cls.as_str(),
                "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "Progman" | "WorkerW"
            ) {
                ctx.out.push((rect.left, rect.right));
            }
        }
        1
    }

    struct ChildEnumCtx<'a> {
        self_hwnd: HWND,
        current_pid: u32,
        explorer_pid: u32,
        tb_width: i32,
        out: &'a mut Vec<(i32, i32)>,
    }
    unsafe extern "system" fn enum_child_cb(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
        let ctx = unsafe { &mut *(lparam as *mut ChildEnumCtx<'_>) };
        if hwnd == ctx.self_hwnd || unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == 0 || pid == ctx.current_pid || pid == ctx.explorer_pid {
            return 1;
        }
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
            return 1;
        }
        let w = rect.right - rect.left;
        if w > 0 && w < ctx.tb_width * 60 / 100 && rect.bottom > rect.top {
            ctx.out.push((rect.left, rect.right));
        }
        1
    }

    let current_pid = unsafe { GetCurrentProcessId() };
    let mut explorer_pid = 0u32;
    unsafe { GetWindowThreadProcessId(tb_hwnd, &mut explorer_pid) };
    let tb_width = (tb_rect.right - tb_rect.left).max(1);
    let tb_height = (tb_rect.bottom - tb_rect.top).max(24);

    let mut top_ctx = TopEnumCtx {
        tb_hwnd,
        self_hwnd,
        current_pid,
        tb_rect,
        tb_width,
        max_height: tb_height + px(24, dpi),
        out,
    };
    unsafe {
        EnumWindows(
            Some(enum_top_cb),
            (&mut top_ctx as *mut TopEnumCtx<'_>) as LPARAM,
        );
    }

    let mut child_ctx = ChildEnumCtx {
        self_hwnd,
        current_pid,
        explorer_pid,
        tb_width,
        out: top_ctx.out,
    };
    unsafe {
        EnumChildWindows(
            tb_hwnd,
            Some(enum_child_cb),
            (&mut child_ctx as *mut ChildEnumCtx<'_>) as LPARAM,
        );
    }
}

thread_local! {
    // 与 tooltip.rs 保持一致：窗口过程可能重入，提取或修改状态时不跨 Win32 窗口管理调用持借用。
    static WIDGETS: RefCell<BTreeMap<isize, WidgetState>> = const { RefCell::new(BTreeMap::new()) };
}

/// 宿主进程持有的任务栏多设备电量浮窗句柄。
pub struct TaskbarWidget {
    hwnd: HWND,
    _thread: PhantomData<Rc<()>>,
}

impl TaskbarWidget {
    pub fn new(
        content: Content,
        tooltip: TooltipTarget,
        callback: impl FnMut(Action) + 'static,
    ) -> Result<Self, String> {
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let class = wide(CLASS);
            let wc = WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: instance,
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_HAND),
                lpszClassName: class.as_ptr(),
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                class.as_ptr(),
                wide("MouseBatteryTaskbarWidget").as_ptr(),
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
            let taskbar_created_msg = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
            let should_show = !content.items.is_empty();
            let scanner = ObstacleScanner::start(hwnd, should_show);
            WIDGETS.with_borrow_mut(|all| {
                all.insert(
                    hwnd as isize,
                    WidgetState {
                        content,
                        dpi: 96,
                        tb_hwnd: 0,
                        tb_rect: RECT::default(),
                        width: 0,
                        height: 0,
                        current_rect: None,
                        target_rect: None,
                        surface_committed: false,
                        obstacles_applied: false,
                        last_obstacle_rev: 0,
                        animation: None,
                        surface: Some(LayeredSurface::new()),
                        tooltip,
                        callback: Rc::new(RefCell::new(Box::new(callback))),
                        taskbar_created_msg,
                        scanner,
                    },
                );
            });
            if should_show {
                sync_widget(hwnd, SyncReason::Reset);
                SetTimer(hwnd, SYNC_TIMER, SYNC_INTERVAL_MS, None);
            }
            Ok(Self {
                hwnd,
                _thread: PhantomData,
            })
        }
    }

    pub fn update(&mut self, content: Content) {
        let should_show = !content.items.is_empty();
        let update_plan = WIDGETS.with_borrow_mut(|all| {
            let state = all.get_mut(&(self.hwnd as isize))?;
            if state.content == content {
                return Some(None);
            }
            let anchor_reset = state.content.position != content.position;
            state.content = content;
            state.scanner.set_active(should_show);
            Some(Some(anchor_reset))
        });
        let Some(Some(anchor_reset)) = update_plan else {
            return;
        };
        unsafe {
            if should_show {
                sync_widget(self.hwnd, SyncReason::ContentChanged { anchor_reset });
                SetTimer(self.hwnd, SYNC_TIMER, SYNC_INTERVAL_MS, None);
            } else {
                KillTimer(self.hwnd, SYNC_TIMER);
                KillTimer(self.hwnd, ANIM_TIMER);
                KillTimer(self.hwnd, COALESCE_TIMER);
                hide_widget(self.hwnd);
            }
        }
    }
}

impl Drop for TaskbarWidget {
    fn drop(&mut self) {
        unsafe {
            KillTimer(self.hwnd, SYNC_TIMER);
            KillTimer(self.hwnd, ANIM_TIMER);
            KillTimer(self.hwnd, COALESCE_TIMER);
            ShowWindow(self.hwnd, SW_HIDE);
        }
        let removed = WIDGETS.with_borrow_mut(|all| all.remove(&(self.hwnd as isize)));
        drop(removed);
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn px(dip: i32, dpi: u32) -> i32 {
    ((dip as i64 * dpi as i64 + 48) / 96) as i32
}

fn hide_widget(hwnd: HWND) {
    let tooltip = WIDGETS.with_borrow_mut(|all| {
        let state = all.get_mut(&(hwnd as isize))?;
        state.current_rect = None;
        state.target_rect = None;
        state.surface_committed = false;
        state.obstacles_applied = false;
        state.last_obstacle_rev = 0;
        state.animation = None;
        Some(state.tooltip)
    });
    if let Some(target) = tooltip {
        target.hide();
    }
    unsafe {
        KillTimer(hwnd, ANIM_TIMER);
        KillTimer(hwnd, COALESCE_TIMER);
        if IsWindowVisible(hwnd) != 0 {
            ShowWindow(hwnd, SW_HIDE);
        }
    }
}

#[derive(Clone, Copy)]
struct TaskbarGeometry {
    tb_hwnd: HWND,
    tb_rect: RECT,
    monitor_rect: RECT,
    notify_rect: Option<RECT>,
    start_rect: Option<RECT>,
    dpi: u32,
}

fn find_child_rect(parent: HWND, class_name: &str) -> Option<RECT> {
    unsafe {
        let child = FindWindowExW(
            parent,
            std::ptr::null_mut(),
            wide(class_name).as_ptr(),
            std::ptr::null(),
        );
        if child.is_null() || IsWindowVisible(child) == 0 {
            return None;
        }
        let mut rect = RECT::default();
        if GetWindowRect(child, &mut rect) == 0
            || rect.right <= rect.left
            || rect.bottom <= rect.top
        {
            return None;
        }
        Some(rect)
    }
}

fn query_taskbar_geometry() -> Option<TaskbarGeometry> {
    unsafe {
        let tb = FindWindowW(wide("Shell_TrayWnd").as_ptr(), std::ptr::null());
        if tb.is_null() || IsWindowVisible(tb) == 0 {
            return None;
        }
        let mut tb_rect = RECT::default();
        if GetWindowRect(tb, &mut tb_rect) == 0 {
            return None;
        }
        if tb_rect.right - tb_rect.left <= 4 || tb_rect.bottom - tb_rect.top <= 4 {
            return None;
        }
        let mon = MonitorFromWindow(tb, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(mon, &mut mi) == 0 {
            return None;
        }
        // 任务栏自动隐藏时会滑出屏幕外；仅在与屏幕相交且高度合理时展示。
        let mon_rect = mi.rcMonitor;
        let visible_w =
            (tb_rect.right.min(mon_rect.right) - tb_rect.left.max(mon_rect.left)).max(0);
        let visible_h =
            (tb_rect.bottom.min(mon_rect.bottom) - tb_rect.top.max(mon_rect.top)).max(0);
        if visible_w <= 8 || visible_h <= 8 {
            return None;
        }
        let (mut xdpi, mut ydpi) = (96u32, 96u32);
        let dpi = if GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut xdpi, &mut ydpi) == 0 {
            xdpi.max(96)
        } else {
            96
        };
        let notify_rect = find_child_rect(tb, "TrayNotifyWnd");
        let start_rect = find_child_rect(tb, "Start")
            .or_else(|| find_child_rect(tb, "Windows.UI.Core.CoreWindow"));
        Some(TaskbarGeometry {
            tb_hwnd: tb,
            tb_rect,
            monitor_rect: mon_rect,
            notify_rect,
            start_rect,
            dpi,
        })
    }
}

fn window_class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let len = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if len <= 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&buf[..len as usize])
    }
}

fn foreground_is_fullscreen(self_hwnd: HWND, monitor_rect: RECT) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() || fg == self_hwnd || IsWindowVisible(fg) == 0 {
            return false;
        }
        let cls = window_class_name(fg);
        if matches!(
            cls.as_str(),
            "Shell_TrayWnd"
                | "Shell_SecondaryTrayWnd"
                | "Progman"
                | "WorkerW"
                | "NotifyIconOverflowWindow"
                | "Windows.UI.Core.CoreWindow"
        ) {
            return false;
        }
        let mut rect = RECT::default();
        if GetWindowRect(fg, &mut rect) == 0 {
            return false;
        }
        rect.left <= monitor_rect.left
            && rect.top <= monitor_rect.top
            && rect.right >= monitor_rect.right
            && rect.bottom >= monitor_rect.bottom
    }
}

/// 将扫描得到的障碍水平区间裁剪、加安全边距并排序合并。
pub(crate) fn merge_occupied_spans(
    tb_left: i32,
    tb_right: i32,
    pad: i32,
    raw_spans: &[(i32, i32)],
) -> Vec<(i32, i32)> {
    let mut list: Vec<(i32, i32)> = raw_spans
        .iter()
        .filter_map(|&(l, r)| {
            if r <= l {
                return None;
            }
            let left = (l - pad).max(tb_left);
            let right = (r + pad).min(tb_right);
            (right > left).then_some((left, right))
        })
        .collect();
    list.sort_unstable();
    let mut merged: Vec<(i32, i32)> = Vec::with_capacity(list.len());
    for (l, r) in list {
        if let Some(last) = merged.last_mut()
            && l <= last.1
        {
            last.1 = last.1.max(r);
        } else {
            merged.push((l, r));
        }
    }
    merged
}

/// 在 `[min_left, max_right]` 内求解所有不被 `occupied` 占用的空白水平区间。
pub(crate) fn free_gaps(min_left: i32, max_right: i32, occupied: &[(i32, i32)]) -> Vec<(i32, i32)> {
    if max_right <= min_left {
        return Vec::new();
    }
    let mut cursor = min_left;
    let mut gaps = Vec::new();
    for &(l, r) in occupied {
        if r <= cursor {
            continue;
        }
        if l > cursor {
            let end = l.min(max_right);
            if end > cursor {
                gaps.push((cursor, end));
            }
        }
        cursor = cursor.max(r);
        if cursor >= max_right {
            break;
        }
    }
    if cursor < max_right {
        gaps.push((cursor, max_right));
    }
    gaps
}

/// 结合任务栏真实控件与第三方窗口占用区间，计算完全不重叠的最佳停靠矩形。
#[allow(clippy::too_many_arguments)]
pub(crate) fn compute_widget_rect_avoiding(
    tb_rect: RECT,
    occupied: &[(i32, i32)],
    fallback_notify: Option<RECT>,
    fallback_start: Option<RECT>,
    position: &TaskbarPosition,
    width: i32,
    height: i32,
    dpi: u32,
) -> RECT {
    if occupied.is_empty() {
        return compute_widget_rect(
            tb_rect,
            fallback_notify,
            fallback_start,
            position,
            width,
            height,
            dpi,
        );
    }
    let margin = px(4, dpi);
    let pad = px(4, dpi);
    let min_left = tb_rect.left + margin;
    let max_right = (tb_rect.right - margin).max(min_left);
    let merged = merge_occupied_spans(min_left, max_right, pad, occupied);
    let gaps = free_gaps(min_left, max_right, &merged);

    let chosen_left = if let Some((gl, gr)) = match position {
        TaskbarPosition::TrayLeft => gaps.iter().rev().copied().find(|&(l, r)| r - l >= width),
        TaskbarPosition::TaskbarLeft => gaps.iter().copied().find(|&(l, r)| r - l >= width),
    } {
        match position {
            TaskbarPosition::TrayLeft => gr - width,
            TaskbarPosition::TaskbarLeft => gl,
        }
    } else if let Some((gl, gr)) = gaps.iter().copied().max_by_key(|&(l, r)| r - l) {
        (gl + (gr - gl - width) / 2).clamp(min_left, (max_right - width).max(min_left))
    } else {
        return compute_widget_rect(
            tb_rect,
            fallback_notify,
            fallback_start,
            position,
            width,
            height,
            dpi,
        );
    };

    let top = tb_rect.top + ((tb_rect.bottom - tb_rect.top - height) / 2).max(0);
    RECT {
        left: chosen_left,
        top,
        right: chosen_left + width,
        bottom: top + height,
    }
}

/// 计算任务栏多设备小组件在桌面屏幕坐标中的目标矩形（无 UIA 占用数据时的回退定位）。
pub(crate) fn compute_widget_rect(
    tb_rect: RECT,
    notify_rect: Option<RECT>,
    start_rect: Option<RECT>,
    position: &TaskbarPosition,
    width: i32,
    height: i32,
    dpi: u32,
) -> RECT {
    let min_left = tb_rect.left + px(4, dpi);
    let tray_limit = notify_rect
        .map(|n| n.left - px(6, dpi))
        .unwrap_or(tb_rect.right - px(160, dpi));
    let max_left = (tray_limit - width).max(min_left);

    let left = match position {
        TaskbarPosition::TrayLeft => max_left,
        TaskbarPosition::TaskbarLeft => {
            let preferred = if let Some(start) = start_rect {
                // Win11 居中任务栏左侧常留有大片空白，可直接贴任务栏最左侧；左对齐任务栏则避开开始按钮。
                if start.left - tb_rect.left >= width + px(16, dpi) {
                    tb_rect.left + px(12, dpi)
                } else {
                    start.right + px(8, dpi)
                }
            } else {
                tb_rect.left + px(12, dpi)
            };
            preferred.clamp(min_left, max_left)
        }
    };
    let top = tb_rect.top + ((tb_rect.bottom - tb_rect.top - height) / 2).max(0);
    RECT {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

fn same_rect(a: RECT, b: RECT) -> bool {
    a.left == b.left && a.top == b.top && a.right == b.right && a.bottom == b.bottom
}

/// 计算两段水平区间的重叠宽度（像素）。
pub(crate) fn span_overlap(a_left: i32, a_right: i32, b_left: i32, b_right: i32) -> i32 {
    (a_right.min(b_right) - a_left.max(b_left)).max(0)
}

/// 三次缓出曲线（输出归一化至 `[0.0, 1.0]`）。
pub(crate) fn ease_out_cubic(progress: f64) -> f64 {
    let p = progress.clamp(0.0, 1.0);
    1.0 - (1.0 - p).powi(3)
}

/// 按缓出曲线计算水平动画当前 X 坐标。
pub(crate) fn interpolate_x(start_x: i32, target_x: i32, progress: f64) -> i32 {
    let eased = ease_out_cubic(progress);
    (start_x as f64 + (target_x - start_x) as f64 * eased).round() as i32
}

/// 判断水平移动路径 `[from_x, to_x]` 是否会扫过并遮挡任何系统按钮或任务栏障碍物。
///
/// 若某障碍物在路径中间产生的重叠度大于起点和终点的重叠度，说明组件会穿越该元素，应禁止动画直接安全落位。
pub(crate) fn is_move_path_clear(
    from_x: i32,
    to_x: i32,
    width: i32,
    obstacles: &[(i32, i32)],
) -> bool {
    let min_x = from_x.min(to_x);
    let max_x = from_x.max(to_x);
    if min_x == max_x || width <= 0 {
        return true;
    }
    for &(ol, or) in obstacles {
        if or <= ol {
            continue;
        }
        let start_ov = span_overlap(from_x, from_x + width, ol, or);
        let end_ov = span_overlap(to_x, to_x + width, ol, or);
        let cand1 = ol.clamp(min_x, max_x);
        let cand2 = (or - width).clamp(min_x, max_x);
        let max_ov = span_overlap(cand1, cand1 + width, ol, or)
            .max(span_overlap(cand2, cand2 + width, ol, or));
        if max_ov > start_ov.max(end_ov) {
            return false;
        }
    }
    true
}

/// 结合既有锚点滞回与避让布局求解目标矩形；若旧位置依然安全且不碰撞任何障碍，则保留旧位置避免反复跳动。
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_widget_rect(
    tb_rect: RECT,
    occupied: &[(i32, i32)],
    fallback_notify: Option<RECT>,
    fallback_start: Option<RECT>,
    position: &TaskbarPosition,
    width: i32,
    height: i32,
    dpi: u32,
    keep_anchor: Option<RECT>,
) -> RECT {
    if let Some(prev) = keep_anchor
        && prev.right - prev.left == width
        && prev.bottom - prev.top == height
    {
        let expected_top = tb_rect.top + ((tb_rect.bottom - tb_rect.top - height) / 2).max(0);
        let min_left = tb_rect.left + px(4, dpi);
        let max_right = (tb_rect.right - px(4, dpi)).max(min_left);
        if prev.top == expected_top && prev.left >= min_left && prev.right <= max_right {
            let keep_overlaps = if !occupied.is_empty() {
                let merged = merge_occupied_spans(min_left, max_right, px(2, dpi), occupied);
                merged.iter().any(|&(l, r)| prev.left < r && prev.right > l)
            } else {
                let notify_overlaps = fallback_notify
                    .is_some_and(|n| prev.right > n.left - px(2, dpi) && prev.left < n.right);
                let start_overlaps = fallback_start
                    .is_some_and(|s| prev.left < s.right + px(2, dpi) && prev.right > s.left);
                notify_overlaps || start_overlaps
            };
            if !keep_overlaps {
                return prev;
            }
        }
    }
    compute_widget_rect_avoiding(
        tb_rect,
        occupied,
        fallback_notify,
        fallback_start,
        position,
        width,
        height,
        dpi,
    )
}

fn take_surface(hwnd: HWND) -> LayeredSurface {
    WIDGETS
        .with_borrow_mut(|all| {
            all.get_mut(&(hwnd as isize))
                .and_then(|s| s.surface.take())
        })
        .unwrap_or_else(LayeredSurface::new)
}

fn put_surface(hwnd: HWND, surface: LayeredSurface) {
    WIDGETS.with_borrow_mut(|all| {
        if let Some(s) = all.get_mut(&(hwnd as isize)) {
            s.surface = Some(surface);
        }
    });
}

fn commit_canvas(hwnd: HWND, canvas: &Canvas, dst: Option<POINT>, size: Option<SIZE>) -> bool {
    let mut surface = take_surface(hwnd);
    let ok = surface.upload(canvas) && surface.commit(hwnd, dst, size);
    put_surface(hwnd, surface);
    ok
}

fn move_layered_window(hwnd: HWND, x: i32, y: i32) -> bool {
    let dst = POINT { x, y };
    unsafe {
        UpdateLayeredWindow(
            hwnd,
            std::ptr::null_mut(),
            &dst,
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            0,
        ) != 0
    }
}

fn show_layered_window(hwnd: HWND) {
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

fn ensure_above_taskbar(hwnd: HWND, tb_hwnd: HWND) {
    if tb_hwnd.is_null() {
        return;
    }
    unsafe {
        let mut cur = GetWindow(hwnd, GW_HWNDPREV);
        let mut steps = 0;
        while !cur.is_null() && steps < 256 {
            if cur == tb_hwnd {
                SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
                break;
            }
            cur = GetWindow(cur, GW_HWNDPREV);
            steps += 1;
        }
    }
}

fn system_animations_enabled() -> bool {
    let mut enabled: i32 = 1;
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            (&mut enabled as *mut i32).cast(),
            0,
        )
    };
    ok == 0 || enabled != 0
}

fn step_move_animation(hwnd: HWND) {
    let snapshot = WIDGETS.with_borrow(|all| {
        all.get(&(hwnd as isize))
            .and_then(|s| s.animation.map(|a| (a, s.width, s.height)))
    });
    let Some((mut anim, width, height)) = snapshot else {
        unsafe {
            KillTimer(hwnd, ANIM_TIMER);
        }
        return;
    };
    let (next_x, finished) = anim.sample(Instant::now());
    if next_x != anim.current_x {
        move_layered_window(hwnd, next_x, anim.y);
        anim.current_x = next_x;
    }
    let next_rect = RECT {
        left: next_x,
        top: anim.y,
        right: next_x + width,
        bottom: anim.y + height,
    };
    WIDGETS.with_borrow_mut(|all| {
        if let Some(s) = all.get_mut(&(hwnd as isize)) {
            s.current_rect = Some(next_rect);
            s.animation = if finished { None } else { Some(anim) };
        }
    });
    if finished {
        unsafe {
            KillTimer(hwnd, ANIM_TIMER);
        }
    }
}

fn sync_widget(hwnd: HWND, reason: SyncReason) {
    let snapshot = WIDGETS.with_borrow(|all| {
        all.get(&(hwnd as isize)).map(|s| {
            (
                s.content.clone(),
                s.dpi,
                s.tb_hwnd,
                s.tb_rect,
                s.width,
                s.height,
                s.current_rect,
                s.target_rect,
                s.surface_committed,
                s.obstacles_applied,
                s.last_obstacle_rev,
                s.animation,
            )
        })
    });
    let Some((
        content,
        prev_dpi,
        prev_tb_hwnd,
        prev_tb_rect,
        prev_w,
        prev_h,
        mut current_rect,
        mut target_rect,
        mut surface_committed,
        obstacles_applied,
        last_obstacle_rev,
        mut animation,
    )) = snapshot
    else {
        return;
    };

    if content.items.is_empty() {
        WIDGETS.with_borrow(|all| {
            if let Some(s) = all.get(&(hwnd as isize)) {
                s.scanner.set_active(false);
            }
        });
        hide_widget(hwnd);
        return;
    }
    let Some(geom) = query_taskbar_geometry() else {
        hide_widget(hwnd);
        return;
    };
    if foreground_is_fullscreen(hwnd, geom.monitor_rect) {
        hide_widget(hwnd);
        return;
    }

    let snap = WIDGETS
        .with_borrow(|all| {
            all.get(&(hwnd as isize))
                .map(|s| s.scanner.latest_spans(geom.tb_hwnd, geom.tb_rect, geom.dpi))
        })
        .unwrap_or_default();

    let tb_height = (geom.tb_rect.bottom - geom.tb_rect.top).max(24);
    let prev_tb_height = (prev_tb_rect.bottom - prev_tb_rect.top).max(24);
    let tb_changed =
        prev_tb_hwnd != geom.tb_hwnd as isize || !same_rect(prev_tb_rect, geom.tb_rect);
    let is_visible = unsafe { IsWindowVisible(hwnd) } != 0;
    let was_visible = is_visible && current_rect.is_some() && surface_committed;

    // 仅在内容变更、首次展示、DPI 或任务栏高度变化时重绘位图。
    let need_redraw = matches!(
        reason,
        SyncReason::Reset | SyncReason::ContentChanged { .. } | SyncReason::DisplayChanged
    ) || !was_visible
        || prev_dpi != geom.dpi
        || prev_tb_height != tb_height;

    let (width, height, maybe_canvas) = if need_redraw {
        let canvas = render_content(&content, geom.dpi, tb_height);
        (canvas.width, canvas.height, Some(canvas))
    } else {
        (prev_w, prev_h, None)
    };

    let size_changed = width != prev_w || height != prev_h;
    let obstacles_changed =
        snap.scanned && (!obstacles_applied || snap.revision != last_obstacle_rev);
    let need_relayout = !was_visible
        || matches!(reason, SyncReason::Reset | SyncReason::DisplayChanged)
        || matches!(reason, SyncReason::ContentChanged { anchor_reset: true })
        || tb_changed
        || size_changed
        || obstacles_changed;

    if !was_visible {
        unsafe {
            KillTimer(hwnd, ANIM_TIMER);
        }
        animation = None;
        let next_target = resolve_widget_rect(
            geom.tb_rect,
            &snap.spans,
            geom.notify_rect,
            geom.start_rect,
            &content.position,
            width,
            height,
            geom.dpi,
            None,
        );
        if let Some(canvas) = maybe_canvas.as_ref() {
            let committed = commit_canvas(
                hwnd,
                canvas,
                Some(POINT {
                    x: next_target.left,
                    y: next_target.top,
                }),
                Some(SIZE {
                    cx: width,
                    cy: height,
                }),
            );
            surface_committed = committed;
        }
        // 首次从隐藏变为可见：先原子提交表面，再显示窗口，防止白帧或空帧闪烁。
        show_layered_window(hwnd);
        current_rect = Some(next_target);
        target_rect = Some(next_target);
    } else if need_relayout {
        let reset_anchor = matches!(reason, SyncReason::Reset | SyncReason::DisplayChanged)
            || matches!(reason, SyncReason::ContentChanged { anchor_reset: true })
            || tb_changed
            || size_changed;
        let keep_anchor = if reset_anchor {
            None
        } else {
            target_rect.or(current_rect)
        };
        let next_target = resolve_widget_rect(
            geom.tb_rect,
            &snap.spans,
            geom.notify_rect,
            geom.start_rect,
            &content.position,
            width,
            height,
            geom.dpi,
            keep_anchor,
        );
        let snap_immediately =
            size_changed || tb_changed || matches!(reason, SyncReason::Reset | SyncReason::DisplayChanged);
        if snap_immediately {
            unsafe {
                KillTimer(hwnd, ANIM_TIMER);
            }
            animation = None;
            if let Some(canvas) = maybe_canvas.as_ref() {
                surface_committed = commit_canvas(
                    hwnd,
                    canvas,
                    Some(POINT {
                        x: next_target.left,
                        y: next_target.top,
                    }),
                    Some(SIZE {
                        cx: width,
                        cy: height,
                    }),
                );
            } else if !current_rect.is_some_and(|r| same_rect(r, next_target)) {
                move_layered_window(hwnd, next_target.left, next_target.top);
            }
            if tb_changed || matches!(reason, SyncReason::Reset | SyncReason::DisplayChanged) {
                ensure_above_taskbar(hwnd, geom.tb_hwnd);
            }
            current_rect = Some(next_target);
            target_rect = Some(next_target);
        } else {
            // 同尺寸换位：若有新画面先原位提交，随后独立评估是否允许平滑位移动画。
            if let Some(canvas) = maybe_canvas.as_ref() {
                surface_committed = commit_canvas(hwnd, canvas, None, None);
            }
            let now = Instant::now();
            let (cur_x, cur_top) = if let Some(anim) = animation {
                (anim.sample(now).0, anim.y)
            } else {
                let r = current_rect.unwrap();
                (r.left, r.top)
            };
            let already_on_target = target_rect.is_some_and(|r| same_rect(r, next_target))
                && (animation.is_some() || (cur_x == next_target.left && cur_top == next_target.top));
            if !already_on_target {
                let mut path_obstacles = snap.spans.clone();
                if let Some(start) = geom.start_rect {
                    path_obstacles.push((start.left, start.right));
                }
                if let Some(notify) = geom.notify_rect {
                    path_obstacles.push((notify.left, notify.right));
                }
                let can_animate = next_target.top == cur_top
                    && next_target.left != cur_x
                    && system_animations_enabled()
                    && is_move_path_clear(cur_x, next_target.left, width, &path_obstacles);
                if can_animate {
                    if let Some(anim) = animation.as_mut() {
                        anim.retarget(next_target.left, next_target.top, now);
                    } else {
                        animation = Some(MoveAnimation {
                            start_x: cur_x,
                            target_x: next_target.left,
                            current_x: cur_x,
                            y: next_target.top,
                            started_at: now,
                            duration: MOVE_ANIM_DURATION,
                        });
                    }
                    current_rect = Some(RECT {
                        left: cur_x,
                        top: next_target.top,
                        right: cur_x + width,
                        bottom: next_target.top + height,
                    });
                    target_rect = Some(next_target);
                    unsafe {
                        SetTimer(hwnd, ANIM_TIMER, ANIM_INTERVAL_MS, None);
                    }
                } else {
                    unsafe {
                        KillTimer(hwnd, ANIM_TIMER);
                    }
                    animation = None;
                    if cur_x != next_target.left || cur_top != next_target.top {
                        move_layered_window(hwnd, next_target.left, next_target.top);
                    }
                    current_rect = Some(next_target);
                    target_rect = Some(next_target);
                }
            }
        }
    } else if let Some(canvas) = maybe_canvas.as_ref() {
        // 仅电量/充电状态/图标/配色变化：原位换图，不调 SetWindowPos，不打断进行中的位移动画。
        surface_committed = commit_canvas(hwnd, canvas, None, None);
    } else if animation.is_none() {
        ensure_above_taskbar(hwnd, geom.tb_hwnd);
    }

    WIDGETS.with_borrow_mut(|all| {
        if let Some(state) = all.get_mut(&(hwnd as isize)) {
            state.dpi = geom.dpi;
            state.tb_hwnd = geom.tb_hwnd as isize;
            state.tb_rect = geom.tb_rect;
            state.width = width;
            state.height = height;
            state.current_rect = current_rect;
            state.target_rect = target_rect;
            state.surface_committed = surface_committed;
            state.obstacles_applied = snap.scanned;
            state.last_obstacle_rev = if snap.scanned { snap.revision } else { 0 };
            state.animation = animation;
        }
    });
}

fn item_color(item: &TaskbarItem, dark_taskbar: bool) -> u32 {
    match item.charge {
        Charge::Charging => {
            if dark_taskbar {
                0x5eead4
            } else {
                0x087088
            }
        }
        Charge::Full => {
            if dark_taskbar {
                0x6ee7b7
            } else {
                0x047857
            }
        }
        _ => match item.percent {
            Some(p) if p < 20 => {
                if dark_taskbar {
                    0xff7b82
                } else {
                    0xc81e2b
                }
            }
            Some(p) if item.low_threshold >= 20 && p <= item.low_threshold => {
                if dark_taskbar {
                    0xfbbf24
                } else {
                    0xb45309
                }
            }
            _ => {
                if dark_taskbar {
                    0xffffff
                } else {
                    0x111827
                }
            }
        },
    }
}

fn item_text(item: &TaskbarItem) -> String {
    item.percent
        .map_or_else(|| "—".to_owned(), |p| format!("{p}%"))
}

struct FontGuard(HFONT);
impl FontGuard {
    fn new(family: &str, height: i32, weight: u32) -> Self {
        Self(unsafe {
            CreateFontW(
                -height,
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
                ANTIALIASED_QUALITY as u32,
                DEFAULT_PITCH as u32,
                wide(family).as_ptr(),
            )
        })
    }
}
impl Drop for FontGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                DeleteObject(self.0);
            }
        }
    }
}

/// 计算当前字号下所有可能电量文案的最大渲染宽度，使百分比位数变化时保持槽位宽恒定。
fn measure_max_battery_text_width(font_px: i32) -> i32 {
    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        if dc.is_null() {
            return (4 * font_px / 2).max(1);
        }
        let font = FontGuard::new("Segoe UI", font_px, FW_SEMIBOLD);
        let old = SelectObject(dc, font.0);
        let mut max_w = 1;
        for sample in ["100%", "88%", "—"] {
            let wide_text = wide(sample);
            let mut sz = SIZE::default();
            GetTextExtentPoint32W(
                dc,
                wide_text.as_ptr(),
                (wide_text.len() - 1) as i32,
                &mut sz,
            );
            max_w = max_w.max(sz.cx);
        }
        SelectObject(dc, old);
        DeleteDC(dc);
        max_w
    }
}

/// 根据布局模式与 DPI 返回单台设备的固定宽度，预留百分比位数与充电闪电占位。
pub(crate) fn fixed_item_width(
    layout: &TaskbarLayout,
    dpi: u32,
    icon_sz: i32,
    max_text_w: i32,
) -> i32 {
    match layout {
        TaskbarLayout::Stacked => (max_text_w + px(10, dpi))
            .max(icon_sz + px(16, dpi))
            .max(px(36, dpi)),
        TaskbarLayout::Inline => icon_sz + px(4, dpi) + max_text_w + px(10, dpi) + px(6, dpi),
    }
}

#[derive(Clone, Copy)]
struct RoundRect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
}

#[derive(Clone, Copy)]
struct Brush {
    rgb: u32,
    alpha: f32,
    stroke: f32,
}

/// 32bpp 预乘 ARGB（内存序 BGRA，小端 `0xAARRGGBB`）软件画布。
pub(crate) struct Canvas {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u32>,
}

impl Canvas {
    fn new(width: i32, height: i32, fill: u32) -> Self {
        Self {
            width: width.max(1),
            height: height.max(1),
            pixels: vec![fill; (width.max(1) as usize) * (height.max(1) as usize)],
        }
    }

    fn blend_px(&mut self, x: i32, y: i32, rgb: u32, alpha: f32) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height || alpha <= 0.0 {
            return;
        }
        let a = alpha.clamp(0.0, 1.0);
        let sa = (a * 255.0).round() as u32;
        if sa == 0 {
            return;
        }
        let sr = (((rgb >> 16) & 0xff) as f32 * a).round() as u32;
        let sg = (((rgb >> 8) & 0xff) as f32 * a).round() as u32;
        let sb = ((rgb & 0xff) as f32 * a).round() as u32;

        let idx = (y as usize) * (self.width as usize) + (x as usize);
        let dst = self.pixels[idx];
        let da = (dst >> 24) & 0xff;
        let dr = (dst >> 16) & 0xff;
        let dg = (dst >> 8) & 0xff;
        let db = dst & 0xff;
        let inv = 255 - sa;

        let oa = (sa + (da * inv + 127) / 255).min(255);
        let or = (sr + (dr * inv + 127) / 255).min(255);
        let og = (sg + (dg * inv + 127) / 255).min(255);
        let ob = (sb + (db * inv + 127) / 255).min(255);
        self.pixels[idx] = (oa << 24) | (or << 16) | (og << 8) | ob;
    }

    fn fill_round_rect(&mut self, rect: RoundRect, rgb: u32, alpha: f32) {
        let cx = rect.x + rect.w * 0.5;
        let cy = rect.y + rect.h * 0.5;
        let hx = (rect.w * 0.5 - rect.r).max(0.0);
        let hy = (rect.h * 0.5 - rect.r).max(0.0);
        let min_x = (rect.x - 1.5).floor().max(0.0) as i32;
        let max_x = (rect.x + rect.w + 1.5).ceil().min(self.width as f32 - 1.0) as i32;
        let min_y = (rect.y - 1.5).floor().max(0.0) as i32;
        let max_y = (rect.y + rect.h + 1.5).ceil().min(self.height as f32 - 1.0) as i32;
        for py in min_y..=max_y {
            for px in min_x..=max_x {
                let dx = ((px as f32 + 0.5) - cx).abs() - hx;
                let dy = ((py as f32 + 0.5) - cy).abs() - hy;
                let dist = (dx.max(0.0).hypot(dy.max(0.0))) + dx.max(dy).min(0.0) - rect.r;
                let cov = (0.5 - dist).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend_px(px, py, rgb, alpha * cov);
                }
            }
        }
    }

    fn stroke_round_rect(&mut self, rect: RoundRect, brush: Brush) {
        let cx = rect.x + rect.w * 0.5;
        let cy = rect.y + rect.h * 0.5;
        let hx = (rect.w * 0.5 - rect.r).max(0.0);
        let hy = (rect.h * 0.5 - rect.r).max(0.0);
        let half_s = brush.stroke * 0.5;
        let min_x = (rect.x - brush.stroke - 1.5).floor().max(0.0) as i32;
        let max_x = (rect.x + rect.w + brush.stroke + 1.5)
            .ceil()
            .min(self.width as f32 - 1.0) as i32;
        let min_y = (rect.y - brush.stroke - 1.5).floor().max(0.0) as i32;
        let max_y = (rect.y + rect.h + brush.stroke + 1.5)
            .ceil()
            .min(self.height as f32 - 1.0) as i32;
        for py in min_y..=max_y {
            for px in min_x..=max_x {
                let dx = ((px as f32 + 0.5) - cx).abs() - hx;
                let dy = ((py as f32 + 0.5) - cy).abs() - hy;
                let sdf = (dx.max(0.0).hypot(dy.max(0.0))) + dx.max(dy).min(0.0) - rect.r;
                let cov = (0.5 + half_s - sdf.abs()).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend_px(px, py, brush.rgb, brush.alpha * cov);
                }
            }
        }
    }

    fn stroke_segment(&mut self, from: (f32, f32), to: (f32, f32), brush: Brush) {
        let (x1, y1) = from;
        let (x2, y2) = to;
        let vx = x2 - x1;
        let vy = y2 - y1;
        let len2 = vx * vx + vy * vy;
        let half_s = brush.stroke * 0.5;
        let min_x = (x1.min(x2) - half_s - 1.5).floor().max(0.0) as i32;
        let max_x = (x1.max(x2) + half_s + 1.5)
            .ceil()
            .min(self.width as f32 - 1.0) as i32;
        let min_y = (y1.min(y2) - half_s - 1.5).floor().max(0.0) as i32;
        let max_y = (y1.max(y2) + half_s + 1.5)
            .ceil()
            .min(self.height as f32 - 1.0) as i32;
        for py in min_y..=max_y {
            for px in min_x..=max_x {
                let pxf = px as f32 + 0.5;
                let pyf = py as f32 + 0.5;
                let t = if len2 <= 1e-6 {
                    0.0
                } else {
                    (((pxf - x1) * vx + (pyf - y1) * vy) / len2).clamp(0.0, 1.0)
                };
                let dist = (pxf - (x1 + vx * t)).hypot(pyf - (y1 + vy * t));
                let cov = (0.5 + half_s - dist).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend_px(px, py, brush.rgb, brush.alpha * cov);
                }
            }
        }
    }

    fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, rgb: u32, alpha: f32) {
        let min_x = (cx - r - 1.5).floor().max(0.0) as i32;
        let max_x = (cx + r + 1.5).ceil().min(self.width as f32 - 1.0) as i32;
        let min_y = (cy - r - 1.5).floor().max(0.0) as i32;
        let max_y = (cy + r + 1.5).ceil().min(self.height as f32 - 1.0) as i32;
        for py in min_y..=max_y {
            for px in min_x..=max_x {
                let dist = ((px as f32 + 0.5) - cx).hypot((py as f32 + 0.5) - cy) - r;
                let cov = (0.5 - dist).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend_px(px, py, rgb, alpha * cov);
                }
            }
        }
    }

    fn draw_polyline(&mut self, pts: &[(f32, f32)], stroke: f32, rgb: u32, alpha: f32) {
        let brush = Brush { rgb, alpha, stroke };
        for pair in pts.windows(2) {
            self.stroke_segment(pair[0], pair[1], brush);
        }
    }

    /// 用临时黑底白字 GDI DIB 提取抗锯齿字形灰度，避免 GDI 直接写目标 DIB 时清零 Alpha 通道。
    fn draw_text_centered(&mut self, text: &str, rect: RECT, font_px: i32, rgb: u32) {
        let w = (rect.right - rect.left).max(1);
        let h = (rect.bottom - rect.top).max(1);
        unsafe {
            let mem_dc = CreateCompatibleDC(std::ptr::null_mut());
            if mem_dc.is_null() {
                return;
            }
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let dib = CreateDIBSection(
                mem_dc,
                &bmi,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            if !dib.is_null() && !bits.is_null() {
                let count = (w as usize) * (h as usize);
                std::ptr::write_bytes(bits.cast::<u32>(), 0, count);
                let old_bmp = SelectObject(mem_dc, dib);
                let font = FontGuard::new("Segoe UI", font_px, FW_SEMIBOLD);
                let old_font = SelectObject(mem_dc, font.0);
                SetBkMode(mem_dc, TRANSPARENT as i32);
                SetTextColor(mem_dc, 0x00ffffff);
                let mut local = RECT {
                    left: 0,
                    top: 0,
                    right: w,
                    bottom: h,
                };
                let wide_text = wide(text);
                DrawTextW(
                    mem_dc,
                    wide_text.as_ptr(),
                    (wide_text.len() - 1) as i32,
                    &mut local,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );
                GdiFlush();
                let src = std::slice::from_raw_parts(bits.cast::<u32>(), count);
                for py in 0..h {
                    for px in 0..w {
                        let px_val = src[(py as usize) * (w as usize) + (px as usize)];
                        let lum = ((px_val >> 16) & 0xff)
                            .max((px_val >> 8) & 0xff)
                            .max(px_val & 0xff);
                        if lum > 0 {
                            self.blend_px(rect.left + px, rect.top + py, rgb, lum as f32 / 255.0);
                        }
                    }
                }
                SelectObject(mem_dc, old_font);
                SelectObject(mem_dc, old_bmp);
            }
            if !dib.is_null() {
                DeleteObject(dib);
            }
            DeleteDC(mem_dc);
        }
    }

    fn draw_device_icon(&mut self, icon: DeviceIcon, left: f32, top: f32, size: f32, rgb: u32) {
        let s = (size / 11.5).max(1.2);
        let brush = Brush {
            rgb,
            alpha: 1.0,
            stroke: s,
        };
        let thin_brush = Brush {
            rgb,
            alpha: 1.0,
            stroke: s * 0.85,
        };
        let p = |u: f32, v: f32| (left + u * size / 16.0, top + v * size / 16.0);
        match icon {
            DeviceIcon::Mouse => {
                self.stroke_round_rect(
                    RoundRect {
                        x: left + 3.5 * size / 16.0,
                        y: top + 1.0 * size / 16.0,
                        w: 9.0 * size / 16.0,
                        h: 14.0 * size / 16.0,
                        r: 4.5 * size / 16.0,
                    },
                    brush,
                );
                self.stroke_segment(p(8.0, 3.8), p(8.0, 6.8), brush);
            }
            DeviceIcon::Keyboard => {
                self.stroke_round_rect(
                    RoundRect {
                        x: left + 1.0 * size / 16.0,
                        y: top + 3.0 * size / 16.0,
                        w: 14.0 * size / 16.0,
                        h: 10.0 * size / 16.0,
                        r: 2.2 * size / 16.0,
                    },
                    brush,
                );
                for kx in [4.2, 6.8, 9.2, 11.8] {
                    let (cx, cy) = p(kx, 6.2);
                    self.fill_circle(cx, cy, s * 0.55, rgb, 1.0);
                }
                self.stroke_segment(p(4.5, 9.8), p(11.5, 9.8), brush);
            }
            DeviceIcon::Headset => {
                let pts = [
                    p(2.2, 9.5),
                    p(2.4, 6.0),
                    p(4.4, 3.0),
                    p(8.0, 2.0),
                    p(11.6, 3.0),
                    p(13.6, 6.0),
                    p(13.8, 9.5),
                ];
                self.draw_polyline(&pts, s, rgb, 1.0);
                self.stroke_round_rect(
                    RoundRect {
                        x: left + 1.2 * size / 16.0,
                        y: top + 8.5 * size / 16.0,
                        w: 3.6 * size / 16.0,
                        h: 5.6 * size / 16.0,
                        r: 1.5 * size / 16.0,
                    },
                    brush,
                );
                self.stroke_round_rect(
                    RoundRect {
                        x: left + 11.2 * size / 16.0,
                        y: top + 8.5 * size / 16.0,
                        w: 3.6 * size / 16.0,
                        h: 5.6 * size / 16.0,
                        r: 1.5 * size / 16.0,
                    },
                    brush,
                );
            }
            DeviceIcon::Gamepad => {
                let pts = [
                    p(2.2, 12.6),
                    p(1.4, 10.2),
                    p(3.2, 4.4),
                    p(6.0, 4.0),
                    p(10.0, 4.0),
                    p(12.8, 4.4),
                    p(14.6, 10.2),
                    p(13.8, 12.6),
                    p(10.8, 9.8),
                    p(5.2, 9.8),
                    p(2.2, 12.6),
                ];
                self.draw_polyline(&pts, s, rgb, 1.0);
                self.stroke_segment(p(3.8, 7.0), p(6.2, 7.0), thin_brush);
                self.stroke_segment(p(5.0, 5.8), p(5.0, 8.2), thin_brush);
                let (bx1, by1) = p(10.4, 6.2);
                self.fill_circle(bx1, by1, s * 0.58, rgb, 1.0);
                let (bx2, by2) = p(11.8, 7.8);
                self.fill_circle(bx2, by2, s * 0.58, rgb, 1.0);
            }
            DeviceIcon::Bluetooth => {
                let pts = [
                    p(4.4, 4.6),
                    p(11.4, 10.6),
                    p(7.8, 14.0),
                    p(7.8, 2.0),
                    p(11.4, 5.4),
                    p(4.4, 11.4),
                ];
                self.draw_polyline(&pts, s, rgb, 1.0);
            }
            DeviceIcon::Receiver24g => {
                self.stroke_round_rect(
                    RoundRect {
                        x: left + 2.5 * size / 16.0,
                        y: top + 1.5 * size / 16.0,
                        w: 11.0 * size / 16.0,
                        h: 5.2 * size / 16.0,
                        r: 1.2 * size / 16.0,
                    },
                    brush,
                );
                self.stroke_round_rect(
                    RoundRect {
                        x: left + 4.4 * size / 16.0,
                        y: top + 6.4 * size / 16.0,
                        w: 7.2 * size / 16.0,
                        h: 7.8 * size / 16.0,
                        r: 1.0 * size / 16.0,
                    },
                    brush,
                );
                self.stroke_segment(p(6.6, 8.6), p(6.6, 11.4), thin_brush);
                self.stroke_segment(p(9.4, 8.6), p(9.4, 11.4), thin_brush);
            }
        }
    }

    fn draw_charging_bolt(&mut self, cx: f32, cy: f32, size: f32, rgb: u32) {
        let h = size * 0.5;
        let w = size * 0.34;
        let pts = [
            (cx + w * 0.25, cy - h),
            (cx - w * 0.75, cy + h * 0.1),
            (cx + w * 0.05, cy + h * 0.1),
            (cx - w * 0.25, cy + h),
            (cx + w * 0.75, cy - h * 0.1),
            (cx - w * 0.05, cy - h * 0.1),
        ];
        self.draw_polyline(
            &[pts[0], pts[1], pts[2], pts[3], pts[4], pts[5], pts[0]],
            (size * 0.15).max(1.1),
            rgb,
            1.0,
        );
    }
}

pub(crate) fn render_content(content: &Content, dpi: u32, tb_height: i32) -> Canvas {
    let pad_x = px(8, dpi);
    let item_gap = match content.layout {
        TaskbarLayout::Stacked => px(6, dpi),
        TaskbarLayout::Inline => px(10, dpi),
    };

    let (text_font_px, icon_sz, widget_h) = match content.layout {
        TaskbarLayout::Stacked => (
            px(12, dpi),
            px(16, dpi),
            px(40, dpi).min((tb_height - 4).max(28)),
        ),
        TaskbarLayout::Inline => (
            px(13, dpi),
            px(15, dpi),
            px(28, dpi).min((tb_height - 6).max(22)),
        ),
    };

    let max_text_w = measure_max_battery_text_width(text_font_px);
    let item_w = fixed_item_width(&content.layout, dpi, icon_sz, max_text_w);
    let count = content.items.len();
    let total_w =
        pad_x * 2 + (count as i32) * item_w + (count.saturating_sub(1) as i32) * item_gap;

    // 未开启文字底框时保持 alpha = 1 的近零透明背景，使透明间隔也能接收鼠标点击与悬停。
    let mut canvas = Canvas::new(total_w.max(1), widget_h.max(1), 0x01000000);
    if content.backdrop {
        let (fill_rgb, fill_a, edge_rgb, edge_a) = if content.dark_taskbar {
            (0x161b24, 0.72, 0xffffff, 0.16)
        } else {
            (0xf8fafc, 0.80, 0x0f172a, 0.14)
        };
        let pill = RoundRect {
            x: 0.5,
            y: 0.5,
            w: canvas.width as f32 - 1.0,
            h: canvas.height as f32 - 1.0,
            r: px(8, dpi) as f32,
        };
        canvas.fill_round_rect(pill, fill_rgb, fill_a);
        canvas.stroke_round_rect(
            pill,
            Brush {
                rgb: edge_rgb,
                alpha: edge_a,
                stroke: 1.0,
            },
        );
    }

    let bolt_slot = px(10, dpi);
    let mut cursor_x = pad_x;
    for item in &content.items {
        let text = item_text(item);
        let color = item_color(item, content.dark_taskbar);
        // 自定义 RGB 颜色仅用于设备图标本身；百分比与充电闪电继续沿用状态配色以保留低电/充电提示。
        let icon_color = item.icon_color.unwrap_or(color);
        match content.layout {
            TaskbarLayout::Stacked => {
                let icon_top = px(3, dpi).max((widget_h - icon_sz - text_font_px - px(2, dpi)) / 2);
                let icon_left = cursor_x as f32 + (item_w - icon_sz) as f32 * 0.5;
                canvas.draw_device_icon(
                    item.icon,
                    icon_left,
                    icon_top as f32,
                    icon_sz as f32,
                    icon_color,
                );
                if item.charge == Charge::Charging {
                    canvas.draw_charging_bolt(
                        icon_left + icon_sz as f32 + px(3, dpi) as f32,
                        icon_top as f32 + icon_sz as f32 * 0.35,
                        px(9, dpi) as f32,
                        color,
                    );
                }
                let text_rect = RECT {
                    left: cursor_x,
                    top: icon_top + icon_sz,
                    right: cursor_x + item_w,
                    bottom: widget_h - px(1, dpi),
                };
                canvas.draw_text_centered(&text, text_rect, text_font_px, color);
            }
            TaskbarLayout::Inline => {
                let icon_left = cursor_x as f32 + px(2, dpi) as f32;
                let icon_top = ((widget_h - icon_sz) as f32 * 0.5).max(1.0);
                canvas.draw_device_icon(item.icon, icon_left, icon_top, icon_sz as f32, icon_color);
                // 横排模式始终预留右侧闪电槽位宽度，防止充放电切换引发窗口宽度和位置抖动。
                let text_rect = RECT {
                    left: cursor_x + icon_sz + px(5, dpi),
                    top: 0,
                    right: cursor_x + item_w - bolt_slot,
                    bottom: widget_h,
                };
                canvas.draw_text_centered(&text, text_rect, text_font_px, color);
                if item.charge == Charge::Charging {
                    canvas.draw_charging_bolt(
                        (cursor_x + item_w - px(5, dpi)) as f32,
                        widget_h as f32 * 0.5,
                        px(10, dpi) as f32,
                        color,
                    );
                }
            }
        }
        cursor_x += item_w + item_gap;
    }

    canvas
}

fn emit(hwnd: HWND, action: Action) {
    let cb = WIDGETS.with_borrow(|all| all.get(&(hwnd as isize)).map(|s| s.callback.clone()));
    if let Some(cb) = cb {
        (cb.borrow_mut())(action);
    }
}

fn append_menu_item(menu: HMENU, id: u32, text: &str, checked: bool) {
    let mut flags = MF_STRING;
    if checked {
        flags |= MF_CHECKED;
    }
    unsafe {
        AppendMenuW(menu, flags, id as usize, wide(text).as_ptr());
    }
}

fn show_context_menu(hwnd: HWND, cursor: POINT) {
    let snapshot = WIDGETS.with_borrow(|all| {
        all.get(&(hwnd as isize))
            .map(|s| (s.content.clone(), s.tooltip))
    });
    let Some((content, tooltip)) = snapshot else {
        return;
    };
    tooltip.hide();
    let loc = content.locale;
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        let pos_menu = CreatePopupMenu();
        let layout_menu = CreatePopupMenu();

        append_menu_item(menu, CMD_OPEN, label(loc, "taskbar.menu.open"), false);
        append_menu_item(
            menu,
            CMD_REFRESH,
            label(loc, "tray.menu.refresh_now"),
            false,
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());

        if !pos_menu.is_null() {
            append_menu_item(
                pos_menu,
                CMD_POS_TRAY_LEFT,
                label(loc, "settings.taskbar_position.tray_left"),
                content.position == TaskbarPosition::TrayLeft,
            );
            append_menu_item(
                pos_menu,
                CMD_POS_TASKBAR_LEFT,
                label(loc, "settings.taskbar_position.taskbar_left"),
                content.position == TaskbarPosition::TaskbarLeft,
            );
            AppendMenuW(
                menu,
                MF_POPUP,
                pos_menu as usize,
                wide(label(loc, "taskbar.menu.position")).as_ptr(),
            );
        }

        if !layout_menu.is_null() {
            append_menu_item(
                layout_menu,
                CMD_LAYOUT_STACKED,
                label(loc, "settings.taskbar_layout.stacked"),
                content.layout == TaskbarLayout::Stacked,
            );
            append_menu_item(
                layout_menu,
                CMD_LAYOUT_INLINE,
                label(loc, "settings.taskbar_layout.inline"),
                content.layout == TaskbarLayout::Inline,
            );
            AppendMenuW(
                menu,
                MF_POPUP,
                layout_menu as usize,
                wide(label(loc, "taskbar.menu.layout")).as_ptr(),
            );
        }

        append_menu_item(
            menu,
            CMD_BACKDROP,
            label(loc, "taskbar.menu.backdrop"),
            content.backdrop,
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        append_menu_item(menu, CMD_CLEAR, label(loc, "taskbar.menu.clear"), false);
        append_menu_item(menu, CMD_SETTINGS, label(loc, "tray.settings"), false);
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        append_menu_item(menu, CMD_QUIT, label(loc, "tray.quit"), false);

        SetForegroundWindow(hwnd);
        let selected = TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            cursor.x,
            cursor.y,
            hwnd,
            std::ptr::null(),
        ) as u32;
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);

        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect);
        match selected {
            CMD_OPEN => emit(
                hwnd,
                Action::OpenFlyout([(rect.left + rect.right) / 2, rect.top]),
            ),
            CMD_REFRESH => emit(hwnd, Action::Refresh),
            CMD_POS_TRAY_LEFT => emit(hwnd, Action::SetPosition(TaskbarPosition::TrayLeft)),
            CMD_POS_TASKBAR_LEFT => emit(hwnd, Action::SetPosition(TaskbarPosition::TaskbarLeft)),
            CMD_LAYOUT_STACKED => emit(hwnd, Action::SetLayout(TaskbarLayout::Stacked)),
            CMD_LAYOUT_INLINE => emit(hwnd, Action::SetLayout(TaskbarLayout::Inline)),
            CMD_BACKDROP => emit(hwnd, Action::SetBackdrop(!content.backdrop)),
            CMD_CLEAR => emit(hwnd, Action::ClearPinned),
            CMD_SETTINGS => emit(hwnd, Action::OpenSettings),
            CMD_QUIT => emit(hwnd, Action::Quit),
            _ => (),
        }
    }
}

unsafe extern "system" fn procedure(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let taskbar_created = WIDGETS
        .with_borrow(|all| all.get(&(hwnd as isize)).map(|s| s.taskbar_created_msg))
        .unwrap_or(0);
    if taskbar_created != 0 && msg == taskbar_created {
        unsafe {
            KillTimer(hwnd, COALESCE_TIMER);
            KillTimer(hwnd, ANIM_TIMER);
        }
        WIDGETS.with_borrow(|all| {
            if let Some(s) = all.get(&(hwnd as isize)) {
                s.scanner.wake();
            }
        });
        sync_widget(hwnd, SyncReason::Reset);
        return 0;
    }

    match msg {
        WM_APP_OBSTACLES => {
            let applied = WIDGETS
                .with_borrow(|all| {
                    all.get(&(hwnd as isize)).map(|s| {
                        s.scanner.acknowledge_notify();
                        s.obstacles_applied
                    })
                })
                .unwrap_or(true);
            if !applied {
                sync_widget(hwnd, SyncReason::Obstacles);
            } else {
                unsafe {
                    SetTimer(hwnd, COALESCE_TIMER, COALESCE_DELAY_MS, None);
                }
            }
            0
        }
        WM_TIMER if w == COALESCE_TIMER => {
            unsafe {
                KillTimer(hwnd, COALESCE_TIMER);
            }
            sync_widget(hwnd, SyncReason::Obstacles);
            0
        }
        WM_TIMER if w == ANIM_TIMER => {
            step_move_animation(hwnd);
            0
        }
        WM_TIMER if w == SYNC_TIMER => {
            sync_widget(hwnd, SyncReason::Poll);
            0
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED => {
            unsafe {
                KillTimer(hwnd, COALESCE_TIMER);
            }
            WIDGETS.with_borrow(|all| {
                if let Some(s) = all.get(&(hwnd as isize)) {
                    s.scanner.wake();
                }
            });
            sync_widget(hwnd, SyncReason::DisplayChanged);
            0
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            sync_widget(hwnd, SyncReason::Poll);
            0
        }
        WM_MOUSEMOVE => {
            let mut rect = RECT::default();
            unsafe {
                if GetWindowRect(hwnd, &mut rect) != 0 {
                    let tooltip =
                        WIDGETS.with_borrow(|all| all.get(&(hwnd as isize)).map(|s| s.tooltip));
                    if let Some(target) = tooltip {
                        target.enter_rect(rect);
                    }
                }
            }
            0
        }
        WM_LBUTTONUP => {
            let mut rect = RECT::default();
            unsafe {
                GetWindowRect(hwnd, &mut rect);
            }
            let tooltip = WIDGETS.with_borrow(|all| all.get(&(hwnd as isize)).map(|s| s.tooltip));
            if let Some(target) = tooltip {
                target.hide();
            }
            emit(
                hwnd,
                Action::OpenFlyout([(rect.left + rect.right) / 2, rect.top]),
            );
            0
        }
        WM_RBUTTONUP => {
            let mut cursor = POINT::default();
            unsafe {
                GetCursorPos(&mut cursor);
            }
            show_context_menu(hwnd, cursor);
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_ERASEBKGND => 1,
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use battery_protocol::{Descriptor, Reading, Source};

    #[test]
    fn content_filters_offline_held_and_preserves_pinned_order() {
        let mut state = State::demo();
        let mouse_id = state.rows[0].device.id.clone();
        let kb_id = state.rows[1].device.id.clone();
        let bt = Descriptor {
            id: "bt:headset".into(),
            name: "Buds".into(),
            kind: Kind::Bluetooth,
            legacy_ids: vec![],
            identity: None,
            generic_name: None,
        };
        state.prefs.bluetooth.insert(bt.id.clone());
        state.rows.push(crate::model::Row {
            device: bt.clone(),
            battery: Reading::new(58, Charge::Discharging, Source::Ble),
            error: None,
            sampled: 1,
            held: false,
        });
        state.prefs.taskbar_devices = vec![bt.id.clone(), kb_id.clone(), mouse_id.clone()];

        // 离线或仅保留缓存（held）的设备不显示在任务栏。
        state.rows[1].error = Some("offline".into());
        state.rows[1].held = true;

        let content = Content::of(&state, true);
        assert_eq!(
            content
                .items
                .iter()
                .map(|i| i.id.as_str())
                .collect::<Vec<_>>(),
            vec![bt.id.as_str(), mouse_id.as_str()]
        );
        assert_eq!(content.items[0].percent, Some(58));
        assert_eq!(content.items[1].percent, Some(72));

        // 恢复在线后自动恢复显示。
        state.rows[1].error = None;
        state.rows[1].held = false;
        let restored = Content::of(&state, true);
        assert_eq!(restored.items.len(), 3);
        assert_eq!(restored.items[1].id, kb_id);
        assert_eq!(restored.items[1].charge, Charge::Charging);
    }

    #[test]
    fn widget_rect_places_tray_left_and_taskbar_left_with_bounds() {
        let tb = RECT {
            left: 0,
            top: 1032,
            right: 1920,
            bottom: 1080,
        };
        let notify = Some(RECT {
            left: 1620,
            top: 1032,
            right: 1920,
            bottom: 1080,
        });
        let centered_start = Some(RECT {
            left: 720,
            top: 1032,
            right: 760,
            bottom: 1080,
        });
        let left_start = Some(RECT {
            left: 0,
            top: 1032,
            right: 48,
            bottom: 1080,
        });

        let r_tray = compute_widget_rect(
            tb,
            notify,
            centered_start,
            &TaskbarPosition::TrayLeft,
            140,
            40,
            96,
        );
        assert_eq!(r_tray.right, 1620 - 6);
        assert_eq!(r_tray.top, 1036);

        let r_centered_left = compute_widget_rect(
            tb,
            notify,
            centered_start,
            &TaskbarPosition::TaskbarLeft,
            140,
            40,
            96,
        );
        assert_eq!(r_centered_left.left, 12);

        let r_avoid_start = compute_widget_rect(
            tb,
            notify,
            left_start,
            &TaskbarPosition::TaskbarLeft,
            140,
            40,
            96,
        );
        assert_eq!(r_avoid_start.left, 48 + 8);
    }

    #[test]
    fn avoiding_layout_skips_search_widgets_and_external_overlays() {
        let tb = RECT {
            left: 0,
            top: 1032,
            right: 1920,
            bottom: 1080,
        };
        let widget_w = 140;

        // 1. Win11 居中任务栏：左侧小组件按钮 [8, 128]，居中图标组 [680, 1240]，右侧托盘 [1680, 1916] + 第三方小组件 [1520, 1660]
        let centered_occupied = [(8, 128), (680, 1240), (1520, 1660), (1680, 1916)];
        let tray_left = compute_widget_rect_avoiding(
            tb,
            &centered_occupied,
            None,
            None,
            &TaskbarPosition::TrayLeft,
            widget_w,
            40,
            96,
        );
        // 紧贴第三方小组件左侧（1520 - pad(4) - 140 = 1376），不重叠。
        assert!(tray_left.right <= 1520 - 4);
        assert!(tray_left.left >= 1240 + 4);

        let tb_left = compute_widget_rect_avoiding(
            tb,
            &centered_occupied,
            None,
            None,
            &TaskbarPosition::TaskbarLeft,
            widget_w,
            40,
            96,
        );
        // 紧贴左侧小组件按钮右侧（128 + pad(4) = 132），不重叠。
        assert_eq!(tb_left.left, 132);
        assert!(tb_left.right <= 680 - 4);

        // 2. 左对齐任务栏：开始 + 搜索胶囊 + 任务栏图标占据 [8, 520]，托盘 [1700, 1916]
        let left_aligned_occupied = [(8, 56), (60, 240), (248, 520), (1700, 1916)];
        let avoid_search = compute_widget_rect_avoiding(
            tb,
            &left_aligned_occupied,
            None,
            None,
            &TaskbarPosition::TaskbarLeft,
            widget_w,
            40,
            96,
        );
        // 不会压在搜索框 [60, 240] 上，而是排到图标区右侧（520 + pad(4) = 524）。
        assert_eq!(avoid_search.left, 524);

        let right_aligned = compute_widget_rect_avoiding(
            tb,
            &left_aligned_occupied,
            None,
            None,
            &TaskbarPosition::TrayLeft,
            widget_w,
            40,
            96,
        );
        assert_eq!(right_aligned.right, 1700 - 4);

        // 3. 极满任务栏（合并安全边距后最大空闲为 [804, 904]，宽 100px < 140px）：选择最宽空闲区间居中放置。
        let crowded = [(8, 800), (908, 1916)];
        let fallback_wide = compute_widget_rect_avoiding(
            tb,
            &crowded,
            None,
            None,
            &TaskbarPosition::TrayLeft,
            widget_w,
            40,
            96,
        );
        assert_eq!((fallback_wide.left, fallback_wide.right), (784, 924));
    }

    #[test]
    fn canvas_renders_visible_icon_text_and_backdrop_difference() {
        let items = vec![
            TaskbarItem {
                id: "m".into(),
                name: "Mouse".into(),
                icon: DeviceIcon::Mouse,
                icon_color: None,
                percent: Some(55),
                charge: Charge::Discharging,
                low_threshold: 20,
            },
            TaskbarItem {
                id: "k".into(),
                name: "Keyboard".into(),
                icon: DeviceIcon::Keyboard,
                icon_color: None,
                percent: Some(60),
                charge: Charge::Charging,
                low_threshold: 20,
            },
        ];
        let base = Content {
            items: items.clone(),
            position: TaskbarPosition::TrayLeft,
            layout: TaskbarLayout::Stacked,
            backdrop: false,
            dark_taskbar: true,
            locale: Locale::Zh,
        };
        let mut with_backdrop = base.clone();
        with_backdrop.backdrop = true;

        let plain = render_content(&base, 96, 48);
        let pill = render_content(&with_backdrop, 96, 48);
        assert_eq!((plain.width, plain.height), (pill.width, pill.height));
        assert!(plain.pixels.iter().any(|p| (p >> 24) > 200));
        assert_ne!(plain.pixels, pill.pixels);

        let mut inline = base;
        inline.layout = TaskbarLayout::Inline;
        let inline_canvas = render_content(&inline, 96, 48);
        assert!(inline_canvas.height < plain.height);
        assert!(inline_canvas.pixels.iter().any(|p| (p >> 24) > 200));
    }

    #[test]
    fn custom_device_icon_and_color_apply_only_to_icon_preserving_battery_text_color() {
        let mut st = State::demo();
        let id = st.rows[0].device.id.clone();
        st.prefs.taskbar_devices.push(id.clone());
        st.prefs.device_appearances.insert(
            id,
            crate::model::DeviceAppearance {
                icon: Some(DeviceIcon::Gamepad),
                color: Some(0x3b82f6),
            },
        );
        let content = Content::of(&st, true);
        assert_eq!(content.items.len(), 1);
        assert_eq!(content.items[0].icon, DeviceIcon::Gamepad);
        assert_eq!(content.items[0].icon_color, Some(0x3b82f6));

        let mut default_content = content.clone();
        default_content.items[0].icon_color = None;
        let colored = render_content(&content, 96, 48);
        let default = render_content(&default_content, 96, 48);
        assert_ne!(colored.pixels, default.pixels);
        // 只有顶部图标区域的像素颜色不同，底部百分比文字保持一致的电池状态色。
        let mid_row = (colored.height / 2) as usize;
        let stride = colored.width as usize;
        assert_eq!(
            colored.pixels[mid_row * stride..],
            default.pixels[mid_row * stride..]
        );
    }

    #[test]
    fn fixed_width_stays_stable_across_percent_digits_and_charging_toggle() {
        for layout in [TaskbarLayout::Inline, TaskbarLayout::Stacked] {
            for dpi in [96u32, 144u32] {
                let tb_height = px(48, dpi);
                let base = Content {
                    items: vec![
                        TaskbarItem {
                            id: "m".into(),
                            name: "Mouse".into(),
                            icon: DeviceIcon::Mouse,
                            icon_color: None,
                            percent: Some(0),
                            charge: Charge::Discharging,
                            low_threshold: 20,
                        },
                        TaskbarItem {
                            id: "k".into(),
                            name: "Keyboard".into(),
                            icon: DeviceIcon::Keyboard,
                            icon_color: None,
                            percent: Some(9),
                            charge: Charge::Discharging,
                            low_threshold: 20,
                        },
                    ],
                    position: TaskbarPosition::TrayLeft,
                    layout: layout.clone(),
                    backdrop: false,
                    dark_taskbar: true,
                    locale: Locale::Zh,
                };
                let baseline = render_content(&base, dpi, tb_height);

                for (percent, charge) in [
                    (Some(9), Charge::Discharging),
                    (Some(10), Charge::Discharging),
                    (Some(99), Charge::Discharging),
                    (Some(100), Charge::Discharging),
                    (Some(100), Charge::Charging),
                    (None, Charge::Charging),
                ] {
                    let mut next = base.clone();
                    next.items[0].percent = percent;
                    next.items[0].charge = charge;
                    let canvas = render_content(&next, dpi, tb_height);
                    assert_eq!(
                        (canvas.width, canvas.height),
                        (baseline.width, baseline.height)
                    );
                }

                let mut changed = base.clone();
                changed.items[0].percent = Some(100);
                changed.items[0].charge = Charge::Charging;
                let changed_canvas = render_content(&changed, dpi, tb_height);
                assert_ne!(baseline.pixels, changed_canvas.pixels);
            }
        }
    }

    #[test]
    fn resolve_widget_rect_keeps_valid_previous_position_and_relocates_on_collision() {
        let tb = RECT {
            left: 0,
            top: 1032,
            right: 1920,
            bottom: 1080,
        };
        let width = 140;
        let height = 28;
        let occupied = [(8, 128), (680, 1240), (1680, 1916)];

        let initial = resolve_widget_rect(
            tb,
            &occupied,
            None,
            None,
            &TaskbarPosition::TrayLeft,
            width,
            height,
            96,
            None,
        );
        assert_eq!(initial.right, 1680 - 4);

        // 任务栏另一侧出现新图标，但原位置仍完全安全：保持旧位置不反复跳动。
        let extra_left_app = [(8, 128), (620, 1240), (1680, 1916)];
        let kept_left_change = resolve_widget_rect(
            tb,
            &extra_left_app,
            None,
            None,
            &TaskbarPosition::TrayLeft,
            width,
            height,
            96,
            Some(initial),
        );
        assert_eq!(kept_left_change.left, initial.left);
        assert_eq!(kept_left_change.top, initial.top);

        // 右侧仅缩进 2px（滞回容差内）仍保留旧坐标。
        let slight_shift = [(8, 128), (680, 1240), (1678, 1916)];
        let kept_hysteresis = resolve_widget_rect(
            tb,
            &slight_shift,
            None,
            None,
            &TaskbarPosition::TrayLeft,
            width,
            height,
            96,
            Some(initial),
        );
        assert_eq!(kept_hysteresis.left, initial.left);

        // 障碍物扩张侵占原位置时，自动向左避让。
        let collided = [(8, 128), (680, 1240), (1560, 1916)];
        let moved = resolve_widget_rect(
            tb,
            &collided,
            None,
            None,
            &TaskbarPosition::TrayLeft,
            width,
            height,
            96,
            Some(initial),
        );
        assert_eq!(moved.right, 1560 - 4);

        // 障碍物随后撤退，刚避让后的位置依然有效，继续保持稳定不来回晃动。
        let restored = resolve_widget_rect(
            tb,
            &occupied,
            None,
            None,
            &TaskbarPosition::TrayLeft,
            width,
            height,
            96,
            Some(moved),
        );
        assert_eq!(restored.left, moved.left);
    }

    #[test]
    fn move_path_clearance_allows_sliding_away_and_blocks_crossing_system_buttons() {
        let width = 140;
        // 1. 被新出现的右侧托盘元素推向左方脱离，未跨过任何按钮 -> 允许平滑移动。
        let tray_growth = [(8, 128), (680, 1100), (1560, 1916)];
        assert!(is_move_path_clear(1536, 1416, width, &tray_growth));

        // 2. 路径跨过居中开始按钮 [680, 728] -> 禁止动画，防止扫过系统按钮。
        let cross_start = [(680, 728)];
        assert!(!is_move_path_clear(800, 500, width, &cross_start));

        // 3. 路径穿过其它第三方图标 [900, 960] -> 禁止动画。
        assert!(!is_move_path_clear(720, 1020, width, &[(900, 960)]));
    }

    #[test]
    fn move_animation_eases_out_retargets_from_current_position_and_finishes_exactly() {
        assert!((ease_out_cubic(0.0) - 0.0).abs() < 1e-9);
        assert!((ease_out_cubic(1.0) - 1.0).abs() < 1e-9);
        assert!(ease_out_cubic(0.5) > 0.5);

        let t0 = Instant::now();
        let mut anim = MoveAnimation {
            start_x: 1000,
            target_x: 1100,
            current_x: 1000,
            y: 1040,
            started_at: t0,
            duration: Duration::from_millis(150),
        };

        let (mid_x, mid_done) = anim.sample(t0 + Duration::from_millis(75));
        assert!(!mid_done);
        assert!(mid_x > 1050 && mid_x < 1100);

        // 中途收到新目标：从当前已走到的坐标继续插值，不能跳回起点。
        let t_mid = t0 + Duration::from_millis(75);
        anim.retarget(1200, 1040, t_mid);
        assert_eq!(anim.start_x, mid_x);
        assert_eq!(anim.target_x, 1200);

        let (end_x, end_done) = anim.sample(t_mid + Duration::from_millis(150));
        assert!(end_done);
        assert_eq!(end_x, 1200);
    }

    #[test]
    fn scanner_latest_spans_is_nonblocking_when_unscanned() {
        let scanner = ObstacleScanner::start(std::ptr::null_mut(), false);
        let tb_rect = RECT {
            left: 0,
            top: 1032,
            right: 1920,
            bottom: 1080,
        };
        let t0 = Instant::now();
        let snap = scanner.latest_spans(std::ptr::null_mut(), tb_rect, 96);
        assert!(t0.elapsed() < Duration::from_millis(50));
        assert!(!snap.scanned);
        assert!(snap.spans.is_empty());
    }
}

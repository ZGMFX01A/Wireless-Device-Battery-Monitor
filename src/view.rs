// SPDX-FileCopyrightText: 2026 ZGMFX01A
// SPDX-License-Identifier: LicenseRef-Wireless-Device-Battery-Monitor
// See LICENSE in the repository root for the full terms.

//! Only this process starts GPUI/Direct3D. No HID, GATT, updater or web engine here.
use crate::model::Style;
use crate::model::*;
use battery_protocol::{Charge, EngineMode, Kind};
use gpui::{MouseButton as Button, prelude::*, *};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::io::{BufRead, Write};
use windows_sys::Win32::{
    Foundation::{HWND, POINT},
    Graphics::Gdi::*,
    UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::*},
};
const PROJECT_HOME_URL: &str = "https://github.com/ZGMFX01A/Wireless-Device-Battery-Monitor";
/// 触发兼容模式提示弹窗的连续无果操作统计窗口（60 秒）。
const COMPATIBLE_HINT_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);
/// 短时间窗口内连续未发现新设备的最小触发次数。
const COMPATIBLE_HINT_THRESHOLD: usize = 2;

/// “添加键盘”弹窗追踪状态。
enum KeyboardPickAttempt {
    /// 正在等待本次候选扫描结果。
    Scanning(u64),
    /// 已有未绑定候选设备展示给用户，等待绑定或取消关闭。
    AwaitingBind,
    /// 本次候选扫描已因无候选被计入，避免重复推送同一回执导致重复计数。
    Counted,
}

/// 追踪短时间内的扫描设备与添加键盘操作，在连续无新增设备时提示开启兼容模式。
#[derive(Default)]
struct CompatibleHintTracker {
    /// 最近在有效时间窗口内完成但未新增设备的尝试时间戳。
    empty_attempts: Vec<std::time::Instant>,
    /// 正在进行的扫描请求及其扫描前已存在的设备 ID 集合。
    pending_scan: Option<(u64, std::collections::BTreeSet<String>)>,
    /// 正在追踪的“添加键盘”弹窗状态。
    pending_keyboard_pick: Option<KeyboardPickAttempt>,
    /// 本次普通模式会话内用户是否已主动拒绝或关闭过提示弹窗。
    dismissed: bool,
}

impl CompatibleHintTracker {
    /// 截取当前面板设备 ID 集合，用于严格比较扫描前后是否出现新增设备。
    fn snapshot_row_ids(rows: &[Row]) -> std::collections::BTreeSet<String> {
        rows.iter().map(|r| r.device.id.clone()).collect()
    }

    /// 对比快照判断本次扫描是否发现了扫描前不存在的新设备 ID。
    fn has_new_device(before: &std::collections::BTreeSet<String>, after: &[Row]) -> bool {
        after.iter().any(|r| !before.contains(&r.device.id))
    }

    /// 记录一次扫描设备请求的起点与扫描前已有设备 ID 集合。
    fn begin_scan(&mut self, id: u64, rows: &[Row]) {
        self.pending_scan = Some((id, Self::snapshot_row_ids(rows)));
    }

    /// 记录一次“添加键盘”候选扫描请求。
    fn begin_keyboard_pick(&mut self, id: u64) {
        self.pending_keyboard_pick = Some(KeyboardPickAttempt::Scanning(id));
    }

    /// 取消尚未完成的键盘候选扫描追踪（如重新打开面板）。
    fn cancel_keyboard_pick(&mut self) {
        self.pending_keyboard_pick = None;
    }

    /// 仅清空已累计的无新增尝试次数，保留正在进行的扫描或弹窗追踪。
    fn clear_attempts(&mut self) {
        self.empty_attempts.clear();
    }

    /// 清空全部计数与进行中的追踪状态。
    fn reset_all(&mut self) {
        self.empty_attempts.clear();
        self.pending_scan = None;
        self.pending_keyboard_pick = None;
    }

    /// 成功绑定设备后清空尝试计数与键盘弹窗状态。
    fn on_device_bound(&mut self) {
        self.clear_attempts();
        self.pending_keyboard_pick = None;
    }

    /// 识别模式切换离开普通模式时重置计数并解除拒绝锁定，便于后续切回普通模式重新提示。
    fn on_mode_changed(&mut self, mode: EngineMode) {
        if mode != EngineMode::Normal {
            self.reset_all();
            self.dismissed = false;
        }
    }

    /// 用户确认开启兼容模式后重置追踪状态。
    fn on_accepted(&mut self) {
        self.reset_all();
        self.dismissed = false;
    }

    /// 用户主动关闭提示弹窗后标记本次会话已拒绝，避免反复打扰。
    fn on_dismissed(&mut self) {
        self.reset_all();
        self.dismissed = true;
    }

    /// 记录一次未新增设备的尝试，达到阈值且未被拒绝时返回 `true`。
    fn record_empty_attempt(&mut self, mode: EngineMode, now: std::time::Instant) -> bool {
        if mode != EngineMode::Normal || self.dismissed {
            return false;
        }
        self.empty_attempts
            .retain(|ts| now.saturating_duration_since(*ts) <= COMPATIBLE_HINT_WINDOW);
        self.empty_attempts.push(now);
        if self.empty_attempts.len() >= COMPATIBLE_HINT_THRESHOLD {
            self.empty_attempts.clear();
            true
        } else {
            false
        }
    }

    /// 完成一次手动扫描并判定是否触发兼容模式提示（严格检查是否新增设备 ID）。
    fn complete_scan(
        &mut self,
        id: u64,
        succeeded: bool,
        rows: &[Row],
        mode: EngineMode,
        now: std::time::Instant,
    ) -> bool {
        let Some((pending_id, before)) = self.pending_scan.take() else {
            return false;
        };
        if pending_id != id {
            self.pending_scan = Some((pending_id, before));
            return false;
        }
        if mode != EngineMode::Normal {
            self.on_mode_changed(mode);
            return false;
        }
        if !succeeded {
            return false;
        }
        if Self::has_new_device(&before, rows) {
            self.clear_attempts();
            false
        } else {
            self.record_empty_attempt(mode, now)
        }
    }

    /// 完成一次“添加键盘”候选扫描：无未绑定新键盘立即计数；有候选则等待用户是否绑定。
    fn complete_keyboard_pick(
        &mut self,
        id: u64,
        succeeded: bool,
        has_unbound_candidate: bool,
        mode: EngineMode,
        now: std::time::Instant,
    ) -> bool {
        if !matches!(
            self.pending_keyboard_pick,
            Some(KeyboardPickAttempt::Scanning(pending_id)) if pending_id == id
        ) {
            return false;
        }
        if mode != EngineMode::Normal {
            self.on_mode_changed(mode);
            return false;
        }
        if !succeeded {
            self.pending_keyboard_pick = None;
            return false;
        }
        if has_unbound_candidate {
            self.pending_keyboard_pick = Some(KeyboardPickAttempt::AwaitingBind);
            false
        } else {
            self.pending_keyboard_pick = Some(KeyboardPickAttempt::Counted);
            self.record_empty_attempt(mode, now)
        }
    }

    /// 当用户关闭“添加键盘”弹窗且未绑定任何设备时，将待绑定状态计入无新增尝试。
    fn finish_cancelled_keyboard_pick(
        &mut self,
        mode: EngineMode,
        now: std::time::Instant,
    ) -> bool {
        match self.pending_keyboard_pick.take() {
            Some(KeyboardPickAttempt::AwaitingBind) => self.record_empty_attempt(mode, now),
            Some(KeyboardPickAttempt::Scanning(_) | KeyboardPickAttempt::Counted) | None => false,
        }
    }
}
/// 返回当前系统时间对应的 Unix 秒时间戳；系统时钟异常时返回 0。
fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
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
/// 将当前窗口样式同步为目标模式所需的边框与按钮标志位，禁止最大化并按需启用/关闭四边缩放。
fn sync_window_style(h: HWND, resizable: bool) {
    if h.is_null() {
        return;
    }
    unsafe {
        let old_bits = GetWindowLongPtrW(h, GWL_STYLE) as u32;
        let mut new_bits = old_bits & !WS_MAXIMIZEBOX;
        if resizable {
            new_bits |= WS_THICKFRAME;
        } else {
            new_bits &= !WS_THICKFRAME;
        }
        if new_bits != old_bits {
            SetWindowLongPtrW(h, GWL_STYLE, new_bits as isize);
            SetWindowPos(
                h,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
}

/// 判定当前逻辑视口尺寸是否构成需要持久化的设置窗口缩放变更。
/// 未进入防抖序列时过滤 0 尺寸与 `<= 1px` DPI 舍入抖动；已在防抖序列中时保留最新 1px 调整。
fn settings_resize_candidate(
    current_logical: [u32; 2],
    settled_baseline: [u32; 2],
    pending: Option<[u32; 2]>,
) -> Option<[u32; 2]> {
    if current_logical[0] == 0 || current_logical[1] == 0 {
        return None;
    }
    let clamped = clamp_settings_window_size(current_logical);
    if let Some(pending) = pending {
        return (clamped != pending).then_some(clamped);
    }
    if clamped[0].abs_diff(settled_baseline[0]) <= 1
        && clamped[1].abs_diff(settled_baseline[1]) <= 1
    {
        return None;
    }
    Some(clamped)
}

/// 判断收到的 Host 状态快照是否已确认（或明确失败拒绝）本地待确认的设置窗口尺寸。
fn reconcile_unconfirmed_settings_size(
    unconfirmed: &mut Option<[u32; 2]>,
    prefs_size: Option<[u32; 2]>,
    note: Option<&Notice>,
) {
    if let Some(pending) = *unconfirmed
        && (prefs_size == Some(pending) || matches!(note, Some(Notice::OperationFailed)))
    {
        *unconfirmed = None;
    }
}

fn place(
    w: &Window,
    m: Mode,
    a: Option<[i32; 2]>,
    show: bool,
    settings_size: [u32; 2],
) -> Option<[u32; 2]> {
    let h = hwnd(w);
    if h.is_null() {
        return None;
    }
    // 在读取非客户区边距差值前先同步 WS_THICKFRAME/WS_MAXIMIZEBOX，确保测量基于当前模式样式。
    sync_window_style(h, m == Mode::Settings);
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
            return None;
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
        let [bw, bh] = if m == Mode::Flyout {
            FLYOUT_WINDOW_SIZE
        } else {
            clamp_settings_window_size(settings_size)
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
            let scale = dpi as f32 / 96.;
            return Some([
                (actual.right as f32 / scale).round().max(0.) as u32,
                (actual.bottom as f32 / scale).round().max(0.) as u32,
            ]);
        }
    }
    None
}
pub(crate) fn apps_use_dark_theme() -> bool {
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
pub fn locale(p: &Prefs) -> strings::Locale {
    match p.lang {
        Language::Zh => strings::Locale::Zh,
        Language::En => strings::Locale::En,
        Language::Ru => strings::Locale::Ru,
        Language::Auto => strings::Locale::from_windows_language(unsafe {
            windows_sys::Win32::Globalization::GetUserDefaultUILanguage()
        }),
    }
}
// Release metadata currently provides Chinese and English notes only.
pub fn english(p: &Prefs) -> bool {
    locale(p) != strings::Locale::Zh
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
    last_locale: Option<strings::Locale>,
    live: bool,
    pending_rows: Option<Vec<Row>>,
    settings_open: bool,
    scroll: [ScrollHandle; 2],
    drag_scroll: Option<f32>,
    last_scroll: (i32, i32),
    popup: bool,
    modal: Option<Dialog>,
    modal_subs: Vec<Subscription>,
    focus: FocusHandle,
    manual: Option<(u64, bool)>,
    toast: Option<(Toast, u64)>,
    compat_hint: CompatibleHintTracker,
    /// 标记托管窗口是否已完成 `Event::Open` 的初次与次帧定位，避免把程序定位误记为用户缩放。
    placement_settled: bool,
    /// 最近一次 `place()` 后在当前工作区实际落地的设置窗口逻辑尺寸，用于过滤工作区钳制引发的伪缩放事件。
    settled_settings_size: Option<[u32; 2]>,
    /// 待防抖提交的设置窗口逻辑尺寸及对应递增令牌。
    pending_settings_size: Option<([u32; 2], u64)>,
    /// 已发往 Host 但尚未在快照中确认的最新设置窗口尺寸，防止旧快照覆盖快速重开尺寸。
    unconfirmed_settings_size: Option<[u32; 2]>,
    /// 递增生成用于设置窗口尺寸防抖的本地令牌。
    resize_token: u64,
    _sub: Option<Subscription>,
    _bounds_sub: Option<Subscription>,
}
impl Panel {
    /// 获取当前设置窗口应使用的有效逻辑尺寸（优先未提交/待确认尺寸，其次持久化偏好）。
    fn effective_settings_window_size(&self) -> [u32; 2] {
        self.pending_settings_size
            .map(|(size, _)| size)
            .or(self.unconfirmed_settings_size)
            .unwrap_or_else(|| self.state.prefs.settings_window_size())
    }

    /// 记录设置窗口的缩放结果并在 300ms 无新变化后持久化；隐藏或切模式前会先强制提交。
    fn on_window_bounds_changed(&mut self, w: &Window, cx: &mut Context<Self>) {
        if !self.visible || self.mode != Mode::Settings || !self.placement_settled {
            return;
        }
        let vp = w.viewport_size();
        let current = [
            f32::from(vp.width).round().max(0.) as u32,
            f32::from(vp.height).round().max(0.) as u32,
        ];
        let settled = self
            .settled_settings_size
            .unwrap_or_else(|| self.effective_settings_window_size());
        let pending = self.pending_settings_size.map(|(size, _)| size);
        let Some(next) = settings_resize_candidate(current, settled, pending) else {
            return;
        };
        self.resize_token = self.resize_token.wrapping_add(1);
        let token = self.resize_token;
        self.pending_settings_size = Some((next, token));
        cx.spawn(async move |weak, cx| {
            Timer::after(std::time::Duration::from_millis(300)).await;
            let _ = weak.update(cx, |s, cx| {
                s.commit_pending_settings_size(Some(token), cx);
            });
        })
        .detach();
    }

    /// 提交待保存的设置窗口尺寸；当 `expected_token` 为 `None` 时立即强制冲刷。
    fn commit_pending_settings_size(
        &mut self,
        expected_token: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        let Some((size, token)) = self.pending_settings_size else {
            return;
        };
        if expected_token.is_some_and(|expected| expected != token) {
            return;
        }
        self.pending_settings_size = None;
        self.settled_settings_size = Some(size);
        if self.state.prefs.settings_window_size != Some(size)
            || self
                .unconfirmed_settings_size
                .is_some_and(|pending| pending != size)
        {
            self.state.prefs.settings_window_size = Some(size);
            if self.managed {
                self.unconfirmed_settings_size = Some(size);
            }
            self.issue(Command::SetSettingsWindowSize(size), cx);
        }
    }

    fn hide(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.commit_pending_settings_size(None, cx);
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
                self.state.update = crate::update::Status::Error(crate::update::Failure::Preview);
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
                self.state.note = Some(Notice::PreviewHardwareUnavailable);
            }
        }
        if !self.managed {
            self.state.demo_reconcile()
        }
        cx.notify()
    }
    fn t(&self, k: &str) -> &'static str {
        strings::label(locale(&self.state.prefs), k)
    }
    fn accept(&mut self, mut st: State, cx: &mut Context<Self>) {
        let completed = self
            .manual
            .is_some_and(|(id, _)| st.manual.id == id && st.manual.completed());
        let members_changed = st.prefs.keyboard != self.state.prefs.keyboard
            || st.prefs.bluetooth != self.state.prefs.bluetooth;
        let had_new_row = st.rows.iter().any(|row| {
            !self
                .state
                .rows
                .iter()
                .any(|old| old.device.id == row.device.id)
        });
        if !self.live && !completed && !members_changed {
            self.pending_rows = Some(std::mem::replace(
                &mut st.rows,
                std::mem::take(&mut self.state.rows),
            ));
        } else {
            self.pending_rows = None;
        }
        let note = if st.note != self.state.note {
            st.note.clone()
        } else {
            None
        };
        self.state = st;
        reconcile_unconfirmed_settings_size(
            &mut self.unconfirmed_settings_size,
            self.state.prefs.settings_window_size,
            self.state.note.as_ref(),
        );
        if self.state.prefs.engine_mode != EngineMode::Normal {
            self.compat_hint
                .on_mode_changed(self.state.prefs.engine_mode);
            if matches!(self.modal, Some(Dialog::SuggestCompatibleMode)) {
                self.modal = None;
            }
        }
        if completed {
            let (id, scan) = self.manual.take().unwrap();
            let scan_aborted = matches!(
                self.state.manual.error,
                Some(Notice::OperationFailed | Notice::Suspended)
            );
            let note = self.state.manual.error.clone().map_or_else(
                || {
                    Toast::Label(if scan {
                        "toast.scan_completed"
                    } else {
                        "toast.refresh_completed"
                    })
                },
                Toast::Notice,
            );
            self.toast(note, cx);
            if scan
                && self.compat_hint.complete_scan(
                    id,
                    !scan_aborted,
                    &self.state.rows,
                    self.state.prefs.engine_mode,
                    std::time::Instant::now(),
                )
            {
                self.maybe_open_compatible_hint(cx);
            }
        } else {
            if had_new_row {
                self.compat_hint.clear_attempts();
            }
            if let Some(n) = note {
                self.toast(Toast::Notice(n), cx)
            }
        }
    }

    /// 关闭升级内容弹窗时将当前版本标记为已读并通知服务层持久化。
    pub(super) fn dismiss_whats_new(&mut self, cx: &mut Context<Self>) {
        if let Some(notes) = self.state.installed_release_notes.take() {
            let version = notes.version;
            self.state.prefs.last_seen_release_notes_version = version.clone();
            self.issue(Command::MarkReleaseNotesSeen(version), cx);
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
pub(crate) mod skin;
#[path = "ui/strings.rs"]
pub(crate) mod strings;
enum Toast {
    Label(&'static str),
    Notice(Notice),
}
impl Toast {
    fn text(&self, locale: strings::Locale) -> String {
        match self {
            Self::Label(key) => strings::label(locale, key).into(),
            Self::Notice(notice) => strings::notice(locale, notice),
        }
    }
}
#[derive(Clone)]
enum Dialog {
    Pick {
        kind: Kind,
        id: u64,
        chosen: Option<String>,
        binding: bool,
    },
    EditDevice {
        id: String,
        initial_name: String,
        had_alias: bool,
        auto_icon: crate::device_display::DeviceIcon,
        icon: Option<crate::device_display::DeviceIcon>,
        name_input: Entity<crate::entry::TextInput>,
        color_input: Entity<crate::entry::TextInput>,
    },
    Remove(String, Kind, String),
    ConfirmCompatibleMode,
    SuggestCompatibleMode,
    ConfirmUnknownProbe,
    Update,
    WhatsNew,
    SupportStar,
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
            let initial_size = if initial == Mode::Settings {
                DEFAULT_SETTINGS_WINDOW_SIZE
            } else {
                FLYOUT_WINDOW_SIZE
            };
            let bounds = Bounds::centered(
                None,
                size(px(initial_size[0] as f32), px(initial_size[1] as f32)),
                cx,
            );
            let h = cx.open_window(
                WindowOptions {
                    titlebar: None,
                    show: !managed,
                    focus: !managed,
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_background: WindowBackgroundAppearance::Blurred,
                    is_resizable: true,
                    window_min_size: Some(size(
                        px(MIN_SETTINGS_WINDOW_SIZE[0] as f32),
                        px(MIN_SETTINGS_WINDOW_SIZE[1] as f32),
                    )),
                    ..Default::default()
                },
                |w, cx| {
                    cx.new(|cx| {
                        let dark_auto = apps_use_dark_theme();
                        sync_window_style(hwnd(w), initial == Mode::Settings);
                        material::apply(hwnd(w), dark_auto);
                        w.set_window_title(&format!(
                            "{} · GPUI",
                            strings::label(locale(&Prefs::default()), "app.header_title")
                        ));
                        let sub = cx.observe_window_activation(w, |s: &mut Panel, w, cx| {
                            if w.is_window_active() {
                                s.has_focus = true
                            } else if s.managed && s.has_focus && s.mode == Mode::Flyout {
                                s.hide(w, cx)
                            }
                        });
                        let bounds_sub = cx.observe_window_bounds(w, |s: &mut Panel, w, cx| {
                            s.on_window_bounds_changed(w, cx);
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
                            dark_auto,
                            last_material: None,
                            last_locale: None,
                            live: true,
                            pending_rows: None,
                            settings_open: initial == Mode::Settings,
                            scroll: Default::default(),
                            drag_scroll: None,
                            last_scroll: (0, 0),
                            popup: false,
                            modal: None,
                            modal_subs: Vec::new(),
                            focus,
                            manual: None,
                            toast: None,
                            compat_hint: Default::default(),
                            placement_settled: !managed,
                            settled_settings_size: None,
                            pending_settings_size: None,
                            unconfirmed_settings_size: None,
                            resize_token: 0,
                            _sub: Some(sub),
                            _bounds_sub: Some(bounds_sub),
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
                                        s.commit_pending_settings_size(None, cx);
                                        s.placement_settled = false;
                                        s.settled_settings_size = None;
                                        s.generation = g;
                                        s.mode = m;
                                        s.settings_open = m == Mode::Settings;
                                        s.modal = None;
                                        s.modal_subs.clear();
                                        s.compat_hint.cancel_keyboard_pick();
                                        if !s.state.demo {
                                            if s.state.prefs.show_release_notes_on_update
                                                && s.state
                                                    .installed_release_notes
                                                    .as_ref()
                                                    .is_some_and(|notes| {
                                                        notes.version
                                                            != s.state
                                                                .prefs
                                                                .last_seen_release_notes_version
                                                    })
                                            {
                                                s.modal = Some(Dialog::WhatsNew);
                                            } else if m == Mode::Flyout {
                                                let now = unix_now_secs();
                                                if s.state.prefs.should_show_star_prompt(now) {
                                                    s.modal = Some(Dialog::SupportStar);
                                                    s.state.prefs.last_star_prompt_at = now;
                                                    s.issue(Command::RecordStarPrompt(now), cx);
                                                }
                                            }
                                        }
                                        s.popup = false;
                                        w.focus(&s.focus);
                                        s.dark_auto = apps_use_dark_theme();
                                        s.visible = true;
                                        s.has_focus = false;
                                        let target = s.effective_settings_window_size();
                                        let _ = place(w, m, a, true, target);
                                        material::apply(hwnd(w), s.skin().dark);
                                        w.activate_window();
                                        // Windows may apply GPUI's initial hidden-window placement during the first ShowWindow.
                                        // Correct the actual client bounds after that transition, without stealing focus.
                                        let weak = cx.weak_entity();
                                        w.on_next_frame(move |w, cx| {
                                            let _ = weak.update(cx, |s, _| {
                                                if s.visible && s.generation == g {
                                                    let target = s.effective_settings_window_size();
                                                    let placed = place(w, m, a, false, target);
                                                    s.settled_settings_size = if m == Mode::Settings
                                                    {
                                                        placed
                                                    } else {
                                                        None
                                                    };
                                                    send(&Command::Presented(g));
                                                    s.placement_settled = true;
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
    use super::{COMPATIBLE_HINT_WINDOW, CompatibleHintTracker, EngineMode, Row, UiInbox};
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

    fn make_row(id: &str, with_battery: bool) -> Row {
        use battery_protocol::{Charge, Descriptor, Kind, Reading, Source};
        Row {
            device: Descriptor {
                id: id.into(),
                name: id.into(),
                kind: Kind::Mouse,
                legacy_ids: vec![],
                identity: None,
                generic_name: None,
            },
            battery: with_battery
                .then(|| Reading::new(80, Charge::Discharging, Source::Hidpp(1)))
                .flatten(),
            error: None,
            sampled: 0,
            held: false,
        }
    }

    #[test]
    fn compatible_hint_triggers_across_all_scan_and_keyboard_combinations() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let two = vec![make_row("mouse-1", true), make_row("kb-1", false)];
        let two_recovered = vec![make_row("mouse-1", true), make_row("kb-1", true)];

        // 0 -> 0：原来 0 台设备，连续只点击扫描 2 次仍为 0 台时触发
        let mut tracker = CompatibleHintTracker::default();
        tracker.begin_scan(1, &[]);
        assert!(!tracker.complete_scan(1, true, &[], EngineMode::Normal, t0));
        tracker.begin_scan(2, &[]);
        assert!(tracker.complete_scan(
            2,
            true,
            &[],
            EngineMode::Normal,
            t0 + Duration::from_secs(5)
        ));

        // 2 -> 2：原来 2 台设备，连续扫描仍为这 2 台（即使其中 1 台离线恢复读数）时触发
        let mut tracker = CompatibleHintTracker::default();
        tracker.begin_scan(1, &two);
        assert!(!tracker.complete_scan(1, true, &two_recovered, EngineMode::Normal, t0));
        tracker.begin_scan(2, &two_recovered);
        assert!(tracker.complete_scan(
            2,
            true,
            &two_recovered,
            EngineMode::Normal,
            t0 + Duration::from_secs(10)
        ));

        // 连续只点 2 次添加键盘（无候选）
        let mut tracker = CompatibleHintTracker::default();
        tracker.begin_keyboard_pick(1);
        assert!(!tracker.complete_keyboard_pick(1, true, false, EngineMode::Normal, t0));
        tracker.begin_keyboard_pick(2);
        assert!(tracker.complete_keyboard_pick(
            2,
            true,
            false,
            EngineMode::Normal,
            t0 + Duration::from_secs(10)
        ));

        // 连续添加键盘：有候选但用户取消未绑定算 1 次，再次点击无候选或再次取消即触发
        let mut tracker = CompatibleHintTracker::default();
        tracker.begin_keyboard_pick(1);
        assert!(!tracker.complete_keyboard_pick(1, true, true, EngineMode::Normal, t0));
        assert!(
            !tracker
                .finish_cancelled_keyboard_pick(EngineMode::Normal, t0 + Duration::from_secs(2))
        );
        tracker.begin_keyboard_pick(2);
        assert!(!tracker.complete_keyboard_pick(
            2,
            true,
            true,
            EngineMode::Normal,
            t0 + Duration::from_secs(4)
        ));
        assert!(
            tracker.finish_cancelled_keyboard_pick(EngineMode::Normal, t0 + Duration::from_secs(6))
        );

        // 扫描 <-> 添加键盘 交替触发
        let mut tracker = CompatibleHintTracker::default();
        tracker.begin_scan(1, &two);
        assert!(!tracker.complete_scan(1, true, &two, EngineMode::Normal, t0));
        tracker.begin_keyboard_pick(2);
        assert!(tracker.complete_keyboard_pick(
            2,
            true,
            false,
            EngineMode::Normal,
            t0 + Duration::from_secs(5)
        ));

        let mut tracker = CompatibleHintTracker::default();
        tracker.begin_keyboard_pick(1);
        assert!(!tracker.complete_keyboard_pick(1, true, false, EngineMode::Normal, t0));
        tracker.begin_scan(2, &two);
        assert!(tracker.complete_scan(
            2,
            true,
            &two,
            EngineMode::Normal,
            t0 + Duration::from_secs(10)
        ));
    }

    #[test]
    fn compatible_hint_respects_mode_dismissal_and_time_window() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let initial = vec![make_row("mouse-1", true)];

        // 兼容模式下不触发；切回普通模式后允许重新触发
        let mut tracker = CompatibleHintTracker::default();
        tracker.begin_scan(1, &initial);
        assert!(!tracker.complete_scan(1, true, &initial, EngineMode::Compatible, t0));
        tracker.begin_keyboard_pick(2);
        assert!(!tracker.complete_keyboard_pick(
            2,
            true,
            false,
            EngineMode::Compatible,
            t0 + Duration::from_secs(5)
        ));

        // 用户关闭弹窗后在本次普通模式会话不再重复触发
        let mut tracker = CompatibleHintTracker::default();
        tracker.on_dismissed();
        tracker.begin_scan(1, &initial);
        assert!(!tracker.complete_scan(1, true, &initial, EngineMode::Normal, t0));
        tracker.begin_keyboard_pick(2);
        assert!(!tracker.complete_keyboard_pick(
            2,
            true,
            false,
            EngineMode::Normal,
            t0 + Duration::from_secs(5)
        ));
        // 切换模式后再切回普通模式解除拒绝锁定
        tracker.on_mode_changed(EngineMode::Compatible);
        tracker.begin_scan(3, &initial);
        assert!(!tracker.complete_scan(
            3,
            true,
            &initial,
            EngineMode::Normal,
            t0 + Duration::from_secs(10)
        ));
        tracker.begin_scan(4, &initial);
        assert!(tracker.complete_scan(
            4,
            true,
            &initial,
            EngineMode::Normal,
            t0 + Duration::from_secs(12)
        ));

        // 超过 60s 窗口后旧记录过期，不触发
        let mut tracker = CompatibleHintTracker::default();
        tracker.begin_scan(1, &initial);
        assert!(!tracker.complete_scan(1, true, &initial, EngineMode::Normal, t0));
        tracker.begin_keyboard_pick(2);
        assert!(!tracker.complete_keyboard_pick(
            2,
            true,
            false,
            EngineMode::Normal,
            t0 + COMPATIBLE_HINT_WINDOW + Duration::from_secs(1),
        ));
    }

    #[test]
    fn compatible_hint_resets_on_new_device_or_bind_and_ignores_duplicate_receipts() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let one = vec![make_row("mouse-1", true)];
        let two = vec![make_row("mouse-1", true), make_row("mouse-2", true)];
        let three = vec![
            make_row("mouse-1", true),
            make_row("mouse-2", true),
            make_row("kb-3", true),
        ];

        // 0 -> 1 或 2 -> 3 真正新增设备 ID 时重置计数，不误触发
        let mut tracker = CompatibleHintTracker::default();
        tracker.begin_scan(1, &[]);
        assert!(!tracker.complete_scan(1, true, &[], EngineMode::Normal, t0));
        tracker.begin_scan(2, &[]);
        assert!(!tracker.complete_scan(
            2,
            true,
            &one,
            EngineMode::Normal,
            t0 + Duration::from_secs(5)
        ));
        tracker.begin_scan(3, &two);
        assert!(!tracker.complete_scan(
            3,
            true,
            &two,
            EngineMode::Normal,
            t0 + Duration::from_secs(6)
        ));
        tracker.begin_scan(4, &two);
        assert!(!tracker.complete_scan(
            4,
            true,
            &three,
            EngineMode::Normal,
            t0 + Duration::from_secs(8)
        ));

        // 成功绑定设备后清空历史计数
        let mut tracker = CompatibleHintTracker::default();
        tracker.begin_scan(1, &one);
        assert!(!tracker.complete_scan(1, true, &one, EngineMode::Normal, t0));
        tracker.begin_keyboard_pick(2);
        assert!(!tracker.complete_keyboard_pick(
            2,
            true,
            true,
            EngineMode::Normal,
            t0 + Duration::from_secs(2)
        ));
        tracker.on_device_bound();
        tracker.begin_scan(3, &two);
        assert!(!tracker.complete_scan(
            3,
            true,
            &two,
            EngineMode::Normal,
            t0 + Duration::from_secs(5)
        ));

        // 键盘扫描失败或重复收到同一 ID 回执时不误计数
        let mut tracker = CompatibleHintTracker::default();
        tracker.begin_keyboard_pick(1);
        assert!(!tracker.complete_keyboard_pick(1, false, false, EngineMode::Normal, t0));
        tracker.begin_keyboard_pick(2);
        assert!(!tracker.complete_keyboard_pick(
            2,
            true,
            false,
            EngineMode::Normal,
            t0 + Duration::from_secs(2)
        ));
        assert!(!tracker.complete_keyboard_pick(
            2,
            true,
            false,
            EngineMode::Normal,
            t0 + Duration::from_secs(3)
        ));
    }

    #[test]
    fn settings_resize_candidate_filters_jitter_and_clamps_bounds() {
        use super::{reconcile_unconfirmed_settings_size, settings_resize_candidate};
        // 0 尺寸与 <= 1px 的 DPI 舍入抖动（或工作区钳制落地尺寸）不应触发持久化命令。
        assert_eq!(settings_resize_candidate([0, 680], [540, 680], None), None);
        assert_eq!(settings_resize_candidate([540, 0], [540, 680], None), None);
        assert_eq!(
            settings_resize_candidate([540, 680], [540, 680], None),
            None
        );
        assert_eq!(
            settings_resize_candidate([541, 679], [540, 680], None),
            None
        );
        // 即使偏好为 [1000, 800]，若工作区钳制落地基线为 [800, 700]，异步 WM_SIZE 不应覆盖偏好。
        assert_eq!(
            settings_resize_candidate([800, 700], [800, 700], None),
            None
        );

        // 超过 1px 变化且在区间内应返回目标尺寸；已在防抖序列中时保留最后 1px 调整。
        assert_eq!(
            settings_resize_candidate([542, 680], [540, 680], None),
            Some([542, 680])
        );
        assert_eq!(
            settings_resize_candidate([543, 680], [540, 680], Some([542, 680])),
            Some([543, 680])
        );
        assert_eq!(
            settings_resize_candidate([543, 680], [540, 680], Some([543, 680])),
            None
        );
        assert_eq!(
            settings_resize_candidate([380, 540], [540, 680], None),
            Some([380, 540])
        );

        // 低于最小或高于最大边界应钳制到合法范围。
        assert_eq!(
            settings_resize_candidate([200, 300], [540, 680], None),
            Some(crate::model::MIN_SETTINGS_WINDOW_SIZE)
        );
        assert_eq!(
            settings_resize_candidate([5000, 3000], [540, 680], None),
            Some(crate::model::MAX_SETTINGS_WINDOW_SIZE)
        );

        // 旧 Host 快照不应清除待确认尺寸，确认匹配或明确操作失败后才清除。
        let mut unconfirmed = Some([620, 700]);
        reconcile_unconfirmed_settings_size(&mut unconfirmed, Some([540, 680]), None);
        assert_eq!(unconfirmed, Some([620, 700]));
        reconcile_unconfirmed_settings_size(&mut unconfirmed, Some([620, 700]), None);
        assert_eq!(unconfirmed, None);

        let mut failed = Some([620, 700]);
        reconcile_unconfirmed_settings_size(
            &mut failed,
            Some([540, 680]),
            Some(&crate::model::Notice::OperationFailed),
        );
        assert_eq!(failed, None);
    }
}

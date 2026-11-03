//! Test-owned HWNDs only. No global mouse/keyboard input, no device access.
use image::{RgbImage, imageops};
use std::{
    io,
    path::Path,
    ptr, thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::Threading::*,
    UI::{HiDpi::*, WindowsAndMessaging::*},
};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub fn require(ok: bool, message: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(io::Error::other(message).into())
    }
}
pub fn pause(ms: u64) {
    thread::sleep(Duration::from_millis(ms));
}
pub fn wait<T>(seconds: u64, mut f: impl FnMut() -> Option<T>) -> Result<T> {
    let end = Instant::now() + Duration::from_secs(seconds);
    loop {
        if let Some(v) = f() {
            return Ok(v);
        }
        if Instant::now() >= end {
            return Err("test condition timed out".into());
        }
        pause(20);
    }
}
pub fn dpi_aware() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}
pub fn windows(pid: u32) -> Vec<HWND> {
    struct Find {
        pid: u32,
        found: Vec<HWND>,
    }
    unsafe extern "system" fn each(h: HWND, arg: LPARAM) -> i32 {
        let data = unsafe { &mut *(arg as *mut Find) };
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(h, &mut pid);
        }
        if pid == data.pid {
            let mut title = [0u16; 256];
            let n = unsafe { GetWindowTextW(h, title.as_mut_ptr(), 256) };
            if n > 0 && String::from_utf16_lossy(&title[..n as usize]).contains("GPUI") {
                data.found.push(h)
            }
        }
        1
    }
    let mut data = Find {
        pid,
        found: Vec::new(),
    };
    unsafe {
        EnumWindows(Some(each), (&mut data as *mut Find) as LPARAM);
    };
    data.found
}
pub fn screen(pid: u32, visible: bool) -> Result<HWND> {
    wait(8, || {
        let ws = windows(pid);
        (ws.len() == 1 && (unsafe { IsWindowVisible(ws[0]) } != 0) == visible).then(|| ws[0])
    })
}
pub fn scale(h: HWND) -> f64 {
    (unsafe { GetDpiForWindow(h).max(96) }) as f64 / 96.
}
pub fn bounds(h: HWND) -> Result<(POINT, RECT)> {
    let mut p = POINT::default();
    let mut r = RECT::default();
    require(
        unsafe { GetClientRect(h, &mut r) } != 0,
        "GetClientRect failed",
    )?;
    require(
        unsafe { ClientToScreen(h, &mut p) } != 0,
        "ClientToScreen failed",
    )?;
    Ok((p, r))
}
fn position(x: i32, y: i32) -> LPARAM {
    ((x as u16 as u32) | ((y as u16 as u32) << 16)) as LPARAM
}
pub fn post(h: HWND, msg: u32, w: usize, l: LPARAM) -> Result<()> {
    require(
        unsafe { PostMessageW(h, msg, w, l) } != 0,
        "PostMessageW failed",
    )
}
pub fn click(h: HWND, x: f64, y: f64) -> Result<()> {
    let s = scale(h);
    let p = position((x * s).round() as i32, (y * s).round() as i32);
    post(h, WM_MOUSEMOVE, 0, p)?;
    pause(35);
    post(h, WM_LBUTTONDOWN, 1, p)?;
    pause(40);
    post(h, WM_LBUTTONUP, 0, p)?;
    pause(130);
    Ok(())
}
pub fn key(h: HWND, v: usize) -> Result<()> {
    post(h, WM_KEYDOWN, v, 1)?;
    post(h, WM_KEYUP, v, 0xc0000001)?;
    pause(120);
    Ok(())
}
pub fn wheel(h: HWND, delta: i32, repeats: usize) -> Result<()> {
    let s = scale(h);
    let mut pt = POINT {
        x: (200. * s).round() as i32,
        y: (420. * s).round() as i32,
    };
    post(h, WM_MOUSEMOVE, 0, position(pt.x, pt.y))?;
    require(
        unsafe { ClientToScreen(h, &mut pt) } != 0,
        "wheel position failed",
    )?;
    for _ in 0..repeats {
        post(
            h,
            WM_MOUSEWHEEL,
            (delta as u16 as usize) << 16,
            position(pt.x, pt.y),
        )?;
        pause(65)
    }
    pause(200);
    Ok(())
}
#[link(name = "user32")]
unsafe extern "system" {
    fn PrintWindow(h: HWND, dc: HDC, flags: u32) -> i32;
}
pub fn capture(h: HWND, path: &Path) -> Result<RgbImage> {
    let (point, client) = bounds(h)?;
    let mut r = RECT::default();
    require(
        unsafe { GetWindowRect(h, &mut r) } != 0,
        "GetWindowRect failed",
    )?;
    let (w, t) = (r.right - r.left, r.bottom - r.top);
    require(
        w > 0 && t > 0 && w <= 8192 && t <= 8192,
        "invalid capture size",
    )?;
    unsafe {
        let dc = GetDC(h);
        require(!dc.is_null(), "GetDC failed")?;
        let mem = CreateCompatibleDC(dc);
        let bitmap = CreateCompatibleBitmap(dc, w, t);
        if mem.is_null() || bitmap.is_null() {
            if !bitmap.is_null() {
                DeleteObject(bitmap);
            }
            if !mem.is_null() {
                DeleteDC(mem);
            }
            ReleaseDC(h, dc);
            return Err("capture allocation failed".into());
        }
        let old = SelectObject(mem, bitmap);
        let painted = PrintWindow(h, mem, 2);
        SelectObject(mem, old);
        let mut pixels = vec![0u8; (w * t * 4) as usize];
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: 40,
                biWidth: w,
                biHeight: -t,
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let n = GetDIBits(
            dc,
            bitmap,
            0,
            t as u32,
            pixels.as_mut_ptr().cast(),
            &mut bi,
            DIB_RGB_COLORS,
        );
        DeleteObject(bitmap);
        DeleteDC(mem);
        ReleaseDC(h, dc);
        require(painted != 0 && n == t, "PrintWindow/GetDIBits failed")?;
        let rgb = pixels
            .chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0]])
            .collect();
        let full = RgbImage::from_raw(w as u32, t as u32, rgb).ok_or("invalid pixels")?;
        let (x, y) = (point.x - r.left, point.y - r.top);
        require(
            x >= 0 && y >= 0 && x + client.right <= w && y + client.bottom <= t,
            "client crop outside window",
        )?;
        let crop = imageops::crop_imm(
            &full,
            x as u32,
            y as u32,
            client.right as u32,
            client.bottom as u32,
        )
        .to_image();
        let s = scale(h);
        let img = imageops::resize(
            &crop,
            (f64::from(client.right) / s).round() as u32,
            (f64::from(client.bottom) / s).round() as u32,
            imageops::FilterType::Lanczos3,
        );
        require(
            img.pixels()
                .map(|p| p.0)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                > 10,
            "blank capture",
        )?;
        img.save(path)?;
        Ok(img)
    }
}
pub fn primary(h: HWND, path: &Path) -> Result<()> {
    let img = capture(h, path)?;
    let mut best = None;
    for y in img.height() / 2..img.height().saturating_sub(12) {
        let mut start = None;
        for x in img.width() / 2..=img.width().saturating_sub(16) {
            let [r, g, b] = img.get_pixel(x, y).0;
            let green = (7..45).contains(&r)
                && (145..206).contains(&g)
                && (96..161).contains(&b)
                && i32::from(g) - i32::from(r) > 108;
            if green {
                start.get_or_insert(x);
            } else if let Some(a) = start.take() {
                let n = x - a;
                if (38..=95).contains(&n) {
                    let v = (n, (a + x - 1) / 2, y);
                    best = Some(best.map_or(v, |b: (u32, u32, u32)| b.max(v)));
                }
            }
        }
    }
    let (_, x, y) = best.ok_or("no primary button in real window")?;
    click(h, f64::from(x), f64::from(y.saturating_sub(1)))
}
pub struct ProcessHandle {
    handle: HANDLE,
    frozen: bool,
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtSuspendProcess(h: HANDLE) -> i32;
    fn NtResumeProcess(h: HANDLE) -> i32;
}
impl ProcessHandle {
    pub fn own(pid: u32, exe: &Path) -> Result<Self> {
        let handle = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE
                    | PROCESS_QUERY_LIMITED_INFORMATION
                    | PROCESS_TERMINATE
                    | PROCESS_SUSPEND_RESUME,
                0,
                pid,
            )
        };
        require(!handle.is_null(), "OpenProcess failed")?;
        let this = Self {
            handle,
            frozen: false,
        };
        let mut buf = vec![0u16; 32768];
        let mut len = buf.len() as u32;
        require(
            unsafe { QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len) } != 0,
            "child image query failed",
        )?;
        require(
            Path::new(&String::from_utf16_lossy(&buf[..len as usize])).canonicalize()?
                == exe.canonicalize()?,
            "child image does not match test executable",
        )?;
        Ok(this)
    }
    pub fn exited(&self) -> bool {
        (unsafe { WaitForSingleObject(self.handle, 0) }) == WAIT_OBJECT_0
    }
    pub fn freeze(&mut self) -> Result<()> {
        require(
            unsafe { NtSuspendProcess(self.handle) } == 0,
            "child suspend failed",
        )?;
        self.frozen = true;
        Ok(())
    }
    pub fn terminate(&self) -> Result<()> {
        require(
            unsafe { TerminateProcess(self.handle, 71) } != 0,
            "child termination failed",
        )?;
        require(
            unsafe { WaitForSingleObject(self.handle, 5000) } == WAIT_OBJECT_0,
            "child did not terminate",
        )
    }
}
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            if self.frozen && !self.exited() {
                NtResumeProcess(self.handle);
            }
            CloseHandle(self.handle);
        }
    }
}

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmFlush() -> i32;
    fn DwmGetWindowAttribute(h: HWND, attr: u32, value: *mut std::ffi::c_void, bytes: u32) -> i32;
}
pub struct Board {
    hwnd: HWND,
    pub point: POINT,
    pub rect: RECT,
    witness: POINT,
    app: HWND,
}
impl Board {
    pub fn new(app: HWND) -> Result<Self> {
        let (p, r) = bounds(app)?;
        let hwnd = unsafe {
            CreateWindowExW(
                0x08000088,
                wide("BUTTON").as_ptr(),
                wide("Battery monitor test backdrop").as_ptr(),
                WS_POPUP,
                p.x - 40,
                p.y - 40,
                r.right + 80,
                r.bottom + 80,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        require(!hwnd.is_null(), "backplate creation failed")?;
        let mut board = Self {
            hwnd,
            point: p,
            rect: r,
            witness: POINT::default(),
            app,
        };
        unsafe {
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                p.x - 40,
                p.y - 40,
                r.right + 80,
                r.bottom + 80,
                0x50,
            );
            SetWindowPos(app, HWND_TOPMOST, 0, 0, 0, 0, 0x13);
            UpdateWindow(hwnd);
        }
        pause(250);
        board.witness = [
            (-20, r.bottom / 2),
            (r.right + 20, r.bottom / 2),
            (r.right / 2, -20),
            (r.right / 2, r.bottom + 20),
        ]
        .into_iter()
        .map(|(x, y)| POINT {
            x: p.x + x,
            y: p.y + y,
        })
        .find(|&p| unsafe { WindowFromPoint(p) } == hwnd)
        .ok_or("no unobscured backdrop witness")?;
        Ok(board)
    }
    pub fn material(&self) -> (i32, i32) {
        let mut value = 0i32;
        let rc = unsafe { DwmGetWindowAttribute(self.app, 38, (&mut value as *mut i32).cast(), 4) };
        (rc, value)
    }
    fn paint(&self, stripes: bool, color: u32) -> Result<()> {
        unsafe {
            let dc = GetDC(self.hwnd);
            require(!dc.is_null(), "backplate DC failed")?;
            let width = self.rect.right + 80;
            let height = self.rect.bottom + 80;
            let colors = if stripes {
                [0x2323dd, 0xdd2323]
            } else {
                [color, color]
            };
            let brushes = colors.map(|c| CreateSolidBrush(c));
            if brushes.iter().any(|b| b.is_null()) {
                for b in brushes {
                    if !b.is_null() {
                        DeleteObject(b);
                    }
                }
                ReleaseDC(self.hwnd, dc);
                return Err("brush creation failed".into());
            }
            let step = if stripes { 4 } else { width };
            for x in (0..width).step_by(step as usize) {
                FillRect(
                    dc,
                    &RECT {
                        left: x,
                        top: 0,
                        right: (x + step).min(width),
                        bottom: height,
                    },
                    brushes[(x / step % 2) as usize],
                );
            }
            GdiFlush();
            ReleaseDC(self.hwnd, dc);
            for b in brushes {
                DeleteObject(b);
            }
            DwmFlush();
        }
        pause(350);
        Ok(())
    }
    pub fn samples(&self, color: u32) -> Result<[f64; 3]> {
        self.paint(false, color)?;
        let dc = unsafe { GetDC(ptr::null_mut()) };
        require(!dc.is_null(), "screen sample DC failed")?;
        let out = (|| {
            let witness = unsafe { GetPixel(dc, self.witness.x, self.witness.y) };
            let diff = (witness & 255) as i32 - ((witness >> 16) & 255) as i32;
            require(
                diff * if color & 255 > 128 { 1 } else { -1 } > 80,
                "test backplate not visible",
            )?;
            let mut channels: [Vec<f64>; 3] = Default::default();
            for y in [0.82, 0.84, 0.86] {
                for x in [0.2, 0.4, 0.6, 0.8] {
                    let p = POINT {
                        x: self.point.x + (f64::from(self.rect.right) * x) as i32,
                        y: self.point.y + (f64::from(self.rect.bottom) * y) as i32,
                    };
                    require(
                        unsafe { WindowFromPoint(p) } == self.app,
                        "test window obscured",
                    )?;
                    let v = unsafe { GetPixel(dc, p.x, p.y) };
                    require(v != u32::MAX, "sample failed")?;
                    for (i, c) in channels.iter_mut().enumerate() {
                        c.push(f64::from((v >> (i * 8)) & 255));
                    }
                }
            }
            Ok(std::array::from_fn(|i| {
                channels[i].sort_by(f64::total_cmp);
                let n = channels[i].len();
                (channels[i][n / 2 - 1] + channels[i][n / 2]) / 2.
            }))
        })();
        unsafe {
            ReleaseDC(ptr::null_mut(), dc);
        }
        out
    }
    pub fn stripes(&self) -> Result<f64> {
        self.paint(true, 0)?;
        pause(100);
        let dc = unsafe { GetDC(ptr::null_mut()) };
        require(!dc.is_null(), "screen DC failed")?;
        let result = (|| {
            let mut samples: [Vec<f64>; 2] = Default::default();
            for x in self.rect.right / 4..self.rect.right * 3 / 4 {
                let p = POINT {
                    x: self.point.x + x,
                    y: self.point.y + self.rect.bottom * 85 / 100,
                };
                require(
                    unsafe { WindowFromPoint(p) } == self.app,
                    "test window obscured",
                )?;
                let v = unsafe { GetPixel(dc, p.x, p.y) };
                require(v != u32::MAX, "sample failed")?;
                samples[((x + 40) / 4 % 2) as usize]
                    .push(f64::from(v & 255) - f64::from((v >> 16) & 255));
            }
            let means = samples.map(|s| s.iter().sum::<f64>() / s.len() as f64);
            Ok((means[0] - means[1]).abs())
        })();
        unsafe {
            ReleaseDC(ptr::null_mut(), dc);
        }
        result
    }
}
impl Drop for Board {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}

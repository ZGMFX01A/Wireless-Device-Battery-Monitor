//! Bounded battery-state art from the original renderer, not a fallback bitmap font.
//! Decode only the selected compressed image. The other states stay file-backed.
use crate::model::Style;
use tray_icon::Icon;

use windows_sys::Win32::{
    Foundation::POINT,
    Graphics::Gdi::{MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint},
    System::Registry::*,
    UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
};
const FILE: &[u8] = include_bytes!("../assets/tray.pages");
const SIZES: [u32; 8] = [32, 40, 48, 56, 64, 80, 96, 128];
const N: usize = 16 * 8 * 7;
fn decode(
    value: Option<u8>,
    charging: bool,
    style: &Style,
    dark: bool,
    desired: u32,
) -> Result<(Vec<u8>, u32), String> {
    let pct = match value {
        None => 0,
        Some(p @ 0..=100) => p as usize + 1,
        _ => return Err("invalid percentage".into()),
    };
    let s = match style {
        Style::Capsule => 0,
        Style::Vertical => 1,
        Style::Number => 2,
        Style::Circle => 3,
    };
    let (idx, &n) = SIZES
        .iter()
        .enumerate()
        .min_by_key(|(_, n)| n.abs_diff(desired))
        .unwrap();
    let group = s * 4 + dark as usize * 2 + charging as usize;
    let slot = (group * 8 + idx) * 7 + pct / 16;
    let u = |i: usize| u32::from_le_bytes(FILE[i..i + 4].try_into().unwrap()) as usize;
    let off = N * 8 + u(slot * 8);
    let data = FILE
        .get(off..off + u(slot * 8 + 4))
        .ok_or("invalid tray page")?;
    let img = image::load_from_memory_with_format(data, image::ImageFormat::WebP)
        .map_err(|e| e.to_string())?
        .into_rgba8();
    if img.dimensions() != (n * 4, n * 4) {
        return Err("invalid tray page size".into());
    }
    let col = (pct % 4) as u32;
    let row = ((pct % 16) / 4) as u32;
    let rgba = image::imageops::crop_imm(&img, col * n, row * n, n, n)
        .to_image()
        .into_raw();
    Ok((rgba, n))
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Appearance {
    pub dark: bool,
    pub size: u32,
}
fn dark_taskbar(mut reg: impl FnMut(&str) -> Option<u32>) -> bool {
    reg("SystemUsesLightTheme")
        .or_else(|| reg("AppsUseLightTheme"))
        .is_none_or(|v| v == 0)
}
pub fn appearance() -> Appearance {
    let key: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"
        .encode_utf16()
        .chain([0])
        .collect();
    let dark = dark_taskbar(|keyname| {
        let name: Vec<u16> = keyname.encode_utf16().chain([0]).collect();
        let mut value = 0u32;
        let mut len = 4;
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut value as *mut u32).cast(),
                &mut len,
            )
        };
        (rc == 0 && len == 4).then_some(value)
    });
    let (mut dx, mut dy) = (96, 96);
    unsafe {
        GetDpiForMonitor(
            MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY),
            MDT_EFFECTIVE_DPI,
            &mut dx,
            &mut dy,
        );
    };
    Appearance {
        dark,
        size: (dx * 32 + 48) / 96,
    }
}
pub fn draw(
    value: Option<u8>,
    charging: bool,
    style: &Style,
    a: Appearance,
) -> Result<Icon, String> {
    let (buf, n) = decode(value, charging, style, a.dark, a.size)?;
    Icon::from_rgba(buf, n, n).map_err(|e| e.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_entries_valid() {
        assert!(FILE.len() > N * 8);
        for i in 0..N {
            let o = u32::from_le_bytes(FILE[i * 8..i * 8 + 4].try_into().unwrap()) as usize;
            let n = u32::from_le_bytes(FILE[i * 8 + 4..i * 8 + 8].try_into().unwrap()) as usize;
            assert!(n > 0 && N * 8 + o + n <= FILE.len())
        }
        for s in [
            Style::Capsule,
            Style::Vertical,
            Style::Number,
            Style::Circle,
        ] {
            for d in [true, false] {
                for c in [true, false] {
                    for v in [None, Some(0), Some(19), Some(20), Some(50), Some(100)] {
                        for n in SIZES {
                            let (px, w) = decode(v, c, &s, d, n).unwrap();
                            assert_eq!(w, n);
                            assert!(px.chunks_exact(4).any(|v| v[3] > 0));
                        }
                    }
                }
            }
        }
        assert!(decode(Some(101), false, &Style::Number, true, 32).is_err())
    }
}

#[cfg(test)]
mod audit_theme {
    use super::*;
    #[test]
    fn system_over_apps_over_dark_default() {
        for (system, apps, expect) in [
            (Some(1), Some(0), false),
            (Some(0), Some(1), true),
            (None, Some(1), false),
            (None, Some(0), true),
            (None, None, true),
        ] {
            assert_eq!(
                dark_taskbar(|k| if k == "SystemUsesLightTheme" {
                    system
                } else {
                    apps
                }),
                expect
            );
        }
    }
}

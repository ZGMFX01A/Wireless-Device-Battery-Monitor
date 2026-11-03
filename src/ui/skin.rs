use gpui::{prelude::*, *};
pub const GREEN: u32 = 0x10b981;
pub const CYAN: u32 = 0x06b6d4;
pub const RED: u32 = 0xef4444;
#[derive(Copy, Clone)]
pub struct Skin {
    pub dark: bool,
    pub glass: u8,
}
impl Skin {
    pub fn text(self) -> Hsla {
        rgb(if self.dark { 0xffffff } else { 0x0f172a }).into()
    }
    pub fn sub(self) -> Hsla {
        rgb(if self.dark { 0xd8e2ec } else { 0x475569 }).into()
    }
    pub fn dim(self) -> Hsla {
        rgb(if self.dark { 0xa5b5c7 } else { 0x64748b }).into()
    }
    pub fn line(self) -> Hsla {
        rgba(if self.dark { 0xffffff17 } else { 0x0000000f }).into()
    }
    // Content surfaces sit ABOVE the window's native acrylic. No extra blur, texture, or timer per card.
    pub fn card_surface(self, hover: bool) -> Background {
        let (start, end) = match (self.dark, hover) {
            (false, false) => (0xfdfefdf7, 0xf7f9faeb),
            (false, true) => (0xfffffffa, 0xfafcfbf3),
            (true, false) => (0x3e454def, 0x333940ec),
            (true, true) => (0x454c55f2, 0x393f47ef),
        };
        gradient(start, end)
    }
    pub fn card_edge(self) -> Hsla {
        rgba(if self.dark { 0xcbd8eb20 } else { 0x34455813 }).into()
    }
    pub fn card_shadows(self) -> Vec<BoxShadow> {
        // A short contact shadow and a wide, negative-spread ambient shadow; neither outlines the card.
        [
            (
                1.,
                2.,
                -0.5,
                if self.dark { 0x070b122b } else { 0x23374b10 },
            ),
            (
                3.,
                10.,
                -2.,
                if self.dark { 0x05080d28 } else { 0x23374b14 },
            ),
        ]
        .into_iter()
        .map(|(dy, blur, spread, color)| BoxShadow {
            color: rgba(color).into(),
            offset: point(px(0.), px(dy)),
            blur_radius: px(blur),
            spread_radius: px(spread),
        })
        .collect()
    }
    pub fn utility(self, hover: bool) -> Hsla {
        rgba(match (self.dark, hover) {
            (false, false) => 0xf8fbfbd4,
            (false, true) => 0xffffffef,
            (true, false) => 0x39414abc,
            (true, true) => 0x49525eda,
        })
        .into()
    }
    pub fn tool_icon(self) -> Hsla {
        rgb(if self.dark { 0x9bc7ba } else { 0x356c61 }).into()
    }
    pub fn card_secondary(self) -> Hsla {
        rgb(if self.dark { 0xbac4cd } else { 0x566474 }).into()
    }
    pub fn meter_rail(self) -> Hsla {
        rgba(if self.dark { 0xc3cdd822 } else { 0x475b701b }).into()
    }
    pub fn control(self) -> Hsla {
        rgba(if self.dark { 0xffffff14 } else { 0xe6ecf5bf }).into()
    }
    pub fn selected(self) -> Hsla {
        rgba(if self.dark { 0xffffff29 } else { 0xffffffff }).into()
    }
    pub fn grad(self, hover: bool) -> Background {
        let (a, b) = match (self.dark, hover) {
            (true, false) => (0xffffff14, 0xffffff09),
            (true, true) => (0xffffff1f, 0xffffff0f),
            (false, false) => (0xffffffe0, 0xffffffad),
            (false, true) => (0xfffffff5, 0xffffffd1),
        };
        gradient(a, b)
    }
    pub fn window(self) -> Background {
        let t = 1. - self.glass.min(100) as f32 / 100.;
        let (a, b, x, y) = if self.dark {
            (0x20263400, 0x12162000, 0.84, 0.88)
        } else {
            (0xfafcff00, 0xeef3fa00, 0.82, 0.75)
        };
        gradient(a | (255. * t * x) as u32, b | (255. * t * y) as u32)
    }
    pub fn dialog(self) -> Hsla {
        rgb(if self.dark { 0x202634 } else { 0xf5f8fc }).into()
    }
}
pub fn gradient(a: u32, b: u32) -> Background {
    linear_gradient(
        145.,
        linear_color_stop(rgba(a), 0.),
        linear_color_stop(rgba(b), 1.),
    )
}
pub fn row() -> Div {
    div().flex().items_center()
}
pub fn col() -> Div {
    div().flex().flex_col()
}
pub fn txt(t: impl Into<SharedString>, sz: f32, c: Hsla) -> Div {
    div()
        .text_size(px(sz))
        .line_height(px(sz * 1.4))
        .text_color(c)
        .child(t.into())
}
pub fn icon(name: &'static str, sz: f32, c: Hsla) -> Svg {
    svg().path(name).size(px(sz)).flex_none().text_color(c)
}
pub fn dot(sz: f32, c: u32) -> Div {
    div().size(px(sz)).flex_none().rounded_full().bg(rgb(c))
}
pub fn action(id: impl Into<SharedString>) -> Stateful<Div> {
    row()
        .id(id.into())
        .justify_center()
        .cursor_pointer()
        .flex_none()
}
/// The number is ink; the fill is a signal. Never use a bright meter color for small text on light glass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatteryTone {
    Offline,
    Charging,
    Normal,
    Warning,
    Critical,
}
impl BatteryTone {
    pub fn for_sample(online: bool, charging: bool, p: u8) -> Self {
        if !online {
            Self::Offline
        } else if charging {
            Self::Charging
        } else if p >= 50 {
            Self::Normal
        } else if p >= 20 {
            Self::Warning
        } else {
            Self::Critical
        }
    }
    pub fn palette(self, dark: bool) -> (u32, u32, u32) {
        match (self, dark) {
            (Self::Normal, false) => (0x08775e, 0x169976, 0x25ae88),
            (Self::Normal, true) => (0x65d7b1, 0x36ab8b, 0x5bc9a5),
            (Self::Charging, false) => (0x087088, 0x1193ac, 0x27aac1),
            (Self::Charging, true) => (0x6dd2e3, 0x39aac1, 0x66c9d9),
            (Self::Warning, false) => (0x965706, 0xb67716, 0xcea03d),
            (Self::Warning, true) => (0xefd091, 0xc79542, 0xdeb466),
            (Self::Critical, false) => (0xb53342, 0xcd5161, 0xe16c72),
            (Self::Critical, true) => (0xff9ba2, 0xd26c77, 0xed8892),
            (Self::Offline, false) => (0x586575, 0x6b7988, 0x6b7988),
            (Self::Offline, true) => (0xb0bbc6, 0x8895a4, 0x8895a4),
        }
    }
}
/// Use a system font, with tabular figures only for changing numeric values.
pub fn numeric_font() -> Font {
    static FEATURES: std::sync::OnceLock<FontFeatures> = std::sync::OnceLock::new();
    let mut f = font("Segoe UI");
    f.features = FEATURES
        .get_or_init(|| FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)])))
        .clone();
    f
}

#[cfg(test)]
mod material_tests {
    use super::BatteryTone;
    #[test]
    fn tone_preserves_charge_offline_and_threshold_precedence() {
        use BatteryTone::*;
        for (pct, want) in [
            (0, Critical),
            (19, Critical),
            (20, Warning),
            (49, Warning),
            (50, Normal),
            (100, Normal),
        ] {
            assert_eq!(BatteryTone::for_sample(true, false, pct), want);
            assert_eq!(BatteryTone::for_sample(true, true, pct), Charging);
            assert_eq!(BatteryTone::for_sample(false, true, pct), Offline);
            assert_eq!(BatteryTone::for_sample(false, false, pct), Offline);
        }
    }
    fn luminance(rgb: u32) -> f64 {
        let linear = |v: u32| {
            let c = (v & 255) as f64 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(rgb >> 16) + 0.7152 * linear(rgb >> 8) + 0.0722 * linear(rgb)
    }
    #[test]
    fn ink_is_readable_on_both_reference_surfaces() {
        use BatteryTone::*;
        for (dark, bg) in [(false, 0xebedf0), (true, 0x404750)] {
            for tone in [Offline, Charging, Normal, Warning, Critical] {
                let (ink, _, _) = tone.palette(dark);
                let a = luminance(ink);
                let b = luminance(bg);
                assert!(
                    (a.max(b) + 0.05) / (a.min(b) + 0.05) >= 4.5,
                    "{dark}: {tone:?}"
                );
            }
        }
    }
}

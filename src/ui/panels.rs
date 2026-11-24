// SPDX-FileCopyrightText: 2026 ZGMFX01A
// SPDX-License-Identifier: LicenseRef-Wireless-Device-Battery-Monitor
// See LICENSE in the repository root for the full terms.

//! Native layout mirrors web_ui/{theme-layout,devices,settings}.css in logical pixels.
use super::{skin::*, *};
use crate::model::PROBED_COMPATIBLE_ONLY_ERROR;
use battery_protocol::{Source, codec::KYSONA_SLOT_UNPAIRED};
fn time_text(sec: u64) -> Option<String> {
    if sec == 0 {
        return None;
    }
    let time = std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(sec))?;
    let t: chrono::DateTime<chrono::Local> = time.into();
    Some(t.format("%H:%M:%S").to_string())
}
impl Panel {
    pub(super) fn skin(&self) -> Skin {
        Skin {
            dark: match self.state.prefs.theme {
                Theme::Auto => self.dark_auto,
                Theme::Dark => true,
                Theme::Light => false,
            },
            glass: self.state.prefs.glass,
        }
    }
    /// 关闭当前弹窗并释放其输入框订阅。
    pub(super) fn close_modal(&mut self, cx: &mut Context<Self>) {
        let mut trigger_compatible_hint = false;
        match self.modal.take() {
            Some(Dialog::SuggestCompatibleMode) => self.compat_hint.on_dismissed(),
            Some(Dialog::WhatsNew) => self.dismiss_whats_new(cx),
            Some(Dialog::Pick {
                kind: Kind::Keyboard,
                ..
            }) => {
                trigger_compatible_hint = self.compat_hint.finish_cancelled_keyboard_pick(
                    self.state.prefs.engine_mode,
                    std::time::Instant::now(),
                );
            }
            _ => {}
        }
        self.modal_subs.clear();
        if trigger_compatible_hint {
            self.maybe_open_compatible_hint(cx);
        }
        cx.notify();
    }

    /// 打开统一设备编辑弹窗，支持同时修改设备名称、内置图标与图标 RGB 颜色。
    fn open_edit_dialog(
        &mut self,
        device: battery_protocol::Descriptor,
        display_name: String,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::device_display::{DeviceIcon, device_icon_in_mode, format_rgb_color};
        self.modal = None;
        self.modal_subs.clear();
        let had_alias = self.state.prefs.aliases.contains_key(&device.id);
        let appearance = self
            .state
            .prefs
            .device_appearance(&device)
            .cloned()
            .unwrap_or_default();
        let auto_icon = device_icon_in_mode(&device, self.state.prefs.engine_mode).unwrap_or(
            match device.kind {
                Kind::Mouse => DeviceIcon::Mouse,
                Kind::Keyboard => DeviceIcon::Keyboard,
                Kind::Audio => DeviceIcon::Headset,
                Kind::Gamepad => DeviceIcon::Gamepad,
                Kind::Bluetooth => DeviceIcon::Bluetooth,
            },
        );
        let name_input = cx.new(|cx| {
            crate::entry::TextInput::with_placeholder(
                display_name.clone(),
                self.t("device.edit.name_placeholder"),
                true,
                cx,
            )
        });
        let color_value = appearance.color.map(format_rgb_color).unwrap_or_default();
        let color_input = cx.new(|cx| {
            crate::entry::TextInput::with_placeholder(
                color_value,
                self.t("device.edit.color_placeholder"),
                false,
                cx,
            )
        });
        let name_focus = name_input.read(cx).focus_handle(cx);
        self.modal_subs = vec![
            cx.observe(&name_input, |_, _, cx| cx.notify()),
            cx.observe(&color_input, |_, _, cx| cx.notify()),
        ];
        self.modal = Some(Dialog::EditDevice {
            id: device.id,
            initial_name: display_name,
            had_alias,
            auto_icon,
            icon: appearance.icon,
            name_input,
            color_input,
        });
        w.focus(&name_focus);
        cx.notify();
    }

    /// 校验并保存编辑弹窗中的设备别名、图标与 RGB 颜色。
    pub(super) fn submit_edit_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::EditDevice {
            id,
            initial_name,
            had_alias,
            icon,
            name_input,
            color_input,
            ..
        }) = self.modal.clone()
        else {
            return;
        };
        let trimmed_name = name_input.read(cx).content.trim().to_owned();
        if trimmed_name.encode_utf16().count() > 32 {
            return;
        }
        let Ok(color) =
            crate::device_display::parse_rgb_color(color_input.read(cx).content.as_ref())
        else {
            return;
        };
        let alias = if trimmed_name.is_empty() {
            Some(String::new())
        } else if !had_alias && trimmed_name == initial_name {
            None
        } else {
            Some(trimmed_name)
        };
        self.issue(
            Command::UpdateDeviceDisplay {
                id,
                alias,
                icon,
                color,
            },
            cx,
        );
        self.close_modal(cx);
    }
    fn header(&self, k: Skin, cx: &mut Context<Self>) -> Div {
        let icon_btn = |id: &'static str, glyph: &'static str, col: Hsla| {
            action(id)
                .size(px(30.))
                .rounded(px(8.))
                .hover(move |d| {
                    d.bg(if glyph == "close" {
                        rgba(0xef44442e).into()
                    } else {
                        k.control()
                    })
                })
                .child(if glyph == "refresh" && self.manual.is_some() {
                    icon(glyph, 16., col)
                        .with_animation(
                            "busy-spin",
                            Animation::new(std::time::Duration::from_millis(800)).repeat(),
                            |el, progress| {
                                el.with_transformation(Transformation::rotate(percentage(progress)))
                            },
                        )
                        .into_any_element()
                } else {
                    icon(glyph, 16., col).into_any_element()
                })
        };
        row()
            .h(px(47.))
            .flex_none()
            .px(px(15.))
            .justify_between()
            .border_b_1()
            .border_color(k.line())
            .child(
                row()
                    .flex_1()
                    .h_full()
                    .gap(px(8.))
                    .child(dot(8., GREEN).shadow(vec![BoxShadow {
                        color: rgba(0x10b981ba).into(),
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(8.),
                        spread_radius: px(0.),
                    }]))
                    .child(
                        txt(self.t("app.header_title"), 13., k.text())
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        txt(concat!("v", env!("APP_VERSION")), 11., k.dim())
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(k.line())
                            .bg(k.grad(false)),
                    )
                    .on_mouse_down(Button::Left, |_, w, _| w.start_window_move()),
            )
            .child(
                row()
                    .gap(px(4.))
                    .child(
                        icon_btn("refresh", "refresh", k.sub())
                            .on_click(cx.listener(|s, _, _, cx| s.refresh(false, cx))),
                    )
                    .child(
                        icon_btn(
                            "settings",
                            "settings",
                            if self.settings_open {
                                rgb(GREEN).into()
                            } else {
                                k.sub()
                            },
                        )
                        .on_click(cx.listener(|s, _, _, cx| {
                            s.settings_open = !s.settings_open;
                            s.popup = false;
                            s.close_modal(cx);
                        })),
                    )
                    .child(
                        icon_btn("close", "close", k.sub())
                            .on_click(cx.listener(|s, _, w, cx| s.hide(w, cx))),
                    ),
            )
    }
    fn device_button(
        &self,
        id: &'static str,
        key: &str,
        glyph: &'static str,
        k: Skin,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        action(id)
            .gap(px(5.))
            .px(px(10.))
            .h(px(30.))
            .rounded(px(14.))
            .border_1()
            .border_color(k.card_edge())
            .bg(k.utility(false))
            .hover(move |d| d.bg(k.utility(true)))
            .active(move |d| d.bg(k.control()))
            .child(icon(glyph, 14., k.tool_icon()))
            .child(txt(self.t(key), 11., k.sub()).font_weight(FontWeight::MEDIUM))
            .on_click(cx.listener(move |s, _, _, cx| match glyph {
                "keyboard" => s.pick(Kind::Keyboard, cx),
                "bluetooth" => s.pick(Kind::Bluetooth, cx),
                _ => s.refresh(true, cx),
            }))
    }
    fn devices(&self, k: Skin, cx: &mut Context<Self>) -> Div {
        let mut list = col().w_full().gap(px(12.)).child(
            row()
                .id("device-actions")
                .flex_wrap()
                .gap(px(6.))
                .child(self.device_button("scan", "action.scan", "scan", k, cx))
                .child(self.device_button("add-keyboard", "action.add_keyboard", "keyboard", k, cx))
                .child(self.device_button("add-bt", "action.add_bluetooth", "bluetooth", k, cx)),
        );
        if self.state.rows.is_empty() {
            return list.child(
                col()
                    .items_center()
                    .py(px(40.))
                    .px(px(16.))
                    .gap(px(8.))
                    .child(icon("empty", 40., k.dim()))
                    .child(
                        txt(self.t("tray.menu.no_device"), 14., k.text())
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(txt(self.t("device.offline_no_data"), 12., k.dim()).text_center())
                    .child(
                        row()
                            .gap(px(8.))
                            .mt(px(8.))
                            .child(
                                action("empty-scan")
                                    .bg(rgb(GREEN))
                                    .rounded(px(8.))
                                    .px(px(14.))
                                    .h(px(30.))
                                    .child(txt(self.t("action.scan"), 12., gpui::white()))
                                    .on_click(cx.listener(|s, _, _, cx| s.refresh(true, cx))),
                            )
                            .child(self.device_button(
                                "empty-bt",
                                "action.add_bluetooth",
                                "bluetooth",
                                k,
                                cx,
                            )),
                    ),
            );
        }
        let mut rows: Vec<_> = self.state.rows.iter().collect();
        rows.sort_by_key(|r| match r.device.kind {
            Kind::Mouse => 0,
            Kind::Keyboard => 1,
            Kind::Bluetooth | Kind::Audio | Kind::Gamepad => 2,
        });
        for r in rows {
            list = list.child(self.card(r, k, cx))
        }
        list
    }
    fn card(&self, r: &Row, k: Skin, cx: &mut Context<Self>) -> Stateful<Div> {
        let d = &r.device;
        let id = d.id.clone();
        let name = self.state.row_name(r);
        let b = r.battery;
        let online = b.is_some() && r.error.is_none();
        let show_percent = (online || r.held) && b.is_some_and(|b| b.percentage_known);
        let charge =
            online && b.is_some_and(|x| matches!(x.charge, Charge::Charging | Charge::Full));
        let pct = b.map_or(0, |x| x.display);
        let (ink, fill_start, fill_end) =
            BatteryTone::for_sample(online, charge, pct).palette(k.dark);
        let src = b.map(|b| b.source);
        let (brand, bg, fg) =
            if matches!(src, Some(Source::Hidpp(_))) || id.starts_with("hid:046d:") {
                (
                    self.t("brand.logitech"),
                    0x3b82f6,
                    if k.dark { 0x9ac8ff } else { 0x285c99 },
                )
            } else if src == Some(Source::Razer) || id.starts_with("hid:1532:") {
                (
                    self.t("brand.razer"),
                    0x22c55e,
                    if k.dark { 0x85dcb4 } else { 0x266b48 },
                )
            } else if src == Some(Source::Rog) || id.starts_with("hid:0b05:") {
                ("ROG", 0xff4d67, if k.dark { 0xffb0b8 } else { 0x9f3c4a })
            } else if crate::device_display::is_lamzu(d) {
                ("LAMZU", 0x5b91ff, if k.dark { 0xb4cbff } else { 0x365cac })
            } else if crate::device_display::is_hyperx(d, src) {
                ("HyperX", 0xff4d67, if k.dark { 0xffb0b8 } else { 0x9f3c4a })
            } else if crate::device_display::is_turtle_beach(d, src) {
                (
                    "Turtle Beach",
                    0x10b981,
                    if k.dark { 0x6ee7b7 } else { 0x047857 },
                )
            } else if crate::device_display::is_scyrox(d, src) {
                ("Scyrox", 0x06b6d4, if k.dark { 0x67e8f9 } else { 0x0e7490 })
            } else if d.kind == Kind::Mouse && id.starts_with("hid:1d57:fa60:") {
                ("Ardor", 0xf59e0b, if k.dark { 0xfcd34d } else { 0x92600a })
            } else if d.kind == Kind::Bluetooth {
                (
                    self.t("device.bluetooth_short"),
                    0x94a3b8,
                    if k.dark { 0xd8e2ec } else { 0x475569 },
                )
            } else {
                (
                    if d.kind == Kind::Mouse {
                        self.t("device.mouse_short")
                    } else if d.kind == Kind::Gamepad {
                        self.t("device.gamepad")
                    } else if d.kind == Kind::Audio {
                        self.t("device.audio_short")
                    } else {
                        self.t("device.keyboard_short")
                    },
                    0x94a3b8,
                    if k.dark { 0xd8e2ec } else { 0x475569 },
                )
            };
        let st = if r.error.as_deref() == Some(KYSONA_SLOT_UNPAIRED) {
            self.t("device.unpaired")
        } else if r.error.as_deref() == Some(PROBED_COMPATIBLE_ONLY_ERROR) {
            self.t("device.compatible_only")
        } else if !online && show_percent {
            self.t("device.offline_previous")
        } else if !online {
            self.t("device.offline")
        } else if r.held {
            self.t("device.previous")
        } else if b.is_some_and(|b| b.charge == Charge::Full) {
            self.t("device.full")
        } else if charge {
            self.t("device.charging_short")
        } else {
            self.t("device.online")
        };
        let badge = txt(brand, 11., rgb(fg).into())
            .px(px(8.))
            .py(px(2.))
            .rounded(px(7.))
            .bg(rgba(bg << 8 | if k.dark { 34 } else { 23 }))
            .border_1()
            .border_color(rgba(bg << 8 | if k.dark { 22 } else { 12 }));
        let labels = row()
            .min_w_0()
            .flex_1()
            .flex_wrap()
            .gap(px(6.))
            .child(badge)
            .child(
                row()
                    .gap(px(5.))
                    .child(dot(5., fill_start))
                    .min_w_0()
                    .child(txt(st, 11., k.card_secondary()).whitespace_normal()),
            );
        let edit_device = d.clone();
        let pin_id = id.clone();
        let pinned = self.state.prefs.taskbar_pinned(&id);
        let display = name.clone();
        let mut controls = row()
            .gap(px(4.))
            .child(
                action(format!("pin-{id}"))
                    .size(px(22.))
                    .rounded(px(4.))
                    .when(pinned, |d| d.bg(rgba(0x10b98124)))
                    .hover(|e| e.bg(rgba(0x10b98128)))
                    .child(icon(
                        "pin",
                        13.,
                        if pinned { rgb(GREEN).into() } else { k.dim() },
                    ))
                    .on_click(cx.listener(move |s, _, _, cx| {
                        let was_pinned = s.state.prefs.taskbar_pinned(&pin_id);
                        s.issue(Command::ToggleTaskbarDevice(pin_id.clone()), cx);
                        s.toast(
                            Toast::Label(if was_pinned {
                                "toast.taskbar_unpinned"
                            } else {
                                "toast.taskbar_pinned"
                            }),
                            cx,
                        );
                    })),
            )
            .child(
                action(format!("edit-{id}"))
                    .size(px(22.))
                    .rounded(px(4.))
                    .hover(|e| e.bg(rgba(0x10b98128)))
                    .child(icon("edit", 13., k.dim()))
                    .on_click(cx.listener(move |s, _, w, cx| {
                        s.open_edit_dialog(edit_device.clone(), display.clone(), w, cx)
                    })),
            );
        if matches!(d.kind, Kind::Keyboard | Kind::Bluetooth) {
            let remove_id = id.clone();
            let title = name.clone();
            let kind = d.kind;
            controls = controls.child(
                action(format!("remove-{id}"))
                    .size(px(22.))
                    .rounded(px(4.))
                    .hover(|e| e.bg(rgba(0xef444426)))
                    .child(icon("close", 13., k.dim()))
                    .on_click(cx.listener(move |s, _, _, cx| {
                        s.modal_subs.clear();
                        s.modal = Some(Dialog::Remove(remove_id.clone(), kind, title.clone()));
                        cx.notify()
                    })),
            )
        }
        let title = txt(name, 15.5, k.text())
            .font_weight(FontWeight::SEMIBOLD)
            .truncate()
            .min_w_0()
            .max_w(px(200.));
        let mut numbers = row().items_baseline().gap(px(3.)).flex_none().child(
            txt(
                if show_percent {
                    pct.to_string()
                } else {
                    "--".into()
                },
                32.,
                rgb(ink).into(),
            )
            .line_height(px(32.))
            .font(numeric_font())
            .font_weight(FontWeight::SEMIBOLD),
        );
        if show_percent {
            numbers =
                numbers.child(txt("%", 13., k.card_secondary()).font_weight(FontWeight::SEMIBOLD))
        }
        if charge {
            numbers = numbers.child(
                txt(self.t("device.charging"), 11., rgb(ink).into())
                    .ml(px(6.))
                    .font_weight(FontWeight::SEMIBOLD),
            )
        }
        let time = time_text(r.sampled)
            .map(|v| self.t("device.updated_at").replace("{time}", &v))
            .unwrap_or_else(|| self.t("device.wait_update").into());
        let ty = self.t(match d.kind {
            Kind::Mouse => "device.mouse",
            Kind::Keyboard => "device.keyboard",
            Kind::Bluetooth => "device.bluetooth",
            Kind::Audio => "device.audio",
            Kind::Gamepad => "device.gamepad",
        });
        col()
            .id(SharedString::from(format!("card-{id}")))
            .relative()
            .w_full()
            .min_h(px(134.))
            .flex_none()
            .py(px(14.))
            .px(px(15.))
            .rounded(px(17.))
            .border_1()
            .border_color(k.card_edge())
            .shadow(k.card_shadows())
            .bg(k.card_surface(false))
            .hover(move |d| d.bg(k.card_surface(true)))
            .child(
                row()
                    .justify_between()
                    .mb(px(8.))
                    .child(labels)
                    .child(controls),
            )
            .child(
                row()
                    .items_baseline()
                    .justify_between()
                    .mb(px(10.))
                    .child(title)
                    .child(numbers),
            )
            .child(
                div()
                    .w_full()
                    .h(px(6.))
                    .rounded_full()
                    .bg(k.meter_rail())
                    .mb(px(8.))
                    .child(
                        div()
                            .h_full()
                            .w(relative(if show_percent { pct as f32 / 100. } else { 0. }))
                            .rounded_full()
                            .bg(gradient(fill_start << 8 | 255, fill_end << 8 | 255)),
                    ),
            )
            .child(
                row()
                    .justify_between()
                    .gap(px(8.))
                    .flex_wrap()
                    .child(txt(ty, 11., k.card_secondary()).whitespace_normal())
                    .child(txt(time, 11., k.card_secondary()).font(numeric_font())),
            )
    }
}

impl Panel {
    fn titles(&self, key: &str, k: Skin) -> Div {
        col()
            .min_w_0()
            .flex_1()
            .child(
                txt(self.t(&format!("settings.{key}.title")), 13., k.text())
                    .whitespace_normal()
                    .font_weight(FontWeight::MEDIUM),
            )
            .child(
                txt(self.t(&format!("settings.{key}.subtitle")), 12., k.sub())
                    .whitespace_normal()
                    .line_height(px(16.2)),
            )
    }
    fn group(&self, key: &str, k: Skin, stacked: bool, control: impl IntoElement) -> Div {
        div()
            .w_full()
            .min_h(px(if stacked { 101. } else { 57. }))
            .flex_none()
            .flex()
            .when(stacked, |e| e.flex_col())
            .when(!stacked, |e| e.items_center().justify_between())
            .gap(px(if stacked { 8. } else { 12. }))
            .rounded(px(12.))
            .border_1()
            .border_color(k.line())
            .bg(k.grad(false))
            .px(px(12.))
            .py(px(10.))
            .child(self.titles(key, k))
            .child(control)
    }
    fn segments(
        &self,
        id: &str,
        values: Vec<(&str, Command, bool)>,
        k: Skin,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut seg = row()
            .w_full()
            .gap(px(3.))
            .p(px(3.))
            .rounded(px(8.))
            .border_1()
            .border_color(k.line())
            .bg(k.control());
        for (i, (label, c, sel)) in values.into_iter().enumerate() {
            seg = seg.child(
                action(format!("{id}-{i}"))
                    .flex_1()
                    .min_w_0()
                    .min_h(px(28.))
                    .py(px(5.))
                    .px(px(3.))
                    .rounded(px(6.))
                    .when(sel, |d| d.bg(k.selected()).shadow_sm())
                    .hover(move |d| d.bg(k.selected()))
                    .child(
                        txt(label.to_string(), 11., if sel { k.text() } else { k.sub() })
                            .min_w_0()
                            .whitespace_normal()
                            .text_center()
                            .when(sel, |d| d.font_weight(FontWeight::SEMIBOLD)),
                    )
                    .on_click(cx.listener(move |s, _, _, cx| {
                        if let Command::SetEngineMode(mode) = c {
                            if mode == s.state.prefs.engine_mode {
                                return;
                            }
                            if mode == EngineMode::Compatible {
                                s.modal = Some(Dialog::ConfirmCompatibleMode);
                                cx.notify();
                                return;
                            }
                        }
                        s.issue(c.clone(), cx);
                    })),
            )
        }
        seg
    }
    fn step(
        &self,
        id: &str,
        value: String,
        minus: Option<Command>,
        plus: Option<Command>,
        k: Skin,
        cx: &mut Context<Self>,
    ) -> Div {
        let b = |name: &str, label: &'static str, cmd: Option<Command>| {
            let d = action(format!("{id}-{name}"))
                .w(px(26.))
                .h(px(24.))
                .rounded(px(6.))
                .child(txt(label, 14., k.text()).font_weight(FontWeight::BOLD));
            if let Some(c) = cmd {
                d.hover(move |e| e.bg(k.selected()))
                    .on_click(cx.listener(move |s, _, _, cx| s.issue(c.clone(), cx)))
            } else {
                d.opacity(0.35)
            }
        };
        row()
            .flex_none()
            .p(px(2.))
            .rounded(px(8.))
            .border_1()
            .border_color(k.line())
            .bg(k.control())
            .child(b("minus", "−", minus))
            .child(
                txt(value, 12., k.text())
                    .min_w(px(36.))
                    .text_center()
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(b("plus", "+", plus))
    }
    fn switch(
        &self,
        id: &'static str,
        on: bool,
        c: Option<Command>,
        k: Skin,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        row()
            .id(id)
            .cursor_pointer()
            .w(px(40.))
            .h(px(22.))
            .px(px(2.))
            .flex_none()
            .rounded_full()
            .border_1()
            .border_color(if on { rgb(GREEN).into() } else { k.line() })
            .bg(if on { rgb(GREEN).into() } else { k.control() })
            .when(on, |d| d.justify_end())
            .child(
                div()
                    .size(px(16.))
                    .rounded_full()
                    .bg(gpui::white())
                    .shadow_sm(),
            )
            .on_click(cx.listener(move |s, _, _, cx| {
                if matches!(c, Some(Command::SetUnknownProbe(true))) {
                    s.modal = Some(Dialog::ConfirmUnknownProbe);
                    cx.notify();
                } else if let Some(c) = &c {
                    s.issue(c.clone(), cx)
                } else {
                    s.live = !s.live;
                    if s.live
                        && let Some(rows) = s.pending_rows.take()
                    {
                        s.state.rows = rows;
                    }
                    cx.notify()
                }
            }))
    }
    fn settings(&self, k: Skin, cx: &mut Context<Self>) -> Div {
        let p = &self.state.prefs;
        let mut group =
            col().w_full().max_w(px(480.)).gap(px(12.)).child(
                txt(self.t("settings.title"), 12., k.sub()).font_weight(FontWeight::SEMIBOLD),
            );
        let language = self.segments(
            "lang",
            vec![
                (
                    self.t("settings.theme.auto"),
                    Command::SetLanguage(Language::Auto),
                    p.lang == Language::Auto,
                ),
                (
                    "简体中文",
                    Command::SetLanguage(Language::Zh),
                    p.lang == Language::Zh,
                ),
                (
                    "English",
                    Command::SetLanguage(Language::En),
                    p.lang == Language::En,
                ),
                (
                    "Русский",
                    Command::SetLanguage(Language::Ru),
                    p.lang == Language::Ru,
                ),
            ],
            k,
            cx,
        );
        group = group.child(self.group("language", k, true, language));
        let styles = [
            ("capsule", Style::Capsule),
            ("vertical", Style::Vertical),
            ("number", Style::Number),
            ("circle", Style::Circle),
        ];
        let opts = styles
            .into_iter()
            .map(|(n, v)| {
                (
                    self.t(&format!("settings.tray_style.{n}")),
                    Command::SetStyle(v.clone()),
                    p.style == v,
                )
            })
            .collect();
        group = group.child(self.group("tray_style", k, true, self.segments("style", opts, k, cx)));
        let priority = |v: &Priority| {
            self.t(match v {
                Priority::Mouse => "settings.tray_priority.mouse_first",
                Priority::Keyboard => "settings.tray_priority.keyboard_first",
                Priority::Bluetooth => "settings.tray_priority.bluetooth_first",
                Priority::Audio => "settings.tray_priority.audio_first",
                Priority::Lowest => "settings.tray_priority.lowest_battery",
            })
        };
        let mut chooser = col().relative().flex_none().child(
            action("priority")
                .gap(px(5.))
                .px(px(10.))
                .h(px(29.))
                .rounded(px(8.))
                .border_1()
                .border_color(k.line())
                .bg(k.control())
                .child(txt(priority(&p.priority), 12., k.text()))
                .child(txt("▾", 10., k.dim()))
                .on_click(cx.listener(|s, _, _, cx| {
                    s.popup = !s.popup;
                    cx.notify()
                })),
        );
        if self.popup {
            let mut items = col()
                .min_w(px(122.))
                .p(px(3.))
                .rounded(px(8.))
                .border_1()
                .border_color(k.line())
                .bg(k.dialog())
                .shadow_md();
            for (i, v) in [
                Priority::Mouse,
                Priority::Keyboard,
                Priority::Bluetooth,
                Priority::Audio,
                Priority::Lowest,
            ]
            .into_iter()
            .enumerate()
            {
                items = items.child(
                    action(format!("priority-opt-{i}"))
                        .h(px(26.))
                        .px(px(8.))
                        .whitespace_nowrap()
                        .when(p.priority == v, |e| e.bg(k.selected()))
                        .child(txt(priority(&v), 12., k.text()))
                        .on_click(cx.listener(move |s, _, _, cx| {
                            s.popup = false;
                            s.issue(Command::SetPriority(v.clone()), cx)
                        })),
                )
            }
            chooser = chooser.child(
                deferred(
                    div()
                        .absolute()
                        .right_0()
                        .top(px(30.))
                        .occlude()
                        .child(items),
                )
                .with_priority(1),
            );
        }
        group = group.child(self.group("tray_priority", k, false, chooser));
        group = group.child(self.group(
            "taskbar_position",
            k,
            true,
            self.segments(
                "taskbar-position",
                vec![
                    (
                        self.t("settings.taskbar_position.tray_left"),
                        Command::SetTaskbarPosition(TaskbarPosition::TrayLeft),
                        p.taskbar_position == TaskbarPosition::TrayLeft,
                    ),
                    (
                        self.t("settings.taskbar_position.taskbar_left"),
                        Command::SetTaskbarPosition(TaskbarPosition::TaskbarLeft),
                        p.taskbar_position == TaskbarPosition::TaskbarLeft,
                    ),
                ],
                k,
                cx,
            ),
        ));
        group = group.child(self.group(
            "taskbar_layout",
            k,
            true,
            self.segments(
                "taskbar-layout",
                vec![
                    (
                        self.t("settings.taskbar_layout.stacked"),
                        Command::SetTaskbarLayout(TaskbarLayout::Stacked),
                        p.taskbar_layout == TaskbarLayout::Stacked,
                    ),
                    (
                        self.t("settings.taskbar_layout.inline"),
                        Command::SetTaskbarLayout(TaskbarLayout::Inline),
                        p.taskbar_layout == TaskbarLayout::Inline,
                    ),
                ],
                k,
                cx,
            ),
        ));
        group = group.child(self.group(
            "taskbar_backdrop",
            k,
            false,
            self.switch(
                "taskbar-backdrop",
                p.taskbar_backdrop,
                Some(Command::SetTaskbarBackdrop(!p.taskbar_backdrop)),
                k,
                cx,
            ),
        ));
        let low = [0, 10, 20, 30];
        group = group.child(
            self.group(
                "low_battery",
                k,
                false,
                self.step(
                    "low",
                    if p.low == 0 {
                        self.t("settings.off").into()
                    } else {
                        format!("{}%", p.low)
                    },
                    low.into_iter()
                        .rev()
                        .find(|v| *v < p.low)
                        .map(Command::SetLow),
                    low.into_iter().find(|v| *v > p.low).map(Command::SetLow),
                    k,
                    cx,
                ),
            ),
        );
        for (key, on, cmd) in [
            ("auto_refresh", self.live, None),
            ("auto_update", p.auto_update, Some(Command::AutoUpdate)),
            (
                "release_notes_prompt",
                p.show_release_notes_on_update,
                Some(Command::SetShowReleaseNotesOnUpdate(
                    !p.show_release_notes_on_update,
                )),
            ),
            ("autostart", self.state.autostart, Some(Command::Autostart)),
        ] {
            group = group.child(self.group(key, k, false, self.switch(key, on, cmd, k, cx)))
        }
        group = group.child(
            self.group(
                "poll_interval",
                k,
                false,
                self.step(
                    "poll-interval",
                    format!("{} {}", p.poll_interval_secs, self.t("settings.seconds")),
                    POLL_INTERVALS
                        .into_iter()
                        .rev()
                        .find(|v| *v < p.poll_interval_secs)
                        .map(Command::SetPollInterval),
                    POLL_INTERVALS
                        .into_iter()
                        .find(|v| *v > p.poll_interval_secs)
                        .map(Command::SetPollInterval),
                    k,
                    cx,
                ),
            ),
        );
        let themes = [
            ("auto", Theme::Auto),
            ("light", Theme::Light),
            ("dark", Theme::Dark),
        ];
        group = group.child(
            self.group(
                "theme",
                k,
                true,
                self.segments(
                    "theme",
                    themes
                        .into_iter()
                        .map(|(key, v)| {
                            (
                                self.t(&format!("settings.theme.{key}")),
                                Command::SetTheme(v.clone()),
                                v == p.theme,
                            )
                        })
                        .collect(),
                    k,
                    cx,
                ),
            ),
        );
        group = group.child(self.group(
            "glass",
            k,
            false,
            self.step(
                "glass",
                format!("{}%", p.glass),
                (p.glass > 0).then_some(Command::Glass(-5)),
                (p.glass < 100).then_some(Command::Glass(5)),
                k,
                cx,
            ),
        ));
        group = group.child(self.group(
            "engine_mode",
            k,
            true,
            self.segments(
                "engine-mode",
                vec![
                    (
                        self.t("settings.engine_mode.normal"),
                        Command::SetEngineMode(EngineMode::Normal),
                        p.engine_mode == EngineMode::Normal,
                    ),
                    (
                        self.t("settings.engine_mode.compatible"),
                        Command::SetEngineMode(EngineMode::Compatible),
                        p.engine_mode == EngineMode::Compatible,
                    ),
                ],
                k,
                cx,
            ),
        ));
        if p.engine_mode == EngineMode::Compatible {
            group = group.child(self.group(
                "unknown_probe",
                k,
                false,
                self.switch(
                    "unknown-probe",
                    p.probe_unknown_devices,
                    Some(Command::SetUnknownProbe(!p.probe_unknown_devices)),
                    k,
                    cx,
                ),
            ));
        }
        let check = action("updates")
            .gap(px(6.))
            .h(px(34.))
            .px(px(16.))
            .bg(k.grad(false))
            .rounded(px(8.))
            .border_1()
            .border_color(k.line())
            .child(icon("update", 14., k.sub()))
            .child(txt(self.t("action.check_update"), 12., k.sub()))
            .on_click(cx.listener(|s, _, _, cx| {
                s.modal = Some(Dialog::Update);
                if matches!(
                    s.state.update,
                    crate::update::Status::Idle
                        | crate::update::Status::Current
                        | crate::update::Status::Cancelled
                        | crate::update::Status::Error(_)
                ) {
                    s.issue(Command::CheckUpdate, cx)
                }
                cx.notify()
            }));
        col()
            .w_full()
            .items_center()
            .child(group)
            .child(
                row()
                    .w_full()
                    .flex_wrap()
                    .justify_center()
                    .mt(px(14.))
                    .gap(px(8.))
                    .child(check)
                    .when(p.unknown_probe_enabled(), |r| {
                        r.child(
                            action("copy-probe-diagnostics")
                                .h(px(34.))
                                .px(px(14.))
                                .bg(k.grad(false))
                                .rounded(px(8.))
                                .border_1()
                                .border_color(k.line())
                                .hover(move |d| d.bg(k.selected()))
                                .child(txt(self.t("settings.copy_probe_diagnostics"), 12., k.sub()))
                                .on_click(cx.listener(|s, _, _, cx| {
                                    let text =
                                        battery_protocol::hid::probe::format_probe_diagnostics(
                                            &s.state.probe_diagnostics,
                                        );
                                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                                    s.toast(Toast::Label("toast.probe_diagnostics_copied"), cx);
                                })),
                        )
                    }),
            )
            .child(self.author_footer(k))
    }
    fn author_footer(&self, k: Skin) -> Stateful<Div> {
        action("project-home")
            .mt(px(10.))
            .h(px(30.))
            .px(px(12.))
            .gap(px(4.))
            .rounded_full()
            .hover(move |d| d.bg(k.grad(false)))
            .child(txt(self.t("about.author"), 11., k.dim()))
            .child(txt("ZGMFX01A", 11., rgb(GREEN).into()).font_weight(FontWeight::MEDIUM))
            .child(txt("·", 11., k.dim()))
            .child(txt(self.t("about.homepage"), 11., k.dim()))
            .on_click(|_, _, cx| cx.open_url(PROJECT_HOME_URL))
    }
}
impl Render for Panel {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = self.skin();
        let lang = locale(&self.state.prefs);
        if self.last_locale != Some(lang) {
            w.set_window_title(&format!("{} · GPUI", self.t("app.header_title")));
            self.last_locale = Some(lang);
        }
        if self.last_material != Some(k.dark) {
            super::material::apply(super::hwnd(w), k.dark);
            self.last_material = Some(k.dark);
        }
        let body = if self.settings_open {
            self.settings(k, cx)
        } else {
            self.devices(k, cx)
        };
        let scroll = self.scroll[usize::from(self.settings_open)].clone();
        let visible: f32 = scroll.bounds().size.height.into();
        let max: f32 = scroll.max_offset().height.into();
        let handle = scroll.clone();
        let position: f32 = (-scroll.offset().y).into();
        let thumb_h = (visible * visible / (max + visible).max(1.)).max(20.);
        let thumb_y = position / max.max(1.) * (visible - thumb_h);
        let prev = self.last_scroll;
        let met = (visible.round() as i32, max.round() as i32);
        self.last_scroll = met;
        if prev != met {
            w.on_next_frame(|_, cx| cx.refresh_windows());
        }
        let mut root = col()
            .relative()
            .size_full()
            .track_focus(&self.focus)
            .font_family("Segoe UI")
            .text_color(k.text())
            .text_size(px(13.))
            // Fill the native rounded window. A second rounded, outlined root exposes
            // the acrylic behind its corners as a separate gray frame.
            .bg(k.window())
            .overflow_hidden()
            .child(self.header(k, cx))
            .child(
                div()
                    .id(if self.settings_open {
                        "prefs-scroll"
                    } else {
                        "devices-scroll"
                    })
                    .min_h_0()
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .on_scroll_wheel(
                        cx.listener(|_, _, w, _| w.on_next_frame(|_, cx| cx.refresh_windows())),
                    )
                    .p(px(14.))
                    .pr(px(20.))
                    .child(
                        body.with_animation(
                            SharedString::from(format!(
                                "panel-{}-{}",
                                self.settings_open, self.generation
                            )),
                            Animation::new(std::time::Duration::from_millis(180))
                                .with_easing(|v| 1. - (1. - v).powi(3)),
                            |el, v| el.relative().top(px(4. * (1. - v))).opacity(v),
                        ),
                    ),
            );
        if max > 0.5 && visible > 0. {
            root = root.child(
                div()
                    .id("scroll-track")
                    .absolute()
                    .right(px(1.))
                    .top(px(48.))
                    .w(px(6.))
                    .h(px(visible))
                    .rounded_full()
                    .cursor_pointer()
                    .child(
                        div()
                            .absolute()
                            .top(px(thumb_y))
                            .w_full()
                            .h(px(thumb_h))
                            .rounded_full()
                            .bg(k.line()),
                    )
                    .on_mouse_down(
                        Button::Left,
                        cx.listener(move |s, e: &MouseDownEvent, _, cx| {
                            let y = f32::from(e.position.y) - 48.;
                            let shift = if y >= thumb_y && y <= thumb_y + thumb_h {
                                y - thumb_y
                            } else {
                                thumb_h / 2.
                            };
                            s.drag_scroll = Some(shift);
                            handle.set_offset(point(
                                px(0.),
                                px(-((y - shift) / (visible - thumb_h).max(1.)).clamp(0., 1.)
                                    * max),
                            ));
                            cx.notify()
                        }),
                    ),
            );
        }
        let mv = scroll.clone();
        root = root
            .on_mouse_move(cx.listener(move |s, e: &MouseMoveEvent, _, cx| {
                if let Some(shift) = s.drag_scroll {
                    if e.pressed_button != Some(Button::Left) {
                        s.drag_scroll = None;
                        return;
                    }
                    let v = ((f32::from(e.position.y) - 48. - shift) / (visible - thumb_h).max(1.))
                        .clamp(0., 1.);
                    mv.set_offset(point(px(0.), px(-v * max)));
                    cx.notify()
                }
            }))
            .on_mouse_up(Button::Left, cx.listener(|s, _, _, _| s.drag_scroll = None));
        if let Some(d) = self.modal.clone() {
            root = root.child(self.dialog(d, k, cx))
        }
        if let Some((msg, _)) = &self.toast {
            root = root.child(
                row()
                    .absolute()
                    .left(px(8.))
                    .right(px(8.))
                    .bottom(px(16.))
                    .justify_center()
                    .child(
                        txt(
                            msg.text(locale(&self.state.prefs)),
                            11.,
                            rgb(0xf8fafc).into(),
                        )
                        .min_w_0()
                        .whitespace_normal()
                        .text_center()
                        .px(px(14.))
                        .py(px(6.))
                        .bg(rgba(0x0f172af2))
                        .rounded(px(12.)),
                    ),
            )
        }
        root.on_key_down(cx.listener(|s, e: &KeyDownEvent, w, cx| {
            if e.keystroke.key == "escape" {
                if s.modal.is_some() || s.popup {
                    s.popup = false;
                    s.close_modal(cx);
                } else {
                    s.hide(w, cx)
                }
                cx.stop_propagation()
            } else if e.keystroke.key == "enter"
                && matches!(s.modal, Some(Dialog::EditDevice { .. }))
            {
                s.submit_edit_dialog(cx);
                cx.stop_propagation()
            }
        }))
    }
}

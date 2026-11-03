//! Native layout mirrors web_ui/{theme-layout,devices,settings}.css in logical pixels.
use super::{skin::*, *};
use battery_protocol::Source;
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
    pub(super) fn finish_edit(&mut self, save: bool, cx: &mut Context<Self>) {
        if let Some((id, ed)) = self.editor.take() {
            self.edit_blur = None;
            if save {
                let n = ed.read(cx).content.trim().to_owned();
                if n.encode_utf16().count() <= 32 {
                    self.issue(Command::Alias(id, n), cx)
                }
            }
            cx.notify()
        }
    }
    fn edit(&mut self, id: String, name: String, w: &mut Window, cx: &mut Context<Self>) {
        self.finish_edit(true, cx);
        let ed = cx.new(|cx| crate::entry::TextInput::new(name, cx));
        let h = ed.read(cx).focus_handle(cx);
        self.editor = Some((id, ed));
        self.edit_blur = Some(cx.on_blur(&h, w, |s, _, cx| s.finish_edit(true, cx)));
        w.focus(&h);
        cx.notify()
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
                            s.finish_edit(true, cx);
                            s.settings_open = !s.settings_open;
                            s.popup = false;
                            s.modal = None;
                            cx.notify()
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
                .overflow_x_scroll()
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
            Kind::Bluetooth => 2,
        });
        for r in rows {
            list = list.child(self.card(r, k, cx))
        }
        list
    }
    fn card(&self, r: &Row, k: Skin, cx: &mut Context<Self>) -> Stateful<Div> {
        let d = &r.device;
        let id = d.id.clone();
        let name = self.state.prefs.name(d).to_string();
        let b = r.battery;
        let online = b.is_some() && r.error.is_none();
        let charge =
            online && b.is_some_and(|x| matches!(x.charge, Charge::Charging | Charge::Full));
        let pct = b.map_or(0, |x| x.display);
        let (ink, fill_start, fill_end) =
            BatteryTone::for_sample(online, charge, pct).palette(k.dark);
        let src = b.map(|b| b.source);
        let (brand, bg, fg) =
            if matches!(src, Some(Source::Hidpp(_))) || id.starts_with("hid:046d:") {
                (
                    self.tr("罗技", "Logitech"),
                    0x3b82f6,
                    if k.dark { 0x9ac8ff } else { 0x285c99 },
                )
            } else if src == Some(Source::Razer) || id.starts_with("hid:1532:") {
                (
                    self.tr("雷蛇", "Razer"),
                    0x22c55e,
                    if k.dark { 0x85dcb4 } else { 0x266b48 },
                )
            } else if src == Some(Source::Rog) || id.starts_with("hid:0b05:") {
                ("ROG", 0xff4d67, if k.dark { 0xffb0b8 } else { 0x9f3c4a })
            } else if d.kind == Kind::Bluetooth {
                (
                    self.tr("蓝牙", "Bluetooth"),
                    0x94a3b8,
                    if k.dark { 0xd8e2ec } else { 0x475569 },
                )
            } else {
                (
                    self.tr("键盘", "Keyboard"),
                    0x94a3b8,
                    if k.dark { 0xd8e2ec } else { 0x475569 },
                )
            };
        let st = if !online {
            self.t("device.offline")
        } else if r.held {
            self.tr("沿用上次读数", "Previous reading")
        } else if b.is_some_and(|b| b.charge == Charge::Full) {
            self.tr("已充满", "Fully charged")
        } else if charge {
            self.tr("充电中", "Charging")
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
        let labels = row().gap(px(6.)).child(badge).child(
            row()
                .gap(px(5.))
                .child(dot(5., fill_start))
                .child(txt(st, 11., k.card_secondary())),
        );
        let edit_id = id.clone();
        let display = name.clone();
        let mut controls = row().gap(px(4.)).child(
            action(format!("edit-{id}"))
                .size(px(22.))
                .rounded(px(4.))
                .hover(|e| e.bg(rgba(0x10b98128)))
                .child(icon("edit", 13., k.dim()))
                .on_click(
                    cx.listener(move |s, _, w, cx| s.edit(edit_id.clone(), display.clone(), w, cx)),
                ),
        );
        if d.kind != Kind::Mouse {
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
                        s.modal = Some(Dialog::Remove(remove_id.clone(), kind, title.clone()));
                        cx.notify()
                    })),
            )
        }
        let title = if let Some((eid, edit)) = &self.editor
            && eid == &id
        {
            div()
                .w(px(170.))
                .min_w_0()
                .child(edit.clone())
                .into_any_element()
        } else {
            txt(name, 15.5, k.text())
                .font_weight(FontWeight::SEMIBOLD)
                .truncate()
                .min_w_0()
                .max_w(px(200.))
                .into_any_element()
        };
        let mut numbers = row().items_baseline().gap(px(3.)).flex_none().child(
            txt(
                if online { pct.to_string() } else { "--".into() },
                32.,
                rgb(ink).into(),
            )
            .line_height(px(32.))
            .font(numeric_font())
            .font_weight(FontWeight::SEMIBOLD),
        );
        if online {
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
                            .w(relative(if online { pct as f32 / 100. } else { 0. }))
                            .rounded_full()
                            .bg(gradient(fill_start << 8 | 255, fill_end << 8 | 255)),
                    ),
            )
            .child(
                row()
                    .justify_between()
                    .child(txt(ty, 11., k.card_secondary()))
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
                    .font_weight(FontWeight::MEDIUM),
            )
            .child(
                txt(self.t(&format!("settings.{key}.subtitle")), 12., k.sub())
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
                    .h(px(28.))
                    .rounded(px(6.))
                    .when(sel, |d| d.bg(k.selected()).shadow_sm())
                    .hover(move |d| d.bg(k.selected()))
                    .child(
                        txt(label.to_string(), 11., if sel { k.text() } else { k.sub() })
                            .when(sel, |d| d.font_weight(FontWeight::SEMIBOLD)),
                    )
                    .on_click(cx.listener(move |s, _, _, cx| s.issue(c.clone(), cx))),
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
                if let Some(c) = &c {
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
            for (i, v) in [Priority::Mouse, Priority::Keyboard, Priority::Lowest]
                .into_iter()
                .enumerate()
            {
                items = items.child(
                    action(format!("priority-opt-{i}"))
                        .h(px(26.))
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
            ("autostart", self.state.autostart, Some(Command::Autostart)),
        ] {
            group = group.child(self.group(key, k, false, self.switch(key, on, cmd, k, cx)))
        }
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
        let import = action("import")
            .px(px(8.))
            .h(px(34.))
            .child(txt(
                self.tr("导入旧版设置", "Import legacy settings"),
                11.,
                k.dim(),
            ))
            .on_click(cx.listener(|s, _, _, cx| s.issue(Command::ImportAppearance, cx)));
        col()
            .w_full()
            .items_center()
            .child(group)
            .child(row().mt(px(14.)).gap(px(8.)).child(check).child(import))
    }
}
impl Render for Panel {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = self.skin();
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
                        txt(msg.clone(), 11., rgb(0xf8fafc).into())
                            .px(px(14.))
                            .py(px(6.))
                            .bg(rgba(0x0f172af2))
                            .rounded_full(),
                    ),
            )
        }
        root.on_key_down(cx.listener(|s, e: &KeyDownEvent, w, cx| {
            if e.keystroke.key == "escape" {
                if s.editor.is_some() {
                    s.finish_edit(false, cx)
                } else if s.modal.take().is_some() || s.popup {
                    s.popup = false;
                    cx.notify()
                } else {
                    s.hide(w, cx)
                }
                cx.stop_propagation()
            } else if e.keystroke.key == "enter" && s.editor.is_some() {
                s.finish_edit(true, cx);
                cx.stop_propagation()
            }
        }))
    }
}

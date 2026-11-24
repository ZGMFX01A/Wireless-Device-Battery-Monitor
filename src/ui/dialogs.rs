// SPDX-FileCopyrightText: 2026 ZGMFX01A
// SPDX-License-Identifier: LicenseRef-Wireless-Device-Battery-Monitor
// See LICENSE in the repository root for the full terms.

use super::{skin::*, *};
use crate::update::Status;
fn token() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64;
    LAST.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some(t.max(v + 1)))
        .map_or(t, |v| t.max(v + 1))
}
fn footbtn(id: &'static str, label: &'static str, primary: bool, k: Skin) -> Stateful<Div> {
    action(id)
        .px(px(13.))
        .min_h(px(29.))
        .py(px(5.))
        .rounded(px(6.))
        .bg(if primary {
            rgb(GREEN).into()
        } else {
            k.control()
        })
        .border_1()
        .border_color(k.line())
        .child(txt(
            label,
            12.,
            if primary { gpui::white() } else { k.sub() },
        ))
}

/// 剥离行内 Markdown 加粗标记 `**`，保持纯文本可读性。
fn strip_bold_markers(text: &str) -> String {
    text.replace("**", "")
}

/// 将发布说明正文按行渲染为结构化列表（支持 `#`/`##`/`###` 标题、`- `/`•` 条目与普通段落）。
fn render_release_notes(text: &str, k: Skin) -> Div {
    let mut box_ = col().w_full().gap(px(6.));
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line == "---" {
            continue;
        }
        if let Some(heading) = line
            .strip_prefix("### ")
            .or_else(|| line.strip_prefix("## "))
            .or_else(|| line.strip_prefix("# "))
        {
            box_ = box_.child(
                txt(strip_bold_markers(heading.trim()), 12.5, k.text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .mt(px(4.))
                    .w_full()
                    .whitespace_normal(),
            );
            continue;
        }
        if let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            let item = item.trim();
            if let Some(rest) = item.strip_prefix("**")
                && let Some((head, tail)) = rest.split_once("**")
            {
                let tail = tail.trim_start_matches(['：', ':']).trim();
                let mut entry = col().w_full().gap(px(2.)).child(
                    txt(format!("• {}", head.trim()), 12., k.text())
                        .font_weight(FontWeight::MEDIUM)
                        .w_full()
                        .whitespace_normal(),
                );
                if !tail.is_empty() {
                    entry = entry.child(
                        txt(strip_bold_markers(tail), 11.5, k.sub())
                            .pl(px(10.))
                            .w_full()
                            .whitespace_normal(),
                    );
                }
                box_ = box_.child(entry);
                continue;
            }
            box_ = box_.child(
                txt(format!("• {}", strip_bold_markers(item)), 11.5, k.sub())
                    .w_full()
                    .whitespace_normal(),
            );
            continue;
        }
        box_ = box_.child(
            txt(strip_bold_markers(line), 11.5, k.sub())
                .w_full()
                .whitespace_normal(),
        );
    }
    box_
}
impl Panel {
    pub(super) fn toast(&mut self, message: Toast, cx: &mut Context<Self>) {
        let i = token();
        self.toast = Some((message, i));
        cx.spawn(async move |e, cx| {
            Timer::after(std::time::Duration::from_millis(2600)).await;
            let _ = e.update(cx, |s, cx| {
                if s.toast.as_ref().is_some_and(|(_, v)| *v == i) {
                    s.toast = None;
                    cx.notify()
                }
            });
        })
        .detach();
        cx.notify()
    }
    pub(super) fn refresh(&mut self, scan: bool, cx: &mut Context<Self>) {
        if self.manual.is_some() {
            return;
        }
        let id = token();
        self.manual = Some((id, scan));
        if scan {
            self.compat_hint.begin_scan(id, &self.state.rows);
        }
        self.toast(
            Toast::Label(if scan {
                "toast.scan_started"
            } else {
                "toast.refresh_started"
            }),
            cx,
        );
        self.issue(Command::Sync(id, scan), cx)
    }
    pub(super) fn pick(&mut self, kind: Kind, cx: &mut Context<Self>) {
        let id = token();
        self.modal = Some(Dialog::Pick {
            kind,
            id,
            chosen: None,
            binding: false,
        });
        if kind == Kind::Keyboard {
            self.compat_hint.begin_keyboard_pick(id);
        }
        self.issue(
            Command::DeviceRequest(DeviceJob {
                id,
                kind,
                bind: None,
            }),
            cx,
        );
        cx.notify()
    }
    pub(super) fn reconcile(&mut self, cx: &mut Context<Self>) {
        let mut trigger_compatible_hint = false;
        if let Some(Dialog::Pick {
            kind,
            id,
            chosen,
            binding,
        }) = &mut self.modal
            && let Some(r) = self.state.reply(*kind).filter(|r| r.id == *id)
        {
            if r.result.is_ok() {
                if *binding {
                    self.modal = None;
                    self.modal_subs.clear();
                    self.compat_hint.on_device_bound();
                    self.toast(Toast::Label("toast.bound_success"), cx);
                    return;
                }
                let has_unbound = self
                    .state
                    .candidates
                    .iter()
                    .any(|d| d.kind == *kind && !self.state.prefs.bound(d));
                if chosen.is_none() {
                    *chosen = self
                        .state
                        .candidates
                        .iter()
                        .find(|d| d.kind == *kind && !self.state.prefs.bound(d))
                        .map(|d| d.id.clone());
                }
                if *kind == Kind::Keyboard {
                    trigger_compatible_hint = self.compat_hint.complete_keyboard_pick(
                        *id,
                        true,
                        has_unbound,
                        self.state.prefs.engine_mode,
                        std::time::Instant::now(),
                    );
                }
            } else if !*binding && *kind == Kind::Keyboard {
                self.compat_hint.complete_keyboard_pick(
                    *id,
                    false,
                    false,
                    self.state.prefs.engine_mode,
                    std::time::Instant::now(),
                );
            }
        }
        if trigger_compatible_hint {
            self.maybe_open_compatible_hint(cx);
        }
    }

    /// 在普通模式且当前未弹出其他关键弹窗时，打开兼容模式引导提示弹窗。
    pub(super) fn maybe_open_compatible_hint(&mut self, cx: &mut Context<Self>) {
        if self.settings_open || self.state.prefs.engine_mode != EngineMode::Normal {
            return;
        }
        let can_replace = match &self.modal {
            None => true,
            Some(Dialog::Pick {
                kind: Kind::Keyboard,
                id,
                binding: false,
                ..
            }) => {
                self.state
                    .reply(Kind::Keyboard)
                    .is_some_and(|r| r.id == *id && r.result.is_ok())
                    && !self
                        .state
                        .candidates
                        .iter()
                        .any(|d| d.kind == Kind::Keyboard && !self.state.prefs.bound(d))
            }
            _ => false,
        };
        if can_replace {
            self.modal = Some(Dialog::SuggestCompatibleMode);
            self.modal_subs.clear();
            cx.notify();
        }
    }
    fn begin_bind(&mut self, cx: &mut Context<Self>) {
        if let Some(Dialog::Pick {
            kind,
            chosen: Some(key),
            id: previous,
            ..
        }) = self.modal.clone()
            && self.state.reply(kind).is_some_and(|r| r.id == previous)
            && self
                .state
                .candidates
                .iter()
                .any(|d| d.kind == kind && d.id == key && !self.state.prefs.bound(d))
        {
            let id = token();
            self.modal = Some(Dialog::Pick {
                kind,
                id,
                chosen: Some(key.clone()),
                binding: true,
            });
            self.issue(
                Command::DeviceRequest(DeviceJob {
                    id,
                    kind,
                    bind: Some(key),
                }),
                cx,
            )
        }
    }
    pub(super) fn dialog(&self, d: Dialog, k: Skin, cx: &mut Context<Self>) -> Stateful<Div> {
        let title;
        let mut sub = String::new();
        let mut body = col()
            .id("dialog-list")
            .w_full()
            .flex_none()
            .max_h(px(340.))
            .min_h(px(42.))
            .overflow_y_scroll()
            .px(px(16.))
            .py(px(8.))
            .gap(px(6.));
        let mut footer = row()
            .flex_none()
            .justify_end()
            .flex_wrap()
            .gap(px(8.))
            .px(px(16.))
            .pt(px(10.))
            .pb(px(14.))
            .border_t_1()
            .border_color(k.line());
        let close = |label: &'static str| {
            footbtn("dialog-close", label, false, k)
                .on_click(cx.listener(|s, _, _, cx| s.close_modal(cx)))
        };
        let mut width = 330.;
        match d {
            Dialog::Pick {
                kind,
                id,
                chosen,
                binding,
            } => {
                let pre = if kind == Kind::Keyboard {
                    "keyboard"
                } else {
                    "bluetooth"
                };
                title = self.t(&format!("{pre}.dialog.title")).to_string();
                sub = self.t(&format!("{pre}.dialog.subtitle")).into();
                let result = self
                    .state
                    .reply(kind)
                    .filter(|r| r.id == id)
                    .map(|r| &r.result);
                let retry = matches!(result, Some(Err(_)));
                let can = result.is_some()
                    && chosen.as_ref().is_some_and(|key| {
                        self.state
                            .candidates
                            .iter()
                            .any(|d| d.kind == kind && d.id == *key && !self.state.prefs.bound(d))
                    });
                let retry_scan = retry && !can;
                match result {
                    None => {
                        body = body.child(
                            txt(
                                if binding {
                                    self.t("device.connecting")
                                } else {
                                    self.t(&format!("{pre}.dialog.loading"))
                                },
                                12.,
                                k.sub(),
                            )
                            .py(px(24.))
                            .text_center(),
                        )
                    }
                    Some(Err(_)) => {
                        body = body.child(txt(
                            self.t(if binding {
                                "device.add_failed"
                            } else if kind == Kind::Keyboard {
                                "toast.hid_scan_failed"
                            } else {
                                "toast.bluetooth_scan_failed"
                            }),
                            12.,
                            rgb(RED).into(),
                        ))
                    }
                    Some(Ok(())) => {
                        let mut count = 0;
                        for dev in self.state.candidates.iter().filter(|d| d.kind == kind) {
                            count += 1;
                            let key = dev.id.clone();
                            let bound = self.state.prefs.bound(dev);
                            let sel = chosen.as_ref() == Some(&key);
                            let mut item = row()
                                .id(SharedString::from(format!("candidate-{}", dev.id)))
                                .justify_between()
                                .p(px(9.))
                                .rounded(px(8.))
                                .border_1()
                                .border_color(if sel { rgb(GREEN).into() } else { k.line() })
                                .bg(if sel {
                                    rgba(0x10b98120).into()
                                } else {
                                    k.grad(false)
                                })
                                .child(
                                    col()
                                        .child(txt(self.state.candidate_name(dev), 12., k.text()))
                                        .child(txt(
                                            if bound {
                                                self.t("device.bound")
                                            } else if kind == Kind::Bluetooth {
                                                self.t("device.paired")
                                            } else {
                                                self.t("device.ready")
                                            },
                                            10.,
                                            k.dim(),
                                        )),
                                );
                            if !bound && !binding {
                                item = item.cursor_pointer().on_click(cx.listener(
                                    move |s, _, _, cx| {
                                        if let Some(Dialog::Pick { chosen, .. }) = &mut s.modal {
                                            *chosen = Some(key.clone());
                                            cx.notify()
                                        }
                                    },
                                ))
                            } else {
                                item = item.opacity(0.5)
                            }
                            body = body.child(item)
                        }
                        if count == 0 {
                            body = body.child(
                                txt(self.t(&format!("{pre}.dialog.empty")), 12., k.dim())
                                    .py(px(24.))
                                    .text_center(),
                            )
                        }
                    }
                }
                footer = footer.child(close(self.t("dialog.cancel")));
                let confirm = footbtn(
                    "dialog-confirm",
                    if retry {
                        self.t("dialog.retry")
                    } else {
                        self.t(if kind == Kind::Keyboard {
                            "dialog.connect"
                        } else {
                            "dialog.add"
                        })
                    },
                    true,
                    k,
                );
                footer = footer.child(if can {
                    confirm.on_click(cx.listener(|s, _, _, cx| s.begin_bind(cx)))
                } else if retry_scan {
                    confirm.on_click(cx.listener(move |s, _, _, cx| s.pick(kind, cx)))
                } else {
                    confirm.opacity(0.45)
                })
            }
            Dialog::EditDevice {
                auto_icon,
                icon: selected_icon,
                name_input,
                color_input,
                ..
            } => {
                use crate::device_display::{DeviceIcon, parse_rgb_color};
                width = 340.;
                title = self.t("device.edit.title").into();
                let name_valid = name_input.read(cx).content.trim().encode_utf16().count() <= 32;
                let color_text = color_input.read(cx).content.clone();
                let parsed_color = parse_rgb_color(color_text.as_ref());
                let custom_color = parsed_color.ok().flatten();
                let color_non_empty = !color_text.trim().is_empty();
                let preview_icon = selected_icon.unwrap_or(auto_icon);
                let preview_tint: Hsla = if parsed_color.is_err() {
                    rgb(RED).into()
                } else if let Some(rgb_val) = custom_color {
                    rgb(rgb_val).into()
                } else {
                    k.text()
                };

                let mut icon_choices = row().flex_wrap().gap(px(6.));
                let auto_selected = selected_icon.is_none();
                icon_choices = icon_choices.child(
                    action("edit-icon-auto")
                        .h(px(28.))
                        .px(px(9.))
                        .gap(px(5.))
                        .rounded(px(7.))
                        .border_1()
                        .border_color(if auto_selected {
                            rgb(GREEN).into()
                        } else {
                            k.line()
                        })
                        .bg(if auto_selected {
                            rgba(0x10b98122).into()
                        } else {
                            k.control()
                        })
                        .child(icon(
                            auto_icon.asset_name(),
                            14.,
                            if auto_selected {
                                custom_color.map_or_else(|| rgb(GREEN).into(), |c| rgb(c).into())
                            } else {
                                k.sub()
                            },
                        ))
                        .child(txt(
                            self.t("device.edit.icon_auto"),
                            11.5,
                            if auto_selected { k.text() } else { k.sub() },
                        ))
                        .on_click(cx.listener(|s, _, _, cx| {
                            if let Some(Dialog::EditDevice { icon, .. }) = &mut s.modal {
                                *icon = None;
                                cx.notify();
                            }
                        })),
                );
                for choice in DeviceIcon::ALL {
                    let is_sel = selected_icon == Some(choice);
                    let glyph_color: Hsla = if is_sel {
                        custom_color.map_or_else(|| rgb(GREEN).into(), |c| rgb(c).into())
                    } else {
                        k.sub()
                    };
                    icon_choices = icon_choices.child(
                        action(format!("edit-icon-{}", choice.asset_name()))
                            .w(px(36.))
                            .h(px(28.))
                            .rounded(px(7.))
                            .border_1()
                            .border_color(if is_sel { rgb(GREEN).into() } else { k.line() })
                            .bg(if is_sel {
                                rgba(0x10b98122).into()
                            } else {
                                k.control()
                            })
                            .child(icon(choice.asset_name(), 15., glyph_color))
                            .on_click(cx.listener(move |s, _, _, cx| {
                                if let Some(Dialog::EditDevice { icon, .. }) = &mut s.modal {
                                    *icon = Some(choice);
                                    cx.notify();
                                }
                            })),
                    );
                }

                let clear_input = color_input.clone();
                let mut color_header = row().justify_between().w_full().child(txt(
                    self.t("device.edit.color"),
                    11.5,
                    k.sub(),
                ));
                if color_non_empty {
                    color_header = color_header.child(
                        action("edit-color-clear")
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(5.))
                            .hover(|d| d.bg(k.control()))
                            .child(txt(
                                self.t("device.edit.color_clear"),
                                11.,
                                rgb(GREEN).into(),
                            ))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                clear_input.update(cx, |input, cx| input.set_content("", cx));
                            })),
                    );
                }

                let preview_box = row()
                    .size(px(28.))
                    .justify_center()
                    .rounded(px(7.))
                    .border_1()
                    .border_color(if parsed_color.is_err() {
                        rgb(RED).into()
                    } else if let Some(rgb_val) = custom_color {
                        rgba((rgb_val << 8) | 0x66).into()
                    } else {
                        k.line()
                    })
                    .bg(k.control())
                    .child(icon(preview_icon.asset_name(), 15., preview_tint));

                body = body
                    .gap(px(10.))
                    .child(
                        col()
                            .gap(px(4.))
                            .child(txt(self.t("device.edit.name"), 11.5, k.sub()))
                            .child(div().w_full().text_color(k.text()).child(name_input)),
                    )
                    .child(
                        col()
                            .gap(px(5.))
                            .child(txt(self.t("device.edit.icon"), 11.5, k.sub()))
                            .child(icon_choices),
                    )
                    .child(
                        col()
                            .gap(px(4.))
                            .child(color_header)
                            .child(
                                row()
                                    .gap(px(8.))
                                    .w_full()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_color(k.text())
                                            .child(color_input),
                                    )
                                    .child(preview_box),
                            )
                            .child(
                                txt(
                                    if parsed_color.is_err() {
                                        self.t("device.edit.color_invalid")
                                    } else {
                                        self.t("device.edit.color_hint")
                                    },
                                    10.5,
                                    if parsed_color.is_err() {
                                        rgb(RED).into()
                                    } else {
                                        k.dim()
                                    },
                                )
                                .whitespace_normal(),
                            ),
                    );

                let can_save = name_valid && parsed_color.is_ok();
                let save = footbtn("dialog-save", self.t("dialog.save"), true, k);
                footer = footer
                    .child(close(self.t("dialog.cancel")))
                    .child(if can_save {
                        save.on_click(cx.listener(|s, _, _, cx| s.submit_edit_dialog(cx)))
                    } else {
                        save.opacity(0.45)
                    });
            }
            Dialog::Remove(id, kind, name) => {
                let prefix = if kind == Kind::Keyboard {
                    "keyboard"
                } else {
                    "bluetooth"
                };
                title = self.t(&format!("{prefix}.remove.title")).into();
                sub = self.t(&format!("{prefix}.remove.message")).into();
                body = body.child(txt(name, 13., k.text()));
                footer = footer.child(close(self.t("dialog.cancel"))).child(
                    footbtn("dialog-confirm", self.t("dialog.remove"), true, k).on_click(
                        cx.listener(move |s, _, _, cx| {
                            s.modal = None;
                            s.issue(Command::Unbind(id.clone()), cx);
                            cx.notify()
                        }),
                    ),
                )
            }
            Dialog::ConfirmCompatibleMode => {
                title = self.t("dialog.compatible.title").into();
                sub = self.t("dialog.compatible.message").into();
                footer = footer.child(close(self.t("dialog.cancel"))).child(
                    footbtn(
                        "dialog-confirm",
                        self.t("dialog.compatible.confirm"),
                        true,
                        k,
                    )
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.modal = None;
                        s.issue(Command::SetEngineMode(EngineMode::Compatible), cx);
                        cx.notify();
                    })),
                );
            }
            Dialog::SuggestCompatibleMode => {
                width = 330.;
                title = self.t("dialog.suggest_compatible.title").into();
                body = body.child(
                    txt(self.t("dialog.suggest_compatible.message"), 12., k.sub())
                        .w_full()
                        .whitespace_normal()
                        .flex_none(),
                );
                footer = footer.child(close(self.t("dialog.cancel"))).child(
                    footbtn(
                        "dialog-confirm",
                        self.t("dialog.suggest_compatible.confirm"),
                        true,
                        k,
                    )
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.modal = None;
                        s.modal_subs.clear();
                        s.compat_hint.on_accepted();
                        s.issue(Command::SetEngineMode(EngineMode::Compatible), cx);
                        s.toast(Toast::Label("toast.compatible_mode_enabled"), cx);
                    })),
                );
            }
            Dialog::ConfirmUnknownProbe => {
                title = self.t("dialog.unknown_probe.title").into();
                sub = self.t("dialog.unknown_probe.message").into();
                footer = footer.child(close(self.t("dialog.cancel"))).child(
                    footbtn(
                        "dialog-confirm",
                        self.t("dialog.unknown_probe.confirm"),
                        true,
                        k,
                    )
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.modal = None;
                        s.issue(Command::SetUnknownProbe(true), cx);
                        cx.notify();
                    })),
                );
            }
            Dialog::SupportStar => {
                width = 310.;
                title = self.t("dialog.star.title").into();
                body = body.child(
                    txt(self.t("dialog.star.message"), 12., k.sub())
                        .w_full()
                        .whitespace_normal()
                        .flex_none(),
                );
                footer = footer.child(close(self.t("dialog.star.later"))).child(
                    footbtn("star-open", self.t("dialog.star.action"), true, k).on_click(
                        cx.listener(|s, _, _, cx| {
                            s.modal = None;
                            cx.notify();
                            cx.open_url(PROJECT_HOME_URL);
                        }),
                    ),
                );
            }
            Dialog::Update => {
                width = 340.;
                let st = &self.state.update;
                title = if matches!(
                    st,
                    Status::Available(_) | Status::Downloading(..) | Status::Ready(_)
                ) {
                    self.t("update.modal_title")
                } else {
                    self.t("action.check_update")
                }
                .into();
                match st {
                    Status::Available(v) => {
                        sub = format!(
                            "v{v}   ·   {:.1} MB",
                            self.state.release_size as f64 / 1048576.
                        );
                        let notes = crate::model::select_release_notes(
                            english(&self.state.prefs),
                            &self.state.release_notes,
                            &self.state.release_notes_en,
                        );
                        body = body
                            .child(txt(self.t("update.notes_title"), 11., k.dim()))
                            .child(
                                col()
                                    .max_h(px(200.))
                                    .id("notes-scroll")
                                    .overflow_y_scroll()
                                    .p(px(10.))
                                    .rounded(px(8.))
                                    .bg(k.grad(false))
                                    .child(if notes.trim().is_empty() {
                                        txt(self.t("update.no_notes"), 12., k.sub())
                                            .into_any_element()
                                    } else {
                                        render_release_notes(notes, k).into_any_element()
                                    }),
                            );
                        footer = footer.child(close(self.t("update.later"))).child(
                            footbtn("update-start", self.t("update.start"), true, k).on_click(
                                cx.listener(|s, _, _, cx| s.issue(Command::DownloadUpdate, cx)),
                            ),
                        )
                    }
                    Status::Downloading(n, t) => {
                        let fraction = *n as f64 / (*t).max(1) as f64;
                        body = body
                            .child(txt(
                                self.t("update.downloading")
                                    .replace("{percent}", &format!("{:.0}", fraction * 100.)),
                                12.,
                                k.text(),
                            ))
                            .child(
                                div()
                                    .w_full()
                                    .h(px(6.))
                                    .rounded_full()
                                    .bg(k.control())
                                    .child(
                                        div()
                                            .w(relative(fraction.clamp(0., 1.) as f32))
                                            .h_full()
                                            .bg(gradient(0x10b981ff, 0x06b6d4ff)),
                                    ),
                            )
                            .child(
                                row()
                                    .justify_between()
                                    .child(txt(
                                        format!(
                                            "{:.1} / {:.1} MB",
                                            *n as f64 / 1048576.,
                                            *t as f64 / 1048576.
                                        ),
                                        11.,
                                        k.dim(),
                                    ))
                                    .child(txt(
                                        format!(
                                            "{:.2} MB/s",
                                            self.state.bytes_per_sec as f64 / 1048576.
                                        ),
                                        11.,
                                        rgb(CYAN).into(),
                                    )),
                            );
                        footer = footer.child(
                            footbtn("cancel-download", self.t("update.cancel"), false, k).on_click(
                                cx.listener(|s, _, _, cx| s.issue(Command::CancelUpdate, cx)),
                            ),
                        )
                    }
                    Status::Ready(v) => {
                        sub = format!("v{v}");
                        body = body.child(txt(self.t("update.verified"), 12., k.text()));
                        footer = footer.child(close(self.t("update.later"))).child(
                            footbtn("install", self.t("update.install_restart"), true, k).on_click(
                                cx.listener(|s, _, _, cx| s.issue(Command::InstallUpdate, cx)),
                            ),
                        )
                    }
                    other => {
                        let msg = match other {
                            Status::Error(failure) => {
                                strings::update_failure(locale(&self.state.prefs), *failure).into()
                            }
                            Status::Current => self
                                .t("update.latest")
                                .replace("{version}", env!("APP_VERSION")),
                            Status::Cancelled => self.t("update.cancelled").into(),
                            _ => self.t("update.checking").into(),
                        };
                        body = body.child(
                            txt(msg, 12., k.sub())
                                .w_full()
                                .whitespace_normal()
                                .flex_none(),
                        );
                        footer = footer.child(close(self.t("dialog.ok")))
                    }
                }
            }
            Dialog::WhatsNew => {
                width = 340.;
                let notes = self
                    .state
                    .installed_release_notes
                    .as_ref()
                    .filter(|n| !n.version.is_empty());
                let version = notes
                    .map(|n| n.version.clone())
                    .unwrap_or_else(|| env!("APP_VERSION").to_string());
                title = self
                    .t("update.whats_new_title")
                    .replace("{version}", &version);
                sub = self.t("update.whats_new_subtitle").into();
                let content = notes
                    .map(|n| n.text_for(english(&self.state.prefs)))
                    .unwrap_or("")
                    .trim();
                body = body.child(
                    col()
                        .max_h(px(240.))
                        .id("whats-new-scroll")
                        .overflow_y_scroll()
                        .p(px(10.))
                        .rounded(px(8.))
                        .bg(k.grad(false))
                        .child(if content.is_empty() {
                            txt(self.t("update.whats_new_loading"), 12., k.sub()).into_any_element()
                        } else {
                            render_release_notes(content, k).into_any_element()
                        }),
                );
                let url = format!("{PROJECT_HOME_URL}/releases/tag/v{version}");
                footer = footer
                    .child(
                        footbtn("whats-new-release", self.t("update.view_release"), false, k)
                            .on_click(cx.listener(move |s, _, _, cx| {
                                s.dismiss_whats_new(cx);
                                s.modal = None;
                                s.modal_subs.clear();
                                cx.notify();
                                cx.open_url(&url);
                            })),
                    )
                    .child(
                        footbtn("dialog-confirm", self.t("dialog.ok"), true, k).on_click(
                            cx.listener(|s, _, _, cx| {
                                s.dismiss_whats_new(cx);
                                s.modal = None;
                                s.modal_subs.clear();
                                cx.notify();
                            }),
                        ),
                    );
            }
        }
        let card = col()
            .id("dialog-card")
            .on_click(|_, _, cx| cx.stop_propagation())
            .w(px(width))
            .max_w_full()
            .flex_none()
            .rounded(px(14.))
            .bg(k.dialog())
            .border_1()
            .border_color(k.line())
            .shadow_lg()
            .overflow_hidden()
            .child(
                col()
                    .px(px(16.))
                    .pt(px(14.))
                    .pb(px(8.))
                    .gap(px(3.))
                    .child(
                        txt(title, 14., k.text())
                            .whitespace_normal()
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .when(!sub.is_empty(), |e| {
                        e.child(txt(sub, 11., k.dim()).whitespace_normal())
                    }),
            )
            .child(body)
            .child(footer);
        row()
            .id("modal-overlay")
            .absolute()
            .inset_0()
            .occlude()
            .on_click(cx.listener(|s, _, _, cx| s.close_modal(cx)))
            .justify_center()
            .p(px(20.))
            .bg(rgba(0x00000099))
            .child(card)
    }
}

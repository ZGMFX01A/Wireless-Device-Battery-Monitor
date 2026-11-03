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
        .h(px(29.))
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
impl Panel {
    pub(super) fn toast(&mut self, text: String, cx: &mut Context<Self>) {
        let i = token();
        self.toast = Some((text, i));
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
        self.toast(
            self.t(if scan {
                "toast.scan_started"
            } else {
                "toast.refresh_started"
            })
            .into(),
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
        if let Some(Dialog::Pick {
            kind,
            id,
            chosen,
            binding,
        }) = &mut self.modal
            && let Some(r) = self.state.reply(*kind).filter(|r| r.id == *id)
            && r.result.is_ok()
        {
            if *binding {
                self.modal = None;
                self.toast(self.t("toast.bound_success").into(), cx)
            } else if chosen.is_none() {
                *chosen = self
                    .state
                    .candidates
                    .iter()
                    .find(|d| d.kind == *kind && !self.state.prefs.bound(d))
                    .map(|d| d.id.clone())
            }
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
            .max_h(px(240.))
            .min_h(px(42.))
            .overflow_y_scroll()
            .px(px(16.))
            .py(px(8.))
            .gap(px(6.));
        let mut footer = row()
            .justify_end()
            .gap(px(8.))
            .px(px(16.))
            .pt(px(10.))
            .pb(px(14.))
            .border_t_1()
            .border_color(k.line());
        let close = |label: &'static str| {
            footbtn("dialog-close", label, false, k).on_click(cx.listener(|s, _, _, cx| {
                s.modal = None;
                cx.notify()
            }))
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
                    Some(Err(e)) => {
                        body = body.child(txt(
                            self.t("toast.op_failed").replace("{msg}", e),
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
                                        .child(txt(
                                            self.state.prefs.name(dev).to_owned(),
                                            12.,
                                            k.text(),
                                        ))
                                        .child(txt(
                                            if bound {
                                                self.t("device.bound")
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
                        self.tr("重试", "Retry")
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
            Dialog::Update => {
                width = 320.;
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
                        body = body
                            .child(txt(self.t("update.notes_title"), 11., k.dim()))
                            .child(
                                col()
                                    .max_h(px(140.))
                                    .id("notes-scroll")
                                    .overflow_y_scroll()
                                    .p(px(10.))
                                    .rounded(px(8.))
                                    .bg(k.grad(false))
                                    .child(txt(
                                        if self.state.release_notes.is_empty() {
                                            self.tr(
                                                "此版本未提供更新说明",
                                                "No release notes provided",
                                            )
                                            .into()
                                        } else {
                                            self.state.release_notes.clone()
                                        },
                                        12.,
                                        k.sub(),
                                    )),
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
                        body = body.child(txt(
                            self.tr("签名与文件校验通过", "Signature and file verified"),
                            12.,
                            k.text(),
                        ));
                        footer = footer.child(close(self.t("update.later"))).child(
                            footbtn(
                                "install",
                                self.tr("安装并重启", "Install & restart"),
                                true,
                                k,
                            )
                            .on_click(
                                cx.listener(|s, _, _, cx| s.issue(Command::InstallUpdate, cx)),
                            ),
                        )
                    }
                    other => {
                        let msg = match other {
                            Status::Error(e) => self.t("update.check_failed").replace("{msg}", e),
                            Status::Current => self
                                .t("update.latest")
                                .replace("{version}", env!("APP_VERSION")),
                            Status::Cancelled => self.t("update.cancelled").into(),
                            _ => self.tr("正在检查更新…", "Checking for updates…").into(),
                        };
                        body = body.child(txt(msg, 12., k.sub()));
                        footer = footer.child(close(self.t("dialog.ok")))
                    }
                }
            }
        }
        let card = col()
            .id("dialog-card")
            .on_click(|_, _, cx| cx.stop_propagation())
            .w_full()
            .max_w(px(width))
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
                    .child(txt(title, 14., k.text()).font_weight(FontWeight::SEMIBOLD))
                    .when(!sub.is_empty(), |e| e.child(txt(sub, 11., k.dim()))),
            )
            .child(body)
            .child(footer);
        row()
            .id("modal-overlay")
            .absolute()
            .inset_0()
            .occlude()
            .on_click(cx.listener(|s, _, _, cx| {
                s.modal = None;
                cx.notify()
            }))
            .justify_center()
            .p(px(20.))
            .bg(rgba(0x00000099))
            .child(card)
    }
}

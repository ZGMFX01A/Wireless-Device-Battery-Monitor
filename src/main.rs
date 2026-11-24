// SPDX-FileCopyrightText: 2026 ZGMFX01A
// SPDX-License-Identifier: LicenseRef-Wireless-Device-Battery-Monitor
// See LICENSE in the repository root for the full terms.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod device_display;
mod entry;
mod host;
mod icon;
mod keyboard;
mod model;
mod service;
mod startup;
mod swap;
mod taskbar;
mod tooltip;
mod update;
#[cfg(test)]
#[path = "../version_format.rs"]
mod version_format;
mod view;
fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let res = match args.as_slice() {
        [s] if s == "--version" => {
            println!("{}", env!("APP_VERSION"));
            Ok(())
        }
        [s] if s == "--gpui-update-worker" => swap::helper().map_err(host::AppError::UpdateWorker),
        [s] if s == "--gpui-health-start" => host::run(false),
        [s] if s == "--gui-child" => {
            view::run(true, model::Mode::Flyout);
            Ok(())
        }
        [s] if s == "--preview" => {
            view::run(false, model::Mode::Flyout);
            Ok(())
        }
        [s] if s == "--smoke-test" => {
            let svc = service::Service::start(model::Prefs::default(), true, || {});
            if svc.state.lock().unwrap().rows.len() == 2 {
                Ok(())
            } else {
                Err(host::AppError::SmokeTest)
            }
        }
        [s, m] if s == "--smoke-test" && m == "compat" => {
            let prefs = model::Prefs {
                engine_mode: battery_protocol::EngineMode::Compatible,
                ..Default::default()
            };
            if service::Service::smoke_test_compatible(prefs) {
                Ok(())
            } else {
                Err(host::AppError::SmokeTest)
            }
        }
        [s] if s == "--demo-tray" || s == "--exercise-tray" => host::run(true),
        [] => host::run(false),
        _ => Err(host::AppError::UnknownOption),
    };
    if let Err(e) = res {
        host::report(e);
        std::process::exit(1)
    }
}

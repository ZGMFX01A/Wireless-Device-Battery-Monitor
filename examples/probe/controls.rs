// SPDX-FileCopyrightText: 2026 ZGMFX01A
// SPDX-License-Identifier: LicenseRef-Wireless-Device-Battery-Monitor
// See LICENSE in the repository root for the full terms.

use super::*;
pub fn run(exe: &Path, out: &Path) -> Result<Value> {
    let mut a = Ui::new(exe, out)?;
    let mut passed = Vec::new();
    let tested = (|| {
        a.update()?;
        a.open("Settings")?;
        for (x, y, cmd, label) in [
            (320., 165., json!({"SetLanguage":"En"}), "language"),
            (319., 277., json!({"SetStyle":"Number"}), "tray style"),
            (478., 420., json!({"SetLow":30}), "low battery threshold"),
            (475., 556., json!("AutoUpdate"), "update switch"),
            (475., 630., json!("Autostart"), "autostart IPC only"),
        ] {
            a.exact(x, y, cmd)?;
            passed.push(label);
        }
        a.click(470., 350.)?;
        a.photo("Priority-dropdown")?;
        a.exact(421., 434., json!({"SetPriority":"Lowest"}))?;
        passed.push("priority dropdown");
        native::wheel(a.hwnd, -120, 1)?;
        a.exact(479., 627., json!({"SetPollInterval":60}))?;
        a.state["prefs"]["poll_interval_secs"] = json!(60);
        a.update()?;
        a.exact(416., 627., json!({"SetPollInterval":30}))?;
        a.state["prefs"]["poll_interval_secs"] = json!(5);
        a.update()?;
        a.click(416., 627.)?;
        require(
            a.p.lines.try_recv().is_err(),
            "poll interval decreased below minimum",
        )?;
        a.state["prefs"]["poll_interval_secs"] = json!(300);
        a.update()?;
        a.click(479., 627.)?;
        require(
            a.p.lines.try_recv().is_err(),
            "poll interval exceeded maximum",
        )?;
        a.state["prefs"]["poll_interval_secs"] = json!(30);
        a.update()?;
        a.photo("Polling-control")?;
        passed.push("poll interval buttons and limits");
        native::wheel(a.hwnd, 360, 9)?;
        a.open("Flyout")?;
        a.click(53., 76.)?;
        let v = a.p.json(|v| v.get("Sync").is_some())?;
        require(v["Sync"][1] == true, "scan flag missing")?;
        a.state["manual"] = json!({"id":v["Sync"][0],"hid":true,"bt":true,"error":null});
        a.update()?;
        passed.push("scan completion");
        a.click(146., 78.)?;
        let q = a.p.json(|v| v.get("DeviceRequest").is_some())?["DeviceRequest"].clone();
        require(
            q["kind"] == "Keyboard" && q["bind"].is_null(),
            "invalid keyboard scan command",
        )?;
        a.photo("Keyboard-loading")?;
        a.state["keyboard_reply"] = json!({"id":q["id"],"result":{"Ok":null}});
        a.update()?;
        a.primary("Keyboard-picker")?;
        let b = a.p.json(|v| v.get("DeviceRequest").is_some())?["DeviceRequest"].clone();
        require(
            b["kind"] == "Keyboard" && b["bind"] == "spare",
            "invalid keyboard binding",
        )?;
        a.state["keyboard_reply"] = json!({"id":b["id"],"result":{"Err":"test: receiver asleep"}});
        a.update()?;
        a.photo("Keyboard-bind-failure")?;
        passed.push("keyboard binding failure");
        a.primary("Keyboard-retry")?;
        let retry = a.p.json(|v| v.get("DeviceRequest").is_some())?["DeviceRequest"].clone();
        require(
            retry["bind"] == b["bind"] && retry["id"] != b["id"],
            "retry reused an old request",
        )?;
        a.state["keyboard_reply"] = json!({"id":retry["id"],"result":{"Err":"test: still asleep"}});
        a.update()?;
        passed.push("retry without closing dialog");
        a.key(27)?;
        a.click(246., 77.)?;
        let q = a.p.json(|v| v.get("DeviceRequest").is_some())?["DeviceRequest"].clone();
        require(q["kind"] == "Bluetooth", "not a Bluetooth request")?;
        a.state["bluetooth_reply"] = json!({"id":q["id"],"result":{"Ok":null}});
        a.update()?;
        a.photo("Bluetooth-picker")?;
        a.key(27)?;
        passed.push("Bluetooth picker");
        a.click(332., 277.)?;
        a.photo("Remove-confirm")?;
        a.key(27)?;
        require(
            a.p.lines.try_recv().is_err(),
            "cancel sent an unexpected command",
        )?;
        a.click(332., 277.)?;
        a.primary("Remove-confirm")?;
        a.p.json(|v| v == &json!({"Unbind":a.state["rows"][1]["device"]["id"]}))?;
        passed.push("cancel and confirm unbind");
        a.click(334., 129.)?;
        pause(90);
        native::post(a.hwnd, 0x102, 'X' as usize, 1)?;
        a.key(13)?;
        a.p.json(|v| v == &json!({"Alias":[a.state["rows"][0]["device"]["id"],"X"]}))?;
        passed.push("inline rename");
        let original = a.hwnd;
        a.click(312., 24.)?;
        pause(80);
        require(
            screen(a.p.child.id(), true)? == original,
            "gear replaced window",
        )?;
        passed.push("gear in same window");
        a.exact(230., 164., json!({"SetLanguage":"En"}))?;
        passed.push("settings in flyout");
        a.open("Settings")?;
        native::wheel(a.hwnd, -360, 9)?;
        a.photo("Settings-bottom")?;
        a.click(390., 553.)?;
        a.photo("Recognition-mode-confirm")?;
        a.key(27)?;
        require(
            a.p.lines.try_recv().is_err(),
            "cancelling the mode change sent a command",
        )?;
        a.click(390., 553.)?;
        a.primary("Recognition-mode-confirm")?;
        a.p.json(|v| v == &json!({"SetEngineMode":"Compatible"}))?;
        a.state["prefs"]["engine_mode"] = json!("Compatible");
        a.update()?;
        a.photo("Recognition-mode-compatible")?;
        // The scroll offset stays at the previous bottom. The new switch is
        // centered in the 57 px row below the mode group and its 12 px gap.
        let unknown_probe = (475., 620.);
        a.click(unknown_probe.0, unknown_probe.1)?;
        a.photo("Unknown-probe-cancel")?;
        a.key(27)?;
        require(
            a.p.lines.try_recv().is_err(),
            "cancelling unknown device probing sent a command",
        )?;
        a.click(unknown_probe.0, unknown_probe.1)?;
        a.primary("Unknown-probe-confirm")?;
        a.p.json(|v| v == &json!({"SetUnknownProbe":true}))?;
        a.state["prefs"]["probe_unknown_devices"] = json!(true);
        a.state["probe_diagnostics"] = json!([{
            "bus_scope": "Usb",
            "vid": 0x0c45,
            "pid": 0x9999,
            "usage_page": 0xffb0,
            "usage": 1,
            "variant": "SparkLinkV2Wireless",
            "state": "confirmed",
            "reason": null
        }]);
        a.update()?;
        a.photo("Unknown-probe-enabled")?;
        native::wheel(a.hwnd, -360, 4)?;
        a.photo("Unknown-probe-bottom")?;
        a.click(345., 610.)?;
        pause(120);
        require(
            a.p.lines.try_recv().is_err(),
            "copying probe diagnostics sent an unexpected IPC command",
        )?;
        let clip = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", "Get-Clipboard -Raw"])
            .output()?;
        let clip_text = String::from_utf8_lossy(&clip.stdout);
        require(
            clip_text.contains("[Usb] 0c45:9999 up=0xffb0 usage=0x0001 variant=SparkLinkV2Wireless state=confirmed"),
            "clipboard did not contain expected redacted probe diagnostics",
        )?;
        a.photo("Probe-diagnostics-copied")?;
        passed.push("copy redacted probe diagnostics");
        // 关闭未知设备探测开关并验证关闭后诊断按钮隐藏。
        a.exact(475., 552., json!({"SetUnknownProbe":false}))?;
        a.state["prefs"]["probe_unknown_devices"] = json!(false);
        a.update()?;
        a.photo("Unknown-probe-disabled-bottom")?;
        passed.push("unknown device probing confirmation, cancel and disable");
        a.exact(150., 480., json!({"SetEngineMode":"Normal"}))?;
        a.state["prefs"]["engine_mode"] = json!("Normal");
        a.update()?;
        native::wheel(a.hwnd, -360, 9)?;
        passed.push("recognition mode confirmation, cancel and return to normal");
        a.exact(270., 610., json!("CheckUpdate"))?;
        passed.push("check updates");
        a.state["update"] = json!({"Available":"0.2.0"});
        a.state["release_notes"] = json!("Fix device sleep recovery.\n修复设备休眠恢复。");
        a.state["release_size"] = json!(10485760);
        a.update()?;
        a.photo("Update-available")?;
        a.primary("Update-offer")?;
        a.p.json(|v| v == "DownloadUpdate")?;
        passed.push("signed release offer");
        a.state["update"] = json!({"Downloading":[3145728,10485760]});
        a.state["bytes_per_sec"] = json!(1048576);
        a.update()?;
        a.photo("Update-progress")?;
        a.key(27)?;
        a.click(270., 650.)?;
        a.photo("Update-progress-reopened")?;
        passed.push("reopen transfer progress");
        Ok::<_, Box<dyn std::error::Error>>(())
    })();
    if tested.is_err() && !a.hwnd.is_null() {
        let _ = a.photo("Control-failure");
    }
    fs::write(
        out.join("controls.json"),
        serde_json::to_vec_pretty(
            &json!({"ok":tested.is_ok(),"count":passed.len(),"tests":passed}),
        )?,
    )?;
    tested?;
    a.p.close()?;
    Ok(json!({"ok":true,"count":passed.len(),"tests":passed}))
}

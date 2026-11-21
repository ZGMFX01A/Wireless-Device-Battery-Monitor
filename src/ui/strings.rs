use crate::{model::Notice, update::Failure};
use battery_protocol::hid::DiscoveryWarning;

pub fn notice(en: bool, notice: &Notice) -> String {
    let key = match notice {
        Notice::HidScanComplete => "toast.hid_scan_completed",
        Notice::BluetoothScanComplete => "toast.bluetooth_scan_completed",
        Notice::DiscoveryWarnings(warnings) => {
            return warnings
                .iter()
                .map(|warning| discovery_warning(en, *warning))
                .collect::<Vec<_>>()
                .join("\n");
        }
        Notice::HidScanFailed => "toast.hid_scan_failed",
        Notice::BluetoothScanFailed => "toast.bluetooth_scan_failed",
        Notice::RefreshFailed => "toast.refresh_failed",
        Notice::Suspended => "toast.suspended",
        Notice::SettingsSaveFailed => "toast.saving_failed",
        Notice::KeyboardReconnected => "toast.keyboard_reconnected",
        Notice::KeyboardReconnectFailed => "toast.keyboard_reconnect_failed",
        Notice::PreferencesImported(pending) => {
            return label(en, "toast.preferences_imported")
                .replace("{pending}", &pending.to_string());
        }
        Notice::PreviewHardwareUnavailable => "toast.preview_hardware_unavailable",
        Notice::AutostartStatusFailed => "toast.autostart_status_failed",
        Notice::UpdateFailed(failure) => return update_failure(en, *failure).into(),
        Notice::UpdateFolderFailed => "toast.update_folder_failed",
        Notice::OperationFailed => "toast.op_failed",
    };
    label(en, key).into()
}

fn discovery_warning(en: bool, warning: DiscoveryWarning) -> String {
    let key = match warning {
        DiscoveryWarning::LogiLongEndpoint(pid) => {
            return label(en, "warning.logi_long_endpoint").replace("{pid}", &format!("{pid:04x}"));
        }
        DiscoveryWarning::LogiDeviceType => "warning.logi_device_type",
        DiscoveryWarning::OmniPairingPort => "warning.omni_pairing_port",
        DiscoveryWarning::OmniPairingList => "warning.omni_pairing_list",
        DiscoveryWarning::OmniDeviceEndpoint => "warning.omni_device_endpoint",
    };
    label(en, key).into()
}

pub fn update_failure(en: bool, failure: Failure) -> &'static str {
    label(
        en,
        match failure {
            Failure::Check => "update.check_failed",
            Failure::Download => "update.download_failed",
            Failure::Install => "update.install_failed",
            Failure::Handoff => "update.handoff_failed",
            Failure::Preview => "update.preview_unavailable",
            Failure::Unavailable => "update.unavailable",
            Failure::AutomaticInstallPaused => "update.automatic_install_paused",
        },
    )
}

// Generated from the two dictionaries in legacy web_ui/locale.js.
pub fn label(en: bool, key: &str) -> &'static str {
    let (z, e) = match key {
        "settings.glass.title" => ("磨砂通透度", "Glass Transparency"),
        "settings.glass.subtitle" => (
            "仅调节背景，数值越大越通透",
            "Background only; higher is clearer",
        ),
        "settings.glass.less" => ("降低通透度", "Less transparent"),
        "settings.glass.more" => ("提高通透度", "More transparent"),
        "settings.auto_refresh.title" => ("自动刷新", "Auto Refresh"),
        "settings.poll_interval.title" => ("电量轮询间隔", "Battery Poll Interval"),
        "settings.poll_interval.subtitle" => (
            "在线、离线设备均按此间隔尝试读取",
            "Check online and offline devices at this interval",
        ),
        "settings.seconds" => ("秒", "s"),
        "settings.auto_refresh.subtitle" => (
            "仅控制此窗口的电量状态同步",
            "Synchronize battery status in this window",
        ),
        "settings.language.title" => ("界面语言 (Language)", "Interface Language"),
        "settings.language.subtitle" => (
            "实时切换中文、英文或跟随系统",
            "Chinese, English, or system preference",
        ),
        "device.connecting" => (
            "正在连接并读取设备电量，可关闭弹窗继续等待",
            "Connecting and reading battery. You can close this dialog.",
        ),
        "device.bound" => ("已添加", "Added"),
        "device.ready" => ("就绪", "Ready"),
        "action.close" => ("收起", "Close"),
        "toast.saving_failed" => ("设置保存失败", "Failed to save settings."),
        "app.header_title" => ("无线设备电量", "Device Battery"),
        "app.already_running" => (
            "应用已在运行。请先从托盘退出。",
            "The app is already running. Exit it from the tray first.",
        ),
        "app.start_failed" => (
            "无法启动无线设备电量监控。",
            "Could not start Device Battery.",
        ),
        "app.smoke_failed" => ("应用自检失败。", "The application self-test failed."),
        "app.unknown_option" => (
            "未知选项。请使用 --preview、--demo-tray 或 --smoke-test。",
            "Unknown option. Use --preview, --demo-tray or --smoke-test.",
        ),
        "action.scan" => ("重新扫描", "Rescan"),
        "action.scan_title" => (
            "重新扫描所有 2.4G 接收器与 HID 外设",
            "Rescan 2.4G receivers & HID devices",
        ),
        "action.add_keyboard" => ("添加键盘", "Add Keyboard"),
        "action.add_keyboard_title" => (
            "扫描并绑定 2.4G 接收器键盘",
            "Scan & bind 2.4G wireless keyboard",
        ),
        "action.add_bluetooth" => ("添加蓝牙设备", "Add Bluetooth"),
        "action.add_bluetooth_title" => (
            "扫描并绑定 Windows 已配对蓝牙设备",
            "Scan & bind paired Bluetooth devices",
        ),
        "action.check_update" => ("检查更新", "Check Updates"),
        "tray.menu.refresh_now" => ("立即刷新", "Refresh Now"),
        "tray.menu.no_device" => ("暂未发现连接的无线设备", "No Connected Devices Found"),
        "device.offline_no_data" => (
            "请确认 2.4G 接收器已插入或蓝牙已配对",
            "Ensure 2.4G receiver is inserted or Bluetooth is paired",
        ),
        "device.mouse" => ("2.4G 无线鼠标", "Wireless Mouse"),
        "device.keyboard" => ("无线键盘", "Wireless Keyboard"),
        "device.bluetooth" => ("蓝牙外设", "Bluetooth Device"),
        "device.offline" => ("已离线", "Offline"),
        "device.online" => ("正常", "Online"),
        "device.charging" => ("⚡ 充电中", "⚡ Charging"),
        "device.wait_update" => ("等待更新", "Waiting for update"),
        "device.updated_at" => ("更新于 {time}", "Updated at {time}"),
        "device.remove_btn" => ("移除", "Remove"),
        "device.unbind_btn" => ("解除绑定", "Unbind"),
        "device.rename_btn" => ("修改设备名称", "Rename device"),
        "settings.title" => ("偏好设置", "Settings"),
        "about.homepage" => ("项目主页 ↗", "Project Homepage ↗"),
        "settings.tray_style.title" => ("托盘图标样式", "Tray Icon Style"),
        "settings.tray_style.subtitle" => (
            "选择胶囊电池、极简纯数字或圆环",
            "Choose capsule battery, minimal number, or circle",
        ),
        "settings.tray_style.capsule" => ("胶囊", "Capsule"),
        "settings.tray_style.vertical" => ("竖向", "Vertical"),
        "settings.tray_style.number" => ("纯数字", "Number"),
        "settings.tray_style.circle" => ("圆环", "Circle"),
        "settings.tray_priority.title" => ("多设备显示优先级", "Tray Display Priority"),
        "settings.tray_priority.subtitle" => (
            "托盘优先展示哪台设备的电量",
            "Device to show in the system tray",
        ),
        "settings.tray_priority.mouse_first" => ("优先鼠标", "Mouse First"),
        "settings.tray_priority.keyboard_first" => ("优先键盘", "Keyboard First"),
        "settings.tray_priority.lowest_battery" => ("低电量优先", "Lowest First"),
        "settings.low_battery.title" => ("低电量提醒", "Low Battery Alert"),
        "settings.low_battery.subtitle" => (
            "低于设定百分比时发送系统通知",
            "Show system notification when battery is low",
        ),
        "settings.off" => ("关闭", "Off"),
        "settings.auto_update.title" => ("自动检查更新", "Auto Update Check"),
        "settings.auto_update.subtitle" => (
            "启动时自动检测新版本并静默升级",
            "Automatically check updates at startup",
        ),
        "settings.autostart.title" => ("开机自启动", "Launch at Startup"),
        "settings.autostart.subtitle" => (
            "开机后在后台静默运行",
            "Run quietly in background on Windows startup",
        ),
        "settings.theme.title" => ("外观风格", "Appearance"),
        "settings.theme.subtitle" => (
            "跟随系统、浅色玻璃或暗夜磨砂",
            "System, Light Glass, or Dark Frosted",
        ),
        "settings.theme.auto" => ("跟随系统", "System"),
        "settings.theme.light" => ("浅色", "Light"),
        "settings.theme.dark" => ("暗夜", "Dark"),
        "dialog.ok" => ("确定", "OK"),
        "dialog.cancel" => ("取消", "Cancel"),
        "dialog.connect" => ("连接", "Connect"),
        "dialog.add" => ("添加", "Add"),
        "dialog.remove" => ("确认移除", "Remove"),
        "keyboard.dialog.title" => ("选择键盘设备", "Select Keyboard Device"),
        "keyboard.dialog.subtitle" => (
            "当前列表已按规则去重，并保留键盘最可信控制接口",
            "Filtered candidate list with reliable control interfaces",
        ),
        "keyboard.dialog.loading" => ("正在扫描键盘候选设备...", "Scanning keyboard candidates..."),
        "keyboard.dialog.empty" => (
            "未发现可绑定的键盘候选设备（请确认 2.4G 接收器已插入）",
            "No bindable keyboard candidates found (check 2.4G receiver)",
        ),
        "keyboard.remove.title" => ("移除键盘", "Remove Keyboard"),
        "keyboard.remove.message" => (
            "确认移除当前绑定的键盘吗？移除后卡片将消失。",
            "Remove the currently bound keyboard? The card will disappear.",
        ),
        "bluetooth.dialog.title" => ("添加蓝牙设备", "Add Bluetooth Device"),
        "bluetooth.dialog.subtitle" => (
            "支持公开标准 BLE 电量服务（0x180F）及经典蓝牙耳机",
            "Supports standard BLE Battery Service (0x180F) & Classic Headsets",
        ),
        "bluetooth.dialog.loading" => (
            "正在读取 Windows 已配对蓝牙设备...",
            "Reading paired Bluetooth devices...",
        ),
        "bluetooth.dialog.empty" => (
            "未发现已配对蓝牙设备（请先在 Windows 设置中配对蓝牙）",
            "No paired Bluetooth devices found (pair in Windows Settings first)",
        ),
        "bluetooth.remove.title" => ("移除蓝牙设备", "Remove Bluetooth Device"),
        "bluetooth.remove.message" => (
            "确认移除该蓝牙设备吗？之后可从已配对列表重新添加。",
            "Remove this Bluetooth device? You can re-add it anytime.",
        ),
        "toast.scan_started" => ("正在重新扫描硬件设备...", "Scanning hardware devices..."),
        "toast.scan_completed" => ("硬件设备扫描完成", "Hardware scan completed"),
        "toast.hid_scan_completed" => ("接收器扫描完成", "Receiver scan completed"),
        "toast.bluetooth_scan_completed" => ("蓝牙扫描完成", "Bluetooth scan completed"),
        "toast.hid_scan_failed" => ("接收器扫描失败", "Receiver scan failed."),
        "toast.bluetooth_scan_failed" => ("蓝牙扫描失败", "Bluetooth scan failed."),
        "toast.refresh_started" => ("正在刷新电量...", "Refreshing battery status..."),
        "toast.refresh_completed" => ("电量刷新完成", "Battery status refreshed"),
        "toast.refresh_failed" => (
            "无法刷新设备电量",
            "Could not refresh device battery status.",
        ),
        "toast.suspended" => (
            "系统已暂停，请在唤醒后重试。",
            "System suspended. Try again after waking it.",
        ),
        "toast.keyboard_reconnected" => ("键盘已自动恢复连接", "Keyboard reconnected"),
        "toast.keyboard_reconnect_failed" => {
            ("无法重新连接键盘", "Could not reconnect the keyboard.")
        }
        "toast.preferences_imported" => (
            "已导入偏好与匹配的设备，待匹配 {pending} 项",
            "Preferences and matching devices imported. Pending matches: {pending}.",
        ),
        "toast.preview_hardware_unavailable" => {
            ("预览模式不访问外设", "Preview does not access hardware.")
        }
        "toast.autostart_status_failed" => (
            "无法读取开机自启动状态",
            "Could not read the launch-at-startup status.",
        ),
        "toast.update_folder_failed" => ("无法打开更新文件夹", "Could not open the update folder."),
        "toast.bound_success" => ("设备绑定成功", "Device bound successfully"),
        "toast.removed_success" => ("已移除设备", "Device removed"),
        "toast.rename_success" => ("设备名称已修改为“{name}”", "Device renamed to \"{name}\""),
        "toast.rename_cleared" => ("已恢复设备默认名称", "Device name reset to default"),
        "toast.op_failed" => ("操作失败，请重试。", "Operation failed. Please try again."),
        "device.add_failed" => (
            "无法添加设备，请检查连接后重试。",
            "Could not add the device. Check its connection and try again.",
        ),
        "warning.logi_long_endpoint" => (
            "HID++ {pid}：无法确定唯一的长报文端点",
            "HID++ {pid}: could not identify a unique long-report endpoint.",
        ),
        "warning.logi_device_type" => (
            "HID++ C54D：暂时无法读取设备类型",
            "HID++ C54D: device type unavailable.",
        ),
        "warning.omni_pairing_port" => (
            "ROG Omni：无法打开配对通道",
            "ROG Omni: could not open the pairing interface.",
        ),
        "warning.omni_pairing_list" => (
            "ROG Omni：无法读取配对列表",
            "ROG Omni: pairing list unavailable.",
        ),
        "warning.omni_device_endpoint" => (
            "ROG Omni：无法确定唯一的设备端点",
            "ROG Omni: could not identify a unique device endpoint.",
        ),
        "window.start_timeout" => (
            "窗口启动超时。后台监控继续运行，可从托盘重试。",
            "The window took too long to open. Monitoring continues. Retry from the tray.",
        ),
        "window.start_failed" => (
            "无法打开窗口。后台监控继续运行。",
            "Could not open the window. Background monitoring continues.",
        ),
        "window.exited" => (
            "窗口意外关闭。后台监控继续运行，可从托盘重新打开。",
            "The window closed unexpectedly. Monitoring continues. Reopen it from the tray.",
        ),
        "update.modal_title" => ("发现新版本", "New Version Found"),
        "update.notes_title" => ("更新说明", "Release Notes"),
        "update.later" => ("稍后再说", "Later"),
        "update.start" => ("立即更新", "Update Now"),
        "update.cancel" => ("取消下载", "Cancel Download"),
        "update.cancelling" => ("正在取消下载...", "Cancelling download..."),
        "update.check_failed" => ("检查更新失败", "Could not check for updates."),
        "update.download_failed" => ("下载更新失败", "Could not download the update."),
        "update.install_failed" => ("安装更新失败", "Could not install the update."),
        "update.handoff_failed" => (
            "更新交接失败，当前版本继续运行。",
            "Could not hand off the update. The current version is still running.",
        ),
        "update.preview_unavailable" => (
            "预览模式不访问更新服务",
            "Preview does not contact the update service.",
        ),
        "update.unavailable" => (
            "此版本不支持自动更新，请从发布页下载。",
            "Automatic updates are unavailable in this build. Download from the releases page.",
        ),
        "update.automatic_install_paused" => (
            "上次更新启动失败，自动安装已暂停。你可以手动重试。",
            "This update previously failed to start. Automatic installation is paused. You can retry manually.",
        ),
        "update.latest" => (
            "当前已是最新版本 ({version})",
            "Already up to date ({version})",
        ),
        "update.connecting_official" => {
            ("正在连接官方更新源...", "Connecting to official source...")
        }
        "update.connecting_mirror" => (
            "正在连接高速镜像更新源...",
            "Connecting to mirror source...",
        ),
        "update.downloading" => (
            "正在下载更新 ({percent}%)",
            "Downloading update ({percent}%)",
        ),
        "update.verifying" => (
            "正在校验文件哈希 (SHA-256)...",
            "Verifying hash integrity (SHA-256)...",
        ),
        "update.applying" => (
            "正在启动安全替换并重启...",
            "Applying update & restarting...",
        ),
        "update.retrying" => (
            "下载连接中断，正在重试...",
            "Download interrupted, retrying...",
        ),
        "update.fallback" => (
            "已自动切换至备用下载源...",
            "Switched to fallback source...",
        ),
        "update.cancelled" => ("更新已取消", "Update cancelled"),
        _ => ("—", "—"),
    };
    if en { e } else { z }
}

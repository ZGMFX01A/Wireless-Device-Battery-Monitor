// SPDX-FileCopyrightText: 2026 ZGMFX01A
// SPDX-License-Identifier: LicenseRef-Wireless-Device-Battery-Monitor
// See LICENSE in the repository root for the full terms.

use crate::{model::Notice, update::Failure};
use battery_protocol::hid::DiscoveryWarning;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locale {
    Zh,
    En,
    Ru,
}

impl Locale {
    pub fn from_windows_language(id: u16) -> Self {
        match id & 0x3ff {
            4 => Self::Zh,
            0x19 => Self::Ru,
            _ => Self::En,
        }
    }
}

pub fn notice(locale: Locale, notice: &Notice) -> String {
    let key = match notice {
        Notice::HidScanComplete => "toast.hid_scan_completed",
        Notice::BluetoothScanComplete => "toast.bluetooth_scan_completed",
        Notice::DiscoveryWarnings(warnings) => {
            return warnings
                .iter()
                .map(|warning| discovery_warning(locale, *warning))
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
            return label(locale, "toast.preferences_imported")
                .replace("{pending}", &pending.to_string());
        }
        Notice::PreviewHardwareUnavailable => "toast.preview_hardware_unavailable",
        Notice::AutostartStatusFailed => "toast.autostart_status_failed",
        Notice::UpdateFailed(failure) => return update_failure(locale, *failure).into(),
        Notice::UpdateFolderFailed => "toast.update_folder_failed",
        Notice::OperationFailed => "toast.op_failed",
    };
    label(locale, key).into()
}

fn discovery_warning(locale: Locale, warning: DiscoveryWarning) -> String {
    let key = match warning {
        DiscoveryWarning::LogiLongEndpoint(pid) => {
            return label(locale, "warning.logi_long_endpoint")
                .replace("{pid}", &format!("{pid:04x}"));
        }
        DiscoveryWarning::LogiDeviceType => "warning.logi_device_type",
        DiscoveryWarning::OmniPairingPort => "warning.omni_pairing_port",
        DiscoveryWarning::OmniPairingList => "warning.omni_pairing_list",
        DiscoveryWarning::OmniDeviceEndpoint => "warning.omni_device_endpoint",
    };
    label(locale, key).into()
}

pub fn update_failure(locale: Locale, failure: Failure) -> &'static str {
    label(
        locale,
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

// UI, native tray and notification text share the same complete dictionaries.
pub fn label(locale: Locale, key: &str) -> &'static str {
    let (z, e, r) = match key {
        "settings.glass.title" => ("磨砂通透度", "Glass Transparency", "Прозрачность стекла"),
        "settings.glass.subtitle" => (
            "仅调节背景，数值越大越通透",
            "Background only; higher is clearer",
            "Только фон: чем больше, тем прозрачнее",
        ),
        "settings.glass.less" => ("降低通透度", "Less transparent", "Менее прозрачно"),
        "settings.glass.more" => ("提高通透度", "More transparent", "Более прозрачно"),
        "settings.auto_refresh.title" => ("自动刷新", "Auto Refresh", "Обновление в окне"),
        "settings.poll_interval.title" => (
            "电量轮询间隔",
            "Battery Poll Interval",
            "Интервал опроса батареи",
        ),
        "settings.poll_interval.subtitle" => (
            "在线、离线设备均按此间隔尝试读取",
            "Check online and offline devices at this interval",
            "Проверять устройства в сети и не в сети с этим интервалом",
        ),
        "settings.seconds" => ("秒", "s", "с"),
        "settings.auto_refresh.subtitle" => (
            "仅控制此窗口的电量状态同步",
            "Synchronize battery status in this window",
            "Синхронизировать заряд в этом окне",
        ),
        "settings.language.title" => (
            "界面语言 (Language)",
            "Interface Language",
            "Язык интерфейса",
        ),
        "settings.language.subtitle" => (
            "实时切换中文、英文、俄文或跟随系统",
            "Chinese, English, Russian, or system preference",
            "Китайский, английский, русский или как в системе",
        ),
        "device.connecting" => (
            "正在连接并读取设备电量，可关闭弹窗继续等待",
            "Connecting and reading battery. You can close this dialog.",
            "Подключение и чтение заряда. Это окно можно закрыть.",
        ),
        "display.device.mouse" => ("鼠标", "Mouse", "Мышь"),
        "display.device.keyboard" => ("键盘", "Keyboard", "Клавиатура"),
        "display.device.headset" => ("耳机", "Headset", "Гарнитура"),
        "display.device.bluetooth" => ("蓝牙设备", "Bluetooth device", "Устройство Bluetooth"),
        "display.device.receiver24g" => ("2.4G 接收器", "2.4G receiver", "Приёмник 2,4 ГГц"),
        "display.device.unknown" => ("未知设备", "Unknown device", "Неизвестное устройство"),
        "display.channel.mouse" => ("鼠标通道", "Mouse channel", "Канал мыши"),
        "display.channel.keyboard" => ("键盘通道", "Keyboard channel", "Канал клавиатуры"),
        "device.bound" => ("已添加", "Added", "Добавлено"),
        "device.ready" => ("就绪", "Ready", "Готово"),
        "device.paired" => ("已配对", "Paired", "Сопряжено"),
        "action.close" => ("收起", "Close", "Закрыть"),
        "toast.saving_failed" => (
            "设置保存失败",
            "Failed to save settings.",
            "Не удалось сохранить настройки.",
        ),
        "app.header_title" => ("无线设备电量", "Device Battery", "Заряд устройств"),
        "app.already_running" => (
            "应用已在运行。请先从托盘退出。",
            "The app is already running. Exit it from the tray first.",
            "Приложение уже запущено. Сначала выйдите из него через трей.",
        ),
        "app.start_failed" => (
            "无法启动无线设备电量监控。",
            "Could not start Device Battery.",
            "Не удалось запустить Device Battery.",
        ),
        "app.smoke_failed" => (
            "应用自检失败。",
            "The application self-test failed.",
            "Самопроверка приложения не пройдена.",
        ),
        "app.unknown_option" => (
            "未知选项。请使用 --preview、--demo-tray 或 --smoke-test。",
            "Unknown option. Use --preview, --demo-tray or --smoke-test.",
            "Неизвестный параметр. Используйте --preview, --demo-tray или --smoke-test.",
        ),
        "action.scan" => ("重新扫描", "Rescan", "Поиск"),
        "action.scan_title" => (
            "重新扫描所有 2.4G 接收器与 HID 外设",
            "Rescan 2.4G receivers & HID devices",
            "Повторно сканировать 2.4G-приёмники и HID-устройства",
        ),
        "action.add_keyboard" => ("添加键盘", "Add Keyboard", "Клавиатура"),
        "action.add_keyboard_title" => (
            "扫描并绑定 2.4G 接收器键盘",
            "Scan & bind 2.4G wireless keyboard",
            "Найти и привязать беспроводную 2.4G-клавиатуру",
        ),
        "action.add_bluetooth" => ("添加蓝牙设备", "Add Bluetooth", "Bluetooth"),
        "action.add_bluetooth_title" => (
            "扫描并绑定 Windows 已配对蓝牙设备",
            "Scan & bind paired Bluetooth devices",
            "Найти и привязать сопряжённые Bluetooth-устройства",
        ),
        "action.check_update" => ("检查更新", "Check Updates", "Проверить обновления"),
        "tray.menu.refresh_now" => ("立即刷新", "Refresh Now", "Обновить сейчас"),
        "tray.menu.no_device" => (
            "暂未发现连接的无线设备",
            "No Connected Devices Found",
            "Подключённые устройства не найдены",
        ),
        "device.offline_no_data" => (
            "请确认 2.4G 接收器已插入或蓝牙已配对",
            "Ensure 2.4G receiver is inserted or Bluetooth is paired",
            "Убедитесь, что 2.4G-приёмник вставлен или Bluetooth-устройство сопряжено",
        ),
        "device.mouse" => ("2.4G 无线鼠标", "Wireless Mouse", "Беспроводная мышь"),
        "device.keyboard" => ("无线键盘", "Wireless Keyboard", "Беспроводная клавиатура"),
        "device.bluetooth" => ("蓝牙外设", "Bluetooth Device", "Bluetooth-устройство"),
        "device.audio" => (
            "无线音频设备",
            "Wireless Audio",
            "Беспроводное аудиоустройство",
        ),
        "device.offline" => ("已离线", "Offline", "Не в сети"),
        "device.online" => ("正常", "Online", "В сети"),
        "device.charging" => ("⚡ 充电中", "⚡ Charging", "⚡ Заряжается"),
        "device.wait_update" => ("等待更新", "Waiting for update", "Ожидание обновления"),
        "device.updated_at" => ("更新于 {time}", "Updated at {time}", "Обновлено в {time}"),
        "device.remove_btn" => ("移除", "Remove", "Удалить"),
        "device.unbind_btn" => ("解除绑定", "Unbind", "Отвязать"),
        "device.rename_btn" => ("修改设备名称", "Rename device", "Переименовать устройство"),
        "settings.title" => ("偏好设置", "Settings", "Настройки"),
        "about.homepage" => ("项目主页 ↗", "Project Homepage ↗", "Страница проекта ↗"),
        "settings.tray_style.title" => ("托盘图标样式", "Tray Icon Style", "Стиль значка в трее"),
        "settings.tray_style.subtitle" => (
            "选择胶囊电池、极简纯数字或圆环",
            "Choose capsule battery, minimal number, or circle",
            "Капсула, вертикальная батарея, число или круг",
        ),
        "settings.tray_style.capsule" => ("胶囊", "Capsule", "Капсула"),
        "settings.tray_style.vertical" => ("竖向", "Vertical", "Столбик"),
        "settings.tray_style.number" => ("纯数字", "Number", "Число"),
        "settings.tray_style.circle" => ("圆环", "Circle", "Круг"),
        "settings.tray_priority.title" => (
            "多设备显示优先级",
            "Tray Display Priority",
            "Приоритет в трее",
        ),
        "settings.tray_priority.subtitle" => (
            "托盘优先展示哪台设备的电量",
            "Device to show in the system tray",
            "Какое устройство показывать в системном трее",
        ),
        "settings.tray_priority.mouse_first" => ("优先鼠标", "Mouse First", "Мышь"),
        "settings.tray_priority.keyboard_first" => ("优先键盘", "Keyboard First", "Клавиатура"),
        "settings.tray_priority.lowest_battery" => ("低电量优先", "Lowest First", "Меньший заряд"),
        "settings.low_battery.title" => (
            "低电量提醒",
            "Low Battery Alert",
            "Уведомление о низком заряде",
        ),
        "settings.low_battery.subtitle" => (
            "低于设定百分比时发送系统通知",
            "Show system notification when battery is low",
            "Показывать системное уведомление при низком заряде",
        ),
        "settings.off" => ("关闭", "Off", "Выкл."),
        "settings.auto_update.title" => {
            ("自动检查更新", "Auto Update Check", "Проверка обновлений")
        }
        "settings.auto_update.subtitle" => (
            "启动时自动检测新版本并静默升级",
            "Automatically check updates at startup",
            "Проверять обновления при запуске",
        ),
        "settings.autostart.title" => ("开机自启动", "Launch at Startup", "Запуск с Windows"),
        "settings.autostart.subtitle" => (
            "开机后在后台静默运行",
            "Run quietly in background on Windows startup",
            "Тихо работать в фоне после запуска Windows",
        ),
        "settings.theme.title" => ("外观风格", "Appearance", "Оформление"),
        "settings.theme.subtitle" => (
            "跟随系统、浅色玻璃或暗夜磨砂",
            "System, Light Glass, or Dark Frosted",
            "Системное, светлое стекло или тёмное матовое",
        ),
        "settings.theme.auto" => ("跟随系统", "System", "Система"),
        "settings.theme.light" => ("浅色", "Light", "Светлое"),
        "settings.theme.dark" => ("暗夜", "Dark", "Тёмное"),
        "dialog.ok" => ("确定", "OK", "ОК"),
        "dialog.cancel" => ("取消", "Cancel", "Отмена"),
        "dialog.connect" => ("连接", "Connect", "Подключить"),
        "dialog.add" => ("添加", "Add", "Добавить"),
        "dialog.remove" => ("确认移除", "Remove", "Удалить"),
        "keyboard.dialog.title" => ("选择键盘设备", "Select Keyboard Device", "Выбор клавиатуры"),
        "keyboard.dialog.subtitle" => (
            "当前列表已按规则去重，并保留键盘最可信控制接口",
            "Filtered candidate list with reliable control interfaces",
            "Список кандидатов с надёжными интерфейсами управления",
        ),
        "keyboard.dialog.loading" => (
            "正在扫描键盘候选设备...",
            "Scanning keyboard candidates...",
            "Поиск клавиатур...",
        ),
        "keyboard.dialog.empty" => (
            "未发现可绑定的键盘候选设备（请确认 2.4G 接收器已插入）",
            "No bindable keyboard candidates found (check 2.4G receiver)",
            "Подходящие клавиатуры не найдены (проверьте 2.4G-приёмник)",
        ),
        "keyboard.remove.title" => ("移除键盘", "Remove Keyboard", "Удалить клавиатуру"),
        "keyboard.remove.message" => (
            "确认移除当前绑定的键盘吗？移除后卡片将消失。",
            "Remove the currently bound keyboard? The card will disappear.",
            "Удалить привязанную клавиатуру? Её карточка исчезнет.",
        ),
        "bluetooth.dialog.title" => (
            "添加蓝牙设备",
            "Add Bluetooth Device",
            "Добавить Bluetooth-устройство",
        ),
        "bluetooth.dialog.subtitle" => (
            "支持公开标准 BLE 电量服务（0x180F）及经典蓝牙耳机",
            "Supports standard BLE Battery Service (0x180F) & Classic Headsets",
            "Поддерживаются стандартная служба батареи BLE (0x180F) и классические гарнитуры",
        ),
        "bluetooth.dialog.loading" => (
            "正在读取 Windows 已配对蓝牙设备...",
            "Reading paired Bluetooth devices...",
            "Чтение сопряжённых Bluetooth-устройств...",
        ),
        "bluetooth.dialog.empty" => (
            "未发现已配对蓝牙设备（请先在 Windows 设置中配对蓝牙）",
            "No paired Bluetooth devices found (pair in Windows Settings first)",
            "Сопряжённые Bluetooth-устройства не найдены (сначала выполните сопряжение в настройках Windows)",
        ),
        "bluetooth.remove.title" => (
            "移除蓝牙设备",
            "Remove Bluetooth Device",
            "Удалить Bluetooth-устройство",
        ),
        "bluetooth.remove.message" => (
            "确认移除该蓝牙设备吗？之后可从已配对列表重新添加。",
            "Remove this Bluetooth device? You can re-add it anytime.",
            "Удалить это Bluetooth-устройство? Его можно будет добавить снова в любой момент.",
        ),
        "toast.scan_started" => (
            "正在重新扫描硬件设备...",
            "Scanning hardware devices...",
            "Сканирование устройств...",
        ),
        "toast.scan_completed" => (
            "硬件设备扫描完成",
            "Hardware scan completed",
            "Сканирование устройств завершено",
        ),
        "toast.hid_scan_completed" => (
            "接收器扫描完成",
            "Receiver scan completed",
            "Сканирование приёмников завершено",
        ),
        "toast.bluetooth_scan_completed" => (
            "蓝牙扫描完成",
            "Bluetooth scan completed",
            "Сканирование Bluetooth завершено",
        ),
        "toast.hid_scan_failed" => (
            "接收器扫描失败",
            "Receiver scan failed.",
            "Не удалось просканировать приёмники.",
        ),
        "toast.bluetooth_scan_failed" => (
            "蓝牙扫描失败",
            "Bluetooth scan failed.",
            "Не удалось просканировать Bluetooth.",
        ),
        "toast.refresh_started" => (
            "正在刷新电量...",
            "Refreshing battery status...",
            "Обновление заряда...",
        ),
        "toast.refresh_completed" => ("电量刷新完成", "Battery status refreshed", "Заряд обновлён"),
        "toast.refresh_failed" => (
            "无法刷新设备电量",
            "Could not refresh device battery status.",
            "Не удалось обновить заряд устройств.",
        ),
        "toast.suspended" => (
            "系统已暂停，请在唤醒后重试。",
            "System suspended. Try again after waking it.",
            "Система в спящем режиме. Повторите после пробуждения.",
        ),
        "toast.keyboard_reconnected" => (
            "键盘已自动恢复连接",
            "Keyboard reconnected",
            "Клавиатура переподключена",
        ),
        "toast.keyboard_reconnect_failed" => (
            "无法重新连接键盘",
            "Could not reconnect the keyboard.",
            "Не удалось переподключить клавиатуру.",
        ),
        "toast.preferences_imported" => (
            "已导入偏好与匹配的设备，待匹配 {pending} 项",
            "Preferences and matching devices imported. Pending matches: {pending}.",
            "Настройки и подходящие устройства импортированы. Ожидают сопоставления: {pending}.",
        ),
        "toast.preview_hardware_unavailable" => (
            "预览模式不访问外设",
            "Preview does not access hardware.",
            "Предпросмотр не обращается к устройствам.",
        ),
        "toast.autostart_status_failed" => (
            "无法读取开机自启动状态",
            "Could not read the launch-at-startup status.",
            "Не удалось прочитать состояние автозапуска.",
        ),
        "toast.update_folder_failed" => (
            "无法打开更新文件夹",
            "Could not open the update folder.",
            "Не удалось открыть папку обновления.",
        ),
        "toast.bound_success" => (
            "设备绑定成功",
            "Device bound successfully",
            "Устройство привязано",
        ),
        "toast.removed_success" => ("已移除设备", "Device removed", "Устройство удалено"),
        "toast.rename_success" => (
            "设备名称已修改为“{name}”",
            "Device renamed to \"{name}\"",
            "Устройство переименовано в «{name}»",
        ),
        "toast.rename_cleared" => (
            "已恢复设备默认名称",
            "Device name reset to default",
            "Имя устройства сброшено",
        ),
        "toast.op_failed" => (
            "操作失败，请重试。",
            "Operation failed. Please try again.",
            "Операция не удалась. Повторите попытку.",
        ),
        "device.add_failed" => (
            "无法添加设备，请检查连接后重试。",
            "Could not add the device. Check its connection and try again.",
            "Не удалось добавить устройство. Проверьте подключение и повторите.",
        ),
        "warning.logi_long_endpoint" => (
            "HID++ {pid}：无法确定唯一的长报文端点",
            "HID++ {pid}: could not identify a unique long-report endpoint.",
            "HID++ {pid}: не удалось определить единственный длинный отчётный интерфейс.",
        ),
        "warning.logi_device_type" => (
            "HID++ C54D：暂时无法读取设备类型",
            "HID++ C54D: device type unavailable.",
            "HID++ C54D: тип устройства недоступен.",
        ),
        "warning.omni_pairing_port" => (
            "ROG Omni：无法打开配对通道",
            "ROG Omni: could not open the pairing interface.",
            "ROG Omni: не удалось открыть интерфейс сопряжения.",
        ),
        "warning.omni_pairing_list" => (
            "ROG Omni：无法读取配对列表",
            "ROG Omni: pairing list unavailable.",
            "ROG Omni: список сопряжения недоступен.",
        ),
        "warning.omni_device_endpoint" => (
            "ROG Omni：无法确定唯一的设备端点",
            "ROG Omni: could not identify a unique device endpoint.",
            "ROG Omni: не удалось определить единственный интерфейс устройства.",
        ),
        "window.start_timeout" => (
            "窗口启动超时。后台监控继续运行，可从托盘重试。",
            "The window took too long to open. Monitoring continues. Retry from the tray.",
            "Окно открывается слишком долго. Мониторинг продолжается. Повторите из трея.",
        ),
        "window.start_failed" => (
            "无法打开窗口。后台监控继续运行。",
            "Could not open the window. Background monitoring continues.",
            "Не удалось открыть окно. Фоновый мониторинг продолжается.",
        ),
        "window.exited" => (
            "窗口意外关闭。后台监控继续运行，可从托盘重新打开。",
            "The window closed unexpectedly. Monitoring continues. Reopen it from the tray.",
            "Окно неожиданно закрылось. Мониторинг продолжается. Откройте его снова из трея.",
        ),
        "update.modal_title" => ("发现新版本", "New Version Found", "Найдена новая версия"),
        "update.notes_title" => ("更新说明", "Release Notes", "Примечания к выпуску"),
        "update.later" => ("稍后再说", "Later", "Позже"),
        "update.start" => ("立即更新", "Update Now", "Обновить сейчас"),
        "update.cancel" => ("取消下载", "Cancel Download", "Отменить загрузку"),
        "update.cancelling" => (
            "正在取消下载...",
            "Cancelling download...",
            "Отмена загрузки...",
        ),
        "update.check_failed" => (
            "检查更新失败",
            "Could not check for updates.",
            "Не удалось проверить обновления.",
        ),
        "update.download_failed" => (
            "下载更新失败",
            "Could not download the update.",
            "Не удалось скачать обновление.",
        ),
        "update.install_failed" => (
            "安装更新失败",
            "Could not install the update.",
            "Не удалось установить обновление.",
        ),
        "update.handoff_failed" => (
            "更新交接失败，当前版本继续运行。",
            "Could not hand off the update. The current version is still running.",
            "Не удалось передать обновление. Текущая версия продолжает работать.",
        ),
        "update.preview_unavailable" => (
            "预览模式不访问更新服务",
            "Preview does not contact the update service.",
            "Предпросмотр не обращается к службе обновлений.",
        ),
        "update.unavailable" => (
            "此版本不支持自动更新，请从发布页下载。",
            "Automatic updates are unavailable in this build. Download from the releases page.",
            "В этой сборке автообновление недоступно. Скачайте со страницы релизов.",
        ),
        "update.automatic_install_paused" => (
            "上次更新启动失败，自动安装已暂停。你可以手动重试。",
            "This update previously failed to start. Automatic installation is paused. You can retry manually.",
            "Прошлое обновление не удалось запустить. Автоустановка приостановлена. Можно повторить вручную.",
        ),
        "update.latest" => (
            "当前已是最新版本 ({version})",
            "Already up to date ({version})",
            "Установлена последняя версия ({version})",
        ),
        "update.connecting_official" => (
            "正在连接官方更新源...",
            "Connecting to official source...",
            "Подключение к официальному источнику...",
        ),
        "update.connecting_mirror" => (
            "正在连接高速镜像更新源...",
            "Connecting to mirror source...",
            "Подключение к зеркалу...",
        ),
        "update.downloading" => (
            "正在下载更新 ({percent}%)",
            "Downloading update ({percent}%)",
            "Загрузка обновления ({percent}%)",
        ),
        "update.verifying" => (
            "正在校验文件哈希 (SHA-256)...",
            "Verifying hash integrity (SHA-256)...",
            "Проверка целостности (SHA-256)...",
        ),
        "update.applying" => (
            "正在启动安全替换并重启...",
            "Applying update & restarting...",
            "Применение обновления и перезапуск...",
        ),
        "update.retrying" => (
            "下载连接中断，正在重试...",
            "Download interrupted, retrying...",
            "Загрузка прервана, повтор...",
        ),
        "update.fallback" => (
            "已自动切换至备用下载源...",
            "Switched to fallback source...",
            "Переключено на резервный источник...",
        ),
        "update.cancelled" => ("更新已取消", "Update cancelled", "Обновление отменено"),
        "about.author" => ("作者", "Built by", "Автор"),
        "brand.logitech" => ("罗技", "Logitech", "Logitech"),
        "brand.razer" => ("雷蛇", "Razer", "Razer"),
        "device.bluetooth_short" => ("蓝牙", "Bluetooth", "Bluetooth"),
        "device.mouse_short" => ("鼠标", "Mouse", "Мышь"),
        "device.audio_short" => ("音频设备", "Audio", "Аудио"),
        "device.keyboard_short" => ("键盘", "Keyboard", "Клавиатура"),
        "device.unpaired" => ("未配对", "Unpaired", "Не сопряжено"),
        "device.offline_previous" => (
            "离线 · 上次读数",
            "Offline · Previous reading",
            "Не в сети · Прошлый заряд",
        ),
        "device.previous" => ("沿用上次读数", "Previous reading", "Прежние показания"),
        "device.full" => ("已充满", "Fully charged", "Заряжено"),
        "device.charging_short" => ("充电中", "Charging", "Заряжается"),
        "settings.import" => (
            "导入旧版设置",
            "Import legacy settings",
            "Импорт старых настроек",
        ),
        "dialog.retry" => ("重试", "Retry", "Повторить"),
        "update.no_notes" => (
            "未提供更新说明",
            "No release notes provided",
            "Примечания к выпуску отсутствуют",
        ),
        "update.verified" => (
            "签名与文件校验通过",
            "Signature and file verified",
            "Подпись и файл проверены",
        ),
        "update.install_restart" => (
            "安装并重启",
            "Install & restart",
            "Установить и перезапустить",
        ),
        "update.checking" => (
            "正在检查更新…",
            "Checking for updates…",
            "Проверка обновлений…",
        ),
        "tray.settings" => ("设置", "Settings", "Настройки"),
        "tray.style" => ("图标样式", "Icon style", "Стиль значка"),
        "tray.quit" => ("退出", "Quit", "Выход"),
        "tray.no_devices" => ("暂无设备", "No devices", "Нет устройств"),
        "tray.offline" => ("离线", "Offline", "Не в сети"),
        "tray.more" => (
            "另有 {count} 个设备",
            "+{count} more devices",
            "Ещё устройств: {count}",
        ),
        _ => ("—", "—", "—"),
    };
    match locale {
        Locale::Zh => z,
        Locale::En => e,
        Locale::Ru => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Manual;

    #[test]
    fn system_language_uses_primary_language_including_russian_regions() {
        for id in [0x0419, 0x0819] {
            assert_eq!(Locale::from_windows_language(id), Locale::Ru);
        }
        for id in [0x0804, 0x0404] {
            assert_eq!(Locale::from_windows_language(id), Locale::Zh);
        }
        for id in [0x0409, 0x0407] {
            assert_eq!(Locale::from_windows_language(id), Locale::En);
        }
    }

    #[test]
    fn every_translation_is_complete_and_preserves_format_placeholders() {
        fn placeholders(text: &str) -> Vec<&str> {
            let mut result: Vec<_> = text
                .split('{')
                .skip(1)
                .map(|part| part.split('}').next().unwrap())
                .collect();
            result.sort_unstable();
            result
        }
        // Read actual dictionary arms, so future entries are checked automatically.
        let dictionary = include_str!("strings.rs")
            .split("let (z, e, r) = match key {")
            .nth(1)
            .unwrap()
            .split("_ =>")
            .next()
            .unwrap();
        let mut count = 0;
        for line in dictionary.lines() {
            if let Some(line) = line.trim().strip_prefix('"')
                && let Some((key, rest)) = line.split_once('"')
            {
                if !rest.trim_start().starts_with("=>") {
                    continue;
                }
                count += 1;
                let expected = placeholders(label(Locale::En, key));
                for lang in [Locale::Zh, Locale::En, Locale::Ru] {
                    let text = label(lang, key);
                    assert!(!text.is_empty() && text != "—", "{lang:?}: {key}");
                    assert_eq!(placeholders(text), expected, "{lang:?}: {key}");
                }
                assert!(
                    matches!(
                        key,
                        "brand.logitech"
                            | "brand.razer"
                            | "device.bluetooth_short"
                            | "action.add_bluetooth"
                    ) || label(Locale::Ru, key)
                        .chars()
                        .any(|c| ('А'..='я').contains(&c)),
                    "missing Russian: {key}"
                );
            }
        }
        assert!(count >= 150);
    }

    #[test]
    fn russian_notices_format_values_and_do_not_depend_on_other_lookups() {
        assert_eq!(
            notice(Locale::Ru, &Notice::RefreshFailed),
            "Не удалось обновить заряд устройств."
        );
        assert_eq!(
            update_failure(Locale::Ru, Failure::Download),
            "Не удалось скачать обновление."
        );
        assert!(notice(Locale::Ru, &Notice::PreferencesImported(12)).contains("12"));
        assert!(
            notice(
                Locale::Ru,
                &Notice::DiscoveryWarnings(vec![DiscoveryWarning::LogiLongEndpoint(0xc548)])
            )
            .contains("c548")
        );
        assert_eq!(label(Locale::En, "settings.title"), "Settings");
        assert_eq!(label(Locale::Ru, "settings.title"), "Настройки");
        assert_eq!(label(Locale::Zh, "settings.title"), "偏好设置");
    }

    #[test]
    fn omni_scan_warnings_survive_ipc_and_follow_the_interface_language() {
        let mut manual = Manual {
            id: 3,
            ..Default::default()
        };
        manual.finish(
            3,
            false,
            Some(Notice::DiscoveryWarnings(vec![
                DiscoveryWarning::OmniPairingList,
                DiscoveryWarning::OmniDeviceEndpoint,
            ])),
        );
        manual.finish(3, true, None);
        let received: Manual =
            serde_json::from_slice(&serde_json::to_vec(&manual).unwrap()).unwrap();
        assert!(received.completed());
        let warning = received.error.unwrap();
        assert_eq!(
            notice(Locale::En, &warning),
            "ROG Omni: pairing list unavailable.\nROG Omni: could not identify a unique device endpoint."
        );
        assert_eq!(
            notice(Locale::Zh, &warning),
            "ROG Omni：无法读取配对列表\nROG Omni：无法确定唯一的设备端点"
        );
    }

    #[test]
    fn asynchronous_errors_have_specific_labels_in_both_languages() {
        assert_eq!(
            notice(Locale::En, &Notice::RefreshFailed),
            "Could not refresh device battery status."
        );
        assert_eq!(
            notice(Locale::Zh, &Notice::RefreshFailed),
            "无法刷新设备电量"
        );
        assert_eq!(
            update_failure(Locale::En, Failure::Download),
            "Could not download the update."
        );
        assert_eq!(
            update_failure(Locale::Zh, Failure::Download),
            "下载更新失败"
        );
    }
}

const I18N = {
        'zh-CN': {
    'settings.glass.title': '磨砂通透度',
    'settings.glass.subtitle': '仅调节背景，数值越大越通透',
    'settings.glass.less': '降低通透度', 'settings.glass.more': '提高通透度',
    'settings.auto_refresh.title': '自动刷新',
    'settings.auto_refresh.subtitle': '仅控制此窗口的电量状态同步',
    'settings.language.title': '界面语言 (Language)',
    'settings.language.subtitle': '实时切换中文、英文或跟随系统',
    'device.connecting': '正在连接并读取设备电量，可关闭弹窗继续等待',
    'device.bound': '已添加', 'device.ready': '就绪',
    'action.close': '收起', 'toast.saving_failed': '设置保存失败：{msg}',
            'app.header_title': '无线设备电量',
            'action.scan': '重新扫描',
            'action.add_keyboard': '添加键盘',
            'action.add_bluetooth': '添加蓝牙设备',
            'action.check_update': '检查更新',
            'tray.menu.refresh_now': '立即刷新',
            'tray.menu.no_device': '暂未发现连接的无线设备',
            'device.offline_no_data': '请确认 2.4G 接收器已插入或蓝牙已配对',
            'device.mouse': '2.4G 无线鼠标',
            'device.keyboard': '无线键盘',
            'device.bluetooth': '蓝牙外设',
            'device.offline': '已离线',
            'device.online': '正常',
            'device.charging': '⚡ 充电中',
            'device.wait_update': '等待更新',
            'device.updated_at': '更新于 {time}',
            'device.remove_btn': '移除',
            'device.unbind_btn': '解除绑定',

            'settings.title': '偏好设置',
            'settings.tray_style.title': '托盘图标样式',
            'settings.tray_style.subtitle': '选择胶囊电池、极简纯数字或圆环',
            'settings.tray_style.capsule': '胶囊',
            'settings.tray_style.vertical': '竖向',
            'settings.tray_style.number': '纯数字',
            'settings.tray_style.circle': '圆环',

            'settings.tray_priority.title': '多设备显示优先级',
            'settings.tray_priority.subtitle': '托盘优先展示哪台设备的电量',
            'settings.tray_priority.mouse_first': '优先鼠标',
            'settings.tray_priority.keyboard_first': '优先键盘',
            'settings.tray_priority.lowest_battery': '低电量优先',

            'settings.low_battery.title': '低电量提醒',
            'settings.low_battery.subtitle': '低于设定百分比时发送系统通知',
            'settings.off': '关闭',

            'settings.auto_update.title': '自动检查更新',
            'settings.auto_update.subtitle': '启动时自动检测新版本并静默升级',

            'settings.autostart.title': '开机自启动',
            'settings.autostart.subtitle': '开机后在后台静默运行',

            'settings.theme.title': '外观风格',
            'settings.theme.subtitle': '跟随系统、浅色玻璃或暗夜磨砂',
            'settings.theme.auto': '跟随系统',
            'settings.theme.light': '浅色',
            'settings.theme.dark': '暗夜',

            'dialog.ok': '确定',
            'dialog.cancel': '取消',
            'dialog.connect': '连接',
            'dialog.add': '添加',
            'dialog.remove': '确认移除',

            'keyboard.dialog.title': '选择键盘设备',
            'keyboard.dialog.subtitle': '当前列表已按规则去重，并保留键盘最可信控制接口',
            'keyboard.dialog.loading': '正在扫描键盘候选设备...',
            'keyboard.dialog.empty': '未发现可绑定的键盘候选设备（请确认 2.4G 接收器已插入）',
            'keyboard.remove.title': '移除键盘',
            'keyboard.remove.message': '确认移除当前绑定的键盘吗？移除后卡片将消失。',

            'bluetooth.dialog.title': '添加蓝牙设备',
            'bluetooth.dialog.subtitle': '支持公开标准 BLE 电量服务（0x180F）及经典蓝牙耳机',
            'bluetooth.dialog.loading': '正在读取 Windows 已配对蓝牙设备...',
            'bluetooth.dialog.empty': '未发现已配对蓝牙设备（请先在 Windows 设置中配对蓝牙）',
            'bluetooth.remove.title': '移除蓝牙设备',
            'bluetooth.remove.message': '确认移除该蓝牙设备吗？之后可从已配对列表重新添加。',

            'toast.scan_started': '正在重新扫描硬件设备...',
            'toast.scan_completed': '硬件设备扫描完成',
            'toast.refresh_started': '正在刷新电量...',
            'toast.refresh_completed': '电量刷新完成',
            'toast.bound_success': '设备绑定成功',
            'toast.removed_success': '已移除设备',
            'toast.op_failed': '操作失败：{msg}',

            'update.modal_title': '发现新版本',
            'update.notes_title': '更新说明',
            'update.later': '稍后再说',
            'update.start': '立即更新',
            'update.cancel': '取消下载',
            'update.cancelling': '正在取消下载...',
            'update.check_failed': '检查更新失败：{msg}',
            'update.latest': '当前已是最新版本 ({version})',
            'update.connecting_official': '正在连接官方更新源...',
            'update.connecting_mirror': '正在连接高速镜像更新源...',
            'update.downloading': '正在下载更新 ({percent}%)',
            'update.verifying': '正在校验文件哈希 (SHA-256)...',
            'update.applying': '正在启动安全替换并重启...',
            'update.retrying': '下载连接中断，正在重试...',
            'update.fallback': '已自动切换至备用下载源...',
            'update.cancelled': '更新已取消',
            'update.failed': '更新失败：{error}',
        },
        'en-US': {
    'settings.glass.title': 'Glass Transparency',
    'settings.glass.subtitle': 'Background only; higher is clearer',
    'settings.glass.less': 'Less transparent', 'settings.glass.more': 'More transparent',
    'settings.auto_refresh.title': 'Auto Refresh',
    'settings.auto_refresh.subtitle': 'Synchronize battery status in this window',
    'settings.language.title': 'Interface Language',
    'settings.language.subtitle': 'Chinese, English, or system preference',
    'device.connecting': 'Connecting and reading battery. You can close this dialog.',
    'device.bound': 'Added', 'device.ready': 'Ready',
    'action.close': 'Close', 'toast.saving_failed': 'Failed to save settings: {msg}',
            'app.header_title': 'Device Battery',
            'action.scan': 'Rescan',
            'action.add_keyboard': 'Add Keyboard',
            'action.add_bluetooth': 'Add Bluetooth',
            'action.check_update': 'Check Updates',
            'tray.menu.refresh_now': 'Refresh Now',
            'tray.menu.no_device': 'No Connected Devices Found',
            'device.offline_no_data': 'Ensure 2.4G receiver is inserted or Bluetooth is paired',
            'device.mouse': 'Wireless Mouse',
            'device.keyboard': 'Wireless Keyboard',
            'device.bluetooth': 'Bluetooth Device',
            'device.offline': 'Offline',
            'device.online': 'Online',
            'device.charging': '⚡ Charging',
            'device.wait_update': 'Waiting update',
            'device.updated_at': 'Updated at {time}',
            'device.remove_btn': 'Remove',
            'device.unbind_btn': 'Unbind',

            'settings.title': 'Settings',
            'settings.tray_style.title': 'Tray Icon Style',
            'settings.tray_style.subtitle': 'Choose capsule battery, minimal number, or circle',
            'settings.tray_style.capsule': 'Capsule',
            'settings.tray_style.vertical': 'Vertical',
            'settings.tray_style.number': 'Number',
            'settings.tray_style.circle': 'Circle',

            'settings.tray_priority.title': 'Tray Display Priority',
            'settings.tray_priority.subtitle': 'Which device to display in tray',
            'settings.tray_priority.mouse_first': 'Mouse First',
            'settings.tray_priority.keyboard_first': 'Keyboard First',
            'settings.tray_priority.lowest_battery': 'Lowest First',

            'settings.low_battery.title': 'Low Battery Alert',
            'settings.low_battery.subtitle': 'Show system notification when battery is low',
            'settings.off': 'Off',

            'settings.auto_update.title': 'Auto Update Check',
            'settings.auto_update.subtitle': 'Automatically check updates at startup',

            'settings.autostart.title': 'Launch at Startup',
            'settings.autostart.subtitle': 'Run quietly in background on Windows startup',

            'settings.theme.title': 'Appearance',
            'settings.theme.subtitle': 'System, Light Glass, or Dark Frosted',
            'settings.theme.auto': 'System',
            'settings.theme.light': 'Light',
            'settings.theme.dark': 'Dark',

            'dialog.ok': 'OK',
            'dialog.cancel': 'Cancel',
            'dialog.connect': 'Connect',
            'dialog.add': 'Add',
            'dialog.remove': 'Remove',

            'keyboard.dialog.title': 'Select Keyboard Device',
            'keyboard.dialog.subtitle': 'Recommended candidate list filtered by reliable control endpoints',
            'keyboard.dialog.loading': 'Scanning keyboard candidates...',
            'keyboard.dialog.empty': 'No bindable keyboard candidates found (check 2.4G receiver)',
            'keyboard.remove.title': 'Remove Keyboard',
            'keyboard.remove.message': 'Remove the currently bound keyboard? The card will disappear.',

            'bluetooth.dialog.title': 'Add Bluetooth Device',
            'bluetooth.dialog.subtitle': 'Supports standard BLE Battery Service (0x180F) & Classic Headsets',
            'bluetooth.dialog.loading': 'Reading paired Bluetooth devices...',
            'bluetooth.dialog.empty': 'No paired Bluetooth devices found (pair in Windows Settings first)',
            'bluetooth.remove.title': 'Remove Bluetooth Device',
            'bluetooth.remove.message': 'Remove this Bluetooth device? You can re-add it anytime.',

            'toast.scan_started': 'Scanning hardware devices...',
            'toast.scan_completed': 'Hardware scan completed',
            'toast.refresh_started': 'Refreshing battery status...',
            'toast.refresh_completed': 'Battery status refreshed',
            'toast.bound_success': 'Device bound successfully',
            'toast.removed_success': 'Device removed',
            'toast.op_failed': 'Operation failed: {msg}',

            'update.modal_title': 'New Version Found',
            'update.notes_title': 'Release Notes',
            'update.later': 'Later',
            'update.start': 'Update Now',
            'update.cancel': 'Cancel Download',
            'update.cancelling': 'Cancelling download...',
            'update.check_failed': 'Check update failed: {msg}',
            'update.latest': 'Already up to date ({version})',
            'update.connecting_official': 'Connecting to official source...',
            'update.connecting_mirror': 'Connecting to mirror source...',
            'update.downloading': 'Downloading update ({percent}%)',
            'update.verifying': 'Verifying hash integrity (SHA-256)...',
            'update.applying': 'Applying update & restarting...',
            'update.retrying': 'Download interrupted, retrying...',
            'update.fallback': 'Switched to fallback source...',
            'update.cancelled': 'Update cancelled',
            'update.failed': 'Update failed: {error}',
        }
    };




// 语言状态封装在当前页面实例中，自动语言由 Python 的系统语言解析结果决定。
export function createTranslator() {
    let language = 'zh-CN';
    function t(key, values = {}) {
        let text = I18N[language][key];
        if (text === undefined) throw new Error(`Missing translation: ${key}`);
        Object.entries(values).forEach(([name, value]) => {
            text = text.replaceAll(`{${name}}`, String(value));
        });
        return text;
    }
    function apply(config) {
        language = config.effective_ui_language;
        if (!I18N[language]) throw new Error(`Unsupported language: ${language}`);
        document.querySelectorAll('[data-i18n]').forEach(node => {
            node.textContent = t(node.dataset.i18n);
        });
        document.querySelectorAll('[data-i18n-title]').forEach(node => {
            node.title = t(node.dataset.i18nTitle);
        });
    }
    return { t, apply };
}

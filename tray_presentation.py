"""托盘图标选择、状态文字与通知。"""
import logging
from typing import Optional
from devices import MouseInfo
from config import (TRAY_ICON_PRIORITY_KEYBOARD_FIRST, TRAY_ICON_PRIORITY_LOWEST_BATTERY,
                    is_windows_taskbar_dark_mode)
from tray_icons import create_battery_icon
logger = logging.getLogger(__name__)


class TrayPresentation:
    def _update_icon(self):
        """按最新配置和快照重绘图标、tooltip 与菜单。"""
        if not self._tray:
            return
        mice = self.device_manager.mice
        keyboard = self.device_manager.keyboard
        bluetooth = self.device_manager.bluetooth_devices
        target = self._select_icon_target(
            [device for device in mice if device.percentage >= 0],
            keyboard if keyboard and keyboard.percentage >= 0 else None,
            [device for device in bluetooth if device.percentage >= 0],
        )
        self._tray.icon = create_battery_icon(
            target['percentage'] if target else -1, target['charging'] if target else False,
            style=self.config_manager.tray_icon_style, is_dark_taskbar=is_windows_taskbar_dark_mode())
        self._tray.title = self._tooltip(mice, keyboard, bluetooth)
        self._tray.menu = self._build_menu()
        self._check_low_battery_notifications(mice, keyboard, bluetooth)

    def _tooltip(self, mice, keyboard, bluetooth):
        """只拼接各设备电量文本，保留 Windows tooltip 长度限制。"""
        lines = [self._t('tray.app_name')]
        if not mice and not keyboard and not bluetooth:
            lines.append(self._t('tray.no_device_or_sleep'))
        for device in mice:
            self._append_device_line(lines, device)
        if keyboard:
            self._append_device_line(lines, keyboard)
        for device in bluetooth:
            self._append_device_line(lines, device)
        return "\n".join(lines)[:120]

    def _check_low_battery_notifications(self, mice, keyboard, bluetooth):
        """按设备身份显式检查通知，覆盖鼠标、绑定键盘和蓝牙设备。"""
        for device in mice:
            identity = f'Mouse|{device.device_id}' if device.device_id else device.name
            self._notify_low_battery(device, identity)
        if keyboard:
            self._notify_low_battery(keyboard, f'Keyboard|{keyboard.device_id}')
        for device in bluetooth:
            self._notify_low_battery(device, f'Bluetooth|{device.device_id}')

    def _device_display_name(self, device):
        """获取设备对外展示名，优先使用用户重命名的自定义别名。"""
        device_id = getattr(device, 'device_id', '')
        return self.config_manager.get_device_display_name(device_id, device.name)

    def _append_device_line(self, lines, device):
        """附加电量和充电标识，未知电量不转换成 0%。"""
        percentage = f'{device.percentage}%' if device.percentage >= 0 else 'N/A'
        charging = ' ⚡' if device.charging else ''
        name = self._device_display_name(device)
        lines.append(f'{self._translate_runtime_text(name)}: {percentage}{charging}')

    def _notify_low_battery(self, device, identity):
        """只对有效且未充电的读数发送低电量通知。"""
        if not device.online or not 0 <= device.percentage <= 100 or device.charging:
            return
        raw = getattr(device, 'raw_percentage', None)
        percent = raw if type(raw) is int and 0 <= raw <= 100 else device.percentage
        if not self.config_manager.should_notify(identity, percent):
            return
        display_name = self._device_display_name(device)
        try:
            self._tray.notify(self._t('tray.notification.low_battery_message',
                name=self._translate_runtime_text(display_name), percent=percent),
                title=self._t('tray.notification.low_battery_title'))
        except Exception:
            # 系统通知失败需要日志，不能中断后续图标与菜单刷新。
            logger.exception('发送低电量通知失败')

    def _select_icon_target(self, valid_mice: list[MouseInfo], valid_keyboard,
                            valid_bluetooth=None) -> Optional[dict]:
        """根据配置选择当前托盘图标要显示哪一台设备的电量。

        三种策略分别对应用户在设置面板中的选择：
        - 优先鼠标：有有效鼠标时优先显示鼠标，否则退回键盘
        - 优先键盘：有有效键盘时优先显示键盘，否则退回鼠标
        - 低电量优先：在所有有效设备中选电量最低者
        """
        priority = self.config_manager.tray_icon_priority
        mouse_low = min(valid_mice, key=lambda item: item.percentage) if valid_mice else None
        keyboard_target = None
        bluetooth_targets = [
            {'percentage': item.percentage, 'charging': item.charging}
            for item in (valid_bluetooth or [])
        ]
        if valid_keyboard:
            keyboard_target = {
                'percentage': valid_keyboard.percentage,
                'charging': valid_keyboard.charging,
            }

        if priority == TRAY_ICON_PRIORITY_KEYBOARD_FIRST:
            if keyboard_target:
                return keyboard_target
            if mouse_low:
                return {'percentage': mouse_low.percentage, 'charging': mouse_low.charging}
            return min(bluetooth_targets, key=lambda item: item['percentage']) if bluetooth_targets else None

        if priority == TRAY_ICON_PRIORITY_LOWEST_BATTERY:
            samples = []
            if mouse_low:
                samples.append({'percentage': mouse_low.percentage, 'charging': mouse_low.charging})
            if keyboard_target:
                samples.append(keyboard_target)
            samples.extend(bluetooth_targets)
            if not samples:
                return None
            return min(samples, key=lambda item: item['percentage'])

        # 默认策略：优先鼠标，再退回键盘。
        if mouse_low:
            return {'percentage': mouse_low.percentage, 'charging': mouse_low.charging}
        if keyboard_target:
            return keyboard_target
        if bluetooth_targets:
            return min(bluetooth_targets, key=lambda item: item['percentage'])
        return None

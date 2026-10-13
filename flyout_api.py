"""WebView 的公开 API，GUI 设备操作通过注入的命令客户端执行。"""
from config import (TRAY_ICON_STYLE_VALUES, TRAY_ICON_PRIORITY_VALUES,
                    THEME_MODE_AUTO, THEME_MODE_LIGHT, THEME_MODE_DARK,
                    validate_glass_transparency)
from flyout_state import get_flyout_state

# 已有低电量提醒的合法档位；关闭使用 0。
NOTIFY_LEVELS = (0, 10, 20, 30)
# 配置入口仅允许偏好键，不能从网页覆盖配置对象的方法或路径。
CHOICE_VALUES = {
    'tray_icon_style': TRAY_ICON_STYLE_VALUES,
    'tray_icon_priority': TRAY_ICON_PRIORITY_VALUES,
    'theme_mode': {THEME_MODE_AUTO, THEME_MODE_LIGHT, THEME_MODE_DARK},
    'ui_language': {'auto', 'zh-CN', 'en-US'},
    'low_battery_notify': NOTIFY_LEVELS,
}
# 开关值须是布尔值，避免字符串 false 被误保存为开启。
BOOLEAN_KEYS = frozenset(('auto_update', 'autostart', 'auto_refresh'))


class FlyoutApi:
    """只暴露设备操作、偏好和窗口交互所需的有限接口。"""

    def __init__(self, manager, services):
        self._manager = manager
        self._commands = services['commands']
        self._updates = services['updates']

    def get_state(self):
        with self._manager.operation():
            return get_flyout_state(self._manager)

    def set_config_value(self, key, value):
        """校验界面输入后保存偏好，配置变化立即同步托盘与窗口外观。"""
        with self._manager.operation():
            return self._save_preference(key, value)

    def _save_preference(self, key, value):
        validate_preference(key, value)
        manager, config = self._manager, self._manager.config_manager
        if key == 'auto_refresh':
            manager.set_auto_refresh(value)
        elif key == 'autostart':
            config.set_autostart(value)
            if config.check_autostart() != value:
                raise RuntimeError('开机自启动设置失败，请查看注册表访问日志')
        else:
            setattr(config, key, value)
        if key in ('tray_icon_style', 'tray_icon_priority', 'ui_language'):
            manager.refresh_tray()
        if key == 'theme_mode':
            manager.refresh_appearance()
        return self.get_state()

    def scan_devices(self):
        """请求托盘重新枚举硬件，等待真正的完成回执。"""
        with self._manager.operation():
            self._commands.execute('scan_devices')
            return self.get_state()

    def scan_keyboard_candidates(self):
        """在托盘枚举候选接口，GUI 只读取结果。"""
        with self._manager.operation():
            self._commands.execute('scan_keyboard_candidates')
            return [dict(device_id=c.device_id, display_name=c.display_name,
                         product_name=c.product_name) for c in self._manager.device_manager.keyboard_candidates]

    def bind_keyboard(self, device_id):
        return self._device_action('bind_keyboard', device_id)

    def unbind_keyboard(self):
        return self._device_action('unbind_keyboard')

    def scan_bluetooth_candidates(self):
        """获取 Windows 配对列表，按配置中的物理身份识别已绑定设备。"""
        with self._manager.operation():
            self._commands.execute('scan_bluetooth_candidates')
            bindings = self._manager.config_manager.bluetooth_bindings
            return [dict(device_id=c.device_id, name=c.name,
                         is_bound=candidate_is_bound(c, bindings))
                    for c in self._manager.device_manager.bluetooth_candidates]

    def bind_bluetooth(self, device_id):
        return self._device_action('bind_bluetooth', device_id)

    def unbind_bluetooth(self, device_id):
        return self._device_action('unbind_bluetooth', device_id)

    def _device_action(self, action, device_id=None):
        """统一执行绑定和解除动作，只有对应命令成功才返回 success。"""
        with self._manager.operation():
            payload = {'device_id': device_id} if device_id is not None else None
            self._commands.execute(action, payload)
            return {'success': True, 'state': self.get_state()}

    def refresh_battery(self):
        """等待托盘强制读取鼠标、键盘和蓝牙电量后更新页面。"""
        with self._manager.operation():
            self._commands.execute('refresh_battery')
            return self.get_state()

    def check_update(self):
        with self._manager.operation():
            return self._updates.check()

    def start_update_download(self, options):
        with self._manager.operation():
            return self._updates.start(options)

    def cancel_update_download(self):
        with self._manager.operation():
            return self._updates.cancel()

    def on_frontend_ready(self):
        """只有首屏成功渲染后才显示窗口，取消定时兜底显示。"""
        self._manager.on_frontend_ready()

    def on_frontend_error(self, message):
        """把首屏渲染异常带回 Python，避免错误只留在不可见的网页中。"""
        if not isinstance(message, str) or not message.strip():
            raise ValueError('GUI 初始化错误必须包含有效消息')
        self._manager.on_frontend_error(message)

    def hide_window(self):
        """前端只请求收起；宿主短期复用，长期闲置后安全释放 GUI。"""
        self._manager.hide()

    def drag_window(self):
        self._manager.drag_window()


def validate_preference(key, value):
    """在网页与 Python 的边界拒绝非法偏好，失败交由界面显示。"""
    if key == 'glass_transparency':
        validate_glass_transparency(value)
    elif key in BOOLEAN_KEYS:
        if not isinstance(value, bool):
            raise ValueError('开关配置必须是布尔值')
    elif key not in CHOICE_VALUES or value not in CHOICE_VALUES[key]:
        raise ValueError(f'非法偏好配置：{key}={value!r}')


def candidate_is_bound(candidate, bindings):
    """端点、ContainerId 或蓝牙地址命中任一项即属于同一物理设备。"""
    return any(
        binding.get('device_id') in candidate.endpoint_device_ids
        or (binding.get('container_id') and binding['container_id'].casefold() == candidate.container_id.casefold())
        or (binding.get('bluetooth_address') and int(binding['bluetooth_address']) == candidate.bluetooth_address)
        for binding in bindings
    )

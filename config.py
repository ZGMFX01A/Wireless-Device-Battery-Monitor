"""
配置与持久化管理模块

通过 JSON 文件保存用户偏好设置。
并提供修改 Windows 注册表实现程序自启的功能。
"""
import os
import sys
import json
import logging
import hashlib
import subprocess
import tempfile
import threading
import ctypes
from contextlib import contextmanager
import winreg

import updater
from i18n import (
    LANGUAGE_AUTO,
    SUPPORTED_UI_LANGUAGE_VALUES,
    normalize_ui_language,
    resolve_ui_language,
)

logger = logging.getLogger(__name__)


# 托盘图标显示逻辑枚举值：控制鼠标 / 键盘共存时的图标取值优先级。
TRAY_ICON_PRIORITY_MOUSE_FIRST = "mouse_first"
TRAY_ICON_PRIORITY_KEYBOARD_FIRST = "keyboard_first"
TRAY_ICON_PRIORITY_LOWEST_BATTERY = "lowest_battery_first"
TRAY_ICON_PRIORITY_VALUES = {
    TRAY_ICON_PRIORITY_MOUSE_FIRST,
    TRAY_ICON_PRIORITY_KEYBOARD_FIRST,
    TRAY_ICON_PRIORITY_LOWEST_BATTERY,
}

# 界面外观主题枚举值：支持自动跟随系统、明亮浅色、深色磨砂
THEME_MODE_AUTO = "auto"
THEME_MODE_LIGHT = "light"
THEME_MODE_DARK = "dark"
SUPPORTED_THEME_MODES = {
    THEME_MODE_AUTO,
    THEME_MODE_LIGHT,
    THEME_MODE_DARK,
}


def is_windows_dark_mode() -> bool:
    """检测 Windows 系统当前应用主题是否为深色模式。"""
    if os.name != 'nt':
        return False
    try:
        key = winreg.OpenKey(
            winreg.HKEY_CURRENT_USER,
            r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
            0,
            winreg.KEY_READ,
        )
        val, _ = winreg.QueryValueEx(key, "AppsUseLightTheme")
        winreg.CloseKey(key)
        return val == 0
    except Exception:
        return False

def _read_version() -> str:
    """从 VERSION 文件读取版本号，兼容打包与源码环境"""
    if getattr(sys, 'frozen', False):
        base = sys._MEIPASS
    else:
        base = os.path.dirname(os.path.abspath(__file__))
    version_file = os.path.join(base, 'VERSION')
    try:
        with open(version_file, 'r', encoding='utf-8-sig') as f:
            return f.read().strip()
    except FileNotFoundError:
        return "dev"

APP_VERSION = _read_version()

# 获取当前程序实际路径，如果被 PyInstaller 打包，获取的是生成的 exe 路径
if getattr(sys, 'frozen', False):
    APP_PATH = sys.executable
else:
    APP_PATH = os.path.abspath(sys.argv[0])

# 在同级目录存储配额
CONFIG_FILE = os.path.join(os.path.dirname(APP_PATH), "config.json")
REG_PATH = r"Software\Microsoft\Windows\CurrentVersion\Run"
REG_NAME = "MouseBatteryMonitor"


def _autostart_command(app_path: str = APP_PATH) -> str:
    """返回可安全存入 Run 键的 Windows 命令行。"""
    return subprocess.list2cmdline([app_path])


_config_process_lock = threading.RLock()


@contextmanager
def _config_file_lock(config_file: str):
    """串行化 tray 与 GUI 对同一配置文件的 read-modify-write。"""
    with _config_process_lock:
        if os.name != 'nt':
            yield
            return

        lock_id = hashlib.sha256(os.path.abspath(config_file).encode('utf-8')).hexdigest()[:24]
        kernel32 = ctypes.windll.kernel32
        handle = kernel32.CreateMutexW(None, False, f'Local\\MouseBatteryConfig_{lock_id}')
        if not handle:
            raise OSError('无法创建配置文件互斥体')
        acquired = False
        try:
            result = kernel32.WaitForSingleObject(handle, 10_000)
            # WAIT_OBJECT_0 / WAIT_ABANDONED 都意味着本线程已拥有 mutex。
            if result not in (0, 0x80):
                raise TimeoutError(f'等待配置文件锁超时或失败: {result}')
            acquired = True
            yield
        finally:
            if acquired:
                kernel32.ReleaseMutex(handle)
            kernel32.CloseHandle(handle)


class ConfigManager:
    """管理应用的用户配置和自启状态"""

    def __init__(self):
        # 启动时先清理上次热更新可能遗留的旧版执行文件被占用导致的残留
        updater.clean_old_version()

        self.config = self._default_config()
        self.load()
        self._refresh_autostart_path_if_needed()

    @staticmethod
    def _default_config() -> dict:
        return {
            "low_battery_notify": 20, # 默认 20%
            "notified_levels": {}, # 记录每个鼠标上次被通知时的电量，防重复弹窗
            "auto_update": False, # 默认不自动更新
            "keyboard_binding": None, # 单键盘绑定信息，供 tray 进程读取指定 HID 接口
            "bluetooth_bindings": [], # 多个标准 BLE Battery Service 设备绑定
            "tray_icon_priority": TRAY_ICON_PRIORITY_MOUSE_FIRST, # 托盘图标显示逻辑
            "ui_language": LANGUAGE_AUTO, # 界面语言策略：默认跟随系统语言，手动切换后写入显式覆盖值
            "theme_mode": THEME_MODE_AUTO, # 界面外观风格：默认跟随系统，支持 light/dark 显式指定
        }

    def _load_unlocked(self):
        """在已获得配置锁时从磁盘完整刷新内存配置。"""
        config = self._default_config()
        if not os.path.exists(CONFIG_FILE):
            self.config = config
            return
        try:
            with open(CONFIG_FILE, "r", encoding="utf-8") as f:
                data = json.load(f)
            if not isinstance(data, dict):
                raise ValueError('配置文件根节点必须是对象')
            config.update(data)
            self.config = config
        except Exception as e:
            logger.error(f"读取配置异常: {e}")
            # 损坏配置不能与旧内存状态混合，更不能在下次保存时把旧键写回。
            self.config = config

    def _save_unlocked(self):
        """在已获得配置锁时以原子替换保存当前配置。"""
        directory = os.path.dirname(CONFIG_FILE) or '.'
        temp_path = ''
        try:
            fd, temp_path = tempfile.mkstemp(prefix='.config.', suffix='.tmp', dir=directory)
            with os.fdopen(fd, "w", encoding="utf-8") as f:
                json.dump(self.config, f, indent=4, ensure_ascii=False)
                f.flush()
                os.fsync(f.fileno())
            os.replace(temp_path, CONFIG_FILE)
        except Exception as e:
            logger.error(f"保存配置异常: {e}")
            if temp_path:
                try:
                    os.remove(temp_path)
                except FileNotFoundError:
                    pass

    def _mutate(self, mutator):
        """执行不可拆分的 reload → mutate → atomic save。"""
        with _config_file_lock(CONFIG_FILE):
            self._load_unlocked()
            result = mutator()
            self._save_unlocked()
            return result

    def _reload_from_disk(self):
        """跨进程读取最新配置。

        tray 与 GUI 分属不同进程：
        - GUI 修改设置时会直接写 `config.json`
        - tray 进程若只读内存副本，就看不到最新设置

        因此关键 getter 在读取前都轻量重载一次磁盘配置，
        让「托盘图标显示逻辑」「低电量提醒」「界面语言」这类设置可以在运行中生效。
        """
        self.load()

    def _refresh_autostart_path_if_needed(self):
        """如果已启用开机自启，则在热更新后同步到当前 exe 路径。"""
        try:
            key = winreg.OpenKey(winreg.HKEY_CURRENT_USER, REG_PATH, 0, winreg.KEY_READ)
            value, _ = winreg.QueryValueEx(key, REG_NAME)
            winreg.CloseKey(key)
        except FileNotFoundError:
            return
        except Exception as e:
            logger.error(f"读取启动项错误: {e}")
            return

        if str(value).strip() == _autostart_command(APP_PATH):
            return

        try:
            key = winreg.OpenKey(winreg.HKEY_CURRENT_USER, REG_PATH, 0, winreg.KEY_SET_VALUE)
            winreg.SetValueEx(key, REG_NAME, 0, winreg.REG_SZ, _autostart_command(APP_PATH))
            winreg.CloseKey(key)
            logger.info(f"已同步开机自启路径到新版本程序: {APP_PATH}")
        except Exception as e:
            logger.error(f"同步启动项路径失败: {e}")

    def load(self):
        """读取配置文件"""
        with _config_file_lock(CONFIG_FILE):
            self._load_unlocked()

    def save(self):
        """写入配置文件"""
        with _config_file_lock(CONFIG_FILE):
            self._save_unlocked()

    @property
    def low_battery_notify(self) -> int:
        self._reload_from_disk()
        return self.config.get("low_battery_notify", 20)

    @low_battery_notify.setter
    def low_battery_notify(self, val: int):
        def apply():
            self.config["low_battery_notify"] = val
            # 重置所有已经通知过的状态，即使用户修改了阈值，也能重新触发一次
            self.config["notified_levels"] = {}
        self._mutate(apply)

    @property
    def auto_update(self) -> bool:
        self._reload_from_disk()
        return self.config.get("auto_update", False)

    @auto_update.setter
    def auto_update(self, val: bool):
        self._mutate(lambda: self.config.__setitem__("auto_update", val))

    @property
    def keyboard_binding(self) -> dict | None:
        """返回当前键盘绑定配置。

        仅当至少包含 `device_id` 时才视为有效绑定；
        这样可以避免旧配置或脏配置把半截结构误当成已绑定设备。
        """
        self._reload_from_disk()
        binding = self.config.get("keyboard_binding")
        if not isinstance(binding, dict):
            return None
        device_id = str(binding.get("device_id", "") or "").strip()
        if not device_id:
            return None
        return {
            "device_id": device_id,
            "vendor_id": int(binding.get("vendor_id", 0) or 0),
            "product_id": int(binding.get("product_id", 0) or 0),
            "usage_page": int(binding.get("usage_page", 0) or 0),
            "usage": int(binding.get("usage", 0) or 0),
            "interface_number": int(binding.get("interface_number", -1) or -1),
            "product_name": str(binding.get("product_name", "") or ""),
        }

    @keyboard_binding.setter
    def keyboard_binding(self, binding: dict | None):
        """保存单键盘绑定配置。

        这里同时保存 HID path 与辅助元数据，原因是键盘重插后 path 可能变化，
        tray 进程后续可用这些元数据重新回收真实接口，而不是让绑定永久失效。
        """
        if binding is None:
            self._mutate(lambda: self.config.__setitem__("keyboard_binding", None))
            return

        device_id = str(binding.get("device_id", "") or "").strip()
        if not device_id:
            logger.warning("忽略空的键盘绑定配置")
            return

        normalized = {
            "device_id": device_id,
            "vendor_id": int(binding.get("vendor_id", 0) or 0),
            "product_id": int(binding.get("product_id", 0) or 0),
            "usage_page": int(binding.get("usage_page", 0) or 0),
            "usage": int(binding.get("usage", 0) or 0),
            "interface_number": int(binding.get("interface_number", -1) or -1),
            "product_name": str(binding.get("product_name", "") or ""),
        }
        self._mutate(lambda: self.config.__setitem__("keyboard_binding", normalized))

    @property
    def bluetooth_bindings(self) -> list[dict]:
        """返回按 Windows device ID 去重后的 BLE 设备绑定。"""
        self._reload_from_disk()
        raw_bindings = self.config.get('bluetooth_bindings', [])
        if not isinstance(raw_bindings, list):
            return []

        bindings: list[dict] = []
        seen_ids: set[str] = set()
        for item in raw_bindings:
            if not isinstance(item, dict):
                continue
            device_id = str(item.get('device_id', '') or '').strip()
            if not device_id or device_id in seen_ids:
                continue
            seen_ids.add(device_id)
            bindings.append({
                'device_id': device_id,
                'name': str(item.get('name', '') or '未知蓝牙设备'),
            })
        return bindings

    def add_bluetooth_binding(self, binding: dict) -> bool:
        device_id = str(binding.get('device_id', '') or '').strip()
        if not device_id:
            logger.warning('忽略空的蓝牙设备绑定')
            return False
        def apply():
            bindings = self.bluetooth_bindings
            if any(item['device_id'] == device_id for item in bindings):
                return False
            bindings.append({
                'device_id': device_id,
                'name': str(binding.get('name', '') or '未知蓝牙设备'),
            })
            self.config['bluetooth_bindings'] = bindings
            return True
        return self._mutate(apply)

    def remove_bluetooth_binding(self, device_id: str) -> bool:
        device_id = str(device_id or '').strip()
        def apply():
            bindings = self.bluetooth_bindings
            remaining = [item for item in bindings if item['device_id'] != device_id]
            if len(remaining) == len(bindings):
                return False
            self.config['bluetooth_bindings'] = remaining
            return True
        return self._mutate(apply)

    @property
    def tray_icon_priority(self) -> str:
        """返回托盘图标显示优先级，非法值统一回退到默认项。"""
        self._reload_from_disk()
        value = str(self.config.get("tray_icon_priority", TRAY_ICON_PRIORITY_MOUSE_FIRST) or "")
        if value not in TRAY_ICON_PRIORITY_VALUES:
            return TRAY_ICON_PRIORITY_MOUSE_FIRST
        return value

    @tray_icon_priority.setter
    def tray_icon_priority(self, val: str):
        """保存托盘图标显示逻辑。

        该配置会直接影响 tray 图标在鼠标和键盘之间取哪一台设备的电量，
        因此不做静默纠正，非法值只记录日志并拒绝保存。
        """
        if val not in TRAY_ICON_PRIORITY_VALUES:
            logger.warning(f"托盘图标优先级非法值: {val!r}")
            return
        self._mutate(lambda: self.config.__setitem__("tray_icon_priority", val))

    @property
    def ui_language(self) -> str:
        """返回当前界面语言偏好。

        业务含义：
        - `auto` 表示默认跟随系统语言
        - `zh-CN` / `en-US` 表示用户手动覆盖系统默认
        """
        self._reload_from_disk()
        value = normalize_ui_language(self.config.get("ui_language", LANGUAGE_AUTO), allow_auto=True)
        if value not in SUPPORTED_UI_LANGUAGE_VALUES:
            return LANGUAGE_AUTO
        return value

    @ui_language.setter
    def ui_language(self, val: str):
        """保存界面语言偏好。

        这里不做静默兜底：非法值只记录日志并拒绝保存，
        避免 tray / GUI 在多进程读取时出现不可解释的语言状态。
        """
        normalized = normalize_ui_language(val, allow_auto=True)
        if normalized not in SUPPORTED_UI_LANGUAGE_VALUES:
            logger.warning(f"界面语言非法值: {val!r}")
            return
        self._mutate(lambda: self.config.__setitem__("ui_language", normalized))

    @property
    def effective_ui_language(self) -> str:
        """返回当前实际生效的界面语言。"""
        self._reload_from_disk()
        return resolve_ui_language(self.config.get("ui_language", LANGUAGE_AUTO))

    @property
    def theme_mode(self) -> str:
        """返回当前设置的外观模式偏好（'auto' / 'light' / 'dark'）。"""
        self._reload_from_disk()
        val = str(self.config.get("theme_mode", THEME_MODE_AUTO) or "")
        if val not in SUPPORTED_THEME_MODES:
            return THEME_MODE_AUTO
        return val

    @theme_mode.setter
    def theme_mode(self, val: str):
        """保存外观模式偏好。"""
        if val not in SUPPORTED_THEME_MODES:
            logger.warning(f"界面外观模式非法值: {val!r}")
            return
        self._mutate(lambda: self.config.__setitem__("theme_mode", val))

    @property
    def effective_theme_mode(self) -> str:
        """返回当前实际生效的主题模式（'light' 或 'dark'）。"""
        mode = self.theme_mode
        if mode == THEME_MODE_AUTO:
            return THEME_MODE_DARK if is_windows_dark_mode() else THEME_MODE_LIGHT
        return mode

    def check_autostart(self) -> bool:
        """检查是否已注册开机自启"""
        try:
            key = winreg.OpenKey(winreg.HKEY_CURRENT_USER, REG_PATH, 0, winreg.KEY_READ)
            value, _ = winreg.QueryValueEx(key, REG_NAME)
            winreg.CloseKey(key)
            return str(value).strip() == _autostart_command(APP_PATH)
        except FileNotFoundError:
            return False
        except Exception as e:
            logger.error(f"读取启动项错误: {e}")
            return False

    def set_autostart(self, enable: bool):
        """设定开机启动状态"""
        try:
            key = winreg.OpenKey(winreg.HKEY_CURRENT_USER, REG_PATH, 0, winreg.KEY_SET_VALUE)
            if enable:
                winreg.SetValueEx(key, REG_NAME, 0, winreg.REG_SZ, _autostart_command(APP_PATH))
                logger.info("已打开开机自启动")
            else:
                try:
                    winreg.DeleteValue(key, REG_NAME)
                    logger.info("已关闭开机自启动")
                except FileNotFoundError:
                    pass
            winreg.CloseKey(key)
        except Exception as e:
            logger.error(f"修改自启注册表失败: {e}")

    def should_notify(self, device_name: str, current_pct: int) -> bool:
        """
        判断此时是否应该弹出低电量警告。
        支持跌穿阈值后自动继续提醒：
        > 5% 时，每掉 5% 提醒一次
        <= 5% 时，每掉 1% 提醒一次

        边界修复：
        - current_pct <= 0 时直接返回（休眠或设备未就绪，不触发误报）
        - last_notified 默认为 101，使首次跌穿阈值能正确触发
        - 仅在电量真正下降时才更新计数，避免回升/持平重复刷新
        """
        def apply():
            threshold = self.config.get("low_battery_notify", 20)
            if threshold == 0 or current_pct <= 0:
                return False, False

            notified_levels = self.config.setdefault("notified_levels", {})
            last_notified = notified_levels.get(device_name, 101)
            if current_pct > threshold:
                if last_notified <= threshold:
                    notified_levels[device_name] = current_pct
                    return False, True
                return False, False
            if last_notified > threshold:
                notified_levels[device_name] = current_pct
                return True, True
            if current_pct >= last_notified:
                return False, False
            if current_pct <= 5 or (last_notified - current_pct) >= 5:
                notified_levels[device_name] = current_pct
                return True, True
            return False, False

        with _config_file_lock(CONFIG_FILE):
            self._load_unlocked()
            should_show, changed = apply()
            if changed:
                self._save_unlocked()
            return should_show

"""
无线设备电量监控 - 系统托盘图标

使用 pystray + Pillow 在 Windows 托盘区显示动态电量图标。

图标设计：圆环
- 圆环弧度按电量比例绘制（满电 = 完整绿色圆环）
- 圆环中心显示粗体白色百分比数字
- 颜色随电量变化：绿 → 黄 → 橙 → 红
- 充电时蓝色 + 闪电
"""

import threading
import logging
import time
import sys
from typing import Optional, Callable

import pystray

from devices import DeviceManager, MouseInfo, Brand
from config import (
    ConfigManager,
    APP_VERSION,
    TRAY_ICON_PRIORITY_MOUSE_FIRST,
    TRAY_ICON_PRIORITY_KEYBOARD_FIRST,
    TRAY_ICON_PRIORITY_LOWEST_BATTERY,
    TRAY_ICON_STYLE_CAPSULE,
    TRAY_ICON_STYLE_VERTICAL,
    TRAY_ICON_STYLE_NUMBER,
    TRAY_ICON_STYLE_CIRCLE,
    is_windows_taskbar_dark_mode,
)
import updater
from i18n import translate, translate_brand_name, translate_runtime_text

logger = logging.getLogger(__name__)

# ============================================================
# 颜色定义
# ============================================================

from tray_icons import create_battery_icon


from tray_presentation import TrayPresentation
from tray_menu import TrayMenu


class TrayApp(TrayPresentation, TrayMenu):
    """系统托盘电量监控"""

    def __init__(self, device_manager: DeviceManager,
                 config_manager: ConfigManager,
                 on_open_flyout: Optional[Callable] = None,
                 on_open_settings: Optional[Callable] = None,
                 on_exit: Optional[Callable] = None, *, capture_window_anchor=None):
        self.device_manager = device_manager
        self.config_manager = config_manager
        self.on_open_flyout = on_open_flyout
        self.on_open_settings = on_open_settings
        self.on_exit = on_exit
        self.capture_window_anchor = capture_window_anchor
        self._tray: Optional[pystray.Icon] = None
        self._running = False
        self._stopping = False

    def _effective_language(self) -> str:
        """返回当前 tray 应采用的实际语言。"""
        return self.config_manager.effective_ui_language

    def _t(self, key: str, **kwargs) -> str:
        """按当前语言获取托盘静态文案。"""
        return translate(key, self._effective_language(), **kwargs)

    def _translate_runtime_text(self, text: str) -> str:
        """翻译运行时状态文案，避免英文模式仍展示中文状态。"""
        return translate_runtime_text(text, self._effective_language())

    def _translate_brand_name(self, name: str) -> str:
        """翻译品牌名，保持菜单与 tooltip 语言一致。"""
        return translate_brand_name(name, self._effective_language())

    def start(self, detached=False):
        """启动主线程托盘循环，硬件初始化交给后台工作线程。"""
        self._running = True
        self.device_manager.add_on_update(self._update_icon)
        self._tray = pystray.Icon(
            name='MouseBattery',
            icon=create_battery_icon(-1, style=self.config_manager.tray_icon_style,
                                     is_dark_taskbar=is_windows_taskbar_dark_mode()),
            title=f"{self._t('tray.app_name')}\n{self._t('tray.scanning')}",
            menu=self._build_menu(),
        )
        self._boot_ready = threading.Event()
        threading.Thread(target=self._bootstrap, daemon=True, name='tray-bootstrap').start()
        if detached:
            self._tray.run_detached(setup=self._ready)
            return
        try:
            self._tray.run(setup=self._ready)
        except KeyboardInterrupt:
            # 用户中断交给统一资源收尾流程。
            logger.info('托盘收到中断信号')
        finally:
            self.stop()

    def _bootstrap(self):
        """扫描完成后才允许确认更新启动，避免尚未初始化就报告成功。"""
        try:
            self.device_manager.start_command_listener()
            self.device_manager.scan_and_refresh()
            self.device_manager.start_auto_refresh(60)
            if self.config_manager.auto_update:
                threading.Thread(target=self._delayed_auto_update, daemon=True).start()
        except Exception:
            # 初始化失败必须中止应用，不能把启动确认写成成功。
            logger.exception('托盘硬件初始化失败')
            self.request_exit()
        finally:
            self._boot_ready.set()

    def _delayed_auto_update(self):
        """沿用启动后的延迟检查，避免与首次设备扫描争抢资源。"""
        time.sleep(10)
        if self._running:
            self._auto_update_once()

    def _ready(self, icon):
        """托盘显示且初始化完成后确认新版启动，并报告上次更新失败。"""
        icon.visible = True
        while self._running and not self._boot_ready.wait(0.1):
            pass
        if not self._running:
            return
        try:
            updater.confirm_update_startup(APP_VERSION)
            result = updater.read_update_result(sys.executable)
            if result.get('status') == 'failed':
                icon.notify(self._t('update.recovered', error=result.get('detail', '')),
                            self._t('tray.app_name'))
        except Exception:
            # 确认失败不能继续作为健康的新版运行，交给原更新事务恢复。
            logger.exception('更新启动确认失败')
            self.request_exit()

    def _auto_update_once(self):
        has_update, version, url, _, size, digest = updater.check_for_update(APP_VERSION, self._effective_language())
        if not has_update:
            return
        result = updater.read_update_result(sys.executable)
        if result.get('status') == 'failed' and result.get('version') == version.lower().removeprefix('v'):
            logger.warning('此版本上次安装失败，暂停自动重试；可从设置中手动重试: %s', version)
            return
        logger.info('系统后台发现新版本，开始静默下载升级...')
        updater.download_and_install(url, expected_size=size, expected_digest=digest, target_version=version)

    def request_exit(self):
        """唤醒 Windows 托盘消息循环，让主线程完成设备收尾。"""
        self._running = False
        if self._tray:
            self._tray.stop()

    def stop(self):
        if self._stopping:
            return
        self._stopping = True
        self._running = False

        # 移除回调避免在停止时被调用
        self.device_manager.remove_on_update(self._update_icon)

        self.device_manager.shutdown()

        if self._tray:
            try:
                self._tray.visible = False
                self._tray.stop()
            except Exception as e:
                logger.debug(f"托盘停止异常: {e}")

        if self.on_exit:
            try:
                self.on_exit()
            except Exception as e:
                logger.debug(f"调用退出回调异常: {e}")

    def refresh_icon(self):
        """外部主动请求刷新托盘图标"""
        self._update_icon()

    # ---- 图标 & 菜单 ----

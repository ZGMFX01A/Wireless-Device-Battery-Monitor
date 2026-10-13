"""托盘可见菜单与左键默认入口。"""
import logging
import threading
from pystray import Menu, MenuItem
from config import (TRAY_ICON_STYLE_CAPSULE, TRAY_ICON_STYLE_VERTICAL,
                    TRAY_ICON_STYLE_NUMBER, TRAY_ICON_STYLE_CIRCLE)
logger = logging.getLogger(__name__)


class TrayMenu:
    def _build_menu(self):
        """右键保留设置和退出入口，左键默认项单独打开浮窗。"""
        items = self._device_menu_items()
        items.append(MenuItem(self._t('tray.menu.refresh_now'), self._on_refresh))
        items.append(MenuItem(self._t('tray.menu.icon_style'), self._style_menu()))
        if self.on_open_flyout:
            items.append(MenuItem('__default_flyout__', self._on_open_flyout_click,
                                  default=True, visible=False))
        if self.on_open_settings:
            items.append(MenuItem(self._t('tray.menu.open_settings'), self._on_open_settings_click,
                                  default=self.on_open_flyout is None))
        items.extend((Menu.SEPARATOR, MenuItem(self._t('tray.menu.quit'), self._on_quit)))
        return Menu(*items)

    def _device_menu_items(self):
        """菜单状态只展示当前快照，不负责触发硬件读取。"""
        devices = [*self.device_manager.mice, *self.device_manager.bluetooth_devices]
        if self.device_manager.keyboard:
            devices.append(self.device_manager.keyboard)
        if not devices:
            return [MenuItem(self._t('tray.menu.no_device'), None, enabled=False), Menu.SEPARATOR]
        return [*(MenuItem(f'{self._translate_runtime_text(device.name)}: '
                           f'{str(device.percentage) + "%" if device.percentage >= 0 else "N/A"}',
                           None, enabled=False) for device in devices), Menu.SEPARATOR]

    def _style_menu(self):
        """四种样式均可直接从托盘切换，保留经典圆环入口。"""
        styles = (TRAY_ICON_STYLE_CAPSULE, TRAY_ICON_STYLE_VERTICAL,
                  TRAY_ICON_STYLE_NUMBER, TRAY_ICON_STYLE_CIRCLE)
        return Menu(*(self._style_item(style) for style in styles))

    def _style_item(self, style):
        """每项回调独立捕获样式值，避免循环闭包都指向最后一项。"""
        def select(icon, item):
            self.config_manager.tray_icon_style = style
            self._update_icon()
        return MenuItem(self._t(f'tray.menu.style_{style}'), select,
                        checked=lambda _: self.config_manager.tray_icon_style == style)

    # ---- 菜单回调 ----

    def _on_refresh(self, icon, item):
        threading.Thread(
            target=self.device_manager.manual_refresh, daemon=True
        ).start()

    def _on_open_flyout_click(self, icon, item):
        """左键点击时固定通知图标锚点，再由后台线程启动 GUI。"""
        if self.on_open_flyout:
            self._open_gui(self.on_open_flyout, icon)

    def _on_open_settings_click(self, icon, item):
        """设置窗口以本次通知图标所在显示器为准。"""
        if self.on_open_settings:
            self._open_gui(self.on_open_settings, icon)

    def _open_gui(self, callback, icon):
        """正式入口注入锚点采集器，其他托盘消费者保留原回调契约。"""
        try:
            args = (self.capture_window_anchor(icon),) if self.capture_window_anchor else ()
            threading.Thread(target=callback, args=args, daemon=True).start()
        except Exception as error:
            # 锚点采集失败不能使用猜测坐标，向用户报告实际原生错误。
            logger.exception('采集 GUI 窗口锚点失败')
            icon.notify(str(error), self._t('tray.app_name'))

    def _on_quit(self, icon, item):
        logger.info("用户退出程序")
        self.stop()

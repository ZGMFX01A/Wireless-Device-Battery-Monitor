"""组合只读状态、跨进程命令和 WebView2 运行时。"""
import ctypes
import json
import logging
import os
import threading

from config import ConfigManager
from device_command_service import DeviceCommandClient
from devices import SharedStateDeviceManager, request_device_command, DEVICE_COMMAND_REFRESH_TRAY_ICON
from flyout import FlyoutManager, STATE_REFRESH_SECONDS
from flyout_geometry import initialize_dpi_awareness, capture_cursor_anchor, validate_anchor
from flyout_host import FlyoutHost
from flyout_api import FlyoutApi
from flyout_update import FlyoutUpdate
from gui_process import process_alive, GUI_IDLE_EXIT_CODE
import updater
import webview

logger = logging.getLogger(__name__)
# 缺少运行时时提供微软官方入口，不假定所有 Windows 10 都已安装。
WEBVIEW2_INSTALL_URL = 'https://developer.microsoft.com/microsoft-edge/webview2/'
# Windows 错误对话框样式：无控制台 EXE 也必须显示启动失败。
ERROR_DIALOG_STYLE = 0x10


def require_webview2():
    """提前初始化指定引擎，缺少运行时时明确失败。"""
    # easy_drag=False 只禁用整页拖动，默认标题栏选择器仍会注册 JS MoveWindow。
    # 标题栏已有原生 HTCAPTION 通道；禁用第二套拖动，不修改 HTML/CSS。
    webview.settings['DRAG_REGION_SELECTOR'] = ':not(*)'
    # 控制器创建前指定透明底色，避免初始化阶段先铺不透明背景再切换。
    os.environ.setdefault('WEBVIEW2_DEFAULT_BACKGROUND_COLOR', '00000000')
    from webview.guilib import initialize
    gui = initialize('edgechromium')
    if gui.renderer != 'edgechromium':
        raise RuntimeError(f'缺少 Microsoft Edge WebView2 Runtime，请安装：{WEBVIEW2_INSTALL_URL}')


def run_gui(mode):
    """只读共享状态；启动失败返回非零状态。"""
    shared = None
    try:
        initialize_dpi_awareness()
        require_webview2()
        shared = SharedStateDeviceManager()
        shared.sync_shared_state_silently()
        manager, updates = compose_gui(mode, shared)
        shared.start_auto_refresh(STATE_REFRESH_SECONDS)
        from main import start_update_shutdown_watchdog
        start_update_shutdown_watchdog(os.getpid(), manager.request_close)
        webview.start(manager.start_controls, gui='edgechromium', http_server=True, debug=False)
        updates.close_and_wait()
        if manager.startup_error:
            raise manager.startup_error
        return GUI_IDLE_EXIT_CODE if manager.idle_exit else 0
    except Exception as error:
        # 运行时缺失和资源加载失败必须对用户可见。
        logger.exception('WebView GUI 启动或运行失败')
        ctypes.windll.user32.MessageBoxW(None, str(error), '无线设备电量监控', ERROR_DIALOG_STYLE)
        return 1
    finally:
        if shared:
            shared.shutdown()


def compose_gui(mode, shared):
    """组合根注入原生宿主、状态推送和更新器，业务对象不创建具体实现。"""
    closed = threading.Event()
    host_text = os.environ.get('MOUSE_BATTERY_HOST_PID', '')
    host_pid = int(host_text) if host_text else None
    manager = FlyoutManager(shared, ConfigManager(), {
        'closed': closed, 'webview': webview, 'mode': mode,
        'refresh_tray': lambda: request_device_command(DEVICE_COMMAND_REFRESH_TRAY_ICON),
        'anchor': startup_anchor(), 'host_factory': FlyoutHost,
    })
    updates = FlyoutUpdate({
        'updater': updater, 'emit': manager.emit, 'host_pid': host_pid or os.getpid(),
        'language': lambda: manager.config_manager.effective_ui_language,
    })
    commands = DeviceCommandClient(shared, {
        'send': request_device_command, 'host_alive': lambda: process_alive(host_pid), 'closed': closed,
        'on_progress': manager._on_devices_updated,
    })
    manager.attach_updates(updates)
    manager.create_window(FlyoutApi(manager, {'commands': commands, 'updates': updates}))
    return manager, updates


def startup_anchor():
    """托盘传入点击时锚点；独立启动在窗口创建前固定当前光标锚点。"""
    text = os.environ.get('MOUSE_BATTERY_WINDOW_ANCHOR')
    return validate_anchor(json.loads(text)) if text else capture_cursor_anchor()

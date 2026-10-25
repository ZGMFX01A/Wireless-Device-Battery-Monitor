"""组合只读状态、跨进程命令和 WebView2 运行时。"""
import ctypes
import json
import logging
import os
import threading

from config import ConfigManager
from device_command_service import DeviceCommandClient
from devices import SharedStateDeviceManager, request_device_command, DEVICE_COMMAND_REFRESH_TRAY_ICON
from flyout import FlyoutManager, GuiFrontendError, STATE_REFRESH_SECONDS
from flyout_geometry import initialize_dpi_awareness, capture_cursor_anchor, validate_anchor
from flyout_host import FlyoutHost
from flyout_api import FlyoutApi
from flyout_update import FlyoutUpdate
from gui_process import (process_alive, current_gui_channel, write_gui_failure,
                         GUI_IDLE_EXIT_CODE, GUI_FAILURE_EXIT_CODE)
from rtss_compat import disable_rtss_hooks, is_rtss_running
import updater
import webview
from webview_prerequisite import (detect_webview2, show_runtime_guidance,
                                  WebView2Unavailable, WEBVIEW2_INSTALL_URL)

logger = logging.getLogger(__name__)
# 缺少运行时时提供微软官方入口，不假定所有 Windows 10 都已安装。
# Windows 错误对话框样式：无控制台 EXE 也必须显示启动失败。
ERROR_DIALOG_STYLE = 0x10


def require_webview2():
    """提前初始化指定引擎，缺少运行时时明确失败。"""
    # easy_drag=False 只禁用整页拖动，默认标题栏选择器仍会注册 JS MoveWindow。
    # 标题栏已有原生 HTCAPTION 通道；禁用第二套拖动，不修改 HTML/CSS。
    webview.settings['DRAG_REGION_SELECTOR'] = ':not(*)'
    # 控制器创建前指定透明底色，避免初始化阶段先铺不透明背景再切换。
    os.environ.setdefault('WEBVIEW2_DEFAULT_BACKGROUND_COLOR', '00000000')
    version = detect_webview2(webview.settings.get('WEBVIEW2_RUNTIME_PATH'))
    from webview.guilib import initialize
    gui = initialize('edgechromium')
    if gui.renderer != 'edgechromium':
        raise RuntimeError('WebView2 已检测到，但 GUI 引擎未能初始化。请检查 .NET/程序依赖；不会切换到旧 IE 引擎。')
    logger.info('WebView2 环境检测通过：%s', version)


def run_gui(mode):
    """只读共享状态；启动失败返回非零状态。"""
    shared = None
    manager = None
    try:
        # 也覆盖直接运行 --flyout/--settings 的入口，必须早于原生 GUI 初始化。
        disable_rtss_hooks(os.environ)
        logger.info('已请求本次 GUI 及 WebView2 子进程禁用 RTSS 图形 API Hook')
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
    except WebView2Unavailable as error:
        logger.warning('WebView2 运行时不可用：%s', error)
        show_runtime_guidance(error)
        return 1
    except Exception as error:
        # 运行时缺失和资源加载失败必须对用户可见。
        logger.exception('WebView GUI 启动或运行失败')
        detail = str(error)
        try:
            if 'BrowserProcessExited' in detail and is_rtss_running():
                detail += (
                    '\n\n检测到系统正在运行 RivaTuner Statistics Server (RTSS)。'
                    '\nRTSS 图形注入可能与 WebView2 冲突导致其崩溃退出。'
                    '\n本程序已启用进程级兼容声明；请检查 RTSS 版本及是否强制启用了 Hooking。'
                )
        except Exception:
            pass
        if isinstance(error, GuiFrontendError) and isinstance(current_gui_channel(), str):
            write_gui_failure(current_gui_channel(), manager.recovery_request, detail)
            return GUI_FAILURE_EXIT_CODE
        ctypes.windll.user32.MessageBoxW(None, detail, '无线设备电量监控', ERROR_DIALOG_STYLE)
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

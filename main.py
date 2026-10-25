"""
无线设备电量监控 - 程序入口

检测无线鼠标、机械键盘与标准 BLE 设备的电池状态，并在系统托盘显示电量。
"""

import sys
import os
import time
import logging
import _thread
import ctypes
import subprocess
import atexit
import threading

from devices import DeviceManager, cleanup_stale_shared_state_files
from tray import TrayApp
from config import ConfigManager
import updater
from gui_process import GuiProcessController
from gui_focus import allow_gui_foreground
from flyout_geometry import initialize_dpi_awareness, capture_tray_anchor


ERROR_ALREADY_EXISTS = 183
ERROR_ACCESS_DENIED = 5
MAIN_INSTANCE_MUTEX = r'Local\MouseBattery_Main_SingleInstance'
_instance_mutex_handle = None
_shutdown_for_update = False
_shutdown_skip_gui_pid = None


def acquire_single_instance(lock_name: str = MAIN_INSTANCE_MUTEX) -> bool:
    """基于当前登录会话的 Windows 命名互斥体实现单实例。"""
    global _instance_mutex_handle
    if os.name != 'nt':
        return True

    try:
        kernel32 = ctypes.windll.kernel32
        kernel32.CreateMutexW.argtypes = [ctypes.c_void_p, ctypes.c_bool, ctypes.c_wchar_p]
        kernel32.CreateMutexW.restype = ctypes.c_void_p
        kernel32.GetLastError.argtypes = []
        kernel32.GetLastError.restype = ctypes.c_uint32

        handle = kernel32.CreateMutexW(None, False, lock_name)
        err = kernel32.GetLastError()
        if not handle:
            logging.getLogger(__name__).warning('创建单实例互斥体失败: name=%s error=%s', lock_name, err)
            # 对既有互斥体无访问权时不能将失败当作“当前没有实例”。
            return err != ERROR_ACCESS_DENIED

        _instance_mutex_handle = handle
        if err == ERROR_ALREADY_EXISTS:
            return False
        return True
    except Exception as e:
        # 单实例校验失败时保持主流程可用，但要留下日志，
        # 否则用户很难知道为什么偶发出现多实例。
        logging.getLogger(__name__).warning(f"创建单实例互斥体失败，继续运行: {e}")
        return True


@atexit.register
def release_single_instance():
    """进程退出时释放互斥体句柄。"""
    global _instance_mutex_handle
    if os.name != 'nt' or not _instance_mutex_handle:
        return
    try:
        ctypes.windll.kernel32.CloseHandle(ctypes.c_void_p(_instance_mutex_handle))
    except Exception as e:
        logging.getLogger(__name__).debug(f"释放互斥体句柄失败: {e}")
    _instance_mutex_handle = None


def setup_logging():
    """配置日志"""
    logging.basicConfig(
        level=logging.INFO,
        format='%(asctime)s [%(levelname)s] %(name)s: %(message)s',
        datefmt='%H:%M:%S',
        handlers=[
            logging.StreamHandler(sys.stdout),
        ]
    )


def check_admin():
    """检查是否有管理员权限"""
    try:
        return ctypes.windll.shell32.IsUserAnAdmin() != 0
    except Exception:
        return False


def run_smoke_test() -> int:
    """执行最小 frozen/source 启动烟雾验证。

    这个模式刻意不打开托盘、不扫描 HID、不拉起 GUI，
    只验证以下关键链路是否还能完成最小初始化：
    - 公开壳入口可启动
    - 私有核心桥接层可导入
    - 配置管理器与设备管理器可完成轻量初始化

    这样可以把验证目标聚焦在“打包产物是否缺模块/导入即崩”，
    而不是把硬件环境差异混进 smoke test 结果里。
    """
    try:
        import core_bridge  # noqa: F401  # 显式验证私有核心桥接层可导入

        config_manager = ConfigManager()
        device_manager = DeviceManager(config_manager=config_manager)

        logging.getLogger(__name__).info(
            "smoke test passed: bridge/config/device-manager initialized"
        )

        # smoke test 不触碰 HID，但仍显式释放管理器，
        # 保持这个模式与主程序退出路径的资源语义一致。
        device_manager.shutdown()
        return 0
    except Exception as e:
        logging.getLogger(__name__).error(f"smoke test failed: {type(e).__name__}: {e}")
        return 1


_settings_processes = []
_gui_controller = None


def start_update_shutdown_watchdog(current_pid: int, on_shutdown=None):
    """监听热更新退出请求，优雅关闭主进程。"""

    def worker():
        global _shutdown_for_update
        while True:
            try:
                request = updater.consume_shutdown_request(current_pid)
            except Exception as e:
                logging.getLogger(__name__).error(f"读取热更新退出请求失败: {e}")
                time.sleep(1.0)
                continue
            if request:
                _shutdown_for_update = request.get('reason') == 'update'
                logging.getLogger(__name__).info("收到热更新退出请求，准备优雅关闭主进程")
                if on_shutdown is not None:
                    on_shutdown()
                else:
                    _thread.interrupt_main()
                return
            time.sleep(0.5)

    threading.Thread(target=worker, daemon=True, name="update-shutdown-watchdog").start()





def _bring_existing_settings_to_front():
    """激活现有设置窗口，保持重复打开时不重新创建进程。"""
    if os.name != 'nt':
        return False
    user32 = ctypes.windll.user32
    user32.FindWindowW.argtypes = [ctypes.c_wchar_p, ctypes.c_wchar_p]
    user32.FindWindowW.restype = ctypes.c_void_p
    hwnd = user32.FindWindowW(None, '无线设备电量监控')
    if not hwnd:
        return False
    # 首屏尚未就绪的隐藏窗体由 GUI 控制器接管，不能提前激活。
    user32.IsWindowVisible.argtypes = [ctypes.c_void_p]
    user32.IsWindowVisible.restype = ctypes.c_bool
    if not user32.IsWindowVisible(hwnd):
        return False
    user32.ShowWindow.argtypes = [ctypes.c_void_p, ctypes.c_int]
    user32.SetForegroundWindow.argtypes = [ctypes.c_void_p]
    user32.ShowWindow(hwnd, 9)
    user32.SetForegroundWindow(hwnd)
    return True


def open_settings_window(anchor=None):
    """已有设置先激活；否则由进程控制器打开或切换设置视图。"""
    global _settings_processes
    active = [process for process in _settings_processes if process.poll() is None]
    if active and _bring_existing_settings_to_front():
        if _gui_controller is not None:
            _gui_controller.open('settings', anchor)
        return
    if _gui_controller is None:
        raise RuntimeError('托盘 GUI 进程控制器尚未初始化')
    _gui_controller.open('settings', anchor)
    _settings_processes = [_gui_controller.process]


if __name__ == '__main__':
    setup_logging()
    initialize_dpi_awareness()
    updater.prepare_update_startup()
    logger = logging.getLogger(__name__)

    # `--smoke-test` 用于 CI / 本地打包后的最小启动验证
    if len(sys.argv) > 1 and sys.argv[1] == '--smoke-test':
        sys.exit(run_smoke_test())

    # 若带有 GUI 启动参数，则作为独立 GUI 子进程运行（免硬件扫描极速秒开，关闭即销毁释放全部内存）
    if len(sys.argv) > 1 and sys.argv[1] in ('--flyout', '--settings', '--gui'):
        mode = 'settings' if sys.argv[1] in ('--settings', '--gui') else 'flyout'
        from flyout import launch_flyout_process
        sys.exit(launch_flyout_process(mode=mode))

    # 命名互斥体单实例保护（仅对后台托盘守护主进程生效）
    if not acquire_single_instance():
        logger.info("主程序实例已存在，本次启动已忽略")
        sys.exit(0)

    logger.info("=" * 50)
    logger.info("无线设备电量监控程序启动")
    logger.info("=" * 50)

    if not check_admin():
        logger.warning("未以管理员身份运行，可能无法访问部分 HID 设备")

    # 初始化配置管理器与硬件设备管理器
    cleanup_stale_shared_state_files()
    config_manager = ConfigManager()
    device_manager = DeviceManager(config_manager=config_manager)

    # GUI 子进程控制只发窗口消息，关闭前由 GUI 等待下载清理。
    _gui_controller = GuiProcessController({
        'spawn': subprocess.Popen,
        'executable': sys.executable,
        'entry': os.path.abspath(__file__),
        'frozen': bool(getattr(sys, 'frozen', False)),
        'host_pid': os.getpid(),
        'creationflags': subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0,
        'allow_foreground': allow_gui_foreground,
        'on_error': lambda detail: tray.notify_gui_error(detail),
    })
    atexit.register(_gui_controller.shutdown)
    tray = TrayApp(
        device_manager=device_manager,
        config_manager=config_manager,
        on_open_flyout=lambda anchor: _gui_controller.open('flyout', anchor),
        on_open_settings=open_settings_window,
        on_exit=_gui_controller.shutdown,
        capture_window_anchor=capture_tray_anchor,
    )

    # 注册热更新退出看门狗
    start_update_shutdown_watchdog(os.getpid(), on_shutdown=tray.request_exit)

    # 主线程运行托盘，WebView 按需在独立子进程启动。
    try:
        tray.start(detached=False)
    except KeyboardInterrupt:
        logger.info("收到中断信号，程序开始退出")
    except Exception as e:
        logger.error(f"托盘运行异常: {e}")
    finally:
        _gui_controller.shutdown()
        tray.stop()
        logger.info("无线设备电量监控程序已退出")

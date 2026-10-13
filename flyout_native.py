"""将原生窗口操作派发到 WinForms UI 线程，不在该线程等待 JS。"""
import ctypes
import sys
from concurrent.futures import Future, TimeoutError
from ctypes import wintypes

from flyout_geometry import WINDOW_SIZES, capture_cursor_anchor, window_bounds
from flyout_material import apply_material_on_ui, WINDOWS_11_BUILD
from flyout_win32 import RECT, native_library, user32

# Windows 10 沿用原圆角半径；Windows 11 由 DWM 绘制圆角。
WINDOW_RADIUS = 16
# 定位只修改边界，窗口显隐和激活由生命周期单独处理。
SWP_NOACTIVATE = 0x0010
SWP_NOZORDER = 0x0004
# 标题栏拖动消息，由 Windows 负责移动窗口。
WM_NCLBUTTONDOWN = 0x00A1
HTCAPTION = 2
NATIVE_DISPATCH_TIMEOUT_SECONDS = 5.0


def run_on_ui(window, operation):
    """原生操作在 UI 线程执行；后台派发有界，关闭时不永久困在 Invoke。"""
    from System import Action
    form = window.native
    if not form.InvokeRequired:
        return operation()
    future = Future()

    def invoke():
        if not future.set_running_or_notify_cancel():
            return
        try:
            future.set_result(operation())
        except Exception as error:
            future.set_exception(error)

    form.BeginInvoke(Action(invoke))
    try:
        return future.result(timeout=NATIVE_DISPATCH_TIMEOUT_SECONDS)
    except TimeoutError as error:
        future.cancel()
        raise TimeoutError('GUI 原生窗口线程超过 5 秒未响应') from error


def window_handle(window):
    """只在窗体线程获取句柄，调用者缓存它用于后台失焦查询。"""
    if window.native.InvokeRequired:
        raise RuntimeError('窗口句柄必须在 UI 线程读取')
    return int(window.native.Handle.ToInt64())


def position_window(window, mode, anchor=None):
    """在 UI 线程设置物理边界，显隐前后不再使用不同 DPI 坐标。"""
    target = anchor if anchor is not None else capture_cursor_anchor()
    bounds = window_bounds(mode, target)
    run_on_ui(window, lambda: _position_on_ui(window, mode, bounds))


def _position_on_ui(window, mode, bounds):
    """同步托管边界和置顶状态，避免 Show 覆盖坐标或跨线程重入。"""
    from System.Drawing import Rectangle
    x, y, width, height = bounds
    form = window.native
    form.TopMost = mode == 'flyout'
    form.Bounds = Rectangle(x, y, width, height)
    hwnd = window_handle(window)
    if not user32().SetWindowPos(hwnd, None, x, y, width, height, SWP_NOACTIVATE | SWP_NOZORDER):
        raise ctypes.WinError(ctypes.get_last_error())
    if sys.getwindowsversion().build < WINDOWS_11_BUILD:
        apply_legacy_corners(hwnd, width / WINDOW_SIZES[mode][0])


def apply_legacy_corners(hwnd, scale):
    """Windows 10 保留原圆角区域，失败时释放未交接句柄并报告。"""
    rect = RECT()
    if not user32().GetWindowRect(hwnd, ctypes.byref(rect)):
        raise ctypes.WinError(ctypes.get_last_error())
    gdi = native_library('gdi32', {
        'CreateRoundRectRgn': ([ctypes.c_int] * 6, wintypes.HRGN),
        'DeleteObject': ([wintypes.HANDLE], wintypes.BOOL),
    })
    diameter = int(WINDOW_RADIUS * scale * 2)
    region = gdi.CreateRoundRectRgn(0, 0, rect.right - rect.left + 1,
                                  rect.bottom - rect.top + 1, diameter, diameter)
    if not region:
        raise ctypes.WinError(ctypes.get_last_error())
    if not user32().SetWindowRgn(hwnd, region, True):
        error = ctypes.get_last_error()
        gdi.DeleteObject(region)
        raise ctypes.WinError(error)


def apply_acrylic(window, dark):
    """背景和材质在窗体线程配置，原生失败不退化成 CSS 假磨砂。"""
    run_on_ui(window, lambda: apply_material_on_ui(window, dark))


def drag_window(window):
    """拖动进入 UI 消息队列，桥接线程不等待系统拖动模态循环。"""
    from System import Action
    window.native.BeginInvoke(Action(lambda: _drag_on_ui(window)))


def _drag_on_ui(window):
    """由窗体线程释放鼠标捕获并启动系统标题栏拖动。"""
    user32().ReleaseCapture()
    user32().SendMessageW(window_handle(window), WM_NCLBUTTONDOWN, HTCAPTION, 0)

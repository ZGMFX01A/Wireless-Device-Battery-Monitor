"""采集并校验窗口锚点；跨进程只传递物理像素坐标。"""
import ctypes
import math
from ctypes import wintypes
from types import MappingProxyType

from flyout_win32 import POINT, RECT, MONITORINFO, native_library, user32, check_hresult

# 页面尺寸仍使用原设计的逻辑像素，原生定位时仅换算一次。
WINDOW_SIZES = MappingProxyType({'window': (540, 680), 'flyout': (380, 540)})
# 浮窗与工作区边缘的物理像素间距，保持原有值。
FLYOUT_MARGIN = 12
# Windows DPI 基准与最近显示器选择策略。
BASE_DPI = 96
MONITOR_DEFAULTTONEAREST = 2
# Per-Monitor V2 使跨屏坐标、窗体大小与 WebView 缩放处于同一体系。
DPI_CONTEXT_PER_MONITOR_V2 = -4
# 当前 pystray Win32 后端给结构传入 hID，实际 uID 字段保持 0；每个图标以 HWND 区分。
PYSTRAY_NOTIFICATION_ID = 0


def initialize_dpi_awareness():
    """在任何窗体创建前设置 DPI；清单已设置同一模式时保持它。"""
    api = native_library('user32', {
        'SetProcessDpiAwarenessContext': ([ctypes.c_void_p], wintypes.BOOL),
        'GetThreadDpiAwarenessContext': ([], ctypes.c_void_p),
        'AreDpiAwarenessContextsEqual': ([ctypes.c_void_p, ctypes.c_void_p], wintypes.BOOL),
    })
    context = DPI_CONTEXT_PER_MONITOR_V2
    if api.AreDpiAwarenessContextsEqual(api.GetThreadDpiAwarenessContext(), context):
        return
    if not api.SetProcessDpiAwarenessContext(context):
        raise ctypes.WinError(ctypes.get_last_error())


class NOTIFYICONIDENTIFIER(ctypes.Structure):
    _fields_ = [('cbSize', wintypes.DWORD), ('hWnd', wintypes.HWND),
                ('uID', wintypes.UINT), ('guidItem', ctypes.c_byte * 16)]


def capture_tray_anchor(icon):
    """点击时采集 pystray 通知图标矩形，不等待 GUI 加载后再读鼠标。"""
    identifier = NOTIFYICONIDENTIFIER()
    identifier.cbSize = ctypes.sizeof(identifier)
    identifier.hWnd = icon._hwnd
    identifier.uID = PYSTRAY_NOTIFICATION_ID
    rect = RECT()
    shell = native_library('shell32', {
        'Shell_NotifyIconGetRect': ([ctypes.POINTER(NOTIFYICONIDENTIFIER), ctypes.POINTER(RECT)], ctypes.c_long),
    })
    check_hresult(shell.Shell_NotifyIconGetRect(ctypes.byref(identifier), ctypes.byref(rect)),
                  'Shell_NotifyIconGetRect')
    return anchor_from_rect(rect)


def capture_cursor_anchor():
    """独立 GUI 没有通知图标，启动时固定当前显示器和光标位置。"""
    point = POINT()
    if not user32().GetCursorPos(ctypes.byref(point)):
        raise ctypes.WinError(ctypes.get_last_error())
    return anchor_from_rect(RECT(point.x, point.y, point.x + 1, point.y + 1))


def anchor_from_rect(rect):
    """采集图标所在显示器的物理工作区和有效 DPI。"""
    point = POINT((rect.left + rect.right) // 2, (rect.top + rect.bottom) // 2)
    monitor = user32().MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST)
    info = MONITORINFO()
    info.cbSize = ctypes.sizeof(info)
    if not user32().GetMonitorInfoW(monitor, ctypes.byref(info)):
        raise ctypes.WinError(ctypes.get_last_error())
    shcore = native_library('shcore', {
        'GetDpiForMonitor': ([wintypes.HMONITOR, ctypes.c_int, ctypes.POINTER(wintypes.UINT),
                             ctypes.POINTER(wintypes.UINT)], ctypes.c_long),
    })
    dpi_x, dpi_y = wintypes.UINT(), wintypes.UINT()
    check_hresult(shcore.GetDpiForMonitor(monitor, 0, ctypes.byref(dpi_x), ctypes.byref(dpi_y)),
                  'GetDpiForMonitor')
    return validate_anchor({'rect': rect_values(rect), 'work_area': rect_values(info.rcWork),
                            'scale': dpi_x.value / BASE_DPI})


def rect_values(rect):
    """将 Win32 矩形转为可序列化坐标，不传递原生句柄。"""
    return [rect.left, rect.top, rect.right, rect.bottom]


def validate_anchor(anchor):
    """在消息边界校验矩形和 DPI，拒绝损坏数据与固定屏幕回退。"""
    if not isinstance(anchor, dict) or set(anchor) != {'rect', 'work_area', 'scale'}:
        raise ValueError('非法窗口锚点')
    for key in ('rect', 'work_area'):
        rect = anchor[key]
        if not isinstance(rect, (list, tuple)) or len(rect) != 4:
            raise ValueError(f'非法窗口锚点矩形: {key}')
        if any(type(value) is not int for value in rect) or rect[2] <= rect[0] or rect[3] <= rect[1]:
            raise ValueError(f'非法窗口锚点坐标: {key}')
    scale = anchor['scale']
    if type(scale) not in (int, float) or not math.isfinite(scale) or scale <= 0:
        raise ValueError('非法窗口锚点缩放')
    return {'rect': list(anchor['rect']), 'work_area': list(anchor['work_area']), 'scale': float(scale)}


def window_bounds(mode, anchor):
    """设置窗口居中；浮窗贴近通知图标，并限制在目标工作区。"""
    anchor = validate_anchor(anchor)
    left, top, right, bottom = anchor['work_area']
    logical_width, logical_height = WINDOW_SIZES[mode]
    width, height = (int(size * anchor['scale']) for size in (logical_width, logical_height))
    if mode == 'window':
        x, y = left + (right - left - width) // 2, top + (bottom - top - height) // 2
    else:
        x, y = flyout_origin(anchor['rect'], (width, height), anchor['work_area'])
    x = max(left, min(x, right - width - FLYOUT_MARGIN))
    y = max(top, min(y, bottom - height - FLYOUT_MARGIN))
    return x, y, width, height


def flyout_origin(rect, size, work_area):
    """按任务栏所在边缘放置浮窗；溢出区仍使用图标实际矩形。"""
    left, top, right, _ = work_area
    width, height = size
    x = (rect[0] + rect[2] - width) // 2
    y = rect[1] - height - FLYOUT_MARGIN
    if rect[2] <= left:
        return left + FLYOUT_MARGIN, (rect[1] + rect[3] - height) // 2
    if rect[0] >= right:
        return right - width - FLYOUT_MARGIN, (rect[1] + rect[3] - height) // 2
    if y < top:
        y = rect[3] + FLYOUT_MARGIN
    return x, y

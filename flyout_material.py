"""原生宿主背景合成；网页布局、配色和滤镜保持原样。"""
import ctypes
import sys
from ctypes import wintypes

from flyout_win32 import native_library, check_hresult

# Windows 11 初始版提供系统圆角，22H2 提供系统背景材质。
WINDOWS_11_BUILD = 22000
DWM_BACKDROP_BUILD = 22621
# DWM 深色主题、圆角和桌面 Acrylic 属性及对应值。
DWMWA_USE_IMMERSIVE_DARK_MODE = 20
DWMWA_WINDOW_CORNER_PREFERENCE = 33
DWMWA_SYSTEMBACKDROP_TYPE = 38
DWMWCP_ROUND = 2
DWMSBT_TRANSIENTWINDOW = 3
# 负边距将 DWM 合成扩展到整个客户区。
FULL_GLASS_MARGIN = -1
# Windows 10 沿用已有 AcrylicBlurBehind 策略、混合标志和色调。
WCA_ACCENT_POLICY = 19
ACCENT_ENABLE_ACRYLICBLURBEHIND = 4
ACCENT_BLEND_FLAGS = 2
ACRYLIC_DARK_COLOR = 0x80101828
ACRYLIC_LIGHT_COLOR = 0x80F0F4F8


class MARGINS(ctypes.Structure):
    _fields_ = [('left', ctypes.c_int), ('right', ctypes.c_int),
                ('top', ctypes.c_int), ('bottom', ctypes.c_int)]


def material_handle(window):
    """材质仅允许在窗体线程配置，禁止绕过派发入口。"""
    if window.native.InvokeRequired:
        raise RuntimeError('原生背景必须在 UI 线程配置')
    return int(window.native.Handle.ToInt64())


def prepare_surface(window):
    """由宿主管理透明，避免 pywebview 透明导航回调提前显示。"""
    from System.Drawing import Color
    hwnd = material_handle(window)
    window.native.BackColor = Color.Black
    window.native.webview.DefaultBackgroundColor = Color.Transparent
    margins = MARGINS(FULL_GLASS_MARGIN, FULL_GLASS_MARGIN, FULL_GLASS_MARGIN, FULL_GLASS_MARGIN)
    dwm = native_library('dwmapi', {
        'DwmExtendFrameIntoClientArea': ([wintypes.HWND, ctypes.POINTER(MARGINS)], ctypes.c_long),
    })
    check_hresult(dwm.DwmExtendFrameIntoClientArea(hwnd, ctypes.byref(margins)),
                  'DwmExtendFrameIntoClientArea')


def apply_material_on_ui(window, dark):
    """按已有系统支持范围选择材质，检查每个 HRESULT 或 BOOL 结果。"""
    prepare_surface(window)
    hwnd = material_handle(window)
    build = sys.getwindowsversion().build
    if build >= WINDOWS_11_BUILD:
        set_dwm_attribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND)
    if build < DWM_BACKDROP_BUILD:
        apply_windows10_acrylic(hwnd, dark)
        return
    set_dwm_attribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, int(dark))
    set_dwm_attribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_TRANSIENTWINDOW)


def set_dwm_attribute(hwnd, attribute, value):
    """配置系统合成属性，保留失败的属性编号和 HRESULT。"""
    setting = ctypes.c_int(value)
    dwm = native_library('dwmapi', {
        'DwmSetWindowAttribute': ([wintypes.HWND, wintypes.DWORD, ctypes.c_void_p, wintypes.DWORD], ctypes.c_long),
    })
    check_hresult(dwm.DwmSetWindowAttribute(hwnd, attribute, ctypes.byref(setting), ctypes.sizeof(setting)),
                  f'DwmSetWindowAttribute({attribute})')


class Accent(ctypes.Structure):
    _fields_ = [('state', ctypes.c_int), ('flags', ctypes.c_int),
                ('color', ctypes.c_uint), ('animation', ctypes.c_int)]


class Composition(ctypes.Structure):
    _fields_ = [('attribute', ctypes.c_int), ('data', ctypes.c_void_p), ('size', ctypes.c_size_t)]


def apply_windows10_acrylic(hwnd, dark):
    """保留早期系统现有 Acrylic 接口，失败报告实际 Win32 错误。"""
    accent = Accent(ACCENT_ENABLE_ACRYLICBLURBEHIND, ACCENT_BLEND_FLAGS,
                    ACRYLIC_DARK_COLOR if dark else ACRYLIC_LIGHT_COLOR, 0)
    composition = Composition(WCA_ACCENT_POLICY, ctypes.cast(ctypes.byref(accent), ctypes.c_void_p),
                              ctypes.sizeof(accent))
    api = native_library('user32', {
        'SetWindowCompositionAttribute': ([wintypes.HWND, ctypes.POINTER(Composition)], wintypes.BOOL),
    })
    if not api.SetWindowCompositionAttribute(hwnd, ctypes.byref(composition)):
        raise ctypes.WinError(ctypes.get_last_error())

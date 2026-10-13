"""浮窗所需的 Win32 类型、函数签名和明确的错误转换。"""
import ctypes
from ctypes import wintypes


class RECT(ctypes.Structure):
    _fields_ = [('left', wintypes.LONG), ('top', wintypes.LONG),
                ('right', wintypes.LONG), ('bottom', wintypes.LONG)]


class POINT(ctypes.Structure):
    _fields_ = [('x', wintypes.LONG), ('y', wintypes.LONG)]


class MONITORINFO(ctypes.Structure):
    _fields_ = [('cbSize', wintypes.DWORD), ('rcMonitor', RECT),
                ('rcWork', RECT), ('dwFlags', wintypes.DWORD)]


def native_library(name, signatures):
    """声明 Windows 指针宽度，避免 ctypes 默认 c_int 截断原生句柄。"""
    library = ctypes.WinDLL(name, use_last_error=True)
    for symbol, (arguments, result) in signatures.items():
        function = getattr(library, symbol)
        function.argtypes, function.restype = arguments, result
    return library


def check_hresult(result, operation):
    """HRESULT 使用符号位表示失败，材质失败不能被伪装成成功。"""
    if result < 0:
        raise OSError(f'{operation} 失败，HRESULT=0x{result & 0xffffffff:08X}')


def user32():
    """返回窗口操作所需的带类型 WinAPI。"""
    return native_library('user32', {
        'GetCursorPos': ([ctypes.POINTER(POINT)], wintypes.BOOL),
        'MonitorFromPoint': ([POINT, wintypes.DWORD], wintypes.HMONITOR),
        'GetMonitorInfoW': ([wintypes.HMONITOR, ctypes.POINTER(MONITORINFO)], wintypes.BOOL),
        'GetWindowRect': ([wintypes.HWND, ctypes.POINTER(RECT)], wintypes.BOOL),
        'GetForegroundWindow': ([], wintypes.HWND),
        'SetForegroundWindow': ([wintypes.HWND], wintypes.BOOL),
        'SetWindowPos': ([wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int,
                          ctypes.c_int, ctypes.c_int, wintypes.UINT], wintypes.BOOL),
        'SetWindowRgn': ([wintypes.HWND, wintypes.HRGN, wintypes.BOOL], ctypes.c_int),
        'ReleaseCapture': ([], wintypes.BOOL),
        'SendMessageW': ([wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM], wintypes.LPARAM),
    })

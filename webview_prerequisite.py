"""在创建 WinForms 前检测 WebView2；缺环境时使用不依赖网页的安装引导。"""
import ctypes
import json
from pathlib import Path
import sysconfig
import webbrowser

WEBVIEW2_INSTALL_URL = 'https://developer.microsoft.com/microsoft-edge/webview2/'
ERROR_FILE_NOT_FOUND_HRESULT = 0x80070002
MB_YESNO_WARNING_DEFAULT_NO = 0x04 | 0x30 | 0x100
IDYES = 6


class WebView2Unavailable(RuntimeError):
    pass


def detect_webview2(runtime_path=None):
    """调用官方 Loader API，支持系统 Evergreen 和显式配置的 Fixed Runtime。"""
    from webview.util import get_app_root, interop_dll_path
    platform = sysconfig.get_platform().lower()
    architecture = 'win-arm64' if 'arm64' in platform else (
        'win-x64' if ctypes.sizeof(ctypes.c_void_p) == 8 else 'win-x86')
    folder = None
    if runtime_path:
        path = Path(runtime_path)
        folder = str(path if path.is_absolute() else Path(get_app_root()) / path)
        if not Path(folder).is_dir():
            raise RuntimeError(f'WebView2 Runtime 路径不存在：{folder}')
    loader = ctypes.WinDLL(str(Path(interop_dll_path(architecture)) / 'WebView2Loader.dll'))
    probe = loader.GetAvailableCoreWebView2BrowserVersionString
    probe.argtypes = [ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_void_p)]
    probe.restype = ctypes.c_long
    ole = ctypes.WinDLL('ole32')
    ole.CoTaskMemFree.argtypes = [ctypes.c_void_p]
    ole.CoTaskMemFree.restype = None
    version = ctypes.c_void_p()
    try:
        result = probe(folder, ctypes.byref(version)) & 0xFFFFFFFF
        if result == ERROR_FILE_NOT_FOUND_HRESULT:
            raise WebView2Unavailable('Microsoft Edge WebView2 Runtime 未安装或指定的运行时不可用。')
        if result & 0x80000000:
            raise RuntimeError(f'WebView2 环境检测失败：HRESULT=0x{result:08X}')
        value = ctypes.wstring_at(version) if version.value else ''
        if not value or value == '0.0.0.0':
            raise WebView2Unavailable('未检测到可用的 Microsoft Edge WebView2 Runtime。')
        return value
    finally:
        if version.value:
            ole.CoTaskMemFree(version)


def _language():
    """只读语言偏好，不创建 ConfigManager，不修改注册表或配置。"""
    from config import CONFIG_FILE
    from i18n import resolve_ui_language
    try:
        config = json.loads(Path(CONFIG_FILE).read_text(encoding='utf-8'))
        value = config.get('ui_language', 'auto') if isinstance(config, dict) else 'auto'
    except (OSError, ValueError):
        value = 'auto'
    return resolve_ui_language(value)


def show_runtime_guidance(error, *, message_box=None, open_url=None, language=None):
    """只有点击“是”才打开官方页面；不下载、不安装、不退回旧 IE 引擎。"""
    if message_box is None:
        from ctypes import wintypes
        api = ctypes.WinDLL('user32', use_last_error=True)
        message_box = api.MessageBoxW
        message_box.argtypes = [wintypes.HWND, ctypes.c_wchar_p, ctypes.c_wchar_p, wintypes.UINT]
        message_box.restype = ctypes.c_int
    open_url = open_url or webbrowser.open
    english = (language or _language()) == 'en-US'
    title = 'WebView2 Runtime required' if english else '需要安装 WebView2 Runtime'
    text = (
        'The interface requires Microsoft Edge WebView2 Runtime.\n'
        'The tray monitor is not stopped. After installation, open the interface again from the tray.\n'
        'Offline computers can use the Evergreen Standalone Installer.\n\n'
        'Open the official Microsoft download page now?'
        if english else
        '当前电脑缺少可用的 Microsoft Edge WebView2 Runtime，暂时无法打开界面。\n'
        '本次操作不会停止托盘后台监控。安装完成后，再次从托盘打开即可。\n'
        '无网络的电脑可使用官方 Evergreen Standalone Installer 离线安装。\n\n'
        '是否现在打开微软官方下载页面？'
    )
    answer = message_box(None, text + '\n\n' + str(error), title, MB_YESNO_WARNING_DEFAULT_NO)
    if answer != IDYES:
        return False
    try:
        if not open_url(WEBVIEW2_INSTALL_URL):
            raise RuntimeError('browser did not open')
    except Exception:
        detail = ('Open this address in your browser:\n' if english else '浏览器未能打开，请手动访问：\n')
        message_box(None, detail + WEBVIEW2_INSTALL_URL, title, 0x10)
        return False
    return True

"""托盘收到用户输入后，仅向本次 GUI 进程交接前台激活权限。"""
import ctypes
from ctypes import wintypes
from dataclasses import dataclass
import logging

from flyout_win32 import native_library

logger = logging.getLogger(__name__)
# 进程快照只用于辨认 onefile 启动器与实际 GUI，不申请进程写入权限。
TH32CS_SNAPPROCESS = 0x00000002
INVALID_HANDLE_VALUE = ctypes.c_void_p(-1).value
ERROR_NO_MORE_FILES = 18
# Windows PROCESSENTRY32W 为可执行文件名预留的字符数。
PROCESS_NAME_LENGTH = 260


class ProcessEntry(ctypes.Structure):
    """Windows 进程快照结构，堆句柄使用指针宽度以兼容 64 位。"""

    _fields_ = [
        ('dwSize', wintypes.DWORD), ('cntUsage', wintypes.DWORD),
        ('th32ProcessID', wintypes.DWORD), ('th32DefaultHeapID', ctypes.c_size_t),
        ('th32ModuleID', wintypes.DWORD), ('cntThreads', wintypes.DWORD),
        ('th32ParentProcessID', wintypes.DWORD), ('pcPriClassBase', wintypes.LONG),
        ('dwFlags', wintypes.DWORD), ('szExeFile', wintypes.WCHAR * PROCESS_NAME_LENGTH),
    ]


@dataclass(frozen=True)
class ProcessIdentity:
    """保留前台授权所需的进程身份，不持有可写进程句柄。"""

    pid: int
    parent_pid: int
    executable: str


def snapshot_processes():
    """读取进程父子关系；快照中的错误必须明确报告。"""
    kernel = native_library('kernel32', {
        'CreateToolhelp32Snapshot': ([wintypes.DWORD, wintypes.DWORD], wintypes.HANDLE),
        'Process32FirstW': ([wintypes.HANDLE, ctypes.POINTER(ProcessEntry)], wintypes.BOOL),
        'Process32NextW': ([wintypes.HANDLE, ctypes.POINTER(ProcessEntry)], wintypes.BOOL),
        'CloseHandle': ([wintypes.HANDLE], wintypes.BOOL),
    })
    handle = kernel.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
    if handle == INVALID_HANDLE_VALUE:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        entry = ProcessEntry()
        entry.dwSize = ctypes.sizeof(entry)
        available = kernel.Process32FirstW(handle, ctypes.byref(entry))
        processes = []
        while available:
            processes.append(ProcessIdentity(entry.th32ProcessID,
                                             entry.th32ParentProcessID, entry.szExeFile))
            available = kernel.Process32NextW(handle, ctypes.byref(entry))
        error = ctypes.get_last_error()
        if error != ERROR_NO_MORE_FILES:
            raise ctypes.WinError(error)
        return tuple(processes)
    finally:
        kernel.CloseHandle(handle)


def gui_process_id(launcher_pid, processes):
    """源码版使用启动 PID；onefile 使用同名启动器的实际执行子进程。"""
    launcher = next((item for item in processes if item.pid == launcher_pid), None)
    if launcher is None:
        raise ProcessLookupError(f'GUI 启动进程已经退出：{launcher_pid}')
    workers = tuple(item for item in processes
                    if item.parent_pid == launcher_pid
                    and item.executable.casefold() == launcher.executable.casefold())
    if len(workers) > 1:
        raise RuntimeError(f'GUI 启动器存在多个同名子进程：{launcher_pid}')
    return workers[0].pid if workers else launcher_pid


def allow_gui_foreground(launcher_pid):
    """把本次托盘点击取得的权限授予 GUI，避免复用窗口被 Windows 拒绝激活。"""
    try:
        target_pid = gui_process_id(launcher_pid, snapshot_processes())
    except ProcessLookupError:
        # 闲置回收可能刚好结束启动器；仍由控制器排队并按退出回执补送打开请求。
        logger.info('GUI 在前台授权前已退出，保留打开请求：launcher_pid=%s', launcher_pid)
        return False
    api = native_library('user32', {
        'AllowSetForegroundWindow': ([wintypes.DWORD], wintypes.BOOL),
    })
    allowed = bool(api.AllowSetForegroundWindow(target_pid))
    if not allowed:
        # 用户已切走或打开请求并非来自用户输入时，Windows 可以拒绝授权。
        logger.warning('Windows 拒绝 GUI 前台授权：launcher_pid=%s gui_pid=%s error=%s',
                       launcher_pid, target_pid, ctypes.get_last_error())
    return allowed

"""托盘进程的睡眠/唤醒回调；不创建 GUI，不订阅设备或 USB 插拔通知。"""
import ctypes
from ctypes import wintypes
import logging

logger = logging.getLogger(__name__)
PBT_APMSUSPEND = 4
PBT_APMRESUMESUSPEND = 7
PBT_APMRESUMEAUTOMATIC = 18
DEVICE_NOTIFY_CALLBACK = 2
PowerCallback = ctypes.WINFUNCTYPE(wintypes.ULONG, ctypes.c_void_p, wintypes.ULONG, ctypes.c_void_p)


class SubscribeParameters(ctypes.Structure):
    _fields_ = [('Callback', PowerCallback), ('Context', ctypes.c_void_p)]


class PowerNotifications:
    def __init__(self, notify):
        self._notify = notify
        self._registration = ctypes.c_void_p()
        self._callback = PowerCallback(self._receive)
        self._parameters = SubscribeParameters(self._callback, None)
        self._api = None

    def start(self):
        if self._registration.value:
            return True
        try:
            self._api = ctypes.WinDLL('powrprof', use_last_error=True)
            self._api.PowerRegisterSuspendResumeNotification.argtypes = [
                wintypes.DWORD, ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p)]
            self._api.PowerRegisterSuspendResumeNotification.restype = wintypes.DWORD
            self._api.PowerUnregisterSuspendResumeNotification.argtypes = [ctypes.c_void_p]
            self._api.PowerUnregisterSuspendResumeNotification.restype = wintypes.DWORD
            error = self._api.PowerRegisterSuspendResumeNotification(
                DEVICE_NOTIFY_CALLBACK, ctypes.byref(self._parameters), ctypes.byref(self._registration))
            if error:
                raise ctypes.WinError(error)
            return True
        except Exception:
            logger.exception('注册电源恢复通知失败，保留正常轮询与手动刷新')
            return False

    def _receive(self, context, event, setting):
        try:
            if event in (PBT_APMSUSPEND, PBT_APMRESUMESUSPEND, PBT_APMRESUMEAUTOMATIC):
                self._notify(int(event))  # 只能标记/唤醒调度器，不能在回调里读取或关闭 HID。
        except Exception:
            logger.exception('电源事件投递失败')
        return 0

    def close(self):
        if self._registration.value:
            error = self._api.PowerUnregisterSuspendResumeNotification(self._registration)
            if error:
                logger.error('取消电源通知失败：%s', error)
                return
            self._registration = ctypes.c_void_p()

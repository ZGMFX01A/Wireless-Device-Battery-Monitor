"""托盘与 WebView 子进程之间的窗口控制；避免强杀下载线程。"""
import json
import logging
import os
from pathlib import Path
import re
import tempfile
import threading
import ctypes
import time
import uuid

from flyout_geometry import capture_cursor_anchor, validate_anchor
from rtss_compat import disable_rtss_hooks

logger = logging.getLogger(__name__)
# 提升窗口响应速度的窗口消息轮询间隔（50ms 确保零延迟瞬时感知）。
GUI_MESSAGE_POLL_SECONDS = 0.05
# Windows 只读进程查询权限与存活状态码，不使用 os.kill(pid, 0)。
PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
STILL_ACTIVE = 259
ERROR_INVALID_PARAMETER = 87
# 允许的跨进程窗口动作，损坏消息必须报错而非触发其他动作。
GUI_ACTIONS = frozenset(('settings', 'toggle', 'shutdown'))
# onefile 的 Popen.pid 是解包启动器，不一定是运行 Python/GUI 的进程。
# 启动前约定独立会话地址，启动器和 GUI 通过继承环境使用同一个收件箱。
GUI_CHANNEL_ENV = 'MOUSE_BATTERY_GUI_CHANNEL'
# GUI 主动闲置退出的专用回执；onefile 启动器会传递子进程退出码。
GUI_IDLE_EXIT_CODE = 75
# 故障回执由子进程写出；与正常关闭、闲置回收分别处理。
GUI_FAILURE_EXIT_CODE = 76
GUI_RECOVERY_LIMIT = 1


def current_gui_channel():
    """托盘子进程使用共享会话；直接启动 GUI 时才使用自身 PID。"""
    channel = os.environ.get(GUI_CHANNEL_ENV)
    if channel is None:
        return os.getpid()
    if not re.fullmatch(r'[0-9a-f]{32}', channel):
        raise ValueError('非法 GUI 会话标识')
    return channel


def message_path(channel):
    """按会话隔离队列；保留独立 GUI 的整数 PID 地址，拒绝任意路径。"""
    if not ((type(channel) is int and channel > 0) or
            (isinstance(channel, str) and re.fullmatch(r'[0-9a-f]{32}', channel))):
        raise ValueError('非法 GUI 窗口通信地址')
    return Path(tempfile.gettempdir()) / 'WirelessDeviceBatteryMonitor' / f'gui_{channel}'


def send_message(channel, action, anchor=None):
    """每条请求独立发布，避免后续退出请求被前一条消息覆盖或删除。"""
    if action not in GUI_ACTIONS:
        raise ValueError('非法 GUI 窗口控制动作')
    directory = message_path(channel)
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / f'{time.time_ns():020d}_{uuid.uuid4().hex}.json'
    temporary = path.with_suffix('.tmp')
    message = {'action': action}
    if anchor is not None:
        message['anchor'] = validate_anchor(anchor)
    temporary.write_text(json.dumps(message), encoding='utf-8')
    os.replace(temporary, path)


def consume_message(channel):
    """按发布顺序原子领取请求；只删除已领取文件，不触及后来入队的消息。"""
    for path in sorted(message_path(channel).glob('*.json')):
        claimed = path.with_suffix(f'.{uuid.uuid4().hex}.processing')
        try:
            path.rename(claimed)
        except FileNotFoundError:
            # 其他消费者已领取该条消息，继续处理队列中剩余请求。
            continue
        try:
            data = json.loads(claimed.read_text(encoding='utf-8'))
            if not isinstance(data, dict) or data.get('action') not in GUI_ACTIONS:
                raise ValueError('非法 GUI 窗口控制消息')
            if 'anchor' in data:
                data['anchor'] = validate_anchor(data['anchor'])
            return data
        finally:
            claimed.unlink()
    return None


def clear_messages(channel):
    """进程退出后清理本会话请求及中断的发布/领取文件，不影响其他 GUI。"""
    directory = message_path(channel)
    for pattern in ('*.json', '*.tmp', '*.processing', '*.result'):
        for path in directory.glob(pattern):
            path.unlink(missing_ok=True)
    if directory.exists():
        directory.rmdir()


def write_gui_failure(channel, request, error):
    """在退出前发布故障和已经消费的显示意图，不混入窗口动作队列。"""
    directory = message_path(channel)
    directory.mkdir(parents=True, exist_ok=True)
    temporary = directory / 'failure.tmp'
    temporary.write_text(json.dumps({'request': request, 'error': str(error)}), encoding='utf-8')
    os.replace(temporary, directory / 'failure.result')


def read_gui_failure(channel):
    """仅在对应进程已退出后读取并校验故障回执。"""
    path = message_path(channel) / 'failure.result'
    if not path.is_file():
        return {'request': None, 'error': 'GUI failure report missing'}
    report = json.loads(path.read_text(encoding='utf-8'))
    if not isinstance(report, dict) or not isinstance(report.get('error'), str):
        raise ValueError('非法 GUI 故障回执')
    request = report.get('request')
    if request is not None:
        if not isinstance(request, dict) or request.get('action') not in ('settings', 'toggle'):
            raise ValueError('非法 GUI 恢复动作')
        report['request'] = {'action': request['action'], 'anchor': validate_anchor(request.get('anchor'))}
    return report


def process_alive(pid):
    """检查宿主是否存在，避免永远等待已退出的托盘。"""
    if pid is None:
        return False
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.OpenProcess.argtypes = [ctypes.c_ulong, ctypes.c_bool, ctypes.c_ulong]
    kernel.OpenProcess.restype = ctypes.c_void_p
    kernel.GetExitCodeProcess.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_ulong)]
    kernel.GetExitCodeProcess.restype = ctypes.c_bool
    kernel.CloseHandle.argtypes = [ctypes.c_void_p]
    handle = kernel.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
    if not handle:
        error = ctypes.get_last_error()
        if error == ERROR_INVALID_PARAMETER:
            return False
        raise ctypes.WinError(error)
    try:
        code = ctypes.c_ulong()
        if not kernel.GetExitCodeProcess(handle, ctypes.byref(code)):
            raise ctypes.WinError(ctypes.get_last_error())
        return code.value == STILL_ACTIVE
    finally:
        kernel.CloseHandle(handle)


class GuiProcessController:
    """串行管理唯一 GUI 子进程，避免托盘回调并发拉起多个窗口。"""

    def __init__(self, options):
        self._spawn = options['spawn']
        self._executable = options['executable']
        self._entry = options['entry']
        self._frozen = options['frozen']
        self._host_pid = options['host_pid']
        self._flags = options['creationflags']
        self._process = None
        self._channel = None
        self._lock = threading.Lock()
        self._stopping = False
        self._last_anchor = None
        self._watch_exit = options.get('watch_exit', True)
        self._recovery_attempts = 0
        self._on_error = options.get('on_error')
        # 托盘组合根注入用户输入权限交接；纯 IPC 消费者不请求前台权限。
        self._allow_foreground = options.get('allow_foreground')

    @property
    def process(self):
        return self._process

    def open(self, mode, anchor=None):
        """复用窗口并传递点击时锚点；独立调用在提交请求时采集光标。"""
        target = validate_anchor(anchor) if anchor is not None else capture_cursor_anchor()
        with self._lock:
            if self._stopping:
                return
            # 显式用户输入开启新的恢复预算，自动重启不能重置预算。
            self._recovery_attempts = 0
            if self._process and self._process.poll() is None:
                self._last_anchor = target
                self._send_window_action('settings' if mode == 'settings' else 'toggle', target)
                return
            if self._channel is not None:
                clear_messages(self._channel)
            self._spawn_locked(mode, target)

    def _send_window_action(self, action, anchor=None):
        """先交接本次点击的激活权限，再让 GUI 控制线程显示窗口。"""
        if self._allow_foreground is not None:
            self._allow_foreground(self._process.pid)
        send_message(self._channel, action, anchor)

    def _spawn_locked(self, mode, target):
        """调用方持有控制器锁，每次新 GUI 都使用新会话地址。"""
        args = [self._executable]
        if not self._frozen:
            args.append(self._entry)
        args.append('--settings' if mode == 'settings' else '--flyout')
        environment = dict(os.environ, MOUSE_BATTERY_HOST_PID=str(self._host_pid))
        # 在子进程创建前声明，覆盖 onefile 启动器、GUI 和后续 WebView2 子进程。
        # environment 是副本；托盘和从桌面启动的其他应用不受影响。
        disable_rtss_hooks(environment)
        environment['MOUSE_BATTERY_WINDOW_ANCHOR'] = json.dumps(target)
        channel = uuid.uuid4().hex
        environment[GUI_CHANNEL_ENV] = channel
        if self._frozen:
            environment['PYINSTALLER_RESET_ENVIRONMENT'] = '1'
        process = self._spawn(args, env=environment, creationflags=self._flags)
        self._process, self._channel, self._last_anchor = process, channel, target
        logger.info('GUI 启动：launcher_pid=%s channel=%s', process.pid, channel)
        if self._watch_exit:
            threading.Thread(target=self._wait_for_exit, args=(process, channel),
                             daemon=True, name='gui-exit-watch').start()

    def _wait_for_exit(self, process, channel):
        """等待句柄而非忙轮询；不占用托盘 UI 线程，不强杀 GUI。"""
        try:
            self._process_exited(process, channel, process.wait())
        except Exception:
            logger.exception('GUI 退出回收失败：channel=%s', channel)

    def _process_exited(self, process, channel, exit_code):
        """回收边界的打开请求必须补送，不能再次出现收回后打不开。"""
        failure = None
        with self._lock:
            if self._process is not process or self._channel != channel:
                return
            pending = None
            if exit_code == GUI_FAILURE_EXIT_CODE and not self._stopping:
                try:
                    failure = read_gui_failure(channel)
                except (OSError, ValueError):
                    # 损坏回执不能让退出监听停在清理前，也不能据此猜测显示意图。
                    logger.exception('GUI 故障回执读取失败：channel=%s', channel)
                    failure = {'request': None, 'error': 'GUI failure report unreadable'}
                pending = failure['request']
            if exit_code in (0, GUI_IDLE_EXIT_CODE, GUI_FAILURE_EXIT_CODE) and not self._stopping:
                while (message := consume_message(channel)) is not None:
                    if message['action'] == 'shutdown':
                        pending = None
                        failure = None
                        break
                    # 退出中的旧窗口尚未领取的请求，以最后一次打开意图为准。
                    pending = message
            clear_messages(channel)
            self._process, self._channel = None, None
            if pending is not None:
                if failure is None or self._recovery_attempts < GUI_RECOVERY_LIMIT:
                    if failure is not None:
                        self._recovery_attempts += 1
                        logger.warning('GUI 故障后自动重建：attempt=%s error=%s',
                                       self._recovery_attempts, failure['error'])
                    mode = 'settings' if pending['action'] == 'settings' else 'flyout'
                    self._spawn_locked(mode, pending.get('anchor', self._last_anchor))
                    return
            else:
                # 闲置隐藏窗口的故障不应主动弹出窗口或通知。
                failure = None
        # 先释放进程和锁，再发送非模态通知，后续点击不必等待“确定”。
        if failure is not None:
            logger.error('GUI 自动恢复已达上限：%s', failure['error'])
            if self._on_error is not None:
                self._on_error(failure['error'])

    def activate(self):
        """激活已打开的设置窗口，不新建进程。"""
        with self._lock:
            if self._stopping or not self._process or self._process.poll() is not None:
                return False
            self._recovery_attempts = 0
            self._send_window_action('settings')
            return True

    def shutdown(self):
        """要求 GUI 取消下载并等待正常释放资源，不强杀更新线程。"""
        with self._lock:
            self._stopping = True
            process = self._process
            channel = self._channel
            if not process:
                return
            if process.poll() is not None:
                clear_messages(channel)
                return
            send_message(channel, 'shutdown')
        process.wait()
        with self._lock:
            clear_messages(channel)
            if self._process is process:
                self._process, self._channel = None, None

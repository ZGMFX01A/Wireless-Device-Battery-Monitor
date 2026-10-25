"""WebView2 浮窗生命周期；硬件仍由独立托盘进程管理。"""
import json
import logging
import os
from pathlib import Path
from queue import Empty, Queue
import sys
import threading
import time
from contextlib import contextmanager

from flyout_geometry import WINDOW_SIZES, window_bounds
from flyout_native import drag_window, user32
from flyout_state import get_flyout_state
from gui_process import consume_message, current_gui_channel, GUI_MESSAGE_POLL_SECONDS

logger = logging.getLogger(__name__)
# 一秒级轮询只读取共享文件，不重复访问硬件。
STATE_REFRESH_SECONDS = 1
# 短时间重开复用 WebView；长期隐藏时归还整个 GUI 进程的内存。
GUI_IDLE_SECONDS = 30.0


class GuiFrontendError(RuntimeError):
    """允许由托盘在独立进程中有限重试的窗口运行故障。"""


class FlyoutManager:
    """串行处理窗口动作，原生操作由注入的宿主派发到窗体线程。"""

    def __init__(self, device_manager, config_manager, options):
        self.device_manager = device_manager
        self.config_manager = config_manager
        self.closed = options['closed']
        self._webview = options['webview']
        self._refresh_tray = options['refresh_tray']
        self._host_factory = options['host_factory']
        self.anchor = options['anchor']
        self._updates = None
        self.mode = 'window' if options['mode'] == 'settings' else 'flyout'
        self.auto_refresh = True
        self._visible = False
        self._requested_window = {
            'action': 'settings' if self.mode == 'window' else 'toggle', 'anchor': self.anchor,
        }
        self.recovery_request = None
        self._ready = threading.Event()
        self._actions = Queue()
        self._message_channel = current_gui_channel()
        self._close_lock = threading.Lock()
        self._state_push_lock = threading.Lock()
        self._last_state = None
        self._closing = False
        self.idle_exit = False
        self._active_operations = 0
        self._idle_since = None
        self._clock = options.get('clock', time.monotonic)
        self._idle_seconds = options.get('idle_seconds', GUI_IDLE_SECONDS)
        self._pending_update = None
        self.startup_error = None
        device_manager.add_on_update(self._on_devices_updated)

    def attach_updates(self, updates):
        """组合根注入更新生命周期，窗口内部不创建下载器。"""
        self._updates = updates

    def create_window(self, api):
        """创建不可见窗口，背景透明由原生宿主配置，避免导航提前 Show。"""
        root = Path(getattr(sys, '_MEIPASS', Path(__file__).parent))
        page = root / 'web_ui' / 'index.html'
        if not page.is_file():
            raise FileNotFoundError(f'未找到 GUI 网页资源：{page}')
        x, y, _, _ = window_bounds(self.mode, self.anchor)
        scale = self.anchor['scale']
        width, height = WINDOW_SIZES[self.mode]
        self.window = self._webview.create_window(
            title='无线设备电量监控', url=str(page), width=width, height=height,
            x=int(x / scale), y=int(y / scale), background_color='#000000',
            resizable=False, frameless=True, easy_drag=False, on_top=self.mode == 'flyout',
            hidden=True, transparent=False, js_api=api)
        self._host = self._host_factory(self.window, {
            'mode': lambda: self.mode, 'anchor': lambda: self.anchor,
            'dark': lambda: self.config_manager.effective_theme_mode == 'dark',
            'error': self.on_frontend_error,
        })
        self.window.events.closing += self._on_closing
        self.window.events.closed += self.closed.set
        return self.window

    def on_frontend_ready(self):
        """首屏完成后入队显示，所有显隐与模式变化由同一控制线程处理。"""
        self._actions.put({'action': 'ready'})

    def on_frontend_error(self, message):
        """关闭故障窗口，保留用户显示意图供托盘有限重建。"""
        with self._close_lock:
            if self._closing:
                return
            self._closing = True
            self.startup_error = GuiFrontendError(f'GUI 初始化或运行失败：{message}')
            self.recovery_request = self._requested_window
        logger.error('%s', self.startup_error)
        threading.Thread(target=self._close, name='gui-fault-close').start()

    def start_controls(self):
        """页面加载期间也接收退出请求，避免隐藏窗口阻塞托盘退出。"""
        logger.info('GUI 控制就绪：pid=%s channel=%s', os.getpid(), self._message_channel)
        threading.Thread(target=self._control_loop, daemon=True, name='gui-control').start()

    def show(self, mode):
        """控制线程先切换内容，再在 UI 线程完成定位、材质和显示。"""
        with self._close_lock:
            if self._closing:
                return
            # 在任何 WebView 调用之前保存目标，显示失败时请求尚未完成。
            self.mode = mode
            self._requested_window = {
                'action': 'settings' if mode == 'window' else 'toggle', 'anchor': self.anchor,
            }
        self._idle_since = None
        self._host.set_background_mode(False)
        self.device_manager.sync_shared_state_silently()
        view = 'settings' if mode == 'window' else 'devices'
        self._host.scripts.evaluate(f'window.__setViewMode({json.dumps(view)});')
        self._host.present({'mode': mode, 'anchor': self.anchor,
                            'dark': self.config_manager.effective_theme_mode == 'dark'})
        self._visible = True
        self._last_state = None
        if self.auto_refresh:
            self.device_manager.start_auto_refresh(STATE_REFRESH_SECONDS)
        self._on_devices_updated()
        with self._state_push_lock:
            if self._pending_update is not None:
                event, self._pending_update = self._pending_update, None
                self._host.scripts.send('update', f'window.__onUpdateStatus({json.dumps(event)});')

    def hide(self):
        """前端收起只提交动作，不让 UI 关闭事件等待后台线程。"""
        self._actions.put({'action': 'hide'})

    def _hide_window(self):
        """由控制线程隐藏窗体并停止向不可见页面推送状态。"""
        with self._close_lock:
            self._requested_window = None
        if self._visible:
            self._visible = False
            self._host.hide()
            self.device_manager.stop_auto_refresh()
            self._idle_since = self._clock()

    def refresh_appearance(self):
        """偏好只更新原生背景，不覆盖网页配色和 CSS。"""
        if self._visible:
            self._host.refresh_appearance(self.config_manager.effective_theme_mode == 'dark')

    def refresh_tray(self):
        """通知独立托盘重新读取图标与语言配置。"""
        self._refresh_tray()

    def set_auto_refresh(self, enabled):
        """只控制 GUI 会话轮询，设备命令的中间状态仍会按变化推送。"""
        self.auto_refresh = enabled
        if enabled and self._visible:
            self.device_manager.start_auto_refresh(STATE_REFRESH_SECONDS)
            self.device_manager.scan_and_refresh()
        else:
            self.device_manager.stop_auto_refresh()

    def emit(self, event):
        """下载快照异步合并，渲染迟滞不能阻塞下载取消和事务资源释放。"""
        with self._state_push_lock:
            if self.closed.is_set() or self._closing:
                return
            if self._visible:
                self._host.scripts.send('update', f'window.__onUpdateStatus({json.dumps(event)});')
            else:
                self._pending_update = dict(event)

    @contextmanager
    def operation(self):
        """设备操作/网络检查/配置保存期间禁止闲置回收，保持原 API 签名。"""
        with self._close_lock:
            if self._closing:
                raise RuntimeError('GUI 正在关闭')
            self._active_operations += 1
        try:
            yield
        finally:
            with self._close_lock:
                self._active_operations -= 1
                if self._idle_since is not None:
                    self._idle_since = self._clock()

    def _release_if_idle(self):
        """只回收托盘管理的闲置 GUI，不取消下载，不强杀外设操作。"""
        if (self._visible or not self._ready.is_set() or self._idle_since is None
                or not isinstance(self._message_channel, str)):
            return
        with self._close_lock:
            if self._closing:
                return
            if (self._active_operations or self._updates.active
                    or self._host.scripts.has_pending):
                self._idle_since = self._clock()
                return
            if self._clock() - self._idle_since < self._idle_seconds:
                return
            self._closing = True
            self.idle_exit = True
        logger.info('GUI 隐藏闲置 %.0f 秒，正常释放进程：channel=%s',
                    self._idle_seconds, self._message_channel)
        threading.Thread(target=self._close, name='gui-idle-close').start()

    def _on_devices_updated(self):
        """只推送变化的快照，串行发布以免旧状态晚于新状态覆盖页面。"""
        if not self._visible or self.closed.is_set() or self._closing:
            return
        try:
            with self._state_push_lock:
                state = json.dumps(get_flyout_state(self), ensure_ascii=False, sort_keys=True)
                if state == self._last_state:
                    return
                self._host.scripts.send('state', f'window.__onBatteryStateUpdated({state});')
                self._last_state = state
        except Exception:
            # 页面退出可能与推送并发，保留完整错误日志。
            logger.exception('GUI 状态推送失败')

    def _next_message(self):
        """领取与保存意图使用故障处理的同一把锁，消除领取后故障的丢请求窗口。"""
        with self._close_lock:
            if self._closing:
                return None
            try:
                message = self._actions.get_nowait()
            except Empty:
                message = consume_message(self._message_channel)
            if message is not None:
                action = message['action']
                hide = (action in ('hide', 'shutdown') or
                        (action == 'toggle' and self._ready.is_set()
                         and self.mode == 'flyout' and self._visible))
                if hide:
                    self._requested_window = None
                elif action in ('settings', 'toggle'):
                    self._requested_window = {
                        'action': action, 'anchor': message.get('anchor', self.anchor),
                    }
            return message

    def _control_loop(self):
        """串行显隐与模式切换；失焦只在没有窗口动作时检测。"""
        while not self.closed.wait(GUI_MESSAGE_POLL_SECONDS):
            if self._closing:
                return
            try:
                message = self._next_message()
                if message is not None:
                    self._handle_message(message)
                else:
                    self._hide_if_unfocused()
                    self._release_if_idle()
            except Exception as error:
                # 原生控制失败必须结束错误窗口，不能永远重试同一故障。
                if self._closing:
                    return
                logger.exception('GUI 窗口控制失败')
                self.on_frontend_error(str(error))
                return

    def _handle_message(self, message):
        """加载期间保留最新目标模式/锚点，就绪后才进行原生显示。"""
        action = message['action']
        if 'anchor' in message:
            self.anchor = message['anchor']
        if action == 'shutdown':
            self.request_close()
            return
        if not self._ready.is_set():
            self._handle_loading_action(action)
            return
        if action == 'hide' or (action == 'toggle' and self.mode == 'flyout' and self._visible):
            self._hide_window()
        elif action in ('settings', 'toggle'):
            self.show('window' if action == 'settings' else 'flyout')

    def _handle_loading_action(self, action):
        """首屏就绪前仅记录目标模式，避免一边加载一边提前显示。"""
        if action in ('settings', 'toggle'):
            self.mode = 'window' if action == 'settings' else 'flyout'
        elif action == 'ready' and not self._closing:
            self.show(self.mode)
            self._ready.set()

    def _hide_if_unfocused(self):
        """使用 UI 线程已缓存的句柄，不从后台访问 Form.Handle。"""
        if self.mode != 'flyout' or not self._visible:
            return
        foreground = user32().GetForegroundWindow()
        if foreground and foreground != self._host.hwnd:
            self._hide_window()

    def _on_closing(self):
        """正常退出才销毁；系统关闭按钮只提交收起动作。"""
        if self.closed.is_set():
            return True
        self.hide()
        return False

    def request_close(self):
        """幂等关闭，等待下载结束的工作放到独立线程。"""
        with self._close_lock:
            if self._closing:
                return
            self._closing = True
            self._requested_window = None
        threading.Thread(target=self._close, name='gui-close').start()

    def _close(self):
        """销毁前取消下载并停止状态轮询，关闭事件不阻塞 UI。"""
        try:
            self._host.scripts.close()
            self._updates.close_and_wait()
            self.closed.set()
            self.device_manager.remove_on_update(self._on_devices_updated)
            self.device_manager.shutdown()
            self.window.destroy()
        except Exception:
            # 收尾异常须报告，不能宣称资源已正常释放。
            logger.exception('GUI 关闭失败')

    def drag_window(self):
        """独立设置窗口允许原生拖动，浮窗维持通知图标定位。"""
        if self.mode == 'window':
            drag_window(self.window)


def launch_flyout_process(mode='flyout'):
    """组合根与独立 GUI 启动入口。"""
    from flyout_runtime import run_gui
    return run_gui(mode)

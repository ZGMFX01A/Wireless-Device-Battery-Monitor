"""有界 WebView2 脚本派发；不让渲染进程拖住窗口控制和设备状态线程。"""
from concurrent.futures import Future, TimeoutError
import json
import threading
import time

SCRIPT_TIMEOUT_SECONDS = 5.0


class FlyoutScripts:
    """COM 访问只发生在 UI 线程；后台等待有期限，快照推送只保留最新值。"""

    def __init__(self, window, on_error):
        self._window = window
        self._on_error = on_error
        self._lock = threading.Lock()
        self._requests = {}
        self._latest = {}
        self._busy = set()
        self._stopped = threading.Event()
        threading.Thread(target=self._watch_timeouts, daemon=True,
                         name='gui-script-watchdog').start()

    @property
    def closed(self):
        return self._stopped.is_set()

    @property
    def has_pending(self):
        """闲置回收不能丢弃尚未返回的桥接回包。"""
        with self._lock:
            return bool(self._requests or self._latest or self._busy)

    def evaluate(self, script):
        """仅供后台启动/模式切换使用，绝不在 UI 线程等待 JS 回调。"""
        if not self._window.native.InvokeRequired:
            raise RuntimeError('禁止在 UI 线程同步等待 JavaScript')
        future = self._submit(script)
        try:
            return future.result(timeout=SCRIPT_TIMEOUT_SECONDS + 0.5)
        except TimeoutError:
            self._finish(future, error=TimeoutError('WebView2 脚本响应超时'))
            raise

    def send(self, channel, script):
        """状态/下载进度不阻塞生产者；每个通道最多一个在途和一个待发快照。"""
        with self._lock:
            if self._stopped.is_set():
                return
            self._latest[channel] = script
            if channel in self._busy:
                return
            self._busy.add(channel)
        self._send_latest(channel)

    def _send_latest(self, channel):
        with self._lock:
            script = self._latest.pop(channel, None)
            if script is None or self._stopped.is_set():
                self._busy.discard(channel)
                return
        self._submit(script).add_done_callback(lambda result: self._sent(channel, result))

    def _sent(self, channel, future):
        if self._stopped.is_set():
            return
        try:
            future.result()  # 回调只在 Future 已完成时执行，不等待消息泵。
        except Exception as error:
            self._on_error(str(error))
            return
        self._send_latest(channel)

    def _submit(self, script):
        from System import Action
        future = Future()
        with self._lock:
            if self._stopped.is_set():
                future.set_exception(RuntimeError('GUI 脚本通道已关闭'))
                return future
            self._requests[future] = time.monotonic() + SCRIPT_TIMEOUT_SECONDS
        try:
            self._window.native.BeginInvoke(Action(lambda: self._start_on_ui(future, script)))
        except Exception as error:
            self._finish(future, error=error)
        return future

    def _start_on_ui(self, future, script):
        from System import Action, String
        from System.Threading.Tasks import Task, TaskScheduler
        if future.done() or self._stopped.is_set():
            return
        try:
            view = self._window.native.webview
            if view.IsDisposed or view.CoreWebView2 is None:
                raise RuntimeError('WebView2 尚未就绪或已释放')
            # ExecuteScriptAsync 本身会把 JS 异常折叠成 null；显式封装异常，
            # 避免缺少启动函数或脚本报错被误判为正常完成。
            wrapped = ('(() => {try {return {ok:true,value:(0,eval)(' + json.dumps(script)
                       + ')}} catch(error) {return {ok:false,error:String(error.stack || error)}}})()')
            view.ExecuteScriptAsync(wrapped).ContinueWith(
                Action[Task[String]](lambda task: self._complete_on_ui(future, task)),
                TaskScheduler.FromCurrentSynchronizationContext())
        except Exception as error:
            self._finish(future, error=error)

    def _complete_on_ui(self, future, task):
        """成功、取消、故障和 JSON 解析异常都必须释放等待者。"""
        if future.done():
            return
        try:
            if task.IsCanceled:
                raise RuntimeError('WebView2 脚本执行已取消')
            if task.IsFaulted:
                raise RuntimeError(f'WebView2 脚本执行失败：{task.Exception}')
            # ContinueWith 保证任务已经结束，此处读取 Result 不阻塞 UI。
            response = json.loads(str(task.Result))
            if not isinstance(response, dict) or response.get('ok') is not True:
                detail = response.get('error', response) if isinstance(response, dict) else response
                raise RuntimeError(f'GUI JavaScript 执行失败：{detail}')
            result = response.get('value')
        except Exception as error:
            self._finish(future, error=error)
        else:
            self._finish(future, result=result)

    def _finish(self, future, result=None, error=None):
        with self._lock:
            if self._requests.pop(future, None) is None:
                return
        # Future 回调可能继续派发或触发关闭，不能在持锁时执行。
        if error is not None:
            future.set_exception(error)
        else:
            future.set_result(result)

    def _watch_timeouts(self):
        while not self._stopped.wait(0.25):
            with self._lock:
                expired = [future for future, deadline in self._requests.items()
                           if time.monotonic() >= deadline]
            for future in expired:
                self._finish(future, error=TimeoutError(
                    f'WebView2 脚本超过 {SCRIPT_TIMEOUT_SECONDS:g} 秒未响应'))

    def close(self):
        """释放后台等待和未发送快照；不等待已经失去响应的渲染进程。"""
        self._stopped.set()
        with self._lock:
            self._latest.clear()
            self._busy.clear()
            pending = list(self._requests)
        for future in pending:
            self._finish(future, error=RuntimeError('GUI 脚本通道已关闭'))

"""WebView 更新交互，保持单文件 EXE 的宿主退出与安全替换契约。"""
import threading
import time

from config import APP_VERSION


class FlyoutUpdate:
    """注入原下载器与事件通道；GUI 关闭前等待下载器释放事务资源。"""

    def __init__(self, options):
        self._backend = options['updater']
        self._emit = options['emit']
        self._host_pid = options['host_pid']
        self._language = options['language']
        self._lock = threading.Lock()
        self._cancel = threading.Event()
        self._thread = None
        self._checked = None
        self._closing = False
        self._active = False
        self._last_progress = None

    @property
    def active(self):
        """只有下载器真正返回并交接/清理完成后，GUI 才能被闲置回收。"""
        with self._lock:
            return self._active

    def check(self):
        """网络错误抛给 JS，只有取得实际版本元数据才报告检查成功。"""
        found, version, url, notes, size, digest = self._backend.check_for_update(
            APP_VERSION, self._language())
        if not version:
            raise RuntimeError(notes or '检查更新失败：未取得版本信息')
        info = dict(has_update=found, version=version, url=url, notes=notes,
                    size=size, digest=digest)
        with self._lock:
            self._checked = dict(info) if found else None
        return info

    def start(self, options):
        """只允许下载本次官方检查所得资源，不能用 JS 参数替换安装来源。"""
        with self._lock:
            if self._active or self._closing:
                raise RuntimeError('更新任务进行中或窗口正在关闭')
            if self._checked is None or any(
                options.get(key) != self._checked[key] for key in ('url', 'size', 'digest', 'version')
            ):
                raise ValueError('更新资源与已检查的发布信息不一致')
            self._cancel.clear()
            self._active = True
            self._last_progress = None
            info = dict(self._checked)
            self._thread = threading.Thread(target=self._download, args=(info,),
                                            name='gui-update-download')
            self._thread.start()
        return {'success': True}

    def _download(self, info):
        """使用托盘 PID；返回前由原下载器完成取消清理或替换事务交接。"""
        try:
            self._backend.download_and_install(
                download_url=info['url'], on_progress=self._progress,
                host_pid=self._host_pid, expected_size=info['size'],
                expected_digest=info['digest'], on_status=self._status,
                target_version=info['version'], cancel_event=self._cancel)
        except Exception as error:
            # 非预期下载异常同样进入明确的失败状态，不能显示更新成功。
            self._emit({'stage': 'error', 'detail': str(error)})
        finally:
            with self._lock:
                self._active = False

    def _progress(self, percent, downloaded, total):
        """适配三参数回调，按增量字节和单调时间计算当前下载速度。"""
        now = time.monotonic()
        speed = 0
        if self._last_progress:
            previous_time, previous_bytes = self._last_progress
            elapsed = now - previous_time
            if elapsed > 0 and downloaded >= previous_bytes:
                speed = (downloaded - previous_bytes) / elapsed
        self._last_progress = (now, downloaded)
        self._emit(dict(stage='downloading', percent=percent, downloaded=downloaded,
                        total=total, speed=speed, detail=''))

    def _status(self, stage, detail):
        """保留下载器实际状态，取消完成通知只能由下载器发出。"""
        self._emit({'stage': stage, 'detail': detail})

    def cancel(self):
        """请求取消下载，不提前宣布文件与更新事务已完成清理。"""
        self._cancel.set()
        return {'success': True}

    def close_and_wait(self):
        """关闭窗口时停止新下载，等待在途下载安全退出或完成交接。"""
        with self._lock:
            self._closing = True
            self._cancel.set()
            worker = self._thread
        if worker and worker is not threading.current_thread():
            worker.join()

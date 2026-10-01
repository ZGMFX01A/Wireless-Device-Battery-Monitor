"""有截止时间、可取消的更新网络访问，不修改全局 socket 行为。"""

import http.client
import logging
import socket
import threading
import time
import urllib.request

logger = logging.getLogger(__name__)
_network_slots = threading.BoundedSemaphore(4)


class UpdateCancelled(Exception):
    pass


def check_budget(deadline, cancel_event=None):
    if cancel_event is not None and cancel_event.is_set():
        raise UpdateCancelled('更新已取消')
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise TimeoutError('更新网络任务超过截止时间')
    return remaining


def close_response(response):
    """读操作可能仍持有缓冲锁，关闭也不能阻塞调用方的取消。"""
    if response is None:
        return

    def close():
        try:
            response.close()
        except Exception as error:
            logger.debug('关闭更新响应失败: %s', error)

    threading.Thread(target=close, daemon=True, name='update-response-close').start()


def bounded_call(call, deadline, cancel_event=None, *, close_abandoned=False):
    """调用方拥有文件写入权；迟到的网络调用只能返回数据或关闭响应。"""
    check_budget(deadline, cancel_event)
    if not _network_slots.acquire(blocking=False):
        raise RuntimeError('先前网络请求尚未释放，请稍后重试')
    guard = threading.Lock()
    done = threading.Event()
    state = {'abandoned': False, 'value': None, 'error': None}

    def worker():
        value, error = None, None
        try:
            value = call()
        except Exception as caught:
            error = caught
        finally:
            with guard:
                abandoned = state['abandoned']
                if not abandoned:
                    state.update(value=value, error=error)
                    done.set()
            if abandoned and close_abandoned and value is not None:
                close_response(value)
            _network_slots.release()

    try:
        threading.Thread(target=worker, daemon=True, name='update-network-call').start()
    except Exception:
        _network_slots.release()
        raise
    try:
        while not done.wait(min(0.1, check_budget(deadline, cancel_event))):
            pass
        check_budget(deadline, cancel_event)
        if state['error'] is not None:
            raise state['error']
        return state['value']
    except BaseException:
        with guard:
            state['abandoned'] = True
            value = state['value']
        if close_abandoned and value is not None:
            close_response(value)
        raise


def _ipv4_connection(address, timeout=socket._GLOBAL_DEFAULT_TIMEOUT, source_address=None):
    host, port = address
    addresses = socket.getaddrinfo(host, port, 0, socket.SOCK_STREAM)
    addresses.sort(key=lambda info: info[0] != socket.AF_INET)
    last_error = None
    for family, socktype, proto, _, sockaddr in addresses:
        connection = socket.socket(family, socktype, proto)
        try:
            if timeout is not socket._GLOBAL_DEFAULT_TIMEOUT:
                connection.settimeout(timeout)
            if source_address:
                connection.bind(source_address)
            connection.connect(sockaddr)
            return connection
        except OSError as error:
            last_error = error
            connection.close()
    raise last_error or OSError('更新服务器未返回可用地址')


class _HTTPConnection(http.client.HTTPConnection):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self._create_connection = _ipv4_connection


class _HTTPSConnection(http.client.HTTPSConnection):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self._create_connection = _ipv4_connection


class _HTTPHandler(urllib.request.HTTPHandler):
    def http_open(self, request):
        return self.do_open(_HTTPConnection, request)


class _HTTPSHandler(urllib.request.HTTPSHandler):
    def https_open(self, request):
        return self.do_open(_HTTPSConnection, request, context=self._context)


def _open_request(request, timeout):
    opener = urllib.request.build_opener(_HTTPHandler(), _HTTPSHandler())
    return opener.open(request, timeout=timeout)


def urlopen(url, timeout, retries=1, on_retry=None, *, deadline=None, cancel_event=None):
    deadline = deadline if deadline is not None else time.monotonic() + timeout * (retries + 1) + retries
    request = urllib.request.Request(url, headers={'User-Agent': 'MouseBattery-Updater'})
    for attempt in range(retries + 1):
        try:
            call_timeout = min(timeout, check_budget(deadline, cancel_event))
            return bounded_call(
                lambda: _open_request(request, call_timeout),
                deadline, cancel_event, close_abandoned=True,
            )
        except UpdateCancelled:
            raise
        except Exception as error:
            check_budget(deadline, cancel_event)
            if attempt >= retries:
                raise
            logger.warning('更新请求失败，准备重试: %s', error)
            if on_retry:
                on_retry(attempt + 1, retries, error)
            wait_end = min(deadline, time.monotonic() + 1)
            while time.monotonic() < wait_end:
                check_budget(deadline, cancel_event)
                time.sleep(max(0, min(0.1, wait_end - time.monotonic())))
    raise RuntimeError('更新请求重试耗尽')


def read_chunk(response, size, deadline, cancel_event=None, *, idle_timeout=5):
    read = getattr(response, 'read1', response.read)
    read_deadline = min(deadline, time.monotonic() + idle_timeout)
    return bounded_call(lambda: read(size), read_deadline, cancel_event)

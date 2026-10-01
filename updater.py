"""有界下载与 Windows 单文件 EXE 更新。"""
import hashlib
import json
import logging
import os
from pathlib import Path
import re
import subprocess
import sys
import threading
import time
import uuid
from urllib.parse import unquote, urlparse
from i18n import LANGUAGE_EN_US, LANGUAGE_ZH_CN
from update_network import (urlopen as _urlopen, read_chunk, close_response,
                            check_budget, UpdateCancelled)
from update_transaction import (UpdateLease, cleanup_job, job_directory, write_atomic,
    process_start_ticks, read_update_result, request_process_shutdown, consume_shutdown_request,
    prepare_update_startup, confirm_update_startup, _build_swap_powershell_script,
    _encode_powershell_command, _powershell_literal)
logger = logging.getLogger(__name__)
API_URL = 'https://api.github.com/repos/ZGMFX01A/mouse-battery/releases/latest'
DOWNLOAD_MIRROR_PREFIX = 'https://ghfast.top/'
MIN_VALID_EXE_BYTES = 1024 * 1024
MAX_UPDATE_BYTES = 512 * 1024 * 1024
CHECK_TIMEOUT = 12
DOWNLOAD_SOURCE_TIMEOUT = 90
DOWNLOAD_TOTAL_TIMEOUT = 240
DOWNLOAD_IDLE_TIMEOUT = 5
DOWNLOAD_RATE_WINDOW = 15
DOWNLOAD_MIN_RATE = 1024
_check_lock = threading.Lock()

_RELEASE_LANGUAGE_MARKER = re.compile(
    r"<!--\s*lang:(?P<language>zh|en)\s*-->\s*",
    re.IGNORECASE,
)
_RELEASE_ANY_LANGUAGE_MARKER = re.compile(r"<!--\s*lang:[^>]+-->\s*", re.IGNORECASE)
_RELEASE_SECTION_SEPARATOR = re.compile(r"\r?\n[ \t]*---[ \t]*(?:\r?\n)?$")
_RELEASE_LANGUAGE_CODES = {
    LANGUAGE_ZH_CN.lower(): "zh",
    LANGUAGE_EN_US.lower(): "en",
}

def parse_version(version_str):
    match = re.fullmatch(r'v?(\d+)\.(\d+)\.(\d+)', str(version_str or '').strip().lower())
    return tuple(map(int, match.groups())) if match else (0, 0, 0)


def _normalize_version_text(version):
    value = str(version or '').strip().lower()
    if not re.fullmatch(r'v?\d+\.\d+\.\d+', value):
        raise ValueError('Release 版本标签非法')
    return value.removeprefix('v')


def _pick_release_asset(assets, latest_version):
    version = _normalize_version_text(latest_version)
    pattern = re.compile(r'(?:WirelessDeviceBatteryMonitor|MouseBattery)-v?' + re.escape(version) + r'\.exe', re.I)
    matching = [a for a in assets if isinstance(a, dict) and pattern.fullmatch(a.get('name', ''))]
    return max(matching, key=lambda a: a.get('updated_at', ''), default={})


def _release_language_code(language: str) -> str:
    """把界面语言转换为 Release 正文使用的语言标记。"""
    normalized = str(language or "").strip().replace("_", "-").lower()
    code = _RELEASE_LANGUAGE_CODES.get(normalized)
    if code is None:
        raise ValueError(f"不支持的更新说明语言: {language!r}")
    return code


def _select_release_notes(notes: str, language: str) -> str:
    """按语言标记提取单一 Release 分段，禁止跨语言混显。"""
    raw_notes = str(notes or "").strip()
    markers = list(_RELEASE_LANGUAGE_MARKER.finditer(raw_notes))
    if not markers:
        if _RELEASE_ANY_LANGUAGE_MARKER.search(raw_notes):
            raise ValueError("更新日志包含不支持的语言标记")
        return raw_notes

    requested_code = _release_language_code(language)
    for index, marker in enumerate(markers):
        if marker.group("language").lower() != requested_code:
            continue
        end = markers[index + 1].start() if index + 1 < len(markers) else len(raw_notes)
        section = raw_notes[marker.end():end].rstrip()
        section = _RELEASE_SECTION_SEPARATOR.sub("", section).strip()
        if not section:
            raise ValueError(f"更新日志的 {requested_code} 分段为空")
        return section

    raise ValueError(f"更新日志缺少 {requested_code} 分段")


def check_for_update(current_version, language=LANGUAGE_ZH_CN):
    if not _check_lock.acquire(blocking=False):
        return False, '', '', '已有更新检查正在进行', 0, ''
    response = None
    try:
        deadline = time.monotonic() + CHECK_TIMEOUT
        response = _urlopen(API_URL, timeout=5, retries=1, deadline=deadline)
        chunks, length = [], 0
        while True:
            chunk = read_chunk(response, 65536, deadline)
            if not chunk:
                break
            length += len(chunk)
            if length > 1024 * 1024:
                raise ValueError('Release 元数据过大')
            chunks.append(chunk)
        data = json.loads(b''.join(chunks).decode('utf-8'))
        latest = data.get('tag_name', '')
        _normalize_version_text(latest)
        _normalize_version_text(current_version)
        if parse_version(latest) <= parse_version(current_version):
            return False, latest, '', '', 0, ''
        selected = _pick_release_asset(data.get('assets', []), latest)
        url = selected.get('browser_download_url', '')
        size = int(selected.get('size', 0) or 0)
        digest = selected.get('digest', '')
        if not selected or not url:
            raise RuntimeError('Release 中未发现版本匹配的单文件 EXE')
        if urlparse(url).scheme != 'https' or unquote(urlparse(url).path.rsplit('/', 1)[-1]) != selected['name']:
            raise ValueError('Release 下载地址与资源文件名不一致')
        if not 0 < size <= MAX_UPDATE_BYTES or not _normalize_sha256(digest):
            raise ValueError('Release 缺少有效文件大小或 SHA-256')
        body = _select_release_notes(data.get('body', ''), language) or '（此次发布未提供更新日志说明）'
        return True, latest, url, body, size, digest
    except Exception as error:
        logger.error('检查更新失败: %s', error)
        return False, '', '', str(error), 0, ''
    finally:
        close_response(response)
        _check_lock.release()


def _normalize_sha256(digest):
    match = re.fullmatch(r'sha256:([0-9a-fA-F]{64})', str(digest or '').strip())
    return match.group(1).lower() if match else ''


def _notify_status(callback, stage, detail=''):
    if callback:
        try:
            callback(stage, detail)
        except Exception:
            logger.exception('更新状态回调失败')


def _download_to_path(url, target_path, on_progress=None, expected_size=0,
                      retries=0, on_retry=None, *, deadline=None, cancel_event=None):
    deadline = deadline if deadline is not None else time.monotonic() + DOWNLOAD_TOTAL_TIMEOUT
    for attempt in range(retries + 1):
        source_deadline = min(deadline, time.monotonic() + DOWNLOAD_SOURCE_TIMEOUT)
        response = None
        try:
            response = _urlopen(url, timeout=5, retries=0, deadline=source_deadline, cancel_event=cancel_event)
            header = str(response.info().get('Content-Length', '0')).strip()
            total = expected_size or (int(header) if header.isdigit() else 0)
            downloaded, hasher = 0, hashlib.sha256()
            window_start, window_bytes = time.monotonic(), 0
            with open(target_path, 'wb') as output:
                while True:
                    chunk = read_chunk(response, 256 * 1024, source_deadline, cancel_event,
                                       idle_timeout=DOWNLOAD_IDLE_TIMEOUT)
                    check_budget(source_deadline, cancel_event)
                    if not chunk:
                        break
                    downloaded += len(chunk)
                    if downloaded > MAX_UPDATE_BYTES or (expected_size and downloaded > expected_size):
                        raise RuntimeError('下载字节数超过预期')
                    output.write(chunk)
                    hasher.update(chunk)
                    elapsed = time.monotonic() - window_start
                    if elapsed >= DOWNLOAD_RATE_WINDOW:
                        if (downloaded - window_bytes) / elapsed < DOWNLOAD_MIN_RATE:
                            raise TimeoutError('下载持续低速，切换来源')
                        window_start, window_bytes = time.monotonic(), downloaded
                    if on_progress:
                        try:
                            on_progress(min(int(downloaded / total * 100), 100) if total else -1, downloaded, total)
                        except Exception:
                            logger.exception('更新进度回调失败')
                    if expected_size and downloaded == expected_size:
                        break
                output.flush()
                os.fsync(output.fileno())
            return downloaded, hasher.hexdigest()
        except UpdateCancelled:
            raise
        except Exception as error:
            check_budget(deadline, cancel_event)
            if attempt >= retries:
                raise
            if on_retry:
                on_retry(attempt + 1, retries, error)
        finally:
            close_response(response)
    raise RuntimeError('下载重试耗尽')


def _validate_download(path, downloaded, actual_sha256, expected_size, expected_sha256):
    if downloaded < MIN_VALID_EXE_BYTES:
        raise RuntimeError('下载到的更新文件过小，疑似截断下载')
    with open(path, 'rb') as source:
        size = os.fstat(source.fileno()).st_size
        disk_hash = hashlib.file_digest(source, 'sha256').hexdigest()
    if size != downloaded or (expected_size and size != expected_size):
        raise RuntimeError('更新文件大小校验失败')
    if disk_hash != expected_sha256 or actual_sha256 != expected_sha256:
        raise RuntimeError('更新文件 SHA-256 校验失败')
    return size


def _wait_helper_ready(child, directory, token, cancel_event):
    deadline = time.monotonic() + 8
    while True:
        check_budget(deadline, cancel_event)
        if child.poll() is not None:
            raise RuntimeError('替换程序启动失败，应用保持运行')
        ready = directory / 'helper.ready'
        if ready.exists() and ready.read_text(encoding='utf-8') == token:
            return
        time.sleep(0.05)


def _stop_helper(child):
    if child is not None and child.poll() is None:
        child.terminate()
        try:
            child.wait(timeout=2)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=2)


def download_and_install(download_url, on_progress=None, host_pid=None, expected_size=0,
                         expected_digest='', on_status=None, *, target_version=None, cancel_event=None):
    if not getattr(sys, 'frozen', False):
        _notify_status(on_status, 'error', 'debug_mode')
        return False
    exe_path = os.path.abspath(sys.executable)
    token, child, transferred = uuid.uuid4().hex, None, False
    directory = None
    try:
        sha256 = _normalize_sha256(expected_digest)
        if not sha256 or not 0 < expected_size <= MAX_UPDATE_BYTES:
            raise ValueError('Release 缺少有效文件大小或 SHA-256')
        asset = unquote(urlparse(download_url).path.rsplit('/', 1)[-1])
        match = re.fullmatch(r'(?:WirelessDeviceBatteryMonitor|MouseBattery)-v?(\d+\.\d+\.\d+)\.exe', asset, re.I)
        if urlparse(download_url).scheme != 'https' or not match:
            raise ValueError('Release 资源文件名非法')
        version = _normalize_version_text(target_version or match.group(1))
        if match.group(1) != version:
            raise ValueError('Release 资源版本与目标版本不一致')
        target = str(Path(exe_path).parent / asset)
        pid = host_pid if isinstance(host_pid, int) and host_pid > 0 else os.getpid()
        with UpdateLease(exe_path) as lease:
            if os.path.normcase(target) != os.path.normcase(exe_path) and os.path.exists(target):
                raise RuntimeError('目标版本文件已存在')
            directory = job_directory(exe_path, token)
            directory.mkdir()
            staged, backup = directory / 'asset.partial', directory / 'previous.exe'
            write_atomic(directory / 'manifest.json', dict(token=token, target_path=target, version=version))
            deadline = time.monotonic() + DOWNLOAD_TOTAL_TIMEOUT
            sources = [('official', download_url)]
            if download_url.startswith('https://github.com/'):
                sources.append(('mirror', DOWNLOAD_MIRROR_PREFIX + download_url))
            for index, (name, url) in enumerate(sources):
                check_budget(deadline, cancel_event)
                _notify_status(on_status, 'connecting', name)
                try:
                    downloaded, actual_hash = _download_to_path(url, str(staged), on_progress=on_progress,
                        expected_size=expected_size, retries=1 if name == 'mirror' else 0,
                        on_retry=lambda a, n, e: _notify_status(on_status, 'retrying', str(e)),
                        deadline=deadline, cancel_event=cancel_event)
                    _notify_status(on_status, 'verifying')
                    size = _validate_download(staged, downloaded, actual_hash, expected_size, sha256)
                    break
                except UpdateCancelled:
                    raise
                except Exception as error:
                    logger.warning('下载来源失败 %s: %s', name, error)
                    if index + 1 == len(sources):
                        raise
                    _notify_status(on_status, 'fallback', str(error))
            check_budget(deadline, cancel_event)
            script = _build_swap_powershell_script(exe_path, target, str(backup), str(staged), pid, size,
                expected_sha256=sha256, token=token, semaphore_name=lease.name,
                target_version=version, target_start_ticks=process_start_ticks(pid))
            child = subprocess.Popen(['powershell.exe', '-NoProfile', '-NonInteractive', '-ExecutionPolicy',
                'Bypass', '-EncodedCommand', _encode_powershell_command(script)],
                creationflags=subprocess.CREATE_NO_WINDOW)
            _wait_helper_ready(child, directory, token, cancel_event)
            check_budget(deadline, cancel_event)
            _notify_status(on_status, 'applying')
            check_budget(deadline, cancel_event)
            write_atomic(directory / 'apply.request', dict(token=token, target_pid=pid))
            lease.handoff()
            transferred = True
            if not request_process_shutdown(pid, reason='update',
                    skip_gui_pid=os.getppid() if pid != os.getpid() else None, update_token=token):
                write_atomic(directory / 'abort.request', dict(token=token))
                raise RuntimeError('无法通知应用退出，更新已取消')
            return True
    except UpdateCancelled as error:
        _notify_status(on_status, 'cancelled', str(error))
        return False
    except Exception as error:
        logger.exception('应用更新失败')
        _notify_status(on_status, 'error', str(error))
        return False
    finally:
        if not transferred:
            try:
                _stop_helper(child)
                if directory is not None:
                    cleanup_job(exe_path, token)
            except Exception:
                logger.exception('更新事务清理失败')

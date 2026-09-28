"""托盘与设置窗口之间的更新退出确认协议。"""

from __future__ import annotations

import json
import logging
import os
import tempfile
import threading
import time
from typing import Optional

logger = logging.getLogger(__name__)

SHUTDOWN_REQUEST_PREFIX = "mouse_battery_shutdown"
SHUTDOWN_ACK_PREFIX = "mouse_battery_shutdown_ack"
SHUTDOWN_ACK_TIMEOUT_SECONDS = 10.0
SHUTDOWN_ACK_POLL_INTERVAL_SECONDS = 0.1
_file_lock = threading.Lock()


def _get_shutdown_request_path(target_pid: int) -> str:
    """返回目标主进程的更新退出请求路径。"""
    return os.path.join(tempfile.gettempdir(), f"{SHUTDOWN_REQUEST_PREFIX}_{target_pid}.json")


def _get_shutdown_ack_path(requester_pid: int, target_pid: int) -> str:
    """返回一次跨进程更新退出请求的确认路径。"""
    return os.path.join(
        tempfile.gettempdir(),
        f"{SHUTDOWN_ACK_PREFIX}_{requester_pid}_{target_pid}.json",
    )


def _remove_file(path: str) -> None:
    """清理原子写入失败留下的临时请求文件。"""
    try:
        if path and os.path.exists(path):
            os.remove(path)
    except OSError as error:
        logger.warning("删除退出请求临时文件失败: path=%s, error=%s", path, error)


def request_process_shutdown(
    target_pid: int,
    *,
    reason: str = "update",
    skip_gui_pid: Optional[int] = None,
) -> bool:
    """通知托盘主进程停止设备访问，完成收尾后交给 Velopack 应用更新。"""
    if not isinstance(target_pid, int) or target_pid <= 0:
        logger.error("写入退出请求失败，目标 PID 非法: %r", target_pid)
        return False

    request_path = _get_shutdown_request_path(target_pid)
    ack_path = _get_shutdown_ack_path(os.getpid(), target_pid)
    temp_path = request_path + ".tmp"
    payload = {
        "reason": reason,
        "target_pid": target_pid,
        "requester_pid": os.getpid(),
        "requested_at": time.time(),
    }
    if isinstance(skip_gui_pid, int) and skip_gui_pid > 0:
        payload["skip_gui_pid"] = skip_gui_pid

    try:
        with _file_lock:
            _remove_file(ack_path)
            with open(temp_path, "w", encoding="utf-8") as request_file:
                json.dump(payload, request_file, ensure_ascii=False)
            os.replace(temp_path, request_path)
        logger.info("已写入更新退出请求: target_pid=%s, skip_gui_pid=%s", target_pid, skip_gui_pid)
        return True
    except Exception as error:
        logger.error("写入退出请求失败: %s", error)
        _remove_file(temp_path)
        return False


def consume_shutdown_request(current_pid: int) -> Optional[dict]:
    """读取并消费当前进程的更新退出请求。"""
    if not isinstance(current_pid, int) or current_pid <= 0:
        logger.warning("读取退出请求时收到非法 PID: %r", current_pid)
        return None

    request_path = _get_shutdown_request_path(current_pid)
    if not os.path.exists(request_path):
        return None

    try:
        with open(request_path, "r", encoding="utf-8") as request_file:
            payload = json.load(request_file)
    except Exception as error:
        logger.error("读取退出请求失败: path=%s, error=%s", request_path, error)
        _remove_file(request_path)
        return None

    _remove_file(request_path)
    if not isinstance(payload, dict):
        logger.error("退出请求格式非法，已忽略: %r", payload)
        return None
    return payload


def acknowledge_shutdown(request: Optional[dict], status: str, detail: str = "") -> bool:
    """确认主进程已收到更新请求，或回传应用失败原因。"""
    if not isinstance(request, dict):
        logger.error("无法确认非法的更新退出请求: %r", request)
        return False
    requester_pid = request.get("requester_pid")
    target_pid = request.get("target_pid")
    if not all(isinstance(pid, int) and pid > 0 for pid in (requester_pid, target_pid)):
        logger.error("更新退出请求缺少有效 PID，无法确认: %r", request)
        return False

    ack_path = _get_shutdown_ack_path(requester_pid, target_pid)
    temp_path = ack_path + ".tmp"
    payload = {"status": status, "detail": detail, "acknowledged_at": time.time()}
    try:
        with _file_lock:
            with open(temp_path, "w", encoding="utf-8") as ack_file:
                json.dump(payload, ack_file, ensure_ascii=False)
            os.replace(temp_path, ack_path)
        return True
    except Exception as error:
        logger.error("写入更新退出确认失败: path=%s, error=%s", ack_path, error)
        _remove_file(temp_path)
        return False


def wait_for_shutdown_ack(
    requester_pid: int,
    target_pid: int,
    *,
    timeout_seconds: float = SHUTDOWN_ACK_TIMEOUT_SECONDS,
) -> Optional[dict]:
    """等待目标主进程确认更新交接，超时返回空值并由调用方报错。"""
    ack_path = _get_shutdown_ack_path(requester_pid, target_pid)
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        if not os.path.exists(ack_path):
            time.sleep(SHUTDOWN_ACK_POLL_INTERVAL_SECONDS)
            continue
        try:
            with open(ack_path, "r", encoding="utf-8") as ack_file:
                payload = json.load(ack_file)
        except Exception as error:
            _remove_file(ack_path)
            raise RuntimeError(f"读取更新退出确认失败: {error}") from error
        _remove_file(ack_path)
        if not isinstance(payload, dict):
            raise RuntimeError(f"更新退出确认格式非法: {payload!r}")
        return payload
    return None

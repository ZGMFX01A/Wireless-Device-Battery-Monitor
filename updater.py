"""基于 Velopack 的 Windows 自动更新适配层。

Velopack 负责更新包来源、校验、断点临时文件、安装目录替换与重启。
本模块只保留应用侧的版本结果、进度回调以及托盘/GUI 进程协调。
"""

from __future__ import annotations

import logging
import os
import re
import sys
import threading
from dataclasses import dataclass
from typing import Any, Callable, Optional

from velopack import App, GithubSource, UpdateManager, UpdateOptions
from update_ipc import (
    acknowledge_shutdown,
    consume_shutdown_request,
    request_process_shutdown,
    wait_for_shutdown_ack,
)
from i18n import LANGUAGE_EN_US, LANGUAGE_ZH_CN

logger = logging.getLogger(__name__)

REPO_URL = "https://github.com/ZGMFX01A/mouse-battery"
MAXIMUM_DELTAS_BEFORE_FALLBACK = 10
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

StatusCallback = Callable[[str, str], None]
ProgressCallback = Callable[[int], None]
_check_lock = threading.Lock()


@dataclass(frozen=True)
class UpdateCandidate:
    """保存一次检查得到的更新及其对应的管理器。"""

    manager: Any
    update_info: Any
    version: str
    release_notes: str


@dataclass(frozen=True)
class UpdateCheckResult:
    """向托盘和 GUI 暴露稳定的更新检查结果。"""

    has_update: bool
    latest_version: str = ""
    release_notes: str = ""
    candidate: Optional[UpdateCandidate] = None
    error: str = ""


def initialize_velopack() -> None:
    """执行 Velopack 启动钩子；入口进程必须且只能调用一次。"""
    App().set_auto_apply_on_startup(False).run()


def _create_update_manager() -> Any:
    """创建使用 GitHub Releases 的更新管理器。"""
    source = GithubSource(REPO_URL, None, False)
    options = UpdateOptions(False, MAXIMUM_DELTAS_BEFORE_FALLBACK)
    return UpdateManager(source, options)


def _format_error(error: Exception) -> str:
    """保留异常类型和原始信息，便于区分未安装、校验和网络故障。"""
    return f"{type(error).__name__}: {error}"


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


def _get_release_notes(update_info: Any, language: str = LANGUAGE_ZH_CN) -> str:
    """读取 Velopack 包内 Markdown，并只保留当前界面的语言分段。"""
    asset = getattr(update_info, "TargetFullRelease", None)
    notes = getattr(asset, "NotesMarkdown", "") or getattr(asset, "NotesHtml", "")
    selected = _select_release_notes(notes, language)
    return selected or "（此次发布未提供更新日志说明）"


def check_for_update(current_version: str, language: str = LANGUAGE_ZH_CN) -> UpdateCheckResult:
    """检查 GitHub Releases，并返回当前语言的更新说明与下载对象。"""
    if not _check_lock.acquire(blocking=False):
        return UpdateCheckResult(
            False,
            latest_version=current_version,
            error="已有更新检查正在进行，请等待当前检查结束",
        )
    try:
        manager = _create_update_manager()
        update_info = manager.check_for_updates()
        if update_info is None:
            return UpdateCheckResult(False, latest_version=current_version)

        asset = getattr(update_info, "TargetFullRelease", None)
        latest_version = str(getattr(asset, "Version", "")).strip()
        if not latest_version:
            raise RuntimeError("更新源未返回目标版本号")

        candidate = UpdateCandidate(
            manager=manager,
            update_info=update_info,
            version=latest_version,
            release_notes=_get_release_notes(update_info, language),
        )
        logger.info("发现 Velopack 更新: current=%s, latest=%s", current_version, latest_version)
        return UpdateCheckResult(
            True,
            latest_version=latest_version,
            release_notes=candidate.release_notes,
            candidate=candidate,
        )
    except Exception as error:
        message = _format_error(error)
        logger.error("检查更新失败: %s", message)
        return UpdateCheckResult(False, latest_version=current_version, error=message)
    finally:
        _check_lock.release()


def _notify_status(callback: Optional[StatusCallback], stage: str, detail: str = "") -> None:
    """把更新阶段传给 UI；UI 回调异常必须留下日志。"""
    if callback is None:
        return
    try:
        callback(stage, detail)
    except Exception as error:
        logger.warning("更新状态回调失败: stage=%s, error=%s", stage, error)


def _notify_progress(callback: Optional[ProgressCallback], percent: int) -> None:
    """转发 Velopack 的百分比进度。"""
    if callback is None:
        return
    callback(max(0, min(int(percent), 100)))


def _request_shutdown_and_confirm(target_pid: int, skip_gui_pid: Optional[int] = None) -> None:
    """发出退出请求并等待主进程确认，避免 UI 把写文件误判为安装成功。"""
    requester_pid = os.getpid()
    if not request_process_shutdown(
        target_pid=target_pid,
        reason="update",
        skip_gui_pid=skip_gui_pid,
    ):
        raise RuntimeError(f"无法通知更新宿主进程退出: pid={target_pid}")
    ack = wait_for_shutdown_ack(requester_pid, target_pid)
    if ack is None:
        raise TimeoutError(f"等待更新宿主进程确认超时: pid={target_pid}")
    if ack.get("status") != "accepted":
        raise RuntimeError(f"更新宿主进程拒绝退出: {ack.get('detail') or ack}")


def _apply_pending_update(manager: Any) -> None:
    """在拥有托盘主进程的进程内应用已下载包并重启主入口。"""
    pending_update = manager.get_update_pending_restart()
    if pending_update is None:
        raise RuntimeError("Velopack 未找到已下载的待应用更新包")
    manager.apply_updates_and_restart_with_args(pending_update, [])


def _request_host_shutdown(host_pid: int) -> None:
    """让 GUI 下载完成后通知托盘主进程接管应用替换。"""
    if not isinstance(host_pid, int) or host_pid <= 0 or host_pid == os.getpid():
        raise ValueError(f"GUI 更新宿主 PID 非法: {host_pid!r}")
    _request_shutdown_and_confirm(host_pid, skip_gui_pid=os.getpid())


def _report_update_error(error: Exception, callback: Optional[StatusCallback]) -> bool:
    """统一记录安装失败并通知界面。"""
    message = _format_error(error)
    logger.error("应用更新失败: %s", message)
    _notify_status(callback, "error", message)
    return False


def download_and_install(
    candidate: Optional[UpdateCandidate],
    *,
    on_progress: Optional[ProgressCallback] = None,
    host_pid: Optional[int] = None,
    on_status: Optional[StatusCallback] = None,
) -> bool:
    """下载并应用更新；GUI 进程只下载，托盘主进程负责最终替换和重启。"""
    if not isinstance(candidate, UpdateCandidate):
        return _report_update_error(ValueError("更新候选为空或类型无效"), on_status)
    if not getattr(sys, "frozen", False):
        return _report_update_error(
            RuntimeError("源码模式不支持安装更新，请使用 Velopack Setup.exe 或 Portable 包运行"),
            on_status,
        )

    try:
        _notify_status(on_status, "downloading")
        candidate.manager.download_updates(
            candidate.update_info,
            lambda percent: _notify_progress(on_progress, percent),
        )
        _notify_status(on_status, "verifying")
        if host_pid is None:
            _request_shutdown_and_confirm(os.getpid())
            _notify_status(on_status, "applying")
            return True

        _request_host_shutdown(host_pid)
        _notify_status(on_status, "applying")
        return True
    except Exception as error:
        return _report_update_error(error, on_status)


def apply_pending_update() -> None:
    """由托盘主进程在优雅退出后应用 GUI 已下载的更新。"""
    manager = _create_update_manager()
    _apply_pending_update(manager)

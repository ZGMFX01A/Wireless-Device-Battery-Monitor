"""RTSS 进程级兼容声明与只读检测；不修改系统环境或 RTSS 配置。"""
import os
import json


_COMPATIBILITY_ENV = 'RTSSHooksCompatibility'
_ORIGINAL_ENV = 'MOUSE_BATTERY_RTSS_ORIGINAL_ENV'


def _replace_compatibility_value(environment, value):
    # Windows 环境名不区分大小写，复制成普通 dict 后需移除同名别名。
    for name in list(environment):
        if name.casefold() == _COMPATIBILITY_ENV.casefold():
            del environment[name]
    if value is not None:
        environment[_COMPATIBILITY_ENV] = value


def disable_rtss_hooks(environment):
    """仅修改调用方指定的进程环境，让 RTSS 跳过本程序的图形 API Hook。

    0 表示不兼容；不能照搬网上将环境变量设为 1 的说法。
    已在 RTSS 7.3.5.28314 及实际 WebView2 子进程中验证，见开发文档。
    此声明不保证 DLL 不会被系统加载，且 RTSS 的 ForceHooking 可覆盖它。
    """
    if _ORIGINAL_ENV not in environment:
        original = next((value for name, value in environment.items()
                         if name.casefold() == _COMPATIBILITY_ENV.casefold()), None)
        environment[_ORIGINAL_ENV] = json.dumps(original)
    _replace_compatibility_value(environment, '0')


def restore_rtss_environment(environment):
    """更新助手启动前恢复原环境，避免新版托盘和回滚进程继承 GUI 的声明。"""
    encoded = environment.pop(_ORIGINAL_ENV, None)
    if encoded is None:
        return
    original = json.loads(encoded)
    if original is not None and not isinstance(original, str):
        raise ValueError('非法 RTSS 原环境记录')
    _replace_compatibility_value(environment, original)


def is_rtss_running() -> bool:
    """检查当前系统是否正在运行 RTSS 核心进程。"""
    if os.name != 'nt':
        return False
    try:
        from gui_focus import snapshot_processes
        return any(proc.executable.casefold() in ('rtss.exe', 'rtsshooksloader64.exe')
                   for proc in snapshot_processes())
    except Exception:
        return False


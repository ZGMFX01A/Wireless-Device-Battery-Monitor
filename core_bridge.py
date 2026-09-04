"""公开壳到私有核心的运行时桥接层。

这个模块的职责不是重复实现协议，而是把公开仓库真正需要依赖的
 DTO、扫描入口、生命周期动作和读电入口统一收口到一个稳定表面。

这样后续即使私有核心内部继续拆模块或调整实现，公开壳也只需要
 维持对这个桥接层的依赖，而不再散落直接 import 私有实现细节。
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal, Optional

from mouse_battery_core.logitech_hid import (
    BatteryInfo,
    LogitechReceiver,
    find_logitech_receivers,
)
from mouse_battery_core.razer_hid import (
    RazerBatteryInfo,
    RazerDevice,
    find_razer_devices,
)
from mouse_battery_core.asus_hid import (
    AsusBatteryInfo,
    AsusMouseDevice,
    create_asus_mouse_device,
    find_asus_mouse_devices,
)
from mouse_battery_core.keyboard_hid import (
    KeyboardCandidate,
    KeyboardInfo,
    enumerate_keyboard_candidates as enumerate_weikav_keyboard_candidates,
    read_keyboard_battery as read_weikav_keyboard_battery,
)
from mouse_battery_core.asus_keyboard_hid import (
    enumerate_asus_keyboard_candidates,
    read_asus_keyboard_battery,
)
from mouse_battery_core.bluetooth_gatt import (
    BluetoothCandidate,
    BluetoothInfo,
    enumerate_bluetooth_candidates as _enumerate_bluetooth_candidates,
    probe_bluetooth_candidate as _probe_bluetooth_candidate,
    read_bluetooth_batteries as _read_bluetooth_batteries,
)


# 公开壳只需要知道“这是哪个品牌的后端”，
# 不需要感知私有核心内部更细的模块拆分。
MouseBackendBrand = Literal["logitech", "razer", "asus"]


@dataclass
class MouseBackendHandle:
    """公开壳持有的鼠标后端句柄。

    - `brand`：用于公开壳按品牌决定展示名和少量诊断逻辑
    - `device`：私有核心里的真实设备对象，仅桥接层和设备管理器内部流转
    - `product_id` / `product_name` / `path`：供日志和公开壳状态编排使用
    """

    brand: MouseBackendBrand
    device: LogitechReceiver | RazerDevice | AsusMouseDevice
    product_id: int
    product_name: str
    path: object


def enumerate_mouse_backends() -> list[MouseBackendHandle]:
    """枚举并打开当前全部可用鼠标后端。"""
    handles: list[MouseBackendHandle] = []

    for dev_info in find_logitech_receivers():
        receiver = LogitechReceiver(dev_info)
        if receiver.open():
            handles.append(
                MouseBackendHandle(
                    brand="logitech",
                    device=receiver,
                    product_id=dev_info["product_id"],
                    product_name=receiver.product_string,
                    path=receiver.path,
                )
            )

    for dev_info in find_razer_devices():
        device = RazerDevice(dev_info)
        if device.open():
            handles.append(
                MouseBackendHandle(
                    brand="razer",
                    device=device,
                    product_id=dev_info["product_id"],
                    product_name=device.product_name,
                    path=device.path,
                )
            )

    for dev_info in find_asus_mouse_devices():
        device = create_asus_mouse_device(dev_info)
        if device.open():
            handles.append(
                MouseBackendHandle(
                    brand="asus",
                    device=device,
                    product_id=dev_info["product_id"],
                    product_name=device.product_name,
                    path=device.path,
                )
            )

    return handles


def close_mouse_backend(handle: MouseBackendHandle):
    """关闭单个鼠标后端。"""
    handle.device.close()


def read_mouse_battery(handle: MouseBackendHandle) -> Optional[BatteryInfo | RazerBatteryInfo | AsusBatteryInfo]:
    """统一读取鼠标后端电量。

    公开壳不再直接分品牌 import 私有对象，只通过桥接层拿到读电结果。
    """
    if handle.brand == "logitech":
        receiver = handle.device
        if receiver.product_id in (0xC539, 0xC547):
            return receiver.get_battery_legacy_long()
        return receiver.get_battery()

    if handle.brand == "razer":
        return handle.device.get_battery()

    return handle.device.get_battery()


def enumerate_keyboard_candidates() -> list[KeyboardCandidate]:
    """枚举现有 Weikav 键盘与 ROG 2.4G/Omni 键盘候选。"""
    return enumerate_weikav_keyboard_candidates() + enumerate_asus_keyboard_candidates()


def read_keyboard_battery(binding: dict) -> KeyboardInfo:
    """按 VID/PID 将键盘电量读取分派到 Weikav 或 ROG 协议。"""
    if int(binding.get("vendor_id", 0) or 0) == 0x0B05:
        return read_asus_keyboard_battery(binding)
    return read_weikav_keyboard_battery(binding)


def keyboard_binding_from_candidate(candidate: KeyboardCandidate) -> dict:
    """把公开候选 DTO 转成可持久化的绑定结构。"""
    return {
        "device_id": candidate.device_id,
        "vendor_id": candidate.vendor_id,
        "product_id": candidate.product_id,
        "usage_page": candidate.usage_page,
        "usage": candidate.usage,
        "interface_number": candidate.interface_number,
        "product_name": candidate.product_name,
    }


def keyboard_binding_from_info(keyboard: KeyboardInfo) -> dict:
    """把键盘快照 DTO 转成可持久化绑定结构。

    读取成功后，tray 进程会用当前真实可读接口回写配置，
    让后续重插或系统重枚举时仍能定位到最新路径。
    """
    return {
        "device_id": keyboard.device_id,
        "vendor_id": keyboard.vendor_id,
        "product_id": keyboard.product_id,
        "usage_page": keyboard.usage_page,
        "usage": keyboard.usage,
        "interface_number": keyboard.interface_number,
        "product_name": keyboard.product_name,
    }


def enumerate_bluetooth_candidates() -> list[BluetoothCandidate]:
    """枚举 Windows 已配对 BLE 设备，包含未连接或休眠设备。"""
    return _enumerate_bluetooth_candidates()


def probe_bluetooth_candidate(candidate: BluetoothCandidate) -> BluetoothInfo:
    """绑定前验证在线设备的标准 Battery Service。"""
    return _probe_bluetooth_candidate(candidate)


def read_bluetooth_batteries(bindings: list[dict]) -> list[BluetoothInfo]:
    """批量刷新公开壳保存的 BLE 绑定。"""
    return _read_bluetooth_batteries(bindings)


def bluetooth_binding_from_candidate(candidate: BluetoothCandidate) -> dict:
    """把 BLE 候选 DTO 转成公开壳可持久化的最小绑定。"""
    return {
        'device_id': candidate.device_id,
        'name': candidate.name,
    }

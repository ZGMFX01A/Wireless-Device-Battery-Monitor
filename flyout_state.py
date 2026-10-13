"""把只读共享快照转换为 WebView 可序列化状态。"""
from dataclasses import asdict

from config import APP_VERSION
from i18n import translate_brand_name, translate_runtime_text


def device_state(device, language):
    """保留设备身份与实际读取时间，翻译界面中的品牌和状态。"""
    data = asdict(device)
    brand = data.get('brand', '')
    data['brand'] = translate_brand_name(getattr(brand, 'value', brand), language)
    data['status_text'] = translate_runtime_text(data.get('status_text', ''), language)
    return data


def get_flyout_state(manager):
    """生成完整 GUI 状态；配置 getter 同步其他进程写入的偏好。"""
    dm, cm = manager.device_manager, manager.config_manager
    language = cm.effective_ui_language
    keyboard = dm.keyboard
    config = dict(cm.config)
    config['autostart'] = cm.check_autostart()
    config['auto_refresh'] = manager.auto_refresh
    config['effective_ui_language'] = language
    return {
        'version': APP_VERSION,
        'mice': [device_state(device, language) for device in dm.mice],
        'keyboard': device_state(keyboard, language) if keyboard else None,
        'bluetooth_devices': [device_state(device, language) for device in dm.bluetooth_devices],
        'config': config,
        'mode': manager.mode,
        'keyboard_operation': {
            'state': dm.keyboard_scan_state,
            'message': translate_runtime_text(dm.keyboard_scan_message, language),
            'request_id': dm.keyboard_request_id,
        },
        'bluetooth_operation': {
            'state': dm.bluetooth_scan_state,
            'message': translate_runtime_text(dm.bluetooth_scan_message, language),
            'request_id': dm.bluetooth_request_id,
        },
        'read_state': dm.last_read_state,
        'read_error': dm.last_read_error,
    }

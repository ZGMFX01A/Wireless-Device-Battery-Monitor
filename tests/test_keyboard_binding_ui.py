import json
import os
import tempfile
import threading
from types import SimpleNamespace
import unittest
from unittest import mock

import devices
from core_bridge import KeyboardCandidate


class KeyboardBindingDeviceTests(unittest.TestCase):
    """测试底层硬件设备管理器的键盘绑定、解绑与状态同步机制。"""

    def test_tray_publishes_binding_before_slow_read(self):
        manager = devices.DeviceManager.__new__(devices.DeviceManager)
        manager._lock = threading.Lock()
        manager._io_lock = threading.Lock()
        manager._keyboard_candidates = [KeyboardCandidate(
            device_id='keyboard-1', vendor_id=1, product_id=2, display_name='My keyboard',
            product_name='My keyboard', interface_number=0, usage_page=0, usage=0,
        )]
        manager.config_manager = mock.Mock()
        manager._notify_update = mock.Mock()

        def read():
            self.assertEqual(manager.keyboard_scan_state, 'binding')
            self.assertEqual(manager.keyboard_request_id, 123)
            manager._notify_update.assert_called_once()

        manager._refresh_keyboard_locked = mock.Mock(side_effect=read)
        manager._bind_keyboard('keyboard-1', request_id=123)
        self.assertEqual(manager.keyboard_scan_state, 'bound')
        self.assertEqual(manager._notify_update.call_count, 2)

        manager._unbind_keyboard()
        self.assertEqual(manager.keyboard_scan_state, 'idle')
        self.assertEqual(manager.keyboard_binding_result[0:2], (123, 'bound'))

    def test_tray_read_exception_returns_matching_error(self):
        manager = devices.DeviceManager.__new__(devices.DeviceManager)
        manager._lock = threading.Lock()
        manager._io_lock = threading.Lock()
        manager._keyboard_candidates = [SimpleNamespace(device_id='keyboard-1')]
        manager.config_manager = mock.Mock()
        manager._notify_update = mock.Mock()
        manager._refresh_keyboard_locked = mock.Mock(side_effect=OSError('Read failed'))
        with mock.patch.object(devices, 'keyboard_binding_from_candidate', return_value={}):
            manager._bind_keyboard('keyboard-1', request_id=123)
        self.assertEqual(manager.keyboard_scan_state, 'error')
        self.assertEqual(manager.keyboard_request_id, 123)
        self.assertIn('Read failed', manager.keyboard_scan_message)

    def test_shared_state_reads_request_id_and_legacy_default(self):
        manager = devices.SharedStateDeviceManager()
        with tempfile.TemporaryDirectory() as directory:
            path = os.path.join(directory, 'state.json')
            with mock.patch.object(devices, 'get_shared_state_path', return_value=path):
                for state, expected in [({'keyboard_request_id': 123, 'keyboard_bind_state': 'bound',
                                         'keyboard_bind_message': 'Bound'}, 123), ({}, 0), ([], 0)]:
                    with open(path, 'w', encoding='utf-8') as handle:
                        json.dump(state, handle)
                    manager.sync_shared_state_silently()
                    self.assertEqual(manager.keyboard_request_id, expected)
                    self.assertEqual(manager.keyboard_binding_result,
                                     (123, 'bound', 'Bound') if expected else (0, 'idle', ''))


if __name__ == '__main__':
    unittest.main()


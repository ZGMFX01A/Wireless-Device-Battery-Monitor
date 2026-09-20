import json
import os
import tempfile
import unittest
from unittest import mock

import config
import devices


class ConfigPersistenceTests(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.config_path = os.path.join(self.temp_dir.name, 'config.json')
        self.path_patch = mock.patch.object(config, 'CONFIG_FILE', self.config_path)
        self.path_patch.start()
        self.cleanup_patch = mock.patch.object(config.updater, 'clean_old_version')
        self.cleanup_patch.start()

    def tearDown(self):
        self.cleanup_patch.stop()
        self.path_patch.stop()
        self.temp_dir.cleanup()

    def test_reload_discards_keys_removed_from_disk(self):
        with open(self.config_path, 'w', encoding='utf-8') as handle:
            json.dump({'bluetooth_bindings': [{'device_id': 'old', 'name': 'Old'}]}, handle)
        manager = config.ConfigManager()
        with open(self.config_path, 'w', encoding='utf-8') as handle:
            json.dump({'low_battery_notify': 10}, handle)

        self.assertEqual(manager.bluetooth_bindings, [])

    def test_autostart_command_quotes_paths_with_spaces(self):
        self.assertEqual(
            config._autostart_command(r'C:\Program Files\Mouse Battery\MouseBattery.exe'),
            r'"C:\Program Files\Mouse Battery\MouseBattery.exe"',
        )


class DeviceCommandQueueTests(unittest.TestCase):
    def test_requests_are_queued_without_overwriting_each_other(self):
        with tempfile.TemporaryDirectory() as directory:
            legacy_path = os.path.join(directory, '.device_command.json')
            with mock.patch.object(devices, 'get_device_command_path', return_value=legacy_path):
                devices.request_device_command('scan_keyboard_candidates')
                devices.request_device_command('unbind_keyboard')
            queue = legacy_path + '.queue'
            actions = []
            for name in os.listdir(queue):
                with open(os.path.join(queue, name), encoding='utf-8') as handle:
                    actions.append(json.load(handle)['action'])
            self.assertCountEqual(actions, ['scan_keyboard_candidates', 'unbind_keyboard'])


class SnapshotOptimisticRemovalTests(unittest.TestCase):
    def test_optimistic_removed_bluetooth_ids_filters_device(self):
        from core_bridge import BluetoothInfo
        import gui

        dm = mock.MagicMock()
        dm.bluetooth_devices = [
            BluetoothInfo(device_id='dev1', name='Dev 1', percentage=80, status_text='已连接', online=True, last_update=100.0),
            BluetoothInfo(device_id='dev2', name='Dev 2', percentage=50, status_text='已连接', online=True, last_update=100.0),
        ]
        app = gui.MouseBatteryApp.__new__(gui.MouseBatteryApp)
        app.device_manager = dm
        app._optimistic_bluetooth_devices = {}
        app._optimistic_removed_bluetooth_ids = {'dev1'}

        snapshot = app._bluetooth_devices_snapshot()
        dev_ids = [d.device_id for d in snapshot]
        self.assertNotIn('dev1', dev_ids)
        self.assertIn('dev2', dev_ids)

        # 模拟底层托盘也移除了 dev1，验证乐观集合自动清理
        dm.bluetooth_devices = [
            BluetoothInfo(device_id='dev2', name='Dev 2', percentage=50, status_text='已连接', online=True, last_update=100.0),
        ]
        snapshot2 = app._bluetooth_devices_snapshot()
        self.assertNotIn('dev1', app._optimistic_removed_bluetooth_ids)
        self.assertEqual(len(snapshot2), 1)

    def test_optimistic_removed_keyboard(self):
        from core_bridge import KeyboardInfo
        import gui

        dm = mock.MagicMock()
        dm.keyboard = KeyboardInfo(name='KB', percentage=90, charging=False, status_text='电量良好', online=True, last_update=100.0, device_id='kb1')
        app = gui.MouseBatteryApp.__new__(gui.MouseBatteryApp)
        app.device_manager = dm
        app._optimistic_removed_keyboard = True

        self.assertIsNone(app._keyboard_snapshot())

        # 模拟底层托盘置空，验证标记自动复位
        dm.keyboard = None
        self.assertIsNone(app._keyboard_snapshot())
        self.assertFalse(app._optimistic_removed_keyboard)


if __name__ == '__main__':
    unittest.main()

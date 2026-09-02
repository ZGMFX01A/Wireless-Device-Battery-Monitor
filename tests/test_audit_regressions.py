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


if __name__ == '__main__':
    unittest.main()

import json
import os
import tempfile
import unittest
from types import SimpleNamespace
from unittest import mock

import config
import devices


class ConfigPersistenceTests(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.config_path = os.path.join(self.temp_dir.name, 'config.json')
        self.path_patch = mock.patch.object(config, 'CONFIG_FILE', self.config_path)
        self.path_patch.start()

    def tearDown(self):
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

    def test_device_custom_names_persistence_and_display(self):
        """测试设备自定义重命名：设置、查询、大小写兼容与清空还原。"""
        manager = config.ConfigManager()
        self.assertEqual(manager.device_custom_names, {})

        # 设置别名
        self.assertTrue(manager.set_device_custom_name('0x0c45:0xfefa:3', '我的客制化键盘'))
        self.assertEqual(manager.get_device_custom_name('0x0c45:0xfefa:3'), '我的客制化键盘')
        # 大小写兼容匹配
        self.assertEqual(manager.get_device_custom_name('0X0C45:0xFEFA:3'), '我的客制化键盘')
        # 获取最终显示名称
        self.assertEqual(
            manager.get_device_display_name('0x0c45:0xfefa:3', '2.4G Dongle'),
            '我的客制化键盘'
        )
        self.assertEqual(
            manager.get_device_display_name('unknown_id', '默认名称'),
            '默认名称'
        )

        # 清除别名（传入空字符串）
        self.assertTrue(manager.set_device_custom_name('0x0c45:0xfefa:3', '  '))
        self.assertEqual(manager.get_device_custom_name('0x0c45:0xfefa:3'), '')
        self.assertEqual(
            manager.get_device_display_name('0x0c45:0xfefa:3', '2.4G Dongle'),
            '2.4G Dongle'
        )

    def test_flyout_device_state_applies_custom_name(self):
        """测试 flyout_state.device_state 正确应用自定义别名并保留 raw_name。"""
        import flyout_state
        manager = config.ConfigManager()
        manager.set_device_custom_name('mouse:logitech:c539', '办公鼠标小黑')

        mouse = devices.MouseInfo(
            device_id='mouse:logitech:c539',
            name='G Pro Wireless',
            brand=devices.Brand.LOGITECH,
            percentage=85,
        )

        state = flyout_state.device_state(mouse, 'zh-CN', manager)
        self.assertEqual(state['name'], '办公鼠标小黑')
        self.assertEqual(state['custom_name'], '办公鼠标小黑')
        self.assertEqual(state['raw_name'], 'G Pro Wireless')
        self.assertEqual(state['brand'], '罗技')

    def scan_mice(self, paths, manager=None):
        manager = manager or devices.DeviceManager(config.ConfigManager())
        backends = [SimpleNamespace(brand='logitech', product_id=0xC539,
                                   product_name='G Pro', path=path) for path in paths]
        with mock.patch.object(devices, 'enumerate_mouse_backends', return_value=backends):
            manager._scan_devices()
        return manager

    def test_same_model_receivers_have_separate_persistent_names(self):
        import flyout_state
        manager = self.scan_mice([b'receiver-a', b'receiver-b'])
        first, second = manager.mice
        self.assertNotEqual(first.device_id, second.device_id)
        manager.config_manager.set_device_custom_name(first.device_id, 'Mouse A')
        self.assertEqual(flyout_state.device_state(first, 'en-US', manager.config_manager)['name'], 'Mouse A')
        self.assertEqual(flyout_state.device_state(second, 'en-US', manager.config_manager)['name'], second.name)
        self.scan_mice(['RECEIVER-B', 'RECEIVER-A'], manager)
        self.assertEqual([m.device_id for m in manager.mice], [second.device_id, first.device_id])
        snapshot = devices._serialize_mouse_state(manager.mice[1])
        restored = devices._deserialize_mouse_state(snapshot, 0)
        self.assertEqual(restored.device_id, first.device_id)

    def test_unique_legacy_mouse_name_is_migrated_without_overwriting_a_new_name(self):
        cm = config.ConfigManager()
        cm.set_device_custom_name('MOUSE:LOGITECH:C539', 'Old alias')
        manager = self.scan_mice([b'receiver-a'], devices.DeviceManager(cm))
        identity = manager.mice[0].device_id
        self.assertNotEqual(identity, 'mouse:logitech:c539')
        self.assertEqual(cm.get_device_custom_name(identity), 'Old alias')
        self.assertEqual(cm.get_device_custom_name('mouse:logitech:c539'), '')
        cm.set_device_custom_name(identity, 'New alias')
        cm.set_device_custom_name('mouse:logitech:c539', 'Stale alias')
        self.scan_mice([b'receiver-a'], manager)
        self.assertEqual(cm.get_device_custom_name(identity), 'New alias')

    def test_ambiguous_legacy_mouse_name_is_not_assigned_to_either_receiver(self):
        cm = config.ConfigManager()
        cm.set_device_custom_name('mouse:logitech:c539', 'Ambiguous alias')
        manager = self.scan_mice([b'receiver-a', b'receiver-b'], devices.DeviceManager(cm))
        for mouse in manager.mice:
            self.assertEqual(cm.get_device_custom_name(mouse.device_id), '')
        self.assertEqual(cm.get_device_custom_name('mouse:logitech:c539'), 'Ambiguous alias')


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

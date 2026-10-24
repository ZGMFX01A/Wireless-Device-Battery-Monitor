import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

import config
import core_bridge
import devices
import main
import tray_presentation


class RuntimeAuditTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.path_patch = mock.patch.object(config, 'CONFIG_FILE', str(Path(self.directory.name) / 'config.json'))
        self.path_patch.start()
        self.addCleanup(self.path_patch.stop)
        self.config = config.ConfigManager()

    def presentation(self):
        app = tray_presentation.TrayPresentation()
        app.config_manager = self.config
        app._tray = mock.Mock()
        app._t = lambda key, **params: params.get('name', key)
        app._translate_runtime_text = lambda text: text
        app._build_menu = mock.Mock()
        return app

    def test_tooltip_only_formats_text_without_notifying_or_changing_notification_state(self):
        app = self.presentation()
        mouse = devices.MouseInfo(name='Mouse', device_id='mouse-a', percentage=10, online=True)
        with mock.patch.object(self.config, 'should_notify') as check:
            self.assertIn('Mouse: 10%', app._tooltip([mouse], None, []))
        check.assert_not_called()
        app._tray.notify.assert_not_called()

    def test_icon_refresh_notifies_keyboard_once_using_its_identity_and_custom_name(self):
        app = self.presentation()
        keyboard = core_bridge.KeyboardInfo(device_id='keyboard-a', name='Keyboard', percentage=19, online=True)
        self.config.set_device_custom_name('keyboard-a', 'Office keyboard')
        app.device_manager = SimpleNamespace(mice=[], keyboard=keyboard, bluetooth_devices=[])
        with (mock.patch.object(tray_presentation, 'create_battery_icon'),
              mock.patch.object(tray_presentation, 'is_windows_taskbar_dark_mode', return_value=False)):
            app._update_icon()
            keyboard.name = 'Updated hardware name'
            app._update_icon()
        app._tray.notify.assert_called_once_with('Office keyboard', title='tray.notification.low_battery_title')
        self.assertIn('Keyboard|keyboard-a', self.config.config['notified_levels'])

    def test_offline_charging_and_invalid_readings_do_not_consume_notification_state(self):
        app = self.presentation()
        for online, charging, percentage in [(False, False, 10), (True, True, 10),
                                               (True, False, -1), (True, False, 101)]:
            keyboard = core_bridge.KeyboardInfo(name='Keyboard', online=online,
                                                charging=charging, percentage=percentage)
            with mock.patch.object(self.config, 'should_notify') as check:
                app._notify_low_battery(keyboard, 'Keyboard|keyboard-a')
            check.assert_not_called()
        app._tray.notify.assert_not_called()

    def test_notification_check_keeps_each_device_kind_separate(self):
        app = self.presentation()
        mouse = devices.MouseInfo(device_id='shared-id', name='Same')
        keyboard = core_bridge.KeyboardInfo(device_id='shared-id', name='Same')
        bluetooth = core_bridge.BluetoothInfo(device_id='shared-id', name='Same')
        with mock.patch.object(app, '_notify_low_battery') as notify:
            app._check_low_battery_notifications([mouse], keyboard, [bluetooth])
        self.assertEqual([call.args[1] for call in notify.call_args_list],
                         ['Mouse|shared-id', 'Keyboard|shared-id', 'Bluetooth|shared-id'])

    def scan_logitech(self, receiver):
        manager = devices.DeviceManager(self.config)
        backend = core_bridge.MouseBackendHandle('logitech', receiver, 0xC545, 'USB Receiver', b'receiver-a')
        with mock.patch.object(devices, 'enumerate_mouse_backends', return_value=[backend]):
            manager._scan_devices()
        return manager

    def test_real_logitech_name_is_read_once_after_valid_battery_without_changing_identity(self):
        receiver = mock.Mock()
        receiver.get_device_name.return_value = '  PRO X SUPERLIGHT  '
        manager = self.scan_logitech(receiver)
        mouse = manager.mice[0]
        identity, poll_key = mouse.device_id, manager._device_key(mouse, 0)
        self.config.set_device_custom_name(identity, 'Personal alias')
        receiver.get_device_name.assert_not_called()
        battery = core_bridge.BatteryInfo(percentage=64, charging=False, status_text='Connected')
        with mock.patch.object(devices, 'read_mouse_battery', return_value=battery):
            manager._refresh_battery(devices.BatteryRefreshMode.STARTUP)
            manager._refresh_battery()
        self.assertEqual(mouse.name, 'PRO X SUPERLIGHT')
        self.assertEqual((mouse.device_id, manager._device_key(mouse, 0)), (identity, poll_key))
        receiver.get_device_name.assert_called_once_with(1)
        self.assertEqual(self.config.get_device_display_name(identity, mouse.name), 'Personal alias')
        self.assertEqual(list(manager._poll_plan.slots), [poll_key])

    def test_missing_or_failed_name_queries_preserve_the_valid_battery_and_fallback(self):
        for result in (None, RuntimeError('Name unsupported')):
            with self.subTest(result=result):
                receiver = mock.Mock()
                if isinstance(result, Exception):
                    receiver.get_device_name.side_effect = result
                else:
                    receiver.get_device_name.return_value = result
                manager = self.scan_logitech(receiver)
                fallback = manager.mice[0].name
                with mock.patch.object(devices, 'read_mouse_battery', return_value=core_bridge.BatteryInfo(percentage=64)):
                    manager._refresh_battery(devices.BatteryRefreshMode.STARTUP)
                    manager._refresh_battery()
                self.assertEqual(manager.mice[0].name, fallback)
                self.assertTrue(manager.mice[0].online)
                self.assertEqual(manager.mice[0].percentage, 64)
                receiver.get_device_name.assert_called_once_with(1)

    def test_startup_cleanup_only_removes_old_shared_state_temporary_files(self):
        root = Path(self.directory.name)
        now = 10_000
        stale = root / '.device_state.json.12.34.56.tmp'
        protected = [root / '.device_state.json', root / '.device_state.json.12.34.57.tmp',
                     root / 'unrelated.tmp', root / '.device_state.json.12.34.58.tmp',
                     root / '.device_state.json.invalid.tmp']
        for file in [stale, *protected]:
            file.write_text('keep', encoding='utf-8')
            os.utime(file, (now - 7200, now - 7200))
        os.utime(protected[1], (now - 10, now - 10))
        os.utime(protected[3], (now - 3600, now - 3600))
        queue = root / '.device_command.json.queue'
        queue.mkdir()
        command = queue / '123.json.tmp'
        command.write_text('pending', encoding='utf-8')
        folder = root / '.device_state.json.99.99.99.tmp'
        folder.mkdir()
        with mock.patch.object(devices, 'get_shared_state_path', return_value=str(protected[0])):
            self.assertEqual(devices.cleanup_stale_shared_state_files(now=now), 1)
        self.assertFalse(stale.exists())
        self.assertTrue(all(file.exists() for file in [*protected, command, folder]))

    def test_cleanup_skips_inaccessible_stale_files(self):
        state = Path(self.directory.name) / '.device_state.json'
        stale = state.with_name(state.name + '.1.2.3.tmp')
        stale.touch()
        os.utime(stale, (1, 1))
        with (mock.patch.object(devices, 'get_shared_state_path', return_value=str(state)),
              mock.patch.object(devices.os, 'unlink', side_effect=PermissionError('Busy'))):
            self.assertEqual(devices.cleanup_stale_shared_state_files(now=10_000), 0)
        self.assertTrue(stale.exists())

    def test_cleanup_never_follows_a_symbolic_link(self):
        entry = mock.Mock(name='.device_state.json.1.2.3.tmp')
        entry.name = '.device_state.json.1.2.3.tmp'
        entry.is_file.return_value = False
        scan = mock.MagicMock()
        scan.__enter__.return_value = [entry]
        with (mock.patch.object(devices, 'get_shared_state_path', return_value=str(Path(self.directory.name) / '.device_state.json')),
              mock.patch.object(devices.os, 'scandir', return_value=scan),
              mock.patch.object(devices.os, 'unlink') as unlink):
            self.assertEqual(devices.cleanup_stale_shared_state_files(now=10_000), 0)
        entry.is_file.assert_called_once_with(follow_symlinks=False)
        entry.stat.assert_not_called()
        unlink.assert_not_called()


class SingleInstanceTests(unittest.TestCase):
    def test_mutex_is_scoped_to_the_current_logon_session(self):
        kernel = mock.Mock()
        kernel.CreateMutexW.return_value = 123
        kernel.GetLastError.return_value = 0
        with (mock.patch.object(main.ctypes, 'windll', SimpleNamespace(kernel32=kernel)),
              mock.patch.object(main, '_instance_mutex_handle', None)):
            self.assertTrue(main.acquire_single_instance())
        self.assertEqual(kernel.CreateMutexW.call_args.args[2], r'Local\MouseBattery_Main_SingleInstance')

    def test_existing_local_instance_is_rejected(self):
        kernel = mock.Mock()
        kernel.CreateMutexW.return_value = 123
        kernel.GetLastError.return_value = 183
        with (mock.patch.object(main.ctypes, 'windll', SimpleNamespace(kernel32=kernel)),
              mock.patch.object(main, '_instance_mutex_handle', None)):
            self.assertFalse(main.acquire_single_instance())

    def test_access_denied_does_not_silently_allow_a_duplicate(self):
        kernel = mock.Mock()
        kernel.CreateMutexW.return_value = None
        kernel.GetLastError.return_value = 5
        with (mock.patch.object(main.ctypes, 'windll', SimpleNamespace(kernel32=kernel)),
              mock.patch.object(main, '_instance_mutex_handle', None)):
            self.assertFalse(main.acquire_single_instance())


if __name__ == '__main__':
    unittest.main()

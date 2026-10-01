import asyncio
import json
import os
import tempfile
import threading
from types import SimpleNamespace
import unittest
from unittest import mock

import devices
import gui
from core_bridge import KeyboardCandidate


class KeyboardBindingTests(unittest.TestCase):
    def make_app(self):
        app = gui.MouseBatteryApp.__new__(gui.MouseBatteryApp)
        app._keyboard_dialog = SimpleNamespace(open=True, content=None, actions=[])
        app._keyboard_bind_action = SimpleNamespace(disabled=False)
        app._keyboard_selected_device_id = 'keyboard-1'
        app._keyboard_dialog_loading = False
        app._keyboard_pending_request_id = 0
        app._keyboard_binding_name = ''
        app._keyboard_binding_started = 0
        app._keyboard_cancel_action = SimpleNamespace(disabled=False, content='Cancel')
        app.device_manager = SimpleNamespace(keyboard_request_id=0, keyboard_binding_result=(0, 'idle', ''))
        app.page = mock.Mock()
        app._keyboard_candidates_snapshot = mock.Mock(return_value=[
            SimpleNamespace(device_id='keyboard-1', display_name='My keyboard'),
        ])
        app._keyboard_scan_state = mock.Mock(return_value=('ready', 'Old scan'))
        app._keyboard_snapshot = mock.Mock(return_value=None)
        app._safe_update = mock.Mock()
        app._show_dialog = mock.Mock()
        app._t = lambda key, **values: key
        app._translate_runtime_text = lambda text: text
        return app

    def test_stale_response_and_binding_response_keep_waiting(self):
        app = self.make_app()
        app._keyboard_pending_request_id = 123
        app.device_manager.keyboard_request_id = 122
        app._keyboard_scan_state.return_value = ('bound', 'Old result')
        app.device_manager.keyboard_binding_result = (122, 'bound', 'Old result')
        app._refresh_keyboard_dialog()
        self.assertTrue(app._keyboard_dialog.open)
        self.assertEqual(app._keyboard_pending_request_id, 123)
        self.assertTrue(app._keyboard_bind_action.disabled)
        app.device_manager.keyboard_request_id = 123
        app._keyboard_scan_state.return_value = ('binding', '')
        app.device_manager.keyboard_binding_result = (123, 'binding', '')
        app._refresh_keyboard_dialog()
        self.assertTrue(app._keyboard_dialog.open)

    def test_matching_success_closes_dialog(self):
        app = self.make_app()
        app._keyboard_pending_request_id = 123
        app.device_manager.keyboard_request_id = 123
        app._keyboard_scan_state.return_value = ('bound', 'Bound')
        app.device_manager.keyboard_binding_result = (123, 'bound', 'Bound')
        app._refresh_keyboard_dialog()
        self.assertFalse(app._keyboard_dialog.open)
        self.assertEqual(app._keyboard_pending_request_id, 0)

    def test_matching_error_is_visible_and_allows_retry(self):
        app = self.make_app()
        app._keyboard_pending_request_id = 123
        app.device_manager.keyboard_request_id = 123
        app._keyboard_scan_state.return_value = ('error', 'Read failed')
        app.device_manager.keyboard_binding_result = (123, 'error', 'Read failed')
        app._refresh_keyboard_dialog()
        self.assertTrue(app._keyboard_dialog.open)
        self.assertEqual(app._keyboard_bind_error, 'Read failed')
        self.assertFalse(app._keyboard_bind_action.disabled)
        self.assertEqual(app._keyboard_pending_request_id, 0)

    def test_slow_request_can_close_and_reopen_without_resubmitting(self):
        app = self.make_app()
        app._keyboard_pending_request_id = 123
        app._keyboard_binding_started = 100
        with mock.patch.object(gui.time, 'monotonic', return_value=105):
            app._close_keyboard_dialog()
            self.assertTrue(app._keyboard_dialog.open)
        with mock.patch.object(gui.time, 'monotonic', return_value=131):
            app._refresh_keyboard_dialog()
            self.assertFalse(app._keyboard_cancel_action.disabled)
            app._close_keyboard_dialog()
        self.assertFalse(app._keyboard_dialog.open)
        self.assertEqual(app._keyboard_pending_request_id, 123)
        app._open_keyboard_picker_dialog = mock.Mock()
        with mock.patch.object(gui, 'request_device_command') as submit:
            app._on_add_keyboard_click(None)
            app._open_keyboard_picker_dialog.assert_called_once()
            submit.assert_not_called()

    def test_submission_error_does_not_enter_busy_state(self):
        app = self.make_app()
        with mock.patch.object(gui, 'request_device_command', side_effect=OSError('write failed')):
            app._on_bind_keyboard_click(None)
        self.assertEqual(app._keyboard_pending_request_id, 0)
        app._show_dialog.assert_called_once()

    def test_reopen_after_completion_does_not_leave_spinner_open(self):
        app = self.make_app()
        app._keyboard_pending_request_id = 123
        app.device_manager.keyboard_binding_result = (123, 'bound', 'Bound')
        app.page.show_dialog.side_effect = lambda dialog: setattr(dialog, 'open', True)
        app._open_keyboard_picker_dialog()
        self.assertFalse(app._keyboard_dialog.open)
        self.assertEqual(app._keyboard_pending_request_id, 0)

    def test_old_scan_error_cannot_complete_new_binding(self):
        app = self.make_app()
        app._keyboard_pending_request_id = 123
        app._keyboard_scan_state.return_value = ('error', 'Old failure')
        app.device_manager.keyboard_binding_result = (123, 'binding', '')
        app._refresh_keyboard_dialog()
        self.assertEqual(app._keyboard_pending_request_id, 123)
        self.assertTrue(app._keyboard_dialog.open)

    def test_submission_keeps_dialog_open_and_blocks_duplicate(self):
        app = self.make_app()
        with mock.patch.object(gui, 'request_device_command', return_value=123) as submit:
            app._on_bind_keyboard_click(None)
            self.assertTrue(app._keyboard_dialog.open)
            self.assertEqual(app._keyboard_pending_request_id, 123)
            self.assertTrue(app._keyboard_bind_action.disabled)
            app._on_bind_keyboard_click(None)
            submit.assert_called_once()

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
        app = self.make_app()
        app.device_manager = manager
        app._keyboard_pending_request_id = 123
        app._keyboard_scan_state.return_value = ('idle', '')
        app._refresh_keyboard_dialog()
        self.assertEqual(app._keyboard_pending_request_id, 0)

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

    def test_binding_poll_works_without_auto_refresh(self):
        app = self.make_app()
        app.device_manager.sync_shared_state_silently = mock.Mock()
        app._refresh_ui = mock.Mock(side_effect=lambda: setattr(app, '_keyboard_pending_request_id', 0))
        with mock.patch.object(gui, 'request_device_command', return_value=123):
            app._on_bind_keyboard_click(None)
        asyncio.run(app.page.run_task.call_args.args[0]())
        app.device_manager.sync_shared_state_silently.assert_called_once()
        app._refresh_ui.assert_called_once()


if __name__ == '__main__':
    unittest.main()

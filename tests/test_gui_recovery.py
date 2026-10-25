"""GUI 故障退出、打开意图保留与有限重试的回归验证。"""
import os
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import MagicMock, patch

import gui_process
from flyout import FlyoutManager


ANCHOR = {'scale': 1.0, 'rect': [0, 0, 100, 100], 'work_area': [0, 0, 1920, 1080]}


def manager_for_test(mode='flyout'):
    manager = FlyoutManager(MagicMock(), MagicMock(), {
        'closed': threading.Event(), 'webview': MagicMock(),
        'refresh_tray': MagicMock(), 'host_factory': MagicMock(),
        'anchor': ANCHOR, 'mode': mode,
    })
    manager._host = MagicMock()
    manager._updates = MagicMock()
    manager.window = MagicMock()
    manager._on_devices_updated = MagicMock()
    return manager


class GuiRecoveryTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        paths = patch.object(gui_process, 'message_path', side_effect=lambda channel: self.root / str(channel))
        paths.start()
        self.addCleanup(paths.stop)
        self.spawn = MagicMock(side_effect=lambda *a, **kw: MagicMock(poll=lambda: None))
        self.notify = MagicMock()
        self.controller = gui_process.GuiProcessController({
            'spawn': self.spawn, 'executable': 'python', 'entry': 'main.py',
            'frozen': False, 'host_pid': 1, 'creationflags': 0,
            'watch_exit': False, 'on_error': self.notify,
        })

    def fail_current(self, request, detail='BrowserProcessExited'):
        gui_process.write_gui_failure(self.controller._channel, request, detail)
        self.controller._process_exited(self.controller.process, self.controller._channel,
                                        gui_process.GUI_FAILURE_EXIT_CODE)

    def test_consumed_wake_failure_reopens_settings_at_requested_anchor(self):
        self.controller.open('flyout', ANCHOR)
        self.controller.open('settings', ANCHOR)
        manager = manager_for_test()
        manager._ready.set()
        manager._message_channel = self.controller._channel
        manager._host.scripts.evaluate.side_effect = RuntimeError('BrowserProcessExited')
        message = manager._next_message()  # 模拟子进程已经领取并删除打开请求。
        with patch.object(manager, '_close'):
            try:
                manager._handle_message(message)
            except RuntimeError as error:
                manager.on_frontend_error(str(error))
        self.assertIsNotNone(manager.startup_error, '唤醒失败必须上报给托盘恢复')
        self.fail_current(manager.recovery_request)
        self.assertEqual(self.spawn.call_count, 2)
        self.assertEqual(self.spawn.call_args.args[0][-1], '--settings')
        self.assertIn('100', self.spawn.call_args.kwargs['env']['MOUSE_BATTERY_WINDOW_ANCHOR'])
        self.notify.assert_not_called()

    def test_repeated_failures_stop_after_one_retry_and_new_click_can_open(self):
        self.controller.open('flyout', ANCHOR)
        request = {'action': 'toggle', 'anchor': ANCHOR}
        self.fail_current(request)
        self.fail_current(request)
        self.assertEqual(self.spawn.call_count, 2)
        self.assertIsNone(self.controller.process)
        self.notify.assert_called_once()
        self.controller.open('settings', ANCHOR)
        self.fail_current({'action': 'settings', 'anchor': ANCHOR})
        self.assertEqual(self.spawn.call_count, 4)

    def test_fault_between_claim_and_show_preserves_claimed_open(self):
        self.controller.open('flyout', ANCHOR)
        manager = manager_for_test()
        manager._ready.set()
        manager._visible = True
        manager._hide_window()
        manager._message_channel = self.controller._channel
        self.controller.open('settings', dict(ANCHOR, scale=1.5))
        # 在领取与执行之间插入 UI 线程的 ProcessFailed，不能依赖 show() 已执行。
        message = manager._next_message()
        with patch.object(manager, '_close'):
            manager.on_frontend_error('BrowserProcessExited')
            manager._handle_message(message)
        self.assertIsNotNone(manager.recovery_request)
        self.fail_current(manager.recovery_request)
        self.assertEqual(self.spawn.call_count, 2)
        self.assertEqual(self.spawn.call_args.args[0][-1], '--settings')
        self.assertIn('1.5', self.spawn.call_args.kwargs['env']['MOUSE_BATTERY_WINDOW_ANCHOR'])

    def test_fault_before_claim_leaves_click_for_parent(self):
        self.controller.open('flyout', ANCHOR)
        manager = manager_for_test()
        manager._ready.set()
        manager._visible = True
        manager._hide_window()
        manager._message_channel = self.controller._channel
        self.controller.open('settings', ANCHOR)
        with patch.object(manager, '_close'):
            manager.on_frontend_error('BrowserProcessExited')
            self.assertIsNone(manager._next_message())
        self.fail_current(manager.recovery_request)
        self.assertEqual(self.spawn.call_count, 2)
        self.assertEqual(self.spawn.call_args.args[0][-1], '--settings')

    def test_claimed_toggle_has_same_intent_as_window_action(self):
        for mode, should_hide in [('flyout', True), ('settings', False)]:
            with self.subTest(mode=mode):
                manager = manager_for_test(mode)
                manager._ready.set()
                manager._visible = True
                manager._actions.put({'action': 'toggle'})
                message = manager._next_message()
                with patch.object(manager, '_close'):
                    manager.on_frontend_error('BrowserProcessExited')
                    manager._handle_message(message)
                self.assertEqual(manager.recovery_request is None, should_hide)

    def test_hidden_fault_does_not_reopen_but_pending_click_is_delivered(self):
        self.controller.open('flyout', ANCHOR)
        self.fail_current(None)
        self.assertEqual(self.spawn.call_count, 1)
        self.notify.assert_not_called()
        self.controller.open('flyout', ANCHOR)
        gui_process.send_message(self.controller._channel, 'settings', ANCHOR)
        self.fail_current(None)
        self.assertEqual(self.spawn.call_count, 3)
        self.assertEqual(self.spawn.call_args.args[0][-1], '--settings')

    def test_shutdown_cancels_recovery_and_notification(self):
        self.controller.open('flyout', ANCHOR)
        self.controller._stopping = True
        self.fail_current({'action': 'toggle', 'anchor': ANCHOR})
        self.assertEqual(self.spawn.call_count, 1)
        self.notify.assert_not_called()

    def test_pending_shutdown_overrides_fault_open_intent(self):
        self.controller.open('flyout', ANCHOR)
        gui_process.send_message(self.controller._channel, 'shutdown')
        self.fail_current({'action': 'toggle', 'anchor': ANCHOR})
        self.assertEqual(self.spawn.call_count, 1)
        self.assertIsNone(self.controller.process)
        self.notify.assert_not_called()

    def test_new_pending_click_overrides_fault_mode_and_anchor(self):
        self.controller.open('flyout', ANCHOR)
        target = dict(ANCHOR, scale=1.5)
        gui_process.send_message(self.controller._channel, 'settings', target)
        self.fail_current({'action': 'toggle', 'anchor': ANCHOR})
        self.assertEqual(self.spawn.call_args.args[0][-1], '--settings')
        self.assertIn('1.5', self.spawn.call_args.kwargs['env']['MOUSE_BATTERY_WINDOW_ANCHOR'])

    def test_corrupt_failure_report_does_not_strand_dead_process(self):
        self.controller.open('flyout', ANCHOR)
        channel = self.controller._channel
        path = self.root / channel
        path.mkdir()
        (path / 'failure.result').write_text('invalid json', encoding='utf-8')
        self.controller._process_exited(self.controller.process, channel, gui_process.GUI_FAILURE_EXIT_CODE)
        self.assertIsNone(self.controller.process)
        self.assertFalse(path.exists())
        self.controller.open('settings', ANCHOR)
        self.assertEqual(self.spawn.call_count, 2)

    def test_idle_exit_preserves_pending_open_without_recovery_budget(self):
        self.controller.open('flyout', ANCHOR)
        gui_process.send_message(self.controller._channel, 'settings', ANCHOR)
        self.controller._process_exited(self.controller.process, self.controller._channel,
                                        gui_process.GUI_IDLE_EXIT_CODE)
        self.assertEqual(self.spawn.call_count, 2)
        self.fail_current({'action': 'settings', 'anchor': ANCHOR})
        self.assertEqual(self.spawn.call_count, 3)

    def test_stale_watcher_cannot_replace_a_new_process(self):
        self.controller.open('flyout', ANCHOR)
        old, channel = self.controller.process, self.controller._channel
        old.poll = lambda: 0
        self.controller.open('settings', ANCHOR)
        new = self.controller.process
        self.controller._process_exited(old, channel, 76)
        self.assertIs(self.controller.process, new)

    def test_failure_report_is_not_consumed_as_window_message_and_is_cleaned(self):
        self.controller.open('flyout', ANCHOR)
        channel = self.controller._channel
        gui_process.write_gui_failure(channel, None, 'BrowserProcessExited')
        self.assertIsNone(gui_process.consume_message(channel))
        self.fail_current(None)
        self.assertFalse((self.root / channel).exists())

    def test_managed_startup_failure_returns_to_parent_without_modal_dialog(self):
        self.check_managed_failure('startup')

    def test_managed_visible_failure_returns_to_parent_without_modal_dialog(self):
        self.check_managed_failure('visible')

    def test_managed_hidden_failure_does_not_reopen_or_show_dialog(self):
        self.check_managed_failure('hidden')

    def check_managed_failure(self, stage):
        import flyout_runtime
        self.controller.open('flyout', ANCHOR)
        channel = self.controller._channel
        with patch.dict(os.environ, {gui_process.GUI_CHANNEL_ENV: channel}):
            manager = manager_for_test()
            if stage != 'startup':
                manager._ready.set()
                manager._visible = True
            if stage == 'hidden':
                manager._hide_window()
            def fail(*args, **kwargs):
                with patch.object(manager, '_close'):
                    manager.on_frontend_error('WebView2 进程故障: BrowserProcessExited')
            with (patch.object(flyout_runtime, 'initialize_dpi_awareness'),
                  patch.object(flyout_runtime, 'require_webview2'),
                  patch.object(flyout_runtime, 'SharedStateDeviceManager'),
                  patch.object(flyout_runtime, 'compose_gui', return_value=(manager, MagicMock())),
                  patch('main.start_update_shutdown_watchdog'),
                  patch.object(flyout_runtime.webview, 'start', side_effect=fail),
                  patch.object(flyout_runtime.ctypes.windll.user32, 'MessageBoxW') as dialog):
                code = flyout_runtime.run_gui('flyout')
        self.assertEqual(code, gui_process.GUI_FAILURE_EXIT_CODE)
        dialog.assert_not_called()
        self.controller._process_exited(self.controller.process, channel, code)
        self.assertEqual(self.spawn.call_count, 1 if stage == 'hidden' else 2)


if __name__ == '__main__':
    unittest.main()

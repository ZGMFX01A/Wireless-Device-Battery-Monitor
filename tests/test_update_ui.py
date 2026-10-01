import asyncio
import importlib
import sys
import threading
from types import SimpleNamespace
import unittest
from unittest import mock


class UpdateUiTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        with mock.patch.dict(sys.modules, {'flet': mock.Mock()}):
            self.gui = importlib.import_module('gui')
        self.app = self.gui.MouseBatteryApp.__new__(self.gui.MouseBatteryApp)
        self.app.page = mock.Mock()
        self.app.page.window.close = mock.AsyncMock()
        self.app._safe_update = mock.Mock()
        self.app._t = lambda key, **values: key
        self.app._effective_language = lambda: 'en-US'
        self.app._make_btn_content = mock.Mock()
        self.app._show_dialog = mock.Mock()

    async def test_check_stays_busy_until_actual_request_finishes(self):
        self.app._check_update_busy = False
        entered, release = threading.Event(), threading.Event()
        button = SimpleNamespace(content='original')
        event = SimpleNamespace(control=button)

        def check(*args):
            entered.set()
            release.wait(3)
            return False, 'v2.6.3', '', '', 0, ''

        with mock.patch.object(self.gui.updater, 'check_for_update', side_effect=check) as checker:
            self.app._on_check_update_click(event)
            worker = asyncio.create_task(self.app.page.run_task.call_args.args[0]())
            try:
                self.assertTrue(await asyncio.to_thread(entered.wait, 1))
                self.app._on_check_update_click(event)
                self.assertEqual(checker.call_count, 1)
                self.assertTrue(self.app._check_update_busy)
            finally:
                release.set()
                await asyncio.wait_for(worker, 2)
        self.assertFalse(self.app._check_update_busy)
        self.assertEqual(button.content, 'original')

    async def test_cancel_download_keeps_window_open_and_allows_retry(self):
        fake_ft = mock.Mock()

        def control(content=None, **values):
            return SimpleNamespace(content=content, disabled=False, open=True, **values)

        fake_ft.TextButton.side_effect = control
        fake_ft.AlertDialog.side_effect = control
        entered = threading.Event()

        def download(*args, **kwargs):
            entered.set()
            cancelled = kwargs['cancel_event'].wait(3)
            if cancelled:
                args[5]('cancelled')
            return False

        with mock.patch.object(self.gui, 'ft', fake_ft), mock.patch.object(self.gui.updater, 'download_and_install', side_effect=download):
            dialog = self.app._show_update_dialog('v2.6.3', 'https://example.test/app.exe', 'notes', 100, 'digest')
            dialog.actions[0].on_click(None)
            worker = asyncio.create_task(self.app.page.run_task.call_args.args[0]())
            self.assertTrue(await asyncio.to_thread(entered.wait, 1))
            dialog.actions[1].on_click(None)
            await asyncio.wait_for(worker, 2)
            self.assertTrue(dialog.open)
            self.assertFalse(dialog.actions[0].disabled)
            self.assertFalse(dialog.actions[1].disabled)
            self.assertEqual(dialog.actions[1].content, 'update.later')
            self.app.page.window.close.assert_not_called()


if __name__ == '__main__':
    unittest.main()

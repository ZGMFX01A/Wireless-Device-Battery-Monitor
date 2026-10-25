"""RTSS 只读检测与 WebView2 故障意图保存测试。"""

import unittest
import os
import json
import subprocess
import sys
import hashlib
import tempfile
import threading
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

from rtss_compat import is_rtss_running, disable_rtss_hooks
from flyout import FlyoutManager


class TestRtssCompat(unittest.TestCase):
    def test_update_helper_restores_original_environment_for_both_restart_routes(self):
        import updater
        content = b'MZ' + b'x' * (1024 * 1024)
        digest = hashlib.sha256(content).hexdigest()
        for original in (None, '1', '0'):
            with self.subTest(original=original), tempfile.TemporaryDirectory() as directory:
                baseline = dict(os.environ)
                for key in list(baseline):
                    if key.casefold() in ('rtsshookscompatibility', 'mouse_battery_rtss_original_env'):
                        del baseline[key]
                if original is not None:
                    baseline['RTSSHooksCompatibility'] = original
                exe = str(Path(directory) / 'MouseBattery-v2.3.5.exe')
                Path(exe).write_bytes(b'original')
                def download(url, target, **kwargs):
                    Path(target).write_bytes(content)
                    return len(content), digest
                with (patch.dict(os.environ, baseline, clear=True),
                      patch.object(updater.sys, 'frozen', True, create=True),
                      patch.object(updater.sys, 'executable', exe),
                      patch.object(updater, '_download_to_path', side_effect=download),
                      patch.object(updater, '_wait_helper_ready'),
                      patch.object(updater, 'request_process_shutdown', return_value=True),
                      patch.object(updater.subprocess, 'Popen') as popen):
                    # 托盘创建GUI、GUI直接入口各设置一次，仍须保留最初的值。
                    disable_rtss_hooks(os.environ)
                    disable_rtss_hooks(os.environ)
                    self.assertTrue(updater.download_and_install(
                        'https://github.com/example/MouseBattery-v2.3.6.exe',
                        expected_size=len(content), expected_digest='sha256:' + digest))
                    helper = popen.call_args.kwargs.get('env', dict(os.environ))
                    values = [v for k, v in helper.items() if k.casefold() == 'rtsshookscompatibility']
                    self.assertEqual(values, [] if original is None else [original])
                    self.assertFalse(any(k.casefold() == 'mouse_battery_rtss_original_env' for k in helper))
                    self.assertEqual(os.environ['RTSSHooksCompatibility'], '0')
                    # 新版启动和旧版回滚均从同一个已清理环境的助手继承。
                    import base64
                    script = base64.b64decode(popen.call_args.args[0][-1]).decode('utf-16le')
                    self.assertIn('[MouseBatteryJob]::Launch($targetPath', script)
                    self.assertIn('Start-Process -FilePath $exePath', script)

    def test_spawn_environment_is_scoped_and_inherited_by_native_children(self):
        from gui_process import GuiProcessController
        # 检查 Win32 实际环境块与下一代子进程，避免只检查 Python 字典。
        native_read = r'''
import ctypes, json
from ctypes import wintypes
k = ctypes.WinDLL('kernel32', use_last_error=True)
k.GetEnvironmentVariableW.argtypes = [wintypes.LPCWSTR, wintypes.LPWSTR, wintypes.DWORD]
k.GetEnvironmentVariableW.restype = wintypes.DWORD
value = ctypes.create_unicode_buffer(32)
k.GetEnvironmentVariableW('RTSSHooksCompatibility', value, len(value))
print(json.dumps(value.value))
'''
        probe = ('import subprocess,sys,json\n' + native_read +
                 '\nchild=subprocess.check_output([sys.executable,"-c",'+repr(native_read)+'],text=True)\n'
                 'print(child.strip())\n')
        captured = []
        def spawn(args, **kwargs):
            values = [value for key, value in kwargs['env'].items()
                      if key.casefold() == 'rtsshookscompatibility']
            self.assertEqual(values, ['0'])
            result = subprocess.run([sys.executable, '-c', probe], env=kwargs['env'],
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            captured.extend(json.loads(line) for line in result.stdout.splitlines())
            return MagicMock()
        controller = GuiProcessController({
            'spawn': spawn, 'executable': sys.executable, 'entry': 'main.py',
            'frozen': True, 'host_pid': os.getpid(), 'creationflags': 0, 'watch_exit': False,
        })
        with patch.dict(os.environ, {'RTSSHooksCompatibility': '1'}):
            controller.open('flyout', {'scale': 1.0, 'rect': [0, 0, 20, 20],
                                       'work_area': [0, 0, 1920, 1080]})
            self.assertEqual(os.environ['RTSSHooksCompatibility'], '1')
        self.assertEqual(captured, ['0', '0'])

    def test_direct_gui_sets_compatibility_before_native_initialization(self):
        import flyout_runtime
        class StopBeforeGui(Exception):
            pass
        observed = []
        def before_native():
            observed.append(os.environ.get('RTSSHooksCompatibility'))
            raise StopBeforeGui()
        with (patch.dict(os.environ, {'RTSSHooksCompatibility': '1'}),
              patch.object(flyout_runtime, 'initialize_dpi_awareness', side_effect=before_native),
              patch.object(flyout_runtime.ctypes.windll.user32, 'MessageBoxW')):
            self.assertEqual(flyout_runtime.run_gui('settings'), 1)
        self.assertEqual(observed, ['0'])

    def test_detects_rtss_case_insensitively(self):
        for name in ('RTSS.exe', 'RTSSHooksLoader64.exe'):
            with patch('gui_focus.snapshot_processes', return_value=[SimpleNamespace(executable=name)]):
                self.assertTrue(is_rtss_running())

    def test_unavailable_process_snapshot_is_not_a_diagnosis(self):
        with patch('gui_focus.snapshot_processes', side_effect=OSError('access denied')):
            self.assertFalse(is_rtss_running())


class TestFlyoutErrorHandling(unittest.TestCase):
    """测试 FlyoutManager 对前后台异常的不同处理策略。"""

    def _create_manager(self):
        shared = MagicMock()
        config = MagicMock()
        options = {
            'closed': threading.Event(),
            'webview': MagicMock(),
            'refresh_tray': MagicMock(),
            'host_factory': MagicMock(),
            'anchor': {'scale': 1.0, 'rect': (0, 0, 100, 100)},
            'mode': 'flyout',
        }
        return FlyoutManager(shared, config, options)

    def test_frontend_error_during_startup_sets_startup_error(self):
        manager = self._create_manager()
        # _ready 未 set，_visible 为 False
        manager._ready.clear()
        manager._visible = False
        with patch.object(manager, '_close'):
            manager.on_frontend_error('ProcessFailed: BrowserProcessExited')
        self.assertIsNotNone(manager.startup_error)
        self.assertIn('BrowserProcessExited', str(manager.startup_error))

    def test_frontend_error_when_visible_sets_startup_error(self):
        manager = self._create_manager()
        manager._ready.set()
        manager._visible = True
        with patch.object(manager, '_close'):
            manager.on_frontend_error('ProcessFailed: BrowserProcessExited')
        self.assertIsNotNone(manager.startup_error)
        self.assertIn('BrowserProcessExited', str(manager.startup_error))

    def test_frontend_error_when_hidden_silently_reclaims(self):
        manager = self._create_manager()
        manager._ready.set()
        manager._host = MagicMock()
        manager._visible = True
        manager._hide_window()
        with patch.object(manager, '_close'):
            manager.on_frontend_error('ProcessFailed: BrowserProcessExited')
        self.assertIsNotNone(manager.startup_error)
        self.assertIsNone(manager.recovery_request)
        self.assertTrue(manager._closing)


if __name__ == '__main__':
    unittest.main()

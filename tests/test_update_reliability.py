import hashlib
import io
import json
import os
import tempfile
import threading
import unittest
from pathlib import Path
from unittest import mock

import config
import updater
import tray


CONTENT = b'MZ' + b'a' * (1024 * 1024)
DIGEST = hashlib.sha256(CONTENT).hexdigest()


class Response(io.BytesIO):
    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()

    def info(self):
        return {'Content-Length': str(len(self.getbuffer()))}


class UpdateReliabilityTests(unittest.TestCase):
    def test_cancellation_at_apply_boundary_keeps_application_running(self):
        cancel = threading.Event()
        with tempfile.TemporaryDirectory() as root:
            exe = str(Path(root) / 'old.exe')
            Path(exe).write_bytes(b'original')

            def download(url, path, **kwargs):
                Path(path).write_bytes(CONTENT)
                return len(CONTENT), DIGEST

            def status(stage, detail=''):
                if stage == 'applying':
                    cancel.set()

            child = mock.Mock()
            child.poll.return_value = None
            with mock.patch.object(updater.sys, 'frozen', True, create=True), mock.patch.object(updater.sys, 'executable', exe), mock.patch.object(updater, '_download_to_path', side_effect=download), mock.patch.object(updater, '_wait_helper_ready'), mock.patch.object(updater.subprocess, 'Popen', return_value=child), mock.patch.object(updater, 'request_process_shutdown') as shutdown:
                self.assertFalse(updater.download_and_install('https://github.com/example/MouseBattery-v2.6.3.exe', expected_size=len(CONTENT), expected_digest='sha256:' + DIGEST, on_status=status, cancel_event=cancel))
                shutdown.assert_not_called()
                child.terminate.assert_called_once()
            self.assertEqual(Path(exe).read_bytes(), b'original')
            self.assertEqual(list(Path(root).iterdir()), [Path(exe)])

    def test_automatic_update_does_not_loop_after_startup_rollback(self):
        app = tray.TrayApp.__new__(tray.TrayApp)
        app._effective_language = lambda: 'zh-CN'
        with mock.patch.object(updater, 'check_for_update', return_value=(True, 'v2.6.3', 'url', '', 100, 'digest')), mock.patch.object(updater, 'read_update_result', return_value={'status': 'failed', 'version': '2.6.3'}), mock.patch.object(updater, 'download_and_install') as download:
            app._auto_update_once()
            download.assert_not_called()

    def test_slow_trickle_cannot_run_without_deadline(self):
        clock = [0.0]

        class Trickle(Response):
            def read1(self, size=-1):
                chunk = super().read(1)
                if chunk:
                    clock[0] += 19
                return chunk

        with (
            tempfile.TemporaryDirectory() as root,
            mock.patch.object(updater, '_urlopen', return_value=Trickle(b'x' * 100)),
            mock.patch.object(updater.time, 'monotonic', side_effect=lambda: clock[0]),
            mock.patch.object(updater, 'DOWNLOAD_SOURCE_TIMEOUT', 15, create=True),
        ):
            with self.assertRaises(TimeoutError):
                updater._download_to_path('https://example.test/app.exe', str(Path(root) / 'download'), expected_size=100)

    def test_dead_helper_does_not_request_application_exit(self):
        with tempfile.TemporaryDirectory() as root:
            exe = str(Path(root) / 'old.exe')
            Path(exe).write_bytes(b'original')

            def download(url, path, **kwargs):
                Path(path).write_bytes(CONTENT)
                return len(CONTENT), DIGEST

            child = mock.Mock()
            child.poll.return_value = 1
            with (
                mock.patch.object(updater.sys, 'frozen', True, create=True),
                mock.patch.object(updater.sys, 'executable', exe),
                mock.patch.object(updater, '_download_to_path', side_effect=download),
                mock.patch.object(updater.subprocess, 'Popen', return_value=child),
                mock.patch.object(updater, 'request_process_shutdown', return_value=True) as shutdown,
            ):
                self.assertFalse(updater.download_and_install(
                    'https://github.com/example/MouseBattery-v2.6.3.exe', expected_size=len(CONTENT), expected_digest='sha256:' + DIGEST,
                ))
                shutdown.assert_not_called()
            self.assertEqual(Path(exe).read_bytes(), b'original')

    def test_second_update_cannot_enter_active_download(self):
        started, release = threading.Event(), threading.Event()
        downloaded_paths = []
        results = []
        with tempfile.TemporaryDirectory() as root:
            exe = str(Path(root) / 'old.exe')
            Path(exe).write_bytes(b'original')

            def download(url, path, **kwargs):
                downloaded_paths.append(path)
                started.set()
                release.wait(timeout=2)
                Path(path).write_bytes(CONTENT)
                return len(CONTENT), DIGEST

            child = mock.Mock()
            child.poll.return_value = 1
            with (
                mock.patch.object(updater.sys, 'frozen', True, create=True),
                mock.patch.object(updater.sys, 'executable', exe),
                mock.patch.object(updater, '_download_to_path', side_effect=download),
                mock.patch.object(updater.subprocess, 'Popen', return_value=child),
                mock.patch.object(updater, 'request_process_shutdown', return_value=True),
            ):
                args = {'expected_size': len(CONTENT), 'expected_digest': 'sha256:' + DIGEST}
                first = threading.Thread(target=lambda: results.append(updater.download_and_install('https://github.com/example/MouseBattery-v2.6.3.exe', **args)))
                first.start()
                try:
                    self.assertTrue(started.wait(timeout=3))
                    self.assertFalse(updater.download_and_install('https://github.com/example/MouseBattery-v2.6.3.exe', **args))
                    self.assertEqual(len(downloaded_paths), 1)
                finally:
                    release.set()
                    first.join(timeout=5)
                self.assertFalse(first.is_alive())

    def test_validation_checks_current_file_bytes(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / 'new.exe'
            path.write_bytes(b'MZ' + b'b' * (1024 * 1024))
            with self.assertRaisesRegex(RuntimeError, 'SHA-256'):
                updater._validate_download(str(path), len(CONTENT), DIGEST, len(CONTENT), DIGEST)

    def test_asset_of_other_version_is_not_an_update(self):
        payload = json.dumps({'tag_name': 'v2.6.3', 'assets': [{
            'name': 'WirelessDeviceBatteryMonitor-v2.6.2.exe',
            'browser_download_url': 'https://example.test/old.exe',
            'size': len(CONTENT), 'digest': 'sha256:' + DIGEST,
        }]}).encode()
        with mock.patch.object(updater, '_urlopen', return_value=Response(payload)):
            result = updater.check_for_update('v2.6.2')
        self.assertFalse(result[0])
        self.assertEqual(result[1], '')

    def test_invalid_tag_is_an_explicit_error(self):
        with mock.patch.object(updater, '_urlopen', return_value=Response(b'{"tag_name":"not-a-version"}')):
            result = updater.check_for_update('v2.6.2')
        self.assertEqual(result[1], '')

    def test_config_initialization_preserves_rollback_copy(self):
        with tempfile.TemporaryDirectory() as root:
            exe = str(Path(root) / 'new.exe')
            backup = Path(exe + '.old')
            backup.write_bytes(b'original')
            with (
                mock.patch.object(updater.sys, 'frozen', True, create=True),
                mock.patch.object(updater.sys, 'executable', exe),
                mock.patch.object(config.ConfigManager, 'load'),
                mock.patch.object(config.ConfigManager, '_refresh_autostart_path_if_needed'),
            ):
                config.ConfigManager()
            self.assertTrue(backup.exists())

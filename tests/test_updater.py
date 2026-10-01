import hashlib
import base64
import io
import json
import os
import subprocess
import tempfile
from pathlib import Path
import unittest
from unittest import mock

import updater


class _Response(io.BytesIO):
    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()

    def info(self):
        return {'Content-Length': str(len(self.getbuffer()))}


class _InterruptedResponse(_Response):
    def __init__(self, value):
        super().__init__(value)
        self._reads = 0

    def read1(self, size=-1):
        self._reads += 1
        if self._reads > 1:
            raise ConnectionResetError('read interrupted')
        return super().read1(2)


class _ExactSizeWithoutEofResponse(_Response):
    def __init__(self, value):
        super().__init__(value)
        self._reads = 0

    def read1(self, size=-1):
        self._reads += 1
        if self._reads > 1:
            raise TimeoutError('server did not close the response')
        return super().read1(size)


class UpdaterTests(unittest.TestCase):
    def test_release_selection_never_installs_setup_as_portable_exe(self):
        assets = [
            {'name': 'MouseBattery-2.6.3-Setup.exe', 'updated_at': '2026-10-01'},
            {'name': 'WirelessDeviceBatteryMonitor-v2.6.3.exe', 'updated_at': '2026-09-30'},
            {'name': 'OtherApp-v2.6.3.exe', 'updated_at': '2026-10-02'},
        ]
        self.assertIs(updater._pick_release_asset(assets, 'v2.6.3'), assets[1])
        self.assertEqual(updater._pick_release_asset([assets[0], assets[2]], 'v2.6.3'), {})

    def test_installer_only_release_is_reported_as_error(self):
        payload = json.dumps({
            'tag_name': 'v2.6.3',
            'assets': [{'name': 'MouseBattery-2.6.3-Setup.exe', 'browser_download_url': 'https://example.test/setup.exe'}],
        }).encode()
        with mock.patch.object(updater, '_urlopen', return_value=_Response(payload)):
            result = updater.check_for_update('2.6.2')
        self.assertFalse(result[0])
        self.assertEqual(result[1], '')
        self.assertIn('版本匹配的单文件 EXE', result[3])

    def test_bilingual_release_notes_follow_interface_language(self):
        payload = json.dumps({
            'tag_name': 'v2.6.3',
            'body': '<!-- lang:zh -->\n中文说明\n\n---\n\n<!-- lang:en -->\nEnglish notes',
            'assets': [{'name': 'WirelessDeviceBatteryMonitor-v2.6.3.exe', 'browser_download_url': 'https://example.test/WirelessDeviceBatteryMonitor-v2.6.3.exe', 'size': 12345, 'digest': 'sha256:' + 'a' * 64}],
        }).encode()
        for language, notes in [('zh-CN', '中文说明'), ('en-US', 'English notes')]:
            with self.subTest(language=language), mock.patch.object(updater, '_urlopen', return_value=_Response(payload)):
                self.assertEqual(updater.check_for_update('2.6.2', language)[3], notes)

    def test_missing_release_language_is_an_explicit_error(self):
        payload = json.dumps({'tag_name': 'v2.6.3', 'body': '<!-- lang:en -->English notes', 'assets': [{'name': 'WirelessDeviceBatteryMonitor-v2.6.3.exe', 'browser_download_url': 'https://example.test/WirelessDeviceBatteryMonitor-v2.6.3.exe', 'size': 12345, 'digest': 'sha256:' + 'a' * 64}]}).encode()
        with mock.patch.object(updater, '_urlopen', return_value=_Response(payload)):
            result = updater.check_for_update('2.6.2', 'zh-CN')
        self.assertEqual(result[1], '')
        self.assertIn('缺少 zh 分段', result[3])

    def test_check_for_update_returns_asset_verification_metadata(self):
        payload = json.dumps({
            'tag_name': 'v2.1.1',
            'body': 'notes',
            'assets': [{
                'name': 'MouseBattery-v2.1.1.exe',
                'browser_download_url': 'https://github.com/example/MouseBattery-v2.1.1.exe',
                'size': 12345,
                'digest': 'sha256:' + 'a' * 64,
                'updated_at': '2026-07-14T00:00:00Z',
            }],
        }).encode()

        with mock.patch.object(updater, '_urlopen', return_value=_Response(payload)):
            result = updater.check_for_update('2.1.0')

        self.assertEqual(
            result,
            (True, 'v2.1.1', 'https://github.com/example/MouseBattery-v2.1.1.exe', 'notes', 12345, 'sha256:' + 'a' * 64),
        )

    def test_official_failure_falls_back_to_verified_mirror(self):
        content = b'MZ' + b'x' * (1024 * 1024)
        digest = hashlib.sha256(content).hexdigest()
        statuses = []

        with tempfile.TemporaryDirectory() as temp_dir:
            exe_path = os.path.join(temp_dir, 'MouseBattery.exe')

            def fake_download(url, target_path, **kwargs):
                if url.startswith('https://github.com/'):
                    raise ConnectionResetError('official reset')
                with open(target_path, 'wb') as output:
                    output.write(content)
                return len(content), digest

            with (
                mock.patch.object(updater.sys, 'frozen', True, create=True),
                mock.patch.object(updater.sys, 'executable', exe_path),
                mock.patch.object(updater, '_download_to_path', side_effect=fake_download) as download,
                mock.patch.object(updater, 'request_process_shutdown', return_value=True),
                mock.patch.object(updater, '_wait_helper_ready'),
                mock.patch.object(updater.subprocess, 'Popen') as popen,
            ):
                success = updater.download_and_install(
                    'https://github.com/example/MouseBattery-v2.6.3.exe',
                    expected_size=len(content),
                    expected_digest='sha256:' + digest,
                    on_status=lambda stage, detail='': statuses.append((stage, detail)),
                )

            self.assertTrue(success)
            self.assertEqual(download.call_count, 2)
            self.assertEqual(download.call_args_list[1].args[0], updater.DOWNLOAD_MIRROR_PREFIX + 'https://github.com/example/MouseBattery-v2.6.3.exe')
            self.assertIn(('fallback', 'official reset'), statuses)
            self.assertIn(('verifying', ''), statuses)
            popen.assert_called_once()

    def test_truncated_official_download_falls_back_to_mirror(self):
        content = b'MZ' + b'x' * (1024 * 1024)
        digest = hashlib.sha256(content).hexdigest()
        statuses = []

        with tempfile.TemporaryDirectory() as temp_dir:
            exe_path = os.path.join(temp_dir, 'MouseBattery.exe')

            def fake_download(url, target_path, **kwargs):
                downloaded = b'MZ' if url.startswith('https://github.com/') else content
                with open(target_path, 'wb') as output:
                    output.write(downloaded)
                return len(downloaded), hashlib.sha256(downloaded).hexdigest()

            with (
                mock.patch.object(updater.sys, 'frozen', True, create=True),
                mock.patch.object(updater.sys, 'executable', exe_path),
                mock.patch.object(updater, '_download_to_path', side_effect=fake_download) as download,
                mock.patch.object(updater, 'request_process_shutdown', return_value=True),
                mock.patch.object(updater, '_wait_helper_ready'),
                mock.patch.object(updater.subprocess, 'Popen') as popen,
            ):
                success = updater.download_and_install(
                    'https://github.com/example/MouseBattery-v2.6.3.exe',
                    expected_size=len(content),
                    expected_digest='sha256:' + digest,
                    on_status=lambda stage, detail='': statuses.append((stage, detail)),
                )

            self.assertTrue(success)
            self.assertEqual(download.call_count, 2)
            self.assertTrue(any(stage == 'fallback' and '过小' in detail for stage, detail in statuses))

    def test_hash_mismatch_rejects_update_and_removes_new_file(self):
        content = b'MZ' + b'x' * (1024 * 1024)
        statuses = []

        with tempfile.TemporaryDirectory() as temp_dir:
            exe_path = os.path.join(temp_dir, 'MouseBattery.exe')

            def fake_download(url, target_path, **kwargs):
                with open(target_path, 'wb') as output:
                    output.write(content)
                return len(content), '0' * 64

            with (
                mock.patch.object(updater.sys, 'frozen', True, create=True),
                mock.patch.object(updater.sys, 'executable', exe_path),
                mock.patch.object(updater, '_download_to_path', side_effect=fake_download),
                mock.patch.object(updater, '_wait_helper_ready'),
                mock.patch.object(updater.subprocess, 'Popen') as popen,
            ):
                success = updater.download_and_install(
                    'https://github.com/example/MouseBattery-v2.6.3.exe',
                    expected_size=len(content),
                    expected_digest='sha256:' + 'a' * 64,
                    on_status=lambda stage, detail='': statuses.append((stage, detail)),
                )

            self.assertFalse(success)
            self.assertFalse(os.path.exists(os.path.join(temp_dir, 'app.exe.new')))
            self.assertEqual(statuses[-1][0], 'error')
            popen.assert_not_called()

    def test_missing_official_digest_rejects_update_before_download(self):
        statuses = []
        with tempfile.TemporaryDirectory() as temp_dir:
            exe_path = os.path.join(temp_dir, 'MouseBattery.exe')
            with (
                mock.patch.object(updater.sys, 'frozen', True, create=True),
                mock.patch.object(updater.sys, 'executable', exe_path),
                mock.patch.object(updater, '_download_to_path') as download,
            ):
                success = updater.download_and_install(
                    'https://github.com/example/MouseBattery-v2.6.3.exe',
                    expected_size=1024 * 1024,
                    expected_digest='',
                    on_status=lambda stage, detail='': statuses.append((stage, detail)),
                )

        self.assertFalse(success)
        self.assertEqual(statuses[-1][0], 'error')
        download.assert_not_called()

    def test_download_retry_restarts_after_interrupted_read(self):
        content = b'MZ-update-content'
        retries = []
        with tempfile.TemporaryDirectory() as temp_dir:
            target = os.path.join(temp_dir, 'update.new')
            with mock.patch.object(
                updater,
                '_urlopen',
                side_effect=[_InterruptedResponse(content), _Response(content)],
            ):
                size, digest = updater._download_to_path(
                    'https://mirror.example/app.exe',
                    target,
                    expected_size=len(content),
                    retries=1,
                    on_retry=lambda attempt, total, error: retries.append((attempt, total)),
                )

            self.assertEqual((size, digest), (len(content), hashlib.sha256(content).hexdigest()))
            self.assertEqual(retries, [(1, 1)])
            with open(target, 'rb') as downloaded:
                self.assertEqual(downloaded.read(), content)

    def test_download_stops_at_expected_size_without_waiting_for_eof(self):
        content = b'MZ-update-content'
        response = _ExactSizeWithoutEofResponse(content)
        with tempfile.TemporaryDirectory() as temp_dir:
            target = os.path.join(temp_dir, 'update.new')
            with mock.patch.object(updater, '_urlopen', return_value=response):
                size, digest = updater._download_to_path(
                    'https://example.test/app.exe',
                    target,
                    expected_size=len(content),
                )

        self.assertEqual(response._reads, 1)
        self.assertEqual((size, digest), (len(content), hashlib.sha256(content).hexdigest()))

    def test_gui_update_uses_release_filename_and_bootloader_parent_pid(self):
        content = b'MZ' + b'x' * (1024 * 1024)
        digest = hashlib.sha256(content).hexdigest()

        with tempfile.TemporaryDirectory() as temp_dir:
            exe_path = os.path.join(temp_dir, 'MouseBattery-v2.3.5.exe')
            downloaded_paths = []

            def fake_download(url, target_path, **kwargs):
                downloaded_paths.append(target_path)
                with open(target_path, 'wb') as output:
                    output.write(content)
                return len(content), digest

            with (
                mock.patch.object(updater.sys, 'frozen', True, create=True),
                mock.patch.object(updater.sys, 'executable', exe_path),
                mock.patch.object(updater.os, 'getpid', return_value=456),
                mock.patch.object(updater.os, 'getppid', return_value=321),
                mock.patch.object(updater, '_download_to_path', side_effect=fake_download),
                mock.patch.object(updater, 'request_process_shutdown', return_value=True) as shutdown,
                mock.patch.object(updater, '_wait_helper_ready'),
                mock.patch.object(updater.subprocess, 'Popen') as popen,
            ):
                success = updater.download_and_install(
                    'https://github.com/example/WirelessDeviceBatteryMonitor-v2.3.6.exe',
                    host_pid=123,
                    expected_size=len(content),
                    expected_digest='sha256:' + digest,
                )

            target_path = os.path.join(temp_dir, 'WirelessDeviceBatteryMonitor-v2.3.6.exe')
            self.assertTrue(success)
            self.assertEqual(len(downloaded_paths), 1)
            self.assertEqual(Path(downloaded_paths[0]).name, 'asset.partial')
            self.assertEqual(Path(downloaded_paths[0]).parent.parent, Path(temp_dir))
            shutdown.assert_called_once()
            self.assertEqual(shutdown.call_args.args, (123,))
            self.assertEqual(shutdown.call_args.kwargs['skip_gui_pid'], 321)
            self.assertRegex(shutdown.call_args.kwargs['update_token'], r'^[0-9a-f]{32}$')
            launched = popen.call_args.args[0]
            self.assertEqual(launched[:6], [
                'powershell.exe', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-EncodedCommand',
            ])
            script_text = base64.b64decode(launched[6]).decode('utf-16le')
            self.assertIn(f"$targetPath = '{target_path}'", script_text)
            self.assertIn('[MouseBatteryJob]::Launch($targetPath', script_text)

    def test_powershell_update_script_preserves_unicode_paths(self):
        script = updater._build_swap_powershell_script(
            exe_path=r'C:\用户\电量工具\旧版.exe',
            target_exe_path=r'C:\用户\电量工具\新版.exe',
            old_exe_path=r'C:\用户\电量工具\新版.exe.old',
            new_exe_path=r'C:\用户\电量工具\新版.exe.new',
            target_pid=123,
            expected_size=456,
            expected_sha256='a' * 64, token='b' * 32, semaphore_name='test', target_version='2.6.3',
        )
        encoded = updater._encode_powershell_command(script)
        self.assertEqual(base64.b64decode(encoded).decode('utf-16le'), script)
        self.assertIn("$targetPath = 'C:\\用户\\电量工具\\新版.exe'", script)

    def test_rejects_unsafe_or_existing_release_target_before_download(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            exe_path = os.path.join(temp_dir, 'MouseBattery-v2.3.5.exe')
            existing_target = os.path.join(temp_dir, 'WirelessDeviceBatteryMonitor-v2.3.6.exe')
            with open(existing_target, 'wb') as target:
                target.write(b'existing')

            with (
                mock.patch.object(updater.sys, 'frozen', True, create=True),
                mock.patch.object(updater.sys, 'executable', exe_path),
                mock.patch.object(updater, '_download_to_path') as download,
            ):
                unsafe = updater.download_and_install(
                    'https://github.com/example/%25TEMP%25.exe',
                    expected_size=1024 * 1024,
                    expected_digest='sha256:' + 'a' * 64,
                )
                conflict = updater.download_and_install(
                    'https://github.com/example/WirelessDeviceBatteryMonitor-v2.3.6.exe',
                    expected_size=1024 * 1024,
                    expected_digest='sha256:' + 'a' * 64,
                )

            self.assertFalse(unsafe)
            self.assertFalse(conflict)
            download.assert_not_called()

if __name__ == '__main__':
    unittest.main()

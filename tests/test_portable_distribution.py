import subprocess
import os
import re
import shutil
import sys
import textwrap
import unittest
from pathlib import Path
from unittest import mock

import build


@unittest.skipUnless(os.name == 'nt', 'Windows GUI process semantics')
class ReleaseSmokeProcessTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.shell = shutil.which('pwsh') or shutil.which('powershell')
        cls.pythonw = Path(sys.executable).with_name('pythonw.exe')
        if not cls.shell or not cls.pythonw.is_file():
            raise unittest.SkipTest('PowerShell and pythonw are required')

    def run_workflow_smoke(self, exit_code):
        root = Path(__file__).resolve().parents[1]
        workflow = (root / '.github/workflows/release.yml').read_text(encoding='utf-8')
        match = re.search(
            r'      - name: Smoke test frozen executable\n'
            r'(?:        [^\n]*\n)*?        run: \|\n'
            r'(?P<script>(?:          [^\n]*\n|\n)+)', workflow,
        )
        self.assertIsNotNone(match, 'Release smoke step must exist')
        script = textwrap.dedent(match.group('script'))
        pythonw_literal = "'" + str(self.pythonw).replace("'", "''") + "'"
        script = script.replace('"./dist/WirelessDeviceBatteryMonitor.exe"', pythonw_literal)
        gui_code = f"__import__('time').sleep(0.4);__import__('sys').exit({exit_code})"
        script = script.replace("@('--smoke-test')", f'@(\'-c\', "{gui_code}")')
        command = (
            "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); "
            "$ErrorActionPreference = 'Stop'; $global:LASTEXITCODE = 55; "
            "$taskWatch = [System.Diagnostics.Stopwatch]::StartNew(); try {\n"
            + script
            + "\n} finally { Write-Output ('SMOKE_ELAPSED_MS=' + $taskWatch.Elapsed.TotalMilliseconds) }; exit 0"
        )
        result = subprocess.run(
            [self.shell, '-NoProfile', '-NonInteractive', '-Command', command],
            capture_output=True, text=True, encoding='utf-8', timeout=15, cwd=root,
        )
        elapsed = re.search(r'SMOKE_ELAPSED_MS=([0-9.]+)', result.stdout)
        self.assertIsNotNone(elapsed, result.stdout + result.stderr)
        return result, float(elapsed.group(1))

    def test_gui_success_waits_and_ignores_stale_shell_exit_code(self):
        result, elapsed = self.run_workflow_smoke(0)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertGreaterEqual(elapsed, 350)

    def test_gui_failure_reports_its_actual_exit_code_after_waiting(self):
        result, elapsed = self.run_workflow_smoke(7)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('exit code 7', result.stderr)
        self.assertGreaterEqual(elapsed, 350)


class PortableDistributionTests(unittest.TestCase):
    def test_standard_build_produces_onefile_without_updater_runtime(self):
        # 在替换 subprocess.run 之前加载依赖，避免影响依赖的平台探测。
        import PyInstaller  # noqa: F401
        import flet  # noqa: F401
        import flet_desktop  # noqa: F401

        with (
            mock.patch.object(build, 'ensure_private_core_available', return_value=None),
            mock.patch.object(build.os.path, 'isfile', return_value=True),
            mock.patch.object(build.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)) as run,
        ):
            self.assertEqual(build.build(), 0)

        command = run.call_args.args[0]
        self.assertIn('--onefile', command)
        self.assertNotIn('--onedir', command)
        self.assertNotIn('velopack', command)
        self.assertTrue(any(arg.endswith(';flet_desktop') for arg in command))

    def test_release_workflow_publishes_the_versioned_portable_executable(self):
        workflow = (Path(__file__).resolve().parents[1] / '.github/workflows/release.yml').read_text(encoding='utf-8')
        self.assertTrue('./dist/WirelessDeviceBatteryMonitor.exe' in workflow, 'Release must smoke-test the single EXE')
        self.assertIn('WirelessDeviceBatteryMonitor-$env:RELEASE_TAG.exe', workflow)
        self.assertIn('--clobber', workflow)
        self.assertNotIn('vpk', workflow)
        self.assertNotIn('setup-dotnet', workflow)


if __name__ == '__main__':
    unittest.main()

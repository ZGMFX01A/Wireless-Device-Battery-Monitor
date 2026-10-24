import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


SHELL = shutil.which('pwsh') or shutil.which('powershell')
SCRIPT = Path(__file__).resolve().parents[1] / 'build-local.ps1'


@unittest.skipUnless(os.name == 'nt' and SHELL, 'Windows PowerShell is required')
class LocalBuildTests(unittest.TestCase):
    def run_build(self, failed_step='', smoke_exit=0, skip_smoke=False):
        # Stub command boundaries: no dependency installs, real builds or hardware access.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shutil.copyfile(SCRIPT, root / SCRIPT.name)
            (root / '.private-requirements.txt').touch()
            (root / 'dist').mkdir()
            (root / 'dist/WirelessDeviceBatteryMonitor.exe').touch()
            command = r'''
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$ErrorActionPreference = 'Stop'
function python {
    $step = $args -join ' '
    Write-Host "STEP $step"
    $global:LASTEXITCODE = if ($step -eq $env:FAILED_STEP) { 7 } else { 0 }
}
function pip { python -m pip @args }
function Start-Process {
    param($FilePath, $ArgumentList, [switch]$Wait, [switch]$PassThru, $WindowStyle)
    if (-not $Wait -or -not $PassThru -or $WindowStyle -ne 'Hidden') {
        throw 'Smoke must wait for its own hidden process and read its exit code'
    }
    Write-Host 'SMOKE'
    [pscustomobject]@{ ExitCode = [int]$env:SMOKE_EXIT }
}
& './build-local.ps1' SKIP_FLAG
'''.replace('SKIP_FLAG', '-SkipSmokeTest' if skip_smoke else '')
            return subprocess.run([SHELL, '-NoProfile', '-NonInteractive', '-Command', command],
                                  cwd=root, env={**os.environ, 'FAILED_STEP': failed_step,
                                                'SMOKE_EXIT': str(smoke_exit)},
                                  capture_output=True, text=True, encoding='utf-8', timeout=20)

    def test_dependency_failure_stops_before_build(self):
        result = self.run_build(failed_step='-m pip install -r requirements.txt')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('exit code 7', result.stderr)
        self.assertNotIn('STEP build.py', result.stdout)
        self.assertNotIn('Local build completed.', result.stdout)

    def test_failed_build_does_not_smoke_test_a_stale_executable(self):
        result = self.run_build(failed_step='build.py')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('exit code 7', result.stderr)
        self.assertNotIn('SMOKE', result.stdout)
        self.assertNotIn('Local build completed.', result.stdout)

    def test_smoke_failure_uses_the_gui_process_exit_code(self):
        result = self.run_build(smoke_exit=7)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('exit code 7', result.stderr)
        self.assertNotIn('Local build completed.', result.stdout)

    def test_success_waits_for_smoke_and_reports_completion(self):
        result = self.run_build()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('SMOKE', result.stdout)
        self.assertIn('Local build completed.', result.stdout)

    def test_skip_smoke_still_requires_a_successful_build(self):
        result = self.run_build(skip_smoke=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn('SMOKE', result.stdout)
        self.assertIn('Local build completed.', result.stdout)


if __name__ == '__main__':
    unittest.main()

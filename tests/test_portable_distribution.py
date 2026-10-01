import subprocess
import unittest
from pathlib import Path
from unittest import mock

import build


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

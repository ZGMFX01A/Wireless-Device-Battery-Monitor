import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
import uuid
from unittest import mock

import update_transaction as transaction


@unittest.skipUnless(os.name == 'nt', 'Windows 更新事务')
class TransactionTests(unittest.TestCase):
    def test_semaphore_excludes_another_process(self):
        with tempfile.TemporaryDirectory() as root:
            exe = str(Path(root) / 'app.exe')
            code = ('import sys; from update_transaction import UpdateLease, UpdateBusy; '
                    '\ntry:\n with UpdateLease(sys.argv[1]): sys.exit(0)'
                    '\nexcept UpdateBusy: sys.exit(23)')
            with transaction.UpdateLease(exe):
                busy = subprocess.run([sys.executable, '-c', code, exe], timeout=5)
                self.assertEqual(busy.returncode, 23)
            free = subprocess.run([sys.executable, '-c', code, exe], timeout=5)
            self.assertEqual(free.returncode, 0)

    def run_swap(self, root, failure):
        root = Path(root)
        exe, target = root / '旧版.exe', root / '新版.exe'
        exe.write_bytes(b'original')
        token = uuid.uuid4().hex
        directory = transaction.job_directory(exe, token)
        directory.mkdir()
        new, backup = directory / 'asset.partial', directory / 'previous.exe'
        content = b'new-content'
        new.write_bytes(content)
        if failure == 'conflict':
            target.write_bytes(b'unrelated')
        if failure == 'hash':
            new.write_bytes(b'bad-content')  # 同样长度
        transaction.write_atomic(directory / 'manifest.json', dict(token=token, target_path=str(target), version='2.6.3'))
        child_script = directory / 'test_child.py'
        child_script.write_text(
            "import json, os, sys, time, subprocess\nfrom pathlib import Path\n"
            "if sys.argv[1] == 'success':\n"
            " p=Path(sys.argv[2]); tmp=p.with_suffix('.tmp'); tmp.write_text(json.dumps(dict(token=os.environ['MOUSE_BATTERY_UPDATE_TOKEN'], version='2.6.3'))); tmp.replace(p)\n"
            "if sys.argv[1] == 'tree':\n"
            " child=subprocess.Popen([sys.executable, '-c', \"import time; time.sleep(30)\"], creationflags=subprocess.CREATE_NO_WINDOW)\n"
            " Path(sys.argv[3]).write_text(str(os.getpid()) + '\\n' + str(child.pid) + '\\n')\n"
            " child.wait()\n"
            "if sys.argv[1] == 'exit': sys.exit(7)\n"
            "time.sleep(30)\n", encoding='utf-8')
        child_pids = root / 'children.txt'
        mode = 'exit' if failure == 'exit' else 'timeout' if failure == 'timeout' else 'tree' if failure == 'tree' else 'success'
        # 只替代启动目标，启动实际可监视的无副作用 Python 子进程。
        # 移动、校验、信号量、确认、回滚均使用真实 Windows 实现。
        prefix = '\n'.join([
            f'$script:original = {transaction._powershell_literal(exe)}',
            f'$script:download = {transaction._powershell_literal(new)}',
            f'$script:python = {transaction._powershell_literal(sys._base_executable)}',
            f'$script:child = {transaction._powershell_literal(child_script)}',
            f'$script:ack = {transaction._powershell_literal(directory / "startup.ready")}',
            f'$script:pids = {transaction._powershell_literal(child_pids)}',
            f"$script:failure = '{failure}'; $script:mode = '{mode}'; $script:firstMoves = 0",
            'function Start-Process { param($FilePath, [switch]$PassThru)',
            "  [IO.File]::WriteAllText((Join-Path ([IO.Path]::GetDirectoryName($FilePath)) 'restart.txt'), $FilePath)",
            '}',
            'function Move-Item { param($LiteralPath, $Destination, [switch]$Force)',
            '  if ($LiteralPath -eq $script:original) {',
            '    $script:firstMoves++',
            "    if ($script:failure -eq 'first' -or ($script:failure -eq 'transient' -and $script:firstMoves -eq 1)) { throw 'injected original move failure' }",
            '  }',
            "  if ($LiteralPath -eq $script:download -and $script:failure -eq 'second') { throw 'injected install failure' }",
            "  if ($LiteralPath -eq $script:download -and $script:failure -eq 'latehash') { [IO.File]::WriteAllText($LiteralPath, 'bad-content') }",
            '  Microsoft.PowerShell.Management\\Move-Item -LiteralPath $LiteralPath -Destination $Destination -Force:$Force',
            '}',
        ])
        try:
            with transaction.UpdateLease(str(exe)) as lease:
                script = transaction._build_swap_powershell_script(str(exe), str(target), str(backup), str(new),
                    2147483647, len(content), expected_sha256=hashlib.sha256(content).hexdigest(),
                    token=token, semaphore_name=lease.name, target_version='2.6.3',
                    startup_timeout=1.5, stability_seconds=0.2)
                # 改变测试启动目标和参数，仍调用产品的暂停创建、Job 归属、恢复线程实现。
                command = f'"{sys._base_executable}" "{child_script}" {mode} "{directory / "startup.ready"}" "{child_pids}"'
                script = script.replace("[MouseBatteryJob]::Launch($targetPath, ('\"' + $targetPath + '\"'), $newJob)",
                    '[MouseBatteryJob]::Launch(' + transaction._powershell_literal(sys._base_executable) + ', ' + transaction._powershell_literal(command) + ', $newJob)')
                child = subprocess.Popen(['powershell.exe', '-NoProfile', '-NonInteractive', '-EncodedCommand',
                    transaction._encode_powershell_command(prefix + '\n' + script)],
                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, creationflags=subprocess.CREATE_NO_WINDOW)
                try:
                    deadline = time.monotonic() + 8
                    while not (directory / 'helper.ready').exists() and child.poll() is None:
                        if time.monotonic() > deadline:
                            self.fail('替换程序没有确认就绪')
                        time.sleep(0.05)
                    if child.poll() is None:
                        transaction.write_atomic(directory / 'apply.request', dict(token=token, target_pid=2147483647))
                        lease.handoff()
                        if failure in ('timeout', 'tree'):
                            lease.__exit__()  # 模拟调用方退出，仅 helper 保持信号量句柄
                            with self.assertRaises(transaction.UpdateBusy):
                                with transaction.UpdateLease(str(exe)):
                                    pass
                    output, errors = child.communicate(timeout=20)
                finally:
                    if child.poll() is None:
                        child.kill()
                        child.communicate(timeout=5)
            result = transaction.read_update_result(exe)
            self.assertEqual(result.get('status'), 'success' if failure in ('success', 'transient') else 'failed', errors)
            if failure in ('success', 'transient'):
                self.assertEqual(child.returncode, 0, errors)
                self.assertEqual(target.read_bytes(), content)
                self.assertFalse(exe.exists())
            else:
                self.assertNotEqual(child.returncode, 0, output)
                self.assertEqual(exe.read_bytes(), b'original')
                if failure == 'conflict':
                    self.assertEqual(target.read_bytes(), b'unrelated')
                else:
                    self.assertFalse(target.exists())
            if failure == 'tree':
                self.assertTrue(child_pids.exists(), result)
                # 启动确认超时以后，父、子进程必须全部退出，不能遗留托盘实例。
                for pid in child_pids.read_text().splitlines():
                    self.assertEqual(transaction.process_start_ticks(int(pid)), 0, result)
            if failure not in ('success', 'transient', 'hash'):
                self.assertEqual((root / 'restart.txt').read_text(encoding='utf-8'), str(exe))
            self.assertFalse(directory.exists(), errors)
            with transaction.UpdateLease(str(exe)):
                pass  # 事务退出后必须释放更新权
        finally:
            if child_pids.exists():
                for pid in child_pids.read_text().splitlines():
                    subprocess.run(['powershell.exe', '-NoProfile', '-NonInteractive', '-Command', f'Stop-Process -Id {int(pid)} -Force -ErrorAction SilentlyContinue'], capture_output=True, timeout=5, creationflags=subprocess.CREATE_NO_WINDOW)

    def test_real_swap_success_and_failures(self):
        for failure in ('success', 'first', 'second', 'transient', 'conflict', 'hash', 'latehash', 'exit', 'timeout', 'tree'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory(prefix='电量更新-') as root:
                self.run_swap(root, failure)

    def test_shutdown_request_requires_live_transaction(self):
        with tempfile.TemporaryDirectory() as root:
            exe = str(Path(root) / 'app.exe')
            token = uuid.uuid4().hex
            path = Path(root) / 'shutdown.json'
            with mock.patch.object(transaction, 'get_shutdown_request_path', return_value=path), mock.patch.object(sys, 'executable', exe):
                self.assertTrue(transaction.request_process_shutdown(os.getpid(), update_token=token))
                self.assertIsNone(transaction.consume_shutdown_request(os.getpid()))
                directory = transaction.job_directory(exe, token)
                directory.mkdir()
                transaction.write_atomic(directory / 'apply.request', dict(token=token, target_pid=os.getpid()))
                self.assertTrue(transaction.request_process_shutdown(os.getpid(), update_token=token))
                self.assertEqual(transaction.consume_shutdown_request(os.getpid())['token'], token)
                self.assertIsNone(transaction.consume_shutdown_request(os.getpid()))

    def test_startup_ack_checks_actual_version_and_executable(self):
        with tempfile.TemporaryDirectory() as root:
            exe = str(Path(root) / 'MouseBattery-v2.6.3.exe')
            token = uuid.uuid4().hex
            directory = transaction.job_directory(exe, token)
            directory.mkdir()
            transaction.write_atomic(directory / 'manifest.json', dict(token=token, target_path=exe, version='2.6.3'))
            with mock.patch.object(sys, 'executable', exe), mock.patch.object(transaction, '_startup_token', token):
                with self.assertRaises(RuntimeError):
                    transaction.confirm_update_startup('2.6.2')
                self.assertFalse((directory / 'startup.ready').exists())
                transaction.confirm_update_startup('v2.6.3')
                self.assertEqual(json.loads((directory / 'startup.ready').read_text())['token'], token)


if __name__ == '__main__':
    unittest.main()

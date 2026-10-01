"""Windows 单 EXE 更新事务：跨进程互斥、交接、启动确认和回滚。"""

import base64
import ctypes
import hashlib
import json
import logging
import os
from pathlib import Path
import re
import shutil
import tempfile
import time
import uuid

logger = logging.getLogger(__name__)
TOKEN_ENV = 'MOUSE_BATTERY_UPDATE_TOKEN'
RESULT_NAME = '.mouse-battery-update-result.json'
SHUTDOWN_REQUEST_PREFIX = 'mouse_battery_shutdown'
_startup_token = ''


class UpdateBusy(RuntimeError):
    pass


def _semaphore_name(exe_path):
    directory = os.path.normcase(str(Path(exe_path).resolve().parent))
    digest = hashlib.sha256(directory.encode('utf-8')).hexdigest()[:32]
    return 'Global\\MouseBatteryUpdate_' + digest


class UpdateLease:
    """信号量计数可跨进程交接；helper 先打开句柄再确认接管。"""

    def __init__(self, exe_path):
        if os.name != 'nt':
            raise RuntimeError('自动替换仅支持 Windows')
        self.name = _semaphore_name(exe_path)
        self.kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        self.kernel.CreateSemaphoreW.argtypes = [ctypes.c_void_p, ctypes.c_long, ctypes.c_long, ctypes.c_wchar_p]
        self.kernel.CreateSemaphoreW.restype = ctypes.c_void_p
        self.kernel.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
        self.kernel.WaitForSingleObject.restype = ctypes.c_ulong
        self.kernel.ReleaseSemaphore.argtypes = [ctypes.c_void_p, ctypes.c_long, ctypes.c_void_p]
        self.kernel.ReleaseSemaphore.restype = ctypes.c_bool
        self.kernel.CloseHandle.argtypes = [ctypes.c_void_p]
        self.kernel.CloseHandle.restype = ctypes.c_bool
        self.handle = None
        self.transferred = False

    def __enter__(self):
        self.handle = self.kernel.CreateSemaphoreW(None, 1, 1, self.name)
        if not self.handle:
            raise ctypes.WinError(ctypes.get_last_error())
        wait = self.kernel.WaitForSingleObject(self.handle, 0)
        if wait != 0:
            self.kernel.CloseHandle(self.handle)
            self.handle = None
            if wait == 258:
                raise UpdateBusy('已有更新正在下载或安装，请等待完成')
            raise ctypes.WinError(ctypes.get_last_error())
        return self

    def handoff(self):
        self.transferred = True

    def __exit__(self, *args):
        if self.handle:
            if not self.transferred:
                if not self.kernel.ReleaseSemaphore(self.handle, 1, None):
                    logger.error('释放更新互斥失败: %s', ctypes.get_last_error())
            self.kernel.CloseHandle(self.handle)
            self.handle = None


def process_start_ticks(pid):
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.OpenProcess.argtypes = [ctypes.c_ulong, ctypes.c_bool, ctypes.c_ulong]
    kernel.OpenProcess.restype = ctypes.c_void_p
    kernel.GetProcessTimes.argtypes = [ctypes.c_void_p] + [ctypes.POINTER(ctypes.c_ulonglong)] * 4
    kernel.GetProcessTimes.restype = ctypes.c_bool
    kernel.CloseHandle.argtypes = [ctypes.c_void_p]
    handle = kernel.OpenProcess(0x1000, False, pid)
    if not handle:
        return 0
    try:
        values = [ctypes.c_ulonglong() for _ in range(4)]
        if not kernel.GetProcessTimes(handle, *(ctypes.byref(value) for value in values)):
            return 0
        return values[0].value
    finally:
        kernel.CloseHandle(handle)


def job_directory(exe_path, token):
    if not re.fullmatch(r'[0-9a-f]{32}', token):
        raise ValueError('更新事务 token 非法')
    return Path(exe_path).resolve().parent / ('.mouse-battery-update-' + token)


def write_atomic(path, payload):
    path = Path(path)
    temporary = path.with_name(path.name + '.tmp-' + uuid.uuid4().hex)
    try:
        with temporary.open('w', encoding='utf-8') as output:
            json.dump(payload, output, ensure_ascii=False)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        if temporary.exists():
            temporary.unlink()


def cleanup_job(exe_path, token):
    directory = job_directory(exe_path, token)
    if directory.exists():
        if directory.resolve().parent != Path(exe_path).resolve().parent or directory.is_symlink():
            raise RuntimeError('拒绝清理超出应用目录的更新事务')
        shutil.rmtree(directory)


def get_shutdown_request_path(pid):
    return Path(tempfile.gettempdir()) / f'{SHUTDOWN_REQUEST_PREFIX}_{pid}.json'


def request_process_shutdown(target_pid, reason='update', skip_gui_pid=None, *, update_token=''):
    if not isinstance(target_pid, int) or target_pid <= 0 or not re.fullmatch(r'[0-9a-f]{32}', update_token):
        logger.error('更新退出请求缺少有效 PID 或事务 token')
        return False
    payload = dict(reason=reason, target_pid=target_pid, requester_pid=os.getpid(),
                   requested_at=time.time(), token=update_token,
                   target_start_ticks=process_start_ticks(target_pid))
    if isinstance(skip_gui_pid, int) and skip_gui_pid > 0:
        payload['skip_gui_pid'] = skip_gui_pid
    try:
        write_atomic(get_shutdown_request_path(target_pid), payload)
        return True
    except Exception:
        logger.exception('无法写入更新退出请求')
        return False


def consume_shutdown_request(current_pid):
    import sys
    path = get_shutdown_request_path(current_pid)
    if not path.exists():
        return None
    try:
        payload = json.loads(path.read_text(encoding='utf-8'))
        token = payload.get('token', '')
        age = time.time() - float(payload['requested_at'])
        if payload.get('reason') != 'update' or payload.get('target_pid') != current_pid or not 0 <= age <= 60:
            raise ValueError('过期或目标不匹配的退出请求')
        apply = json.loads((job_directory(sys.executable, token) / 'apply.request').read_text(encoding='utf-8'))
        if apply.get('token') != token or apply.get('target_pid') != current_pid:
            raise ValueError('退出请求未绑定已交接的更新事务')
        expected_ticks = payload.get('target_start_ticks', 0)
        if expected_ticks and expected_ticks != process_start_ticks(current_pid):
            raise ValueError('退出请求的进程实例已变化')
        return payload
    except Exception as error:
        logger.warning('忽略无效更新退出请求: %s', error)
        return None
    finally:
        try:
            path.unlink(missing_ok=True)
        except OSError:
            logger.exception('清理退出请求失败')


def prepare_update_startup():
    global _startup_token
    _startup_token = os.environ.pop(TOKEN_ENV, '')
    if _startup_token and not re.fullmatch(r'[0-9a-f]{32}', _startup_token):
        raise ValueError('启动更新 token 非法')


def confirm_update_startup(version):
    import sys
    global _startup_token
    if not _startup_token:
        return
    directory = job_directory(sys.executable, _startup_token)
    manifest = json.loads((directory / 'manifest.json').read_text(encoding='utf-8'))
    actual_version = str(version).strip().lower().removeprefix('v')
    if (manifest.get('token') != _startup_token
            or os.path.normcase(os.path.abspath(manifest['target_path'])) != os.path.normcase(os.path.abspath(sys.executable))
            or manifest.get('version') != actual_version):
        raise RuntimeError('新版本启动信息与更新目标不一致')
    write_atomic(directory / 'startup.ready', dict(token=_startup_token, pid=os.getpid(), version=actual_version))
    _startup_token = ''


def read_update_result(exe_path):
    path = Path(exe_path).resolve().parent / RESULT_NAME
    try:
        with path.open(encoding='utf-8-sig') as source:
            text = source.read(65537)
        if len(text) > 65536:
            raise ValueError('更新结果记录过大')
        result = json.loads(text)
        return result if isinstance(result, dict) else {}
    except FileNotFoundError:
        return {}
    except Exception:
        logger.exception('读取更新结果失败')
        return {}


def _powershell_literal(value):
    return "'" + str(value).replace("'", "''") + "'"


def _encode_powershell_command(script):
    return base64.b64encode(script.encode('utf-16le')).decode('ascii')


def _build_swap_powershell_script(exe_path, target_exe_path, old_exe_path, new_exe_path,
                                  target_pid, expected_size, *, expected_sha256,
                                  token, semaphore_name, target_version, target_start_ticks=0,
                                  startup_timeout=60, stability_seconds=1):
    directory = job_directory(exe_path, token)
    values = dict(exePath=exe_path, targetPath=target_exe_path, oldPath=old_exe_path,
                  newPath=new_exe_path, jobDir=str(directory), token=token,
                  semaphoreName=semaphore_name, expectedHash=expected_sha256,
                  version=target_version,
                  resultPath=str(Path(exe_path).resolve().parent / RESULT_NAME))
    prefix = ["$ErrorActionPreference = 'Stop'"]
    prefix += [f'${name} = {_powershell_literal(value)}' for name, value in values.items()]
    prefix += [f'$targetPid = {int(target_pid)}', f'$expectedSize = {int(expected_size)}',
               f'$targetTicks = {int(target_start_ticks)}', f'$startupTimeout = {float(startup_timeout)}',
               f'$stabilitySeconds = {float(stability_seconds)}']
    script = r'''
$lease = $null; $ownsLease = $false; $originalMoved = $false; $newInstalled = $false
$hostStopped = $false; $committed = $false; $newProcess = $null; $verifiedFile = $null
$newJob = [IntPtr]::Zero
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
public static class MouseBatteryJob {
  [StructLayout(LayoutKind.Sequential)]
  struct StartupInfo {
    public int cb;
    public IntPtr reserved, desktop, title;
    public int x, y, width, height, charsX, charsY, fill, flags;
    public short show, reservedSize;
    public IntPtr reservedBytes, stdin, stdout, stderr;
  }
  [StructLayout(LayoutKind.Sequential)]
  struct ProcessInfo {
    public IntPtr process, thread;
    public uint pid, tid;
  }
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  static extern bool CreateProcessW(string application, StringBuilder command, IntPtr processAttributes,
    IntPtr threadAttributes, bool inherit, uint flags, IntPtr environment, string directory,
    ref StartupInfo startup, out ProcessInfo information);
  [DllImport("kernel32.dll", SetLastError=true)]
  static extern uint ResumeThread(IntPtr thread);
  [DllImport("kernel32.dll", SetLastError=true)]
  static extern bool TerminateProcess(IntPtr process, uint code);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  public static extern IntPtr CreateJobObject(IntPtr attributes, string name);
  [DllImport("kernel32.dll", SetLastError=true)]
  public static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
  [DllImport("kernel32.dll", SetLastError=true)]
  public static extern bool TerminateJobObject(IntPtr job, uint exitCode);
  [DllImport("kernel32.dll")]
  public static extern bool CloseHandle(IntPtr handle);
  public static Process Launch(string application, string command, IntPtr job) {
    StartupInfo startup = new StartupInfo();
    startup.cb = Marshal.SizeOf(typeof(StartupInfo));
    ProcessInfo info;
    // CREATE_SUSPENDED | CREATE_NO_WINDOW: 在归入 Job 之前不执行任何用户代码。
    if (!CreateProcessW(application, new StringBuilder(command), IntPtr.Zero, IntPtr.Zero,
      false, 0x08000004, IntPtr.Zero, System.IO.Path.GetDirectoryName(application), ref startup, out info))
      throw new Win32Exception(Marshal.GetLastWin32Error());
    Process process = null;
    try {
      if (!AssignProcessToJobObject(job, info.process))
        throw new Win32Exception(Marshal.GetLastWin32Error());
      process = Process.GetProcessById((int)info.pid);
      IntPtr monitoredHandle = process.Handle;
      if (ResumeThread(info.thread) == UInt32.MaxValue)
        throw new Win32Exception(Marshal.GetLastWin32Error());
      return process;
    } catch {
      TerminateProcess(info.process, 1);
      if (process != null) process.Dispose();
      throw;
    } finally { CloseHandle(info.thread); CloseHandle(info.process); }
  }
}
'@
function Write-Result($status, $detail) {
  $json = @{status=$status; detail=$detail; token=$token; target=$targetPath; version=$version; time=[DateTime]::UtcNow.ToString('o')} | ConvertTo-Json -Compress
  $temporary = $resultPath + '.' + $token + '.tmp'
  [IO.File]::WriteAllText($temporary, $json, (New-Object Text.UTF8Encoding($false)))
  Move-Item -LiteralPath $temporary -Destination $resultPath -Force
}
function Verify-File($path) {
  $file = [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
  try {
    if ($file.Length -ne $expectedSize) { throw '更新文件大小校验失败' }
    $sha = [Security.Cryptography.SHA256]::Create()
    try { $hash = ([BitConverter]::ToString($sha.ComputeHash($file))).Replace('-', '').ToLowerInvariant() }
    finally { $sha.Dispose() }
    if ($hash -ne $expectedHash) { throw '更新文件 SHA-256 校验失败' }
    return $file
  } catch { $file.Dispose(); throw }
}
function Move-Retry($source, $destination) {
  for ($i=0; $i -lt 20; $i++) {
    try { Move-Item -LiteralPath $source -Destination $destination -Force; return }
    catch { if ($i -eq 19) { throw }; Start-Sleep -Milliseconds 250 }
  }
}
function Get-UpdateHost {
  $p = Get-Process -Id $targetPid -ErrorAction SilentlyContinue
  if ($p -and (($targetTicks -eq 0) -or ($p.StartTime.ToUniversalTime().ToFileTimeUtc() -ne $targetTicks))) { throw '更新宿主进程实例已变化' }
  return $p
}
try {
  $lease = [Threading.Semaphore]::OpenExisting($semaphoreName)
  $verifiedFile = Verify-File $newPath
  [IO.File]::WriteAllText((Join-Path $jobDir 'helper.ready'), $token)
  $clock = [Diagnostics.Stopwatch]::StartNew()
  $applyPath = Join-Path $jobDir 'apply.request'
  while (-not (Test-Path -LiteralPath $applyPath)) {
    if ((Test-Path -LiteralPath (Join-Path $jobDir 'abort.request')) -or $clock.Elapsed.TotalSeconds -gt 10) { throw '更新交接已取消或超时' }
    Start-Sleep -Milliseconds 50
  }
  $apply = Get-Content -LiteralPath $applyPath -Raw -Encoding UTF8 | ConvertFrom-Json
  if ($apply.token -ne $token -or $apply.target_pid -ne $targetPid) { throw '更新交接信息不匹配' }
  $ownsLease = $true
  $clock.Restart()
  while ($p = Get-UpdateHost) {
    if (Test-Path -LiteralPath (Join-Path $jobDir 'abort.request')) { throw '更新交接已取消' }
    if ($clock.Elapsed.TotalSeconds -gt 20) {
      Stop-Process -Id $targetPid -Force -ErrorAction Stop
      if (-not $p.WaitForExit(5000)) { throw '更新宿主无法退出' }
      break
    }
    Start-Sleep -Milliseconds 100
  }
  $hostStopped = $true
  if (Test-Path -LiteralPath (Join-Path $jobDir 'abort.request')) { throw '更新交接已取消' }
  $verifiedFile.Dispose(); $verifiedFile = Verify-File $newPath
  if (($targetPath -ne $exePath) -and (Test-Path -LiteralPath $targetPath)) { throw '目标版本文件已存在' }
  Move-Retry $exePath $oldPath
  $originalMoved = $true
  $verifiedFile.Dispose(); $verifiedFile = $null
  Move-Retry $newPath $targetPath
  $newInstalled = $true
  $verifiedFile = Verify-File $targetPath
  Write-Result 'installing' ''
  $env:PYINSTALLER_RESET_ENVIRONMENT = '1'
  $env:MOUSE_BATTERY_HOST_PID = $null
  $env:MOUSE_BATTERY_UPDATE_TOKEN = $token
  $newJob = [MouseBatteryJob]::CreateJobObject([IntPtr]::Zero, $null)
  if ($newJob -eq [IntPtr]::Zero) { throw '无法创建新版本进程组' }
  $newProcess = [MouseBatteryJob]::Launch($targetPath, ('"' + $targetPath + '"'), $newJob)
  $clock.Restart(); $readyAt = $null
  $startupPath = Join-Path $jobDir 'startup.ready'
  while ($true) {
    $newProcess.Refresh()
    if ($newProcess.HasExited) { throw ('新版本启动失败，退出码 ' + $newProcess.ExitCode) }
    if (Test-Path -LiteralPath $startupPath) {
      $ack = Get-Content -LiteralPath $startupPath -Raw -Encoding UTF8 | ConvertFrom-Json
      if ($ack.token -ne $token -or $ack.version -ne $version) { throw '新版本启动确认不匹配' }
      if ($null -eq $readyAt) { $readyAt = $clock.Elapsed.TotalSeconds }
      if (($clock.Elapsed.TotalSeconds - $readyAt) -ge $stabilitySeconds) { break }
    }
    if ($clock.Elapsed.TotalSeconds -gt $startupTimeout) { throw '等待新版本启动确认超时' }
    Start-Sleep -Milliseconds 100
  }
  Write-Result 'success' ''
  $committed = $true
} catch {
  $failure = $_.ToString(); $oldRestored = -not $originalMoved
  if ($verifiedFile) { $verifiedFile.Dispose(); $verifiedFile = $null }
  try {
    if ($newJob -ne [IntPtr]::Zero) { [void][MouseBatteryJob]::TerminateJobObject($newJob, 1) }
    if ($newProcess -and -not $newProcess.HasExited) {
      Stop-Process -Id $newProcess.Id -Force -ErrorAction SilentlyContinue
      if (-not $newProcess.WaitForExit(5000)) { throw '新版本进程无法结束' }
    }
    if ($originalMoved -and (Test-Path -LiteralPath $oldPath)) { Move-Retry $oldPath $exePath; $oldRestored = $true }
    if ($newInstalled -and $targetPath -ne $exePath -and (Test-Path -LiteralPath $targetPath)) { Remove-Item -LiteralPath $targetPath -Force }
  } catch { $failure += ('；恢复失败，备份位于 ' + $oldPath + '：' + $_) }
  Write-Result 'failed' $failure
  $env:PYINSTALLER_RESET_ENVIRONMENT = '1'; $env:MOUSE_BATTERY_UPDATE_TOKEN = $null
  if ($hostStopped -and $oldRestored -and (Test-Path -LiteralPath $exePath)) { Start-Process -FilePath $exePath | Out-Null }
  throw $failure
} finally {
  if ($verifiedFile) { $verifiedFile.Dispose() }
  if ($newJob -ne [IntPtr]::Zero) { [void][MouseBatteryJob]::CloseHandle($newJob) }
  try {
    if ($committed -and (Test-Path -LiteralPath $oldPath)) { Remove-Item -LiteralPath $oldPath -Force }
    if (-not (Test-Path -LiteralPath $oldPath)) { [IO.Directory]::Delete($jobDir, $true) }
  } catch { Write-Warning ('更新临时目录清理失败：' + $_) }
  if ($lease) { if ($ownsLease) { [void]$lease.Release() }; $lease.Dispose() }
}
'''
    return '\n'.join(prefix) + '\n' + script

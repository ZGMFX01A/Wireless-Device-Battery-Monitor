"""Isolated Win32 fault injection. Suspend only our own verified GPUI child after it hides.
No process discovery by name, no global input, no physical devices, no HKCU writes.
"""
import ctypes as C
from ctypes import wintypes as W
import json,queue,subprocess,sys,threading,time
from pathlib import Path
from window_probe import windows
from tray_probe import k,wait

def run(executable):
    exe=Path(executable).resolve(strict=True)
    proc=subprocess.Popen([str(exe),'--exercise-tray'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True,encoding='utf8');q=queue.Queue()
    threading.Thread(target=lambda:[q.put(line.strip()) for line in proc.stdout],daemon=True).start()
    def send(cmd):proc.stdin.write(cmd+'\n');proc.stdin.flush()
    def event(key,timeout=10):
        end=time.monotonic()+timeout
        while time.monotonic()<end:
            line=q.get(timeout=max(.01,end-time.monotonic()))
            if line.startswith(key+' '):return line.split()[1:]
        raise TimeoutError(key)
    n=C.WinDLL('ntdll');n.NtSuspendProcess.argtypes=n.NtResumeProcess.argtypes=[W.HANDLE];n.NtSuspendProcess.restype=n.NtResumeProcess.restype=W.LONG
    k.QueryFullProcessImageNameW.argtypes=[W.HANDLE,W.DWORD,W.LPWSTR,C.POINTER(W.DWORD)];h=None;frozen=False
    try:
        send('flyout');pid=int(event('GUI_PID')[0]);event('PRESENTED');send('flyout');wait(lambda:any(not a[1] for a in windows(pid)),4);time.sleep(.4)
        h=k.OpenProcess(0x100000|0x1000|0x0800,False,pid);assert h
        buf=C.create_unicode_buffer(32768);length=W.DWORD(len(buf));assert k.QueryFullProcessImageNameW(h,0,buf,C.byref(length)) and Path(buf.value).resolve()==exe
        assert n.NtSuspendProcess(h)==0;frozen=True
        # The child acknowledged Hidden, but it cannot read Exit. Issue an intent
        # during graceful shutdown and verify it reaches a replacement process.
        time.sleep(30.15);assert k.WaitForSingleObject(h,0)==258
        start=time.monotonic();send('settings');newpid=int(event('GUI_PID',10)[0]);assert newpid!=pid;event('PRESENTED');assert wait(lambda:any(x[1] for x in windows(newpid)),5)
        assert k.WaitForSingleObject(h,1500)==0;frozen=False
        send('quit');assert proc.wait(7)==0
        print(json.dumps(dict(ok=True,hardware=False,original_child=pid,replacement_child=newpid,reopen_elapsed_s=round(time.monotonic()-start,3))))
    finally:
        if h:
            if frozen and k.WaitForSingleObject(h,0)==258:n.NtResumeProcess(h)
            k.CloseHandle(h)
        if proc.poll() is None:
            try:send('quit');proc.wait(6)
            except (OSError,subprocess.TimeoutExpired):proc.kill();proc.wait()
if __name__=='__main__':run(sys.argv[1])

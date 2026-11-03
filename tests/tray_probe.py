"""End-to-end tray supervisor test against an isolated, hardware-free GPUI preview."""
from window_probe import windows
import ctypes as C,json,queue,subprocess,sys,threading,time,os
from pathlib import Path
from ctypes import wintypes as W
k=C.windll.kernel32;k.OpenProcess.argtypes=[W.DWORD,W.BOOL,W.DWORD];k.OpenProcess.restype=W.HANDLE;k.CloseHandle.argtypes=[W.HANDLE];k.WaitForSingleObject.argtypes=[W.HANDLE,W.DWORD]
class Memory(C.Structure):_fields_=[('cb',W.DWORD),('faults',W.DWORD)]+[(n,C.c_size_t) for n in 'peak_ws ws peak_paged paged peak_np np pagefile peak_pf private_commit private_ws shared_commit'.split()]
api=C.windll.psapi;api.GetProcessMemoryInfo.argtypes=[W.HANDLE,C.POINTER(Memory),W.DWORD]
def mem(pid):
    h=k.OpenProcess(0x0400|0x10,False,pid); m=Memory();m.cb=C.sizeof(m)
    try:
        if not h or not api.GetProcessMemoryInfo(h,C.byref(m),m.cb):return None
        return {'private_ws_mib':round(m.private_ws/1048576,2),'private_commit_mib':round(m.private_commit/1048576,2)}
    finally:
        if h:k.CloseHandle(h)
def wait(fn,timeout):
    start=time.monotonic()
    while time.monotonic()-start<timeout:
        x=fn()
        if x:return x
        time.sleep(.025)
    raise RuntimeError('expected state not reached')
def run(exe):
    p=subprocess.Popen([str(exe),'--exercise-tray'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,encoding='utf8',errors='replace'); q=queue.Queue();threading.Thread(target=lambda:[q.put(l.strip()) for l in p.stdout],daemon=True).start()
    def cmd(s):p.stdin.write(s+'\n');p.stdin.flush()
    def next_event(prefix,timeout=10):
        end=time.monotonic()+timeout
        while time.monotonic()<end:
            line=q.get(timeout=max(.01,end-time.monotonic()))
            if line.startswith(prefix+' '):return line
        raise RuntimeError('missing '+prefix)
    def child_pid():return int(next_event('GUI_PID').split()[1])
    def shown(pid):return any(w[1] for w in windows(pid))
    try:
        time.sleep(1);assert p.poll() is None;before=mem(p.pid);t=time.monotonic();cmd('flyout');a=child_pid();next_event('PRESENTED');wait(lambda:shown(a),10);first=round((time.monotonic()-t)*1000);ui=mem(a);cmd('flyout');wait(lambda:not shown(a),5);t=time.monotonic();cmd('settings');next_event('PRESENTED');wait(lambda:shown(a),6);warm=round((time.monotonic()-t)*1000);cmd('flyout');next_event('PRESENTED');wait(lambda:shown(a),5);cmd('flyout');wait(lambda:not shown(a),4);h=k.OpenProcess(0x100000,False,a);assert h;t=time.monotonic()
        try:assert k.WaitForSingleObject(h,40000)==0,'idle GUI failed to exit'
        finally:k.CloseHandle(h)
        elapsed=round(time.monotonic()-t,2);after=mem(p.pid);assert p.poll() is None;t=time.monotonic();cmd('settings');b=child_pid();assert a!=b;wait(lambda:shown(b),10);cold=round((time.monotonic()-t)*1000)
        crash=k.OpenProcess(0x100001,False,b);assert crash
        try:
            k.TerminateProcess.argtypes=[W.HANDLE,W.UINT]; assert k.TerminateProcess(crash,71);assert k.WaitForSingleObject(crash,5000)==0
        finally:k.CloseHandle(crash)
        time.sleep(.25);cmd('settings');c=child_pid();assert c!=b;wait(lambda:shown(c),10);cmd('quit');assert p.wait(7)==0
        print(json.dumps(dict(ok=True,preview=True,hardware=False,first_pid=a,reopened_pid=b,after_crash_pid=c,initial_ms=first,warm_observed_ms=warm,idle_exit_s=elapsed,cold_reopen_ms=cold,backend_before=before,visible_gui_only=ui,backend_after=after)))
    finally:
        if p.poll() is None:
            try:cmd('quit');p.wait(5)
            except (OSError,subprocess.TimeoutExpired):p.kill();p.wait()
        print(p.stderr.read()[-3500:],file=sys.stderr)
if __name__=='__main__':run(Path(sys.argv[1]).resolve())

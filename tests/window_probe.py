"""Drive only a newly spawned GUI child (no tray, config, HID/BLE or registry writes)."""
import ctypes as C, json, os, queue, subprocess, sys, threading, time
from ctypes import wintypes as W
from pathlib import Path
u=C.WinDLL('user32',use_last_error=True)
cb_type=C.WINFUNCTYPE(W.BOOL,W.HWND,W.LPARAM)
u.SetProcessDpiAwarenessContext.argtypes=[C.c_void_p];u.SetProcessDpiAwarenessContext.restype=W.BOOL
u.SetProcessDpiAwarenessContext(C.c_void_p(-4))  # PrintWindow uses physical pixels; query bounds in same space.
u.GetWindowTextW.argtypes=[W.HWND,W.LPWSTR,C.c_int];u.GetWindowThreadProcessId.argtypes=[W.HWND,C.POINTER(W.DWORD)];u.IsWindowVisible.argtypes=[W.HWND];u.EnumWindows.argtypes=[cb_type,W.LPARAM];u.GetWindowRect.argtypes=[W.HWND,C.POINTER(W.RECT)]
def windows(pid):
    targets=[]
    @cb_type
    def each(hwnd,_):
        found=W.DWORD();u.GetWindowThreadProcessId(hwnd,C.byref(found))
        if found.value==pid:
            name=C.create_unicode_buffer(256);u.GetWindowTextW(hwnd,name,256)
            if 'GPUI' in name.value:
                r=W.RECT();u.GetWindowRect(hwnd,C.byref(r));targets.append((hwnd,bool(u.IsWindowVisible(hwnd)),r.right-r.left,r.bottom-r.top))
        return True
    u.EnumWindows(each,0);return targets

def run(exe):
    proc=subprocess.Popen([str(exe),'--gui-child'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,encoding='utf8',errors='replace');stream=queue.Queue();threading.Thread(target=lambda:[stream.put(x.strip()) for x in proc.stdout],daemon=True).start()
    def emit(value):proc.stdin.write(json.dumps(value)+'\n');proc.stdin.flush()
    def screen(visible):
        end=time.monotonic()+5
        while time.monotonic()<end:
            ws=windows(proc.pid)
            if len(ws)==1 and ws[0][1]==visible:return ws[0]
            if proc.poll() is not None:raise RuntimeError('unexpected GUI exit')
            time.sleep(.025)
        raise RuntimeError(f'window visible={visible} not observed: {ws}')
    try:
        line=stream.get(timeout=15);assert json.loads(line)=='Ready',line
        emit({'Hide':0});assert json.loads(stream.get(timeout=4))=={'Hidden':0},'lost hide before first open'
        t0=time.perf_counter();emit({'Open':['Settings',1,None]});first=screen(True);times=[]
        for g in range(1,101):
            emit({'Hide':g});screen(False);t=time.perf_counter();emit({'Open':['Settings' if g%2 else 'Flyout',g+1,None]});w=screen(True);assert w[0]==first[0];times.append(round((time.perf_counter()-t)*1000,1))
        emit('Exit');assert proc.wait(6)==0
        print(json.dumps({'ok':True,'real_hwnd':first[0],'same_window':True,'presents':101,'hides':100,'hardware_access':False,'warm_open_observed_ms':times}))
    finally:
        if proc.poll() is None:
            try:emit('Exit');proc.wait(4)
            except (OSError,subprocess.TimeoutExpired):proc.kill();proc.wait()
        if proc.returncode: print('CHILD STDERR',proc.stderr.read()[-3500:],file=sys.stderr)
if __name__=='__main__': run(Path(sys.argv[1]).resolve())

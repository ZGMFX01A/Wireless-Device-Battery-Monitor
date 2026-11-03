"""Capture only our own preview HWND, never the full desktop. No hardware/config writes."""
import ctypes as C,sys,subprocess,time,json
from ctypes import wintypes as W
from PIL import Image
from pathlib import Path
from window_probe import windows
u=C.windll.user32;g=C.windll.gdi32
class BI(C.Structure):_fields_=[('size',W.DWORD),('width',W.LONG),('height',W.LONG),('planes',W.WORD),('bpp',W.WORD),('compression',W.DWORD),('image',W.DWORD),('xp',W.LONG),('yp',W.LONG),('used',W.DWORD),('important',W.DWORD),('colors',W.DWORD*3)]
for f,args,ret in [(u.GetDC,[W.HWND],W.HDC),(u.ReleaseDC,[W.HWND,W.HDC],C.c_int),(u.PrintWindow,[W.HWND,W.HDC,W.UINT],W.BOOL),(g.CreateCompatibleDC,[W.HDC],W.HDC),(g.CreateCompatibleBitmap,[W.HDC,C.c_int,C.c_int],W.HBITMAP),(g.SelectObject,[W.HDC,W.HANDLE],W.HANDLE),(g.GetDIBits,[W.HDC,W.HBITMAP,W.UINT,W.UINT,C.c_void_p,C.POINTER(BI),W.UINT],C.c_int),(g.DeleteObject,[W.HANDLE],W.BOOL),(g.DeleteDC,[W.HDC],W.BOOL)]:f.argtypes=args;f.restype=ret

def capture(h,p):
    rect=W.RECT();u.GetWindowRect(h,C.byref(rect));w=rect.right-rect.left;t=rect.bottom-rect.top;dc=u.GetDC(h);m=g.CreateCompatibleDC(dc);b=g.CreateCompatibleBitmap(dc,w,t);old=g.SelectObject(m,b)
    try:
        assert u.PrintWindow(h,m,2);g.SelectObject(m,old);raw=C.create_string_buffer(w*t*4);bi=BI();bi.size=40;bi.width=w;bi.height=-t;bi.planes=1;bi.bpp=32;assert g.GetDIBits(dc,b,0,t,raw,C.byref(bi),0)==t;im=Image.frombytes('RGB',(w,t),raw.raw,'raw','BGRX');im.save(p);return len(im.resize((50,50)).getcolors(3000) or [])
    finally:g.SelectObject(m,old);g.DeleteObject(b);g.DeleteDC(m);u.ReleaseDC(h,dc)

def run(exe,out):
    out.mkdir(exist_ok=True,parents=True);p=subprocess.Popen([str(exe),'--gui-child'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,encoding='utf8',text=True);assert p.stdout.readline().strip()=='"Ready"';send=lambda v:(p.stdin.write(json.dumps(v)+'\n'),p.stdin.flush());rows=[dict(device=dict(name=n,id=n,kind=k),battery=dict(raw=v,display=v,charge=c,source='Rog',mv=None),sampled=0,error=None) for n,k,v,c in [('办公鼠标','Mouse',72,'Discharging'),('无线键盘','Keyboard',35,'Charging')]];state=dict(prefs=dict(theme='Dark',lang='Zh'),rows=rows,candidates=[],note='',demo=True,autostart=False,update='Idle');send({'State':state});
    try:
        for i,m in enumerate(['Flyout','Settings']):
            send({'Open':[m,i+1,None]});time.sleep(.65);ws=windows(p.pid);assert len(ws)==1 and ws[0][1];colors=capture(ws[0][0],out/(m+'.png'));print(m,ws[0][2:],colors);assert colors>10,'capture not useful'
    finally:send('Exit');p.wait(5)
if __name__=='__main__':run(Path(sys.argv[1]).resolve(),Path(sys.argv[2]))

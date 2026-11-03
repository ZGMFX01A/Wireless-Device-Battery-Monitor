"""Capture the actual composed GPUI client against an owned, opaque neutral window.
Both executables receive identical test inputs. No tray, real device, desktop file or config is used.
Usage: python gui/tests/card_material_probe.py <exe> <new-output-directory>
"""
from pathlib import Path
import ctypes as C
from ctypes import wintypes as W
from datetime import datetime
import hashlib, json, sys, time
from PIL import Image, ImageStat
from control_probe import Probe
from glass_probe import G, D, u
from render_probe import BI

G.BitBlt.argtypes=[W.HDC,C.c_int,C.c_int,C.c_int,C.c_int,W.HDC,C.c_int,C.c_int,W.DWORD]
G.BitBlt.restype=W.BOOL

def screenshot(rect):
    x,y,w,h=rect
    src=u.GetDC(None); dc=G.CreateCompatibleDC(src); bmp=G.CreateCompatibleBitmap(src,w,h); previous=G.SelectObject(dc,bmp)
    try:
        assert G.BitBlt(dc,0,0,w,h,src,x,y,0x00CC0020)
        G.SelectObject(dc,previous)
        raw=C.create_string_buffer(w*h*4); bi=BI(); bi.size=40;bi.width=w;bi.height=-h;bi.planes=1;bi.bpp=32
        assert G.GetDIBits(dc,bmp,0,h,raw,C.byref(bi),0)==h
        return Image.frombytes('RGB',(w,h),raw.raw,'raw','BGRX')
    finally:
        G.DeleteObject(bmp);G.DeleteDC(dc);u.ReleaseDC(None,src)

def main(exe,out):
    out.mkdir(parents=True,exist_ok=False)
    app=Probe(exe); back=None; result={}
    try:
        app.open('Flyout'); hw=app.hw; rc=W.RECT();u.GetClientRect(hw,C.byref(rc));w,h=rc.right,rc.bottom
        # Reserve an owned witness margin even around the rounded corners. Keep both windows inside the primary monitor.
        assert u.GetSystemMetrics(0)>=w+88 and u.GetSystemMetrics(1)>=h+88
        cur=W.POINT();u.GetCursorPos(C.byref(cur));left=44 if cur.x>u.GetSystemMetrics(0)//2 else u.GetSystemMetrics(0)-w-44
        top=(u.GetSystemMetrics(1)-h)//2
        assert u.SetWindowPos(hw,-1,left,top,0,0,0x51);pos=W.POINT();u.ClientToScreen(hw,C.byref(pos));x,y=pos.x,pos.y
        back=u.CreateWindowExW(0x08000088,'BUTTON','Material probe: neutral backplate',0x80000000,x-32,y-32,w+64,h+64,None,None,None,None);assert back
        assert u.SetWindowPos(back,-1,x-32,y-32,w+64,h+64,0x50);assert u.SetWindowPos(hw,-1,0,0,0,0,0x13);u.UpdateWindow(back)
        wr=W.RECT();u.GetClientRect(back,C.byref(wr))
        names=['G502 X LIGHTSPEED','Galaxy Buds4 Pro','NUT75 BT1'];values=[80,76,94]; times=[30,44,47]
        for theme,scene,plate in [('Light','daily',0xf1eae5),('Dark','daily',0x39332e),('Light','states',0xf1eae5),('Dark','states',0x39332e)]:
            app.state['prefs'].update(theme=theme,lang='Zh',glass=25);rows=[]
            for i,n in enumerate(names):
                kind='Mouse' if i==0 else 'Bluetooth';dev={'name':n,'kind':kind,'id':'hid:046d:fixed' if i==0 else 'bt:fixed'+str(i)};pct=values[i] if scene=='daily' else [12,35,94][i]
                rows.append(dict(device=dev,battery=dict(raw=pct,display=pct,charge='Charging' if scene=='states' and i==1 else 'Discharging',source={'Hidpp':4100} if i==0 else 'Classic',mv=None),error='offline' if scene=='states' and i==2 else None,sampled=int(datetime(2026,11,3,11,24,times[i]).timestamp())))
            app.state['rows']=rows;app.state['candidates']=[r['device'] for r in rows];app.update()
            dc=u.GetDC(back);brush=G.CreateSolidBrush(plate)
            try:u.FillRect(dc,C.byref(wr),brush);G.GdiFlush()
            finally:u.ReleaseDC(back,dc);G.DeleteObject(brush)
            D.DwmFlush();time.sleep(.5);assert u.WindowFromPoint(W.POINT(x-15,y+h//2))==back
            for dx in (1,w//2,w-2):
                for dy in (1,h//2,h-2):assert u.WindowFromPoint(W.POINT(x+dx,y+dy)) in (hw,back), 'test area obscured'
            im=screenshot((x,y,w,h));tag=theme+'-'+scene;im.save(out/(tag+'.png')); logical=im.resize((380,540),Image.Resampling.LANCZOS)
            zones={'card_upper':(148,119,182,130),'card_lower':(150,207,184,219),'window':(4,158,9,170)}
            result[tag]={k:[round(v,1) for v in ImageStat.Stat(logical.crop(box)).mean] for k,box in zones.items()}
        (out/'capture.json').write_text(json.dumps(dict(exe_sha256=hashlib.sha256(exe.read_bytes()).hexdigest(),dpi=round(app.scale*96),px=[w,h],rows=names,values=values,material=result),indent=2),encoding='utf8');print(result)
    finally:
        if back:u.DestroyWindow(back)
        app.close()
if __name__=='__main__':main(Path(sys.argv[1]).resolve(),Path(sys.argv[2]).resolve())

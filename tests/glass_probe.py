"""Probe the compositor using two opaque, synthetic backplates, sampling only the test HWND.
No desktop screenshot is saved; colors sampled inside the app are the only output.
"""
import os
from control_probe import Probe,ROOT,OUT,C,W,u
import sys,json,time,statistics
from pathlib import Path
G=C.windll.gdi32; D=C.windll.dwmapi
u.CreateWindowExW.argtypes=[W.DWORD,W.LPCWSTR,W.LPCWSTR,W.DWORD,C.c_int,C.c_int,C.c_int,C.c_int,W.HWND,W.HMENU,W.HINSTANCE,C.c_void_p];u.CreateWindowExW.restype=W.HWND
u.SetWindowPos.argtypes=[W.HWND,W.HWND,C.c_int,C.c_int,C.c_int,C.c_int,W.UINT];u.DestroyWindow.argtypes=[W.HWND];u.FillRect.argtypes=[W.HDC,C.POINTER(W.RECT),W.HBRUSH];G.GetPixel.argtypes=[W.HDC,C.c_int,C.c_int];G.GetPixel.restype=W.DWORD;G.CreateSolidBrush.argtypes=[W.DWORD];G.CreateSolidBrush.restype=W.HBRUSH;u.WindowFromPoint.argtypes=[W.POINT];u.WindowFromPoint.restype=W.HWND;D.DwmGetWindowAttribute.argtypes=[W.HWND,W.DWORD,C.c_void_p,W.DWORD];D.DwmFlush.argtypes=[]
def run(exe):
 a=Probe(exe);board=None
 try:
  a.open('Flyout');a.state['rows']=[];a.state['prefs']['theme']='Dark';a.update();hw=a.hw;pos=W.POINT();u.ClientToScreen(hw,C.byref(pos));r=W.RECT();u.GetClientRect(hw,C.byref(r));board=u.CreateWindowExW(0x08000088,'BUTTON','GPUI synthetic backdrop',0x80000000,pos.x-40,pos.y-40,r.right+80,r.bottom+80,None,None,None,None);assert board
  assert u.SetWindowPos(board,-1,pos.x-40,pos.y-40,r.right+80,r.bottom+80,0x50);assert u.SetWindowPos(hw,-1,0,0,0,0,0x13);u.UpdateWindow(board);time.sleep(.25);value=C.c_int();hr=D.DwmGetWindowAttribute(hw,38,C.byref(value),4);out={}
  # Paint the entire opaque backplate including a visible 40-pixel witness strip.
  results=[];wr=W.RECT();u.GetClientRect(board,C.byref(wr));witness=None
  for dx,dy in [(-20,r.bottom//2),(r.right+20,r.bottom//2),(r.right//2,-20),(r.right//2,r.bottom+20)]:
   pt=W.POINT(pos.x+dx,pos.y+dy)
   if u.WindowFromPoint(pt)==board:witness=pt;break
  assert witness is not None,('background not visible',hw,board,pos.x,pos.y,r.right,r.bottom,[(v,u.WindowFromPoint(W.POINT(pos.x+v,pos.y+r.bottom//2))) for v in [-20,0,r.right+20]])
  class AP(C.Structure):_fields_=[('state',W.DWORD),('flags',W.DWORD),('color',W.DWORD),('anim',W.DWORD)]
  class DA(C.Structure):_fields_=[('attr',C.c_int),('data',C.c_void_p),('size',C.c_size_t)]
  def accent(v,color=0):
   ap=AP(v,2,color,0);da=DA(19,C.cast(C.pointer(ap),C.c_void_p),C.sizeof(ap));u.SetWindowCompositionAttribute.argtypes=[W.HWND,C.POINTER(DA)];return u.SetWindowCompositionAttribute(hw,C.byref(da))
  def sysmat(v):
   q=C.c_int(v);D.DwmSetWindowAttribute.argtypes=[W.HWND,W.DWORD,C.c_void_p,W.DWORD];return D.DwmSetWindowAttribute(hw,38,C.byref(q),4)
  def samples(color):
   br=G.CreateSolidBrush(color);dc=u.GetDC(board);u.FillRect(dc,C.byref(wr),br);G.GdiFlush();u.ReleaseDC(board,dc);G.DeleteObject(br);D.DwmFlush();time.sleep(.35);src=u.GetDC(None);vals=[]
   try:
    seen=G.GetPixel(src,witness.x,witness.y);assert ((seen&255)-(seen>>16&255))*(1 if color&255>128 else -1)>80,("backplate paint mismatch",hex(seen),hex(color));
    for yy in [.82,.84,.86]:
     for xx in [.2,.4,.6,.8]:
      x,y=pos.x+int(r.right*xx),pos.y+int(r.bottom*yy);assert u.WindowFromPoint(W.POINT(x,y))==hw,'test window obscured';v=G.GetPixel(src,x,y);assert v!=0xffffffff;vals.append([v&255,(v>>8)&255,(v>>16)&255]);
   finally:u.ReleaseDC(None,src)
   return [round(statistics.median(p[i] for p in vals),1) for i in range(3)]
  for level in [0,25,100]:
   a.state['prefs']['glass']=level;a.update();red=samples(0x2323dd);blue=samples(0xdd2323);out[str(level)]=dict(red=red,blue=blue,backdrop_response=sum(abs(x-y) for x,y in zip(red,blue)))
  # Stripes of known contrast four physical pixels wide: true blur suppresses these high frequencies,
  # while a plain translucent surface would preserve their red/blue alternation.
  brushes=[G.CreateSolidBrush(x) for x in (0x2323dd,0xdd2323)];dc=u.GetDC(board);local=W.POINT(pos.x,pos.y);u.ScreenToClient(board,C.byref(local));
  for x in range(0,wr.right,4):
   rect=W.RECT(x,0,min(wr.right,x+4),wr.bottom);u.FillRect(dc,C.byref(rect),brushes[(x//4)%2]);
  G.GdiFlush();u.ReleaseDC(board,dc)
  for b in brushes:G.DeleteObject(b)
  D.DwmFlush();time.sleep(.45);dc=u.GetDC(None);split=[[],[]]
  for x in range(int(r.right*.25),int(r.right*.75)):
   y=int(r.bottom*.85);pt=W.POINT(pos.x+x,pos.y+y);assert u.WindowFromPoint(pt)==hw;v=G.GetPixel(dc,pt.x,pt.y);split[((x+local.x)//4)%2].append((v&255)-(v>>16&255));
  u.ReleaseDC(None,dc);striped=abs(statistics.mean(split[0])-statistics.mean(split[1]));full=abs((red[0]-red[2])-(blue[0]-blue[2]));ratio=striped/max(1.,full)
  if os.getenv('GPUI_DEV_DIAG_GLASS'):
   for method in ['accentoff-mat3','mat1-acrylic','mat1-blur','accentoff-mat1','mat1-gradient']:
    if method=='accentoff-mat3':accent(0);sysmat(3)
    elif method=='mat1-acrylic':sysmat(1);accent(4,0x200f0f0f)
    elif method=='mat1-blur':sysmat(1);accent(3)
    elif method=='accentoff-mat1':accent(0);sysmat(1)
    else:sysmat(1);accent(2,0x01010101)
    red=samples(0x2323dd);blue=samples(0xdd2323);results.append([method,red,blue]);print(method,red,blue,flush=True)
  result=dict(stripe_width_px=4,contrast_ratio=round(ratio,4),blur_observed=full>40 and ratio<0.45,diagnostic=results,backplate_witness_verified=True,dwm_hresult=hr,dwm_material=value.value,dpi=round(a.scale*96),samples=out,backdrop_visible=out['100']['backdrop_response']>12,control_changes_backdrop=out['100']['backdrop_response']>out['0']['backdrop_response']+10);(OUT/('glass-diagnostic.json' if results else 'glass.json')).write_text(json.dumps(result,indent=2));print(json.dumps(result));return result
 finally:
  if board:u.DestroyWindow(board)
  a.close()
if __name__=='__main__':
 result=run(Path(sys.argv[1]).resolve());sys.exit(0 if result['backdrop_visible'] and result['control_changes_backdrop'] and result['blur_observed'] else 2)

"""Compare the unchanged legacy HTML/CSS/JS to the real GPUI HWND. Uses only synthetic input.
Browser is a developer-only reference renderer, not shipped or embedded in the Rust app.
"""
import os, sys, time, json, threading, tempfile, subprocess, functools, base64, io, http.server, urllib.request, ctypes as C
from pathlib import Path
from PIL import Image,ImageDraw
from websockets.sync.client import connect
from render_probe import capture,u,W
from window_probe import windows
ROOT=Path(__file__).resolve().parents[1]; OUT=Path(os.environ.get('GPUI_VERIFY_UI_DIR',ROOT/'out/evidence/ui-parity'));OUT.mkdir(parents=True,exist_ok=True); LEG=Path(os.environ.get('LEGACY_WEB_UI_DIR', ROOT.parent/'legacy-test-run/mouse-battery/web_ui'))
FIX=[('hid:046d:1:Mouse:test','G PRO X SUPERLIGHT','Mouse',72,'Discharging',{'Hidpp':4100},'罗技'),('hid:0b05:2:Keyboard:test','ROG Azoth','Keyboard',35,'Charging','Rog','ROG'),('bt:test','Bluetooth Headset','Bluetooth',12,'Unknown','Classic','蓝牙')]
def states(theme='Dark',lang='Zh'):
 base=dict(theme=theme,lang=lang,keyboard=FIX[1][0],bluetooth=[FIX[2][0]]);rows=[dict(device=dict(id=i,name=n,kind=k),battery=dict(raw=v,display=v,charge=c,source=so,mv=None),sampled=0,error=None) for i,n,k,v,c,so,_ in FIX];state=dict(prefs=base,rows=rows,candidates=[r['device'] for r in rows]+[dict(id='spare',name='NUT75',kind='Keyboard')],note='',demo=True,autostart=False,update='Idle');lc='zh-CN' if lang=='Zh' else 'en-US';ls=[dict(device_id=i,name=n,brand=br if lang=='Zh' else {'罗技':'Logitech','蓝牙':'Bluetooth'}.get(br,br),percentage=v,charging=c=='Charging',online=True,last_update=0,status_text=('充电中' if c=='Charging' else '正常') if lang=='Zh' else ('Charging' if c=='Charging' else 'Online')) for i,n,k,v,c,so,br in FIX];old=dict(version='0.1.1',mice=ls[:1],keyboard=ls[1],bluetooth_devices=ls[2:],read_state='ok',config=dict(effective_ui_language=lc,ui_language=lc,theme_mode=theme.lower(),tray_icon_priority='mouse_first',tray_icon_style='capsule',auto_update=False,autostart=False,auto_refresh=True,low_battery_notify=20,glass_transparency=25));return state,old
class Quiet(http.server.SimpleHTTPRequestHandler):
 def log_message(self,*a):pass
class CDP:
 def __init__(self,ws): self.ws=ws;self.n=0
 def call(self,method,**params):
  self.n+=1;self.ws.send(json.dumps(dict(id=self.n,method=method,params=params)))
  while True:
   r=json.loads(self.ws.recv(timeout=10))
   if r.get('id')==self.n:
    if 'error'in r:raise RuntimeError(r['error'])
    return r.get('result',{})
 def ev(self,expr):
  v=self.call('Runtime.evaluate',expression=expr,returnByValue=True,awaitPromise=True)
  if 'exceptionDetails' in v:raise RuntimeError(v['exceptionDetails'])
  return v['result'].get('value')
def legacy(port,out):
 edge=Path(os.environ['ProgramFiles(x86)'])/'Microsoft/Edge/Application/msedge.exe';doc={}
 with tempfile.TemporaryDirectory(prefix='gpui-reference-',ignore_cleanup_errors=True) as td:
  p=subprocess.Popen([str(edge),'--headless=new','--remote-debugging-port=0','--disable-extensions','--no-first-run','--disable-background-networking','--user-data-dir='+td,'about:blank'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  try:
   f=Path(td)/'DevToolsActivePort';end=time.monotonic()+14
   while not f.exists() and time.monotonic()<end:time.sleep(.05)
   cp=f.read_text().splitlines()[0];pages=json.load(urllib.request.urlopen('http://127.0.0.1:'+cp+'/json/list',timeout=4));sock=next(x['webSocketDebuggerUrl'] for x in pages if x['type']=='page')
   with connect(sock,max_size=8<<20) as ws:
    c=CDP(ws);_,default=states();c.call('Page.enable');c.call('Page.addScriptToEvaluateOnNewDocument',source='window.pywebview={api:{get_state:async()=>('+json.dumps(default)+'),on_frontend_ready:async()=>{window.__testReady=true},on_frontend_error:async e=>{window.__bad=String(e)}}}');c.call('Page.navigate',url=f'http://127.0.0.1:{port}/index.html');end=time.monotonic()+10
    while not c.ev('!!window.__testReady') and time.monotonic()<end:time.sleep(.05)
    assert c.ev('!!window.__testReady'),c.ev('window.__bad')
    for theme,lang,mode,w,h in CASES:
     _,st=states(theme,lang);c.call('Emulation.setDeviceMetricsOverride',width=w,height=h,deviceScaleFactor=1,mobile=False);c.call('Emulation.setDefaultBackgroundColorOverride',color=dict(r=25,g=29,b=38,a=1) if theme=='Dark' else dict(r=238,g=241,b=246,a=1));c.ev('window.__onBatteryStateUpdated('+json.dumps(st)+');window.__setViewMode('+json.dumps('settings' if mode=='Settings' else 'flyout')+')');time.sleep(.23);name=f'{theme}-{lang}-{mode}';png=c.call('Page.captureScreenshot',format='png',fromSurface=True,captureBeyondViewport=False);(out/(name+'-legacy.png')).write_bytes(base64.b64decode(png['data']));doc[name]=c.ev('Object.fromEntries([...document.querySelectorAll("#btnRefresh,#btnToggleSettings,#btnScanDevices,#btnAddKeyboard,#btnAddBluetooth,.device-card,.card-btn-edit,.card-btn-remove,.setting-item,.seg-btn,.switch-control,#trayPrioritySelect,.stepper-control")].filter(x=>x.offsetWidth).map((x,i)=>[x.id||x.className+":"+i,Array.from([x.getBoundingClientRect().x,x.getBoundingClientRect().y,x.offsetWidth,x.offsetHeight])]))')
  finally:p.terminate();p.wait(8)
 (out/'legacy-layout.json').write_text(json.dumps(doc,ensure_ascii=False,indent=2),encoding='utf8');return doc
CASES=[(t,l,m,w,h) for t,l in [('Dark','Zh'),('Light','Zh'),('Dark','En')] for m,w,h in [('Flyout',380,540),('Settings',540,680)]]
def native(exe,out):
 p=subprocess.Popen([str(exe),'--gui-child'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,encoding='utf8',text=True);send=lambda x:(p.stdin.write(json.dumps(x)+'\n'),p.stdin.flush());assert json.loads(p.stdout.readline())=='Ready';meta={}
 try:
  for i,(t,l,m,w,h) in enumerate(CASES):
   st,_=states(t,l);send({'State':st});send({'Open':[m,i+1,None]});time.sleep(.40);win=windows(p.pid);assert len(win)==1 and win[0][1];hwnd=win[0][0];name=f'{t}-{l}-{m}';raw=out/(name+'-native.png');assert capture(hwnd,raw)>10;im=Image.open(raw);rc=W.RECT();rect=W.RECT();pt=W.POINT();u.GetClientRect(hwnd,C.byref(rc));u.GetWindowRect(hwnd,C.byref(rect));u.ClientToScreen(hwnd,C.byref(pt));x,y=pt.x-rect.left,pt.y-rect.top;client=im.crop((x,y,x+rc.right,y+rc.bottom));dpi=u.GetDpiForWindow(hwnd);dim=(round(rc.right*96/dpi),round(rc.bottom*96/dpi));client=client.resize(dim,Image.Resampling.LANCZOS);client.save(out/(name+'-gpui.png'));meta[name]=dict(dpi=dpi,client_px=[rc.right,rc.bottom],logical=dim)
   old=Image.open(out/(name+'-legacy.png')).convert('RGB');canvas=Image.new('RGB',(old.width+client.width+18,max(old.height,client.height)+32),'#383e48');dr=ImageDraw.Draw(canvas);dr.text((8,8),'Legacy  /  original frontend',fill='white');dr.text((old.width+20,8),'GPUI  /  native window',fill='white');canvas.paste(old,(0,32));canvas.paste(client,(old.width+18,32));canvas.save(out/(name+'-compare.png'))
 finally:send('Exit');p.wait(6)
 return meta
if __name__=='__main__':
 srv=http.server.ThreadingHTTPServer(('127.0.0.1',0),functools.partial(Quiet,directory=str(LEG)));threading.Thread(target=srv.serve_forever,daemon=True).start()
 try:legacy(srv.server_port,OUT)
 finally:srv.shutdown();srv.server_close()
 metrics=native(Path(sys.argv[1]).resolve(),OUT);(OUT/'capture.json').write_text(json.dumps(metrics,indent=2));print(json.dumps(metrics))

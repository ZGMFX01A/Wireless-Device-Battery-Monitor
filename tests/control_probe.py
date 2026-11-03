"""Interact with the release GPUI HWND via window-targeted messages. No global input automation.
Validate that actual buttons emit the expected parent commands. Parent replies are fixtures.
"""
from pathlib import Path
import ctypes as C, json, sys, subprocess, queue, threading, time, copy
from ctypes import wintypes as W
from PIL import Image
from visual_compare import states,windows,ROOT,capture,u,OUT
u.PostMessageW.argtypes=[W.HWND,W.UINT,W.WPARAM,W.LPARAM];u.GetDpiForWindow.argtypes=[W.HWND];u.GetDpiForWindow.restype=W.UINT
class Probe:
 def __init__(self,exe):
  self.p=subprocess.Popen([str(exe),'--gui-child'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,encoding='utf8',errors='replace');self.q=queue.Queue();threading.Thread(target=lambda:[self.q.put(json.loads(x)) for x in self.p.stdout],daemon=True).start();self.trace=[];self.state=states()[0];self.g=0;self.results=[];self.expect(lambda x:x=='Ready');self.update();self.open('Settings')
 def emit(self,v):self.p.stdin.write(json.dumps(v)+'\n');self.p.stdin.flush()
 def update(self):self.emit({'State':self.state});time.sleep(.12)
 def expect(self,f,timeout=3.2):
  t=time.monotonic()+timeout
  while time.monotonic()<t:
   try:x=self.q.get(timeout=max(.015,t-time.monotonic()))
   except queue.Empty:break
   self.trace.append(x)
   if f(x):return x
  raise AssertionError('UI did not emit expected command. Recent events: '+repr(self.trace[-7:]))
 def open(self,m):
  self.g+=1;self.emit({'Open':[m,self.g,None]});self.expect(lambda v:v=={'Presented':self.g});time.sleep(.22);self.hw=windows(self.p.pid)[0][0];self.scale=u.GetDpiForWindow(self.hw)/96;rect=W.RECT();u.GetClientRect(self.hw,C.byref(rect));print("open",m,round(rect.right/self.scale),round(rect.bottom/self.scale),flush=True);assert abs(rect.right/self.scale-(540 if m=="Settings" else 380))<2, "wrong client width"
 def click(self,x,y):
  xy=round(y*self.scale)<<16|round(x*self.scale);assert u.PostMessageW(self.hw,0x200,0,xy);time.sleep(.035)
  assert u.PostMessageW(self.hw,0x201,1,xy);time.sleep(.04);assert u.PostMessageW(self.hw,0x202,0,xy);time.sleep(.13)
 def key(self,key):
  u.PostMessageW(self.hw,0x100,key,1);u.PostMessageW(self.hw,0x101,key,0xC0000001);time.sleep(.12)
 def photo(self,n):
  file=OUT/(n+'-control.png');capture(self.hw,file);r=W.RECT();c=W.RECT();pt=W.POINT();u.GetClientRect(self.hw,C.byref(c));u.GetWindowRect(self.hw,C.byref(r));u.ClientToScreen(self.hw,C.byref(pt));x,y=pt.x-r.left,pt.y-r.top;im=Image.open(file).convert('RGB').crop((x,y,x+c.right,y+c.bottom)).resize((round(c.right/self.scale),round(c.bottom/self.scale)),Image.Resampling.LANCZOS);im.save(file);return im
 def green(self,n):
  im=self.photo(n);mask=Image.new('L',im.size);mask.putdata([255 if 7<=r<45 and 145<g<206 and 96<b<161 and g-r>108 else 0 for r,g,b in im.getdata()]);cols,rows=mask.size;matches=[]
  for y in range(rows//2,rows-12):
   xs=[x for x in range(cols//2,cols-16) if mask.getpixel((x,y))];run=[]
   for x in xs+[9999]:
    if run and x!=run[-1]+1:
     if 38<=len(run)<=95:matches.append((len(run),int((run[-1]+run[0])/2),y));
     run=[]
    run.append(x)
  assert matches,'no identifiable native primary button '+n;best=max(matches);self.click(best[1],best[2]-1)
 def exact(self,x,y,cmd):self.click(x,y);self.expect(lambda v:v==cmd)
 def ok(self,n):self.results.append(n);print('PASS',n,flush=True)
 def run(self):
  self.exact(415,165,{'SetLanguage':'En'});self.ok('language segmented button');self.exact(319,277,{'SetStyle':'Number'});self.ok('tray style segmented button');self.exact(478,420,{'SetLow':30});self.ok('threshold plus');self.exact(475,556,'AutoUpdate');self.ok('updates switch');self.exact(475,630,'Autostart');self.ok('autostart switch (IPC only)');self.click(470,350);self.photo('Priority-dropdown');self.click(421,434);self.expect(lambda v:v=={'SetPriority':'Lowest'});self.ok('priority dropdown -> lowest');
  self.open('Flyout');self.click(53,76);v=self.expect(lambda v:isinstance(v,dict) and 'Sync'in v);token,scan=v['Sync'];assert scan;self.state['manual']={'id':token,'hid':True,'bt':True,'error':None};self.update();self.ok('scan then matching completion');self.click(146,78);q=self.expect(lambda v:isinstance(v,dict)and 'DeviceRequest'in v)['DeviceRequest'];assert q['kind']=='Keyboard' and q['bind'] is None;self.photo('Keyboard-loading');self.state['keyboard_reply']={'id':q['id'],'result':{'Ok':None}};self.update();self.green('Keyboard-picker');b=self.expect(lambda v:isinstance(v,dict)and 'DeviceRequest'in v)['DeviceRequest'];assert b['kind']=='Keyboard' and b['bind']=='spare';self.state['keyboard_reply']={'id':b['id'],'result':{'Err':'test: receiver asleep'}};self.update();self.photo('Keyboard-bind-failure');self.ok('keyboard scan, select, confirm and hardware-error state');self.key(27)
  self.click(246,77);q=self.expect(lambda v:isinstance(v,dict)and 'DeviceRequest'in v)['DeviceRequest'];assert q['kind']=='Bluetooth';self.state['bluetooth_reply']={'id':q['id'],'result':{'Ok':None}};self.update();self.photo('Bluetooth-picker');self.key(27);self.ok('separate Bluetooth picker and Escape');
  self.click(332,277);im=self.photo('Remove-confirm');self.key(27);assert self.q.empty(),'cancel emitted a device command';self.click(332,277);self.green('Remove-confirm');self.expect(lambda v:v=={'Unbind':self.state['rows'][1]['device']['id']});self.ok('cancel/confirm unbind');self.click(334,129);time.sleep(.09);u.PostMessageW(self.hw,0x102,ord('X'),1);self.key(13);self.expect(lambda v:v=={'Alias':[self.state['rows'][0]['device']['id'],'X']});self.ok('inline rename via text input and Enter');
  self.click(312,24);time.sleep(.08);self.ok('gear toggles page in same native window' ) if windows(self.p.pid)[0][0]==self.hw else (_ for _ in ()).throw(AssertionError('HWND changed'));self.exact(282,164,{'SetLanguage':'En'});self.ok('settings reachable in flyout dimensions');
  self.open('Settings');self.wheel(-120*3,9);self.photo('Settings-bottom');self.click(205,650);self.expect(lambda v:v=='CheckUpdate');self.ok('scrolled lower settings: check updates');self.state['update']={'Available':'0.2.0'};self.state['release_notes']='Fix device sleep recovery.\n修复设备休眠恢复。';self.state['release_size']=10485760;self.update();self.photo('Update-available');self.green('Update-offer');self.expect(lambda v:v=='DownloadUpdate');self.ok('signed release offer: start action');self.state['update']={'Downloading':[3145728,10485760]};self.state['bytes_per_sec']=1048576;self.update();self.photo('Update-progress');self.key(27);self.click(205,650);self.photo('Update-progress-reopened');self.ok('transfer progress view can reopen');
 def wheel(self,delta,repeats):
  pt=W.POINT(round(200*self.scale),round(420*self.scale));u.ClientToScreen(self.hw,C.byref(pt));xy=((pt.y&65535)<<16)|(pt.x&65535)
  u.PostMessageW(self.hw,0x200,0,(round(420*self.scale)<<16)|round(200*self.scale))
  for _ in range(repeats):u.PostMessageW(self.hw,0x20a,(delta&65535)<<16,xy);time.sleep(.065)
  time.sleep(.2)
 def close(self):
  if self.p.poll() is None:self.emit('Exit');self.p.wait(6)
if __name__=='__main__':
 a=Probe(Path(sys.argv[1]).resolve());passed=False
 try:a.run();passed=True
 finally:
  if not passed:a.photo('Control-failure')
  a.close();(OUT/'control-probe.json').write_text(json.dumps(dict(ok=passed,hardware=False,tests=a.results,events=a.trace),ensure_ascii=False,indent=2),encoding='utf8')
 print(json.dumps(dict(ok=True,count=len(a.results),tests=a.results)))

"""Build-time only: exact legacy finite-state tray art. No font files, Python or SVG engine at runtime.
Rasterize at 128px just once per state. Bank entries are individually addressable, deduplicated zlib streams.
"""
from pathlib import Path
import ast, hashlib, struct,zlib, json
from PIL import Image, ImageDraw
ROOT=Path(__file__).resolve().parents[1]
OLD=ROOT.parent/'mouse-battery/tray_icons.py'; sizes=(32,40,48,56,64,80,96,128);styles=('capsule','vertical','number','circle')
def load():
 tree=ast.parse(OLD.read_text(encoding='utf8'));tree.body=[n for n in tree.body if not isinstance(n,ast.ImportFrom) or n.module!='config'];ns={'TRAY_ICON_STYLE_'+k.upper():k for k in styles};exec(compile(tree,str(OLD),'exec'),ns);return ns['create_battery_icon']
if __name__=='__main__':
 draw=load();size=len(styles)*2*2*102*len(sizes);table=bytearray(size*8);data=bytearray();seen={};i=0
 for st in styles:
  for dark in (False,True):
   for charge in (False,True):
    for pct in range(-1,101):
     src=draw(pct,charge,style=st,is_dark_taskbar=dark,size=128)
     for n in sizes:
      im=src if n==128 else src.resize((n,n),Image.Resampling.LANCZOS);raw=im.tobytes();h=hashlib.sha256(raw).digest()
      if h not in seen:
       blob=zlib.compress(raw,6);seen[h]=(len(data),len(blob));data+=blob
      struct.pack_into('<II',table,i*8,*seen[h]);i+=1
 assert i==size;out=ROOT/'gui/assets/tray.bank';out.write_bytes(table+data);print('states',size,'unique',len(seen),'bytes',out.stat().st_size)
 (ROOT/'gui/assets/tray-assets.json').write_text(json.dumps({'source_sha256':hashlib.sha256(OLD.read_bytes()).hexdigest(),'file_sha256':hashlib.sha256(out.read_bytes()).hexdigest(),'entries':i,'sizes':sizes,'styles':styles,'unique':len(seen)},indent=2))

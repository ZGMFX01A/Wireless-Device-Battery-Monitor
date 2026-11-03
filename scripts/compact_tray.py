"""Pack a development tray.bank into independent lossless 16-cell pages. Verify every pixel."""
from pathlib import Path
import struct,io,zlib,hashlib,json
from PIL import Image
root=Path(__file__).resolve().parents[1];a=root/'gui/assets';p=a/'tray.bank';src=p.read_bytes();sizes=[32,40,48,56,64,80,96,128];n=4*2*2*102*8;pages=16*8*7;idx=bytearray(pages*8);assets=bytearray();seen={};pageid=0
for group in range(16):
 for si,w in enumerate(sizes):
  for page in range(7):
   im=Image.new('RGBA',(4*w,4*w));cells=[]
   for j in range(16):
    pct=page*16+j
    if pct>=102:break
    i=(group*102+pct)*8+si;o,l=struct.unpack_from('<II',src,i*8);raw=zlib.decompress(src[n*8+o:n*8+o+l]);cells.append(raw);im.paste(Image.frombytes('RGBA',(w,w),raw),(j%4*w,j//4*w))
   h=hashlib.sha256(im.tobytes()).digest()
   if h not in seen:
    b=io.BytesIO();im.save(b,'WEBP',lossless=True,exact=True,method=3);blob=b.getvalue();decoded=Image.open(io.BytesIO(blob)).convert('RGBA');assert decoded.tobytes()==im.tobytes();seen[h]=(len(assets),len(blob));assets+=blob
   struct.pack_into('<II',idx,pageid*8,*seen[h]);pageid+=1
assert pageid==pages;(a/'tray.pages').write_bytes(idx+assets);print('webp pages',pages,'unique',len(seen),'size',len(idx)+len(assets));meta=json.loads((a/'tray-assets.json').read_text());meta.update(page_count=pages,page_sha256=hashlib.sha256(idx+assets).hexdigest(),verified_entries=n);(a/'tray-assets.json').write_text(json.dumps(meta,indent=2));cache=root/'out'/'asset-build';cache.mkdir(exist_ok=True);p.replace(cache/p.name)

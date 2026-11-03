"""Development-only comparison against the original private Python decoders; fake transport, no hardware."""
import hashlib,sys,types,json,logging,subprocess
from pathlib import Path
from unittest.mock import patch
from types import SimpleNamespace
sys.dont_write_bytecode=True;logging.disable(logging.CRITICAL)
r=Path(__file__).resolve().parents[1];old=r.parent/'mouse-battery-core';sys.path.insert(0,str(old));hid=types.ModuleType('hid');hid.device=type('NoRealHid',(),{'__init__':lambda self:(_ for _ in ()).throw(AssertionError('real HID not permitted'))});sys.modules['hid']=hid
from mouse_battery_core import razer_hid as rz,asus_hid as rog,asus_models as models,keyboard_hid as keys
rows=[]
# Read the real Python transaction factory + level conversion, using a mock reply for each raw byte.
m=rz.RazerDevice(dict(path=b'fixture',product_id=0x007b));
for raw in range(256):
 def response(request):
  assert request==rz._build_razer_report(7,0x80,2,0xff);return bytes(9)+bytes([raw])+bytes(80)
 m._send_report=response;level=m.get_battery_level();rows.append(f'R {raw} {level}')
# Exercise the actual original model methods with the same structured raw payload as the Rust decoder.
for pid in [0x18a0,0x1a94]:
 layout=models.ASUS_DIRECT_MOUSE_LAYOUTS[pid];mouse=rog.AsusMouseDevice(dict(path=b'x',product_id=pid),layout,'test');mouse._device=object()
 for raw in [0,1,2,3,4,5,17,50,99,100,101,255]:
  for c in [0,1,2]:
   b=bytearray(16);b[:2]=bytes([0x12,7]);b[4]=raw;b[9]=c
   with patch.object(rog,'_transact',return_value=bytes(b)):sample=mouse.get_battery()
   rows.append(f'A {layout.percentage_multiplier} {raw} {c} {sample.percentage if sample else -1} {int(sample.charging) if sample else 0}')
for raw in range(101):
 for c in [0,1,2]:
  b=bytearray(48);b[4:8]=bytes.fromhex('450cf9fe');b[17]=raw;b[18]=c;v=keys._parse_device_info_payload(bytes(b));rows.append(f'K {raw} {c} {v.battery_level} {v.charge_status}')
f=r/'core/tests/fixtures/vendor-oracle.txt';f.write_text('\n'.join(rows)+'\n',encoding='ascii');info={'source_revision':subprocess.check_output(['git','-C',str(old),'rev-parse','HEAD'],text=True).strip(),'record_source':'Original Python code; synthetic transport inputs, no physical devices or capture','counts':{k:sum(row.startswith(k+' ') for row in rows) for k in 'RAK'},'sha256':hashlib.sha256(f.read_bytes()).hexdigest()};(f.parent/'vendor-oracle.json').write_text(json.dumps(info,indent=2));print(info)

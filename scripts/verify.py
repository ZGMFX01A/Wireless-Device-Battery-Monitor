"""Windows developer regression. Hardware, original settings and HKCU Run are never used.
Run from any folder with Python/Pillow, websockets, Edge (legacy reference only) and the Rust MSVC toolchain installed.
"""
import json,hashlib,os,subprocess,sys,re,shutil,datetime
from pathlib import Path
import argparse, tomllib
package_version=tomllib.loads((Path(__file__).resolve().parents[1]/'gui/Cargo.toml').read_text(encoding='utf8'))['package']['version']
parser=argparse.ArgumentParser();parser.add_argument('--out-dir',default='out/GPUI-'+package_version,help='Distinct release destination; do not replace a running or previous candidate.')
root=Path(__file__).resolve().parents[1];os.chdir(root);out=(root/parser.parse_args().out_dir).resolve();out.relative_to(root);e=out/'evidence';os.environ['GPUI_VERIFY_UI_DIR']=str(e/'ui-parity');e.mkdir(parents=True,exist_ok=True);exe=root/'target/release/mouse-battery-gpui.exe';report={}
def run(name,args,time=180):
    res=subprocess.run(list(map(str,args)),capture_output=True,encoding='utf-8',errors='replace',timeout=time);txt=res.stdout+res.stderr;(e/(name+'.log')).write_text(txt,encoding='utf-8');report[name]={'exit_code':res.returncode,'command':list(map(str,args))};print(name,res.returncode,txt[-470:],flush=True)
    if res.returncode:raise RuntimeError(name+': '+txt[-1300:])
    return res.stdout
try:
    run('format',['cargo','fmt','--all','--','--check']);run('clippy',['cargo','clippy','--workspace','--all-targets','--locked','--','-D','warnings'],1200);unit=run('unit',['cargo','test','--locked','--workspace'],1200);report['unit']['passed']=sum(map(int,re.findall(r'test result: ok\. (\d+) passed',unit)));assert report['unit']['passed']>=31
    run('build',['cargo','build','--release','--locked','-j','6'],2400);run('smoke',[exe,'--smoke-test']);tree=run('deps',['cargo','tree','--workspace','-e','normal','--locked']);assert not re.search(r'\b(?:tauri|wry|webview2-com|pyo3|pythonnet) v\d',tree,re.I)
    for name,script in [('window','window_probe.py'),('tray','tray_probe.py'),('controls','control_probe.py'),('hung_reap','hung_reap_probe.py')]:
        data=run(name,[sys.executable,'gui/tests/'+script,exe]);report[name]['results']=json.loads(data.strip().splitlines()[-1]);assert report[name]['results']['ok']
    g=run('glass',[sys.executable,'gui/tests/glass_probe.py',exe]);report['glass']['results']=json.loads(g.strip().splitlines()[-1]);assert report['glass']['results']['control_changes_backdrop'] and report['glass']['results']['blur_observed']
    v=run('visual',[sys.executable,'gui/tests/visual_compare.py',exe],180);report['visual']['results']=json.loads(v.strip().splitlines()[-1]);assert len(report['visual']['results'])==6
    for name,case in report['visual']['results'].items():assert case['logical']==([380,540] if name.endswith('Flyout') else [540,680])
    baseline=root/'out/legacy-safety/178e5c5_17f7634';manifest=json.loads((baseline/'manifest.json').read_text(encoding='utf8'));guard={}
    for repo in ('mouse-battery','mouse-battery-core'):
        info=manifest[repo];rc=lambda *args: subprocess.check_output(['git','-C',str(root.parent/repo),*args],encoding='utf8');assert rc('rev-parse','HEAD').strip()==info['head'];assert rc('status','--porcelain')==info['status'];bundle=baseline/(repo+'.bundle');assert hashlib.sha256(bundle.read_bytes()).hexdigest()==info['bundle_sha256'];subprocess.run(['git','-C',str(root.parent/repo),'bundle','verify',str(bundle)],check=True,capture_output=True);guard[repo]=info['head']
    old=manifest['previous_exe'];assert hashlib.sha256((root.parent/old['path']).read_bytes()).hexdigest()==old['sha256'];guard['old_exe_sha256']=old['sha256'];report['legacy_guard']=guard
    run('screens',[sys.executable,'gui/tests/render_probe.py',exe,e]);dst=out/exe.name;shutil.copyfile(exe,dst);report['artifact']={'path':dst.relative_to(root).as_posix(),'bytes':dst.stat().st_size,'sha256':hashlib.sha256(dst.read_bytes()).hexdigest()};report['physical_devices_tested']=False;report['ok']=True
finally:
    (e/'verification.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')

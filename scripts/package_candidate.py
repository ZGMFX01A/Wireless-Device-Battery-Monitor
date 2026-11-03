"""Package only the verified Windows executable and its own real-window evidence."""
import argparse, base64, hashlib, html, json, os, tomllib, zipfile
from pathlib import Path
root = Path(__file__).resolve().parents[1]
version = tomllib.loads((root/'gui/Cargo.toml').read_text(encoding='utf8'))['package']['version']
parser=argparse.ArgumentParser(); parser.add_argument('--out-dir',default='out/GPUI-'+version)
out=(root/parser.parse_args().out_dir).resolve();out.relative_to(root/'out')
ev = out / 'evidence'
ui = ev / 'ui-parity'
v = json.loads((ev/'verification.json').read_text(encoding='utf8'))
exe = out/'mouse-battery-gpui.exe'
sha = hashlib.sha256(exe.read_bytes()).hexdigest()
assert v.get('ok') and v['unit']['passed']>=34 and v['controls']['results']['ok']
assert sha == v['artifact']['sha256'] == hashlib.sha256((root/'target/release'/exe.name).read_bytes()).hexdigest()
g = v['glass']['results']; q = v['tray']['results']; w = v['window']['results']; count = v['controls']['results']['count']
assert g['blur_observed'] and g['control_changes_backdrop'] and len(v['visual']['results'])==6
report = f'''# GPUI {version} 本机验收记录

Windows x64 候选 `{exe.name}`；{exe.stat().st_size:,} 字节。SHA-256 `{sha}`。

| 检查 | 实际结果 |
|---|---|
| 构建 | release、fmt、严格 Clippy、无硬件 smoke、依赖树通过；运行不使用 Python/Tauri/WebView2。 |
| 自动测试 | {v['unit']['passed']} 项 Rust 测试；旧协议离线对照共 3,632 组输入：罗技 3,001、雷蛇 256、ROG 72、公版键盘 303。不是实物设备数。 |
| 界面操作 | {count} 项 GPUI 实窗操作检查。按钮、选择、编辑、失败和取消通过实际 HWND 操作；后端响应使用隔离测试数据，不改系统自启动或采集真实硬件。 |
| 窗口生命周期 | 显示 {w['presents']} 次、隐藏 {w['hides']} 次；同一窗口复用。托盘用例中闲置 {q['idle_exit_s']} 秒后 GUI 退出；回收及模拟 GUI 崩溃后都能重开。 |
| 原版视觉对照 | 未修改的旧 HTML/CSS/JS 由 Edge 加载相同测试快照，右侧为 GPUI 实窗。共 6 对，150% 缩放；浮窗 380 × 540、设置大窗 540 × 680 逻辑像素。原版参考图不包含旧 EXE 的 DWM 效果。 |
| 桌面磨砂 | 属性 {g['dwm_material']}；受控红/蓝背板在 0%、25%、100% 下响应不同；4 物理像素条纹的对比度比率 {g['contrast_ratio']}，验证背后细节被模糊。 |
| 托盘绘制 | 单图标、原四种样式、8 档尺寸，13,056 个源状态组合；压缩时逐像素核验。 |
| 更新与回退 | 签名/大小/完整性校验及临时目录更新交换、健康失败与冲突回退覆盖；没有使用生产签名通道或替换用户程序。 |
| 原版保护 | 原两个仓库 HEAD、工作树状态和备份 bundle 均核对，旧 EXE 的 SHA-256 不变。 |

未实测：各型号真实 HID/BLE/HFP、物理睡眠/唤醒、混合 DPI、多显卡、Windows 10 材质回退、8 小时真机长测及真实发布通道升级。不同渲染器不承诺逐像素相同，不能从本组测试推断所有固件均兼容。
'''
(ev/'RESULTS.md').write_text(report,encoding='utf8')
def image(path,caption):
    src='data:image/png;base64,'+base64.b64encode(path.read_bytes()).decode('ascii')
    return '<figure><figcaption>'+html.escape(caption)+'</figcaption><img src="'+src+'" alt="'+html.escape(caption)+'"></figure>'
style='body{margin:0;background:#f7f8fa;color:#192534;font:16px/1.6 system-ui,"Microsoft YaHei",sans-serif}main{max-width:1160px;margin:auto;padding:30px 24px}h1{font-size:30px;margin:.25em 0}h2{font-size:21px;margin-top:30px}small,figcaption{color:#495b6d}details{padding:12px 16px;margin:8px 0;background:white;border:1px solid #d7dfe6;border-radius:10px}summary{cursor:pointer;font-weight:600}.pair{display:flex;flex-wrap:wrap;gap:16px}.pair figure{flex:1 1 330px}figure{margin:8px 0}img{max-width:100%;height:auto;display:block}aside{background:#e6f4ef;padding:12px 18px;border-left:3px solid #078567}code{overflow-wrap:anywhere}'
body=['<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>原版与 GPUI 验收对照</title><style>'+style+'</style><main><small>无线设备电量监控 / Windows x64</small><h1>原版与 GPUI '+version+' 界面对照</h1><p>同一组模拟设备、电量和主题。左侧为原版前端；右侧为候选 EXE 的实际 GPUI 窗口。测试设备不是你的外设。</p><nav><a href="#pair">界面对照</a>　<a href="#control">交互状态</a>　<a href="#how">试用与回退</a></nav><h2 id="pair">01　设备浮窗与设置大窗</h2>']
# Use the actual composed HWND images, not the legacy HTML oracle, for this native-to-native comparison.
card=ev/'card-material'
if card.is_dir():
    before=json.loads((card/'before/capture.json').read_text(encoding='utf8'));after=json.loads((card/'after/capture.json').read_text(encoding='utf8'))
    assert after['exe_sha256']==sha and before['exe_sha256']!=sha and before['px']==after['px'] and before['dpi']==after['dpi'] and before['values']==after['values']
    header=body[0]; marker='<h2 id="pair">'; h,tail=header.split(marker,1);body=[h,'<h2>本次改动：设备卡片</h2><p>左为 GPUI 0.1.1，右为 GPUI '+version+'。同一台 Windows 机器、同一测试背板、同一倍率；图像来自实际窗口。设备名称与电量为隔离测试输入。</p>']
    for name,label in [('Light-daily','浅色 · 三台在线'),('Dark-daily','深色 · 三台在线'),('Light-states','浅色 · 低电、充电、离线'),('Dark-states','深色 · 低电、充电、离线')]:
        body.append('<details '+('open' if name=='Light-daily' else '')+'><summary>'+label+'</summary><div class="pair">'+image(card/'before'/(name+'.png'),'改前 · GPUI 0.1.1')+image(card/'after'/(name+'.png'),'改后 · GPUI '+version)+'</div></details>')
    body.append(marker+tail)
for key,case in v['visual']['results'].items():
    theme,language,mode=key.split('-');label=' · '.join(['深色' if theme=='Dark' else '浅色','中文' if language=='Zh' else '英文','设备浮窗' if mode=='Flyout' else '设置大窗']);dim=' × '.join(map(str,case['logical']))
    body.append('<details '+('open' if key.startswith('Dark-Zh-') else '')+'><summary>'+label+'　'+dim+'</summary><div class="pair">'+image(ui/(key+'-legacy.png'),'原版 HTML / CSS / JS')+image(ui/(key+'-gpui.png'),'GPUI 原生窗口')+'</div></details>')
body.append('<h2 id="control">02　实窗交互</h2><p>以下截图来自对 GPUI 窗口的操作。扫描/连接和更新状态由测试端提供。</p>')
for key,label in [('Priority-dropdown','托盘低电量优先选择'),('Settings-bottom','主题、通透度和更新入口'),('Keyboard-picker','键盘绑定'),('Keyboard-bind-failure','读取失败，不记录为绑定成功'),('Bluetooth-picker','蓝牙绑定'),('Remove-confirm','解绑需要确认'),('Update-available','版本与更新说明'),('Update-progress','下载进度与取消')]:
    body.append('<details><summary>'+label+'</summary>'+image(ui/(key+'-control.png'),label)+'</details>')
body.append('<h2 id="how">03　用同一个 EXE 验收</h2><aside><b>真实采集：从托盘正常退出旧版和演示实例，双击 mouse-battery-gpui.exe，不带参数。</b><br>只看样式可以使用 --demo-tray，不会访问真实外设。</aside><p>单击托盘检查设备；右键进入设置，试明暗主题和 0% / 25% / 100% 通透度。让浮窗收起超过 30 秒，再分别打开浮窗和设置。键盘与蓝牙应从各自的入口绑定，失败不应保存。</p><p>新旧配置分离。导入旧版设置只读访问原配置，按唯一身份匹配绑定和别名。要回退，退出 GPUI，再运行原目录的 mouse-battery\\dist\\WirelessDeviceBatteryMonitor.exe。</p><p>未覆盖真机型号、长测、系统睡眠、所有显卡和生产更新；详细边界见 evidence/RESULTS.md。未切换默认版本。</p><small>候选 SHA-256：<code>'+sha+'</code></small></main></html>')
gallery=out/'验收对照.html';gallery.write_text(''.join(body),encoding='utf8');readme=out/'如何验证.txt';readme.write_text('GPUI '+version+' / Windows x64\n\n打开 验收对照.html 查看并排对照。\n真实读电：正常退出旧版和演示托盘后，双击 mouse-battery-gpui.exe，不带参数。\n只试界面：mouse-battery-gpui.exe --demo-tray。\n回退：退出新版，运行原 mouse-battery\\dist\\WirelessDeviceBatteryMonitor.exe。\n正在分发的候选未注入生产更新公钥。\n\nSHA-256 '+sha,encoding='utf-8-sig')
archive=out/f'GPUI-{version}-Windows-x64-验收包.zip';tmp=archive.with_suffix('.tmp')
with zipfile.ZipFile(tmp,'w',zipfile.ZIP_DEFLATED,compresslevel=6) as z:
    for f in [exe,gallery,readme]:z.write(f,f.name)
    for f in [ev/'RESULTS.md',ev/'verification.json',ui/'glass.json',ui/'capture.json',ui/'control-probe.json',root/'gui/assets/tray-assets.json']:z.write(f,'evidence/'+f.name)
    if (ev/'AUDIT.md').is_file():
        for name in ('AUDIT.md','red-before.log','unit.log','hung_reap.log','controls.log','tray.log','window.log'):
            z.write(ev/name,'evidence/'+name)
    if card.is_dir():
        for f in sorted(card.rglob('*')):
            if f.is_file():z.write(f,f.relative_to(out).as_posix())
    z.write(root.parent/'mouse-battery/LICENSE','LICENSE.txt');z.write(root/'gui/third-party/gpui-LICENSE-APACHE.txt','licenses/GPUI-APACHE.txt')
with zipfile.ZipFile(tmp) as z:assert z.testzip() is None and hashlib.sha256(z.read(exe.name)).hexdigest()==sha
os.replace(tmp,archive);print(json.dumps({'exe':str(exe),'sha256':sha,'bytes':exe.stat().st_size,'gallery':str(gallery),'zip':str(archive),'zip_bytes':archive.stat().st_size,'unit':v['unit']['passed'],'controls':count,'glass':g['blur_observed']},ensure_ascii=False))

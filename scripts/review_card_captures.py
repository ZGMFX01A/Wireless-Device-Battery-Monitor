"""Summarize matched native card captures; no editing of screenshots or desktop access."""
import argparse
from collections import Counter
import json
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont


def luminance(rgb):
    v=[x/255 for x in rgb]
    return sum(w*(c/12.92 if c <= .04045 else ((c+.055)/1.055)**2.4) for w,c in zip((.2126,.7152,.0722),v))

def ink_contrast(file, panel_rgb):
    with Image.open(file) as im:
        w,h=im.size
        roi=im.crop(tuple(round(v*scale) for v,scale in zip((270,145,337,180),(w/380,h/540,w/380,h/540))))
        full=Counter(c for c in roi.convert('RGB').get_flattened_data() if c[1]-c[0]>30 and c[1]-c[2]>12)
    assert full, 'No healthy percentage ink present in expected first-card region'
    ink,n=full.most_common(1)[0];assert n>=50, 'percentage glyph core not resolved'
    a,b=sorted([luminance(ink),luminance(panel_rgb)])
    return dict(ink=ink,card=panel_rgb,contrast=round((b+.05)/(a+.05),2))

def review(root):
    old,new=[json.loads((root/s/'capture.json').read_text(encoding='utf8')) for s in ['before','after']]
    for key in ['dpi','px','values','rows']:assert old[key]==new[key],key
    assert len(old['rows'])==3 and old['exe_sha256']!=new['exe_sha256'];result={}
    for theme in ['Light','Dark']:
        tag=theme+'-daily';res={}
        for sub,record in [('before',old),('after',new)]:
            surf=record['material'][tag];res[sub]=ink_contrast(root/sub/(tag+'.png'),surf['card_upper']);res[sub]['card_minus_window_luma']=round(255*(luminance(surf['card_upper'])-luminance(surf['window'])),2)
        assert res['after']['contrast']>=4.5, (theme,res)
        assert res['after']['card_minus_window_luma']>0, (theme,'card should read as a surface above its backplate')
        result[theme]=res
        with Image.open(root/'before'/(tag+'.png')) as b,Image.open(root/'after'/(tag+'.png')) as a:
            w,h=a.size;assert b.size==a.size
            full=Image.new('RGB',(w*2+18,h+46),'#eff2f5');d=ImageDraw.Draw(full);font=ImageFont.truetype('segoeui.ttf',20)
            for j,(img,label) in enumerate([(b,'0.1.1  /  Before'),(a,'0.1.2  /  After')]):full.paste(img,(j*(w+18),46));d.text((j*(w+18)+10,9),label,font=font,fill='#28313a')
            full.save(root/(theme+'-comparison.png'))
    (root/'measurements.json').write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf8');print(json.dumps(result))

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('folder',type=Path);review(parser.parse_args().folder)

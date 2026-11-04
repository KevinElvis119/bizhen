import json, sys
from PIL import Image, ImageDraw
src = sys.argv[1]; dst = sys.argv[2]
d = json.load(open(src, encoding="utf-8"))
PW, PH = d["page"]["width"], d["page"]["height"]
DPI = 150.0 / 25.4   # px per mm
pages = d["pages"]
gap = 6
W = int(PW * DPI)
H = int((PH * len(pages) + gap * (len(pages)-1)) * DPI)
img = Image.new("RGB", (W, H), (245, 245, 245))
dr = ImageDraw.Draw(img)
for i, pg in enumerate(pages):
    oy = i * (PH + gap) * DPI
    dr.rectangle([0, oy, W, oy + PH*DPI], fill=(255,255,255), outline=(180,180,180))
    for s in pg["strokes"]:
        pts = [(x*DPI, oy + (PH - y)*DPI) for x, y in s["points"]]
        if len(pts) < 2: continue
        dr.line(pts, fill=(20,20,20), width=2, joint="curve")
img.save(dst)
print(f"saved {dst} {W}x{H}, pages={len(pages)}, strokes={sum(len(p['strokes']) for p in pages)}")

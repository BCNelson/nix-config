# Build SpoolSans-Regular.ttf: Noto Sans (static wght 400, subset) plus a few
# marker glyphs from Noto Sans Symbols 2 / Noto Sans Math (CFF -> quadratic).
# All sources are SIL OFL 1.1 (see OFL.txt). Sources: nixpkgs noto-fonts
# 2026.09.01 NotoSans.ttf, NotoSansSymbols2-Regular.otf,
# NotoSansMath-Regular.otf, NotoSansSymbols.ttf (in that order).
# usage: build.py NotoSans.ttf out.ttf SYMBOLFONT...
import sys
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer
from fontTools import subset
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.pens.cu2quPen import Cu2QuPen

noto, out = sys.argv[1:3]
syms = sys.argv[3:]
f = TTFont(noto)
f = instancer.instantiateVariableFont(f, {"wght": 400, "wdth": 100})
ranges = []
def r(a, b): ranges.extend(range(a, b + 1))
r(0x20, 0x7E); r(0xA0, 0x24F); r(0x250, 0x36F)
r(0x370, 0x3FF); r(0x400, 0x52F); r(0x1E00, 0x1EFF); r(0x2000, 0x206F)
r(0x20A0, 0x20C0); r(0x2100, 0x214F); r(0x2190, 0x22FF)
r(0x25A0, 0x25FF); r(0xFFFD, 0xFFFD)
opts = subset.Options()
opts.layout_features = ["*"]
opts.name_IDs = ["*"]
opts.notdef_outline = True
opts.hinting = False
s = subset.Subsetter(opts)
s.populate(unicodes=ranges)
s.subset(f)

extra = [0x2400 + i for i in range(0x25)] + [0x2605, 0x2606, 0x27EA, 0x27EB, 0x23CE, 0x2192, 0x21B5]
cmap = f.getBestCmap()
glyf = f["glyf"]; hmtx = f["hmtx"]
added = []
for sym in syms:
    sf = TTFont(sym)
    scm = sf.getBestCmap()
    gs = sf.getGlyphSet()
    for cp in extra:
        if cp in cmap or cp in added or cp not in scm:
            continue
        src = scm[cp]
        name = "spool_uni%04X" % cp
        tp = TTGlyphPen(None)
        gs[src].draw(Cu2QuPen(tp, 1.0, reverse_direction=True))
        g = tp.glyph()
        glyf[name] = g
        g.recalcBounds(glyf)
        hmtx[name] = (sf["hmtx"][src][0], getattr(g, "xMin", 0))
        for t in f["cmap"].tables:
            if t.isUnicode():
                t.cmap[cp] = name
        added.append(cp)
f.setGlyphOrder(list(glyf.glyphOrder))
f["maxp"].numGlyphs = len(glyf.glyphOrder)
for rec in f["name"].names:
    if rec.nameID in (1, 4, 16):
        rec.string = "Spool Sans"
    elif rec.nameID == 6:
        rec.string = "SpoolSans-Regular"
f.save(out)
print("added", "".join(chr(c) for c in added), "missing",
      "".join(chr(c) for c in extra if c not in added and c not in cmap))

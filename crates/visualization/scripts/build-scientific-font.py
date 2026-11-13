#!/usr/bin/env python3
"""Build a pinned Latin/Greek + Simplified Chinese font without runtime downloads.

Development dependency: fonttools[woff]==4.63.0. Pass the original Google Fonts
NotoSansSC[wght].ttf downloaded at SOURCE_URL; its SHA256 is verified before use.
The Latin component is the original Liberation TTF shipped for BioIcons rendering.
"""
import argparse, copy, hashlib, json, tempfile
from pathlib import Path
import fontTools
from fontTools import subset
from fontTools.merge import Merger
from fontTools.ttLib import TTFont
from fontTools.ttLib.scaleUpem import scale_upem
from fontTools.varLib.instancer import instantiateVariableFont
ROOT = Path(__file__).resolve().parents[3]
COMMIT = 'a85815a42757630ce188fdad368c2dfc444d4773'
SOURCE_URL = f'https://raw.githubusercontent.com/google/fonts/{COMMIT}/ofl/notosanssc/NotoSansSC%5Bwght%5D.ttf'
SOURCE_SHA = 'a3041811a78c361b1de50f953c805e0244951c21c5bd412f7232ef0d899af0da'
# Enclosed alphanumerics cover common figure labels such as ① and ②.
RANGES = [(0x2460,0x24ff),(0x2e80,0x2fff),(0x3000,0x303f),(0x3100,0x312f),(0x31a0,0x31bf),(0x3400,0x9fff),(0xf900,0xfaff),(0xfe10,0xfe1f),(0xfe30,0xfe4f),(0xff00,0xffef),(0x20000,0x323af)]
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('source',type=Path)
parser.add_argument('--output',type=Path,default=ROOT/'apps/gui/public/excalidraw/fonts/Liberation/LiberationSans-Regular.woff2')
args=parser.parse_args()
assert args.output.parent.resolve() != args.source.parent.resolve(), 'Keep build output separate from upstream source files'
assert fontTools.__version__ == '4.63.0', 'Use fonttools[woff]==4.63.0 for reproducible output'
assert hashlib.sha256(args.source.read_bytes()).hexdigest()==SOURCE_SHA, 'Unrecognized Noto font bytes'
ofl=args.source.with_name('OFL.txt').read_bytes()
assert hashlib.sha256(ofl).hexdigest()=='1c05c68c34f9708415aada51f17e1b0092d2cea709bf4a94cd38114f9e73d7d9', 'Unrecognized Noto license'
latin_path=ROOT/'crates/bioicons/fonts/LiberationSans-Regular.ttf'
latin=TTFont(latin_path,recalcTimestamp=False)
latin_cmap=latin.getBestCmap()
vertical={table:{key:getattr(latin[table],key) for key in keys if hasattr(latin[table],key)} for table,keys in {'hhea':['ascent','descent','lineGap'],'OS/2':['sTypoAscender','sTypoDescender','sTypoLineGap','usWinAscent','usWinDescent','sxHeight','sCapHeight']}.items()}
font=instantiateVariableFont(TTFont(args.source,recalcTimestamp=False),{'wght':400},inplace=True)
extra=sorted(cp for cp in font.getBestCmap() if cp not in latin_cmap and any(a<=cp<=b for a,b in RANGES))
options=subset.Options();options.layout_features=[];options.recalc_timestamp=False
subsetter=subset.Subsetter(options=options);subsetter.populate(unicodes=extra);subsetter.subset(font)
# Use upright SC cmap glyphs, without variable, locale, vertical or proportional
# alternates. Keep the original Latin/Greek layout and pair kerning untouched.
for tag in ['GDEF','GPOS','GSUB','BASE','STAT','vhea','vmtx','VORG']:
    if tag in font: del font[tag]
scale_upem(font,latin['head'].unitsPerEm)
font.recalcTimestamp=False
with tempfile.TemporaryDirectory(prefix='autonomics-font-') as directory:
    cjk=Path(directory)/'cjk.ttf';font.save(cjk)
    merged=Merger().merge([str(latin_path),str(cjk)])
# With CJK layout features intentionally omitted, preserve the complete original
# shaping/kerning tables. Merger drops the legacy kern table by default.
assert merged.getGlyphOrder()[:len(latin.getGlyphOrder())] == latin.getGlyphOrder()
for tag in ['kern','GDEF','GPOS','GSUB']:
    merged[tag]=copy.deepcopy(latin[tag])
    assert merged[tag].compile(merged)==latin[tag].compile(latin), f'Latin shaping changed: {tag}'
merged.recalcTimestamp=False
merged['head'].created=latin['head'].created;merged['head'].modified=latin['head'].modified
for table,values in vertical.items():
    for key,value in values.items(): setattr(merged[table],key,value)
name='Autonomics Scientific Sans'
replacements={1:name,2:'Regular',3:'AutonomicsScientificSans-Regular-1.0',4:name+' Regular',5:'Version 1.000',6:'AutonomicsScientificSans-Regular',16:name,17:'Regular',21:name,22:'Regular',25:'AutonomicsScientificSans'}
for record in merged['name'].names:
    if record.nameID in replacements:
        record.string=replacements[record.nameID].encode(record.getEncoding())
for name_id,value in replacements.items(): merged['name'].setName(value,name_id,3,1,0x409)
copyright=latin['name'].getDebugName(0)+'\nCopyright 2014-2021 Adobe (http://www.adobe.com/).\nModified font: Autonomics Scientific Sans; Latin/Greek from Liberation Sans, Chinese from Noto Sans SC Regular.'
merged['name'].setName(copyright,0,3,1,0x409)
merged['name'].setName('SIL Open Font License 1.1; see OFL.txt and NotoSansSC-OFL.txt distributed with this font.',13,3,1,0x409)
merged['name'].setName('https://openfontlicense.org/open-font-license-official-text/',14,3,1,0x409)
# Verify original glyph metrics and outline identities before shipping. Merger
# can rename duplicate glyph names, so compare through each font's cmap.
for cp,glyph in latin_cmap.items():
    target=merged.getBestCmap()[cp]
    assert merged['hmtx'].metrics[target]==latin['hmtx'].metrics[glyph], f'Latin metrics changed: U+{cp:04X}'
    assert merged['glyf'][target].compile(merged['glyf'])==latin['glyf'][glyph].compile(latin['glyf']), f'Latin outline changed: U+{cp:04X}'
merged.flavor='woff2';args.output.parent.mkdir(parents=True,exist_ok=True);merged.save(args.output)
args.output.with_name('NotoSansSC-OFL.txt').write_bytes(ofl)
args.output.with_name('OFL.txt').write_bytes((ROOT/'crates/bioicons/fonts/OFL.txt').read_bytes())
manifest={'family':name,'css_alias':'Liberation Sans','font_family_id':9,'fonttools':fontTools.__version__,'latin_source':'crates/bioicons/fonts/LiberationSans-Regular.ttf','latin_sha256':hashlib.sha256(latin_path.read_bytes()).hexdigest(),'cjk_source_url':SOURCE_URL,'cjk_source_sha256':SOURCE_SHA,'cjk_weight':400,'cjk_ranges':[[hex(a),hex(b)]for a,b in RANGES],'latin_codepoints':len(latin_cmap),'added_cjk_codepoints':len(extra),'glyphs':len(merged.getGlyphOrder()),'sha256':hashlib.sha256(args.output.read_bytes()).hexdigest()}
args.output.with_suffix('.sources.json').write_text(json.dumps(manifest,indent=2)+'\n')
print(json.dumps(manifest,indent=2))

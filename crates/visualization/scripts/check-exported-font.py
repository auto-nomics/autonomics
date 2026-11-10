#!/usr/bin/env python3
"""Audit an actual standalone SVG export: every text glyph must be embedded.
Development-only fonttools[woff] is required. No network or system-font lookup.
"""
import argparse,base64,io,re,xml.etree.ElementTree as ET
from fontTools.ttLib import TTFont
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('svg')
parser.add_argument('--require-chinese',action='store_true')
args=parser.parse_args()
root=ET.parse(args.svg).getroot()
text=''.join(''.join(node.itertext()) for node in root.iter('{http://www.w3.org/2000/svg}text'))
assert text, 'Export has no text elements'
if args.require_chinese: assert any(0x3400<=ord(c)<=0x9fff for c in text), 'No Chinese text in this export'
styles=''.join(''.join(node.itertext()) for node in root.iter('{http://www.w3.org/2000/svg}style'))
coverage={}
fonts=0
for rule in re.findall(r'@font-face\s*\{([^}]+)\}',styles):
    family=re.search(r'font-family\s*:\s*([^;]+)',rule)
    if not family: continue
    name=family.group(1).strip().strip('\'"')
    for encoded in re.findall(r'data:(?:font/woff2|application/font-woff2);base64,([A-Za-z0-9+/=]+)',rule):
        font=TTFont(io.BytesIO(base64.b64decode(encoded)))
        coverage.setdefault(name,set()).update(cp for cp,glyph in font.getBestCmap().items() if glyph!='.notdef')
        fonts+=1
assert fonts, 'No embedded WOFF2 font found'
missing=set()
for node in root.iter('{http://www.w3.org/2000/svg}text'):
    families=node.attrib.get('font-family')
    assert families, 'Audit requires explicit font-family on each text element'
    assert all('font-family' not in child.attrib for child in node.iter() if child is not node), 'Mixed-family text runs require individual audit'
    covered=set().union(*(coverage.get(name.strip().strip('\'"'),set()) for name in families.split(',')))
    missing.update({ord(c) for c in ''.join(node.itertext()) if not c.isspace()}-covered)
assert not missing, 'Text would depend on system fonts: '+','.join(f'U+{cp:04X}'for cp in sorted(missing))
print(f'export-font-audit: {fonts} embedded font(s), {len(set(text))} text codepoints, no system-font dependencies for the exported text')

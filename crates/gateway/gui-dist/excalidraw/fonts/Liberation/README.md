# Autonomics Scientific Sans

The `LiberationSans-Regular.woff2` file here is a modified font named **Autonomics Scientific Sans**, not the original upstream Liberation font. Excalidraw 0.18.1 resolves font family ID 9 through this fixed path and CSS alias `Liberation Sans`; the alias is retained for existing documents.

The original Liberation Sans Latin/Greek glyphs, advances, vertical metrics, kerning and shaping tables are preserved. The font adds 28,642 CJK codepoints from upright Noto Sans SC Regular (weight 400), with a total cmap of 30,969 codepoints. It is not universal Unicode coverage; missing glyphs and complex script shaping still need separate handling. Chinese glyphs use the SC forms. Japanese/Korean localized forms and vertical/proportional alternates are not added.

Components and notices:

- Original Latin font: `crates/bioicons/fonts/LiberationSans-Regular.ttf`, under the accompanying `OFL.txt`.
- [Noto Sans SC source](https://github.com/google/fonts/tree/a85815a42757630ce188fdad368c2dfc444d4773/ofl/notosanssc), under the unmodified `NotoSansSC-OFL.txt` notice. Source TTF SHA256: `a3041811a78c361b1de50f953c805e0244951c21c5bd412f7232ef0d899af0da`.
- `LiberationSans-Regular.sources.json` records source/version, selected Unicode ranges, fonttools version and the actual output checksum. Font name records identify the modified font and retain component attribution.

Developer reproduction requires `fonttools[woff]==4.63.0`. Download the pinned source TTF and its `OFL.txt` together into a directory outside the output directory, then run from the repository root:

```sh
python3 crates/visualization/scripts/build-scientific-font.py '/path/to/NotoSansSC[wght].ttf'
python3 crates/visualization/scripts/generate-font-metrics.py
npm --prefix apps/gui run build:gateway
```

The build checks upstream source/notice hashes and verifies original glyph outlines, advances and shaping/kerning tables. The backend uses metrics generated from the exact shipped WOFF2; generated labels record that font's name and checksum. The Gateway embeds this font and the component notices through an explicit static route. No runtime font download or system font is needed for codepoints in this font.

For an actual standalone SVG export, verify the embedded font subsets against the text and its declared font families:

```sh
python3 crates/visualization/scripts/check-exported-font.py figure.svg --require-chinese
```

The audit rejects missing text glyphs even if they would render through an installed system fallback. It currently requires explicit `font-family` on text elements and rejects mixed-family nested runs instead of guessing their inheritance.

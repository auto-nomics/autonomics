//! Minimal OOXML (.pptx) writer — the pptxgenjs image-based shape:
//! each slide is one full-bleed PNG background at exact page size
//! (EMU = px × 9525, inches = px / 96), speaker notes ride along as
//! real PowerPoint notes (`notesSlideN.xml`).
//!
//! Parts: `[Content_Types].xml`, `_rels/.rels`, `docProps/app.xml`,
//! `ppt/presentation.xml` (+rels), one slideMaster + slideLayout +
//! notesMaster + theme, `ppt/slides/slideN.xml` (+rels),
//! `ppt/notesSlides/notesSlideN.xml` (+rels), `ppt/media/imageN.png`.

use crate::error::{Error, Result};

/// EMU per CSS px (pptxgenjs `EMU_PER_PX`).
const EMU_PER_PX: u32 = 9525;

/// Build the presentation from full-bleed PNG frames.
///
/// `notes[i]` is the speaker note for `pngs[i]` (empty = no notes slide).
pub fn build_pptx(
    pngs: &[Vec<u8>],
    canvas_width: u32,
    canvas_height: u32,
    notes: &[String],
) -> Result<Vec<u8>> {
    if pngs.is_empty() {
        return Err(Error::Export("pptx needs at least one page".into()));
    }
    let cx = u64::from(canvas_width) * u64::from(EMU_PER_PX);
    let cy = u64::from(canvas_height) * u64::from(EMU_PER_PX);

    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let add = |zip: &mut zip::ZipWriter<std::io::Cursor<Vec<u8>>>, name: &str, data: &[u8]| -> Result<()> {
        zip.start_file(name, options)
            .map_err(|e| Error::Export(format!("pptx zip {name}: {e}")))?;
        std::io::Write::write_all(zip, data)
            .map_err(|e| Error::Export(format!("pptx zip write {name}: {e}")))
    };

    // [Content_Types].xml
    let mut ct = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Default Extension="png" ContentType="image/png"/>
<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
<Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/>
<Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/>
<Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>
<Override PartName="/ppt/notesMasters/notesMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.notesMaster+xml"/>
"#,
    );
    for i in 1..=pngs.len() {
        ct.push_str(&format!(
            r#"<Override PartName="/ppt/slides/slide{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>"#
        ));
        if i <= notes.len() && !notes[i - 1].trim().is_empty() {
            ct.push_str(&format!(
                r#"<Override PartName="/ppt/notesSlides/notesSlide{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml"/>"#
            ));
        }
    }
    ct.push_str(r#"<Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/></Types>"#);
    add(&mut zip, "[Content_Types].xml", ct.as_bytes())?;

    // _rels/.rels
    add(
        &mut zip,
        "_rels/.rels",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/app.xml"/>
</Relationships>"#,
    )?;

    // docProps/app.xml
    add(
        &mut zip,
        "docProps/app.xml",
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"><Application>autonomics deck engine</Application><Slides>{}</Slides></Properties>"#,
            pngs.len()
        )
        .as_bytes(),
    )?;

    // ppt/_rels/presentation.xml.rels + presentation.xml
    let mut pres_rels = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rIdm1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="slideMasters/slideMaster1.xml"/>
<Relationship Id="rIdn1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesMaster" Target="notesMasters/notesMaster1.xml"/>
<Relationship Id="rIdt1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="theme/theme1.xml"/>"#,
    );
    for i in 1..=pngs.len() {
        pres_rels.push_str(&format!(
            r#"<Relationship Id="rId{i}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide{i}.xml"/>"#
        ));
    }
    pres_rels.push_str("</Relationships>");
    add(&mut zip, "ppt/_rels/presentation.xml.rels", pres_rels.as_bytes())?;

    let mut sld_ids = String::new();
    for i in 1..=pngs.len() {
        sld_ids.push_str(&format!(r#"<p:sldId id="{}" r:id="rId{i}"/>"#, 256 + i));
    }
    add(
        &mut zip,
        "ppt/presentation.xml",
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rIdm1"/></p:sldMasterIdLst>
<p:notesMasterIdLst><p:notesMasterId r:id="rIdn1"/></p:notesMasterIdLst>
<p:sldIdLst>{sld_ids}</p:sldIdLst>
<p:sldSz cx="{cx}" cy="{cy}"/>
<p:notesSz cx="{cx}" cy="{cy}"/>
</p:presentation>"#
        )
        .as_bytes(),
    )?;

    // slide master / layout / theme / notes master (minimal-but-valid)
    add(&mut zip, "ppt/slideMasters/slideMaster1.xml", slide_master(cx, cy).as_bytes())?;
    add(
        &mut zip,
        "ppt/slideMasters/_rels/slideMaster1.xml.rels",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme1.xml"/>
</Relationships>"#,
    )?;
    add(&mut zip, "ppt/slideLayouts/slideLayout1.xml", slide_layout(cx, cy).as_bytes())?;
    add(
        &mut zip,
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="../slideMasters/slideMaster1.xml"/>
</Relationships>"#,
    )?;
    add(&mut zip, "ppt/theme/theme1.xml", theme().as_bytes())?;
    add(&mut zip, "ppt/notesMasters/notesMaster1.xml", notes_master(cx, cy).as_bytes())?;
    add(
        &mut zip,
        "ppt/notesMasters/_rels/notesMaster1.xml.rels",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme1.xml"/>
</Relationships>"#,
    )?;

    // slides + media + notes
    for (i, png) in pngs.iter().enumerate() {
        let n = i + 1;
        add(
            &mut zip,
            &format!("ppt/slides/_rels/slide{n}.xml.rels"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image{n}.png"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>{}</Relationships>"#,
                if i < notes.len() && !notes[i].trim().is_empty() {
                    format!(
                        r#"<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide" Target="../notesSlides/notesSlide{n}.xml"/>"#
                    )
                } else {
                    String::new()
                }
            )
            .as_bytes(),
        )?;
        add(
            &mut zip,
            &format!("ppt/slides/slide{n}.xml"),
            slide_xml(n, cx, cy, i < notes.len() && !notes[i].trim().is_empty()).as_bytes(),
        )?;
        add(&mut zip, &format!("ppt/media/image{n}.png"), png)?;
        if i < notes.len() && !notes[i].trim().is_empty() {
            add(
                &mut zip,
                &format!("ppt/notesSlides/_rels/notesSlide{n}.xml.rels"),
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesMaster" Target="../notesMasters/notesMaster1.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="../slides/slide1.xml"/>
</Relationships>"#,
            )?;
            add(
                &mut zip,
                &format!("ppt/notesSlides/notesSlide{n}.xml"),
                notes_slide(&notes[i]).as_bytes(),
            )?;
        }
    }

    let cursor = zip
        .finish()
        .map_err(|e| Error::Export(format!("pptx finish: {e}")))?;
    Ok(cursor.into_inner())
}

fn slide_xml(n: usize, cx: u64, cy: u64, has_notes: bool) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld><p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>
<p:pic>
<p:nvPicPr><p:cNvPr id="2" name="Slide {n}"/><p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr><p:nvPr/></p:nvPicPr>
<p:blipFill><a:blip r:embed="rId1"/><a:stretch><a:fillRect/></a:stretch></p:blipFill>
<p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>
</p:pic>
</p:spTree></p:cSld>
<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>{notes_close}
</p:sld>"#,
        notes_close = if has_notes {
            format!(r#"<p:notes><p:notes relationshipId="rId3"/></p:notes>"#)
        } else {
            String::new()
        }
    )
}

fn notes_slide(note: &str) -> String {
    let paragraphs: String = note
        .lines()
        .map(|line| {
            format!(
                r#"<a:p><a:r><a:rPr lang="en-US" dirty="0"/><a:t>{}</a:t></a:r></a:p>"#,
                escape_xml(line)
            )
        })
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:notes xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld><p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>
<p:sp>
<p:nvSpPr><p:cNvPr id="3" name="Notes Placeholder"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr>
<p:spPr/>
<p:txBody><a:bodyPr/><a:lstStyle/>{paragraphs}</p:txBody>
</p:sp>
</p:spTree></p:cSld>
</p:notes>"#
    )
}

fn slide_master(cx: u64, cy: u64) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldMaster xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld><p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/><a:chOff x="0" y="0"/><a:chExt cx="{cx}" cy="{cy}"/></a:xfrm></p:grpSpPr>
</p:spTree></p:cSld>
<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>
<p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst>
</p:sldMaster>"#
    )
}

fn slide_layout(cx: u64, cy: u64) -> String {
    let _ = (cx, cy);
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" type="blank" preserve="1">
<p:cSld name="Blank"><p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr/>
</p:spTree></p:cSld>
<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>
</p:sldLayout>"#
        .to_string()
}

fn notes_master(cx: u64, cy: u64) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:notesMaster xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld><p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/><a:chOff x="0" y="0"/><a:chExt cx="{cx}" cy="{cy}"/></a:xfrm></p:grpSpPr>
</p:spTree></p:cSld>
<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>
</p:notesMaster>"#
    )
}

fn theme() -> String {
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="deck">
<a:themeElements>
<a:clrScheme name="deck"><a:dk1><a:srgbClr val="1A1A1F"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="26262B"/></a:dk2><a:lt2><a:srgbClr val="F1F1F4"/></a:lt2><a:accent1><a:srgbClr val="4C6EF5"/></a:accent1><a:accent2><a:srgbClr val="12B886"/></a:accent2><a:accent3><a:srgbClr val="FAB005"/></a:accent3><a:accent4><a:srgbClr val="E8590C"/></a:accent4><a:accent5><a:srgbClr val="7950F2"/></a:accent5><a:accent6><a:srgbClr val="15AABF"/></a:accent6><a:hlink><a:srgbClr val="4C6EF5"/></a:hlink><a:folHlink><a:srgbClr val="7950F2"/></a:folHlink></a:clrScheme>
<a:fontScheme name="deck"><a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme>
<a:fmtScheme name="deck">
<a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst>
<a:lnStyleLst><a:ln><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst>
<a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst>
<a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst>
</a:fmtScheme>
</a:themeElements>
</a:theme>"#
        .to_string()
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// 1×1 PNG.
    fn tiny_png() -> Vec<u8> {
        const B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(B64)
            .unwrap()
    }

    #[test]
    fn rejects_empty() {
        assert!(build_pptx(&[], 980, 551, &[]).is_err());
    }

    #[test]
    fn golden_structure() {
        let png = tiny_png();
        let bytes = build_pptx(
            &[png.clone(), png],
            980,
            551,
            &["第一页备注\n第二行".to_string(), String::new()],
        )
        .unwrap();

        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();

        // required parts
        for part in [
            "[Content_Types].xml",
            "_rels/.rels",
            "docProps/app.xml",
            "ppt/presentation.xml",
            "ppt/_rels/presentation.xml.rels",
            "ppt/slideMasters/slideMaster1.xml",
            "ppt/slideLayouts/slideLayout1.xml",
            "ppt/theme/theme1.xml",
            "ppt/notesMasters/notesMaster1.xml",
            "ppt/slides/slide1.xml",
            "ppt/slides/slide2.xml",
            "ppt/media/image1.png",
            "ppt/media/image2.png",
            "ppt/notesSlides/notesSlide1.xml",
        ] {
            assert!(archive.by_name(part).is_ok(), "missing part {part}");
        }

        let read = |archive: &mut zip::ZipArchive<_>, name: &str| -> String {
            let mut f = archive.by_name(name).unwrap();
            let mut s = String::new();
            f.read_to_string(&mut s).unwrap();
            s
        };

        // sldSz EMU = px × 9525: 980 × 551
        let pres = read(&mut archive, "ppt/presentation.xml");
        assert_eq!(980 * 9525, 9_334_500);
        assert_eq!(551 * 9525, 5_248_275);
        assert!(
            pres.contains(r#"<p:sldSz cx="9334500" cy="5248275"/>"#),
            "{pres}"
        );

        // slide 1 references its image + notes; slide 2 has no notes part
        let s1 = read(&mut archive, "ppt/slides/slide1.xml");
        assert!(s1.contains(r#"<a:ext cx="9334500" cy="5248275"/>"#), "{s1}");
        assert!(s1.contains(r#"r:embed="rId1""#));
        assert!(s1.contains(r#"<p:notes><p:notes relationshipId="rId3"/></p:notes>"#));
        let s2 = read(&mut archive, "ppt/slides/slide2.xml");
        assert!(!s2.contains("p:notes"));

        // notes text survives XML escaping
        let notes1 = read(&mut archive, "ppt/notesSlides/notesSlide1.xml");
        assert!(notes1.contains("第一页备注"), "{notes1}");
        assert!(notes1.contains("第二行"));

        // content types lists both slides + one notes slide
        let ct = read(&mut archive, "[Content_Types].xml");
        assert!(ct.contains("/ppt/slides/slide2.xml"));
        assert!(ct.contains("/ppt/notesSlides/notesSlide1.xml"));
        assert!(!ct.contains("/ppt/notesSlides/notesSlide2.xml"));

        let app = read(&mut archive, "docProps/app.xml");
        assert!(app.contains("<Slides>2</Slides>"), "{app}");
    }
}

//! Parse vector-only SVG, deny external/active content, and reserialize the render tree.
use crate::{Asset, Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use quick_xml::{Reader, events::Event};
use resvg::{tiny_skia, usvg};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VectorAsset {
    pub asset: Asset,
    #[serde(flatten)]
    pub vector: PreparedSvg,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreparedSvg {
    pub source_sha256: String,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    #[serde(rename = "dataURL")]
    pub data_url: String,
    #[serde(rename = "previewDataURL")]
    pub preview_data_url: String,
    pub colors: Vec<String>,
    pub normalization: Vec<String>,
    pub text_outlined: bool,
}
fn invalid(s: impl Into<String>) -> Error {
    Error::Invalid(s.into())
}
pub fn prepare(asset: Asset, bytes: &[u8]) -> Result<VectorAsset> {
    Ok(VectorAsset {
        asset,
        vector: normalize(bytes)?,
    })
}
fn references(value: &str) -> Result<()> {
    let lower = value.to_ascii_lowercase();
    if lower.contains("@import") || lower.contains("javascript:") || value.contains('\\') {
        return Err(invalid(
            "external CSS or escaped references are unsupported",
        ));
    }
    for (index, _) in lower.match_indices("url(") {
        let tail = &value[index + 4..];
        let end = tail
            .find(')')
            .ok_or_else(|| invalid("invalid SVG reference"))?;
        let reference = tail[..end].trim().trim_matches(['\'', '"']);
        if !reference.starts_with('#') {
            return Err(invalid("SVG references must be local fragments"));
        }
    }
    Ok(())
}
pub fn normalize(bytes: &[u8]) -> Result<PreparedSvg> {
    let svg = std::str::from_utf8(bytes).map_err(|_| invalid("SVG must be UTF-8"))?;
    if bytes.len() > 4 * 1024 * 1024 || svg.contains("<!ENTITY") {
        return Err(invalid("SVG is too large or contains entity declarations"));
    }
    let mut reader = Reader::from_str(svg);
    let mut nodes = 0;
    let mut depth = 0;
    let mut has_text = false;
    let mut level = 0usize;
    let font = ttf_parser::Face::parse(include_bytes!("../fonts/LiberationSans-Regular.ttf"), 0)
        .map_err(|e| invalid(e.to_string()))?;
    loop {
        let event = reader.read_event().map_err(|e| invalid(e.to_string()))?;
        if matches!(event, Event::Start(_)) {
            level += 1;
            if level > 128 {
                return Err(invalid("SVG nesting exceeds 128"));
            }
        }
        if matches!(event, Event::End(_)) {
            level = level.saturating_sub(1);
        }
        match event {
            Event::Start(e) | Event::Empty(e) => {
                nodes += 1;
                if nodes > 50000 {
                    return Err(invalid("SVG exceeds 50000 nodes"));
                }
                let tag = e.local_name();
                if !matches!(
                    tag.as_ref(),
                    b"svg"
                        | b"g"
                        | b"defs"
                        | b"path"
                        | b"rect"
                        | b"circle"
                        | b"ellipse"
                        | b"line"
                        | b"polyline"
                        | b"polygon"
                        | b"use"
                        | b"symbol"
                        | b"clipPath"
                        | b"mask"
                        | b"linearGradient"
                        | b"radialGradient"
                        | b"stop"
                        | b"pattern"
                        | b"filter"
                        | b"feBlend"
                        | b"feColorMatrix"
                        | b"feComponentTransfer"
                        | b"feComposite"
                        | b"feConvolveMatrix"
                        | b"feDiffuseLighting"
                        | b"feDisplacementMap"
                        | b"feDistantLight"
                        | b"feDropShadow"
                        | b"feFlood"
                        | b"feFuncA"
                        | b"feFuncR"
                        | b"feFuncG"
                        | b"feFuncB"
                        | b"feGaussianBlur"
                        | b"feMerge"
                        | b"feMergeNode"
                        | b"feMorphology"
                        | b"feOffset"
                        | b"fePointLight"
                        | b"feSpecularLighting"
                        | b"feSpotLight"
                        | b"feTile"
                        | b"feTurbulence"
                        | b"style"
                        | b"title"
                        | b"desc"
                        | b"metadata"
                        | b"switch"
                        | b"marker"
                        | b"text"
                        | b"tspan"
                        | b"textPath"
                        | b"a"
                ) && !e.name().as_ref().contains(&b':')
                {
                    return Err(invalid("unsupported SVG element"));
                }

                if matches!(tag.as_ref(), b"text" | b"tspan") {
                    has_text = true;
                }
                if matches!(
                    tag.as_ref(),
                    b"script"
                        | b"foreignObject"
                        | b"image"
                        | b"animate"
                        | b"animateMotion"
                        | b"animateTransform"
                        | b"set"
                ) {
                    return Err(invalid(
                        "unsupported active, raster or text content; use a vector-path icon",
                    ));
                }
                for a in e.attributes() {
                    let a = a.map_err(|e| invalid(e.to_string()))?;
                    let key = a.key.local_name();
                    let value = a
                        .decode_and_unescape_value(reader.decoder())
                        .map_err(|e| invalid(e.to_string()))?;
                    references(&value)?;
                    if key.as_ref().to_ascii_lowercase().starts_with(b"on")
                        || (matches!(key.as_ref(), b"href" | b"src") && !value.starts_with('#'))
                        || value.contains('\\')
                        || value.to_ascii_lowercase().contains("javascript:")
                    {
                        return Err(invalid(
                            "external references or event attributes are not supported",
                        ));
                    }
                }
            }
            Event::Text(e) => {
                let text = e.decode().map_err(|e| invalid(e.to_string()))?;
                references(&text)?;
                if has_text
                    && text
                        .chars()
                        .any(|c| !c.is_whitespace() && font.glyph_index(c).is_none())
                {
                    return Err(invalid(
                        "SVG text contains characters outside the bundled font",
                    ));
                }
            }
            Event::GeneralRef(e) => {
                let reference = format!(
                    "&{};",
                    std::str::from_utf8(e.as_ref()).map_err(|e| invalid(e.to_string()))?
                );
                let decoded =
                    quick_xml::escape::unescape(&reference).map_err(|e| invalid(e.to_string()))?;
                if has_text
                    && decoded
                        .chars()
                        .any(|c| !c.is_whitespace() && font.glyph_index(c).is_none())
                {
                    return Err(invalid(
                        "SVG text contains characters outside the bundled font",
                    ));
                }
            }
            Event::CData(e) => {
                let text = e.decode().map_err(|e| invalid(e.to_string()))?;
                references(&text)?;
                if has_text
                    && text
                        .chars()
                        .any(|c| !c.is_whitespace() && font.glyph_index(c).is_none())
                {
                    return Err(invalid(
                        "SVG text contains characters outside the bundled font",
                    ));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        depth += 1;
        if depth > 200000 {
            return Err(invalid("SVG event limit exceeded"));
        }
    }
    let mut fontdb = usvg::fontdb::Database::new();
    fontdb.load_font_data(include_bytes!("../fonts/LiberationSans-Regular.ttf").to_vec());
    fontdb.set_sans_serif_family("Liberation Sans");
    fontdb.set_serif_family("Liberation Sans");
    let options = usvg::Options {
        font_family: "Liberation Sans".into(),
        fontdb: std::sync::Arc::new(fontdb),
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(svg, &options).map_err(|e| invalid(e.to_string()))?;
    let size = tree.size();
    if size.width() > 100000. || size.height() > 100000. || tree.root().children().is_empty() {
        return Err(invalid("invalid or empty SVG dimensions/content"));
    }
    let serialized = tree.to_string(&usvg::WriteOptions::default());
    // Excalidraw normalizes SVG through DOMParser/outerHTML on file load.
    // Include the equivalent root namespace/viewBox and omit document trailing
    // whitespace so the editor preserves the exact content-addressed bytes.
    let mut root_reader = Reader::from_str(&serialized);
    let Event::Start(root) = root_reader
        .read_event()
        .map_err(|e| invalid(e.to_string()))?
    else {
        return Err(invalid("invalid normalized SVG root"));
    };
    let mut width_attr = None;
    let mut height_attr = None;
    let mut xlink = false;
    for attr in root.attributes() {
        let attr = attr.map_err(|e| invalid(e.to_string()))?;
        match attr.key.as_ref() {
            b"width" => width_attr = Some(String::from_utf8_lossy(&attr.value).into_owned()),
            b"height" => height_attr = Some(String::from_utf8_lossy(&attr.value).into_owned()),
            b"xmlns:xlink" => xlink = true,
            _ => {}
        }
    }
    let w = width_attr.ok_or_else(|| invalid("SVG width unavailable"))?;
    let h = height_attr.ok_or_else(|| invalid("SVG height unavailable"))?;
    let end = serialized
        .find('>')
        .ok_or_else(|| invalid("invalid SVG root"))?;
    let ns = if xlink {
        " xmlns:xlink=\"http://www.w3.org/1999/xlink\""
    } else {
        ""
    };
    let canonical = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\"{ns} width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">{}",
        serialized[end + 1..].trim_end()
    );
    if canonical.len() > 4 * 1024 * 1024 {
        return Err(invalid("normalized SVG exceeds 4 MiB"));
    }
    // Render the final stored SVG, not the higher-precision source tree before
    // serialization. This keeps previews stable when the same asset is reopened.
    drop(tree);
    let tree = usvg::Tree::from_str(&canonical, &options).map_err(|e| invalid(e.to_string()))?;
    let scale = (768. / size.width().max(size.height())).min(1.);
    let width = (size.width() * scale).ceil().max(1.) as u32;
    let height = (size.height() * scale).ceil().max(1.) as u32;
    let mut pixmap =
        tiny_skia::Pixmap::new(width, height).ok_or_else(|| invalid("invalid preview size"))?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let png = pixmap.encode_png().map_err(|e| invalid(e.to_string()))?;
    let mut colors = Vec::new();
    let mut reader = Reader::from_str(&canonical);
    loop {
        match reader.read_event().map_err(|e| invalid(e.to_string()))? {
            Event::Start(e) | Event::Empty(e) => {
                for a in e.attributes().flatten() {
                    if matches!(
                        a.key.as_ref(),
                        b"fill" | b"stroke" | b"stop-color" | b"flood-color" | b"lighting-color"
                    ) {
                        let v = String::from_utf8_lossy(&a.value).into_owned();
                        if v.starts_with('#') && !colors.contains(&v) {
                            colors.push(v);
                        }
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(PreparedSvg {
        source_sha256: format!("{:x}", Sha256::digest(bytes)),
        sha256: format!("{:x}", Sha256::digest(canonical.as_bytes())),
        width: size.width().ceil() as u32,
        height: size.height().ceil() as u32,
        data_url: format!("data:image/svg+xml;base64,{}", STANDARD.encode(canonical)),
        preview_data_url: format!("data:image/png;base64,{}", STANDARD.encode(png)),
        colors,
        text_outlined: has_text,
        normalization: vec![
            if has_text {
                "SVG normalized; text outlined using bundled Liberation Sans"
            } else {
                "SVG normalized to a standalone render tree"
            }
            .into(),
        ],
    })
}

/// Replace exact solid paint colours simultaneously; never edit path geometry,
/// opacity, gradient references or text glyphs as a side effect of replacement.
pub fn recolor(
    bytes: &[u8],
    replacements: &std::collections::BTreeMap<String, String>,
) -> Result<PreparedSvg> {
    use quick_xml::{Writer, events::BytesStart};
    use std::{borrow::Cow, collections::BTreeMap};
    fn color(value: &str) -> Result<String> {
        if value.len() != 7
            || !value.starts_with('#')
            || !value[1..].bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(invalid("colours must use #RRGGBB"));
        }
        Ok(value.to_ascii_lowercase())
    }
    if replacements.is_empty() || replacements.len() > 32 {
        return Err(invalid("requires 1..32 colour replacements"));
    }
    let original = normalize(bytes)?;
    if original.text_outlined {
        return Err(invalid(
            "outline SVG text before recoloring; font substitution is not part of a colour edit",
        ));
    }
    let mut mapping = BTreeMap::new();
    for (from, to) in replacements {
        let from = color(from)?;
        let to = color(to)?;
        if !original.colors.contains(&from) {
            return Err(invalid(format!(
                "source colour {from} is absent from this vector"
            )));
        }
        if mapping.insert(from, to).is_some() {
            return Err(invalid("duplicate source colours after case normalization"));
        }
    }
    if mapping.iter().all(|(a, b)| a == b) {
        return Err(invalid("colour replacements do not change the vector"));
    }
    let encoded = original
        .data_url
        .split_once(',')
        .ok_or_else(|| invalid("invalid normalized SVG"))?
        .1;
    let source = STANDARD
        .decode(encoded)
        .map_err(|e| invalid(e.to_string()))?;
    let mut reader = Reader::from_reader(source.as_slice());
    let mut writer = Writer::new(Vec::new());
    loop {
        let event = reader.read_event().map_err(|e| invalid(e.to_string()))?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let name = std::str::from_utf8(element.name().as_ref())
                    .map_err(|e| invalid(e.to_string()))?
                    .to_owned();
                let mut next = BytesStart::new(name);
                for attribute in element.attributes() {
                    let mut attribute = attribute.map_err(|e| invalid(e.to_string()))?;
                    if matches!(
                        attribute.key.as_ref(),
                        b"fill" | b"stroke" | b"stop-color" | b"flood-color" | b"lighting-color"
                    ) {
                        if let Some(replacement) = std::str::from_utf8(&attribute.value)
                            .ok()
                            .and_then(|v| mapping.get(&v.to_ascii_lowercase()))
                        {
                            attribute.value = Cow::Owned(replacement.as_bytes().to_vec());
                        }
                    }
                    next.push_attribute(attribute);
                }
                writer
                    .write_event(if matches!(event, Event::Empty(_)) {
                        Event::Empty(next)
                    } else {
                        Event::Start(next)
                    })
                    .map_err(|e| invalid(e.to_string()))?;
            }
            Event::Eof => break,
            event => writer
                .write_event(event)
                .map_err(|e| invalid(e.to_string()))?,
        }
    }
    normalize(&writer.into_inner())
}

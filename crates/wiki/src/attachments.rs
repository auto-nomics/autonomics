//! Shared attachment contracts for browser, desktop, and agent writes.
use crate::storage::error::{Error, Result};
use pulldown_cmark::{Event, Options, Parser, Tag};

pub const MAX_IMAGE_BYTES: usize = 50 * 1024 * 1024;

pub fn validate_image(filename: &str, data: &[u8]) -> Result<()> {
    if data.is_empty() || data.len() > MAX_IMAGE_BYTES {
        return Err(Error::InvalidAttachment("图片为空或超过 50 MiB".into()));
    }
    if filename.is_empty()
        || filename
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
    {
        return Err(Error::InvalidAttachment("图片文件名无效".into()));
    }
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let valid = match ext.as_str() {
        "png" => data.starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpg" | "jpeg" => data.starts_with(b"\xff\xd8\xff"),
        "gif" => data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a"),
        "webp" => data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP"),
        "bmp" => data.starts_with(b"BM"),
        "avif" => {
            data.get(4..8) == Some(b"ftyp")
                && data
                    .get(8..data.len().min(32))
                    .unwrap_or(&[])
                    .chunks_exact(4)
                    .any(|b| b == b"avif" || b == b"avis")
        }
        "svg" => safe_svg(data),
        _ => false,
    };
    if !valid {
        return Err(Error::InvalidAttachment(
            "图片格式与内容不匹配，或 SVG 含不支持的主动内容".into(),
        ));
    }
    Ok(())
}

fn safe_svg(data: &[u8]) -> bool {
    use quick_xml::{Reader, events::Event};
    let mut reader = Reader::from_reader(data);
    let mut root = false;
    let mut depth = 0usize;
    loop {
        match reader.read_event() {
            Ok(event @ (Event::Start(_) | Event::Empty(_))) => {
                let empty = matches!(&event, Event::Empty(_));
                let e = match event {
                    Event::Start(e) | Event::Empty(e) => e,
                    _ => unreachable!(),
                };
                let name = e.name();
                let name = name.as_ref();
                if depth == 0 {
                    if root || name != b"svg" {
                        return false;
                    }
                    root = true;
                }
                // A conservative static SVG subset; external resources, CSS and scripts are rejected.
                if !matches!(
                    name,
                    b"svg"
                        | b"g"
                        | b"path"
                        | b"rect"
                        | b"circle"
                        | b"ellipse"
                        | b"line"
                        | b"polyline"
                        | b"polygon"
                        | b"text"
                        | b"tspan"
                        | b"defs"
                        | b"clipPath"
                        | b"mask"
                        | b"linearGradient"
                        | b"radialGradient"
                        | b"stop"
                        | b"title"
                        | b"desc"
                        | b"use"
                        | b"symbol"
                ) {
                    return false;
                }
                for attr in e.attributes() {
                    let Ok(attr) = attr else {
                        return false;
                    };
                    let Ok(value) = attr.unescape_value() else {
                        return false;
                    };
                    let key = String::from_utf8_lossy(attr.key.as_ref()).to_ascii_lowercase();
                    let lower = value.to_ascii_lowercase();
                    if key.starts_with("on")
                        || key == "style"
                        || (key.ends_with("href") && !value.starts_with('#'))
                        || lower
                            .split("url(")
                            .skip(1)
                            .any(|suffix| !suffix.starts_with('#'))
                    {
                        return false;
                    }
                }
                if !empty {
                    depth += 1;
                }
            }
            Ok(Event::End(_)) => {
                if depth == 0 {
                    return false;
                }
                depth -= 1;
            }
            Ok(Event::Text(text)) if depth == 0 => {
                if !text.iter().all(u8::is_ascii_whitespace) {
                    return false;
                }
            }
            Ok(Event::DocType(_) | Event::PI(_)) | Err(_) => return false,
            Ok(Event::Eof) => return root && depth == 0,
            _ => {}
        }
    }
}

/// Collect image destinations with the same legacy and portable forms used by the GUI.
pub fn references(content: &str, source_entry: &str, owner: &str, attachment: &str) -> bool {
    Parser::new_ext(content, Options::all()).any(|event| match event {
        Event::Start(Tag::Image { dest_url, .. }) => {
            let dest = dest_url.to_ascii_lowercase();
            dest.eq_ignore_ascii_case(&format!("wiki-attach://{owner}/{attachment}"))
                || (source_entry == owner
                    && dest
                        .starts_with(&format!("wiki-attach://{attachment}/").to_ascii_lowercase()))
                || dest.eq_ignore_ascii_case(&format!(
                    "/api/v1/wiki/entries/{owner}/attachments/{attachment}/raw"
                ))
        }
        // Raw HTML images are supported by the renderer, too. Conservatively protect their IDs.
        Event::Html(html) | Event::InlineHtml(html) => html
            .to_ascii_lowercase()
            .contains(&attachment.to_ascii_lowercase()),
        _ => false,
    })
}

pub fn markdown_ref(entry: &str, attachment: &str, filename: &str) -> String {
    let mut alt = String::new();
    for c in filename.chars() {
        if matches!(c, '\\' | '[' | ']' | '<' | '>' | '`' | '*' | '_' | '$') {
            alt.push('\\');
        }
        alt.push(if c.is_control() { ' ' } else { c });
    }
    format!("![{alt}](wiki-attach://{entry}/{attachment})")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checks_format_filename_and_svg_content() {
        assert!(validate_image("plot.png", b"not an image").is_err());
        assert!(validate_image("../plot.png", b"\x89PNG\r\n\x1a\n").is_err());
        assert!(validate_image("plot\n.png", b"\x89PNG\r\n\x1a\n").is_err());
        assert!(validate_image("plot.svg", br##"<svg xmlns="http://www.w3.org/2000/svg"><path fill="url(#a)" d="M0 0"/></svg>"##).is_ok());
        for svg in [
            "<svg><script>alert(1)</script></svg>",
            "<svg onload='alert(1)'/>",
            "<svg><use href='https://example.com/a'/></svg>",
            "<svg><path>",
            "<svg/><svg/>",
            "<!DOCTYPE svg><svg/>",
            "<svg><path fill='url(#a) url(https://a)'/></svg>",
        ] {
            assert!(validate_image("plot.svg", svg.as_bytes()).is_err(), "{svg}");
        }
    }
    #[test]
    fn portable_legacy_and_html_refs_but_not_code() {
        let owner = "b2666ff7-a305-456e-8f4e-a7733b7bc2f0";
        let image = "c2666ff7-a305-456e-8f4e-a7733b7bc2f0";
        let reference = markdown_ref(owner, image, "file [1] & _2_.png");
        assert!(references(&reference, "other", owner, image));
        let legacy = format!("![x](wiki-attach://{image}/file.png)");
        assert!(references(&legacy, owner, owner, image));
        assert!(!references(&legacy, "other", owner, image));
        assert!(!references(
            &format!("```md\n{reference}\n```"),
            owner,
            owner,
            image
        ));
        assert!(references(
            &format!("<img src='/api/v1/wiki/entries/{owner}/attachments/{image}/raw'>"),
            owner,
            owner,
            image
        ));
    }
}

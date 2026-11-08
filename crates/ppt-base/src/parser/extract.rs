//! Slide-content extraction — ports of Slidev `parseSlide`'s note/title
//! extraction, `extractImagesUsage`, and `detectFeatures`
//! (`packages/parser/src/core.ts:112-237`).

use serde_json::{Map, Value};

/// `^```[\s\S]+?^```/gm` — complete fenced blocks (column-0 fences only).
static RE_CODE_BLOCK: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"(?ms)^```.+?^```").expect("valid regex"));

/// `^#+ (.*)$` multiline.
static RE_HEADING: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"(?m)^(#+) (.*)$").expect("valid regex"));

/// `<!--([\s\S]*?)-->`.
static RE_HTML_COMMENT: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"(?s)<!--(.*?)-->").expect("valid regex"));

/// Remove complete fenced code blocks (column-0 opening fences), line-based
/// equivalent of the Slidev global regex.
pub fn strip_code_blocks(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut in_fence = false;
    for line in content.lines() {
        if in_fence {
            if line.starts_with("```") {
                in_fence = false;
            }
            continue; // fence lines and their content are stripped
        }
        if line.starts_with("```") {
            in_fence = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Extract the trailing speaker note: the LAST `<!-- … -->` comment that
/// extends to the end of the (already trimmed) content. Returns
/// `(content_without_note, note)`.
pub fn extract_note(content: &str) -> (String, Option<String>) {
    let content = content.trim();
    let mut last: Option<(usize, usize, String)> = None;
    for caps in RE_HTML_COMMENT.captures_iter(content) {
        let m = caps.get(0).unwrap();
        last = Some((m.start(), m.end(), caps.get(1).map(|c| c.as_str().to_string()).unwrap_or_default()));
    }
    if let Some((start, end, body)) = last {
        if end >= content.len() {
            return (content[..start].trim().to_string(), Some(body.trim().to_string()));
        }
    }
    (content.to_string(), None)
}

/// Slide title + TOC level: frontmatter `title`/`name` wins; otherwise
/// the first `#` heading outside fenced code blocks (`#` inside a fence
/// is a code comment, not a title — Slidev pins this).
pub fn extract_title_level(
    content: &str,
    frontmatter: &Map<String, Value>,
) -> (Option<String>, Option<u8>) {
    let fm_title = frontmatter
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            frontmatter
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    let stripped = strip_code_blocks(content);
    if let Some(t) = fm_title {
        let level = frontmatter.get("level").and_then(Value::as_u64).map(|l| l as u8)
            .or_else(|| RE_HEADING.captures(&stripped).and_then(|caps| caps.get(1)).and_then(|m| m.as_str().len().try_into().ok()));
        return (Some(t), level);
    }
    match RE_HEADING.captures(&stripped) {
        Some(caps) => {
            let title = caps.get(2).map(|m| m.as_str().trim().to_string());
            let level = caps
                .get(1)
                .and_then(|m| m.as_str().len().try_into().ok());
            let level = frontmatter
                .get("level")
                .and_then(Value::as_u64)
                .map(|l| l.max(1) as u8)
                .or(level);
            (title, level)
        }
        None => {
            let level = frontmatter.get("level").and_then(Value::as_u64).map(|l| l as u8);
            (None, level)
        }
    }
}

/// `\.(png|jpe?g|gif|svg|webp|avif|ico|bmp|tiff?)$` case-insensitive.
static RE_IMAGE_EXT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\.(?:png|jpe?g|gif|svg|webp|avif|ico|bmp|tiff?)$").expect("valid regex")
});

static RE_MD_IMAGE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"!\[[^\]]*\]\(([^)]+)\)").expect("valid regex")
});

static RE_MD_IMAGE_TITLE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r#"\s+(?:"[^"]*"|'[^']*')\s*$"#).expect("valid regex")
});

static RE_PROP: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r#"\b(?:src|image)=["']([^"']+)["']"#).expect("valid regex")
});

static RE_BOUND_PROP: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r#":(?:src|image)=["']'([^']+)'["']"#).expect("valid regex")
});

static RE_CSS_URL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r#"url\(["']?([^"')]+)["']?\)"#).expect("valid regex")
});

/// Extract image URLs from slide content and frontmatter, stripping code
/// blocks first (Slidev `extractImagesUsage`). Deduplicated, insertion
/// order preserved.
pub fn extract_images(content: &str, frontmatter: &Map<String, Value>) -> Vec<String> {
    let mut images: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut push = |url: &str, images: &mut Vec<String>, seen: &mut std::collections::HashSet<String>| {
        let url = url.trim().to_string();
        if !url.is_empty() && seen.insert(url.clone()) {
            images.push(url);
        }
    };

    for key in ["image", "backgroundImage", "background"] {
        if let Some(Value::String(val)) = frontmatter.get(key) {
            if val.is_empty() || val.starts_with("data:") {
                continue;
            }
            if key == "background" {
                if RE_IMAGE_EXT.is_match(val) || val.starts_with('/') || val.starts_with("http") {
                    push(val, &mut images, &mut seen);
                }
            } else {
                push(val, &mut images, &mut seen);
            }
        }
    }

    let stripped = strip_code_blocks(content);

    // Markdown images
    for caps in RE_MD_IMAGE.captures_iter(&stripped) {
        let target = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        let url = normalize_markdown_image_target(target);
        if !url.is_empty() && !url.starts_with("data:") {
            push(&url, &mut images, &mut seen);
        }
    }

    // Component props src=/image=
    for caps in RE_PROP.captures_iter(&stripped) {
        if let Some(url) = caps.get(1).map(|m| m.as_str()) {
            if !url.starts_with("data:")
                && !url.contains("{{")
                && RE_IMAGE_EXT.is_match(url)
            {
                push(url, &mut images, &mut seen);
            }
        }
    }

    // Vue bound props :src="'…'"
    for caps in RE_BOUND_PROP.captures_iter(&stripped) {
        if let Some(url) = caps.get(1).map(|m| m.as_str()) {
            if !url.starts_with("data:") && RE_IMAGE_EXT.is_match(url) {
                push(url, &mut images, &mut seen);
            }
        }
    }

    // CSS url()
    for caps in RE_CSS_URL.captures_iter(&stripped) {
        if let Some(url) = caps.get(1).map(|m| m.as_str()) {
            if !url.starts_with("data:") && RE_IMAGE_EXT.is_match(url) {
                push(url, &mut images, &mut seen);
            }
        }
    }

    images
}

/// Strip the optional CommonMark title and angle-bracket form:
/// `![a](/x.png "Title")` → `/x.png`.
fn normalize_markdown_image_target(target: &str) -> String {
    let url = RE_MD_IMAGE_TITLE.replace(target.trim(), "").trim().to_string();
    if url.starts_with('<') && url.ends_with('>') && url.len() >= 2 {
        url[1..url.len() - 1].trim().to_string()
    } else {
        url
    }
}

/// Deck-level feature detection (Slidev `detectFeatures`, minus the
/// Node-coupled features we do not support).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DetectedFeatures {
    pub katex: bool,
    pub mermaid: bool,
}

/// `$…$` inline (same line) or `$$` block math.
static RE_DOLLAR_INLINE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"\$.*\$").expect("valid regex"));

static RE_DOLLAR_BLOCK: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"\$\$").expect("valid regex"));

/// ```` ```mermaid ```` at a line start.
static RE_MERMAID: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"(?m)^```mermaid").expect("valid regex"));

pub fn detect_features(code: &str) -> DetectedFeatures {
    DetectedFeatures {
        katex: RE_DOLLAR_INLINE.is_match(code) || RE_DOLLAR_BLOCK.is_match(code),
        mermaid: RE_MERMAID.is_match(code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use Map as JM;

    fn fm(pairs: serde_json::Value) -> JM<String, Value> {
        pairs.as_object().cloned().unwrap_or_default()
    }

    // ---- note ----------------------------------------------------------

    #[test]
    fn trailing_comment_is_note() {
        let (content, note) = extract_note("# Title\n\nbody\n\n<!--\nspeaker note\n-->");
        assert_eq!(content, "# Title\n\nbody");
        assert_eq!(note.as_deref(), Some("speaker note"));
    }

    #[test]
    fn non_trailing_comment_is_not_note() {
        let (content, note) = extract_note("<!-- mid -->\n\nbody\n\nafter");
        assert!(note.is_none());
        assert!(content.contains("after"));
    }

    #[test]
    fn only_last_comment_extending_to_end_is_note() {
        let (content, note) =
            extract_note("<!-- a -->\nbody\n<!-- b -->");
        assert_eq!(content, "<!-- a -->\nbody");
        assert_eq!(note.as_deref(), Some("b"));
    }

    // ---- title ---------------------------------------------------------

    #[test]
    fn code_fence_heading_is_not_title() {
        // Slidev "does not take the slide title from a heading inside a
        // code block" fixture
        let content = "```bash\n# Install the CLI\nnpm i\n```\n\n## Getting started\n";
        let (title, level) = extract_title_level(content, &fm(json!({})));
        assert_eq!(title.as_deref(), Some("Getting started"));
        assert_eq!(level, Some(2));
    }

    #[test]
    fn fence_only_slide_has_no_title() {
        let content = "```bash\n# Only a code comment\n```";
        let (title, level) = extract_title_level(content, &fm(json!({})));
        assert_eq!(title, None);
        assert_eq!(level, None);
    }

    #[test]
    fn frontmatter_title_wins() {
        let (title, level) =
            extract_title_level("# Heading\n", &fm(json!({"title": "FM", "level": 1})));
        assert_eq!(title.as_deref(), Some("FM"));
        assert_eq!(level, Some(1));
    }

    #[test]
    fn frontmatter_title_keeps_heading_depth_when_level_is_absent() {
        let (title, level) = extract_title_level("## Heading\n", &fm(json!({"title": "Short title"})));
        assert_eq!(title.as_deref(), Some("Short title"));
        assert_eq!(level, Some(2));
    }

    #[test]
    fn name_fallback_and_level_override() {
        let (title, level) =
            extract_title_level("## H\n", &fm(json!({"name": "Named", "level": 3})));
        assert_eq!(title.as_deref(), Some("Named"));
        assert_eq!(level, Some(3));
    }

    // ---- images --------------------------------------------------------

    #[test]
    fn images_from_frontmatter_keys() {
        assert_eq!(
            extract_images("", &fm(json!({"image": "/path/to/image.png"}))),
            ["/path/to/image.png"]
        );
        assert_eq!(
            extract_images("", &fm(json!({"backgroundImage": "https://example.com/bg.jpg"}))),
            ["https://example.com/bg.jpg"]
        );
        assert_eq!(
            extract_images("", &fm(json!({"background": "/assets/bg.webp"}))),
            ["/assets/bg.webp"]
        );
        // background without image-ish value is ignored
        assert_eq!(extract_images("", &fm(json!({"background": "gradient-to-r"}))), Vec::<String>::new());
        // data URLs ignored
        assert_eq!(
            extract_images("", &fm(json!({"image": "data:image/png;base64,abc"}))),
            Vec::<String>::new()
        );
    }

    #[test]
    fn images_markdown_syntax() {
        assert_eq!(
            extract_images(r#"![alt text](/images/photo.jpg)"#, &fm(json!({}))),
            ["/images/photo.jpg"]
        );
        assert_eq!(
            extract_images(
                r#"![alt](/images/photo.jpg "A photo")
![alt](/images/other.png 'Another')"#,
                &fm(json!({}))
            ),
            ["/images/photo.jpg", "/images/other.png"]
        );
        assert_eq!(
            extract_images(r#"![alt](</images/my photo.jpg>)"#, &fm(json!({}))),
            ["/images/my photo.jpg"]
        );
        assert_eq!(
            extract_images(r#"![inline](data:image/png;base64,abc)"#, &fm(json!({}))),
            Vec::<String>::new()
        );
        // whitespace-trimmed
        assert_eq!(
            extract_images(r#"![img]( /path/with/spaces.png )"#, &fm(json!({}))),
            ["/path/with/spaces.png"]
        );
    }

    #[test]
    fn images_props_and_css() {
        assert_eq!(
            extract_images(r#"<img src="/assets/logo.png" />"#, &fm(json!({}))),
            ["/assets/logo.png"]
        );
        assert_eq!(
            extract_images(r#"<MyImage image="/path/to/image.jpg" />"#, &fm(json!({}))),
            ["/path/to/image.jpg"]
        );
        // non-image extension props ignored
        assert_eq!(
            extract_images(r#"<div src="/some/path" />"#, &fm(json!({}))),
            Vec::<String>::new()
        );
        // template expressions ignored
        assert_eq!(
            extract_images(r#"<img src="{{imagePath}}.png" />"#, &fm(json!({}))),
            Vec::<String>::new()
        );
        assert_eq!(
            extract_images(r#"<img :src="'/static/image.png'" />"#, &fm(json!({}))),
            ["/static/image.png"]
        );
        assert_eq!(
            extract_images(
                r#"<div style="background: url(/bg/image.png)">
<style> .class { background-image: url('/another/image.jpg'); } </style>"#,
                &fm(json!({}))
            ),
            ["/bg/image.png", "/another/image.jpg"]
        );
        assert_eq!(
            extract_images(r#"<div style="background: url(/fonts/font.woff2)">"#, &fm(json!({}))),
            Vec::<String>::new()
        );
    }

    #[test]
    fn images_code_blocks_stripped_and_deduped() {
        let content = "Some content\n![real image](/real.png)\n\n```markdown\n![fake image](/fake.png)\n<img src=\"/also-fake.jpg\" />\n```\n\n![another real](/real2.jpg)";
        assert_eq!(
            extract_images(content, &fm(json!({}))),
            ["/real.png", "/real2.jpg"]
        );
        assert_eq!(
            extract_images(
                "![img](/same.png)\n![img2](/same.png)\n<img src=\"/same.png\" />",
                &fm(json!({}))
            ),
            ["/same.png"]
        );
    }

    #[test]
    fn images_extension_matrix() {
        let content = r#"
![png](/img.png)
![jpg](/img.jpg)
![jpeg](/img.jpeg)
![gif](/img.gif)
![svg](/img.svg)
![webp](/img.webp)
![avif](/img.avif)
![ico](/img.ico)
![bmp](/img.bmp)
![tiff](/img.tiff)
"#;
        assert_eq!(extract_images(content, &fm(json!({}))).len(), 10);
        // case-insensitive
        assert_eq!(
            extract_images(
                r#"<img src="/image.PNG" />
<img src="/image.JpG" />
<img src="/image.WEBP" />"#,
                &fm(json!({}))
            ),
            ["/image.PNG", "/image.JpG", "/image.WEBP"]
        );
    }

    // ---- features ------------------------------------------------------

    #[test]
    fn feature_detection() {
        let f = detect_features("a $x^2$ b");
        assert!(f.katex);
        assert!(!f.mermaid);
        let f = detect_features("$$\nx\n$$");
        assert!(f.katex);
        let f = detect_features("```mermaid\ngraph TD\n```");
        assert!(f.mermaid);
        let f = detect_features("plain text, no math, ```bash\nls\n```");
        assert!(!f.katex);
        assert!(!f.mermaid);
        // $ detection needs two on one line
        let f = detect_features("one dollar: $\ntwo lines");
        assert!(!f.katex);
    }
}

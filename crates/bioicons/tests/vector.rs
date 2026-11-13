use base64::{Engine, engine::general_purpose::STANDARD};
use bioicons::vector::normalize;
#[test]
fn vector_shapes_gradients_and_text_become_standalone_paths() {
    let svg=br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 80"><defs><linearGradient id="g"><stop stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></linearGradient></defs><rect width="100" height="80" fill="url(#g)"/><text x="20" y="40" font-family="Arial" font-size="20">P</text></svg>"##;
    let out = normalize(svg).unwrap();
    let canonical = String::from_utf8(
        STANDARD
            .decode(out.data_url.split_once(',').unwrap().1)
            .unwrap(),
    )
    .unwrap();
    assert!(!canonical.contains("<text"));
    assert!(
        canonical.matches("<path").count() >= 2,
        "the text glyph must survive as an additional path"
    );
    assert_eq!(out.width, 100);
    assert!(out.normalization[0].contains("text outlined"));
    assert_ne!(out.source_sha256, out.sha256);
    let png = STANDARD
        .decode(out.preview_data_url.split_once(',').unwrap().1)
        .unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
}
#[test]
fn active_external_and_missing_text_are_rejected_instead_of_silently_lost() {
    for content in [
        "<script>alert(1)</script>",
        "<foreignObject/>",
        "<image href='https://example.org/x.png'/>",
        "<rect width='2' height='2' onload='x()'/>",
        "<rect width='2' height='2' fill='url(https://example.org/x)'/>",
        "<style>@import 'https://example.org/x';</style>",
        "<text x='1' y='2'>&#x4E00;</text>",
        "<meshgradient/>",
    ] {
        let svg = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'>{content}</svg>"
        );
        assert!(normalize(svg.as_bytes()).is_err(), "accepted {content}");
    }
}
#[test]
fn huge_viewports_and_empty_icons_are_rejected() {
    assert!(normalize(b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 200000 1'><path d='M0 0h10v1z'/></svg>").is_err());
    assert!(normalize(b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'/>").is_err());
}
#[test]
fn real_receptor_fixture_keeps_its_phosphorylation_marks_when_outlined() {
    let prepared = normalize(include_bytes!("fixtures/receptor.svg")).unwrap();
    assert!(
        prepared
            .normalization
            .iter()
            .any(|s| s.contains("text outlined"))
    );
    let png = STANDARD
        .decode(prepared.preview_data_url.split_once(',').unwrap().1)
        .unwrap();
    let pixmap = resvg::tiny_skia::Pixmap::decode_png(&png).unwrap();
    let black_marks = pixmap
        .pixels()
        .iter()
        .filter(|p| p.alpha() > 128 && p.red() < 20 && p.green() < 20 && p.blue() < 20)
        .count();
    assert!(
        black_marks > 10,
        "the two black P glyphs must survive, not just their coloured circles"
    );
    assert!(prepared.colors.contains(&"#e90f97".into()));
}
#[test]
fn recoloring_is_simultaneous_and_preserves_geometry_and_black_text_marks() {
    use std::collections::BTreeMap;
    fn structure(url: &str) -> Vec<(String, Vec<(String, String)>)> {
        use quick_xml::{Reader, events::Event};
        let bytes = STANDARD.decode(url.split_once(',').unwrap().1).unwrap();
        let mut reader = Reader::from_reader(bytes.as_slice());
        let mut result = Vec::new();
        loop {
            match reader.read_event().unwrap() {
                Event::Start(e) | Event::Empty(e) => {
                    let attrs = e
                        .attributes()
                        .map(|a| a.unwrap())
                        .filter(|a| {
                            !matches!(
                                a.key.as_ref(),
                                b"fill"
                                    | b"stroke"
                                    | b"stop-color"
                                    | b"flood-color"
                                    | b"lighting-color"
                            )
                        })
                        .map(|a| {
                            (
                                String::from_utf8_lossy(a.key.as_ref()).into_owned(),
                                String::from_utf8_lossy(&a.value).into_owned(),
                            )
                        })
                        .collect();
                    result.push((
                        String::from_utf8_lossy(e.name().as_ref()).into_owned(),
                        attrs,
                    ));
                }
                Event::Eof => break,
                _ => {}
            }
        }
        result
    }
    let original = normalize(include_bytes!("fixtures/receptor.svg")).unwrap();
    let bytes = STANDARD
        .decode(original.data_url.split_once(',').unwrap().1)
        .unwrap();
    let next = bioicons::vector::recolor(
        &bytes,
        &BTreeMap::from([
            ("#e90f97".into(), "#00aa88".into()),
            ("#39acf3".into(), "#e90f97".into()),
        ]),
    )
    .unwrap();
    assert_eq!(structure(&original.data_url), structure(&next.data_url));
    assert!(next.colors.contains(&"#00aa88".into()) && next.colors.contains(&"#e90f97".into()));
    assert!(!next.colors.contains(&"#39acf3".into()));
    assert_eq!((original.width, original.height), (next.width, next.height));
    let black_pixels = |url: &str| {
        let data = STANDARD.decode(url.split_once(',').unwrap().1).unwrap();
        let pixmap = resvg::tiny_skia::Pixmap::decode_png(&data).unwrap();
        pixmap
            .pixels()
            .iter()
            .filter(|p| p.alpha() > 128 && p.red() < 20 && p.green() < 20 && p.blue() < 20)
            .count()
    };
    assert_eq!(
        black_pixels(&original.preview_data_url),
        black_pixels(&next.preview_data_url)
    );
}
#[test]
fn invalid_missing_and_identity_colour_maps_are_explicit_errors() {
    use std::collections::BTreeMap;
    let source=b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 10 10'><path fill='#ff0000' d='M0 0h10v10H0z'/></svg>";
    for pairs in [
        vec![],
        vec![("#00ff00", "#000000")],
        vec![("#ff0000", "#ff0000")],
        vec![("#ff0000", "url(http://example.org)")],
        vec![("#FF0000", "#000000"), ("#ff0000", "#ffffff")],
    ] {
        let map = BTreeMap::from_iter(pairs.into_iter().map(|(a, b)| (a.into(), b.into())));
        assert!(bioicons::vector::recolor(source, &map).is_err());
    }
}
#[test]
fn recoloring_gradient_stops_keeps_offsets_opacity_and_references() {
    use std::collections::BTreeMap;
    let svg=br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><defs><linearGradient id="paint"><stop offset="0.2" stop-color="#ff0000" stop-opacity="0.3"/><stop offset="0.8" stop-color="#0000ff"/></linearGradient></defs><path fill="url(#paint)" d="M0 0h100v100H0z"/></svg>"##;
    let next =
        bioicons::vector::recolor(svg, &BTreeMap::from([("#ff0000".into(), "#00aa88".into())]))
            .unwrap();
    let canonical = String::from_utf8(
        STANDARD
            .decode(next.data_url.split_once(',').unwrap().1)
            .unwrap(),
    )
    .unwrap();
    assert!(canonical.contains("<linearGradient"));
    assert!(canonical.contains("stop-color=\"#00aa88\""));
    assert!(canonical.contains("stop-opacity=\"0.3\""));
    assert!(canonical.contains("offset=\"0.2\""));
    assert!(canonical.contains("fill=\"url(#"));
}
#[test]
fn normalized_vector_bytes_and_hash_survive_repeated_loading() {
    let first = normalize(include_bytes!("fixtures/receptor.svg")).unwrap();
    let bytes = STANDARD
        .decode(first.data_url.split_once(',').unwrap().1)
        .unwrap();
    let again = normalize(&bytes).unwrap();
    assert_eq!(first.data_url, again.data_url);
    assert_eq!(first.sha256, again.sha256);
    assert_eq!(first.preview_data_url, again.preview_data_url);
}
#[test]
fn colour_edit_does_not_silently_substitute_a_raw_text_font() {
    let svg=b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><text x='20' y='40' fill='#ff0000' font-family='serif'>P</text></svg>";
    let result = bioicons::vector::recolor(
        svg,
        &std::collections::BTreeMap::from([("#ff0000".into(), "#00aa88".into())]),
    );
    assert!(result.unwrap_err().to_string().contains("outline SVG text"));
}

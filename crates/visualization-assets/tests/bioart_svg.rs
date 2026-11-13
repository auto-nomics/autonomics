use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use visualization_assets::bioart::{Format, prepare_svg};

fn fixture() -> (bioart::Asset, bioart::AssetFile) {
    let file = bioart::AssetFile {
        asset_id: 1,
        file_id: 2,
        format: "SVG".into(),
        url: "https://example.org/fixture.svg".into(),
    };
    let asset = serde_json::from_value(json!({"id":1,"accession":"fixture","title":"Synthetic vector","description":"","source_url":"https://example.org/fixture","licenses":["Fixture license"],"creators":["Fixture creator"],"collections":[],"keywords":[],"ontology_terms":[],"created":null,"updated":null,"files":[file]})).unwrap();
    (asset, file)
}
const SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="40"><path fill="#ff0000" d="M0 0H80V40H0Z"/></svg>"##;

#[test]
fn default_png_keeps_original_provenance_protocol() {
    let (asset, mut file) = fixture();
    file.format = "PNG".into();
    let expected = json!({"source":"NIH BioArt","asset":asset,"file":file,"sha256":"fixture-hash"});
    let prepared: visualization_assets::bioart::PreparedAsset = bioart::CanvasImage {
        asset,
        file,
        sha256: "fixture-hash".into(),
        data_url: "data:image/png;base64,fixture".into(),
        width: 80,
        height: 40,
    }
    .into();
    assert_eq!(Format::default(), Format::Png);
    assert_eq!(prepared.provenance(), expected);
    assert_eq!(prepared.preview_url(), prepared.data_url);
    assert!(prepared.source_sha256.is_none());
}

#[test]
fn normalized_bioart_can_be_recolored_without_losing_attribution_or_other_instances() {
    let (asset, file) = fixture();
    let prepared = prepare_svg(asset, file, SVG).unwrap();
    assert_eq!(prepared.format, Format::Svg);
    assert_eq!(
        prepared.source_sha256.as_deref(),
        Some(format!("{:x}", Sha256::digest(SVG)).as_str())
    );
    let bytes = STANDARD
        .decode(
            prepared
                .data_url
                .strip_prefix("data:image/svg+xml;base64,")
                .unwrap(),
        )
        .unwrap();
    assert_eq!(prepared.sha256, format!("{:x}", Sha256::digest(bytes)));
    assert!(prepared.preview_url().starts_with("data:image/png;base64,"));
    assert!(prepared.metadata().get("dataURL").is_none());
    let empty = json!({"elements":[],"files":{},"appState":{}});
    let image = || visualization::image::Image {
        file_id: &prepared.sha256,
        data_url: &prepared.data_url,
        width: prepared.width,
        height: prepared.height,
        provenance: prepared.provenance(),
    };
    let (doc, target) = visualization::image::insert(&empty, image(), 0., 0.).unwrap();
    let (doc, other) = visualization::image::insert(&doc, image(), 300., 0.).unwrap();
    let changed = visualization_assets::recolor_document(
        &doc,
        &target,
        &[("#ff0000".into(), "#0000ff".into())].into(),
    )
    .unwrap();
    let element =
        |document: &Value, id: &str| visualization::image::element(document, id).unwrap().clone();
    let before = element(&doc, &target);
    let after = element(&changed, &target);
    for field in ["source", "asset", "file", "source_sha256", "normalization"] {
        assert_eq!(after["customData"][field], before["customData"][field]);
    }
    for field in ["id", "x", "y", "width", "height", "scale", "crop"] {
        assert_eq!(after[field], before[field]);
    }
    assert_ne!(after["fileId"], before["fileId"]);
    assert_eq!(after["customData"]["modifications"][0]["kind"], "recolor");
    assert_eq!(element(&changed, &other), element(&doc, &other));
    assert_eq!(
        changed["files"][&prepared.sha256],
        doc["files"][&prepared.sha256]
    );
}

#[test]
fn rejects_wrong_identity_format_and_active_svg() {
    let (asset, mut file) = fixture();
    file.asset_id = 9;
    assert!(prepare_svg(asset.clone(), file, SVG).is_err());
    let (_, mut file) = fixture();
    file.format = "PNG".into();
    assert!(prepare_svg(asset.clone(), file, SVG).is_err());
    for bytes in [
        br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#.as_slice(),
        b"not an SVG",
    ] {
        assert!(prepare_svg(asset.clone(), fixture().1, bytes).is_err());
    }
}

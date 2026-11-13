use base64::{Engine, engine::general_purpose::STANDARD};
use bioicons::parts::{self, Edit};
fn decode(url: &str) -> Vec<u8> {
    STANDARD.decode(url.split_once(',').unwrap().1).unwrap()
}
#[test]
fn stroke_width_changes_only_requested_path_and_keeps_geometry_and_sibling() {
    let source=br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><g stroke="#ff0088" stroke-width="1" fill="none"><path d="M20 20L20 80"/><path d="M60 20L60 80"/></g></svg>"##;
    let before = parts::inspect(source).unwrap();
    assert_eq!(before.parts[0].stroke_width, 1.);
    let edit: Edit = serde_json::from_value(
        serde_json::json!({"part_id":before.parts[0].part_id,"stroke_width":4.}),
    )
    .unwrap();
    let output = parts::edit(source, &before.sha256, &[edit]).unwrap();
    let after = parts::inspect(&decode(&output.data_url)).unwrap();
    assert_eq!(after.parts[0].stroke_width, 4.);
    assert_eq!(after.parts[1].stroke_width, 1.);
    assert_eq!(after.parts[0].fill, before.parts[0].fill);
    assert_eq!(after.parts[0].stroke, before.parts[0].stroke);
    assert_eq!(after.parts[1].bounds, before.parts[1].bounds);
    assert_eq!(after.parts[0].point_count, before.parts[0].point_count);
    assert!(after.parts[0].bounds[2] > before.parts[0].bounds[2]);
    assert_ne!(
        output.preview_data_url,
        bioicons::vector::normalize(source)
            .unwrap()
            .preview_data_url
    );
    assert!(
        parts::edit(
            source,
            "stale",
            &[serde_json::from_value(
                serde_json::json!({"part_id":before.parts[0].part_id,"stroke_width":4.})
            )
            .unwrap()]
        )
        .is_err()
    );
}
#[test]
fn stroke_width_rejects_invalid_numbers_and_zero_without_treating_them_as_hide() {
    let source=br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><path d="M20 20L20 80" stroke="#ff0088"/></svg>"##;
    let before = parts::inspect(source).unwrap();
    for width in [0., -1., 1000.001, f64::NAN, f64::INFINITY] {
        let edit = Edit {
            part_id: before.parts[0].part_id.clone(),
            fill: None,
            stroke: None,
            stroke_width: Some(width),
            opacity: None,
            translate: None,
            points: None,
        };
        assert!(parts::edit(source, &before.sha256, &[edit]).is_err());
    }
    assert_eq!(parts::inspect(source).unwrap().sha256, before.sha256);
}
#[test]
fn real_receptor_strokes_can_be_thickened_without_changing_path_points() {
    let outlined = bioicons::vector::normalize(include_bytes!("fixtures/receptor.svg")).unwrap();
    let normalized = decode(&outlined.data_url);
    let source = normalized.as_slice();
    let before = parts::inspect(source).unwrap();
    let targets = before
        .parts
        .iter()
        .filter(|p| p.stroke_width > 0.)
        .collect::<Vec<_>>();
    assert!(!targets.is_empty());
    assert!(targets.len() <= 32);
    let edits = targets
        .iter()
        .map(|p| Edit {
            part_id: p.part_id.clone(),
            fill: None,
            stroke: None,
            stroke_width: Some(p.stroke_width * 2.),
            opacity: None,
            translate: None,
            points: None,
        })
        .collect::<Vec<_>>();
    let output = parts::edit(source, &before.sha256, &edits).unwrap();
    let after = parts::inspect(&decode(&output.data_url)).unwrap();
    assert_eq!(after.parts.len(), before.parts.len());
    for (a, b) in before.parts.iter().zip(&after.parts) {
        assert_eq!(a.point_count, b.point_count);
        assert_eq!(a.fill, b.fill);
        assert_eq!(a.stroke, b.stroke);
        assert_eq!(a.opacity, b.opacity);
        assert!((b.stroke_width - a.stroke_width * 2.).abs() < 1e-6);
    }
    if let Ok(dir) = std::env::var("BIOICONS_STROKE_EVIDENCE") {
        std::fs::create_dir_all(&dir).unwrap();
        for (name, url) in [
            ("before.png", &before.preview_data_url),
            ("after.png", &output.preview_data_url),
        ] {
            std::fs::write(std::path::Path::new(&dir).join(name), decode(url)).unwrap();
        }
        std::fs::write(
            std::path::Path::new(&dir).join("after.svg"),
            decode(&output.data_url),
        )
        .unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join("inspection.json"),
            serde_json::to_vec_pretty(
                &serde_json::json!({"before":before.parts,"after":after.parts}),
            )
            .unwrap(),
        )
        .unwrap();
    }
}

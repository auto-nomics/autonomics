use base64::{Engine, engine::general_purpose::STANDARD};
use bioicons::{
    parts::{self, Edit},
    vector,
};
fn bytes(vector: &vector::PreparedSvg) -> Vec<u8> {
    STANDARD
        .decode(vector.data_url.split_once(',').unwrap().1)
        .unwrap()
}
const SVG:&[u8]=br##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="80"><defs><clipPath id="clip"><rect width="110" height="80"/></clipPath></defs><g transform="translate(10 5)" clip-path="url(#clip)"><path id="left" fill="#ff0000" d="M0 0h20v20H0z"/><path id="right" fill="#ff0000" d="M40 0h20v20H40z"/></g></svg>"##;
#[test]
fn same_colour_paths_are_distinct_and_resources_are_not_components() {
    let original = vector::normalize(SVG).unwrap();
    let source = bytes(&original);
    let read = parts::inspect(&source).unwrap();
    assert_eq!(read.sha256, original.sha256);
    assert_eq!(read.parts.len(), 2);
    assert_eq!(read.parts[0].bounds, [10., 5., 20., 20.]);
    assert_eq!(read.parts[1].bounds, [50., 5., 20., 20.]);
    assert!(
        read.annotated_data_url
            .starts_with("data:image/png;base64,")
    );
    let edits = [Edit {
        part_id: read.parts[0].part_id.clone(),
        fill: Some("#00ff00".into()),
        stroke: None,
        opacity: None,
        translate: None,
        points: None,
    }];
    let result = parts::edit(&source, &read.sha256, &edits).unwrap();
    let next = parts::inspect(&bytes(&result)).unwrap();
    assert_eq!(next.parts[0].fill, "#00ff00");
    assert_eq!(next.parts[1].fill, "#ff0000");
    assert_eq!(next.parts[0].bounds, read.parts[0].bounds);
    assert_eq!(next.parts[1].bounds, read.parts[1].bounds);
    assert_ne!(read.parts[0].part_id, next.parts[0].part_id);
    assert!(parts::edit(&bytes(&result), &read.sha256, &edits).is_err());
    let old_id_new_hash = parts::edit(&bytes(&result), &next.sha256, &edits);
    assert!(old_id_new_hash.is_err());
    assert!(parts::edit(&source, &read.sha256, &[edits[0].clone(), edits[0].clone()]).is_err());
}
#[test]
fn paint_opacity_validation_and_no_ops_do_not_silently_change_assets() {
    let source = bytes(&vector::normalize(SVG).unwrap());
    let read = parts::inspect(&source).unwrap();
    let edit = Edit {
        part_id: read.parts[0].part_id.clone(),
        fill: None,
        stroke: None,
        opacity: Some(0.4),
        translate: None,
        points: None,
    };
    let result = parts::edit(&source, &read.sha256, std::slice::from_ref(&edit)).unwrap();
    assert_ne!(result.preview_data_url, read.preview_data_url);
    for invalid in [
        Edit {
            opacity: Some(f64::NAN),
            ..edit.clone()
        },
        Edit {
            opacity: Some(1.01),
            ..edit.clone()
        },
        Edit {
            fill: Some("url(https://example.org)".into()),
            opacity: None,
            translate: None,
            points: None,
            ..edit.clone()
        },
        Edit {
            fill: Some("#ff0000".into()),
            opacity: None,
            translate: None,
            points: None,
            ..edit.clone()
        },
        Edit {
            opacity: None,
            translate: None,
            points: None,
            ..edit.clone()
        },
    ] {
        assert!(parts::edit(&source, &read.sha256, &[invalid]).is_err());
    }
    assert!(parts::edit(&source, &read.sha256, &[]).is_err());
    assert!(
        parts::inspect(b"<svg width='20' height='20'><text x='0' y='10'>A</text></svg>").is_err()
    );
}
#[test]
fn actual_receptor_paths_include_outlined_labels_and_preserve_them_unless_selected() {
    let source = bytes(&vector::normalize(include_bytes!("fixtures/receptor.svg")).unwrap());
    let read = parts::inspect(&source).unwrap();
    assert!(read.parts.len() > 5);
    let selected = read.parts.iter().find(|p| p.stroke == "#e90f97").unwrap();
    let result = parts::edit(
        &source,
        &read.sha256,
        &[Edit {
            part_id: selected.part_id.clone(),
            stroke: Some("#008844".into()),
            fill: None,
            opacity: None,
            translate: None,
            points: None,
        }],
    )
    .unwrap();
    let next = parts::inspect(&bytes(&result)).unwrap();
    assert_eq!(read.parts.len(), next.parts.len());
    for (old, new) in read.parts.iter().zip(&next.parts) {
        assert_eq!(old.bounds, new.bounds);
        assert_eq!(old.fill, new.fill);
        if old.index == selected.index {
            assert_eq!(new.stroke, "#008844");
        } else {
            assert_eq!(old.stroke, new.stroke);
        }
    }
}

#[test]
fn replacing_one_gradient_keeps_other_users_and_zero_opacity_only_hides_target() {
    let svg=br##"<svg width="100" height="50"><defs><linearGradient id="shared"><stop stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></linearGradient></defs><path id="a" fill="url(#shared)" d="M0 0h20v20H0z"/><path id="b" fill="url(#shared)" d="M40 0h20v20H40z"/></svg>"##;
    let source = bytes(&vector::normalize(svg).unwrap());
    let read = parts::inspect(&source).unwrap();
    assert!(read.parts[0].fill.starts_with("url("));
    let edited = parts::edit(
        &source,
        &read.sha256,
        &[Edit {
            part_id: read.parts[0].part_id.clone(),
            fill: Some("#334455".into()),
            stroke: None,
            opacity: None,
            translate: None,
            points: None,
        }],
    )
    .unwrap();
    let next = parts::inspect(&bytes(&edited)).unwrap();
    assert_eq!(next.parts[1].fill, read.parts[1].fill);
    let xml = String::from_utf8(bytes(&edited)).unwrap();
    assert!(xml.contains("linearGradient") && xml.contains("#ff0000") && xml.contains("#0000ff"));
    let hidden = parts::edit(
        &source,
        &read.sha256,
        &[Edit {
            part_id: read.parts[0].part_id.clone(),
            fill: None,
            stroke: None,
            opacity: Some(0.),
            translate: None,
            points: None,
        }],
    )
    .unwrap();
    let remaining = parts::inspect(&bytes(&hidden)).unwrap();
    assert_eq!(remaining.parts.len(), 2);
    assert_eq!(remaining.parts[1].fill, read.parts[1].fill);
    let decode = |url: &str| {
        resvg::tiny_skia::Pixmap::decode_png(
            &STANDARD.decode(url.split_once(',').unwrap().1).unwrap(),
        )
        .unwrap()
    };
    let before = decode(&read.preview_data_url);
    let after = decode(&hidden.preview_data_url);
    assert_eq!(after.pixel(10, 10).unwrap().alpha(), 0);
    assert_eq!(after.pixel(50, 10), before.pixel(50, 10));
}
#[test]
fn complex_icons_fail_explicitly_instead_of_truncating_parts() {
    let mut svg = String::from("<svg width='100' height='100'>");
    for _ in 0..257 {
        svg.push_str("<path d='M0 0h10v10H0z'/>");
    }
    svg.push_str("</svg>");
    assert!(
        parts::inspect(svg.as_bytes())
            .unwrap_err()
            .to_string()
            .contains("256")
    );
}

#[test]
fn movement_uses_root_coordinates_under_rotation_scaling_and_reflection() {
    let svg=br##"<svg width="300" height="200"><g transform="translate(180 20) rotate(30) scale(-2 3)"><path id="moving" d="M0 0h20v20H0z" fill="#ff0000"/><path id="fixed" d="M40 0h20v20H40z" fill="#ff0000"/></g></svg>"##;
    let source = bytes(&vector::normalize(svg).unwrap());
    let before = parts::inspect(&source).unwrap();
    let edit = Edit {
        part_id: before.parts[0].part_id.clone(),
        fill: None,
        stroke: None,
        opacity: None,
        translate: Some([13., -7.]),
        points: None,
    };
    let result = parts::edit(&source, &before.sha256, std::slice::from_ref(&edit)).unwrap();
    let after = parts::inspect(&bytes(&result)).unwrap();
    for (index, (a, b)) in before.parts.iter().zip(&after.parts).enumerate() {
        for axis in 0..4 {
            let expected = a.bounds[axis]
                + if index == 0 && axis < 2 {
                    [13., -7.][axis]
                } else {
                    0.
                };
            assert!(
                (b.bounds[axis] - expected).abs() < 0.001,
                "bounds {axis}: {} vs {expected}",
                b.bounds[axis]
            );
        }
        assert_eq!(a.fill, b.fill);
        assert_eq!(a.stroke, b.stroke);
    }
    let repeated = parts::edit(
        &bytes(&result),
        &after.sha256,
        &[Edit {
            part_id: after.parts[0].part_id.clone(),
            translate: Some([-13., 7.]),
            points: None,
            ..edit.clone()
        }],
    )
    .unwrap();
    let restored = parts::inspect(&bytes(&repeated)).unwrap();
    for axis in 0..4 {
        assert!((restored.parts[0].bounds[axis] - before.parts[0].bounds[axis]).abs() < 0.001);
    }
    for delta in [[f64::NAN, 0.], [100001., 0.], [0., f64::INFINITY], [0., 0.]] {
        assert!(
            parts::edit(
                &source,
                &before.sha256,
                &[Edit {
                    translate: Some(delta),
                    ..edit.clone()
                }]
            )
            .is_err()
        );
    }
}
#[test]
fn movement_carries_gradient_paint_but_keeps_shared_parent_clips_fixed() {
    let svg=br##"<svg width="100" height="60"><defs><linearGradient id="paint"><stop stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></linearGradient><clipPath id="clip"><rect width="20" height="20"/></clipPath></defs><path id="gradient" d="M0 30h20v20H0z" fill="url(#paint)"/><g clip-path="url(#clip)"><path id="clipped" d="M0 0h20v20H0z" fill="#ff0000"/></g><g transform="translate(40 0)" clip-path="url(#clip)"><path id="fixed" d="M0 0h20v20H0z" fill="#00ff00"/></g></svg>"##;
    let source = bytes(&vector::normalize(svg).unwrap());
    let before = parts::inspect(&source).unwrap();
    let edits = before
        .parts
        .iter()
        .take(2)
        .map(|p| Edit {
            part_id: p.part_id.clone(),
            fill: None,
            stroke: None,
            opacity: None,
            translate: Some([10., 0.]),
            points: None,
        })
        .collect::<Vec<_>>();
    let result = parts::edit(&source, &before.sha256, &edits).unwrap();
    let decode = |url: &str| {
        resvg::tiny_skia::Pixmap::decode_png(
            &STANDARD.decode(url.split_once(',').unwrap().1).unwrap(),
        )
        .unwrap()
    };
    let a = decode(&before.preview_data_url);
    let b = decode(&result.preview_data_url);
    assert_eq!(
        a.pixel(10, 40),
        b.pixel(20, 40),
        "gradient follows the moved path"
    );
    assert_eq!(
        b.pixel(25, 10).unwrap().alpha(),
        0,
        "parent clip stays fixed"
    );
    assert_eq!(
        a.pixel(50, 10),
        b.pixel(50, 10),
        "shared clip user remains intact"
    );
}

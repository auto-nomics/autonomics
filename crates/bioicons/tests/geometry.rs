use base64::{Engine, engine::general_purpose::STANDARD};
use bioicons::{
    geometry::{self, PointEdit},
    parts::{self, Edit},
    vector,
};
fn data(v: &vector::PreparedSvg) -> Vec<u8> {
    STANDARD
        .decode(v.data_url.split_once(',').unwrap().1)
        .unwrap()
}
fn edit(id: &str, points: Vec<PointEdit>) -> Edit {
    Edit {
        part_id: id.into(),
        fill: None,
        stroke: None,
        opacity: None,
        translate: None,
        points: Some(points),
    }
}
const SVG:&[u8]=br##"<svg width="300" height="250"><g transform="translate(180 30) rotate(25) scale(-2 3)"><path id="curve" fill="none" stroke="#008844" d="M0 0 C5 0 10 20 20 20 Q25 30 30 10 Z M40 0L50 10L60 0Z"/><path id="fixed" d="M0 40h20v20H0z" fill="#ff0000"/></g></svg>"##;
#[test]
fn sparse_control_and_anchor_edits_preserve_topology_and_other_points() {
    let original = vector::normalize(SVG).unwrap();
    let bytes = data(&original);
    let list = parts::inspect(&bytes).unwrap();
    let id = &list.parts[0].part_id;
    let before = geometry::inspect(&bytes, &list.sha256, id).unwrap();
    assert!(before.points.iter().any(|p| p.kind == "control"));
    assert!(before.commands.iter().filter(|c| c.command == "M").count() > 1);
    let control = before.points.iter().find(|p| p.kind == "control").unwrap();
    let anchor = before
        .points
        .iter()
        .find(|p| p.index > control.index && p.kind == "anchor")
        .unwrap();
    let positions = vec![
        PointEdit {
            index: control.index,
            position: [control.position[0] + 5., control.position[1] - 9.],
        },
        PointEdit {
            index: anchor.index,
            position: [anchor.position[0] + 4., anchor.position[1] + 3.],
        },
    ];
    let result = parts::edit(&bytes, &list.sha256, &[edit(id, positions.clone())]).unwrap();
    let after_list = parts::inspect(&data(&result)).unwrap();
    let after = geometry::inspect(
        &data(&result),
        &after_list.sha256,
        &after_list.parts[0].part_id,
    )
    .unwrap();
    assert_eq!(
        before
            .commands
            .iter()
            .map(|c| (&c.command, &c.points))
            .collect::<Vec<_>>(),
        after
            .commands
            .iter()
            .map(|c| (&c.command, &c.points))
            .collect::<Vec<_>>()
    );
    for (old, new) in before.points.iter().zip(&after.points) {
        let expected = positions
            .iter()
            .find(|p| p.index == old.index)
            .map(|p| p.position)
            .unwrap_or(old.position);
        assert_eq!(old.kind, new.kind);
        for axis in 0..2 {
            assert!((new.position[axis] - expected[axis]).abs() < 0.001);
        }
    }
    assert_eq!(list.parts[1].bounds, after_list.parts[1].bounds);
    assert_eq!(list.parts[1].fill, after_list.parts[1].fill);
    assert_ne!(original.preview_data_url, result.preview_data_url);
    assert!(
        before
            .annotated_data_url
            .starts_with("data:image/png;base64,")
    );
}
#[test]
fn invalid_point_changes_and_snapshot_mismatch_fail_explicitly() {
    let bytes = data(&vector::normalize(SVG).unwrap());
    let list = parts::inspect(&bytes).unwrap();
    let id = &list.parts[0].part_id;
    let read = geometry::inspect(&bytes, &list.sha256, id).unwrap();
    let p = &read.points[0];
    for points in [
        vec![],
        vec![PointEdit {
            index: usize::MAX,
            position: [0., 0.],
        }],
        vec![PointEdit {
            index: 0,
            position: [f64::NAN, 0.],
        }],
        vec![PointEdit {
            index: 0,
            position: [1e6 + 1., 0.],
        }],
        vec![
            PointEdit {
                index: 0,
                position: [0., 0.]
            };
            65
        ],
        vec![
            PointEdit {
                index: 0,
                position: [0., 0.]
            };
            2
        ],
        vec![PointEdit {
            index: p.index,
            position: p.position,
        }],
    ] {
        assert!(parts::edit(&bytes, &list.sha256, &[edit(id, points)]).is_err());
    }
    let mut mixed = edit(
        id,
        vec![PointEdit {
            index: 0,
            position: [0., 0.],
        }],
    );
    mixed.translate = Some([1., 0.]);
    assert!(parts::edit(&bytes, &list.sha256, &[mixed]).is_err());
    assert!(geometry::inspect(&bytes, "stale", id).is_err());
    assert!(geometry::inspect(&bytes, &list.sha256, "missing").is_err());
}
#[test]
fn complex_paths_are_reported_without_truncating_point_indices() {
    let mut svg = String::from("<svg width='1000' height='100'><path d='M0 0");
    for x in 1..514 {
        svg.push_str(&format!(" L{x} {}", x % 5));
    }
    svg.push_str("' stroke='black' fill='none'/></svg>");
    let bytes = data(&vector::normalize(svg.as_bytes()).unwrap());
    let list = parts::inspect(&bytes).unwrap();
    assert!(list.parts[0].point_count > 512);
    assert!(!list.parts[0].geometry_supported);
    assert!(geometry::inspect(&bytes, &list.sha256, &list.parts[0].part_id).is_err());
}

#[test]
fn collapsing_a_contour_cannot_silently_remove_the_target_path() {
    let bytes=data(&vector::normalize(br##"<svg width="100" height="100"><path id="target" d="M0 0L20 0L20 20Z"/><path id="keep" d="M40 0h20v20H40z"/></svg>"##).unwrap());
    let list = parts::inspect(&bytes).unwrap();
    let id = &list.parts[0].part_id;
    let geometry = geometry::inspect(&bytes, &list.sha256, id).unwrap();
    let collapse = geometry
        .points
        .iter()
        .map(|p| PointEdit {
            index: p.index,
            position: [10., 10.],
        })
        .collect();
    if let Ok(result) = parts::edit(&bytes, &list.sha256, &[edit(id, collapse)]) {
        // usvg may retain a zero-area contour. It must remain addressable,
        // with its commands and point indices intact, rather than disappear.
        let after_list = parts::inspect(&data(&result)).unwrap();
        assert_eq!(after_list.parts.len(), list.parts.len());
        let after = geometry::inspect(
            &data(&result),
            &after_list.sha256,
            &after_list.parts[0].part_id,
        )
        .unwrap();
        assert_eq!(
            after
                .commands
                .iter()
                .map(|c| (&c.command, &c.points))
                .collect::<Vec<_>>(),
            geometry
                .commands
                .iter()
                .map(|c| (&c.command, &c.points))
                .collect::<Vec<_>>()
        );
        assert!(after.points.iter().all(|p| p.position == [10., 10.]));
    }
}

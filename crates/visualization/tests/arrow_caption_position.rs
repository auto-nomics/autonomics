use serde_json::{Value, json};
use visualization::scene::{self, Operation};
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/arrow-caption-component.json")).unwrap()
}
fn e<'a>(d: &'a Value, id: &str) -> &'a Value {
    d["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}
fn edit<'a>(d: &'a mut Value, id: &str) -> &'a mut Value {
    d["elements"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["id"] == id)
        .unwrap()
}
fn omit(v: &Value, keys: &[&str]) -> Value {
    Value::Object(
        v.as_object()
            .unwrap()
            .iter()
            .filter(|(k, _)| !keys.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    )
}
fn simple(v: &Value) -> Vec<[f64; 2]> {
    let mut out: Vec<[f64; 2]> = vec![];
    for p in v.as_array().unwrap() {
        let p = [p[0].as_f64().unwrap(), p[1].as_f64().unwrap()];
        while out.len() > 1 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            if ((a[0] - b[0]).abs() < 1e-6 && (b[0] - p[0]).abs() < 1e-6
                || (a[1] - b[1]).abs() < 1e-6 && (b[1] - p[1]).abs() < 1e-6)
                && (0..2).all(|axis| {
                    b[axis] >= a[axis].min(p[axis]) - 1e-6 && b[axis] <= a[axis].max(p[axis]) + 1e-6
                })
            {
                out.pop();
            } else {
                break;
            }
        }
        out.push(p);
    }
    out
}
#[test]
fn fractional_caption_position_keeps_the_actual_path_endpoints_science_and_typography() {
    let before = fixture();
    for position in [0.25, 0.5, 0.8] {
        let after = scene::apply(
            &before,
            &[Operation::PositionArrowCaption {
                id: "caption".into(),
                position,
            }],
        )
        .unwrap();
        let a = e(&after, "a");
        let old = e(&before, "a");
        assert_eq!(simple(&a["points"]), simple(&old["points"]));
        assert_eq!(
            omit(a, &["points", "version", "versionNonce", "updated"]),
            omit(old, &["points", "version", "versionNonce", "updated"])
        );
        assert_eq!(
            omit(
                e(&after, "caption"),
                &["x", "y", "version", "versionNonce", "updated"]
            ),
            omit(
                e(&before, "caption"),
                &["x", "y", "version", "versionNonce", "updated"]
            )
        );
        for id in ["f", "source", "target"] {
            assert_eq!(e(&after, id), e(&before, id));
        }
        assert_eq!(after["files"], before["files"]);
        let points = a["points"].as_array().unwrap();
        let n = points.len();
        let center = if n % 2 == 1 {
            [
                points[n / 2][0].as_f64().unwrap(),
                points[n / 2][1].as_f64().unwrap(),
            ]
        } else {
            [
                (points[n / 2 - 1][0].as_f64().unwrap() + points[n / 2][0].as_f64().unwrap()) / 2.,
                (points[n / 2 - 1][1].as_f64().unwrap() + points[n / 2][1].as_f64().unwrap()) / 2.,
            ]
        };
        let expected = if position == 0.25 {
            [150., 575.]
        } else if position == 0.5 {
            [150., 610.]
        } else {
            [140., 642.]
        };
        assert!((e(&after, "caption")["x"].as_f64().unwrap() + 130. - expected[0]).abs() < 1e-6);
        assert!((e(&after, "caption")["y"].as_f64().unwrap() + 20. - expected[1]).abs() < 1e-6);
        let label = e(&after, "caption");
        assert!(
            (label["x"].as_f64().unwrap() + 130. - a["x"].as_f64().unwrap() - center[0]).abs()
                < 1e-6
        );
        assert!(
            (label["y"].as_f64().unwrap() + 20. - a["y"].as_f64().unwrap() - center[1]).abs()
                < 1e-6
        );
    }
}
#[test]
fn unsupported_or_ambiguous_native_geometry_rejects_without_mutating_source() {
    for variant in [
        "text lock",
        "arrow lock",
        "detached",
        "reciprocal",
        "multiple",
        "curve",
        "elbow",
        "diagonal",
        "rotation",
        "frame",
    ] {
        let mut doc = fixture();
        match variant {
            "text lock" => edit(&mut doc, "caption")["locked"] = json!(true),
            "arrow lock" => edit(&mut doc, "a")["locked"] = json!(true),
            "detached" => edit(&mut doc, "caption")["containerId"] = Value::Null,
            "reciprocal" => edit(&mut doc, "a")["boundElements"] = json!([]),
            "multiple" => {
                let mut extra = e(&doc, "caption").clone();
                extra["id"] = json!("extra");
                doc["elements"].as_array_mut().unwrap().push(extra);
            }
            "curve" => edit(&mut doc, "a")["roundness"] = json!({"type":2}),
            "elbow" => edit(&mut doc, "a")["elbowed"] = json!(true),
            "diagonal" => edit(&mut doc, "a")["points"] = json!([[0, 0], [100, 100]]),
            "rotation" => edit(&mut doc, "a")["angle"] = json!(0.2),
            "frame" => edit(&mut doc, "f")["width"] = json!(100),
            _ => unreachable!(),
        }
        let old = doc.clone();
        assert!(
            scene::apply(
                &doc,
                &[Operation::PositionArrowCaption {
                    id: "caption".into(),
                    position: 0.5
                }]
            )
            .is_err(),
            "{variant}"
        );
        assert_eq!(doc, old);
    }
    for position in [0., 1., -0.1, f64::NAN] {
        assert!(
            scene::apply(
                &fixture(),
                &[Operation::PositionArrowCaption {
                    id: "caption".into(),
                    position
                }]
            )
            .is_err()
        );
    }
}

#[test]
fn balancing_a_dense_turning_path_is_bounded_and_does_not_save_partial_edits() {
    let mut doc = fixture();
    edit(&mut doc, "a")["points"] = json!(
        (0..512)
            .map(|i| [(i / 2) as f64, ((i + 1) / 2) as f64])
            .collect::<Vec<_>>()
    );
    edit(&mut doc, "a")["width"] = json!(255);
    edit(&mut doc, "a")["height"] = json!(256);
    let error = scene::apply(
        &doc,
        &[Operation::PositionArrowCaption {
            id: "caption".into(),
            position: 0.01,
        }],
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("512 points"), "{error}");
}

use serde_json::json;
use visualization::placement::{self, Spec};
fn spec() -> Spec {
    Spec {
        relative_to: "panel".into(),
        width: 280.,
        height: 360.,
        gap: 24.,
    }
}
fn document() -> serde_json::Value {
    json!({"elements":[{"id":"panel","type":"frame","x":0,"y":0,"width":320,"height":480},{"id":"inside","type":"rectangle","x":10,"y":10,"width":300,"height":100},{"id":"other-panel","type":"frame","x":460,"y":0,"width":320,"height":960},{"id":"scope","type":"text","x":10,"y":1000,"width":1200,"height":120,"fontSize":16}]})
}
#[test]
fn adjacent_space_is_ranked_without_mutating_or_reserving_the_scene() {
    let doc = document();
    let original = doc.clone();
    let review = placement::find(&doc, &spec()).unwrap();
    assert_eq!(review.status, "available");
    let first = &review.candidates[0];
    assert_eq!(first.position, "below");
    assert_eq!(first.bounds.x, 20.);
    assert_eq!(first.bounds.y, 504.);
    assert_eq!(first.added_bounding_area, 0.);
    assert_eq!(
        [
            first.resulting_bounds.x,
            first.resulting_bounds.y,
            first.resulting_bounds.width,
            first.resulting_bounds.height
        ],
        [0., 0., 1210., 1120.]
    );
    assert_eq!(first.resulting_aspect_ratio, 1210. / 1120.);
    assert!(
        review
            .candidates
            .iter()
            .all(|c| c.bounds.width == 280. && c.bounds.height == 360.)
    );
    assert_eq!(doc, original);
    let mut crowded = doc.clone();
    crowded["elements"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"new-note","type":"rectangle","x":20,"y":504,"width":280,"height":360}));
    let next = placement::find(&crowded, &spec()).unwrap();
    assert!(next.candidates.iter().all(|c| c.position != "below"));
    assert_eq!(next.candidates[0].added_bounding_area, 0.);
    let mut fully_crowded = crowded.clone();
    fully_crowded["elements"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"cover","type":"frame","x":0,"y":0,"width":1210,"height":1120}));
    let outside = placement::find(&fully_crowded, &spec()).unwrap();
    assert!(
        outside
            .candidates
            .iter()
            .any(|c| c.bounds.x + c.bounds.width <= -24.)
    );
    assert!(
        outside
            .candidates
            .iter()
            .all(|c| c.added_bounding_area > 0.)
    );
}

#[test]
fn bounded_internal_gaps_avoid_expanding_when_adjacent_slots_are_blocked() {
    let doc = json!({"elements":[
        {"id":"center","type":"rectangle","x":0,"y":0,"width":40,"height":40},
        {"id":"below","type":"rectangle","x":0,"y":50,"width":40,"height":40},
        {"id":"above","type":"rectangle","x":0,"y":-50,"width":40,"height":40},
        {"id":"left","type":"rectangle","x":-50,"y":0,"width":40,"height":40},
        {"id":"right","type":"rectangle","x":50,"y":0,"width":40,"height":40}
    ]});
    let before = doc.clone();
    let query = Spec {
        relative_to: "center".into(),
        width: 40.,
        height: 40.,
        gap: 10.,
    };
    let review = placement::find(&doc, &query).unwrap();
    assert_eq!(review.candidates[0].position, "internal_gap");
    assert_eq!(review.candidates[0].added_bounding_area, 0.);
    assert_eq!(
        [
            review.candidates[0].resulting_bounds.x,
            review.candidates[0].resulting_bounds.y,
            review.candidates[0].resulting_bounds.width,
            review.candidates[0].resulting_bounds.height
        ],
        [-50., -50., 140., 140.]
    );
    assert_eq!(review.candidates[0].resulting_aspect_ratio, 1.);
    assert!(review.candidates.len() <= 8);
    for c in &review.candidates {
        for e in doc["elements"].as_array().unwrap() {
            let x = e["x"].as_f64().unwrap();
            let y = e["y"].as_f64().unwrap();
            assert!(
                c.bounds.x + c.bounds.width <= x - query.gap
                    || c.bounds.x >= x + 40. + query.gap
                    || c.bounds.y + c.bounds.height <= y - query.gap
                    || c.bounds.y >= y + 40. + query.gap
            );
        }
    }
    assert_eq!(
        serde_json::to_value(&review).unwrap(),
        serde_json::to_value(placement::find(&doc, &query).unwrap()).unwrap()
    );
    assert_eq!(doc, before);
}
#[test]
fn partial_geometry_never_produces_a_clearance_suggestion() {
    let mut doc = document();
    doc["elements"].as_array_mut().unwrap().push(json!({"id":"curve","type":"arrow","x":0,"y":0,"width":100,"height":50,"points":[[0,0],[100,50]],"roundness":{"type":2}}));
    let review = placement::find(&doc, &spec()).unwrap();
    assert_eq!(review.status, "unavailable");
    assert_eq!(review.scene_geometry_review.skipped_element_count, 1);
    assert!(review.candidates.is_empty());
    doc["elements"][4]["isDeleted"] = json!(true);
    assert_eq!(placement::find(&doc, &spec()).unwrap().status, "available");
    doc["elements"]
        .as_array_mut()
        .unwrap()
        .extend((0..5000).map(
            |i| json!({"id":format!("r-{i}"),"type":"rectangle","x":0,"y":0,"width":1,"height":1}),
        ));
    let review = placement::find(&doc, &spec()).unwrap();
    assert!(review.scene_geometry_review.omitted_element_count > 0);
    assert!(review.candidates.is_empty());
}
#[test]
fn invalid_and_ambiguous_references_fail_and_rotation_uses_native_extents() {
    let mut doc = document();
    doc["elements"][0]["angle"] = json!(0.4);
    let review = placement::find(&doc, &spec()).unwrap();
    assert_eq!(review.status, "available");
    doc["elements"][0]["isDeleted"] = json!(true);
    assert!(placement::find(&doc, &spec()).is_err());
    doc["elements"][0]["isDeleted"] = json!(false);
    let duplicate = doc["elements"][0].clone();
    doc["elements"].as_array_mut().unwrap().push(duplicate);
    assert!(placement::find(&doc, &spec()).is_err());
    let mut invalid = spec();
    invalid.width = f64::NAN;
    assert!(placement::validate(&invalid).is_err());
    invalid.width = 0.;
    assert!(placement::validate(&invalid).is_err());
    invalid.width = 280.;
    invalid.gap = 3.;
    assert!(placement::validate(&invalid).is_err());
}

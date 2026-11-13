use serde_json::json;
use visualization::{Store, planning::Brief};
#[test]
fn compiled_panel_metrics_change_only_after_explicit_plan_save_without_scene_or_proposal_writes() {
    let mut store = Store::in_memory().unwrap();
    store.create_project("p", "Layout").unwrap();
    let original = json!({"elements":[]});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &original)
        .unwrap();
    let mut brief:Brief=serde_json::from_value(json!({"message":"Compare concepts","audience":"Researchers","layout":"vertical","relationship_note":"Independent concepts","panels":[{"id":"a","title":"Panel A","column":0,"row":0},{"id":"b","title":"Panel B","column":1,"row":0},{"id":"c","title":"Panel C","column":2,"row":0}],"nodes":[{"id":"a","label":"A","panel":"a"},{"id":"b","label":"B","panel":"b"},{"id":"c","label":"C","panel":"c"}]})).unwrap();
    let first = store.save_planning("p", &figure.id, 1, 0, &brief).unwrap();
    assert_eq!(first.status, "ready");
    let metrics = first.layout_review.as_ref().unwrap();
    assert_eq!(metrics.status, "available");
    let bounds = metrics.bounds.as_ref().unwrap();
    assert_eq!(
        [bounds.x, bounds.y, bounds.width, bounds.height],
        [0., 0., 1160., 285.]
    );
    assert_eq!(metrics.panels.len(), 3);
    assert_eq!(metrics.panels[2].bounds.x, 840.);
    let reread = store.planning("p", &figure.id).unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&reread.layout_review).unwrap(),
        serde_json::to_value(&first.layout_review).unwrap()
    );
    brief.panels[2].column = 1;
    brief.panels[2].row = 1;
    let next = store
        .save_planning("p", &figure.id, 1, first.version, &brief)
        .unwrap();
    let metrics = next.layout_review.unwrap();
    let bounds = metrics.bounds.unwrap();
    assert_eq!([bounds.width, bounds.height], [740., 670.]);
    assert!(
        metrics.aspect_ratio.unwrap() < first.layout_review.as_ref().unwrap().aspect_ratio.unwrap()
    );
    assert_eq!(
        store
            .planning_revision("p", &figure.id, first.version)
            .unwrap()
            .document
            .panels[2]
            .column,
        2
    );
    assert!(store.proposals("p", &figure.id).unwrap().is_empty());
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
    assert_eq!(
        store.revision("p", &figure.id, 1).unwrap().document,
        original
    );
}
#[test]
fn failed_compilation_reports_unavailable_instead_of_zero_or_guessed_size() {
    let brief:Brief=serde_json::from_value(json!({"message":"Incomplete","audience":"Researchers","layout":"grid","nodes":[{"id":"a","label":"A"}]})).unwrap();
    let review = brief.layout_review();
    assert_eq!(review.status, "unavailable");
    assert!(review.bounds.is_none());
    assert!(review.aspect_ratio.is_none());
    assert!(review.panels.is_empty());
    assert!(review.error.unwrap().contains("column and row"));
}

#[test]
fn long_labels_use_real_compiler_lines_with_bounded_read_only_results() {
    let label = "需要保留的科研组件名称与明确状态".repeat(5);
    let nodes = (0..25)
        .map(|i| json!({"id":format!("n{i}"),"label":label}))
        .collect::<Vec<_>>();
    let brief: Brief = serde_json::from_value(json!({"message":"Readable labels","audience":"Researchers","layout":"vertical","nodes":nodes})).unwrap();
    let before = serde_json::to_value(&brief).unwrap();
    let review = brief.layout_review();
    assert_eq!(review.status, "available");
    assert_eq!(review.long_label_count, 25);
    assert_eq!(review.long_labels.len(), 20);
    assert_eq!(review.long_labels_omitted, 5);
    assert_eq!(
        review.long_labels[0].plan_field.as_deref(),
        Some("nodes[0].label")
    );
    assert_eq!(
        review.long_labels[19].plan_field.as_deref(),
        Some("nodes[19].label")
    );
    assert!(review.long_labels.iter().all(|item| item.lines >= 4
        && item.width > 0.
        && item.height > 0.
        && item.preview == label));
    assert_eq!(serde_json::to_value(&brief).unwrap(), before);
    let short: Brief = serde_json::from_value(
        json!({"message":"Short","audience":"Researchers","nodes":[{"id":"a","label":"ATP"}]}),
    )
    .unwrap();
    assert_eq!(short.layout_review().long_label_count, 0);
    let mut legacy = serde_json::to_value(short.layout_review()).unwrap();
    for key in ["long_labels", "long_label_count", "long_labels_omitted"] {
        legacy.as_object_mut().unwrap().remove(key);
    }
    let restored: visualization::planning::LayoutReview = serde_json::from_value(legacy).unwrap();
    assert!(restored.long_labels.is_empty());
}

#[test]
fn panel_and_relation_labels_refer_to_saved_plan_fields() {
    let text = "必须保留的图形说明与限定".repeat(6);
    let brief: Brief = serde_json::from_value(json!({"message":"Fields","audience":"Researchers","layout":"vertical","panels":[{"id":"p","title":text,"column":0,"row":0},{"id":"q","title":"Other","column":1,"row":0}],"nodes":[{"id":"b","label":"B","panel":"q"},{"id":"a","label":"A","panel":"p"}],"edges":[{"from":"a","to":"b","label":text}]})).unwrap();
    let review = brief.layout_review();
    assert_eq!(review.status, "available");
    let fields = review
        .long_labels
        .iter()
        .filter_map(|item| item.plan_field.as_deref())
        .collect::<Vec<_>>();
    assert!(fields.contains(&"panels[0].title"));
    assert!(fields.contains(&"edges[0].label"));
}

#[test]
fn label_context_distinguishes_same_panel_cross_panel_and_unspecified_membership() {
    let text = "需要保留的科研标签和区室限定".repeat(6);
    let mut brief:Brief=serde_json::from_value(json!({"message":"Topology context","audience":"Researchers","layout":"vertical","panels":[{"id":"p","title":text,"column":0,"row":0},{"id":"q","title":"Other","column":1,"row":0}],"nodes":[{"id":"a","label":text,"panel":"p"},{"id":"b","label":"B","panel":"p"},{"id":"c","label":"C","panel":"q"}],"edges":[{"from":"a","to":"b","label":text},{"from":"b","to":"c","label":text}]})).unwrap();
    let snapshot = serde_json::to_value(&brief).unwrap();
    let review = serde_json::to_value(brief.layout_review()).unwrap();
    let context = |field: &str| {
        review["long_labels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|label| label["plan_field"] == field)
            .unwrap()["context"]
            .clone()
    };
    assert_eq!(
        context("edges[0].label"),
        json!({"kind":"relation","from_node_id":"a","to_node_id":"b","from_panel_id":"p","to_panel_id":"p","cross_panel":false})
    );
    assert_eq!(
        context("edges[1].label"),
        json!({"kind":"relation","from_node_id":"b","to_node_id":"c","from_panel_id":"p","to_panel_id":"q","cross_panel":true})
    );
    assert_eq!(
        context("nodes[0].label"),
        json!({"kind":"node","node_id":"a","panel_id":"p"})
    );
    assert_eq!(
        context("panels[0].title"),
        json!({"kind":"panel","panel_id":"p"})
    );
    assert_eq!(serde_json::to_value(&brief).unwrap(), snapshot);
    let mut legacy = review;
    for label in legacy["long_labels"].as_array_mut().unwrap() {
        label.as_object_mut().unwrap().remove("context");
    }
    let restored: visualization::planning::LayoutReview = serde_json::from_value(legacy).unwrap();
    assert!(
        restored
            .long_labels
            .iter()
            .all(|label| label.context.is_none())
    );
    brief.panels.clear();
    for node in &mut brief.nodes {
        node.panel = None;
    }
    let review = serde_json::to_value(brief.layout_review()).unwrap();
    let label = review["long_labels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|label| label["plan_field"] == "edges[0].label")
        .unwrap();
    assert!(label["context"]["cross_panel"].is_null());
    assert!(label["context"]["from_panel_id"].is_null());
}

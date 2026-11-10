use serde_json::{Value, json};
use visualization::{
    Store, image,
    scene::{self, Operation},
};
use visualization_assets::candidate::{Replacement, replace};

fn fixture() -> (Store, String, Value) {
    let mut store = Store::in_memory().unwrap();
    store.create_project("p", "Study").unwrap();
    store.create_project("other", "Other").unwrap();
    let plan = serde_json::from_value(json!({"message":"Generic pathway","audience":"Researchers","layout":"horizontal","nodes":[{"id":"a","label":"Receptor","evidence":["supplied source A"]},{"id":"b","label":"Response","evidence":["supplied source B"]}],"edges":[{"from":"a","to":"b","label":"activates","evidence":["supplied relation source"]}]})).unwrap();
    let original = visualization::sketch::compile(&plan).unwrap();
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &original)
        .unwrap();
    (store, figure.id, original)
}
fn asset() -> image::Image<'static> {
    image::Image {
        file_id: "raster-fixture",
        data_url: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==",
        width: 1,
        height: 1,
        provenance: json!({"source":"Fixture","asset":{"license":"cc-by-4.0","author":"Fixture author","source_url":"https://example.org/generic"}}),
    }
}
fn by_id<'a>(document: &'a Value, id: &str) -> &'a Value {
    document["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}
#[test]
fn cumulative_materialization_preserves_parent_edits_graph_files_and_formal_revision() {
    let (mut store, figure, original) = fixture();
    let edited = scene::apply(
        &original,
        &[Operation::AddWrappedText {
            id: "scope".into(),
            text: "Generic art; supplied mechanism".into(),
            x: 0.,
            y: 800.,
            max_width: 600.,
            font_size: Some(16.),
            frame_id: None,
        }],
    )
    .unwrap();
    let parent = store
        .propose("p", &figure, 1, "Add scope", &edited)
        .unwrap();
    let (child, first_image) = replace(
        &store,
        "p",
        &figure,
        1,
        Replacement {
            parent_id: &parent.id,
            parent_document_sha256: &parent.document_sha256,
            target: "node-0",
            image: asset(),
            summary: "Illustrate receptor",
            markup_ids: None,
        },
    )
    .unwrap();
    let (grandchild, second_image) = replace(
        &store,
        "p",
        &figure,
        1,
        Replacement {
            parent_id: &child.id,
            parent_document_sha256: &child.document_sha256,
            target: "node-1",
            image: asset(),
            summary: "Illustrate response",
            markup_ids: Some(&[]),
        },
    )
    .unwrap();
    assert_ne!(first_image, second_image);
    assert_eq!(
        by_id(&grandchild.document, "node-0")["strokeWidth"],
        by_id(&original, "node-0")["strokeWidth"]
    );
    assert_eq!(
        by_id(&grandchild.document, "node-1")["strokeWidth"],
        by_id(&original, "node-1")["strokeWidth"]
    );
    assert_eq!(
        grandchild.parent_proposal_id.as_deref(),
        Some(child.id.as_str())
    );
    assert_eq!(
        grandchild.parent_document_sha256.as_deref(),
        Some(child.document_sha256.as_str())
    );
    assert_eq!(
        by_id(&grandchild.document, &first_image),
        by_id(&child.document, &first_image)
    );
    assert_eq!(
        by_id(&grandchild.document, "scope"),
        by_id(&edited, "scope")
    );
    for e in original["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "arrow")
    {
        assert_eq!(by_id(&grandchild.document, e["id"].as_str().unwrap()), e);
    }
    for node in ["node-0", "node-1"] {
        assert_eq!(
            by_id(&grandchild.document, node)["customData"]["evidence"],
            by_id(&original, node)["customData"]["evidence"]
        );
    }
    assert_eq!(grandchild.document["files"], child.document["files"]);
    assert_eq!(
        store.proposal("p", &figure, &parent.id).unwrap().document,
        edited
    );
    assert_eq!(
        store.proposal("p", &figure, &child.id).unwrap().document,
        child.document
    );
    assert_eq!(store.revision("p", &figure, 1).unwrap().document, original);
    assert_eq!(store.get("p", &figure).unwrap().revision, 1);
    store.accept_proposal("p", &figure, &grandchild.id).unwrap();
    assert_eq!(
        store.revision("p", &figure, 2).unwrap().document,
        grandchild.document
    );
}
#[test]
fn stale_hash_scope_rejected_parent_and_missing_target_fail_without_writes() {
    let (store, figure, original) = fixture();
    let parent = store.propose("p", &figure, 1, "Parent", &original).unwrap();
    for (project, revision, hash, target) in [
        ("other", 1, parent.document_sha256.as_str(), "node-0"),
        ("p", 2, parent.document_sha256.as_str(), "node-0"),
        ("p", 1, "wrong hash", "node-0"),
        ("p", 1, parent.document_sha256.as_str(), "missing"),
    ] {
        assert!(
            replace(
                &store,
                project,
                &figure,
                revision,
                Replacement {
                    parent_id: &parent.id,
                    parent_document_sha256: hash,
                    target,
                    image: asset(),
                    summary: "Invalid",
                    markup_ids: None
                }
            )
            .is_err()
        );
        assert_eq!(store.proposals("p", &figure).unwrap().len(), 1);
    }
    store.reject_proposal("p", &figure, &parent.id).unwrap();
    assert!(
        replace(
            &store,
            "p",
            &figure,
            1,
            Replacement {
                parent_id: &parent.id,
                parent_document_sha256: &parent.document_sha256,
                target: "node-0",
                image: asset(),
                summary: "Invalid",
                markup_ids: None
            }
        )
        .is_err()
    );
    assert_eq!(store.proposals("p", &figure).unwrap().len(), 1);
    assert_eq!(store.revision("p", &figure, 1).unwrap().document, original);
}
#[test]
fn open_reviews_require_explicit_retain_or_drop_and_never_resolve_on_refinement() {
    let (store, figure, original) = fixture();
    let ink = [visualization::markup::Stroke {
        kind: visualization::markup::Kind::Cross,
        points: vec![[0., 0.], [10., 10.]],
    }];
    let review = store
        .create_markup("p", &figure, 1, "Use generic art", &ink, asset().data_url)
        .unwrap();
    let links = [review.id.clone()];
    let parent = store
        .propose_with_markups("p", &figure, 1, "Parent", &original, &links)
        .unwrap();
    let refine = |ids| {
        replace(
            &store,
            "p",
            &figure,
            1,
            Replacement {
                parent_id: &parent.id,
                parent_document_sha256: &parent.document_sha256,
                target: "node-0",
                image: asset(),
                summary: "Refine",
                markup_ids: ids,
            },
        )
    };
    assert!(refine(None).is_err());
    let missing = ["missing-review".into()];
    assert!(refine(Some(&missing)).is_err());
    assert_eq!(store.proposals("p", &figure).unwrap().len(), 1);
    let (dropped, _) = refine(Some(&[])).unwrap();
    assert!(dropped.review_markups.is_empty());
    let (retained, _) = refine(Some(&links)).unwrap();
    assert_eq!(retained.review_markups[0].id, links[0]);
    assert_eq!(retained.review_markups[0].status, "open");
    assert_eq!(
        store
            .proposal("p", &figure, &parent.id)
            .unwrap()
            .review_markups[0]
            .status,
        "open"
    );
}

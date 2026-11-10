use serde_json::{Value, json};
use std::collections::BTreeMap;
use visualization::{Store, image};
use visualization_assets::candidate::{self, Inspection, Reference, SvgEdit, SvgRefinement};
fn by_id<'a>(doc: &'a Value, id: &str) -> &'a Value {
    doc["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}
fn reference(p: &visualization::proposal::Proposal) -> Reference<'_> {
    Reference {
        proposal_id: &p.id,
        document_sha256: &p.document_sha256,
    }
}
fn fixture() -> (Store, String, visualization::proposal::Proposal, String) {
    let mut store = Store::in_memory().unwrap();
    store.create_project("p", "Study").unwrap();
    store.create_project("other", "Other").unwrap();
    let plan=serde_json::from_value(json!({"message":"Conceptual relation","audience":"Researchers","layout":"horizontal","nodes":[{"id":"a","label":"Receptor","evidence":["source A"]},{"id":"b","label":"Response"}],"edges":[{"from":"a","to":"b","label":"declared relation","evidence":["source B"]}]})).unwrap();
    let original = visualization::sketch::compile(&plan).unwrap();
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &original)
        .unwrap();
    let vector=bioicons::vector::normalize(b"<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'><g transform='translate(10 5)'><path fill='#ff0000' d='M0 0h20v20H0z'/><path fill='#00ff00' d='M40 0h20v20H40z'/></g></svg>").unwrap();
    let asset = || image::Image {
        file_id: &vector.sha256,
        data_url: &vector.data_url,
        width: vector.width,
        height: vector.height,
        provenance: json!({"source":"BioIcons","source_sha256":"original-source","asset":{"author":"Fixture","license":"cc-0","source_url":"https://example.org/fixture"}}),
    };
    let (illustrated, _) = image::illustrate(&original, "node-0", asset()).unwrap();
    let (doc, other) = image::insert(&illustrated, asset(), 900., 100.).unwrap();
    let parent = store
        .propose("p", &figure.id, 1, "Unadopted illustration", &doc)
        .unwrap();
    (store, figure.id, parent, other)
}
#[test]
fn unadopted_svg_can_be_inspected_recolored_translated_and_edited_without_formal_changes() {
    let (mut store, figure, parent, other) = fixture();
    let original = store.revision("p", &figure, 1).unwrap().document;
    assert!(
        visualization_assets::palette(&store, "p", &figure, 1, "node-0").is_err(),
        "formal scene has no image"
    );
    let inspect = |p: &visualization::proposal::Proposal, kind| {
        candidate::inspect_svg(&store, "p", &figure, 1, Some(reference(p)), "node-0", kind).unwrap()
    };
    let palette = inspect(&parent, Inspection::Palette);
    assert_eq!(palette["scope"], "proposal_candidate");
    assert_eq!(palette["document_sha256"], parent.document_sha256);
    assert_eq!(palette["proposal_id"], parent.id);
    assert!(
        palette["colors"]
            .as_array()
            .unwrap()
            .contains(&json!("#ff0000"))
    );
    let colors = BTreeMap::from([("#ff0000".into(), "#0044aa".into())]);
    let blue = candidate::refine_svg(
        &store,
        "p",
        &figure,
        1,
        SvgRefinement {
            reference: Some(reference(&parent)),
            target: "node-0",
            edit: SvgEdit::Recolor(&colors),
            markup_ids: Some(&[]),
        },
    )
    .unwrap();
    assert_eq!(
        blue.parent_document_sha256.as_deref(),
        Some(parent.document_sha256.as_str())
    );
    assert_eq!(
        by_id(&blue.document, &other),
        by_id(&parent.document, &other)
    );
    let parts = inspect(&blue, Inspection::Parts);
    assert_eq!(parts["scope"], "proposal_candidate");
    assert_eq!(
        parts["file_id"],
        by_id(&blue.document, palette["element_id"].as_str().unwrap())["fileId"]
    );
    assert_ne!(parts["file_id"], palette["file_id"]);
    let old_parts = inspect(&parent, Inspection::Parts);
    assert!(
        candidate::inspect_svg(
            &store,
            "p",
            &figure,
            1,
            Some(reference(&blue)),
            "node-0",
            Inspection::Geometry {
                sha256: old_parts["sha256"].as_str().unwrap(),
                part_id: old_parts["parts"][0]["part_id"].as_str().unwrap()
            }
        )
        .is_err(),
        "parent path IDs do not apply to the new file"
    );
    let edits = [bioicons::parts::Edit {
        part_id: parts["parts"][0]["part_id"].as_str().unwrap().into(),
        fill: None,
        stroke: None,
        opacity: None,
        translate: Some([3., 0.]),
        points: None,
    }];
    let moved = candidate::refine_svg(
        &store,
        "p",
        &figure,
        1,
        SvgRefinement {
            reference: Some(reference(&blue)),
            target: "node-0",
            edit: SvgEdit::Parts {
                sha256: parts["sha256"].as_str().unwrap(),
                edits: &edits,
            },
            markup_ids: None,
        },
    )
    .unwrap();
    let moved_parts = inspect(&moved, Inspection::Parts);
    assert!(
        (moved_parts["parts"][0]["bounds"][0].as_f64().unwrap()
            - parts["parts"][0]["bounds"][0].as_f64().unwrap()
            - 3.)
            .abs()
            < 1e-5
    );
    assert_eq!(
        moved_parts["parts"][1]["bounds"],
        parts["parts"][1]["bounds"]
    );
    let geometry = inspect(
        &moved,
        Inspection::Geometry {
            sha256: moved_parts["sha256"].as_str().unwrap(),
            part_id: moved_parts["parts"][0]["part_id"].as_str().unwrap(),
        },
    );
    assert_eq!(geometry["file_id"], moved_parts["file_id"]);
    let first = &geometry["points"][0];
    let point = bioicons::geometry::PointEdit {
        index: first["index"].as_u64().unwrap() as usize,
        position: [
            first["position"][0].as_f64().unwrap() + 1.,
            first["position"][1].as_f64().unwrap() + 1.,
        ],
    };
    let sparse = [bioicons::parts::Edit {
        part_id: geometry["part_id"].as_str().unwrap().into(),
        fill: None,
        stroke: None,
        opacity: None,
        translate: None,
        points: Some(vec![point]),
    }];
    let final_candidate = candidate::refine_svg(
        &store,
        "p",
        &figure,
        1,
        SvgRefinement {
            reference: Some(reference(&moved)),
            target: "node-0",
            edit: SvgEdit::Parts {
                sha256: geometry["sha256"].as_str().unwrap(),
                edits: &sparse,
            },
            markup_ids: Some(&[]),
        },
    )
    .unwrap();
    let image_id = palette["element_id"].as_str().unwrap();
    for e in parent.document["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["id"] != image_id)
    {
        assert_eq!(
            by_id(&final_candidate.document, e["id"].as_str().unwrap()),
            e
        );
    }
    let image = by_id(&final_candidate.document, image_id);
    assert_eq!(
        image["customData"]["asset"],
        by_id(&parent.document, image_id)["customData"]["asset"]
    );
    assert_eq!(image["customData"]["source_sha256"], "original-source");
    assert_eq!(
        image["customData"]["modifications"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    for p in [&parent, &blue, &moved] {
        assert_eq!(
            store.proposal("p", &figure, &p.id).unwrap().document,
            p.document
        );
        for (id, file) in p.document["files"].as_object().unwrap() {
            assert_eq!(final_candidate.document["files"][id], *file);
        }
    }
    assert_eq!(store.revision("p", &figure, 1).unwrap().document, original);
    assert_eq!(store.get("p", &figure).unwrap().revision, 1);
    store
        .accept_proposal("p", &figure, &final_candidate.id)
        .unwrap();
    assert_eq!(
        store.revision("p", &figure, 2).unwrap().document,
        final_candidate.document
    );
}
#[test]
fn wrong_candidate_hash_scope_revision_and_rejected_parent_never_fall_back_to_saved_art() {
    let (store, figure, parent, _) = fixture();
    let count = store.proposals("p", &figure).unwrap().len();
    let colors = BTreeMap::from([("#ff0000".into(), "#0044aa".into())]);
    for (project, rev, hash) in [
        ("p", 1, "wrong"),
        ("other", 1, parent.document_sha256.as_str()),
        ("p", 2, parent.document_sha256.as_str()),
    ] {
        let r = Reference {
            proposal_id: &parent.id,
            document_sha256: hash,
        };
        assert!(
            candidate::inspect_svg(
                &store,
                project,
                &figure,
                rev,
                Some(r),
                "node-0",
                Inspection::Palette
            )
            .is_err()
        );
        assert!(
            candidate::refine_svg(
                &store,
                project,
                &figure,
                rev,
                SvgRefinement {
                    reference: Some(r),
                    target: "node-0",
                    edit: SvgEdit::Recolor(&colors),
                    markup_ids: None
                }
            )
            .is_err()
        );
        assert_eq!(store.proposals("p", &figure).unwrap().len(), count);
    }
    assert!(Reference::optional(Some(&parent.id), None).is_err());
    assert!(Reference::optional(None, Some(&parent.document_sha256)).is_err());
    store.reject_proposal("p", &figure, &parent.id).unwrap();
    assert!(
        candidate::inspect_svg(
            &store,
            "p",
            &figure,
            1,
            Some(reference(&parent)),
            "node-0",
            Inspection::Parts
        )
        .is_err()
    );
    assert_eq!(store.proposals("p", &figure).unwrap().len(), count);
}
#[test]
fn candidate_svg_edits_require_review_decisions_but_reads_do_not_modify_review_links() {
    let (store, figure, parent, _) = fixture();
    let ink = [visualization::markup::Stroke {
        kind: visualization::markup::Kind::Cross,
        points: vec![[0., 0.], [10., 10.]],
    }];
    let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
    let review = store
        .create_markup("p", &figure, 1, "Adjust art", &ink, png)
        .unwrap();
    let ids = [review.id.clone()];
    let linked = store
        .propose_with_markups("p", &figure, 1, "Linked", &parent.document, &ids)
        .unwrap();
    candidate::inspect_svg(
        &store,
        "p",
        &figure,
        1,
        Some(reference(&linked)),
        "node-0",
        Inspection::Parts,
    )
    .unwrap();
    let colors = BTreeMap::from([("#ff0000".into(), "#0044aa".into())]);
    let refine = |links| {
        candidate::refine_svg(
            &store,
            "p",
            &figure,
            1,
            SvgRefinement {
                reference: Some(reference(&linked)),
                target: "node-0",
                edit: SvgEdit::Recolor(&colors),
                markup_ids: links,
            },
        )
    };
    let missing = ["missing".into()];
    let count = store.proposals("p", &figure).unwrap().len();
    assert!(refine(None).is_err());
    assert!(refine(Some(&missing)).is_err());
    assert_eq!(store.proposals("p", &figure).unwrap().len(), count);
    assert!(refine(Some(&[])).unwrap().review_markups.is_empty());
    let retained = refine(Some(&ids)).unwrap();
    assert_eq!(retained.review_markups[0].id, review.id);
    assert_eq!(retained.review_markups[0].status, "open");
    assert_eq!(
        store
            .proposal("p", &figure, &linked.id)
            .unwrap()
            .review_markups[0]
            .status,
        "open"
    );
}

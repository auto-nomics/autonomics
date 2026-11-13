use serde_json::json;
use visualization::{
    Store,
    proposal::{Proposal, ProposedRevision},
};

fn revise(store: &Store, figure: &str, parent: &Proposal) -> visualization::Result<Proposal> {
    store.propose_revision(
        "p",
        figure,
        1,
        ProposedRevision {
            parent_id: &parent.id,
            parent_document_sha256: &parent.document_sha256,
            summary: "continued edit",
            document: &json!({"elements":[]}),
            markup_ids: &[],
        },
    )
}

#[test]
fn leaf_policy_rejects_stale_parent_but_preserves_explicit_alternatives() {
    let mut store = Store::in_memory().unwrap();
    store.create_project("p", "Study").unwrap();
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))
        .unwrap();
    let root = store
        .propose("p", &figure.id, 1, "root", &json!({"elements":[]}))
        .unwrap();
    let store = store.with_linear_candidate_revisions(true);
    let child = revise(&store, &figure.id, &root).unwrap();
    assert!(
        revise(&store, &figure.id, &root)
            .unwrap_err()
            .to_string()
            .contains("pending child")
    );
    let grandchild = revise(&store, &figure.id, &child).unwrap();
    assert_eq!(
        grandchild.parent_proposal_id.as_deref(),
        Some(child.id.as_str())
    );
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
    let store = store.with_linear_candidate_revisions(false);
    let alternative = revise(&store, &figure.id, &root).unwrap();
    assert_eq!(
        alternative.parent_proposal_id.as_deref(),
        Some(root.id.as_str())
    );
    assert_ne!(alternative.id, child.id);
}

#[test]
fn concurrent_revisions_of_same_parent_have_one_winner() {
    let path = std::env::temp_dir().join(format!(
        "visualization-linear-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let mut store = Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let document = json!({"elements":[]});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &document)
        .unwrap();
    let parent = store
        .propose("p", &figure.id, 1, "root", &document)
        .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            let figure = figure.id.clone();
            let parent = parent.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let store = Store::open(path)
                    .unwrap()
                    .with_linear_candidate_revisions(true);
                barrier.wait();
                revise(&store, &figure, &parent)
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .find_map(|r| r.as_ref().err())
            .unwrap()
            .to_string()
            .contains("pending child")
    );
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 2);
    assert_eq!(
        store
            .proposal("p", &figure.id, &parent.id)
            .unwrap()
            .document,
        document
    );
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
    drop(store);
    std::fs::remove_file(path).unwrap();
}

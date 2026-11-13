use serde_json::json;
use visualization::{Store, planning::Brief};
#[test]
fn legacy_questions_block_but_explicit_optional_preferences_remain_unanswered_in_history_and_sketch()
 {
    let mut store = Store::in_memory().unwrap();
    store.create_project("p", "Study").unwrap();
    let original = json!({"elements":[]});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &original)
        .unwrap();
    let mut brief:Brief=serde_json::from_value(json!({"message":"Supplied conceptual cell","audience":"Researchers","nodes":[{"id":"cell","label":"Cell"}],"questions":[{"id":"species","question":"Which species?","answer":""},{"id":"language","question":"Preferred label language?","answer":"","blocking":false}]})).unwrap();
    assert!(brief.questions[0].blocking);
    assert!(!brief.questions[1].blocking);
    let first = store.save_planning("p", &figure.id, 1, 0, &brief).unwrap();
    assert_eq!(first.status, "needs_clarification");
    assert!(
        first
            .issues
            .iter()
            .any(|issue| issue.field == "question:species")
    );
    assert!(
        !first
            .issues
            .iter()
            .any(|issue| issue.field == "question:language")
    );
    assert!(
        store
            .planned_proposal("p", &figure.id, 1, first.version, "Must not guess")
            .is_err()
    );
    assert!(store.proposals("p", &figure.id).unwrap().is_empty());
    brief.questions[0].answer = "Human, user supplied".into();
    let second = store.save_planning("p", &figure.id, 1, 1, &brief).unwrap();
    assert_eq!(second.status, "ready");
    assert!(second.document.questions[1].answer.is_empty());
    let candidate = store
        .planned_proposal(
            "p",
            &figure.id,
            1,
            second.version,
            "Known concept, optional preference open",
        )
        .unwrap();
    let recorded =
        &candidate.document["elements"][0]["customData"]["planningSource"]["document"]["questions"];
    assert_eq!(recorded[1]["blocking"], false);
    assert_eq!(recorded[1]["answer"], "");
    assert_eq!(recorded[0]["answer"], "Human, user supplied");
    assert_eq!(
        store
            .planning_revision("p", &figure.id, first.version)
            .unwrap()
            .document
            .questions[0]
            .answer,
        ""
    );
    assert!(
        store
            .planning_revision("p", &figure.id, first.version)
            .unwrap()
            .document
            .questions[0]
            .blocking
    );
    assert_eq!(
        store.revision("p", &figure.id, 1).unwrap().document,
        original
    );
    brief.questions[1].blocking = true;
    let third = store
        .save_planning("p", &figure.id, 1, second.version, &brief)
        .unwrap();
    assert_eq!(third.status, "needs_clarification");
    assert!(
        store
            .planned_proposal(
                "p",
                &figure.id,
                1,
                third.version,
                "Blocked preference now required"
            )
            .is_err()
    );
}
#[test]
fn optional_is_explicit_and_does_not_allow_malformed_questions() {
    let invalid =
        json!({"questions":[{"id":"q","question":"Question","answer":"","blocking":"false"}]});
    assert!(serde_json::from_value::<Brief>(invalid).is_err());
    let malformed:Brief=serde_json::from_value(json!({"message":"Concept","audience":"Researchers","nodes":[{"id":"a","label":"A"}],"questions":[{"id":"q","question":"","answer":"","blocking":false}]})).unwrap();
    assert!(
        malformed
            .issues()
            .iter()
            .any(|issue| issue.field == "question:q")
    );
}

#[test]
fn clearing_a_required_answer_blocks_again_without_changing_requiredness() {
    let mut store = Store::in_memory().unwrap();
    store.create_project("p", "Study").unwrap();
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))
        .unwrap();
    let mut brief: Brief = serde_json::from_value(json!({"message":"Supplied concept","audience":"Researchers","nodes":[{"id":"cell","label":"Cell"}],"questions":[{"id":"species","question":"Which species?","answer":"Human","blocking":true}]})).unwrap();
    let answered = store.save_planning("p", &figure.id, 1, 0, &brief).unwrap();
    assert_eq!(answered.status, "ready");
    assert!(answered.document.questions[0].blocking);
    brief.questions[0].answer.clear();
    let cleared = store
        .save_planning("p", &figure.id, 1, answered.version, &brief)
        .unwrap();
    assert_eq!(cleared.status, "needs_clarification");
    assert!(cleared.document.questions[0].blocking);
    assert!(
        store
            .planned_proposal(
                "p",
                &figure.id,
                1,
                cleared.version,
                "Unanswered required scope"
            )
            .is_err()
    );
    assert_eq!(
        store
            .planning_revision("p", &figure.id, answered.version)
            .unwrap()
            .document
            .questions[0]
            .answer,
        "Human"
    );
}

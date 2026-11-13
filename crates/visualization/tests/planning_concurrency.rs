use std::sync::{Arc, Barrier};
use visualization::{
    Error, Store,
    planning::{Brief, Question},
};
#[test]
fn independent_planners_cannot_overwrite_an_answer_from_a_newer_version()
-> visualization::Result<()> {
    let path = std::env::temp_dir().join(format!(
        "visualization-planning-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let mut observer = Store::open(&path)?;
    observer.create_project("p", "Study")?;
    let figure = observer.create(
        "p",
        "Figure",
        "excalidraw/v2",
        &serde_json::json!({"elements":[]}),
    )?;
    let brief = Brief {
        questions: vec![Question {
            id: "q1".into(),
            question: "Which cell?".into(),
            answer: String::new(),
            blocking: true,
        }],
        ..Brief::default()
    };
    observer.save_planning("p", &figure.id, 1, 0, &brief)?;
    let barrier = Arc::new(Barrier::new(2));
    let run = |answer: &str| -> visualization::Result<_> {
        let mut store = Store::open(&path)?;
        let id = figure.id.clone();
        let barrier = barrier.clone();
        let mut document = brief.clone();
        document.questions[0].answer = answer.into();
        Ok(std::thread::spawn(move || {
            barrier.wait();
            store.save_planning("p", &id, 1, 1, &document)
        }))
    };
    let left = run("Cell A")?;
    let right = run("Cell B")?;
    let winner = match (left.join().unwrap(), right.join().unwrap()) {
        (Ok(plan), Err(Error::Conflict)) | (Err(Error::Conflict), Ok(plan)) => plan,
        other => panic!("expected one committed answer and one conflict, got {other:?}"),
    };
    assert_eq!(winner.version, 2);
    assert_eq!(
        observer
            .planning("p", &figure.id)?
            .unwrap()
            .document
            .questions[0]
            .answer,
        winner.document.questions[0].answer
    );
    assert!(
        observer
            .planning_revision("p", &figure.id, 1)?
            .document
            .questions[0]
            .answer
            .is_empty()
    );
    assert_eq!(
        observer
            .planning_revision("p", &figure.id, 2)?
            .document
            .questions[0]
            .answer,
        winner.document.questions[0].answer
    );
    assert_eq!(observer.get("p", &figure.id)?.revision, 1);
    drop(observer);
    std::fs::remove_file(path).unwrap();
    Ok(())
}

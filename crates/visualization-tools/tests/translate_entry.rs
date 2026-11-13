use serde_json::json;
#[tokio::test]
async fn flat_translation_preserves_ids_sources_and_candidate_lineage() {
    let directory=tempfile::tempdir().unwrap();let path=directory.path().join("figures.sqlite");
    let mut store=visualization::Store::open(&path).unwrap();store.create_project("p","Study").unwrap();
    let original=json!({"elements":[{"id":"note","type":"text","x":0,"y":100,"width":200,"height":20,"angle":0,"text":"Source note","originalText":"Source note","customData":{"evidence":["source"]}}],"files":{}});
    let figure=store.create("p","Figure","excalidraw/v2",&original).unwrap();let parent=store.propose("p",&figure.id,1,"Parent",&original).unwrap();
    let tool=visualization_tools::registrations(path).into_iter().find(|t|t.definition.name=="visualization_translate").unwrap();
    let input=json!({"project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"title":"Shift note","element_ids":["note"],"dx":0,"dy":85,"markup_ids":[]});
    let mut invalid=input.clone();invalid["dx"]=json!("0");assert!(tool.implementation.execute(invalid).await.is_err());assert_eq!(store.proposals("p",&figure.id).unwrap().len(),1);
    let result=serde_json::to_value(tool.implementation.execute(input.clone()).await.unwrap()).unwrap();let payload=&result["content"];
    let child=store.proposal("p",&figure.id,payload["id"].as_str().unwrap()).unwrap();
    assert_eq!(child.parent_proposal_id.as_deref(),Some(parent.id.as_str()));
    assert_eq!(child.document["elements"][0]["id"],"note");assert_eq!(child.document["elements"][0]["y"].as_f64(),Some(185.));assert_eq!(child.document["elements"][0]["customData"],original["elements"][0]["customData"]);
    assert_eq!(payload["follow_up_inputs"]["read_proposal"]["proposal_id"],child.id);
    assert_eq!(store.proposal("p",&figure.id,&parent.id).unwrap().document,parent.document);assert_eq!(store.get("p",&figure.id).unwrap().revision,1);
    let mut stale=input;stale["document_sha256"]=json!("wrong");assert!(tool.implementation.execute(stale).await.is_err());assert_eq!(store.proposals("p",&figure.id).unwrap().len(),2);
}

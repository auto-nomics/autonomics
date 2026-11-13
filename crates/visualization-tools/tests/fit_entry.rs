use serde_json::json;

#[tokio::test]
async fn scalar_fit_requires_explicit_scope_and_real_numbers() {
    let directory=tempfile::tempdir().unwrap();
    let path=directory.path().join("unused.sqlite");
    let tool=visualization_tools::registrations(path.clone()).into_iter().find(|tool|tool.definition.name=="visualization_fit").unwrap();
    let schema=serde_json::to_value(&tool.definition.input_schema).unwrap();
    for name in ["x","y","width","height"] {
        assert_eq!(schema["properties"][name]["type"],"number");
        assert!(schema["required"].as_array().unwrap().contains(&json!(name)));
    }
    let mut input=json!({"project_id":"p","figure_id":"f","proposal_id":"candidate","document_sha256":"hash","expected_revision":1,"title":"Fit","element_id":"node","file_id":"file","x":0.1,"y":0.2,"width":0.3,"height":0.4});
    input["x"]=json!("0.1");
    assert!(tool.implementation.execute(input.clone()).await.is_err());
    assert!(!path.exists());
    input["x"]=json!(0.1);input.as_object_mut().unwrap().remove("proposal_id");
    assert!(tool.implementation.execute(input).await.is_err());
    assert!(!path.exists());
}

#[tokio::test]
async fn scalar_fit_continues_translated_candidate_without_losing_notes_or_sources() {
    let directory=tempfile::tempdir().unwrap();let path=directory.path().join("figures.sqlite");
    let original:serde_json::Value=serde_json::from_str(include_str!("../../../apps/gui/tests/fixtures/scientific-ifng-art-before-clearance.json")).unwrap();
    let file=original["elements"].as_array().unwrap().iter().find(|e|e["type"]=="image").unwrap()["fileId"].clone();
    let mut store=visualization::Store::open(&path).unwrap();store.create_project("p","Review").unwrap();let figure=store.create("p","Figure","excalidraw/v2",&original).unwrap();
    let moved=visualization::scene::apply(&original,&[visualization::scene::Operation::Translate{element_ids:vec!["note_scope_heading".into(),"note_scope_body".into(),"generic-receptor-note".into()],dx:0.,dy:85.}]).unwrap();
    let parent=store.propose("p",&figure.id,1,"Notes already moved",&moved).unwrap();
    let tool=visualization_tools::registrations(path.clone()).into_iter().find(|t|t.definition.name=="visualization_fit").unwrap();
    let input=json!({"project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"title":"Fit after notes","element_id":"node-0","file_id":file,"x":0.347441,"y":0.016803,"width":0.268522,"height":0.960235,"markup_ids":[]});
    let result=serde_json::to_value(tool.implementation.execute(input.clone()).await.unwrap()).unwrap();
    let child=store.proposal("p",&figure.id,result["content"]["id"].as_str().unwrap()).unwrap();
    assert_eq!(child.parent_proposal_id.as_deref(),Some(parent.id.as_str()));
    for id in ["note_scope_heading","note_scope_body","generic-receptor-note"] {
        let find=|doc:&serde_json::Value|doc["elements"].as_array().unwrap().iter().find(|e|e["id"]==id).unwrap().clone();
        assert_eq!(find(&child.document),find(&moved));
    }
    assert_eq!(child.document["files"],moved["files"]);
    for old in moved["elements"].as_array().unwrap(){let actual=child.document["elements"].as_array().unwrap().iter().find(|e|e["id"]==old["id"]).unwrap();for (key,value) in old["customData"].as_object().into_iter().flatten(){if !["visualAnchorFit","visualAssetSlot"].contains(&key.as_str()){assert_eq!(&actual["customData"][key],value,"metadata {key}");}}}
    assert_eq!(store.proposal("p",&figure.id,&parent.id).unwrap().document,parent.document);
    assert_eq!(store.get("p",&figure.id).unwrap().revision,1);
    let count=store.proposals("p",&figure.id).unwrap().len();let mut stale=input;stale["document_sha256"]=json!("wrong");assert!(tool.implementation.execute(stale).await.is_err());assert_eq!(store.proposals("p",&figure.id).unwrap().len(),count);
}

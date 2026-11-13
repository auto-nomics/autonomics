use serde_json::{Value, json};
use visualization::scene::{self, Operation};
fn source() -> Value {
    json!({"elements":[
      {"id":"a","type":"frame","x":10.,"y":20.,"width":200.,"height":180.,"angle":0.},
      {"id":"ta","type":"text","x":30.,"y":40.,"width":80.,"height":20.,"angle":0.,"frameId":"a","text":"Keep scientific wording"},
      {"id":"b","type":"frame","x":600.,"y":400.,"width":300.,"height":200.,"angle":0.},
      {"id":"tb","type":"text","x":620.,"y":420.,"width":80.,"height":20.,"angle":0.,"frameId":"b","text":"Keep another label"},
      {"id":"outside","type":"rectangle","x":1500.,"y":1000.,"width":20.,"height":20.,"angle":0.}
    ],"files":{},"scientificPlan":{"history":"unchanged"}})
}
fn pack(v: &Value, vertical: bool, gap: f64) -> Result<Value, scene::SceneError> {
    scene::apply(v, &[Operation::PackFrames {element_ids:vec!["b".into(),"a".into()],vertical,gap}])
}
#[test]
fn packs_by_existing_order_preserving_orthogonal_position_and_content() {
    for vertical in [false,true] {
        let old=source();let next=pack(&old,vertical,40.).unwrap();
        assert_eq!(next["elements"][0],old["elements"][0]);
        assert_eq!(next["elements"][1],old["elements"][1]);
        assert_eq!(next["elements"][4],old["elements"][4]);
        let axis=if vertical {"y"} else {"x"};let other=if vertical {"x"} else {"y"};
        let expected=if vertical {240.} else {250.};
        assert_eq!(next["elements"][2][axis],json!(expected));
        assert_eq!(next["elements"][3][axis],json!(expected+20.));
        for i in [2,3] {assert_eq!(next["elements"][i][other],old["elements"][i][other]);}
        assert_eq!(next["elements"][3]["text"],old["elements"][3]["text"]);
        assert_eq!(next["files"],old["files"]);assert_eq!(next["scientificPlan"],old["scientificPlan"]);
        let again=pack(&next,vertical,40.).unwrap();assert_eq!(again,next);
    }
}
#[test]
fn invalid_gap_or_unsafe_frames_reject_without_mutation() {
    let old=source();for gap in [-1.,0.,7.,513.,f64::NAN] {assert!(pack(&old,false,gap).is_err());}
    for key in ["locked","angle","frameId","type"] {
        let mut bad=old.clone();bad["elements"][2][key]=match key {"locked"=>json!(true),"angle"=>json!(0.5),"frameId"=>json!("a"),_=>json!("rectangle")};
        let snapshot=bad.clone();assert!(pack(&bad,false,40.).is_err());assert_eq!(bad,snapshot);
    }
}
#[test]
#[ignore = "requires an explicit saved real-model scene"]
fn saved_whole_figure_pack_preserves_files_and_object_inventory() {
    let path=std::env::var("VISUALIZATION_PACK_SOURCE").unwrap();
    let input:Value=serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let doc=input.get("document").unwrap_or(&input);
    let ids=doc["elements"].as_array().unwrap().iter().filter(|e| e["type"]=="frame").map(|e|e["id"].as_str().unwrap().to_owned()).collect();
    let next=scene::apply(doc,&[Operation::PackFrames{element_ids:ids,vertical:false,gap:80.}]).unwrap();
    assert_eq!(next["files"],doc["files"]);
    assert_eq!(next["elements"].as_array().unwrap().len(),doc["elements"].as_array().unwrap().len());
    for old in doc["elements"].as_array().unwrap() {
        let new=next["elements"].as_array().unwrap().iter().find(|e|e["id"]==old["id"]).unwrap();
        for field in ["text","originalText","fontSize","fontFamily","fileId","frameId"] {assert_eq!(new[field],old[field],"{} / {field}",old["id"]);}
    }
}

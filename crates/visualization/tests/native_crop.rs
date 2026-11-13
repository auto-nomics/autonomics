use serde_json::{Value,json};
use visualization::scene::{self,Operation};
fn source()->Value {json!({"elements":[{"id":"image","type":"image","x":10,"y":20,"width":100,"height":80,"angle":0,"scale":[1,1],"crop":null,"fileId":"f"}],"files":{"f":{"dataURL":"preserved"}}})}
fn update(value:Value)->Operation {Operation::Update{id:"image".into(),changes:value.as_object().unwrap().clone()}}
#[test]
fn malformed_native_crops_reject_atomically_before_poisoning_preview_geometry(){
 let original=source();
 for crop in [json!({"height":"1","width":"0.19","x":"0.51","y":"0"}),json!({"x":0,"y":0,"width":10,"height":10}),json!({"x":0,"y":0,"width":101,"height":80,"naturalWidth":100,"naturalHeight":80}),json!({"x":0,"y":0,"width":0,"height":80,"naturalWidth":100,"naturalHeight":80}),json!(""),json!([])] {
  let result=scene::apply(&original,&[update(json!({"strokeColor":"#112233"})),update(json!({"crop":crop}))]);assert!(result.is_err());assert_eq!(original,source());
 }
}
#[test]
fn complete_pixel_crop_and_numeric_flips_are_preserved_and_can_be_cleared(){
 let crop=json!({"x":20,"y":10,"width":70,"height":60,"naturalWidth":100,"naturalHeight":80});
 let changed=scene::apply(&source(),&[update(json!({"crop":crop,"scale":[-1,1]}))]).unwrap();assert_eq!(changed["elements"][0]["crop"],crop);assert_eq!(changed["files"],source()["files"]);
 let cleared=scene::apply(&changed,&[update(json!({"crop":null,"scale":[1,1]}))]).unwrap();assert!(cleared["elements"][0]["crop"].is_null());
 for scale in [json!(["1","-1"]),json!([2,1]),json!([1]),Value::Null] {assert!(scene::apply(&source(),&[update(json!({"scale":scale}))]).is_err());}
}
#[test]
fn new_objects_cannot_bypass_native_numeric_checks(){
 for field in ["x","y","width","height","angle","strokeWidth","opacity"] {
  let mut element=json!({"id":"new","type":"frame","x":0,"y":0,"width":100,"height":100});element[field]=json!("100");
  assert!(scene::apply(&source(),&[Operation::Add{element}]).is_err());
 }
 let mut legacy=source();legacy["elements"][0]["crop"]=json!({"x":"invalid legacy"});assert!(scene::apply(&legacy,&[]).is_ok());assert!(scene::apply(&legacy,&[update(json!({"crop":null}))]).is_ok());
}

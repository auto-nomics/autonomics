//! Bounded ownership/fit snapshot diagnostics, never a scientific or paint review.
use serde_json::{json, Value};
use std::collections::HashSet;
fn id(v: &Value) -> bool {
    v.as_str()
        .is_some_and(|s| !s.is_empty() && s.encode_utf16().count() <= 512)
}
fn numbers(v: &Value, len: usize) -> Option<Vec<f64>> {
    let a = v.as_array()?;
    (a.len() == len).then_some(())?;
    a.iter().map(Value::as_f64).collect()
}
fn snapshot(fit: &Value, image: &Value) -> &'static str {
    let Some(bounds) = numbers(&fit["imageBounds"], 4) else {
        return "invalid_record";
    };
    let Some(n) = numbers(&fit["normalizedBounds"], 4) else {
        return "invalid_record";
    };
    let t = &fit["imageTransform"];
    let Some(scale) = numbers(&t["scale"], 2) else {
        return "invalid_record";
    };
    if !id(&fit["fileId"])
        || bounds[2] <= 0.
        || bounds[3] <= 0.
        || n[0] < 0.
        || n[1] < 0.
        || n[2] <= 0.
        || n[3] <= 0.
        || n[0] + n[2] > 1.000001
        || n[1] + n[3] > 1.000001
        || t["angle"].as_f64().is_none()
        || scale.iter().any(|v| *v != 1. && *v != -1.)
        || t.get("crop").is_none()
    {
        return "invalid_record";
    }
    if fit["fileId"] != image["fileId"] {
        return "stale_file";
    }
    let current: Option<Vec<_>> = ["x", "y", "width", "height"]
        .iter()
        .map(|k| image[k].as_f64())
        .collect();
    let same_crop = if t["crop"].is_null() {
        image["crop"].is_null()
    } else {
        ["x", "y", "width", "height", "naturalWidth", "naturalHeight"]
            .iter()
            .all(|k| t["crop"][k].as_f64().is_some() && t["crop"][k] == image["crop"][k])
    };
    if current.is_none_or(|a| a.iter().zip(bounds).any(|(a, b)| (a - b).abs() > 1e-6))
        || t["angle"].as_f64()
            != if image["angle"].is_null() {
                Some(0.)
            } else {
                image["angle"].as_f64()
            }
        || numbers(&image["scale"], 2) != Some(scale)
        || !same_crop
    {
        return "stale_geometry";
    }
    "current_snapshot"
}
fn fit_status(elements: &[&Value], anchor: &Value, image: &Value) -> &'static str {
    let data = &anchor["customData"];
    let fit = &data["visualAnchorFit"];
    if fit.is_null() {
        return "not_recorded";
    }
    let status = snapshot(fit, image);
    if status != "current_snapshot" || fit["scope"] != "illustration_group" {
        return status;
    }
    let empty = Vec::new();
    let badges = if data["visualBadgeIds"].is_null() {
        &empty
    } else {
        let Some(b) = data["visualBadgeIds"].as_array() else {
            return "invalid_record";
        };
        b
    };
    let Some(members) = fit["members"].as_array() else {
        return "invalid_record";
    };
    let mut expected = HashSet::new();
    expected.insert(image["id"].as_str().unwrap_or_default());
    for badge in badges {
        if !id(badge) || !expected.insert(badge.as_str().unwrap()) {
            return "invalid_record";
        }
    }
    if members.len() != badges.len() + 1
        || members.len() > 33
        || elements
            .iter()
            .filter(|e| {
                e["type"] == "image"
                    && (e["customData"]["semanticAnchorId"] == anchor["id"]
                        || e["customData"]["semanticBadgeAnchorId"] == anchor["id"])
            })
            .count()
            != members.len()
    {
        return "invalid_record";
    }
    let mut seen = HashSet::new();
    for member in members {
        let Some(mid) = member["elementId"]
            .as_str()
            .filter(|_| id(&member["elementId"]))
        else {
            return "invalid_record";
        };
        if !expected.contains(mid) || !seen.insert(mid) {
            return "invalid_record";
        }
        let matches: Vec<_> = elements.iter().filter(|e| e["id"] == mid).collect();
        if matches.len() != 1 {
            return "invalid_record";
        }
        let m = matches[0];
        let md = &m["customData"];
        if m["type"] != "image"
            || m["frameId"] != anchor["frameId"]
            || !m["groupIds"]
                .as_array()
                .is_some_and(|g| g.contains(&data["visualAssetGroupId"]))
            || if m["id"] == image["id"] {
                md["semanticAnchorId"] != anchor["id"]
            } else {
                md["semanticBadgeAnchorId"] != anchor["id"]
                    || md["visualBadgeGroupId"] != data["visualAssetGroupId"]
            }
        {
            return "invalid_record";
        }
        let status = snapshot(member, m);
        if status != "current_snapshot" {
            return status;
        }
    }
    let Some(b) = numbers(&fit["anchorBounds"], 4) else {
        return "invalid_record";
    };
    if b[2] <= 0. || b[3] <= 0. {
        return "invalid_record";
    }
    if ["x", "y", "width", "height"]
        .iter()
        .zip(b)
        .any(|(k, b)| anchor[k].as_f64().is_none_or(|v| (v - b).abs() > 1e-6))
    {
        return "stale_geometry";
    }
    "current_snapshot"
}
/// Matches GUI checkpoint semantics; bounded/truncated results cannot establish completion.
pub fn review(document: &Value) -> Value {
    let empty = Vec::new();
    let all = document["elements"].as_array().unwrap_or(&empty);
    let elements: Vec<_> = all
        .iter()
        .take(5000)
        .filter(|e| e["isDeleted"] != true)
        .collect();
    let anchors: Vec<_> = elements
        .iter()
        .filter(|e| id(&e["id"]) && id(&e["customData"]["scientificId"]))
        .collect();
    let mut components = Vec::new();
    for anchor in anchors.iter().take(200) {
        let data = &anchor["customData"];
        let images: Vec<_> = elements
            .iter()
            .filter(|e| e["type"] == "image" && e["customData"]["semanticAnchorId"] == anchor["id"])
            .collect();
        let matches: Vec<_> = elements
            .iter()
            .filter(|e| e["id"] == data["visualAssetElementId"])
            .collect();
        let image = (matches.len() == 1 && matches[0]["type"] == "image").then(|| matches[0]);
        let ambiguous = anchors.iter().filter(|e| e["id"] == anchor["id"]).count() != 1
            || images.len() > 1
            || images.len() == 1 && !id(&data["visualAssetElementId"])
            || !data["visualAssetElementId"].is_null()
                && (image.is_none() || images.first().map(|e| &e["id"]) != image.map(|e| &e["id"]));
        let owned = data["visualLabelIds"].as_array().unwrap_or(&empty);
        let native = elements.iter().any(|e| {
            e["type"] == "text"
                && e["frameId"] == anchor["frameId"]
                && e["containerId"] == anchor["id"]
                && anchor["boundElements"]
                    .as_array()
                    .is_some_and(|b| b.iter().any(|b| b["id"] == e["id"] && b["type"] == "text"))
        });
        let detached = !owned.is_empty()
            && owned.iter().collect::<HashSet<_>>().len() == owned.len()
            && owned.iter().all(|lid| {
                elements.iter().any(|e| {
                    e["id"] == *lid
                        && e["type"] == "text"
                        && e["frameId"] == anchor["frameId"]
                        && e["containerId"].is_null()
                        && data["visualAssetGroupId"].is_string()
                        && e["groupIds"]
                            .as_array()
                            .is_some_and(|g| g.contains(&data["visualAssetGroupId"]))
                })
            });
        let reason = if ambiguous {
            "ambiguous"
        } else if images.is_empty() {
            "missing"
        } else {
            image.map_or("invalid_record", |i| fit_status(&elements, anchor, i))
        };
        let status = match reason {
            "ambiguous" | "missing" => reason,
            "current_snapshot" => "snapshot_current",
            _ => "needs_fit",
        };
        components.push(json!({"anchor_id":anchor["id"],"scientific_id":data["scientificId"],"image_id":image.map(|i| &i["id"]),"status":status,"fit_reason":reason,"has_verified_label_ownership":native||detached}));
    }
    let unassigned: Vec<_> = elements
        .iter()
        .filter(|e| e["type"] == "image")
        .filter(|image| {
            let d = &image["customData"];
            let p: Vec<_> = elements
                .iter()
                .filter(|e| e["id"] == d["semanticAnchorId"])
                .collect();
            let b: Vec<_> = elements
                .iter()
                .filter(|e| e["id"] == d["semanticBadgeAnchorId"])
                .collect();
            !(p.len() == 1 && p[0]["customData"]["visualAssetElementId"] == image["id"]
                || b.len() == 1
                    && b[0]["customData"]["visualBadgeIds"]
                        .as_array()
                        .is_some_and(|ids| ids.contains(&image["id"])))
        })
        .collect();
    let labels: Vec<_> = components
        .iter()
        .filter(|c| !c["image_id"].is_null() && c["has_verified_label_ownership"] != true)
        .map(|c| &c["anchor_id"])
        .collect();
    let count = |s: &str| components.iter().filter(|c| c["status"] == s).count();
    json!({"scope":"scientific_anchor_ownership_and_fit_snapshot_only","scientific_validity_verified":false,
        "instruction":"Resolve missing, ambiguous, needs_fit, label ownership and unassigned-image issues for the requested components before reporting materialization complete. Snapshot freshness does not verify paint/text overlap, layout quality, scientific identity or molecular structure; inspect PNG and geometry separately. Partial work must be reported as partial. Never adopt automatically.",
        "total":anchors.len(),"represented":components.len(),"truncated":anchors.len()>components.len()||all.len()>5000,
        "counts":{"missing":count("missing"),"ambiguous":count("ambiguous"),"needs_fit":count("needs_fit"),"snapshot_current":count("snapshot_current")},
        "label_ownership_issues":{"total":labels.len(),"items":labels.iter().take(40).collect::<Vec<_>>(),"truncated":labels.len()>40||anchors.len()>200},
        "unassigned_images":{"total":unassigned.len(),"items":unassigned.iter().take(40).map(|e| &e["id"]).collect::<Vec<_>>(),"truncated":unassigned.len()>40||all.len()>5000},"components":components})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scene() -> Value {
        json!({"elements":[
            {"id":"a","type":"rectangle","x":10,"y":20,"width":30,"height":40,"boundElements":[{"id":"label","type":"text"}],"customData":{"scientificId":"protein","visualAssetElementId":"im","visualAssetGroupId":"g","visualAnchorFit":{"fileId":"f","imageBounds":[10,20,30,40],"normalizedBounds":[0,0,1,1],"imageTransform":{"angle":0,"scale":[1,1],"crop":null}}}},
            {"id":"im","type":"image","fileId":"f","x":10,"y":20,"width":30,"height":40,"angle":0,"scale":[1,1],"groupIds":["g"],"customData":{"semanticAnchorId":"a"}},
            {"id":"label","type":"text","containerId":"a","text":"Protein"}
        ]})
    }
    #[test]
    fn detects_stale_or_invalid_primary_snapshots_without_mutation() {
        let original = scene();
        let p = review(&original);
        assert_eq!(p["counts"]["snapshot_current"], 1);
        assert_eq!(p["label_ownership_issues"]["total"], 0);
        assert_eq!(p["unassigned_images"]["total"], 0);
        assert_eq!(p["scientific_validity_verified"], false);
        for (field, value, reason) in [
            ("x", json!(11), "stale_geometry"),
            ("fileId", json!("other"), "stale_file"),
            ("scale", json!([-1, 1]), "stale_geometry"),
            (
                "crop",
                json!({"x":0,"y":0,"width":20,"height":30,"naturalWidth":30,"naturalHeight":40}),
                "stale_geometry",
            ),
        ] {
            let mut doc = original.clone();
            doc["elements"][1][field] = value;
            let p = review(&doc);
            assert_eq!(p["counts"]["needs_fit"], 1);
            assert_eq!(p["components"][0]["fit_reason"], reason);
        }
        let mut doc = original.clone();
        doc["elements"][0]["customData"]["visualAnchorFit"]["normalizedBounds"] =
            json!([0, 0, "1", 1]);
        assert_eq!(
            review(&doc)["components"][0]["fit_reason"],
            "invalid_record"
        );
        assert_eq!(original, scene());
    }
    #[test]
    fn diagnoses_full_group_members_and_label_ownership() {
        let mut d = scene();
        let primary = d["elements"][0]["customData"]["visualAnchorFit"].clone();
        let mut accessory = d["elements"][1].clone();
        accessory["id"] = json!("badge");
        accessory["x"] = json!(50);
        accessory["customData"] = json!({"semanticBadgeAnchorId":"a","visualBadgeGroupId":"g"});
        d["elements"].as_array_mut().unwrap().push(accessory);
        let mut p = primary.clone();
        p["elementId"] = json!("im");
        let mut b = primary;
        b["elementId"] = json!("badge");
        b["imageBounds"] = json!([50, 20, 30, 40]);
        d["elements"][0]["customData"]["visualBadgeIds"] = json!(["badge"]);
        let fit = &mut d["elements"][0]["customData"]["visualAnchorFit"];
        fit["scope"] = json!("illustration_group");
        fit["anchorBounds"] = json!([10, 20, 30, 40]);
        fit["members"] = json!([p, b]);
        assert_eq!(review(&d)["counts"]["snapshot_current"], 1);
        let mut changed = d.clone();
        changed["elements"][3]["x"] = json!(51);
        assert_eq!(
            review(&changed)["components"][0]["fit_reason"],
            "stale_geometry"
        );
        let mut changed = d.clone();
        changed["elements"][0]["x"] = json!(11);
        assert_eq!(
            review(&changed)["components"][0]["fit_reason"],
            "stale_geometry"
        );
        let mut changed = d.clone();
        changed["elements"][0]["customData"]["visualAnchorFit"]["members"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert_eq!(
            review(&changed)["components"][0]["fit_reason"],
            "invalid_record"
        );
        d["elements"][0]["boundElements"] = json!([]);
        assert_eq!(review(&d)["label_ownership_issues"]["total"], 1);
        d["elements"][1]["customData"]["semanticAnchorId"] = json!("missing");
        assert_eq!(review(&d)["unassigned_images"]["total"], 1);
        assert_eq!(review(&d)["counts"]["ambiguous"], 1);
    }
    #[test]
    fn actual_incomplete_materialization_is_not_reported_as_finished() {
        let doc: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/materialization-stale-v295.json"
        ))
        .unwrap();
        let r = review(&doc);
        assert_eq!(r["total"], 7);
        assert_eq!(r["counts"]["snapshot_current"], 5);
        assert_eq!(r["counts"]["needs_fit"], 2);
        assert_eq!(r["label_ownership_issues"]["total"], 0);
        assert_eq!(r["unassigned_images"]["total"], 0);
        let pending: Vec<_> = r["components"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["status"] == "needs_fit")
            .map(|c| c["anchor_id"].as_str().unwrap())
            .collect();
        assert_eq!(pending, vec!["node-5", "node-6"]);
    }
    #[test]
    fn inventory_is_bounded_and_never_reports_truncation_as_complete() {
        let anchor = scene()["elements"][0].clone();
        let d = json!({"elements":vec![anchor;201]});
        let r = review(&d);
        assert_eq!(r["represented"], 200);
        assert_eq!(r["total"], 201);
        assert_eq!(r["truncated"], true);
        assert_eq!(r["counts"]["ambiguous"], 200);
    }
}

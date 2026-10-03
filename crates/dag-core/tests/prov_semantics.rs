//! Regression tests pinning the PROV modeling semantics fixed after the
//! first real-world export review: no duplicate relations, source nodes
//! ingest (not generate) external files, a run-level activity carries real
//! timestamps, DataFrame entities join the derivation chain, specs / wiring
//! ports / dispatch order ride their activities and relations, and all
//! sections (used/agent/prefix/…) are present.

use dag_core::dag::RunRecord;
use dag_core::dag::export::build_prov_document;
use serde_json::{Value, json};

fn report() -> Value {
    json!({
        "nodes": [
            {"id":"counts_file","status":"success","node_type":"file_reference",
             "elapsed_ms":3,"dispatch_seq":0,"fingerprint":"fp-src",
             "output_files":[{"path":"/bixbench/counts.txt","format":"txt",
               "fingerprint":{"size":10,"mtime_ns":1,"content_hash":null,"immutable_remote":false}}],
             "port_assignments":{"0":{"path":"/bixbench/counts.txt","format":"txt",
               "fingerprint":{"size":10,"mtime_ns":1,"content_hash":null,"immutable_remote":false}}},
             "inputs":[]},
            {"id":"normalize","status":"success","node_type":"sql",
             "elapsed_ms":50,"dispatch_seq":1,"fingerprint":"fp-sql","output_type":"DataFrame",
             "output_schema":{"column_count":2,"columns":{"a":"Int64"},"type_distribution":null},
             "output_rows":42,
             "inputs":[{"from":"counts_file","from_port":0,"to_port":0,"kind":"File",
               "path":"/bixbench/counts.txt","fingerprint":{"size":10,"mtime_ns":1,"content_hash":null}}]},
            {"id":"deseq2_condition","status":"success","node_type":"deseq2_condition",
             "elapsed_ms":9000,"dispatch_seq":2,"fingerprint":"fp-tool",
             "output_files":[{"path":"vfs:///artifacts/r1/out.csv","format":"csv",
               "fingerprint":{"size":5,"mtime_ns":0,"content_hash":"sha256:abc","immutable_remote":true}}],
             "port_assignments":{"0":{"path":"vfs:///artifacts/r1/out.csv","format":"csv",
               "fingerprint":{"size":5,"mtime_ns":0,"content_hash":"sha256:abc","immutable_remote":true}}},
             "inputs":[{"from":"normalize","from_port":0,"to_port":0,"kind":"DataFrame"}],
             "execution":{"image":"ghcr.io/x@sha256:ee","image_digest":"sha256:ee",
               "exit_code":0,"run_name":"r1"}}
        ]
    })
}

fn manifest() -> String {
    json!({
        "nodes": [
            {"id":"counts_file","kind":"file_reference","spec":{"path":"/bixbench/counts.txt"}},
            {"id":"normalize","kind":"sql","spec":{"sql_query":"SELECT a FROM counts"}},
            {"id":"deseq2_condition","kind":"deseq2_condition","spec":{"design":"~condition"}}
        ],
        "edges": [
            {"from":"counts_file","from_port":0,"to":"normalize","to_port":0},
            {"from":"normalize","from_port":0,"to":"deseq2_condition","to_port":0}
        ]
    })
    .to_string()
}

fn doc() -> Value {
    let run = RunRecord {
        id: "run-1".into(),
        ref_name: "asxl1-deseq2".into(),
        snapshot_id: Some("snap1".into()),
        manifest_hash: "mh".into(),
        trigger: Some("agent:/root/bixbench".into()),
        started_at: "2026-09-29T10:00:00Z".into(),
        finished_at: "2026-09-29T10:05:00Z".into(),
        ok: true,
        cancelled: false,
        error: None,
        message: Some("m".into()),
        engine_version: "0.1.0".into(),
        source_revision: "rev1".into(),
        run_report_json: None,
    };
    build_prov_document(&run, &report(), Some(&manifest()))
}

fn pair_count(array: &[Value], key_a: &str, key_b: &str) -> (usize, usize) {
    let mut seen = std::collections::BTreeSet::new();
    let mut dups = 0;
    for entry in array {
        let key = (
            entry[key_a].as_str().unwrap_or("").to_string(),
            entry[key_b].as_str().unwrap_or("").to_string(),
        );
        if !seen.insert(key) {
            dups += 1;
        }
    }
    (seen.len(), dups)
}

#[test]
fn all_sections_present_and_nonempty() {
    let doc = doc();
    for section in [
        "prefix",
        "entity",
        "activity",
        "agent",
        "used",
        "wasGeneratedBy",
        "wasInformedBy",
        "wasDerivedFrom",
        "wasAssociatedWith",
    ] {
        assert!(
            doc.get(section).is_some_and(|v| !v.is_null()),
            "section {section} missing"
        );
    }
    assert!(
        !doc["agent"].as_object().unwrap().is_empty(),
        "agent declarations"
    );
    assert!(
        !doc["used"].as_array().unwrap().is_empty(),
        "used relations"
    );
}

#[test]
fn no_duplicate_relations() {
    let doc = doc();
    let (_, gen_dups) = pair_count(
        doc["wasGeneratedBy"].as_array().unwrap(),
        "entity",
        "activity",
    );
    let (_, used_dups) = pair_count(doc["used"].as_array().unwrap(), "activity", "entity");
    let (_, der_dups) = pair_count(
        doc["wasDerivedFrom"].as_array().unwrap(),
        "generatedEntity",
        "usedEntity",
    );
    let (_, inf_dups) = pair_count(
        doc["wasInformedBy"].as_array().unwrap(),
        "activity",
        "informed",
    );
    assert_eq!((gen_dups, used_dups, der_dups, inf_dups), (0, 0, 0, 0));
}

#[test]
fn source_nodes_ingest_external_files_instead_of_generating_them() {
    let doc = doc();
    let src = "urn:autonomics:run:run-1:node:counts_file";
    let original = "urn:autonomics:path:/bixbench/counts.txt";
    // The original data file is used, never generated.
    assert!(
        doc["used"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["activity"] == src && r["entity"] == original)
    );
    assert!(
        !doc["wasGeneratedBy"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["activity"] == src),
        "a source node must not generate files"
    );
    assert_eq!(
        doc["entity"][original]["autonomics:kind"],
        json!("external-input"),
        "original file entity is tagged as external input"
    );
}

#[test]
fn run_level_activity_carries_wall_clock() {
    let doc = doc();
    let run_activity = &doc["activity"]["urn:autonomics:run:run-1"];
    assert_eq!(
        run_activity["prov:startTime"],
        json!("2026-09-29T10:00:00Z")
    );
    assert_eq!(run_activity["prov:endTime"], json!("2026-09-29T10:05:00Z"));
    assert_eq!(run_activity["autonomics:ref"], json!("asxl1-deseq2"));
    // Node activities reference their run.
    assert_eq!(
        doc["activity"]["urn:autonomics:run:run-1:node:deseq2_condition"]["autonomics:run"],
        json!("urn:autonomics:run:run-1")
    );
}

#[test]
fn dataframe_entities_join_the_derivation_chain() {
    let doc = doc();
    let df = "urn:autonomics:run:run-1:df:normalize";
    let original = "urn:autonomics:path:/bixbench/counts.txt";
    let out = "urn:autonomics:sha256:abc";
    // The df entity is generated by its node…
    assert!(
        doc["wasGeneratedBy"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["entity"] == df)
    );
    // …carries its reported shape (schema as literal JSON text)…
    assert_eq!(doc["entity"][df]["autonomics:rows"], json!(42));
    let schema = doc["entity"][df]["autonomics:schema"].as_str().unwrap();
    let schema: Value = serde_json::from_str(schema).unwrap();
    assert_eq!(schema["column_count"], json!(2));
    // …and links the chain: original → df → container output.
    assert!(
        doc["wasDerivedFrom"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["generatedEntity"] == df && r["usedEntity"] == original)
    );
    assert!(
        doc["wasDerivedFrom"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["generatedEntity"] == out && r["usedEntity"] == df)
    );
}

#[test]
fn container_evidence_lands_on_activities() {
    let doc = doc();
    let act = &doc["activity"]["urn:autonomics:run:run-1:node:deseq2_condition"];
    assert_eq!(act["autonomics:image_digest"], json!("sha256:ee"));
    assert_eq!(act["autonomics:exit_code"], json!(0));
    assert_eq!(act["autonomics:image"], json!("ghcr.io/x@sha256:ee"));
}

#[test]
fn snapshot_entity_is_used_by_the_run() {
    let doc = doc();
    let snap = "urn:autonomics:snapshot:snap1";
    assert_eq!(doc["entity"][snap]["autonomics:ref"], json!("asxl1-deseq2"));
    // The manifest joins the graph: the run consumed its own definition.
    assert!(
        doc["used"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["activity"] == "urn:autonomics:run:run-1" && r["entity"] == snap),
        "snapshot entity must be used by the run activity"
    );
}

/// PROV-JSON attribute values are never null; inside entity/activity/agent
/// records every attribute key is a qualified name (`prov:*`, `autonomics:*`).
/// Relation records and prefix declarations use structural keys instead.
#[test]
fn no_null_values_and_namespaced_attribute_keys() {
    let doc = doc();
    fn walk(value: &Value, path: &str, in_record: bool, problems: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (k, v) in map {
                    if v.is_null() {
                        problems.push(format!("null value at {path}.{k}"));
                    }
                    if in_record && !k.contains(':') {
                        problems.push(format!("unqualified attribute key {path}.{k}"));
                    }
                    let child_record =
                        in_record || matches!(k.as_str(), "entity" | "activity" | "agent");
                    walk(v, &format!("{path}.{k}"), child_record, problems);
                }
            }
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    walk(item, &format!("{path}[{i}]"), in_record, problems);
                }
            }
            _ => {}
        }
    }
    let mut problems = Vec::new();
    walk(&doc, "$", false, &mut problems);
    assert!(problems.is_empty(), "PROV-JSON conformance: {problems:#?}");
}

#[test]
fn file_entities_omit_unrecorded_fields() {
    let doc = doc();
    let original = &doc["entity"]["urn:autonomics:path:/bixbench/counts.txt"];
    assert!(
        original.get("autonomics:sha256").is_none(),
        "no recorded hash → key omitted, not null"
    );
    assert!(
        original.get("autonomics:format").is_some(),
        "recorded format is kept"
    );
}

/// The document alone reconstructs the DAG: every activity carries its spec
/// and dispatch order, every wiring edge appears as a port-role on `used` /
/// `wasGeneratedBy` and as a `wasInformedBy` between the node activities —
/// exactly once even though the manifest and the observed bindings both
/// declare it.
#[test]
fn specs_order_and_wiring_ports_join_the_graph() {
    let doc = doc();
    let normalize = "urn:autonomics:run:run-1:node:normalize";
    let deseq2 = "urn:autonomics:run:run-1:node:deseq2_condition";
    let df = "urn:autonomics:run:run-1:df:normalize";

    // Specs from the executed manifest, as compact JSON literals.
    let spec: Value =
        serde_json::from_str(doc["activity"][normalize]["autonomics:spec"].as_str().unwrap())
            .unwrap();
    assert_eq!(spec["sql_query"], json!("SELECT a FROM counts"));

    // Dispatch order: the scheduler's serialization of the run.
    assert_eq!(
        doc["activity"][normalize]["autonomics:dispatch_seq"],
        json!(1)
    );
    assert_eq!(doc["activity"][deseq2]["autonomics:dispatch_seq"], json!(2));

    // `used` names the wiring edge: producer:from_port>to_port.
    let used = doc["used"].as_array().unwrap();
    let df_binding = used
        .iter()
        .find(|r| r["activity"] == deseq2 && r["entity"] == df)
        .unwrap();
    assert_eq!(df_binding["prov:role"], json!("normalize:0>0"));
    let file_binding = used
        .iter()
        .find(|r| {
            r["activity"] == normalize && r["entity"] == "urn:autonomics:path:/bixbench/counts.txt"
        })
        .unwrap();
    assert_eq!(file_binding["prov:role"], json!("counts_file:0>0"));

    // `wasGeneratedBy` names the producing output port.
    let generation = doc["wasGeneratedBy"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["activity"] == deseq2 && r["entity"] == "urn:autonomics:sha256:abc")
        .unwrap();
    assert_eq!(generation["prov:role"], json!("port:0"));

    // Node-level topology: exactly one informed record per declared edge
    // (manifest + observed binding merge), ports in the role.
    let informed = doc["wasInformedBy"].as_array().unwrap();
    let matches: Vec<&Value> = informed
        .iter()
        .filter(|r| r["activity"] == normalize && r["informed"] == "urn:autonomics:run:run-1:node:counts_file")
        .collect();
    assert_eq!(matches.len(), 1, "declared + observed edges dedupe");
    assert_eq!(matches[0]["prov:role"], json!("0>0"));
}

use reactome::types::*;
use reactome::convert::{analysis_to_table, pathways_to_table};

#[test]
fn decodes_species() {
    let raw = serde_json::json!({
        "dbId": 48887,
        "displayName": "Homo sapiens",
        "name": ["Homo sapiens", "H. sapiens", "Hs", "human", "man"],
        "taxId": "9606",
        "abbreviation": "HSA",
        "className": "Species",
        "schemaClass": "Species"
    });
    let species: Species = serde_json::from_value(raw).unwrap();
    assert_eq!(species.db_id, 48887);
    assert_eq!(species.display_name, "Homo sapiens");
    assert_eq!(species.tax_id, "9606");
    assert_eq!(species.abbreviation.as_deref(), Some("HSA"));
}

#[test]
fn decodes_pathway() {
    let raw = serde_json::json!({
        "dbId": 1640170,
        "displayName": "Cell Cycle",
        "stId": "R-HSA-1640170",
        "stIdVersion": "R-HSA-1640170.5",
        "isInDisease": false,
        "isInferred": false,
        "hasDiagram": true,
        "hasEHLD": true,
        "speciesName": "Homo sapiens",
        "schemaClass": "TopLevelPathway"
    });
    let pathway: Pathway = serde_json::from_value(raw).unwrap();
    assert_eq!(pathway.db_id, 1640170);
    assert_eq!(pathway.stable_id.as_deref(), Some("R-HSA-1640170"));
    assert!(pathway.has_diagram);
    assert_eq!(pathway.species_name.as_deref(), Some("Homo sapiens"));
}

#[test]
fn decodes_mapped_pathway() {
    let raw = serde_json::json!({
        "dbId": 6796648,
        "displayName": "TP53 Regulates Transcription of DNA Repair Genes",
        "stId": "R-HSA-6796648",
        "schemaClass": "Pathway",
        "species": [{"dbId": 48887, "taxId": "9606"}]
    });
    let mapped: MappedPathway = serde_json::from_value(raw).unwrap();
    assert_eq!(mapped.stable_id.as_deref(), Some("R-HSA-6796648"));
    assert!(mapped.display_name.contains("TP53"));
}

#[test]
fn decodes_analysis_result() {
    let raw = serde_json::json!({
        "summary": {
            "token": "MjAyNjA5MDExOTU4MjFfNDMwOA%3D%3D",
            "projection": true,
            "interactors": false,
            "type": "OVERREPRESENTATION",
            "sampleName": "",
            "text": true,
            "includeDisease": true
        },
        "expression": {"columnNames": []},
        "identifiersNotFound": 0,
        "pathwaysFound": 2,
        "pathways": [{
            "stId": "R-HSA-6796648",
            "dbId": 6796648,
            "name": "TP53 Regulates Transcription of DNA Repair Genes",
            "species": {"dbId": 48887, "taxId": "9606", "name": "Homo sapiens"},
            "llp": true,
            "entities": {
                "resource": "TOTAL",
                "total": 86,
                "found": 4,
                "ratio": 0.005,
                "pValue": 5.39763289619799e-08,
                "fdr": 1.4033845530114775e-05,
                "exp": []
            },
            "inDisease": false
        }],
        "resourceSummary": [{
            "resource": "TOTAL",
            "pathways": 2,
            "filtered": 2
        }],
        "speciesSummary": [{
            "dbId": 48887,
            "taxId": "9606",
            "name": "Homo sapiens",
            "pathways": 2,
            "filtered": 2
        }],
        "warnings": ["Missing header. Using a default one."]
    });
    let result: AnalysisResult = serde_json::from_value(raw).unwrap();
    assert_eq!(result.summary.analysis_type.as_deref(), Some("OVERREPRESENTATION"));
    assert!(result.summary.projection);
    assert_eq!(result.pathways_found, 2);
    assert_eq!(result.pathways.len(), 1);
    let p = &result.pathways[0];
    assert_eq!(p.stable_id, "R-HSA-6796648");
    assert!(p.entities.p_value.unwrap() < 1e-6);
    assert!(p.entities.fdr.unwrap() < 1e-4);
    assert_eq!(p.entities.found, 4);
    assert_eq!(p.entities.total, 86);
}

#[test]
fn pathway_table_has_stable_columns() {
    let pathways = vec![
        Pathway {
            db_id: 1640170,
            display_name: "Cell Cycle".to_string(),
            stable_id: Some("R-HSA-1640170".to_string()),
            ..Default::default()
        },
        Pathway {
            db_id: 9612973,
            display_name: "Autophagy".to_string(),
            stable_id: Some("R-HSA-9612973".to_string()),
            ..Default::default()
        },
    ];
    let table = pathways_to_table(&pathways);
    assert_eq!(table.len(), 2);
    assert!(table.columns.contains(&"stable_id".to_string()));
    assert!(table.columns.contains(&"display_name".to_string()));
    assert!(
        table
            .rows
            .iter()
            .all(|row| row.iter().all(|cell| !cell.is_array() && !cell.is_object()))
    );
}

#[test]
fn analysis_table_reports_p_value_and_fdr() {
    let result = AnalysisResult {
        pathways: vec![EnrichedPathway {
            stable_id: "R-HSA-6796648".to_string(),
            name: "TP53 Regulates Transcription".to_string(),
            entities: PathwayEntities {
                found: 4,
                total: 86,
                p_value: Some(5.4e-8),
                fdr: Some(1.4e-5),
                ..Default::default()
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let table = analysis_to_table(&result);
    assert_eq!(table.len(), 1);
    assert!(table.columns.contains(&"p_value".to_string()));
    assert!(table.columns.contains(&"fdr".to_string()));
}

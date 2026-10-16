use eutils::geo::GeoSearchRequest;

mod common;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
#[ignore]
#[common::serial]
async fn geo_search_returns_typed_records() -> TestResult {
    let client = common::test_client();
    let request = GeoSearchRequest::new("single cell RNA-seq")
        .with_organism("Homo sapiens")
        .with_dataset_type("Expression profiling by high throughput sequencing")
        .with_retmax(5);
    let response = client.geo_search_records(&request).await?;

    assert!(!response.records.is_empty());
    assert!(response.records.len() <= 5);
    assert!(
        response
            .records
            .iter()
            .all(|record| record.accession.starts_with("GSE"))
    );
    assert!(
        response
            .records
            .iter()
            .all(|record| !record.ftp_link.is_empty())
    );

    Ok(())
}

#[tokio::test]
#[ignore]
#[common::serial]
async fn geo_lookup_exact_series_accession() -> TestResult {
    let client = common::test_client();
    common::rate_limit();
    let records = client.geo_lookup_accession("GSE48094").await?;

    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.uid, "200048094");
    assert_eq!(record.accession, "GSE48094");
    assert_eq!(record.entry_type, "GSE");
    assert_eq!(record.bioproject, "PRJNA208991");
    assert_eq!(record.n_samples, Some(6));
    assert_eq!(
        record.ftp_link,
        "ftp://ftp.ncbi.nlm.nih.gov/geo/series/GSE48nnn/GSE48094/"
    );

    Ok(())
}

#[tokio::test]
#[ignore]
#[common::serial]
async fn geo_fetch_series_brief_text() -> TestResult {
    let client = common::test_client();
    common::rate_limit();
    let text = client
        .geo_fetch("200048094", Some("brief"), Some("text"))
        .await?;

    assert!(text.contains("GSE48094"));
    assert!(text.contains("microRNA Expression of mice corneal endothelia"));

    Ok(())
}

use biofusion::datasource::BioReadOptions;
use biofusion::ext::DataFusionReadExt;
use datafusion::prelude::SessionContext;

#[tokio::test]
async fn reads_a_json_array_as_a_dataframe() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("records.json");
    std::fs::write(&path, r#"[{"id":1},{"id":2}]"#).unwrap();

    let ctx = SessionContext::new();
    let df = ctx
        .read_bio_json(path.to_str().unwrap(), BioReadOptions::default())
        .await
        .unwrap();

    assert_eq!(df.clone().count().await.unwrap(), 2);
    assert!(df.schema().field_with_name(None, "id").is_ok());
}

#[tokio::test]
async fn reads_ndjson_as_a_dataframe() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("records.ndjson");
    std::fs::write(&path, "{\"id\":1}\n{\"id\":2}\n").unwrap();

    let ctx = SessionContext::new();
    let df = ctx
        .read_bio_json(path.to_str().unwrap(), BioReadOptions::default())
        .await
        .unwrap();

    assert_eq!(df.clone().count().await.unwrap(), 2);
    assert!(df.schema().field_with_name(None, "id").is_ok());
}

#[tokio::test]
async fn reads_a_gzipped_json_array_as_a_dataframe() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("records.json.gz");
    let mut encoder =
        flate2::write::GzEncoder::new(std::fs::File::create(&path).unwrap(), Default::default());
    std::io::Write::write_all(&mut encoder, br#"[{"id":1},{"id":2}]"#).unwrap();
    encoder.finish().unwrap();

    let ctx = SessionContext::new();
    let df = ctx
        .read_bio_json(path.to_str().unwrap(), BioReadOptions::default())
        .await
        .unwrap();

    assert_eq!(df.clone().count().await.unwrap(), 2);
    assert!(df.schema().field_with_name(None, "id").is_ok());
}

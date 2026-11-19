use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?).join("proto");
    let include_str = proto_root.to_string_lossy().into_owned();
    let includes = [include_str.as_str()];
    let files = [
        "alphagenome/protos/atlas_service.proto",
        "alphagenome/protos/v1/dna_model_service.proto",
    ];
    for file in &files {
        println!("cargo:rerun-if-changed={}", proto_root.join(file).display());
    }
    tonic_prost_build::configure()
        .build_server(false)
        .compile_protos(&files, &includes)?;
    Ok(())
}

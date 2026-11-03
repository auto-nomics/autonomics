//! Explicitly admitted site smoke; never runs in the ordinary unit-test suite.

#[path = "../examples/apptainer_slurm_smoke.rs"]
mod site_smoke;

#[tokio::test]
#[ignore = "submits a real synthetic Slurm job; requires explicit operator approval and managed evidence"]
async fn apptainer_site_synthetic_roundtrip() {
    let home = std::env::var_os("HOME").expect("operator HOME");
    let manifest = std::path::PathBuf::from(home)
        .join(".local/state/autonomics/apptainer-site/smoke-images.json");
    let permanent =
        std::env::var("AUTONOMICS_VALIDATION_PERMANENT").expect("managed acceptance required");
    site_smoke::run(&manifest,
        "ghcr.io/zj-2002/biotools-py@sha256:7ad6aeafa87672699d28b61b6bed12fc81fcdbfc43222a0abf17e2d56fb806f5",
        std::path::Path::new(&permanent)).await.expect("synthetic site acceptance");
}

//! Run the real Python helper regression suites as part of the normal Cargo
//! test entry. Their fixtures are self-cleaning and never contact a cluster.
use std::{fs, path::Path, process::Command};

use serde_json::json;
use sha2::{Digest, Sha256};

#[test]
fn python_remote_helper_regressions() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    // The Python suites import source files. Ensure those are the same helper
    // bytes embedded in the actual tested tool, rather than a newer checkout.
    for (source, embedded) in [("src/remote_jobs.py", crate::jobs::REMOTE_JOBS_HELPER)] {
        assert_eq!(fs::read(root.join(source)).unwrap(), embedded.as_bytes());
    }
    let python = std::env::var_os("AUTONOMICS_HPC_TEST_PYTHON").unwrap_or_else(|| "python3".into());
    let mut results = Vec::new();
    for suite in ["jobs_remote_test.py", "quota_remote_test.py"] {
        let output = Command::new(&python)
            .arg("-B")
            .arg(root.join("tests").join(suite))
            .output()
            .expect("Python 3 is required for the HPC helper regression suites");
        results.push(json!({
            "suite":suite, "success":output.status.success(), "exit_code":output.status.code(),
            "stdout":String::from_utf8_lossy(&output.stdout),
            "stderr":String::from_utf8_lossy(&output.stderr)
        }));
    }
    let mut hashes = serde_json::Map::new();
    for source in [
        "src/lib.rs",
        "src/jobs.rs",
        "src/remote_jobs.py",
        "tests/jobs_remote_test.py",
        "tests/quota_remote_test.py",
        "src/helper_tests.rs",
    ] {
        hashes.insert(
            source.into(),
            json!(format!(
                "{:x}",
                Sha256::digest(fs::read(root.join(source)).unwrap())
            )),
        );
    }
    let report = json!({
        "scope":"technical helper regressions using toy fixtures; no live account or scientific execution",
        "source_sha256":hashes, "results":results
    });
    if let Some(permanent) = std::env::var_os("AUTONOMICS_VALIDATION_PERMANENT") {
        fs::write(
            Path::new(&permanent).join("hpc-remote-helper-regressions.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .expect("write managed permanent technical evidence");
    }
    assert!(
        results.iter().all(|result| result["success"] == true),
        "Python helper regressions failed: {report}"
    );
}

//! Inject the workspace's git commit at build time for audit records.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    // dag-core lives at `<workspace>/crates/dag-core`; the repository root is
    // two levels up. A missing `.git` (release tarball, sdist) degrades to
    // "unknown" rather than failing the build.
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo_root = manifest_dir
        .parent()
        .and_then(|crates| crates.parent())
        .expect("dag-core lives at <workspace>/crates/dag-core")
        .to_path_buf();

    // Re-run when HEAD moves so builds always carry the checked-out revision.
    println!(
        "cargo:rerun-if-changed={}",
        repo_root.join(".git").join("HEAD").display()
    );

    let revision = Command::new("git")
        .arg("-C")
        .arg(&repo_root)
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|stdout| stdout.trim().to_string())
        .filter(|short| !short.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=AUTONOMICS_SOURCE_REVISION={revision}");
}

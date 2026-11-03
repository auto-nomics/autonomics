//! Inject the workspace's git commit at build time for audit records and
//! the node execution fingerprint.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    // dag-core lives at `<workspace>/crates/dag-core`; the repository root is
    // two levels up. Frozen sources must carry an explicit release stamp.
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo_root = manifest_dir
        .parent()
        .and_then(|crates| crates.parent())
        .expect("dag-core lives at <workspace>/crates/dag-core")
        .to_path_buf();

    println!("cargo:rerun-if-env-changed=AUTONOMICS_SOURCE_REVISION");
    println!("cargo:rerun-if-changed=build.rs");
    // A measured release/CI stamp is the authority for this build. Watching
    // unrelated Git maintenance would only invalidate an unchanged stamp.
    if let Ok(stamped) = std::env::var("AUTONOMICS_SOURCE_REVISION") {
        assert!(
            !stamped.is_empty() && !stamped.chars().any(char::is_control),
            "AUTONOMICS_SOURCE_REVISION must be nonempty and contain no control characters"
        );
        println!("cargo:rustc-env=AUTONOMICS_SOURCE_REVISION={stamped}");
        return;
    }

    // Do not let Git discover an unrelated ancestor repository around a frozen
    // source directory. Watch .git as well, so removing/replacing repository
    // metadata invalidates a previous incremental build.
    watch_existing_path(repo_root.join(".git"));
    assert!(
        repo_root.join(".git").exists(),
        "no workspace .git: set AUTONOMICS_SOURCE_REVISION from verified frozen-source provenance"
    );

    // Re-run when HEAD moves so builds always carry the checked-out revision.
    // Resolve through git: .git is a file in worktrees, and HEAD remains
    // unchanged when a commit advances the same branch. Watch both the HEAD
    // file and its symbolic ref, plus packed refs used by git maintenance.
    for git_path in ["HEAD", "packed-refs"] {
        if let Some(path) = git_output(&repo_root, &["rev-parse", "--git-path", git_path]) {
            let path = PathBuf::from(path);
            let path = if path.is_absolute() {
                path
            } else {
                repo_root.join(path)
            };
            watch_existing_path(path);
        }
    }
    if let Some(reference) = git_output(&repo_root, &["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = git_output(&repo_root, &["rev-parse", "--git-path", &reference])
    {
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            repo_root.join(path)
        };
        watch_existing_path(path);
    }
    let revision =
        git_output(&repo_root, &["rev-parse", "HEAD"])
            .expect("cannot resolve workspace HEAD: set AUTONOMICS_SOURCE_REVISION from verified source provenance");

    println!("cargo:rustc-env=AUTONOMICS_SOURCE_REVISION={revision}");
}

fn watch_existing_path(mut path: PathBuf) {
    // Cargo treats a missing watched path as permanently dirty. The existing
    // ancestor also observes creation/removal of a loose ref or packed-refs.
    while !path.exists() {
        if !path.pop() {
            return;
        }
    }
    println!("cargo:rerun-if-changed={}", path.display());
}

fn git_output(repo_root: &std::path::Path, args: &[&str]) -> Option<String> {
    Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|stdout| stdout.trim().to_string())
        .filter(|value| !value.is_empty())
}

use crate::PodmanRuntime;
use crate::types::PullPolicy;

use super::parser::{parse_image_inspect, parse_image_records};
use super::podman::{
    image_exists_args, image_inspect_args, image_list_args, image_pull_args, image_remove_args,
};
use super::types::ImageManager;
use super::types::{ImageListOptions, ImageRemoveOptions};

#[test]
fn image_commands_are_direct_and_explicit() {
    assert_eq!(image_exists_args("tool:1"), ["exists", "tool:1"]);
    assert_eq!(
        image_inspect_args("tool:1"),
        ["inspect", "--format={{json .}}", "tool:1"]
    );
    assert_eq!(
        image_list_args(&ImageListOptions::default().with_filter("dangling=true")),
        ["ls", "--format={{json .}}", "--filter", "dangling=true"]
    );
    assert_eq!(
        image_pull_args("tool:1", PullPolicy::Newer),
        ["pull", "--policy=newer", "tool:1"]
    );
    assert_eq!(
        image_remove_args(
            "tool:1",
            ImageRemoveOptions {
                force: true,
                ignore_missing: true
            }
        ),
        ["rm", "--force", "--ignore", "tool:1"]
    );
}

#[test]
fn parses_podman_image_list_json_lines() {
    let stdout = r#"{"repository":"localhost/tool","tag":"latest","Id":"abc","RepoTags":["localhost/tool:latest"],"RepoDigests":["localhost/tool@sha256:abc"],"Created":123,"Size":100,"SharedSize":0,"VirtualSize":100,"Containers":2,"Labels":{"a":"b"},"Names":["localhost/tool:latest"]}
{"repository":"<none>","tag":"<none>","Id":"def","RepoTags":null,"RepoDigests":[],"Created":456,"Size":200,"Containers":0,"Dangling":true,"Digest":"sha256:def"}"#;

    let records = parse_image_records(stdout).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].id, "abc");
    assert_eq!(records[0].repo_tags, ["localhost/tool:latest"]);
    assert_eq!(records[0].labels.get("a").map(String::as_str), Some("b"));
    assert!(!records[0].dangling);
    assert!(records[1].dangling);
    assert_eq!(records[1].digest.as_deref(), Some("sha256:def"));
}

#[test]
fn parses_podman_image_inspect_json() {
    let stdout = r#"{"Id":"abc","Digest":"sha256:abc","RepoTags":["tool:1"],"RepoDigests":["tool@sha256:abc"],"Created":"2026-01-01T00:00:00Z","Architecture":"amd64","Os":"linux","Size":100,"Labels":{"a":"b"}}"#;

    let details = parse_image_inspect(stdout).unwrap();
    assert_eq!(details.id, "abc");
    assert_eq!(details.repo_tags, ["tool:1"]);
    assert_eq!(details.architecture.as_deref(), Some("amd64"));
    assert_eq!(details.labels.get("a").map(String::as_str), Some("b"));
    assert_eq!(details.raw["Size"], 100);
}

#[tokio::test]
#[ignore = "requires a working rootless Podman runtime and a local OCI image"]
async fn real_podman_inspects_lists_and_ensures_local_image() {
    let image = std::env::var("AUTONOMICS_CONTAINER_IT_IMAGE")
        .unwrap_or_else(|_| "docker.io/library/debian:bookworm-slim".into());
    let runtime = PodmanRuntime::from_env();

    assert!(runtime.image_exists(&image).await.unwrap());
    let details = runtime.image_inspect(&image).await.unwrap();
    assert!(!details.id.trim().is_empty());

    let records = runtime
        .image_list(ImageListOptions::default().with_filter(format!("reference={image}")))
        .await
        .unwrap();
    assert!(!records.is_empty());

    let ensured = runtime
        .ensure_image(&image, PullPolicy::Never)
        .await
        .unwrap();
    assert!(!ensured.pulled);
    assert_eq!(ensured.image.id, details.id);
}

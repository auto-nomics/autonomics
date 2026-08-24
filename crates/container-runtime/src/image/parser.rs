use std::collections::BTreeMap;

use serde::Deserialize;

use crate::ContainerRuntimeError;

use super::types::{ImageInspect, ImageRecord};

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawImageRecord {
    id: String,
    #[serde(rename = "repository")]
    repository: String,
    #[serde(rename = "tag")]
    tag: String,
    #[serde(default)]
    repo_tags: Option<Vec<String>>,
    #[serde(default)]
    repo_digests: Option<Vec<String>>,
    #[serde(default)]
    digest: Option<String>,
    created: i64,
    size: u64,
    #[serde(default)]
    shared_size: u64,
    #[serde(default)]
    virtual_size: u64,
    #[serde(default)]
    containers: i64,
    #[serde(default)]
    dangling: bool,
    #[serde(default)]
    labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    names: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawImageInspect {
    id: String,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    repo_tags: Option<Vec<String>>,
    #[serde(default)]
    repo_digests: Option<Vec<String>>,
    #[serde(default)]
    created: Option<String>,
    #[serde(default)]
    architecture: Option<String>,
    #[serde(default)]
    os: Option<String>,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    virtual_size: Option<u64>,
    #[serde(default)]
    labels: Option<BTreeMap<String, String>>,
}

pub(super) fn parse_image_records(stdout: &str) -> Result<Vec<ImageRecord>, ContainerRuntimeError> {
    let mut records = Vec::new();
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let raw: serde_json::Value = serde_json::from_str(line).map_err(|error| {
            ContainerRuntimeError::Invalid(format!("parse podman image list output: {error}"))
        })?;
        let raw_record: RawImageRecord = serde_json::from_value(raw).map_err(|error| {
            ContainerRuntimeError::Invalid(format!("decode podman image record: {error}"))
        })?;
        records.push(ImageRecord {
            id: raw_record.id,
            repository: raw_record.repository,
            tag: raw_record.tag,
            repo_tags: raw_record.repo_tags.unwrap_or_default(),
            repo_digests: raw_record.repo_digests.unwrap_or_default(),
            digest: raw_record.digest,
            created_unix_seconds: raw_record.created,
            size_bytes: raw_record.size,
            shared_size_bytes: raw_record.shared_size,
            virtual_size_bytes: raw_record.virtual_size,
            container_count: raw_record.containers,
            dangling: raw_record.dangling,
            labels: raw_record.labels.unwrap_or_default(),
            names: raw_record.names.unwrap_or_default(),
        });
    }
    Ok(records)
}

pub(super) fn parse_image_inspect(stdout: &str) -> Result<ImageInspect, ContainerRuntimeError> {
    let raw: serde_json::Value = serde_json::from_str(stdout.trim()).map_err(|error| {
        ContainerRuntimeError::Invalid(format!("parse podman image inspect output: {error}"))
    })?;
    let value: RawImageInspect = serde_json::from_value(raw.clone()).map_err(|error| {
        ContainerRuntimeError::Invalid(format!("decode podman image details: {error}"))
    })?;
    Ok(ImageInspect {
        id: value.id,
        digest: value.digest,
        repo_tags: value.repo_tags.unwrap_or_default(),
        repo_digests: value.repo_digests.unwrap_or_default(),
        created: value.created,
        architecture: value.architecture,
        os: value.os,
        size_bytes: value.size,
        virtual_size_bytes: value.virtual_size,
        labels: value.labels.unwrap_or_default(),
        raw,
    })
}

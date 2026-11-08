//! Typed channel values for task transport.
//!
//! A JSON value and a file are different dataflow values even when the JSON
//! value happens to contain a path. Explicit tags keep that distinction at
//! process and machine boundaries, including inside nested collections.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::value::{DomainId, FileRef};

/// A transport representation, not an in-memory DataFusion plan. `Arrow`
/// refers to a materialized Arrow IPC artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ChannelTransportValue {
    Json(serde_json::Value),
    File(FileRef),
    Arrow(FileRef),
    List(Vec<ChannelTransportValue>),
    Tuple(Vec<ChannelTransportValue>),
    Map(BTreeMap<String, ChannelTransportValue>),
}

impl ChannelTransportValue {
    /// Decode untrusted transport bytes with a byte bound and structural limits.
    pub fn decode(
        bytes: &[u8],
        max_bytes: usize,
        max_depth: usize,
        max_nodes: usize,
    ) -> Result<Self, String> {
        if bytes.len() > max_bytes {
            return Err("channel value exceeds byte limit".into());
        }
        let value: Self =
            serde_json::from_slice(bytes).map_err(|e| format!("invalid channel value: {e}"))?;
        value.validate(max_depth, max_nodes)?;
        Ok(value)
    }

    /// Check typed values and nested JSON before staging artifacts.
    pub fn validate(&self, max_depth: usize, max_nodes: usize) -> Result<(), String> {
        enum Item<'a> {
            Typed(&'a ChannelTransportValue),
            Json(&'a serde_json::Value),
        }
        let mut pending = vec![(Item::Typed(self), 0usize)];
        let mut count = 0usize;
        while let Some((value, depth)) = pending.pop() {
            count = count
                .checked_add(1)
                .ok_or("channel value node count overflow")?;
            if count > max_nodes || depth > max_depth {
                return Err("channel value exceeds nesting or item limit".into());
            }
            match value {
                Item::Typed(Self::File(file) | Self::Arrow(file)) if file.path.is_empty() => {
                    return Err("channel artifact path must not be empty".into());
                }
                Item::Typed(Self::List(items) | Self::Tuple(items)) => {
                    pending.extend(items.iter().map(|item| (Item::Typed(item), depth + 1)));
                }
                Item::Typed(Self::Map(items)) => {
                    pending.extend(items.values().map(|item| (Item::Typed(item), depth + 1)));
                }
                Item::Typed(Self::Json(value)) => pending.push((Item::Json(value), depth + 1)),
                Item::Json(serde_json::Value::Array(items)) => {
                    pending.extend(items.iter().map(|item| (Item::Json(item), depth + 1)))
                }
                Item::Json(serde_json::Value::Object(items)) => {
                    pending.extend(items.values().map(|item| (Item::Json(item), depth + 1)))
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Artifact references in deterministic traversal order, including Arrow.
    pub fn artifacts(&self) -> Vec<&FileRef> {
        let mut result = Vec::new();
        let mut pending = vec![self];
        while let Some(value) = pending.pop() {
            match value {
                Self::File(file) | Self::Arrow(file) => result.push(file),
                Self::List(items) | Self::Tuple(items) => pending.extend(items.iter().rev()),
                Self::Map(items) => pending.extend(items.values().rev()),
                Self::Json(_) => {}
            }
        }
        result
    }

    /// Install staged artifact references in `artifacts()` order. A wrong
    /// count leaves the value untouched.
    pub fn replace_artifacts(&mut self, replacements: Vec<FileRef>) -> Result<(), String> {
        if self.artifacts().len() != replacements.len() {
            return Err("channel artifact replacement count does not match".into());
        }
        let mut replacements = replacements.into_iter();
        let mut pending = vec![self];
        while let Some(value) = pending.pop() {
            match value {
                Self::File(file) | Self::Arrow(file) => {
                    *file = replacements.next().expect("replacement count checked");
                }
                Self::List(items) | Self::Tuple(items) => pending.extend(items.iter_mut().rev()),
                Self::Map(items) => pending.extend(items.values_mut().rev()),
                Self::Json(_) => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::FileFingerprint;

    fn file(path: &str) -> FileRef {
        FileRef {
            domain: DomainId::local(),
            path: path.into(),
            format: Some("fastq".into()),
            fingerprint: Some(FileFingerprint {
                size: 7,
                mtime_ns: 9,
                content_hash: Some("digest".into()),
                immutable_remote: false,
            }),
        }
    }

    #[test]
    fn nested_transport_round_trip_preserves_types_and_fingerprints() {
        let value = ChannelTransportValue::Tuple(vec![
            ChannelTransportValue::Json(serde_json::json!({"type":"file", "value":"a"})),
            ChannelTransportValue::List(vec![
                ChannelTransportValue::File(file("a")),
                ChannelTransportValue::Arrow(file("b")),
            ]),
        ]);
        let encoded = serde_json::to_vec(&value).unwrap();
        let decoded: ChannelTransportValue = serde_json::from_slice(&encoded).unwrap();
        decoded.validate(8, 10).unwrap();
        let ChannelTransportValue::Tuple(items) = &decoded else {
            panic!("tuple expected")
        };
        assert!(matches!(&items[0], ChannelTransportValue::Json(_)));
        let files = decoded.artifacts();
        assert_eq!(
            files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(
            files[0]
                .fingerprint
                .as_ref()
                .unwrap()
                .content_hash
                .as_deref(),
            Some("digest")
        );
    }

    #[test]
    fn replacements_are_ordered_and_count_is_checked_before_mutation() {
        let mut value = ChannelTransportValue::Map(BTreeMap::from([
            ("b".into(), ChannelTransportValue::File(file("b"))),
            (
                "a".into(),
                ChannelTransportValue::Tuple(vec![ChannelTransportValue::Arrow(file("a"))]),
            ),
        ]));
        assert!(value.replace_artifacts(vec![file("only")]).is_err());
        assert_eq!(
            value
                .artifacts()
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        value
            .replace_artifacts(vec![file("remote-a"), file("remote-b")])
            .unwrap();
        assert_eq!(
            value
                .artifacts()
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            ["remote-a", "remote-b"]
        );
    }

    #[test]
    fn rejects_excessive_depth_count_and_empty_paths() {
        let value = ChannelTransportValue::Tuple(vec![ChannelTransportValue::List(vec![
            ChannelTransportValue::File(file("a")),
        ])]);
        assert!(value.validate(1, 10).is_err());
        assert!(value.validate(10, 2).is_err());
        assert!(
            ChannelTransportValue::File(file(""))
                .validate(1, 1)
                .is_err()
        );
    }

    #[test]
    fn bounded_decode_checks_json_leaf_structure_and_protocol_fields() {
        let value = ChannelTransportValue::Json(serde_json::json!({"a":[1,2,3]}));
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(ChannelTransportValue::decode(&bytes, bytes.len() - 1, 8, 20).is_err());
        assert!(ChannelTransportValue::decode(&bytes, bytes.len(), 2, 20).is_err());
        assert!(ChannelTransportValue::decode(&bytes, bytes.len(), 8, 4).is_err());
        assert!(ChannelTransportValue::decode(&bytes, bytes.len(), 8, 20).is_ok());
        assert!(
            ChannelTransportValue::decode(
                br#"{"type":"json","value":42,"unexpected":1}"#,
                1024,
                8,
                20
            )
            .is_err()
        );
    }
}

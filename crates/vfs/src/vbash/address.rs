//! Publish addresses at the agent boundary without rewriting file contents.
use agentik_core::tools::ToolError;
use agentik_sdk::types::{ToolResult, ToolResultContent};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::OpendalFileStorage;

/// Agent-facing mount discovery. Host/backend paths are metadata only.
#[derive(Debug, Clone, Serialize)]
pub struct AgentMount {
    pub path: String,
    pub uri: String,
    pub virtual_path: String,
    pub domain: Option<String>,
    pub domain_root: Option<String>,
    pub backend: String,
    pub source: String,
    pub read_only: bool,
}

pub(super) fn uri(storage: &OpendalFileStorage, path: &str) -> Result<String, ToolError> {
    let resolved = storage
        .try_resolve_virtual(path)
        .map_err(|e| e.to_string())?;
    match storage.canonical_uri(path) {
        Ok(uri) => Ok(uri),
        // Unnamed remote mounts survive only in legacy callers. Do not invent
        // a domain or claim that their objects belong to the local namespace.
        Err(error) if error.kind() == opendal::ErrorKind::ConfigInvalid => {
            Ok(crate::uri::uri(resolved))
        }
        Err(error) => Err(error.to_string().into()),
    }
}

pub fn agent_mounts(storage: &OpendalFileStorage) -> Result<Vec<AgentMount>, ToolError> {
    crate::browse::mounts(storage)
        .into_iter()
        .map(|mount| {
            let uri = uri(storage, &mount.path)?;
            let domain = crate::uri::domain(&uri).map(str::to_owned);
            Ok(AgentMount {
                path: uri.clone(),
                domain_root: domain.as_ref().map(|d| format!("{d}://")),
                uri,
                virtual_path: mount.path,
                domain,
                backend: mount.backend,
                source: mount.source,
                read_only: mount.read_only,
            })
        })
        .collect()
}

fn publish_fields(
    storage: &OpendalFileStorage,
    object: &mut Map<String, Value>,
) -> Result<(), ToolError> {
    if let Some(path) = object
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_owned)
    {
        let address = uri(storage, &path)?;
        object.entry("virtual_path").or_insert(Value::String(
            storage
                .try_resolve_virtual(&path)
                .map_err(|e| e.to_string())?,
        ));
        object.insert("domain".into(), crate::uri::domain(&address).into());
        object.insert("path".into(), address.clone().into());
        object.insert("uri".into(), address.into());
    }
    for key in [
        "src",
        "dst",
        "sampled_existing_vfs_path",
        "resolved_existing_vfs_path",
    ] {
        if let Some(path) = object.get(key).and_then(Value::as_str) {
            let address = uri(storage, path)?;
            let key = key.replace("_vfs_path", "_uri");
            object.insert(key, address.into());
        }
    }
    object.remove("sampled_existing_vfs_path");
    object.remove("resolved_existing_vfs_path");
    Ok(())
}

pub(super) fn publish_result(
    storage: &OpendalFileStorage,
    mut result: ToolResult,
) -> Result<ToolResult, ToolError> {
    if result.is_error != Some(true) {
        if let ToolResultContent::Json(Value::Object(object)) = &mut result.content {
            publish_fields(storage, object)?;
            // Only tool-owned address records. Never walk arbitrary notebook,
            // JSON file contents, text, source_version tokens, or image data.
            for key in ["entries", "changes"] {
                if let Some(Value::Array(entries)) = object.get_mut(key) {
                    for entry in entries {
                        if let Value::Object(entry) = entry {
                            publish_fields(storage, entry)?;
                        }
                    }
                }
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vbash::{VfsBashInput, VfsBashTool};
    use agentik_core::tools::ToolFunction;
    use serde_json::json;
    use std::sync::Arc;

    async fn call(tool: &VfsBashTool, args: Value) -> Value {
        let input: VfsBashInput = serde_json::from_value(args).unwrap();
        let result = tool.run(input).await.unwrap();
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let ToolResultContent::Json(json) = result.content else {
            panic!("expected JSON")
        };
        json
    }

    #[tokio::test]
    async fn agent_addresses_roundtrip_across_domains_and_legacy_inputs() {
        let local = tempfile::tempdir().unwrap();
        let remote = tempfile::tempdir().unwrap();
        std::fs::create_dir(remote.path().join("readonly")).unwrap();
        let manifest = crate::VfsManifest::from_toml(&format!(
            "[[backend]]\nid='local'\ntype='local'\nroot={}\n[[backend]]\nid='remote'\ntype='local'\nroot={}\n[[mount]]\npath='/'\nbackend='local'\nsource='/'\ndomain='local'\n[[mount]]\npath='/data/ht'\nbackend='remote'\nsource='/'\ndomain='ht'\n[[mount]]\npath='/data/ht/readonly'\nbackend='remote'\nsource={}\nread_only=true\n",
            serde_json::to_string(&local.path().to_string_lossy()).unwrap(),
            serde_json::to_string(&remote.path().to_string_lossy()).unwrap(),
            serde_json::to_string(&remote.path().join("readonly").to_string_lossy()).unwrap(),
        )).unwrap();
        let storage = Arc::new(OpendalFileStorage::with_mounts(
            local.path(),
            Arc::new(crate::MountedObjectStore::from_manifest(&manifest).unwrap()),
        ));
        let tool = VfsBashTool { storage };
        let mounts = call(&tool, json!({"op":"mount_list"})).await;
        let mounts = mounts["mounts"].as_array().unwrap();
        assert!(
            mounts
                .iter()
                .any(|m| m["uri"] == "ht://" && m["virtual_path"] == "/data/ht")
        );
        assert!(
            mounts.iter().any(|m| m["uri"] == "ht://readonly"
                && m["domain"] == "ht"
                && m["read_only"] == true)
        );
        let contents = "needle {\"path\":\"/do-not-rewrite\"}\nsecond\n";
        let written = call(
            &tool,
            json!({"op":"write","path":"vfs:///Mixed %Case.txt","content":contents}),
        )
        .await;
        assert_eq!(written["path"], "local://Mixed%20%25Case.txt");
        let copied = call(
            &tool,
            json!({"op":"cp","src":written["uri"],"dst":"ht://Mixed%20%25Case.txt"}),
        )
        .await;
        assert_eq!(copied["src"], written["uri"]);
        assert_eq!(copied["dst"], "ht://Mixed%20%25Case.txt");
        for op in ["read", "cat", "head", "tail", "wc", "stat", "read_chunk"] {
            let result = call(&tool, json!({"op":op,"path":"ht:///Mixed%20%25Case.txt"})).await;
            assert_eq!(result["path"], copied["dst"], "{op}");
            assert_eq!(result["domain"], "ht");
            if op == "cat" {
                assert_eq!(result["content"], contents.trim_end_matches('\n'));
            }
        }
        let listing = call(&tool, json!({"op":"ls","path":"ht://"})).await;
        let entry = listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "Mixed %Case.txt")
            .unwrap();
        assert_eq!(entry["path"], copied["dst"]);
        for (op, pattern) in [("grep", "needle"), ("glob", "*.txt")] {
            let result = call(&tool, json!({"op":op,"path":"ht://","pattern":pattern,"output_mode":"files_with_matches"})).await;
            assert_eq!(result["matches"], copied["dst"]);
        }
        let tree = call(&tool, json!({"op":"tree","path":"ht://"})).await;
        assert!(tree["content"].as_str().unwrap().starts_with("ht://\n"));
        let moved = call(
            &tool,
            json!({"op":"mv","src":copied["dst"],"dst":"local://back.txt"}),
        )
        .await;
        assert_eq!(moved["dst"], "local://back.txt");
        let patched = call(&tool, json!({"op":"patch","patch":"*** Begin Patch\n*** Add File: ht://patched.txt\n+patched\n*** End Patch"})).await;
        assert_eq!(patched["changes"][0]["path"], "ht://patched.txt");
        let removed = call(&tool, json!({"op":"rm","path":"/data/ht/patched.txt"})).await;
        assert_eq!(removed["path"], "ht://patched.txt");
        for path in [
            "missing://x",
            "ht://../escape",
            "local://data/ht/escape",
            "ht://readonly/refused.txt",
        ] {
            let result = tool
                .run(
                    serde_json::from_value(json!({"op":"write","path":path,"content":"refused"}))
                        .unwrap(),
                )
                .await;
            assert!(
                result.is_err() || result.unwrap().is_error == Some(true),
                "{path}"
            );
        }
        assert!(!remote.path().join("readonly/refused.txt").exists());
    }

    #[test]
    fn publishing_does_not_rewrite_contents_or_consistency_tokens() {
        let storage = OpendalFileStorage::new_temp();
        let payload = json!({"path":"/x","content":{"path":"/untouched"},"source_version":"vfs:///unchanged","next_offset":17});
        let result = publish_result(&storage, ToolResult::success_json(payload)).unwrap();
        let ToolResultContent::Json(value) = result.content else {
            panic!("expected JSON")
        };
        assert_eq!(value["path"], "local://x");
        assert_eq!(value["content"]["path"], "/untouched");
        assert_eq!(value["source_version"], "vfs:///unchanged");
        assert_eq!(value["next_offset"], 17);
    }
}

#[cfg(test)]
mod schema_tests {
    #[test]
    fn registered_tool_teaches_canonical_domain_addresses() {
        let storage = std::sync::Arc::new(crate::OpendalFileStorage::new_temp());
        let registration = crate::vbash_registrations(storage).remove(0);
        assert!(
            registration
                .definition
                .description
                .contains("domain://path")
        );
        let schema = serde_json::to_value(&registration.definition.input_schema).unwrap();
        for field in ["path", "src", "dst"] {
            assert!(
                schema["properties"][field]["description"]
                    .as_str()
                    .unwrap()
                    .contains("domain://path")
            );
        }
    }
}

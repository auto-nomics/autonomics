//! Node execution fingerprints and content-level cache invalidation.
//!
//! A fingerprint is a blake3 digest over *everything that determines a node's
//! output*: its kind, canonical spec, engine version, source revision, the
//! identity of every input value it consumes, and — for plugin-backed
//! nodes — the identity of the plugin implementation that executes (see
//! [`PluginIdentity`]). Following the Nextflow task-hash model, one digest
//! simultaneously serves as provenance evidence ("this output came from
//! exactly these inputs"), a cache key (equal fingerprint ⇒ equal result),
//! and an invalidation check (see [`cached_file_changed`]).
//!
//! Input identity is value-shaped:
//! - file-like values contribute their path plus a content hash when one is
//!   recorded (any dialect — `sha256:{hex}`, bare hex, etag — is used
//!   verbatim so the dialect stays auditable), degrading to an explicitly
//!   marked `meta:` (size + mtime) identity when no hash exists;
//! - `DataFrame` values have no address and no serialization (they are lazy
//!   logical plans), so they contribute the **executing upstream node's
//!   fingerprint** — a chain that propagates content changes through the
//!   whole graph automatically.
//!
//! Encoding note: every constituent is joined with NUL separators and each
//! carries a prefix tag, so no field can be confused with a concatenation
//! boundary (paths cannot contain NUL).

use blake3::Hasher;
use datafusion::common::HashMap;
use sha2::{Digest, Sha256};

use crate::dag::NodeId;
use crate::dag::graph::{EdgeLabel, PortOutputs};
use crate::value::{FileFingerprint, FileRef, NodeValue};

/// Domain separator for the fingerprint hash, so digests from different
/// schemes or versions can never collide.
///
/// v3 (F15 ⊕ WO-R09 merge) adds **both** the source revision and the
/// plugin-identity constituent: v1 covered only kind + spec + engine
/// version + input identities, so an implementation change with an
/// unchanged spec — engine rebuild or plugin family script/manifest/image
/// swap — kept the fingerprint, and its cache, stale. Every node's
/// fingerprint changes once when an engine built with v3 first recomputes
/// it, plugin-backed or not (the absent-plugin marker is itself a
/// constituent). v1/v2 digests are not compatible by design.
pub const FINGERPRINT_DOMAIN: &str = "autonomics-node-fingerprint-v3";
/// Marker written into the digest when a node has no retained spec (raw
/// `add_node` path, tests only) — auditable degradation, never silent.
pub const NOSPEC_TAG: &str = "nospec";

/// Marker written into the digest when a node's behavior is defined entirely
/// by its kind + spec (built-in factories; no plugin family underneath).
/// Explicit, so "no plugin" can never be confused with a plugin identity
/// that happens to encode to empty bytes.
pub const NOPLUGIN_TAG: &str = "noplg";

/// Marker for a plugin node that runs no script (argv-only command).
pub const NOSCRIPT_TAG: &str = "noscript";

/// Marker for a mounted catalog panel whose resolved bundle carries no
/// content digest (legacy / built-in bundles) — visible degradation, never a
/// silently unchecked mount.
pub const NODIGEST_TAG: &str = "nodigest";

// ── input identities ──────────────────────────────────────────────────────────

/// One resolved upstream input, expressed in the shape that matters for
/// identity. Built by [`collect_input_identities`] at dispatch time.
#[derive(Debug, Clone)]
pub struct InputIdentity {
    /// Id of the upstream node that produced the value (also part of the
    /// canonical sort key).
    pub from: String,
    /// Output port on the upstream node.
    pub from_port: u8,
    /// Input port on the consuming node.
    pub to_port: u8,
    /// The value's identity payload.
    pub value: IdentityValue,
}

/// Value-shaped identity payload.
#[derive(Debug, Clone)]
pub enum IdentityValue {
    File {
        path: String,
        fingerprint: Option<FileFingerprint>,
    },
    /// One entry per file, in order.
    FileSet(Vec<(String, Option<FileFingerprint>)>),
    /// Chain identity: the upstream node's execution fingerprint. `None`
    /// when the upstream fingerprint is unavailable (first incremental run
    /// after an upgrade, spec-less test nodes) — encoded as an explicit
    /// `pending` marker, stable but visibly not content-level.
    DataFrame { upstream: Option<String> },
    /// Canonical JSON content of all channel items, preserving order.
    Channel(Vec<serde_json::Value>),
}

/// Gather a node's input identities from the wiring edges, mirroring
/// [`crate::dag::utils::build_inputs`] iteration (including the "upstream
/// output not yet produced / port absent → skip" semantics) so the recorded
/// set matches exactly what was injected.
pub fn collect_input_identities(
    id: &str,
    incoming: &HashMap<NodeId, Vec<(NodeId, EdgeLabel)>>,
    outputs: &HashMap<NodeId, PortOutputs>,
    fingerprints: &HashMap<NodeId, String>,
) -> Vec<InputIdentity> {
    let mut identities = Vec::new();
    let Some(edges) = incoming.get(id) else {
        return identities;
    };
    for (from, edge) in edges {
        let Some(pred_outputs) = outputs.get(from) else {
            continue;
        };
        let Some(value) = pred_outputs.get(&edge.from_port) else {
            continue;
        };
        let value_kind = match value {
            NodeValue::File(file) => IdentityValue::File {
                path: file.path.clone(),
                fingerprint: file.fingerprint.clone(),
            },
            NodeValue::FileSet(files) => IdentityValue::FileSet(
                files
                    .iter()
                    .map(|file| (file.path.clone(), file.fingerprint.clone()))
                    .collect(),
            ),
            NodeValue::DataFrame(_) => IdentityValue::DataFrame {
                upstream: fingerprints.get(from).cloned(),
            },
            NodeValue::Channel(channel) => IdentityValue::Channel(channel.items.clone()),
        };
        identities.push(InputIdentity {
            from: from.clone(),
            from_port: edge.from_port,
            to_port: edge.to_port,
            value: value_kind,
        });
    }
    identities
}

// ── plugin identity ───────────────────────────────────────────────────────────

/// The identity of the plugin implementation a node executes (WO-R09).
///
/// A plugin-backed node's behavior is determined not only by its kind and
/// spec but by the plugin family loaded next to the engine: the manifest
/// that declared it, the exact script it stages into the container, and the
/// image the container runs. None of those appear in the node spec, so
/// without this constituent a plugin swap under an unchanged kind/spec kept
/// the old fingerprint — and its cached outputs — forever.
///
/// Built by the plugin loader from the *loaded* definition (not re-read from
/// disk at dispatch time), so the fingerprint always reflects what the
/// engine would actually execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginIdentity {
    /// `sha256:{hex}` over the family `manifest.toml` bytes exactly as
    /// loaded — every declaration (node definitions, params, panels, image
    /// metadata) participates at the byte level.
    pub manifest_sha256: String,
    /// `sha256:{hex}` over the exact script source the node stages into the
    /// container. For `script_file` nodes this is the referenced file's
    /// content (inlined by the loader before validation); for inline
    /// `script` nodes the manifest hash already covers it and this is the
    /// same bytes hashed uniformly. `None` for argv-only commands.
    pub script_sha256: Option<String>,
    /// Full image reference string (`host/path@sha256:…`) from the
    /// manifest's `[image]` table — pinned by digest, so any digest change
    /// is a different execution environment.
    pub image_reference: String,
    /// Catalog panels mounted for this node. The declaration (binding,
    /// mount, bundle id) is covered by the manifest hash; the `digest` pins
    /// *which content* the catalog resolved, when it is known.
    pub panels: Vec<PanelIdentity>,
}

/// One mounted catalog panel, in the shape that matters for identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelIdentity {
    /// Local binding slot the node mounts through.
    pub binding: String,
    /// Stable bundle identifier in the runtime `BundleRegistry`.
    pub bundle_id: String,
    /// Immutable catalog content digest of the resolved bundle, when known.
    pub digest: Option<String>,
}

// ── fingerprint computation ───────────────────────────────────────────────────

/// Compute a node's execution fingerprint, binding the build's
/// [`crate::source_revision()`] into the digest.
///
/// ```text
/// blake3( FINGERPRINT_DOMAIN ‖ engine_version ‖ rev ‖ kind ‖ spec ‖ plugin ‖ Σ sorted identities )
/// ```
///
/// * `spec = None` marks the node as spec-less (see [`NOSPEC_TAG`]).
/// * `plugin = None` marks the node as plugin-less (see [`NOPLUGIN_TAG`]);
///   `Some` feeds the full [`PluginIdentity`] (WO-R09).
/// * identities are canonically ordered by `(to_port, from, from_port)`.
/// * the **source revision** participates: `engine_version` is the crate
///   version, which stays flat across many commits, so without the
///   revision an implementation change with an unchanged spec and
///   unchanged inputs would keep hitting the same fingerprint and reuse
///   stale incremental cache entries (audit F15). The revision is the
///   short git commit by default; builds can stamp an explicit identity
///   via the `AUTONOMICS_SOURCE_REVISION` env override at build time.
///
/// Canonical JSON relies on serde_json's sorted-key object representation
/// (the workspace does not enable `preserve_order`) — the same premise as
/// [`crate::dag::DagManifest::content_hash`], pinned by a test below.
pub fn compute_node_fingerprint(
    kind: &str,
    spec: Option<&serde_json::Value>,
    engine_version: &str,
    identities: &[InputIdentity],
    plugin: Option<&PluginIdentity>,
) -> String {
    compute_node_fingerprint_with_revision(
        kind,
        spec,
        engine_version,
        crate::source_revision(),
        identities,
        plugin,
    )
}

/// Revision-explicit form of [`compute_node_fingerprint`]. The revision is
/// injectable so tests can prove two builds from different source
/// revisions produce different fingerprints (and the same revision
/// reproduces the same one) without rebuilding the crate.
pub fn compute_node_fingerprint_with_revision(
    kind: &str,
    spec: Option<&serde_json::Value>,
    engine_version: &str,
    source_revision: &str,
    identities: &[InputIdentity],
    plugin: Option<&PluginIdentity>,
) -> String {
    let mut ordered: Vec<&InputIdentity> = identities.iter().collect();
    ordered
        .sort_by(|a, b| (a.to_port, &a.from, a.from_port).cmp(&(b.to_port, &b.from, b.from_port)));

    let mut hasher = Hasher::new();
    let mut feed = |bytes: &[u8]| {
        hasher.update(bytes);
    };
    feed(FINGERPRINT_DOMAIN.as_bytes());
    feed(&[0]);
    feed(engine_version.as_bytes());
    feed(&[0]);
    feed(b"rev");
    feed(&[0]);
    feed(source_revision.as_bytes());
    feed(&[0]);
    feed(kind.as_bytes());
    feed(&[0]);
    match spec {
        Some(spec) => {
            feed(b"spec");
            // serde_json::to_vec cannot fail for a Value.
            feed(&serde_json::to_vec(spec).unwrap_or_default());
        }
        None => feed(NOSPEC_TAG.as_bytes()),
    }
    feed(&[0]);
    match plugin {
        Some(identity) => encode_plugin_identity(&mut feed, identity),
        None => feed(NOPLUGIN_TAG.as_bytes()),
    }
    for identity in ordered {
        feed(&[0]);
        feed(&identity.to_port.to_be_bytes());
        feed(&[0]);
        feed(identity.from.as_bytes());
        feed(&[0]);
        feed(&identity.from_port.to_be_bytes());
        feed(&[0]);
        encode_value(&mut feed, identity);
    }
    hasher.finalize().to_hex().to_string()
}

/// Fingerprint contribution of a node's plugin implementation. Every
/// constituent is NUL-separated and prefixed so no field can bleed into its
/// neighbour; panels are canonically ordered by binding so declaration order
/// in the manifest never changes the digest.
fn encode_plugin_identity(feed: &mut dyn FnMut(&[u8]), identity: &PluginIdentity) {
    feed(b"plugin");
    feed(&[0]);
    feed(identity.manifest_sha256.as_bytes());
    feed(&[0]);
    match &identity.script_sha256 {
        Some(hash) => feed(hash.as_bytes()),
        None => feed(NOSCRIPT_TAG.as_bytes()),
    }
    feed(&[0]);
    feed(identity.image_reference.as_bytes());
    let mut panels: Vec<&PanelIdentity> = identity.panels.iter().collect();
    panels.sort_by(|a, b| a.binding.cmp(&b.binding));
    for panel in panels {
        feed(&[0]);
        feed(b"panel");
        feed(&[0]);
        feed(panel.binding.as_bytes());
        feed(&[0]);
        feed(panel.bundle_id.as_bytes());
        feed(&[0]);
        match &panel.digest {
            Some(digest) => feed(digest.as_bytes()),
            None => feed(NODIGEST_TAG.as_bytes()),
        }
    }
}

fn encode_value(feed: &mut dyn FnMut(&[u8]), identity: &InputIdentity) {
    match &identity.value {
        IdentityValue::File { path, fingerprint } => {
            feed(b"file:");
            feed(path.as_bytes());
            feed(&[0]);
            encode_fingerprint(feed, fingerprint.as_ref());
        }
        IdentityValue::FileSet(entries) => {
            feed(b"fileset:");
            feed(entries.len().to_string().as_bytes());
            for (path, fingerprint) in entries {
                feed(&[0]);
                feed(path.as_bytes());
                feed(&[0]);
                encode_fingerprint(feed, fingerprint.as_ref());
            }
        }
        IdentityValue::DataFrame { upstream } => {
            feed(b"df:");
            match upstream {
                Some(fingerprint) => feed(fingerprint.as_bytes()),
                None => {
                    feed(format!("pending:{}:{}", identity.from, identity.from_port).as_bytes())
                }
            }
        }
        IdentityValue::Channel(items) => {
            feed(b"channel:");
            for item in items {
                feed(&serde_json::to_vec(item).unwrap_or_default());
                feed(&[0]);
            }
        }
    }
}

/// Fingerprint contribution of one file-like value. Content hashes are used
/// verbatim (dialect stays visible: `sha256:{hex}`, bare hex, etag); a
/// recorded fingerprint without a hash degrades to an explicit `meta:`
/// identity; a missing fingerprint degrades to `nofp`.
fn encode_fingerprint(feed: &mut dyn FnMut(&[u8]), fingerprint: Option<&FileFingerprint>) {
    match fingerprint {
        Some(fp) => match &fp.content_hash {
            Some(hash) => {
                feed(b"hash:");
                feed(hash.as_bytes());
            }
            None => {
                feed(format!("meta:{}:{}", fp.size, fp.mtime_ns).as_bytes());
            }
        },
        None => feed(b"nofp"),
    }
}

// ── content hashing (Content mode + invalidation confirm) ─────────────────────

/// Best-effort sha256 over a file-like path, resolved through the object
/// store when possible. Returns the bare lowercase hex digest.
///
/// Resolution merges the two historical precedents: `vfs://` URIs go straight
/// to the store; bare absolute / `file://` paths go to the store **only when
/// mounted** (mirroring `file_to_dataframe::source_path`), otherwise the host
/// filesystem is read directly with `tokio::fs`.
pub async fn file_sha256(
    path: &str,
    storage: Option<&vfs::OpendalFileStorage>,
) -> Option<(u64, String)> {
    if let Some(vpath) = path.strip_prefix("vfs://") {
        let storage = storage?;
        return storage
            .sha256(&vfs::OpendalFileStorage::normalize_path(vpath))
            .await
            .ok();
    }
    let candidate = path.strip_prefix("file://").unwrap_or(path);
    if candidate.starts_with('/') {
        let normalized = vfs::OpendalFileStorage::normalize_path(candidate);
        if let Some(storage) = storage
            && storage.is_mounted(&normalized)
        {
            return storage.sha256(&normalized).await.ok();
        }
        return host_sha256(candidate).await;
    }
    // Relative path: host filesystem only.
    host_sha256(path).await
}

async fn host_sha256(path: &str) -> Option<(u64, String)> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path).await.ok()?;
    let metadata = file.metadata().await.ok()?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).await.ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Some((metadata.len(), hex_lower(&hasher.finalize())))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Fill in content hashes for file-like identities that lack one
/// (`SchedulerConfig::input_hashing = Content`). Best-effort: entries whose
/// path cannot be read keep their `meta:`/`nofp` degradation.
///
/// The computed hash uses the `sha256:{hex}` dialect. When the entry had no
/// fingerprint at all, the size/mtime fields are left zeroed — once a content
/// hash is present the identity encoding no longer consults them.
pub async fn upgrade_identities_with_content_hashes(
    identities: &mut [InputIdentity],
    storage: Option<&vfs::OpendalFileStorage>,
) {
    for identity in identities.iter_mut() {
        match &mut identity.value {
            IdentityValue::File { path, fingerprint } => {
                upgrade_one(path, fingerprint, storage).await;
            }
            IdentityValue::FileSet(entries) => {
                for (path, fingerprint) in entries.iter_mut() {
                    upgrade_one(path, fingerprint, storage).await;
                }
            }
            IdentityValue::DataFrame { .. } => {}
            IdentityValue::Channel { .. } => {}
        }
    }
}

async fn upgrade_one(
    path: &str,
    fingerprint: &mut Option<FileFingerprint>,
    storage: Option<&vfs::OpendalFileStorage>,
) {
    if fingerprint
        .as_ref()
        .is_some_and(|fp| fp.content_hash.is_some())
    {
        return;
    }
    if let Some((size, hex)) = file_sha256(path, storage).await {
        let slot = fingerprint.get_or_insert_with(|| FileFingerprint {
            size: 0,
            mtime_ns: 0,
            content_hash: None,
            immutable_remote: false,
        });
        if slot.size == 0 {
            slot.size = size;
        }
        slot.content_hash = Some(format!("sha256:{hex}"));
    }
}

// ── content-level invalidation ────────────────────────────────────────────────

/// Whether a cached file output has changed since `file.fingerprint` was
/// recorded. Three-stage judgement, in cost order:
///
/// 1. **Immutable remote**: published objects carry
///    [`FileFingerprint::immutable_remote`] and a content hash — clean
///    without touching the filesystem (also covers bare virtual paths that
///    lack the `vfs://` prefix, which the old path-spelling check missed).
/// 2. **Metadata match**: local stat succeeds and size + mtime match —
///    clean, zero extra I/O.
/// 3. **Content confirm**: metadata differs (or the stat failed) but the
///    recorded hash is in the `sha256:` dialect — recompute the digest and
///    compare, **ignoring mtime**: a touched-but-unchanged file is clean.
///    Only files that were already about to be judged stale pay for a read.
/// 4. Anything else — stale.
///
/// A file with no recorded fingerprint never invalidates (unchanged
/// historical behaviour).
pub async fn cached_file_changed(
    file: &FileRef,
    storage: Option<&vfs::OpendalFileStorage>,
) -> bool {
    let Some(expected) = file.fingerprint.as_ref() else {
        return false;
    };

    // Stage 1: identity declared immutable at publish time.
    if expected.immutable_remote && expected.content_hash.is_some() {
        return false;
    }

    // Stage 2: local metadata match.
    if let Some(current) = FileFingerprint::from_path(&file.path)
        && current.size == expected.size
        && current.mtime_ns == expected.mtime_ns
    {
        return false;
    }

    // Stage 3: content confirmation for the recomputable dialect. Note the
    // comparison is against `from_path`-style expectations too: a recorded
    // hash makes the mtime field untrustworthy (publishers of hashed local
    // files used to be perpetually stale for exactly this reason).
    if let Some(hash) = expected
        .content_hash
        .as_deref()
        .filter(|hash| hash.starts_with("sha256:"))
    {
        let expected_hex = hash.strip_prefix("sha256:").unwrap_or(hash);
        if let Some((_, actual_hex)) = file_sha256(&file.path, storage).await
            && actual_hex.eq_ignore_ascii_case(expected_hex)
        {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_identity(path: &str, fingerprint: Option<FileFingerprint>) -> InputIdentity {
        InputIdentity {
            from: "upstream".to_string(),
            from_port: 0,
            to_port: 0,
            value: IdentityValue::File {
                path: path.to_string(),
                fingerprint,
            },
        }
    }

    fn fp(size: u64, mtime_ns: i128, content_hash: Option<&str>) -> FileFingerprint {
        FileFingerprint {
            size,
            mtime_ns,
            content_hash: content_hash.map(str::to_string),
            immutable_remote: false,
        }
    }

    #[test]
    fn same_inputs_produce_same_fingerprint() {
        let identities = vec![file_identity(
            "/data/x.csv",
            Some(fp(10, 1234, Some("sha256:deadbeef"))),
        )];
        let a = compute_node_fingerprint(
            "sql",
            Some(&serde_json::json!({"q": 1})),
            "v1",
            &identities,
            None,
        );
        let b = compute_node_fingerprint(
            "sql",
            Some(&serde_json::json!({"q": 1})),
            "v1",
            &identities,
            None,
        );
        assert_eq!(a, b);
        assert_eq!(a.len(), 64, "blake3 hex");
    }

    #[test]
    fn fingerprint_reacts_to_every_constituent() {
        let base = vec![file_identity(
            "/data/x.csv",
            Some(fp(10, 1234, Some("sha256:deadbeef"))),
        )];
        let reference = compute_node_fingerprint_with_revision(
            "sql",
            Some(&serde_json::json!({"q": 1})),
            "v1",
            "aaaaaaaaaaaa",
            &base,
            None,
        );
        let changed_kind = compute_node_fingerprint_with_revision(
            "other_kind",
            Some(&serde_json::json!({"q": 1})),
            "v1",
            "aaaaaaaaaaaa",
            &base,
            None,
        );
        let changed_spec = compute_node_fingerprint_with_revision(
            "sql",
            Some(&serde_json::json!({"q": 2})),
            "v1",
            "aaaaaaaaaaaa",
            &base,
            None,
        );
        let changed_engine = compute_node_fingerprint_with_revision(
            "sql",
            Some(&serde_json::json!({"q": 1})),
            "v2",
            "aaaaaaaaaaaa",
            &base,
            None,
        );
        let changed_revision = compute_node_fingerprint_with_revision(
            "sql",
            Some(&serde_json::json!({"q": 1})),
            "v1",
            "bbbbbbbbbbbb",
            &base,
            None,
        );
        let changed_hash = {
            let identities = vec![file_identity(
                "/data/x.csv",
                Some(fp(10, 1234, Some("sha256:feedface"))),
            )];
            compute_node_fingerprint_with_revision(
                "sql",
                Some(&serde_json::json!({"q": 1})),
                "v1",
                "aaaaaaaaaaaa",
                &identities,
                None,
            )
        };
        let nospec =
            compute_node_fingerprint_with_revision("sql", None, "v1", "aaaaaaaaaaaa", &base, None);
        for candidate in [
            changed_kind,
            changed_spec,
            changed_engine,
            changed_revision,
            changed_hash,
            nospec,
        ] {
            assert_ne!(reference, candidate);
        }
    }

    #[test]
    fn source_revision_participates_in_the_fingerprint() {
        // F15: the engine version is the crate version and stays flat
        // across commits; the revision is what distinguishes two builds
        // with identical specs and inputs. Same revision → same digest;
        // different revision → different digest (no silent incremental
        // cache reuse across an engine change).
        let identities = vec![file_identity(
            "/data/x.csv",
            Some(fp(10, 1234, Some("sha256:deadbeef"))),
        )];
        let a = compute_node_fingerprint_with_revision(
            "sql",
            Some(&serde_json::json!({"q": 1})),
            "v1",
            "aaaaaaaaaaaa",
            &identities,
            None,
        );
        let a_again = compute_node_fingerprint_with_revision(
            "sql",
            Some(&serde_json::json!({"q": 1})),
            "v1",
            "aaaaaaaaaaaa",
            &identities,
            None,
        );
        let b = compute_node_fingerprint_with_revision(
            "sql",
            Some(&serde_json::json!({"q": 1})),
            "v1",
            "bbbbbbbbbbbb",
            &identities,
            None,
        );
        assert_eq!(a, a_again);
        assert_ne!(a, b);
    }

    #[test]
    fn embedded_source_revision_is_available() {
        // build.rs guarantees a non-empty identity: the env override, the
        // git short revision, or the literal "unknown" — never "".
        assert!(!crate::source_revision().is_empty());
    }

    #[test]
    fn spec_key_order_does_not_change_the_fingerprint() {
        // Pins the canonical-JSON premise: without `preserve_order`,
        // serde_json maps are key-sorted, so insertion order is irrelevant.
        let a: serde_json::Value =
            serde_json::from_str(r#"{"alpha": 1, "beta": {"z": 2, "a": 3}}"#).unwrap();
        let b: serde_json::Value =
            serde_json::from_str(r#"{"beta": {"a": 3, "z": 2}, "alpha": 1}"#).unwrap();
        assert_eq!(
            compute_node_fingerprint("k", Some(&a), "v", &[], None),
            compute_node_fingerprint("k", Some(&b), "v", &[], None),
        );
    }

    #[test]
    fn identity_sorting_is_canonical_regardless_of_edge_order() {
        let first = InputIdentity {
            from: "a".to_string(),
            from_port: 0,
            to_port: 1,
            value: IdentityValue::DataFrame {
                upstream: Some("fpa".to_string()),
            },
        };
        let second = InputIdentity {
            from: "b".to_string(),
            from_port: 0,
            to_port: 0,
            value: IdentityValue::DataFrame {
                upstream: Some("fpb".to_string()),
            },
        };
        assert_eq!(
            compute_node_fingerprint("k", None, "v", &[first.clone(), second.clone()], None),
            compute_node_fingerprint("k", None, "v", &[second, first], None),
        );
    }

    #[test]
    fn dialects_and_degradations_are_distinguishable() {
        let hash_sha = compute_node_fingerprint(
            "k",
            None,
            "v",
            &[file_identity(
                "/f",
                Some(fp(10, 1, Some("sha256:deadbeef"))),
            )],
            None,
        );
        let hash_bare = compute_node_fingerprint(
            "k",
            None,
            "v",
            &[file_identity("/f", Some(fp(10, 1, Some("deadbeef"))))],
            None,
        );
        let hash_etag = compute_node_fingerprint(
            "k",
            None,
            "v",
            &[file_identity("/f", Some(fp(10, 1, Some("\"etag1\""))))],
            None,
        );
        let meta = compute_node_fingerprint(
            "k",
            None,
            "v",
            &[file_identity("/f", Some(fp(10, 1, None)))],
            None,
        );
        let meta_other_mtime = compute_node_fingerprint(
            "k",
            None,
            "v",
            &[file_identity("/f", Some(fp(10, 2, None)))],
            None,
        );
        let nofp = compute_node_fingerprint("k", None, "v", &[file_identity("/f", None)], None);

        let all = [hash_sha, hash_bare, hash_etag, meta, meta_other_mtime, nofp];
        for (i, a) in all.iter().enumerate() {
            for b in all.iter().skip(i + 1) {
                assert_ne!(a, b, "identity encodings must be pairwise distinct");
            }
        }
    }

    #[test]
    fn dataframe_identity_follows_the_upstream_fingerprint() {
        let upstream = |fingerprint: Option<&str>| InputIdentity {
            from: "src".to_string(),
            from_port: 0,
            to_port: 0,
            value: IdentityValue::DataFrame {
                upstream: fingerprint.map(str::to_string),
            },
        };
        let a = compute_node_fingerprint("k", None, "v", &[upstream(Some("fp1"))], None);
        let b = compute_node_fingerprint("k", None, "v", &[upstream(Some("fp2"))], None);
        let pending = compute_node_fingerprint("k", None, "v", &[upstream(None)], None);
        assert_ne!(a, b, "upstream fingerprint change must propagate");
        assert_ne!(a, pending);
        assert_ne!(b, pending);
    }

    // ── plugin identity (WO-R09) ─────────────────────────────────────────

    fn plugin_identity() -> PluginIdentity {
        PluginIdentity {
            manifest_sha256: "sha256:aaaa".to_string(),
            script_sha256: Some("sha256:bbbb".to_string()),
            image_reference: "ghcr.io/auto-nomics/autonomics/ldsc@sha256:1".to_string(),
            panels: vec![PanelIdentity {
                binding: "ref_ld".to_string(),
                bundle_id: "owner/ref-ld".to_string(),
                digest: Some("sha256:panel1".to_string()),
            }],
        }
    }

    /// Same plugin identity, same everything: two computations must agree
    /// (the "pure data re-run leaves the fingerprint unchanged" acceptance).
    #[test]
    fn plugin_identity_fingerprint_is_stable() {
        let identities = vec![file_identity(
            "/data/x.csv",
            Some(fp(10, 1234, Some("sha256:deadbeef"))),
        )];
        let plugin = plugin_identity();
        let a = compute_node_fingerprint(
            "ldsc_h2",
            Some(&serde_json::json!({"intercept": 1.0})),
            "v1",
            &identities,
            Some(&plugin),
        );
        let b = compute_node_fingerprint(
            "ldsc_h2",
            Some(&serde_json::json!({"intercept": 1.0})),
            "v1",
            &identities,
            Some(&plugin),
        );
        assert_eq!(a, b);
    }

    /// Every plugin-identity constituent must move the digest: manifest
    /// bytes, script source, image reference, panel digest — and a plugin
    /// identity must never collide with the plugin-less marker.
    #[test]
    fn plugin_identity_reacts_to_every_constituent() {
        let base = vec![file_identity(
            "/data/x.csv",
            Some(fp(10, 1234, Some("sha256:deadbeef"))),
        )];
        let plugin = plugin_identity();
        let reference = compute_node_fingerprint("k", None, "v", &base, Some(&plugin));

        let changed_manifest = {
            let mut other = plugin.clone();
            other.manifest_sha256 = "sha256:aaab".to_string();
            compute_node_fingerprint("k", None, "v", &base, Some(&other))
        };
        let changed_script = {
            let mut other = plugin.clone();
            other.script_sha256 = Some("sha256:bbbc".to_string());
            compute_node_fingerprint("k", None, "v", &base, Some(&other))
        };
        let script_absent_vs_present = {
            let mut other = plugin.clone();
            other.script_sha256 = None;
            compute_node_fingerprint("k", None, "v", &base, Some(&other))
        };
        let changed_image = {
            let mut other = plugin.clone();
            other.image_reference = "ghcr.io/auto-nomics/autonomics/ldsc@sha256:2".to_string();
            compute_node_fingerprint("k", None, "v", &base, Some(&other))
        };
        let changed_panel_digest = {
            let mut other = plugin.clone();
            other.panels[0].digest = Some("sha256:panel2".to_string());
            compute_node_fingerprint("k", None, "v", &base, Some(&other))
        };
        let panel_digest_absent = {
            let mut other = plugin.clone();
            other.panels[0].digest = None;
            compute_node_fingerprint("k", None, "v", &base, Some(&other))
        };
        let no_plugin = compute_node_fingerprint("k", None, "v", &base, None);

        for candidate in [
            changed_manifest,
            changed_script,
            script_absent_vs_present,
            changed_image,
            changed_panel_digest,
            panel_digest_absent,
            no_plugin,
        ] {
            assert_ne!(reference, candidate);
        }
    }

    /// Panel declaration order in the manifest must not leak into the
    /// digest: the same mounts in a different order are the same execution.
    #[test]
    fn plugin_panel_order_is_canonical() {
        let mut panels = vec![
            PanelIdentity {
                binding: "b".to_string(),
                bundle_id: "owner/b".to_string(),
                digest: None,
            },
            PanelIdentity {
                binding: "a".to_string(),
                bundle_id: "owner/a".to_string(),
                digest: None,
            },
        ];
        let plugin = PluginIdentity {
            manifest_sha256: "sha256:aaaa".to_string(),
            script_sha256: None,
            image_reference: "reg/p@sha256:1".to_string(),
            panels: panels.clone(),
        };
        let first = compute_node_fingerprint("k", None, "v", &[], Some(&plugin));
        panels.reverse();
        let plugin = PluginIdentity {
            manifest_sha256: "sha256:aaaa".to_string(),
            script_sha256: None,
            image_reference: "reg/p@sha256:1".to_string(),
            panels,
        };
        let second = compute_node_fingerprint("k", None, "v", &[], Some(&plugin));
        assert_eq!(first, second);
    }

    // ── cached_file_changed ───────────────────────────────────────────────

    fn write_temp(dir: &tempfile::TempDir, name: &str, contents: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn file_ref(path: &std::path::Path, fingerprint: FileFingerprint) -> FileRef {
        FileRef {
            path: path.to_string_lossy().into_owned(),
            format: None,
            fingerprint: Some(fingerprint),
        }
    }

    #[tokio::test]
    async fn touched_but_unchanged_local_file_with_hash_is_clean() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_temp(&dir, "x.csv", b"stable content");

        let expected = FileFingerprint {
            // Simulate a recorded publish-time hash with a stale mtime.
            size: b"stable content".len() as u64,
            mtime_ns: 1,
            content_hash: Some(format!(
                "sha256:{}",
                hex_lower(&Sha256::digest(b"stable content"))
            )),
            immutable_remote: false,
        };
        assert!(!cached_file_changed(&file_ref(&path, expected), None).await);
    }

    #[tokio::test]
    async fn changed_content_with_hash_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_temp(&dir, "x.csv", b"new content");

        let expected = FileFingerprint {
            size: 0,
            mtime_ns: 1,
            content_hash: Some(format!(
                "sha256:{}",
                hex_lower(&Sha256::digest(b"old content"))
            )),
            immutable_remote: false,
        };
        assert!(cached_file_changed(&file_ref(&path, expected), None).await);
    }

    #[tokio::test]
    async fn mtime_change_without_hash_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_temp(&dir, "x.csv", b"same");
        let expected = FileFingerprint {
            size: b"same".len() as u64,
            mtime_ns: 1,
            content_hash: None,
            immutable_remote: false,
        };
        // The file on disk has a real (different) mtime than the record.
        assert!(cached_file_changed(&file_ref(&path, expected), None).await);
    }

    #[tokio::test]
    async fn immutable_remote_with_hash_is_clean_even_when_file_is_gone() {
        let file = FileRef {
            path: "/bundles/gone.parquet".to_string(),
            format: None,
            fingerprint: Some(FileFingerprint {
                size: 1,
                mtime_ns: 0,
                content_hash: Some("sha256:whatever".to_string()),
                immutable_remote: true,
            }),
        };
        assert!(!cached_file_changed(&file, None).await);
    }

    #[tokio::test]
    async fn matching_metadata_without_hash_is_clean() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_temp(&dir, "x.csv", b"same");
        let current = FileFingerprint::from_path(path.as_path()).unwrap();
        assert!(!cached_file_changed(&file_ref(&path, current), None).await);
    }

    #[tokio::test]
    async fn missing_fingerprint_never_invalidates() {
        let file = FileRef {
            path: "/does/not/exist".to_string(),
            format: None,
            fingerprint: None,
        };
        assert!(!cached_file_changed(&file, None).await);
    }

    // ── file_sha256 ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn host_path_sha256_matches_direct_digest() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_temp(&dir, "bin", &[7_u8; 1024 * 1024 + 3]); // multi-chunk
        let expected = hex_lower(&Sha256::digest(vec![7_u8; 1024 * 1024 + 3].as_slice()));
        let (size, hex) = file_sha256(&path.to_string_lossy(), None).await.unwrap();
        assert_eq!(hex, expected);
        assert_eq!(size, 1024 * 1024 + 3);
    }

    #[tokio::test]
    async fn vfs_path_sha256_goes_through_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let storage = vfs::OpendalFileStorage::new(dir.path());
        let contents = b"hello vfs";
        storage
            .write_bytes("/obj.bin", contents.to_vec())
            .await
            .unwrap();

        let (size, hex) = file_sha256("vfs:///obj.bin", Some(&storage)).await.unwrap();
        assert_eq!(hex, hex_lower(&Sha256::digest(contents)));
        assert_eq!(size, contents.len() as u64);
    }

    #[tokio::test]
    async fn upgrade_fills_missing_content_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_temp(&dir, "x.csv", b"payload");
        let mut identities = vec![file_identity(
            &path.to_string_lossy(),
            Some(FileFingerprint {
                size: 7,
                mtime_ns: 42,
                content_hash: None,
                immutable_remote: false,
            }),
        )];
        upgrade_identities_with_content_hashes(&mut identities, None).await;
        let IdentityValue::File { fingerprint, .. } = &identities[0].value else {
            panic!("expected file identity");
        };
        assert_eq!(
            fingerprint.as_ref().unwrap().content_hash.as_deref(),
            Some(format!("sha256:{}", hex_lower(&Sha256::digest(b"payload"))).as_str()),
        );
    }
}

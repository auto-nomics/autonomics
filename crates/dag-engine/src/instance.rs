//! Run-scoped item instances: identity, spec rendering, and fingerprints.
//!
//! An [`Instance`] is one materialized copy of a [`Process`](crate::model::Process)
//! for one item (or the single copy of a singleton). Instances never enter
//! authored state; they are created at run start and die with the run. Only
//! their fingerprints and outputs survive, keyed by the stable
//! [`InstanceId`], so incremental reuse works without polluting the graph.
//!
//! # Design provenance (studied against Nextflow's implementation)
//!
//! - **Fingerprints hash the raw spec template and the item value
//!   separately**, never the rendered spec — mirroring `TaskHasher`, which
//!   hashes the script source plus each input's `(name, value)` so a template
//!   reformat does not invalidate the cache while an input change does.
//! - **Item keys are content-derived and stable** (`process#key`). Nextflow
//!   names tasks `PROC (N)` by dispatch index and matches resume purely by
//!   content hash; we intentionally derive stable keys instead, because our
//!   instance result cache is indexed by them across runs.
//! - **No session UUID in the fingerprint.** Nextflow binds its cache to the
//!   session (resume reuses the run id); cross-run reuse is our core goal, so
//!   identity is engine version + source revision, matching dag-core.
//! - **No lazy/eager placeholder split.** Nextflow needs `${}` vs `!{}`
//!   because a task command is rendered twice (create, then after
//!   stage-in). We render exactly once at materialization.
//! - **Strict templates.** Only `{{item}}` / `{{item.field…}}` are special.
//!   Nextflow must escape literal `$` because authors hand-write Groovy
//!   strings; our specs are JSON authored programmatically, so any other
//!   `{{…}}` token is a hard error instead of an escape hatch.

use sha2::{Digest, Sha256};

use crate::model::{Error, Process};

/// Separator between process name and item key inside an instance id.
/// Reserved: process names containing it are rejected at validation
/// ([`crate::model::FlowGraph::validate`]).
pub const INSTANCE_ID_SEPARATOR: char = '#';

/// Stable identity of one instance: `"{process}#{item_key}"`, or
/// `"{process}#0"` for singletons.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct InstanceId(pub(crate) String);

impl InstanceId {
    /// Build an instance id from its parts. The process name is trusted here
    /// (graph validation already rejected `#` inside names); the item key is
    /// trusted to be separator-free because [`item_key`] derives it that way.
    pub fn new(process: &str, item_key: Option<&str>) -> Self {
        Self(match item_key {
            Some(key) => format!("{process}{INSTANCE_ID_SEPARATOR}{key}"),
            None => format!("{process}{INSTANCE_ID_SEPARATOR}0"),
        })
    }

    /// Split back into `(process name, item key)`. The singleton suffix `0`
    /// is returned as `None` — note this is lossy for a fanout item whose
    /// derived key is the literal string `0` (a bare string item `"0"`),
    /// which is acceptable because parsing exists for display and grouping,
    /// never for identity decisions.
    pub fn parts(&self) -> (&str, Option<&str>) {
        let (process, key) = self
            .0
            .rsplit_once(INSTANCE_ID_SEPARATOR)
            .expect("instance ids are constructed with a separator");
        match key {
            "0" => (process, None),
            other => (process, Some(other)),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for InstanceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Maximum retained length of a human-readable item key prefix.
const KEY_MAX_CHARS: usize = 64;
/// Length of the content-hash suffix used for structured items.
const KEY_HASH_CHARS: usize = 16;

/// Derive the stable item key of one item.
///
/// Rules, in order of the item's JSON type:
/// - strings: sanitized to `[A-Za-z0-9._-]` (anything else becomes `_`),
///   truncated to [`KEY_MAX_CHARS`]. Readability first: `qc#sample_A.fastq`
///   survives as itself.
/// - numbers / booleans / null: their literal JSON form.
/// - objects / arrays: keys would be unwieldy and collision-prone, so the key
///   is `hash-{sha256(canonical JSON)[..16]}`. Deterministic across runs;
///   distinct structured items hash to distinct keys except by SHA-256
///   collision.
///
/// Uniqueness *within one fanout set* is not checked here — the plan layer
/// owns the set and rejects duplicate keys (two distinct items deriving the
/// same key would silently merge instances otherwise).
pub fn item_key(item: &serde_json::Value) -> String {
    match item {
        serde_json::Value::String(text) => {
            let mut key = String::with_capacity(text.len());
            for character in text.chars().take(KEY_MAX_CHARS) {
                if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                    key.push(character);
                } else {
                    key.push('_');
                }
            }
            if key.is_empty() { "_".into() } else { key }
        }
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::Bool(flag) => flag.to_string(),
        serde_json::Value::Null => "null".into(),
        structured => {
            let bytes = serde_json::to_vec(structured).unwrap_or_default();
            let digest = Sha256::digest(&bytes);
            format!("hash-{}", hex_prefix(&digest, KEY_HASH_CHARS))
        }
    }
}

fn hex_prefix(digest: &[u8], chars: usize) -> String {
    digest
        .iter()
        .flat_map(|byte| [byte >> 4, byte & 0x0f])
        .map(|nibble| char::from_digit(nibble as u32, 16).expect("nibble"))
        .take(chars)
        .collect()
}

/// One materialized copy of a process for one item. Node construction (from
/// `kind` + the rendered spec, via the dag-core registry) is deliberately
/// **not** part of this type: the scheduler owns registry access and executor
/// dispatch; the instance carries only identity and the rendered contract.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Instance {
    pub id: InstanceId,
    /// The owning process's name (redundant with `id`, kept for cheap
    /// grouping without re-parsing).
    pub process: String,
    pub item_key: Option<String>,
    pub item: Option<serde_json::Value>,
    /// Spec with `{{item…}}` placeholders resolved against `item`.
    pub rendered_spec: serde_json::Value,
}

impl Instance {
    /// Materialize the single instance of a process with no driving input
    /// (derived singleton — see [`crate::plan`]).
    pub fn singleton(process: &Process) -> Result<Self, Error> {
        Ok(Self {
            id: InstanceId::new(&process.id, None),
            process: process.id.clone(),
            item_key: None,
            item: None,
            rendered_spec: process.spec.clone(),
        })
    }

    /// Materialize the pseudo-instance of one source item. Sources do not
    /// execute; the item IS the delivery. Rendered spec carries the raw
    /// item (never built into a node).
    pub fn for_item_source(source: &str, item: &serde_json::Value) -> Result<Self, Error> {
        let item_key = item_key(item);
        Ok(Self {
            id: InstanceId::new(source, Some(&item_key)),
            process: source.to_string(),
            item_key: Some(item_key),
            item: Some(item.clone()),
            rendered_spec: item.clone(),
        })
    }

    /// Materialize one instance of a process for `item` (the item that
    /// arrived on its driving channel, propagated from a source).
    pub fn for_item(process: &Process, item: &serde_json::Value) -> Result<Self, Error> {
        let item_key = item_key(item);
        Ok(Self {
            id: InstanceId::new(&process.id, Some(&item_key)),
            process: process.id.clone(),
            item_key: Some(item_key),
            item: Some(item.clone()),
            rendered_spec: render_spec(&process.spec, item)?,
        })
    }
}

/// Resolve `{{item}}` / `{{item.field…}}` placeholders in a spec against one
/// item.
///
/// Semantics (stricter than Nextflow's template engine, by design — see the
/// module docs):
/// - a string that is **exactly** one placeholder resolves to the pointed-to
///   JSON value with its type preserved (`"{{item}}"` over `42` renders to
///   the number `42`, not `"42"`);
/// - a placeholder embedded in a longer string stringifies the value
///   (objects/arrays as compact JSON);
/// - the pointer path supports dotted field access (`{{item.sample.id}}`)
///   and numeric array indices (`{{item.0}}`), mapped to JSON-pointer
///   segments;
/// - any other `{{…}}` token, or an unterminated `{{`, is a hard error —
///   there is no escape sequence.
pub fn render_spec(
    spec: &serde_json::Value,
    item: &serde_json::Value,
) -> Result<serde_json::Value, Error> {
    match spec {
        serde_json::Value::String(template) => render_string(template, item),
        serde_json::Value::Array(values) => values
            .iter()
            .map(|value| render_spec(value, item))
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        serde_json::Value::Object(fields) => {
            let mut rendered = serde_json::Map::new();
            for (name, value) in fields {
                rendered.insert(name.clone(), render_spec(value, item)?);
            }
            Ok(serde_json::Value::Object(rendered))
        }
        value => Ok(value.clone()),
    }
}

fn render_string(template: &str, item: &serde_json::Value) -> Result<serde_json::Value, Error> {
    let invalid =
        |reason: &str| Error::Invalid(format!("item template `{template}` is invalid: {reason}"));
    if let Some(value) = resolve_whole_token(template, item)? {
        return Ok(value);
    }
    let mut rendered = String::with_capacity(template.len());
    let mut cursor = 0usize;
    while let Some(start) = template[cursor..].find("{{") {
        let start = cursor + start;
        rendered.push_str(&template[cursor..start]);
        let body_start = start + 2;
        let Some(end) = template[body_start..].find("}}") else {
            return Err(invalid("missing `}}`"));
        };
        let end = body_start + end;
        let token = &template[body_start..end];
        let value = resolve_item_pointer(token, item)
            .ok_or_else(|| invalid(&format!("unsupported template token `{{{{{token}}}}}`")))?;
        rendered.push_str(&stringify(&value));
        cursor = end + 2;
    }
    rendered.push_str(&template[cursor..]);
    Ok(serde_json::Value::String(rendered))
}

/// If the template is exactly one placeholder, resolve it with the value's
/// JSON type preserved. Returns `Ok(None)` when the template is anything
/// else (including a template with several placeholders).
fn resolve_whole_token(
    template: &str,
    item: &serde_json::Value,
) -> Result<Option<serde_json::Value>, Error> {
    let Some(body) = template
        .strip_prefix("{{")
        .and_then(|rest| rest.strip_suffix("}}"))
    else {
        return Ok(None);
    };
    if body.contains("{{") || body.contains("}}") {
        return Ok(None);
    }
    match resolve_item_pointer(body, item) {
        Some(value) => Ok(Some(value)),
        None => Err(Error::Invalid(format!(
            "item template `{template}` uses an unsupported token"
        ))),
    }
}

/// Resolve one placeholder body against the item. `item` (empty body) is the
/// whole item; `item.a.b.0` walks the dotted pointer. Returns `None` when
/// the token is not an item reference (the caller decides whether that is
/// fatal). A well-formed item pointer that misses (e.g. `item.absent`)
/// resolves to JSON null, mirroring JSON-pointer semantics.
fn resolve_item_pointer(body: &str, item: &serde_json::Value) -> Option<serde_json::Value> {
    let path = body.strip_prefix("item")?;
    if path.is_empty() {
        return Some(item.clone());
    }
    let path = path.strip_prefix('.')?;
    let mut pointer = String::with_capacity(path.len() + 1);
    pointer.push('/');
    for character in path.chars() {
        if character == '.' {
            pointer.push('/');
        } else {
            pointer.push(character);
        }
    }
    Some(
        item.pointer(&pointer)
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    )
}

fn stringify(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Identity inputs to [`instance_fingerprint`]: one upstream binding that the
/// fingerprint must cover.
#[derive(Debug, Clone)]
pub struct UpstreamBinding {
    /// Input name (the wire's target port label at plan time) — hashed
    /// together with the value, mirroring `TaskHasher`'s per-input
    /// `(name, value)` pairs so renaming an input invalidates dependent
    /// instances.
    pub name: String,
    /// Fingerprint of the upstream instance (or, for a file value in later
    /// steps, its content identity).
    pub fingerprint: String,
}

/// Compute the stable fingerprint of one instance.
///
/// Hashed, in order (following `TaskHasher`'s key list shape):
/// engine version, source revision, process name, **raw spec template**
/// (pre-rendering), the item value, and each upstream binding's
/// `(name, fingerprint)`. The rendered spec is deliberately absent — it is a
/// pure function of the raw spec and the item, both already covered.
///
/// Consequences: a template reformat invalidates every instance of the
/// process (matches Nextflow hashing the script source); an item-set change
/// affects only the added/removed instances; an upstream change cascades.
pub fn instance_fingerprint(
    process: &Process,
    instance: &Instance,
    upstream: &[UpstreamBinding],
    engine_version: &str,
    source_revision: &str,
) -> String {
    instance_fingerprint_of(
        &process.id,
        &process.spec,
        instance,
        upstream,
        engine_version,
        source_revision,
    )
}

/// Parts-based form used by the scheduler, which holds the raw spec on the
/// process plan rather than a [`Process`] value.
pub fn instance_fingerprint_of(
    process_id: &str,
    raw_spec: &serde_json::Value,
    instance: &Instance,
    upstream: &[UpstreamBinding],
    engine_version: &str,
    source_revision: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(engine_version.as_bytes());
    hasher.update([0]);
    hasher.update(source_revision.as_bytes());
    hasher.update([0]);
    hasher.update(process_id.as_bytes());
    hasher.update([0]);
    hasher.update(serde_json::to_vec(raw_spec).unwrap_or_default());
    hasher.update([0]);
    hasher.update(
        serde_json::to_vec(instance.item.as_ref().unwrap_or(&serde_json::Value::Null))
            .unwrap_or_default(),
    );
    for binding in upstream {
        hasher.update([0]);
        hasher.update(binding.name.as_bytes());
        hasher.update([0]);
        hasher.update(binding.fingerprint.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Source;

    fn item_process() -> Process {
        Process::new(
            "qc",
            "fastqc",
            serde_json::json!({
                "input": "{{item}}",
                "threads": "{{item.threads}}",
                "meta": "{{item}}",
            }),
        )
    }

    #[test]
    fn instance_ids_round_trip() {
        let id = InstanceId::new("qc", Some("sample_A.fastq"));
        assert_eq!(id.as_str(), "qc#sample_A.fastq");
        assert_eq!(id.parts(), ("qc", Some("sample_A.fastq")));

        let singleton = InstanceId::new("multiqc", None);
        assert_eq!(singleton.parts(), ("multiqc", None));
    }

    #[test]
    fn string_items_derive_readable_keys() {
        assert_eq!(
            item_key(&serde_json::json!("sample_A.fastq")),
            "sample_A.fastq"
        );
        assert_eq!(item_key(&serde_json::json!("a b/c")), "a_b_c");
        assert_eq!(item_key(&serde_json::json!("")), "_");
    }

    #[test]
    fn scalar_items_derive_literal_keys_and_structured_items_hash() {
        assert_eq!(item_key(&serde_json::json!(42)), "42");
        assert_eq!(item_key(&serde_json::json!(true)), "true");
        assert_eq!(item_key(&serde_json::Value::Null), "null");

        let first = item_key(&serde_json::json!({"sample": "a", "n": 1}));
        let second = item_key(&serde_json::json!({"n": 1, "sample": "a"}));
        // Canonical JSON: field order must not change the derived key.
        assert_eq!(first, second);
        assert!(first.starts_with("hash-"));
        assert_eq!(first.len(), "hash-".len() + KEY_HASH_CHARS);

        let other = item_key(&serde_json::json!({"sample": "b"}));
        assert_ne!(first, other);
    }

    #[test]
    fn whole_token_preserves_type_and_embedded_token_stringifies() {
        let item = serde_json::json!({"threads": 4});
        let exact = render_string("{{item.threads}}", &item).unwrap();
        assert_eq!(exact, serde_json::json!(4));

        let embedded = render_string("--threads {{item.threads}}", &item).unwrap();
        assert_eq!(embedded, serde_json::json!("--threads 4"));
    }

    #[test]
    fn render_spec_walks_nested_spec_and_dotted_pointers() {
        let process = item_process();
        let item = serde_json::json!({"path": "a.fastq", "threads": 8});
        let instance = Instance::for_item(&process, &item).unwrap();
        // Whole-token `{{item}}` preserves the item's JSON type.
        assert_eq!(
            instance.rendered_spec["input"],
            serde_json::json!({"path": "a.fastq", "threads": 8})
        );
        assert_eq!(instance.rendered_spec["threads"], serde_json::json!(8));
        assert_eq!(
            instance.rendered_spec["meta"],
            serde_json::json!({"path": "a.fastq", "threads": 8})
        );
        // Structured items derive content-hash keys, stable across calls.
        assert_eq!(
            instance.item_key.as_deref(),
            Some(item_key(&serde_json::json!({"path": "a.fastq", "threads": 8})).as_str())
        );
    }

    #[test]
    fn missing_pointer_resolves_to_null_like_json_pointer() {
        let item = serde_json::json!({"a": 1});
        let rendered = render_string("{{item.absent}}", &item).unwrap();
        assert_eq!(rendered, serde_json::json!(null));
    }

    #[test]
    fn unsupported_tokens_and_unterminated_templates_are_errors() {
        let item = serde_json::json!({});
        assert!(render_string("{{other}}", &item).is_err());
        assert!(render_string("{{item", &item).is_err());
    }

    #[test]
    fn singleton_materialization_needs_no_items() {
        let process = item_process();
        assert!(Instance::singleton(&process).is_ok());
        assert!(Instance::for_item(&process, &serde_json::json!(1)).is_ok());
    }

    #[test]
    fn fingerprint_is_stable_and_sensitivity_is_scoped() {
        let process = item_process();
        let item = serde_json::json!("a.fastq");
        let instance = Instance::for_item(&process, &item).unwrap();
        let upstream = vec![UpstreamBinding {
            name: "reads".into(),
            fingerprint: "up-1".into(),
        }];

        let base = instance_fingerprint(&process, &instance, &upstream, "eng-1", "rev-1");
        // Deterministic across calls.
        assert_eq!(
            base,
            instance_fingerprint(&process, &instance, &upstream, "eng-1", "rev-1")
        );
        // Different item -> different instance.
        let other_item = Instance::for_item(&process, &serde_json::json!("b.fastq")).unwrap();
        assert_ne!(
            base,
            instance_fingerprint(&process, &other_item, &upstream, "eng-1", "rev-1")
        );
        // Upstream change cascades.
        let changed_upstream = vec![UpstreamBinding {
            name: "reads".into(),
            fingerprint: "up-2".into(),
        }];
        assert_ne!(
            base,
            instance_fingerprint(&process, &instance, &changed_upstream, "eng-1", "rev-1")
        );
        // Engine/source revision bumps invalidate everything.
        assert_ne!(
            base,
            instance_fingerprint(&process, &instance, &upstream, "eng-2", "rev-1")
        );
        // Spec template edit invalidates every instance of the process.
        let mut edited = process.clone();
        edited.spec["threads"] = serde_json::json!("{{item.threads}}!");
        assert_ne!(
            base,
            instance_fingerprint(&edited, &instance, &upstream, "eng-1", "rev-1")
        );
    }
}

# Evidence channel

`evidence` is a format contract on `FileRef` values: structured, auditable
information (first shape: bibliographic citations) flowing on DAG file edges
with wiring-time format gating, content-addressed fingerprints, and inline
rendering for the agent.

## Contract

A `FileRef` with `format == "evidence"` carries the UTF-8 JSON encoding of
`bib_types::evidence::EvidenceSet`:

```json
{
  "schema_version": 1,
  "records": [
    {
      "citation": { "title": "…", "authors": […], "identifiers": […], "year": 2024 },
      "note": "why this record is relevant",
      "origin": "pubmed"
    }
  ]
}
```

- `citation` is the canonical `bib_types::Article` (normalized by twelve
  crates and six retrieval backends).
- Writers always emit `schema_version: 1`; readers **fail closed** on newer
  versions rather than guessing at unknown semantics.
- Producers attach a `sha256:{hex}` content hash to the `FileRef`
  fingerprint, so downstream node fingerprints are content-addressed and
  incremental invalidation works when a source rewrites the same path.

## Nodes

| Kind | Ports | Behavior |
|---|---|---|
| `source_literature` | 0 in; 1 out `File(evidence)` | Fans a `StructuredSearch` out through the literature gateway (pubmed, arxiv, biorxiv, openalex, crossref, semantic_scholar), stamps `origin`/`note`, dedups, writes one artifact. Per-source failures warn and degrade; all-failed or an unknown source name fails the node. `limit` is per source (default 25, cap 200). |
| `evidence_merge` | variadic in `File(evidence)`; 1 out `File(evidence)` | Reads inputs in port order, merges, dedups. First occurrence wins for citation fields and `origin`; a missing `note` is filled from the first duplicate that has one. |
| `evidence_export` | 1 in `File(evidence)`; 1 out `File(bibtex\|ris\|markdown)` | Renders the citations as a bibliography; the output port's format contract follows the spec (`ports_for_spec`). |
| `literature_fulltext` | 1 in `File(evidence)`; 1 out `FileSet` | Resolves every record to its bibliography full-text file as a `FileSet` of VFS `FileRef`s — `get_output` shows the actual files (path / format `pdf\|txt\|html` / sha256 fingerprint). With `fetch_missing: true` (default), unmatched records are saved and their full text fetched from Europe PMC open access **as a stored VFS object** (same chain as `bib_save`); `false` makes the node a pure resolver over the existing library. |

## Channel division of labor

| Payload | Channel |
|---|---|
| Bibliography full text (PDF / HTML / extracted text) | **File channel** — content-addressed VFS objects at `vfs:///literature/{article_id}/{sha256}-{filename}` |
| Citation records (search / fetch / citation graph / recommendations) | **File channel** — `evidence` artifacts |
| Structured table data / database queries / node-to-node dataflow | **DataFrame channel** — `source_*` and transformation nodes |
| Human-facing audit trail (node input lines, per-node evidence) | `NodeRunDetails` + `InputBinding` (audit, not data) |

OA full texts are written as real VFS objects at fetch time (canonical
chain in `bib_base::oa_fetch::fetch_fulltext_stored`, used by both `bib_save`
and `literature_fulltext`); rows never point at synthetic `europepmc:`
pointers. Rows written by older versions carry such pointers with the text
inline — the `literature_fulltext` node materializes those on first
resolution.

## Dedup semantics

Two records are the same evidence when **any** identifier collides:

- DOI: normalized (URL prefixes stripped) and lowercased
- PMID / arXiv / S2 / OpenAlex / …: trimmed and lowercased
- No identifiers at all: fuzzy `cite_key` (first author + year + first
  title word)

Known limitation: the same paper arriving once PMID-only and once DOI-only
(with disjoint identifiers) is kept twice — resolving that requires a
cross-source lookup the merge node intentionally does not perform.

## Gating

- Declared ports (merge port 0, export input, all outputs): the scheduler's
  format gate rejects mismatched wiring at `add_edge` time
  (`PortFormatMismatch`).
- Variadic undeclared ports carry no wiring-time contract (dag-core
  semantics shared with `container_command`); `evidence_merge` rejects
  non-evidence values at run time instead.

## Path discipline

Spec-driven explicit `path` (`vfs://` URI or bare absolute), overwritten on
re-run, `sink_path()` declared. **Bare absolute paths are auto-normalized to
`vfs:///...` and routed through the mounted runtime VFS, so the engine and
the agent share one object-store namespace** — this is the only reliable
form for cross-process artifacts. An explicit `vfs://` URI on an engine
without a mounted VFS fails closed; bare absolute paths on such embedded
engines fall back to the host filesystem (test/embedded use only — the
production runtime always mounts the VFS). The engine does not detect
write-write collisions — give every evidence node its own path.

## Agent experience

`get_output` on an evidence output reads the artifact (4 MiB engine-side
cap, `DataEngineCmd::ReadFile`) and renders a compact citation list
(`cite`/`title`/`year`/`doi`/`origin`/`note`, capped at 50 records with
`total`/`returned`) instead of a bare path. Read or parse failures degrade
to a metadata entry carrying the error.

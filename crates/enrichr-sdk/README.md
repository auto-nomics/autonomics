# enrichr-sdk

Async Rust SDK for the [Enrichr](https://maayanlab.cloud/Enrichr/help#api)
gene-set enrichment API and its Speedrichr background-corrected companion.
The client layer has no DAG or agent-framework dependencies so either layer
can wrap it independently, mirroring `string-sdk`.

## Coverage

| SDK call | HTTP endpoint |
|---|---|
| `dataset_statistics`, `libraries` | `GET /datasetStatistics` |
| `add_list` | `POST /addList` (multipart) |
| `view` | `GET /view` |
| `enrich` | `GET /enrich` |
| `export` | `GET /export` (TSV) |
| `gene_set_library` | `GET /geneSetLibrary?mode=text` (GMT) |
| `genemap` | `GET /genemap` |
| `raw` | any documented text endpoint |
| `speedrichr_add_list` | `POST /api/addList` |
| `speedrichr_add_background` | `POST /api/addbackground` |
| `speedrichr_background_enrich` | `POST /api/backgroundenrich` |

## Quirks handled

- Enrichment rows arrive as positional JSON arrays, so `GeneSetTerm`
  implements `Deserialize` by hand with lenient number coercion.
- Speedrichr emits bare `Infinity` literals (not valid JSON) when a term's
  overlap with the background is complete; the client rewrites them to
  `±1e308` before parsing while leaving string contents untouched.
- An unknown `backgroundType` yields HTTP 200 with `{}`; `enrich` reports
  this as an `Api` error pointing at `libraries`.
- Error bodies are HTML pages; they are truncated before surfacing.

## Usage

```rust,no_run
use enrichr_sdk::EnrichrClient;

# async fn example() -> enrichr_sdk::Result<()> {
let client = EnrichrClient::new();
let added = client
    .add_list(["TP53", "BRCA1", "EGFR"], "my list")
    .await?;
let result = client
    .enrich(added.user_list_id, "KEGG_2021_Human")
    .await?;
# Ok(())
# }
```

Organism hosts (`FlyEnrichr`, `YeastEnrichr`, `WormEnrichr`, `FishEnrichr`)
are selected with `EnrichrClient::builder().host(EnrichrHost::Fly)` or
`with_host`. `ENDPOINT_ENRICHR_URL` / `ENDPOINT_SPEEDRICHR_URL` override the
defaults.

## Agent and DAG integration

`enrichr_registrations` exposes six agent tools:

- `enrichr_libraries`
- `enrichr_add_list`
- `enrichr_enrich`
- `enrichr_view_list`
- `enrichr_background_enrich`
- `enrichr_gene_map`

`nodes::Plugin` registers four DAG sources:

- `source_enrichr_enrich`
- `source_enrichr_libraries`
- `source_enrichr_view_list`
- `source_enrichr_genemap`

The plugin is registered by `nodes-io` in the default data-engine registry.

## Verification

```bash
cargo test -p enrichr-sdk                                    # offline
cargo test -p enrichr-sdk --test live -- --ignored           # network smoke
cargo test -p enrichr-sdk --test e2e -- --ignored            # tools + DAG nodes
```

# string-sdk

Async Rust SDK for the STRING protein association REST API. The crate has no
DAG or agent-framework dependencies so either layer can wrap it independently.

## Coverage

- Resolve gene, protein, synonym, and UniProt identifiers with
  `get_string_ids`.
- Fetch typed interaction, homology, annotation, enrichment, and PPI
  enrichment results.
- Render PNG or SVG network images for preview surfaces.
- Return native JSON, TSV, XML, PSI-MI, or PSI-MITAB payloads through `raw`.
- Request and use the asynchronous Values/Ranks enrichment job API.

## Authentication

Conventional STRING calls do not use an API key. Values/Ranks jobs are the
exception: call `get_api_key`, persist the returned key securely, and pass it
to submit/status/remove methods. STRING activates the free anonymous key within
about 30 minutes and cannot recover a lost key.

## Usage

```rust,no_run
use string_sdk::{StringDbClient, request::StringIdQuery};

# async fn example() -> string_sdk::Result<()> {
let client = StringDbClient::builder()
    .caller_identity("my-research-tool")
    .build()?;

let mappings = client
    .get_string_ids(&StringIdQuery::new(["TP53", "CDK2"]).species("9606"))
    .await?;
# Ok(())
# }
```

For reproducible integrations, call `version`, then rebuild the client with
the returned stable address instead of the floating default endpoint.

The client sends a polite caller identity and spaces public requests by one
second by default. Disable pacing only for local tests or a transport that
already enforces a stricter policy.

## Verification

Run offline checks with:

```bash
cargo test -p string-sdk
```

Run the small network smoke test explicitly with:

```bash
cargo test -p string-sdk --test live -- --ignored
```

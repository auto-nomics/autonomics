# biofusion

[Apache Arrow DataFusion][datafusion] integration for JSON and bioinformatics file formats, built on top of [`oxbow`][oxbow].

biofusion turns a JSON / VCF / BAM / CRAM / BigWig / … file into a queryable [`DataFrame`] using the same API DataFusion already uses for CSV and Parquet. Each oxbow-supported format is exposed as a typed reader on [`SessionContext`] via the [`DataFusionReadExt`] trait, so you can write SQL — or DataFrame operations — directly against genomic data.

> Only the **read** path is implemented. Writes return `NotImplemented`.

[`DataFrame`]: https://docs.rs/datafusion/latest/datafusion/dataframe/struct.DataFrame.html
[`SessionContext`]: https://docs.rs/datafusion/latest/datafusion/prelude/struct.SessionContext.html
[`DataFusionReadExt`]: https://docs.rs/biofusion/latest/biofusion/ext/trait.DataFusionReadExt.html
[datafusion]: https://github.com/apache/datafusion
[oxbow]: https://crates.io/crates/oxbow

## Supported formats

| Reader         | Format                  | Notes                                       |
|---------------|-------------------------|---------------------------------------------|
| `read_bio_json` | JSON, NDJSON          | Arrays and newline-delimited objects; gzip auto-detected |
| `read_vcf`    | VCF (`.vcf[.gz]`)       | BGZF and plain-gzip auto-detected           |
| `read_bcf`    | BCF                     |                                             |
| `read_fasta`  | FASTA                   |                                             |
| `read_fastq`  | FASTQ                   |                                             |
| `read_mtx`    | MatrixMarket (`.mtx`)   | sparse coordinate input as a long table      |
| `read_bed`    | BED                     |                                             |
| `read_gtf`    | GTF                     |                                             |
| `read_gff`    | GFF                     |                                             |
| `read_sam`    | SAM                     |                                             |
| `read_bam`    | BAM                     |                                             |
| `read_cram`   | CRAM                    |                                             |
| `read_bigwig` | BigWig                  | read-only via `bigtools`                    |
| `read_bigbed` | BigBed                  | read-only via `bigtools`                    |

Compression (gzip / BGZF) is auto-detected from the file's magic bytes — you don't need to tell the reader whether a `.vcf.gz` is BGZF or plain gzip.

`read_mtx` represents sparse coordinate data as `row`, `column`, and `value`
columns. Matrix Market coordinates are 1-based and are preserved as written.
Pattern entries produce `value = 1`; `general` matrices are supported.

## Quick start

Add biofusion to your `Cargo.toml`:

```toml
[dependencies]
biofusion = "0.1"
datafusion = "53"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

Then bring [`DataFusionReadExt`] into scope and read a file:

```rust
use biofusion::ext::DataFusionReadExt;
use biofusion::datasource::BioReadOptions;
use datafusion::prelude::SessionContext;

#[tokio::main]
async fn main() -> datafusion::common::Result<()> {
    let ctx = SessionContext::new();

    // Register a VCF as a table and query it with SQL.
    let df = ctx.read_vcf("sample.vcf.gz", BioReadOptions::default()).await?;
    ctx.register_table("variants", df.into_view())?;

    let high_qual = ctx
        .sql(r#"SELECT "chr", "pos", "ref", "alt" FROM variants WHERE "qual" > 100"#)
        .await?
        .collect()
        .await?;

    datafusion::arrow::util::pretty::print_batches(&high_qual)?;
    Ok(())
}
```

You can also drive everything through the DataFrame API instead of SQL:

```rust
let df = ctx.read_bam("reads.bam", BioReadOptions::default()).await?;
let filtered = df
    .select_columns(&["qname", "flag", "rname", "pos"])?
    .limit(0, Some(10))?;
filtered.show().await?;
```

## Read options

`BioReadOptions` is a concrete, builder-style options struct (mirroring DataFusion's `CsvReadOptions`) shared by every `read_<format>` helper:

```rust
use biofusion::datasource::BioReadOptions;

let options = BioReadOptions::new()
    .with_batch_size(4096)            // rows per RecordBatch (default 8192)
    .with_columns(vec!["chr".into(), "pos".into(), "alt".into()]) // projection pushdown
    .with_limit(1000)                 // row limit
    .with_file_extension("vcf.gz");   // override auto-detected extension
```

| Option             | Description                                                    |
|--------------------|----------------------------------------------------------------|
| `batch_size`       | Target rows per `RecordBatch` produced by the scan.            |
| `columns`          | Project only the named columns (pushed into the file scan).    |
| `limit`            | Cap the number of rows read.                                   |
| `file_extension`   | Override the auto-detected file extension (e.g. for odd names).|

## How it works

Every format shares **one** generic DataFusion `FileFormat` / `FileSource` / `FileOpener` stack (see `datasource::core`). A format only contributes a tiny [`BioDriver`] — a type-level strategy with two functions: schema inference and a batch scanner. oxbow does the actual format-specific decoding; biofusion handles projection pushdown, batch sizing, listing-table resolution, and decompression transparently.

Because the underlying scanners are not byte-range seekable, biofusion disables DataFusion's automatic file repartitioning — a single file is scanned as one partition.

[`BioDriver`]: https://docs.rs/biofusion/latest/biofusion/datasource/trait.BioDriver.html

## License

MIT.
